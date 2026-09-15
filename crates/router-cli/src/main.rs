//! The `router` command.
//!
//! # Design intent
//!
//! This is the interface the product is judged by. Someone runs one command,
//! reads the output, and decides whether this tool is any good. So the rules
//! the rest of the codebase follows matter most here:
//!
//! - **The answer is the most visible thing.** Values are emphasised; labels
//!   are not. The port, the workspace, and the URL are what a reader scans for.
//!
//! - **Every failure names a cause and a fix.** No bare errors.
//!
//! - **Nothing is printed that was not asked for.** No banner on every command.
//!
//! - **Output stays correct when piped.** Colour disappears; alignment does not.
//!
//! - **Exit codes mean something.** `0` succeeded; `1` failed; `2` is a usage
//!   error, so a script can tell a typo from a runtime problem.

use clap::{Parser, Subcommand};
use router_core::registry::{
    is_valid_instance_name, Instance, Registry, RegistryLock, DEFAULT_BASE_PORT,
};
use router_core::term::{self, ColourMode, Ink, Style, Verbosity};
use router_core::{allocate, validate_workspace, PortClaims, WorkspaceMode};
use router_dsh::{InstanceSpec, MultiConfig, MultiSupervisor};
use std::path::PathBuf;
use std::process::ExitCode;

mod control;
mod paths;

use paths::RouterHome;

/// Run several DeepSeek Harness instances on one machine.
#[derive(Parser, Debug)]
#[command(
    name = "router",
    version,
    about = "Run several DeepSeek Harness instances on one machine",
    long_about = "Run several DeepSeek Harness instances on one machine.\n\n\
                  Each instance gets its own port, its own workspace, its own model, and \
                  its own state directory, so they can be used in parallel without \
                  interfering with each other or with a harness you already run.",
    after_help = "EXAMPLES:\n  \
                  router init\n  \
                  router add api --workspace ~/projects/api\n  \
                  router add notes --workspace ~/notes --model deepseek-v4-flash\n  \
                  router list\n  \
                  router open api",
    disable_help_subcommand = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// When to colourise output: auto, always, never.
    #[arg(long, global = true, value_name = "WHEN")]
    colour: Option<String>,

    /// Print more detail.
    #[arg(long, short, global = true)]
    verbose: bool,

    /// Print only the result.
    #[arg(long, short, global = true, conflicts_with = "verbose")]
    quiet: bool,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Create the router home and an empty registry.
    Init {
        /// Override the router home location.
        #[arg(long, value_name = "DIR")]
        home: Option<PathBuf>,
    },

    /// Register and start a new instance.
    Add {
        /// A short name, used to address the instance.
        name: String,

        /// The project directory the agent works in.
        #[arg(long, short, value_name = "DIR")]
        workspace: PathBuf,

        /// The model this instance should start on.
        ///
        /// Either a bare model id, or `provider/model` to name the route.
        #[arg(long, short, value_name = "MODEL")]
        model: Option<String>,

        /// Share your existing harness credentials instead of a private copy.
        #[arg(long)]
        share_credentials: bool,

        /// Register without starting.
        #[arg(long)]
        no_start: bool,
    },

    /// Show every instance and its state.
    List {
        /// Probe each instance's port to confirm it is really serving.
        #[arg(long)]
        probe: bool,
    },

    /// Start a stopped instance.
    Start {
        /// Which instance.
        name: String,
    },

    /// Stop a running instance.
    Stop {
        /// Which instance. Omit it with --all to stop everything.
        ///
        /// Optional because `--all` is documented as stopping every instance,
        /// and a required argument made that form impossible to run: clap
        /// demanded a name, so the only accepted spelling was `stop <name>
        /// --all`, which then ignored the name it required.
        #[arg(required_unless_present = "all")]
        name: Option<String>,

        /// Stop every running instance.
        #[arg(long)]
        all: bool,
    },

    /// Restart an instance.
    Restart {
        /// Which instance.
        name: String,
    },

    /// Open an instance's UI in your browser.
    Open {
        /// Which instance.
        name: String,
    },

    /// Show an instance's captured output.
    Logs {
        /// Which instance.
        name: String,
    },

    /// Change an instance's model or credential sharing.
    ///
    /// # Why this exists
    ///
    /// Without it, `--model` and `--share-credentials` were one-way doors: the
    /// only way to change either was `router rm` followed by `router add`, and
    /// `rm` deliberately leaves the state directory on disk. Re-adding the same
    /// name then reuses that directory, so the "fix" silently carried the old
    /// settings forward — there was no way to actually undo a choice.
    Edit {
        /// Which instance.
        name: String,

        /// New model. Use `default` to clear the pinned model.
        #[arg(long, short)]
        model: Option<String>,

        /// Share the host installation's credentials with this instance.
        #[arg(long, conflicts_with = "no_share_credentials")]
        share_credentials: bool,

        /// Stop sharing: give the instance its own credentials file.
        #[arg(long)]
        no_share_credentials: bool,
    },

    /// Unregister an instance.
    ///
    /// Its workspace directory is never touched.
    Rm {
        /// Which instance.
        name: String,

        /// Do not ask for confirmation.
        #[arg(long, short)]
        yes: bool,
    },

    /// Summarise every instance; exits non-zero if any is down.
    Status,

    /// Check the environment and every instance. Changes nothing.
    Doctor,

    /// Serve a control page listing every instance.
    Serve {
        /// Port for the control page.
        #[arg(long, default_value_t = 3090)]
        port: u16,

        /// Do not open a browser.
        #[arg(long)]
        no_open: bool,
    },

    /// Print where the router keeps its files.
    Home,
}

/// A runtime failure.
const FAIL_EXIT: u8 = 1;
/// A usage error, so a script can tell a typo from a failure.
const USAGE_EXIT: u8 = 2;

fn main() -> ExitCode {
    let cli = Cli::parse();

    let verbosity = if cli.quiet {
        Verbosity::Quiet
    } else if cli.verbose {
        Verbosity::Verbose
    } else {
        Verbosity::Normal
    };
    let mode = ColourMode::from_env(cli.colour.as_deref());
    let style = Style::with_colour(mode, verbosity);

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            term::err(&style.error(
                &format!("cannot start the async runtime: {e}"),
                "This is unexpected. Please report it with the command you ran.",
            ));
            return ExitCode::from(FAIL_EXIT);
        }
    };

    match runtime.block_on(run(cli, &style)) {
        Ok(code) => code,
        Err(failure) => {
            term::err(&style.error(&failure.message, &failure.remedy));
            ExitCode::from(failure.exit)
        }
    }
}

/// A command failure, ready to print.
struct Failure {
    message: String,
    remedy: String,
    exit: u8,
}

impl Failure {
    fn runtime(message: impl Into<String>, remedy: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            remedy: remedy.into(),
            exit: FAIL_EXIT,
        }
    }

    fn usage(message: impl Into<String>, remedy: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            remedy: remedy.into(),
            exit: USAGE_EXIT,
        }
    }
}

/// Resolve the router home and load the registry.
fn load(home_override: Option<PathBuf>) -> Result<(RouterHome, Registry), Failure> {
    let home = RouterHome::resolve(home_override).map_err(|e| {
        Failure::runtime(
            e.detail.clone(),
            "Set DSH_ROUTER_HOME to a writable directory.",
        )
    })?;
    let registry = Registry::load(&home.registry_path()).map_err(|e| {
        Failure::runtime(
            e.detail.clone(),
            "Fix or remove the registry file, then try again.",
        )
    })?;
    Ok((home, registry))
}

/// How long to wait for the registry lock before giving up.
///
/// Generous, because the work it protects is a read, a small edit, and an
/// atomic write — milliseconds. The timeout exists to bound a *stale* lock left
/// by a killed process, not to accommodate slow work.
const LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Change the registry under an exclusive lock.
///
/// # Why every mutation goes through here
///
/// The registry is a whole-document read-modify-write, so two concurrent
/// commands that each load, edit and save will lose one of the edits — the last
/// rename wins and the other change disappears. Sixteen concurrent `add` calls
/// lost six registrations this way while every one of them reported success.
///
/// The lock is taken *before* the load, and the document is re-read inside it.
/// Reading first and locking second would not help: the edit would then be
/// applied to a snapshot that another process had already replaced.
///
/// `edit` returns `Err` to abandon the change, which is how a caller rejects a
/// duplicate name without writing anything.
fn update_registry<T>(
    home: &RouterHome,
    edit: impl FnOnce(&mut Registry) -> Result<T, Failure>,
) -> Result<T, Failure> {
    let path = home.registry_path();
    let _lock = RegistryLock::acquire(&path, LOCK_TIMEOUT).map_err(|e| {
        Failure::runtime(
            e.detail.clone(),
            "Another router command may be running. If not, delete the .lock file.",
        )
    })?;

    // Re-read inside the lock: the document on disk now is the current one.
    let mut registry = Registry::load(&path).map_err(|e| {
        Failure::runtime(
            e.detail.clone(),
            "Fix or remove the registry file, then try again.",
        )
    })?;

    let outcome = edit(&mut registry)?;
    registry.save(&path).map_err(|e| {
        Failure::runtime(e.detail.clone(), "Check that the router home is writable.")
    })?;
    Ok(outcome)
}

async fn run(cli: Cli, style: &Style) -> Result<ExitCode, Failure> {
    match cli.command {
        Command::Init { home } => cmd_init(style, home),
        Command::Add {
            name,
            workspace,
            model,
            share_credentials,
            no_start,
        } => cmd_add(style, name, workspace, model, share_credentials, no_start).await,
        Command::Edit {
            name,
            model,
            share_credentials,
            no_share_credentials,
        } => cmd_edit(style, &name, model, share_credentials, no_share_credentials).await,
        Command::List { probe } => cmd_list(style, probe),
        Command::Start { name } => cmd_start(style, &name).await,
        Command::Stop { name, all } => cmd_stop(style, name.as_deref(), all).await,
        Command::Restart { name } => cmd_restart(style, &name).await,
        Command::Open { name } => cmd_open(style, &name),
        Command::Logs { name } => cmd_logs(style, &name).await,
        Command::Rm { name, yes } => cmd_rm(style, &name, yes).await,
        Command::Status => cmd_status(style),
        Command::Doctor => cmd_doctor(style).await,
        Command::Serve { port, no_open } => control::serve(style, port, no_open).await,
        Command::Home => {
            let home = RouterHome::resolve(None).map_err(|e| {
                Failure::runtime(
                    e.detail.clone(),
                    "Set DSH_ROUTER_HOME to a writable directory.",
                )
            })?;
            term::out(&home.root().display().to_string());
            Ok(ExitCode::SUCCESS)
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// init
// ─────────────────────────────────────────────────────────────────────────

fn cmd_init(style: &Style, home_override: Option<PathBuf>) -> Result<ExitCode, Failure> {
    let home = RouterHome::resolve(home_override).map_err(|e| {
        Failure::runtime(
            e.detail.clone(),
            "Set DSH_ROUTER_HOME to a writable directory.",
        )
    })?;

    let registry_path = home.registry_path();
    let existed = registry_path.exists();

    // An existing registry is never overwritten.
    //
    // This was the most destructive bug in the tool. `init` unconditionally
    // saved a fresh empty registry, so running it on a home whose registry had
    // a typo — the natural next step after `list` reports one — replaced every
    // instance with `instances: {}` and printed "already initialised", exit 0.
    // The user lost every name, port and workspace with a success message.
    //
    // Two separate concerns, now separated: if the file parses, `init` has
    // nothing to do; if it does not, that is a problem to report, not a state
    // to reset. Deliberately no `--force`: wiping this file should require the
    // user to move or delete it themselves, having seen what is in it.
    if existed {
        match Registry::load(&registry_path) {
            Ok(existing) => {
                if style.verbosity != Verbosity::Quiet {
                    term::out(&style.info(&format!(
                        "Router home already initialised at {}",
                        style.strong(&home.root().display().to_string())
                    )));
                    term::out(&style.field("instances", &existing.instances.len().to_string()));
                }
                term::out(&home.root().display().to_string());
                return Ok(ExitCode::SUCCESS);
            }
            Err(e) => {
                // `Registry::load` already names the file in its message, so the
                // path is not repeated here. Saying it twice reads as two
                // different problems.
                return Err(Failure::runtime(
                    e.detail.clone(),
                    "Fix or move that file, then run `router init` again. \
                     It is left untouched so you can recover it.",
                ));
            }
        }
    }

    let registry = Registry::default();
    registry
        .save(&registry_path)
        .map_err(|e| Failure::runtime(e.detail.clone(), "Check that the directory is writable."))?;

    if style.verbosity == Verbosity::Quiet {
        term::out(&home.root().display().to_string());
        return Ok(ExitCode::SUCCESS);
    }

    term::out(&style.ok("Router home created"));
    term::out(&style.field("home", &home.root().display().to_string()));
    term::out(&style.field("registry", &registry_path.display().to_string()));
    term::out(&style.field("first port", &DEFAULT_BASE_PORT.to_string()));
    term::out("");
    term::out(&style.dim("  Add your first instance:"));
    term::out(&format!(
        "    router add my-project --workspace {}",
        style.paint(Ink::Blue, "~/projects/my-project")
    ));

    Ok(ExitCode::SUCCESS)
}

// ─────────────────────────────────────────────────────────────────────────
// add
// ─────────────────────────────────────────────────────────────────────────

async fn cmd_add(
    style: &Style,
    name: String,
    workspace: PathBuf,
    model: Option<String>,
    share_credentials: bool,
    no_start: bool,
) -> Result<ExitCode, Failure> {
    if !is_valid_instance_name(&name) {
        return Err(Failure::usage(
            format!("'{name}' is not a valid instance name"),
            "Use letters, digits, '-', '_' or '.', starting with a letter or digit.",
        ));
    }

    // The workspace must be a real directory: the harness refuses to register a
    // workspace over a path that does not exist, so discovering that here gives
    // a better message than discovering it three layers down. Done before the
    // lock, because it touches no shared state and is the slowest step.
    let validated = validate_workspace(&workspace.to_string_lossy(), WorkspaceMode::Existing)
        .map_err(|e| {
            Failure::usage(
                e.detail.clone(),
                e.remediation()
                    .unwrap_or("Choose an existing project directory."),
            )
        })?;

    let home = RouterHome::resolve(None).map_err(|e| {
        Failure::runtime(
            e.detail.clone(),
            "Set DSH_ROUTER_HOME to a writable directory.",
        )
    })?;

    // Everything that reads shared state happens inside the lock, and the
    // registry is re-read there.
    //
    // This is not only about losing writes. The checks themselves are
    // read-modify-write: the duplicate-name check, the duplicate-workspace
    // check, and port allocation all read the registry and then act on it. Two
    // concurrent adds could each see a name as free, each find a port free, and
    // each save — producing a lost registration and, in the worst case, two
    // instances on one port. Holding the lock across the whole decision is what
    // makes the decision true at the moment it is made.
    let (port, workspace_path) = update_registry(&home, |registry| {
        if registry.contains(&name) {
            return Err(Failure::usage(
                format!("an instance named '{name}' already exists"),
                format!("Choose another name, or remove it first: router rm {name}"),
            ));
        }

        // Two instances on one directory means two agents editing one tree. The
        // harness will not stop that, so the router does.
        if let Some((other, _)) = registry.find_by_workspace(validated.host()) {
            return Err(Failure::usage(
                format!(
                    "instance '{other}' already uses {}",
                    validated.host().display()
                ),
                "Two agents editing one project conflict. Use a different directory, \
                 or remove the other instance first.",
            ));
        }

        let claims = PortClaims::from_ports(registry.claimed_ports());
        let outcome = allocate(None, registry.base_port, &claims).map_err(|e| {
            Failure::runtime(
                e.detail.clone(),
                e.remediation().unwrap_or("Free a port and try again."),
            )
        })?;

        let mut instance = Instance::new(validated.host().to_path_buf(), outcome.port());
        instance.model = model.clone();
        instance.share_credentials = share_credentials;

        registry
            .insert(&name, instance)
            .map_err(|e| Failure::usage(e.detail.clone(), "Choose a different name."))?;

        Ok((outcome.port(), validated.host().to_path_buf()))
    })?;
    let _ = workspace_path;

    if style.verbosity != Verbosity::Quiet {
        term::out(&style.ok(&format!("Registered {}", style.strong(&name))));
        term::out(&style.field("workspace", &validated.host().display().to_string()));
        term::out(&style.field("port", &port.to_string()));
        if let Some(m) = &model {
            term::out(&style.field("model", m));
        }
        term::out(
            &style.field(
                "state",
                &Registry::state_root(home.root(), &name)
                    .display()
                    .to_string(),
            ),
        );
    }

    if no_start {
        if style.verbosity != Verbosity::Quiet {
            term::out("");
            term::out(&style.dim(&format!("  Start it with:  router start {name}")));
        }
        return Ok(ExitCode::SUCCESS);
    }

    // Re-read rather than reusing a snapshot: the registry was mutated inside
    // the lock, and starting from a stale document would look up a state root
    // for an instance recorded with different values.
    let (_home, registry) = load(None)?;
    start_one(style, &home, &registry, &name).await
}

// ─────────────────────────────────────────────────────────────────────────
// start / stop / restart
// ─────────────────────────────────────────────────────────────────────────

async fn cmd_start(style: &Style, name: &str) -> Result<ExitCode, Failure> {
    let (home, registry) = load(None)?;
    if !registry.contains(name) {
        return Err(unknown_instance(name, &registry));
    }
    start_one(style, &home, &registry, name).await
}

async fn start_one(
    style: &Style,
    home: &RouterHome,
    registry: &Registry,
    name: &str,
) -> Result<ExitCode, Failure> {
    let instance = registry
        .get(name)
        .ok_or_else(|| unknown_instance(name, registry))?;
    let spec = InstanceSpec::from_registry(name, instance, home.root());

    let supervisor = build_supervisor()?;
    supervisor.register(spec).await;

    if style.verbosity == Verbosity::Verbose {
        term::out(&style.dim(&format!(
            "  starting {name} on port {} with DSH_HOME={}",
            instance.port,
            Registry::state_root(home.root(), name).display()
        )));
    }

    match supervisor.start(name).await {
        Ok(report) => {
            let url = format!("http://127.0.0.1:{}", instance.port);
            if style.verbosity == Verbosity::Quiet {
                term::out(&url);
            } else {
                term::out(&style.ok(&format!("{} is ready", style.strong(name))));
                if report.settings_written {
                    term::out(&style.info(&format!(
                        "model set to {}",
                        instance.model.as_deref().unwrap_or("the default")
                    )));
                }
                term::out("");
                term::out(&format!(
                    "  {}  {}",
                    style.dim(style.glyphs.arrow),
                    style.url(&url)
                ));
            }
            Ok(ExitCode::SUCCESS)
        }
        Err(e) => {
            let failure = e.to_failure();
            let tail = supervisor.stderr_tail(name).await;
            let hint = if tail.is_empty() {
                format!("Run `router doctor`, then check: router logs {name}")
            } else {
                format!(
                    "Last output:\n{}",
                    tail.iter()
                        .rev()
                        .take(6)
                        .rev()
                        .map(|l| format!("    {l}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                )
            };
            Err(Failure::runtime(
                format!("{name} failed to start: {}", failure.code),
                format!("{}\n{hint}", failure.detail),
            ))
        }
    }
}

async fn cmd_stop(style: &Style, name: Option<&str>, all: bool) -> Result<ExitCode, Failure> {
    // Only the registry is needed: stopping works from the port, so nothing here
    // depends on the router home or on a supervisor that this process does not
    // own.
    let (_, registry) = load(None)?;

    // A name together with `--all` is contradictory. Previously the name was
    // required and then silently ignored, so `stop alpha --all` stopped
    // everything while naming one instance — the user could believe they had
    // stopped only `alpha`. Refusing is the honest answer.
    if all && name.is_some() {
        return Err(Failure::usage(
            "a name and --all cannot both be given".to_string(),
            "Use `router stop <name>` for one instance, or `router stop --all` for every one.",
        ));
    }

    if all {
        let mut failures = Vec::new();
        let mut stopped = 0usize;
        for n in registry.names() {
            let instance = registry.get(n).expect("name came from the registry");
            // By port, not by child handle: this process did not start the
            // harness. See `stop_instance`.
            match stop_instance(instance.port).await {
                Ok(()) => stopped += 1,
                Err(reason) => failures.push((n.to_string(), reason, instance.port)),
            }
        }
        for (n, reason, port) in &failures {
            term::err(&style.warn(&format!("{n} was not stopped")));
            for line in reason.lines(*port) {
                term::err(&style.dim(&format!("  {line}")));
            }
        }
        if failures.is_empty() {
            if style.verbosity != Verbosity::Quiet {
                term::out(&style.ok(&format!("Stopped {stopped} instance(s)")));
            } else {
                term::out(&stopped.to_string());
            }
            return Ok(ExitCode::SUCCESS);
        }
        return Err(Failure::runtime(
            format!("{} instance(s) could not be stopped", failures.len()),
            "Check for orphaned processes with `router doctor`.",
        ));
    }

    // `--all` is the only way to reach here without a name, and clap enforces
    // that, so this is unreachable in practice; it exists so the function does
    // not have to unwrap.
    let Some(name) = name else {
        return Err(Failure::usage(
            "no instance named".to_string(),
            "Give a name, or use --all to stop every instance.",
        ));
    };

    if !registry.contains(name) {
        return Err(unknown_instance(name, &registry));
    }
    let instance = registry.get(name).expect("checked above");

    if let Err(reason) = stop_instance(instance.port).await {
        // Reporting success here would be the worst outcome: the user stops
        // looking while the harness keeps the port, and the next start fails
        // for a reason nothing on screen explains.
        return Err(stop_failure(name, &reason, instance.port));
    }

    if style.verbosity != Verbosity::Quiet {
        term::out(&style.ok(&format!("Stopped {}", style.strong(name))));
        term::out(&style.dim(&format!(
            "  Port {} is remembered; `router start {name}` reclaims it.",
            instance.port
        )));
    }
    Ok(ExitCode::SUCCESS)
}

/// Stop whatever holds an instance's port.
///
/// # Why the port, and not a child handle
///
/// `stop` runs in a different process from `start`. The supervisor that spawned
/// the harness is in the other one, so this process has no child to signal — and
/// a supervisor asked to stop an instance it never started would do nothing and
/// report success. The harness's port is the one fact both processes agree on,
/// and holding it is the whole reason the harness is running.
///
/// Stop whatever holds an instance's port, if it is the harness we started.
///
/// # Why the port, and not a child handle
///
/// `stop` runs in a different process from `start`. The supervisor that spawned
/// the harness is in the other one, so this process has no child to signal — and
/// a supervisor asked to stop an instance it never started would do nothing and
/// report success. The harness's port is the one fact both processes agree on.
///
/// # Why the listener is identified first
///
/// Agreement on a port is not identity. An earlier revision killed whatever held
/// the port, and was confirmed to kill an unrelated process that happened to
/// bind it — while reporting success. The listener is now checked against the
/// harness invocation before anything is signalled.
///
/// Returns `Ok(())` when nothing is listening afterwards, which includes the
/// already-stopped case: "make it not run" is satisfied either way.
async fn stop_instance(port: u16) -> Result<(), StopFailure> {
    match router_dsh::process::stop_listener_on(port, std::time::Duration::from_secs(5)).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(StopFailure::StillListening { port }),
        Err(router_dsh::process::StopRefusal::NotOurs { pids, commands }) => {
            Err(StopFailure::Foreign { pids, commands })
        }
        Err(router_dsh::process::StopRefusal::Unknown) => Err(StopFailure::Unidentified),
    }
}

/// Why an instance could not be stopped.
///
/// A typed reason rather than a string, because callers must distinguish "our
/// harness would not die" from "something that is not ours holds the port". The
/// first must block a removal — it would orphan a running agent. The second must
/// not: nothing of ours is running, the registry entry is ours to delete, and
/// the foreign process is not ours to kill.
#[derive(Debug, Clone, PartialEq, Eq)]
enum StopFailure {
    /// The port is held by a process that is not our harness.
    Foreign {
        /// The offending process ids.
        pids: Vec<u32>,
        /// Their command lines, for display.
        commands: Vec<String>,
    },
    /// Something is still listening after an attempt to stop it.
    StillListening {
        /// The port that did not free.
        port: u16,
    },
    /// The owner of the port could not be determined.
    Unidentified,
}

impl StopFailure {
    /// Whether this is a foreign process rather than a stubborn harness.
    ///
    /// Decided by variant, not by matching on message text: a reworded message
    /// must not silently change when an instance can be removed.
    const fn is_foreign(&self) -> bool {
        matches!(self, Self::Foreign { .. })
    }

    /// A multi-line explanation for the user, referencing the port that failed.
    fn describe(&self, port: u16) -> String {
        match self {
            Self::Foreign { pids, commands } => {
                let who = commands
                    .iter()
                    .zip(pids.iter())
                    .map(|(c, p)| format!("pid {p}: {c}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                format!(
                    "port {port} is held by something that is not this router's harness:\n{who}"
                )
            }
            Self::StillListening { port } => {
                format!("something is still listening on port {port}")
            }
            Self::Unidentified => format!("cannot determine which process holds port {port}"),
        }
    }

    /// Every line of the explanation, for indented display.
    fn lines(&self, port: u16) -> Vec<String> {
        self.describe(port)
            .lines()
            .map(ToString::to_string)
            .collect()
    }
}

/// Turn a stop refusal into a failure the user can act on.
fn stop_failure(name: &str, reason: &StopFailure, port: u16) -> Failure {
    Failure::runtime(
        format!("{name} was not stopped"),
        format!(
            "{}\n  Nothing was killed. Find the owner with `router doctor`.",
            reason.describe(port)
        ),
    )
}

async fn cmd_restart(style: &Style, name: &str) -> Result<ExitCode, Failure> {
    let (home, registry) = load(None)?;
    if !registry.contains(name) {
        return Err(unknown_instance(name, &registry));
    }

    let supervisor = build_supervisor()?;
    let instance = registry.get(name).expect("checked above");
    supervisor
        .register(InstanceSpec::from_registry(name, instance, home.root()))
        .await;

    // Stopping an instance that is not running is not a failure.
    let _ = supervisor.stop(name).await;

    if style.verbosity != Verbosity::Quiet {
        term::out(&style.dim(&format!("  Restarting {name}…")));
    }
    start_one(style, &home, &registry, name).await
}

// ─────────────────────────────────────────────────────────────────────────
// list / status
// ─────────────────────────────────────────────────────────────────────────

fn cmd_list(style: &Style, probe: bool) -> Result<ExitCode, Failure> {
    let (_home, registry) = load(None)?;

    // `--quiet` emits one line per instance, tab-separated, with nothing else.
    // A table is for a person reading a terminal; a script wants fields it can
    // split without stripping colour and guessing at column widths.
    if style.verbosity == Verbosity::Quiet {
        for (name, instance) in &registry.instances {
            let serving = if probe {
                if probe_port(instance.port) {
                    "up"
                } else {
                    "down"
                }
            } else {
                "-"
            };
            term::out(&format!(
                "{name}\t{}\t{serving}\t{}\t{}",
                instance.port,
                instance.workspace.display(),
                instance.model.as_deref().unwrap_or("default"),
            ));
        }
        return Ok(ExitCode::SUCCESS);
    }

    if registry.instances.is_empty() {
        term::out(&style.dim("No instances yet."));
        term::out("");
        term::out(&format!(
            "  Add one:  router add my-project --workspace {}",
            style.paint(Ink::Blue, "~/projects/my-project")
        ));
        return Ok(ExitCode::SUCCESS);
    }

    let rows: Vec<Vec<String>> = registry
        .instances
        .iter()
        .map(|(name, instance)| {
            // Without --probe the marker reflects registration, and the legend
            // below says so, rather than implying a liveness check happened.
            let marker = if probe {
                if probe_port(instance.port) {
                    style.paint(Ink::Green, style.glyphs.running)
                } else {
                    style.paint(Ink::Amber, style.glyphs.stopped)
                }
            } else {
                style.paint(Ink::Dim, style.glyphs.bullet)
            };
            vec![
                marker,
                style.strong(name),
                style.paint(Ink::Blue, &instance.port.to_string()),
                compact_path(&instance.workspace),
                instance
                    .model
                    .clone()
                    .unwrap_or_else(|| style.dim("default")),
            ]
        })
        .collect();

    term::out("");
    // Only the two columns whose contents have no natural bound may shrink: a
    // workspace path can be arbitrarily deep and a model name is user-supplied.
    // The name, port, and status marker are short by construction and are left
    // at full width — truncating a port would be a lie, not a summary.
    term::out(&term::table_fitted(
        style,
        &["", "NAME", "PORT", "WORKSPACE", "MODEL"],
        &rows,
        &[(1, 26), (3, 46), (4, 28)],
    ));

    if !probe {
        term::out("");
        term::out(&style.dim(&format!(
            "  {} registered. Add --probe to confirm each one is serving.",
            style.glyphs.bullet
        )));
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_status(style: &Style) -> Result<ExitCode, Failure> {
    let (_home, registry) = load(None)?;

    // Count first, then print: the numbers are the answer and the prose is
    // commentary, so `--quiet` emits the numbers alone. A script that gates on
    // this command wants a value, not a sentence to parse.
    let mut up = 0usize;
    let mut down = 0usize;
    for instance in registry.instances.values() {
        if probe_port(instance.port) {
            up += 1;
        } else {
            down += 1;
        }
    }

    if style.verbosity == Verbosity::Quiet {
        term::out(&format!("{up} up {down} down"));
    } else if registry.instances.is_empty() {
        term::out(&style.dim("No instances registered."));
    } else {
        term::out(&format!(
            "{} up  {}  {} down",
            style.paint(Ink::Green, &up.to_string()),
            style.glyphs.sep,
            style.paint(
                if down > 0 { Ink::Amber } else { Ink::Dim },
                &down.to_string()
            )
        ));
    }

    // A non-zero exit when something is down, so a script can gate on it.
    if down > 0 {
        Ok(ExitCode::from(FAIL_EXIT))
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

// ─────────────────────────────────────────────────────────────────────────
// open / logs / rm
// ─────────────────────────────────────────────────────────────────────────

fn cmd_open(style: &Style, name: &str) -> Result<ExitCode, Failure> {
    let (_home, registry) = load(None)?;
    let instance = registry
        .get(name)
        .ok_or_else(|| unknown_instance(name, &registry))?;

    let url = format!("http://127.0.0.1:{}", instance.port);

    if !probe_port(instance.port) {
        return Err(Failure::runtime(
            format!("{name} is not serving on port {}", instance.port),
            format!("Start it first: router start {name}"),
        ));
    }

    match open_browser(&url) {
        Ok(()) => {
            if style.verbosity != Verbosity::Quiet {
                term::out(&style.ok(&format!("Opened {}", style.url(&url))));
            }
            Ok(ExitCode::SUCCESS)
        }
        Err(e) => {
            // A failed browser handoff is not fatal: the URL is the answer.
            term::err(&style.warn(&format!("could not open a browser: {e}")));
            term::out(&style.url(&url));
            Ok(ExitCode::SUCCESS)
        }
    }
}

async fn cmd_logs(style: &Style, name: &str) -> Result<ExitCode, Failure> {
    let (home, registry) = load(None)?;
    if !registry.contains(name) {
        return Err(unknown_instance(name, &registry));
    }

    let supervisor = build_supervisor()?;
    let instance = registry.get(name).expect("checked above");
    supervisor
        .register(InstanceSpec::from_registry(name, instance, home.root()))
        .await;

    let tail = supervisor.stderr_tail(name).await;
    if tail.is_empty() {
        term::out(&style.dim(&format!("No output captured for {name} in this process.")));
        term::out(&style.dim(&format!(
            "  Its state lives at {}",
            Registry::state_root(home.root(), name).display()
        )));
        return Ok(ExitCode::SUCCESS);
    }
    for line in tail {
        term::out(&line);
    }
    Ok(ExitCode::SUCCESS)
}

async fn cmd_rm(style: &Style, name: &str, yes: bool) -> Result<ExitCode, Failure> {
    let (home, registry) = load(None)?;
    let instance = registry
        .get(name)
        .cloned()
        .ok_or_else(|| unknown_instance(name, &registry))?;

    // An instance that is serving must be stopped before it is forgotten.
    //
    // Removing the registry entry alone left the harness running with nothing
    // pointing at it: an agent still holding the port, still holding its
    // credentials, and unreachable through `router` — no name to stop, no name
    // to show in `list`. The user believed it was gone.
    let serving = probe_port(instance.port);

    if !yes {
        // Say plainly what will and will not happen. The reassurance is the
        // important half: people hesitate before a destructive-looking command,
        // and rightly so.
        term::out(&format!("  Remove instance {}?", style.strong(name)));
        if serving {
            term::out(&style.warn(&format!(
                "    It is SERVING on port {} and will be stopped first.",
                instance.port
            )));
        }
        term::out(&style.dim(&format!(
            "    Its workspace {} is NOT touched.",
            instance.workspace.display()
        )));
        term::out(&style.dim(&format!(
            "    Its state at {} is NOT deleted.",
            Registry::state_root(home.root(), name).display()
        )));
        term::out("");
        term::out(&style.dim("  Re-run with --yes to confirm."));
        // Not a success: nothing was removed. Exiting 0 would let a script
        // treat "printed a question" as "did the work".
        return Err(Failure::runtime(
            format!("{name} was not removed"),
            "Confirm with: router rm ".to_string() + name + " --yes",
        ));
    }

    // Stop it first, and refuse to proceed if one of *our* harnesses will not
    // stop. Forgetting a running instance is worse than failing: the process
    // outlives the record of it.
    //
    // A port held by something that is **not** our harness is a different case,
    // and deliberately does not block the removal. Nothing of ours is running,
    // so there is no orphan to create; the registry entry is ours to delete; and
    // the foreign process is not ours to kill. Blocking here would trap the user
    // with an instance they cannot remove because an unrelated program happens
    // to hold its port. The collision is reported, because it explains why the
    // port may be unusable later.
    if serving {
        match stop_instance(instance.port).await {
            Ok(()) => {
                if style.verbosity != Verbosity::Quiet {
                    term::out(&style.dim(&format!("  Stopped {name} on port {}.", instance.port)));
                }
            }
            Err(ref reason) if reason.is_foreign() => {
                term::err(&style.warn(&format!(
                    "  port {} is not held by this router",
                    instance.port
                )));
                for line in reason.lines(instance.port) {
                    term::err(&style.dim(&format!("    {line}")));
                }
                term::err(
                    &style.dim("    Removing the instance anyway: nothing of ours is running."),
                );
            }
            Err(ref reason) => return Err(stop_failure(name, reason, instance.port)),
        }
    }

    // The removal itself is a read-modify-write like any other, so it takes the
    // lock. Without it, a concurrent `add` could be lost: `rm` would load the
    // registry, delete its entry, and save a document written before the
    // addition landed.
    update_registry(&home, |registry| {
        if registry.remove(name).is_none() {
            return Err(unknown_instance(name, registry));
        }
        Ok(())
    })?;

    if style.verbosity != Verbosity::Quiet {
        term::out(&style.ok(&format!("Removed {}", style.strong(name))));
        term::out(&style.dim(&format!(
            "  Workspace left untouched at {}",
            instance.workspace.display()
        )));
        term::out(&style.dim(&format!(
            "  State left on disk at {}",
            Registry::state_root(home.root(), name).display()
        )));
    }
    Ok(ExitCode::SUCCESS)
}

// ─────────────────────────────────────────────────────────────────────────
// doctor
// ─────────────────────────────────────────────────────────────────────────

async fn cmd_doctor(style: &Style) -> Result<ExitCode, Failure> {
    let quiet = style.verbosity == Verbosity::Quiet;

    // `--quiet` prints the report and drops only the decoration. The first
    // attempt at this suppressed every line, so `doctor --quiet` printed nothing
    // and exited 0 — which reads as "all clear" and was the worst possible
    // outcome for a diagnostic. The report itself is the useful part; the
    // heading and the closing banner are not.
    let say = |line: &str| term::out(line);

    if !quiet {
        term::out(&style.heading("DeepSeek Harness Router — diagnostics"));
        term::out("");
    }

    let mut problems = 0usize;

    match RouterHome::resolve(None) {
        Ok(home) => {
            if home.registry_path().exists() {
                say(&style.ok(&format!("home          {}", home.root().display())));
            } else {
                say(&style.warn(&format!(
                    "home          {} (not initialised)",
                    home.root().display()
                )));
                say(&style.dim("              run `router init`"));
                problems += 1;
            }
        }
        Err(e) => {
            say(&style.warn(&format!("home          unavailable: {}", e.detail)));
            problems += 1;
        }
    }

    let binary = std::env::var("DSH_BINARY").unwrap_or_else(|_| "dsh".to_string());
    match which(&binary) {
        Some(path) => {
            let version = harness_version(&path).await;
            say(&style.ok(&format!(
                "harness       {}{}",
                path.display(),
                version.map_or(String::new(), |v| format!("  ({v})"))
            )));
        }
        None => {
            say(&style.warn(&format!("harness       '{binary}' not found on PATH")));
            say(&style.dim("              install DeepSeek Harness, or set DSH_BINARY"));
            problems += 1;
        }
    }

    if probe_port(3080) {
        say(&style.info("port 3080     in use (expected — the router starts at 3081)"));
    } else {
        say(&style.info("port 3080     free"));
    }

    let (_home, registry) = load(None)?;
    if registry.instances.is_empty() {
        say(&style.info("instances     none registered"));
    } else {
        say("");
        say(&style.strong("  Instances"));
        for (name, instance) in &registry.instances {
            let live = probe_port(instance.port);
            let workspace_ok = instance.workspace.is_dir();
            let marker = if live {
                style.paint(Ink::Green, style.glyphs.running)
            } else {
                style.paint(Ink::Dim, style.glyphs.stopped)
            };
            let state = if live { "serving" } else { "not serving" };
            let state_text = if workspace_ok {
                style.dim(state)
            } else {
                style.paint(Ink::Amber, &format!("{state} — workspace missing"))
            };
            say(&format!(
                "    {marker} {}  :{}  {state_text}",
                term::pad(name, 16),
                instance.port
            ));
            if !workspace_ok {
                problems += 1;
            }
        }
    }

    say("");
    if problems == 0 {
        say(&style.ok("Everything checks out"));
        Ok(ExitCode::SUCCESS)
    } else {
        say(&style.warn(&format!("{problems} thing(s) need attention")));
        Ok(ExitCode::from(FAIL_EXIT))
    }
}

// ─────────────────────────────────────────────────────────────────────────
// helpers
// ─────────────────────────────────────────────────────────────────────────

/// Build a supervisor, resolving the harness the same way `doctor` does.
///
/// # Why this resolves rather than passing the name through
///
/// On Windows the installed harness is a `.cmd` shim. `Command::new("dsh")` does
/// not find it: process spawning does not consult `PATHEXT`, so only an exact
/// filename or an executable works. The bare name would therefore start nothing,
/// and the failure surfaced as `DSH_NOT_INSTALLED` — "program not found" — while
/// `doctor` happily reported the harness present. Two commands disagreeing about
/// whether the product is installed is worse than either answer alone.
///
/// `DSH_BINARY` still wins when set, so an explicit choice is never overridden.
/// Resolution is fallible on purpose: a missing harness is a real error with a
/// real remedy, and it is raised where the user asked for something that needs
/// one, rather than deferred to a spawn failure that reads as a mystery.
fn resolve_harness() -> Result<PathBuf, Failure> {
    let requested = std::env::var("DSH_BINARY").unwrap_or_else(|_| "dsh".to_string());
    which(&requested).ok_or_else(|| {
        Failure::runtime(
            format!("cannot find the harness executable '{requested}'"),
            "Install it, or set DSH_BINARY to its full path. \
             Run `router doctor` to see what is detected.",
        )
    })
}

fn build_supervisor() -> Result<MultiSupervisor, Failure> {
    // The host credential path must be derived here, or `--share-credentials`
    // silently does nothing: provisioning simply sees no host file and writes a
    // private empty one. The OS home comes from the same environment the harness
    // uses, so a relocated harness still resolves correctly.
    let os_home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from);

    Ok(MultiSupervisor::new(
        MultiConfig {
            binary: resolve_harness()?,
            ..MultiConfig::default()
        }
        .with_host_credentials(os_home.as_deref()),
    ))
}

async fn cmd_edit(
    style: &Style,
    name: &str,
    model: Option<String>,
    share_credentials: bool,
    no_share_credentials: bool,
) -> Result<ExitCode, Failure> {
    let (home, registry) = load(None)?;
    let instance = registry
        .get(name)
        .cloned()
        .ok_or_else(|| unknown_instance(name, &registry))?;

    if model.is_none() && !share_credentials && !no_share_credentials {
        return Err(Failure::usage(
            "nothing to change".to_string(),
            format!(
                "Give at least one of: --model <MODEL>, --share-credentials, \
                 --no-share-credentials. Or see `router edit {name} --help`."
            ),
        ));
    }

    // A running instance holds its settings in memory: the harness reads
    // `settings.yaml` at boot and the model default is process-wide, so editing
    // the files underneath it would change nothing until it restarts — while
    // the registry claimed the new value immediately. Refusing is clearer than
    // recording a change that has not taken effect.
    if probe_port(instance.port) {
        return Err(Failure::runtime(
            format!("{name} is serving on port {}", instance.port),
            format!("Stop it first: router stop {name}"),
        ));
    }

    // `default` is how a user clears a pinned model. Clearing has to be
    // spellable, or the original choice remains a one-way door.
    let clear_model = model
        .as_deref()
        .is_some_and(|m| m.eq_ignore_ascii_case("default"));
    if let Some(m) = &model {
        if m.is_empty() {
            return Err(Failure::usage(
                "the model name is empty".to_string(),
                format!("Use `router edit {name} --model default` to clear it."),
            ));
        }
    }

    let state_root = Registry::state_root(home.root(), name);
    let os_home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from);
    let host_credentials = host_credential_path(os_home.as_deref());

    // Credentials are reconciled on disk *before* the registry is written.
    // If the file cannot be changed, the registry must not claim it was.
    let mut share_outcome = None;
    if share_credentials || no_share_credentials {
        let outcome = router_dsh::set_credential_sharing(
            &state_root,
            share_credentials,
            host_credentials.as_deref(),
        )
        .map_err(|e| {
            Failure::runtime(e.detail.clone(), "Check that the state root is writable.")
        })?;
        share_outcome = Some(outcome);
    }

    let new_model = if clear_model {
        None
    } else if let Some(m) = &model {
        Some(m.clone())
    } else {
        instance.model.clone()
    };
    let new_share = if share_credentials {
        true
    } else if no_share_credentials {
        false
    } else {
        instance.share_credentials
    };

    update_registry(&home, |registry| {
        // Checked before taking the mutable borrow: `unknown_instance` reads the
        // whole registry to list the names it knows, which cannot happen while
        // an entry is mutably borrowed.
        if !registry.contains(name) {
            return Err(unknown_instance(name, registry));
        }
        let entry = registry.get_mut(name).expect("checked immediately above");
        entry.model = new_model.clone();
        entry.share_credentials = new_share;
        Ok(())
    })?;

    if style.verbosity != Verbosity::Quiet {
        term::out(&style.ok(&format!("Updated {}", style.strong(name))));
        match (&model, clear_model) {
            (Some(_), true) => term::out(&style.field("model", "cleared (harness default)")),
            (Some(m), false) => term::out(&style.field("model", m)),
            (None, _) => term::out(&style.field("model", "unchanged")),
        }

        match &share_outcome {
            Some(router_dsh::ShareOutcome::Shared) => {
                term::out(&style.field("credentials", "shared with the host installation"));
            }
            Some(router_dsh::ShareOutcome::Private) => {
                term::out(&style.field("credentials", "private to this instance"));
            }
            Some(router_dsh::ShareOutcome::Unavailable(why)) => {
                // The registry still records the request, but the user must know
                // the instance is not actually sharing — otherwise the next
                // thing they see is a harness that cannot authenticate, with
                // nothing pointing at why.
                term::err(&style.warn(&format!("  credentials were NOT shared: {why}")));
                term::err(&style.dim(
                    "    The instance will use its own file. On Windows a symlink \
                     needs Developer Mode or an elevated shell.",
                ));
            }
            None => {}
        }

        term::out("");
        term::out(&style.dim(&format!(
            "  Changes apply when it next starts:  router start {name}"
        )));
    }

    Ok(ExitCode::SUCCESS)
}

/// Where the host installation keeps its credentials.
///
/// `DSH_HOME` wins when set, so a relocated harness is still found; otherwise
/// the harness default under the OS home.
fn host_credential_path(os_home: Option<&std::path::Path>) -> Option<PathBuf> {
    if let Some(dsh_home) = std::env::var_os("DSH_HOME") {
        return Some(PathBuf::from(dsh_home).join(".credentials.yaml"));
    }
    os_home.map(|h| h.join(".dsh").join(".credentials.yaml"))
}

fn unknown_instance(name: &str, registry: &Registry) -> Failure {
    let known = registry.names();
    let hint = if known.is_empty() {
        "No instances are registered yet. Add one with: router add <name> --workspace <dir>"
            .to_string()
    } else {
        format!("Registered instances: {}", known.join(", "))
    };
    Failure::usage(format!("no instance named '{name}'"), hint)
}

/// Whether something is listening on a loopback port.
///
/// A connection test, not a listing: the only reliable answer to "is it
/// serving" is to ask it.
fn probe_port(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        std::time::Duration::from_millis(300),
    )
    .is_ok()
}

/// Shorten a path for display, without lying about where it is.
///
/// Only the home prefix is abbreviated, and only to `~`. A path shortened
/// further would be a path the reader cannot act on.
fn compact_path(path: &std::path::Path) -> String {
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        let home = PathBuf::from(home);
        if let Ok(rest) = path.strip_prefix(&home) {
            return format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display());
        }
    }
    path.display().to_string()
}

/// Locate an executable on PATH.
fn which(name: &str) -> Option<PathBuf> {
    let candidate = PathBuf::from(name);
    if candidate.is_absolute() && candidate.is_file() {
        return Some(candidate);
    }

    let path = std::env::var_os("PATH")?;
    let extensions: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT".to_string())
            .split(';')
            .map(str::to_string)
            .collect()
    } else {
        vec![String::new()]
    };

    for dir in std::env::split_paths(&path) {
        for ext in &extensions {
            let candidate = dir.join(format!("{name}{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Read the harness version, best-effort.
async fn harness_version(binary: &std::path::Path) -> Option<String> {
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::process::Command::new(binary)
            .arg("--version")
            .output(),
    )
    .await
    .ok()?
    .ok()?;

    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Open a URL in the platform's default browser.
fn open_browser(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        // `start` is a shell builtin, so it needs a shell. The empty first
        // argument is the window title; without it a quoted URL would be
        // consumed as the title.
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()?;
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).spawn()?;
        Ok(())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(url).spawn()?;
        Ok(())
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = url;
        Err(std::io::Error::other("unsupported platform"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_path_abbreviates_only_the_home_prefix() {
        let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"));
        if let Some(home) = home {
            let full = PathBuf::from(&home).join("projects").join("demo");
            let short = compact_path(&full);
            assert!(short.starts_with('~'), "got {short}");
            assert!(short.contains("demo"), "the leaf must survive");
        }
    }

    #[test]
    fn unrelated_paths_are_shown_in_full() {
        // Abbreviating a path we do not own would be a small lie.
        let p = std::path::Path::new("/somewhere/else/entirely");
        assert_eq!(compact_path(p), p.display().to_string());
    }

    #[test]
    fn unknown_instance_lists_what_exists() {
        let mut registry = Registry::default();
        registry
            .insert("alpha", Instance::new(PathBuf::from("/tmp/a"), 3081))
            .unwrap();
        let f = unknown_instance("beta", &registry);
        assert!(f.message.contains("beta"));
        assert!(f.remedy.contains("alpha"));
    }

    #[test]
    fn unknown_instance_with_an_empty_registry_teaches_the_command() {
        let f = unknown_instance("beta", &Registry::default());
        assert!(f.remedy.contains("router add"));
    }

    #[test]
    fn a_mistyped_name_is_a_usage_error_not_a_failure() {
        // Exit 2 lets a script tell a typo from a runtime problem.
        assert_eq!(unknown_instance("x", &Registry::default()).exit, USAGE_EXIT);
        assert_ne!(unknown_instance("x", &Registry::default()).exit, FAIL_EXIT);
    }

    #[test]
    fn probe_reports_a_listening_port() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(probe_port(port), "a bound port must probe as serving");
    }

    #[test]
    fn probe_reports_nothing_on_an_unused_port() {
        // Port 1 is reserved and refuses connections.
        assert!(!probe_port(1));
    }
}
