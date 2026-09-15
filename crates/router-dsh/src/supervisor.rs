//! Process supervision: spawn the harness, detect readiness, keep it alive.
//!
//! The supervisor owns exactly one child. It never sleeps to decide readiness —
//! it watches for the harness's own announcement **and** confirms the socket
//! answers, because the announcement proves intent while the probe proves
//! reachability. A sleep would be neither.

use crate::readiness::{classify_line, is_fatal, push_bounded, OutputSignal, DEFAULT_TAIL_LINES};
use crate::state::{RuntimeFailure, RuntimeState, RuntimeStatus};
use router_core::Config;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{watch, Mutex};

/// How the supervisor should behave.
#[derive(Debug, Clone)]
pub struct SupervisorConfig {
    /// Path to the `dsh` executable.
    pub binary: std::path::PathBuf,
    /// Loopback port the harness should listen on.
    pub internal_port: u16,
    /// Working directory for the child (the workspace).
    pub workspace: std::path::PathBuf,
    /// Harness home directory.
    pub dsh_home: std::path::PathBuf,
    /// How long to wait for readiness.
    pub ready_timeout: Duration,
    /// Extra authorities to accept.
    pub trusted_hosts: Vec<String>,
}

impl SupervisorConfig {
    /// Derive supervisor settings from the process configuration.
    #[must_use]
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            binary: cfg.dsh_binary.clone(),
            internal_port: cfg.dsh_internal_port,
            workspace: cfg.workspace_path.clone(),
            dsh_home: cfg.dsh_home.clone(),
            ready_timeout: Duration::from_secs(cfg.ready_timeout_secs),
            trusted_hosts: cfg.trusted_hosts.clone(),
        }
    }

    /// The command line the supervisor will run.
    ///
    /// Exposed so tests and the doctor command can assert the exact arguments
    /// rather than guessing, and so no caller builds an argv by string
    /// concatenation, so a hostile argument cannot become an extra flag.
    ///
    /// # Why `--profile web` and not the `web` alias
    ///
    /// The harness accepts `dsh web …` as a convenience alias, and it works.
    /// But the harness's own help documents the interface as
    /// `dsh --profile web [options]`, and lists only that form under Examples.
    /// An alias is a convenience that can be withdrawn or re-pointed; the
    /// documented profile selector is the contract. This project pins to
    /// documented interfaces so an upstream release is a tested upgrade rather
    /// than a surprise, so the documented form is what we emit.
    #[must_use]
    pub fn command_argv(&self) -> Vec<String> {
        let mut argv = vec![
            self.binary.to_string_lossy().to_string(),
            "--profile".to_string(),
            "web".to_string(),
            "--host".to_string(),
            "127.0.0.1".to_string(),
            "--port".to_string(),
            self.internal_port.to_string(),
            // The host launcher owns the browser handoff; a container must never
            // try to open one; the router owns any browser handoff.
            "--no-open".to_string(),
        ];
        for host in &self.trusted_hosts {
            argv.push("--trusted-host".to_string());
            argv.push(host.clone());
        }
        argv
    }
}

/// How long to keep listening for the announced browser URL after the port
/// answers.
///
/// The harness binds its socket slightly before printing the line that carries
/// the authentication token — measured at roughly 0.7s apart. Returning the
/// moment the port answers discards the token, producing an instance that runs
/// but cannot be opened in a browser.
const URL_GRACE_MS: u64 = 3000;

/// Why a start attempt failed.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    /// The executable could not be launched.
    #[error("cannot launch the harness: {0}")]
    Spawn(String),
    /// The harness exited before becoming ready.
    #[error("the harness exited before becoming ready (code {code:?})")]
    ExitedEarly {
        /// Exit code, when the process reported one.
        code: Option<i32>,
        /// The captured stderr tail, which usually explains why.
        tail: Vec<String>,
    },
    /// The harness never announced readiness within the budget.
    #[error("the harness did not become ready within {0:?}")]
    Timeout(Duration, Vec<String>),
    /// The harness announced readiness but the socket did not answer.
    #[error("the harness announced readiness but {0} did not respond")]
    Unreachable(String),
    /// Something else was already answering on the instance's port.
    ///
    /// Distinct from [`Self::Timeout`]: the port is not silent, it is occupied
    /// by a process this supervisor did not start and cannot vouch for.
    #[error("{detail}")]
    PortInUse {
        /// The port that was already answering.
        port: u16,
        /// A human-readable description, including the owner when it is known.
        detail: String,
    },
}

impl StartError {
    /// The stable code for this failure.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Spawn(_) => "DSH_NOT_INSTALLED",
            Self::ExitedEarly { .. } => "DSH_EXITED",
            Self::Timeout(..) => "DSH_READY_TIMEOUT",
            Self::Unreachable(_) => "RELAY_UPSTREAM_UNREACHABLE",
            Self::PortInUse { .. } => "DSH_PORT_IN_USE",
        }
    }

    /// Convert into the health model's failure shape.
    #[must_use]
    pub fn to_failure(&self) -> RuntimeFailure {
        match self {
            Self::Spawn(d) => RuntimeFailure::new(self.code(), d.clone()),
            Self::ExitedEarly { code, tail } => {
                let mut f = RuntimeFailure::new(self.code(), "the harness stopped during startup");
                if let Some(c) = code {
                    f = f.with_exit_code(*c);
                }
                f.with_stderr_tail(tail.clone())
            }
            Self::Timeout(d, tail) => {
                RuntimeFailure::new(self.code(), format!("no readiness within {}s", d.as_secs()))
                    .with_stderr_tail(tail.clone())
            }
            Self::Unreachable(u) => {
                RuntimeFailure::new(self.code(), format!("{u} did not respond"))
            }
            Self::PortInUse { detail, .. } => RuntimeFailure::new(self.code(), detail.clone()),
        }
    }
}

/// Supervises one harness process.
pub struct Supervisor {
    config: SupervisorConfig,
    inner: Arc<Mutex<Inner>>,
    status_tx: watch::Sender<RuntimeStatus>,
}

struct Inner {
    child: Option<Child>,
    status: RuntimeStatus,
    stderr_tail: Vec<String>,
    started_at: Option<Instant>,
}

impl Supervisor {
    /// Create a supervisor. Nothing is spawned until [`Supervisor::start`].
    #[must_use]
    pub fn new(config: SupervisorConfig) -> Self {
        let port = config.internal_port;
        let (status_tx, _) = watch::channel(RuntimeStatus::new(RuntimeState::Absent, port));
        Self {
            config,
            inner: Arc::new(Mutex::new(Inner {
                child: None,
                status: RuntimeStatus::new(RuntimeState::Absent, port),
                stderr_tail: Vec::new(),
                started_at: None,
            })),
            status_tx,
        }
    }

    /// The current status.
    pub async fn status(&self) -> RuntimeStatus {
        self.inner.lock().await.status.clone()
    }

    /// Subscribe to status changes.
    #[must_use]
    pub fn watch(&self) -> watch::Receiver<RuntimeStatus> {
        self.status_tx.subscribe()
    }

    /// The exact command line used to launch the harness.
    #[must_use]
    pub fn command_argv(&self) -> Vec<String> {
        self.config.command_argv()
    }

    async fn set_state(&self, state: RuntimeState) {
        let mut inner = self.inner.lock().await;
        inner.status.state = state;
        let snapshot = inner.status.clone();
        drop(inner);
        let _ = self.status_tx.send(snapshot);
    }

    /// Record the authenticated URL the harness announced.
    ///
    /// `None` clears it, which is what stopping must do: the token belongs to a
    /// process, and offering a URL for a process that has exited invites the
    /// user to open a page that cannot work — a failure that would look like a
    /// bug in the router rather than what it is, a stale link.
    async fn set_auth_url(&self, url: Option<String>) {
        let mut inner = self.inner.lock().await;
        inner.status.auth_url = url;
        let snapshot = inner.status.clone();
        drop(inner);
        let _ = self.status_tx.send(snapshot);
    }

    /// Spawn the harness and wait until it is genuinely reachable.
    ///
    /// # Errors
    ///
    /// Returns [`StartError`] when the binary cannot be launched, the process
    /// exits before readiness, or readiness is not confirmed within the budget.
    pub async fn start(&self) -> Result<(), StartError> {
        {
            let mut inner = self.inner.lock().await;
            if inner.status.state.is_usable() {
                return Ok(());
            }
            inner.status.restarts += 1;
            inner.stderr_tail.clear();
            inner.started_at = Some(Instant::now());
        }

        // Refuse to start when something is already answering on the port.
        //
        // # Why this check has to come first
        //
        // Without it, the readiness loop below sees a port that answers, calls
        // it ready, and reports success — attributing the *other* process's
        // liveness to this instance. The router then believes it is running an
        // instance it never started, while its own child boots into
        // `EADDRINUSE` and dies. The user is told "ready" about a harness the
        // router does not own, cannot stop, and whose sessions are not the ones
        // they think they are talking to.
        //
        // This is reachable in normal use: a harness started outside the router
        // on an instance port, a leftover from a previous run, or — the case
        // that exposed it — a supervisor script restarting a harness on a port
        // the router also uses.
        //
        // Checked before spawning, because a refusal that leaves no process
        // behind is far easier to reason about than one that has to clean up.
        if probe_once(&format!("http://127.0.0.1:{}", self.config.internal_port)).await {
            let port = self.config.internal_port;
            let detail = crate::process::describe_listener(port).unwrap_or_default();
            let message = if detail.is_empty() {
                format!("port {port} is already answering, so this instance was not started")
            } else {
                format!("port {port} is already in use by {detail}")
            };
            self.set_state(RuntimeState::Failed).await;
            return Err(StartError::PortInUse {
                port,
                detail: message,
            });
        }

        self.set_state(RuntimeState::Starting).await;

        let argv = self.config.command_argv();
        let (executable, arguments) = argv
            .split_first()
            .ok_or_else(|| StartError::Spawn("the command line is empty".to_string()))?;

        let mut cmd = Command::new(executable);
        cmd.args(arguments)
            .current_dir(&self.config.workspace)
            .env("DSH_HOME", &self.config.dsh_home)
            .env(
                "HOME",
                self.config
                    .dsh_home
                    .parent()
                    .unwrap_or(&self.config.dsh_home),
            )
            // The harness must never believe it may open a browser.
            .env("BROWSER", "false")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd
            .spawn()
            .map_err(|e| StartError::Spawn(format!("{executable}: {e}")))?;

        let pid = child.id();
        {
            let mut inner = self.inner.lock().await;
            inner.status.pid = pid;
        }

        // Read both streams: readiness arrives on stdout, and the explanation
        // for a failure usually arrives on stderr.
        let (line_tx, line_rx) = tokio::sync::mpsc::channel::<String>(256);
        pump_stream(child.stdout.take(), line_tx.clone(), None);
        pump_stream(
            child.stderr.take(),
            line_tx.clone(),
            Some(Arc::clone(&self.inner)),
        );
        drop(line_tx);

        self.await_readiness(child, line_rx).await
    }

    /// Watch the child until it is reachable, fails, or times out.
    ///
    /// Split out of [`Supervisor::start`] because the wait is a self-contained
    /// state machine: it owns the readiness signal, the reachability probe, the
    /// failure paths and the deadline, and nothing about process creation.
    async fn await_readiness(
        &self,
        mut child: Child,
        mut line_rx: tokio::sync::mpsc::Receiver<String>,
    ) -> Result<(), StartError> {
        let deadline = Instant::now() + self.config.ready_timeout;
        let mut announced_ready = false;
        // The authenticated URL the harness announced, captured rather than
        // discarded. See the assignment below for why this is the only way to
        // obtain it.
        let mut captured_url: Option<String> = None;

        loop {
            if deadline.saturating_duration_since(Instant::now()).is_zero() {
                let tail = self.stderr_tail().await;
                self.set_state(RuntimeState::Failed).await;
                return Err(StartError::Timeout(self.config.ready_timeout, tail));
            }

            tokio::select! {
                biased;

                // The child ending is always decisive.
                status = child.wait() => {
                    let code = status.ok().and_then(|s| s.code());
                    let tail = self.stderr_tail().await;
                    {
                        let mut inner = self.inner.lock().await;
                        inner.status.pid = None;
                        inner.status.last_error = Some(
                            StartError::ExitedEarly { code, tail: tail.clone() }.to_failure()
                        );
                    }
                    self.set_state(RuntimeState::Failed).await;
                    return Err(StartError::ExitedEarly { code, tail });
                }

                line = line_rx.recv() => {
                    // A closed channel means both streams ended; fall through to
                    // the probe rather than declaring failure, because a runtime
                    // that closed its streams may still be serving.
                    if let Some(l) = line {
                        // The announced URL is captured, not discarded.
                        //
                        // This line is the *only* place the harness publishes its
                        // per-process authentication token: 32 random bytes
                        // generated in memory at startup and never written to
                        // disk. A URL built from the port alone gets
                        // "dsh web authentication required", which reads as a
                        // broken instance rather than a missing parameter.
                        if let OutputSignal::Ready { url } = classify_line(&l) {
                            captured_url = Some(url);
                            announced_ready = true;
                        }
                        log_signal(&l);
                    }
                }

                // Once announced, confirm the socket actually answers. The
                // announcement proves intent; the probe proves reachability.
                () = tokio::time::sleep(Duration::from_millis(250)),
                    if announced_ready =>
                {
                    if self.probe_and_confirm(&child).await {
                        // Publish the URL the moment the instance is confirmed
                        // reachable, so `open` has something that works.
                        if let Some(url) = captured_url.take() {
                            self.set_auth_url(Some(url)).await;
                        }
                        return Ok(());
                    }
                }

                // While not yet announced, probe periodically too: an operator
                // may run a harness build that does not print the line, and
                // refusing to start in that case would be needlessly brittle.
                // The line remains the fast path.
                () = tokio::time::sleep(Duration::from_millis(500)) => {
                    if self.probe_and_confirm(&child).await {
                        // The port answers slightly before the URL line is
                        // printed, so keep listening briefly rather than
                        // returning at once — otherwise the token is discarded
                        // and the instance can never be opened in a browser.
                        let grace = Instant::now() + Duration::from_millis(URL_GRACE_MS);
                        while captured_url.is_none() && Instant::now() < grace {
                            let Ok(Some(l)) =
                                tokio::time::timeout(Duration::from_millis(100), line_rx.recv())
                                    .await
                            else {
                                continue;
                            };
                            if let OutputSignal::Ready { url } = classify_line(&l) {
                                captured_url = Some(url);
                            } else {
                                log_signal(&l);
                            }
                        }
                        if let Some(url) = captured_url.take() {
                            self.set_auth_url(Some(url)).await;
                        }
                        return Ok(());
                    }
                }
            }
        }
    }

    /// Probe the instance once, and on success record the transition to ready.
    ///
    /// Returns `true` when the runtime answered and its status was updated.
    async fn probe_and_confirm(&self, child: &Child) -> bool {
        let url = format!("http://127.0.0.1:{}", self.config.internal_port);
        if !probe_once(&url).await {
            return false;
        }

        let elapsed = {
            let inner = self.inner.lock().await;
            inner
                .started_at
                .map(|t| u64::try_from(t.elapsed().as_millis()).unwrap_or(u64::MAX))
        };
        let version = read_version(&self.config.binary).await;

        {
            let mut inner = self.inner.lock().await;
            inner.status.ready_in_ms = elapsed;
            inner.status.version = version;
            // The child handle is retained so `stop` can signal it. Cloning the
            // process handle is not possible, so the caller keeps ownership and
            // this only records that a child exists.
            let _ = child.id();
            // Also record the process that actually holds the port. On Windows
            // the spawned child is the `dsh.cmd` shim, which exits once it has
            // started `node`; the listener is a different, longer-lived process,
            // and it is the one a later restart must recognise as ours.
            if let Some(listener) = crate::process::listener_pid_on(self.config.internal_port) {
                inner.status.pid = Some(listener);
            }
            inner.status.state = RuntimeState::Ready;
        }

        self.set_state(RuntimeState::Ready).await;
        true
    }

    /// The captured stderr tail.
    pub async fn stderr_tail(&self) -> Vec<String> {
        self.inner.lock().await.stderr_tail.clone()
    }

    /// Ask the harness to stop, then ensure it has.
    ///
    /// # Errors
    ///
    /// Returns an error only if the child cannot be signalled at all; a child
    /// that already exited is not an error.
    pub async fn stop(&self, grace: Duration) -> Result<(), std::io::Error> {
        self.set_state(RuntimeState::Stopping).await;

        let mut inner = self.inner.lock().await;
        if let Some(child) = inner.child.as_mut() {
            // A tree kill, not a handle kill: on Windows the handle belongs to a
            // `cmd.exe` shim and the harness is its grandchild, so killing the
            // handle alone leaves the harness running while reporting success.
            // See `crate::process` for the full account.
            crate::process::kill_tree(child, grace).await;
        }
        inner.child = None;
        inner.status.pid = None;
        // The token died with the process. Clearing it here means no later
        // command can offer a URL that no longer authenticates.
        inner.status.auth_url = None;
        drop(inner);

        self.set_state(RuntimeState::Stopped).await;
        Ok(())
    }
}

/// Forward one child stream into the supervisor's line channel.
///
/// When `tail_sink` is present, each line is also appended to a bounded
/// buffer — that buffer is what explains a crash, so only stderr uses it.
fn pump_stream<R>(
    stream: Option<R>,
    tx: tokio::sync::mpsc::Sender<String>,
    tail_sink: Option<Arc<Mutex<Inner>>>,
) where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let Some(stream) = stream else { return };

    tokio::spawn(async move {
        let mut lines = BufReader::new(stream).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(sink) = &tail_sink {
                let mut inner = sink.lock().await;
                push_bounded(&mut inner.stderr_tail, line.clone(), DEFAULT_TAIL_LINES);
            }
            if tx.send(line).await.is_err() {
                break;
            }
        }
    });
}

/// Record one line of harness output at the right level.
///
/// Most lines are noise. Only the ones that carry meaning — a reported problem,
/// unavailable confinement, a fatal diagnostic — are logged, so a healthy boot
/// does not drown the log in plugin chatter.
fn log_signal(line: &str) {
    match classify_line(line) {
        OutputSignal::Ready { .. } => tracing::debug!(line = %line, "harness announced readiness"),
        OutputSignal::FrontendMissing
        | OutputSignal::MissingCredential
        | OutputSignal::PortInUse => {
            // Recorded as it happens; a startup that continues may still succeed.
            tracing::warn!(line = %line, "harness reported an issue");
        }
        OutputSignal::SandboxUnavailable => {
            tracing::warn!(
                target: "sandbox",
                line = %line,
                "process confinement is unavailable"
            );
        }
        OutputSignal::Other => {
            if is_fatal(line) {
                tracing::debug!(line = %line, "harness diagnostic");
            }
        }
    }
}

/// One reachability probe.
///
/// Any HTTP response counts, including 401 and 403: those prove the server is
/// listening and applying its trust fence, which is exactly what readiness
/// means. Only a transport failure means not-yet-ready.
async fn probe_once(url: &str) -> bool {
    let addr = match url
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .split('/')
        .next()
    {
        Some(a) => a.to_string(),
        None => return false,
    };

    matches!(
        tokio::time::timeout(
            Duration::from_secs(2),
            tokio::net::TcpStream::connect(addr.as_str())
        )
        .await,
        Ok(Ok(_))
    )
}

/// Read the harness version by running `dsh --version`.
///
/// Best-effort: an unreadable version never blocks readiness, because a
/// missing version is a cosmetic gap while a failed boot is not.
async fn read_version(binary: &std::path::Path) -> Option<String> {
    let out = tokio::time::timeout(
        Duration::from_secs(10),
        Command::new(binary).arg("--version").output(),
    )
    .await
    .ok()?
    .ok()?;

    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> SupervisorConfig {
        SupervisorConfig {
            binary: "/opt/dsh/bin/dsh".into(),
            internal_port: 3081,
            workspace: "/workspace".into(),
            dsh_home: "/data/dsh".into(),
            ready_timeout: Duration::from_secs(1),
            trusted_hosts: vec![],
        }
    }

    /// A config whose port nothing is listening on.
    ///
    /// `start` refuses a port that already answers, so a test that calls it must
    /// not use a fixed one: a real instance may be sitting on any `308x` port,
    /// and the test would then fail for a reason that has nothing to do with what
    /// it is checking.
    fn cfg_on_a_free_port() -> SupervisorConfig {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|l| l.local_addr())
            .map(|a| a.port())
            .expect("a loopback port must be available");
        SupervisorConfig {
            internal_port: port,
            ..cfg()
        }
    }

    #[test]
    fn argv_binds_loopback_and_never_opens_a_browser() {
        let argv = cfg().command_argv();
        assert_eq!(argv[0], "/opt/dsh/bin/dsh");
        assert_eq!(
            argv[1], "--profile",
            "the documented profile form, not the alias"
        );
        assert_eq!(argv[2], "web");

        let joined = argv.join(" ");
        // The harness's own safety refusal is honoured, not worked around.
        assert!(joined.contains("--host 127.0.0.1"));
        assert!(!joined.contains("0.0.0.0"));
        // A container must never attempt a browser handoff.
        assert!(joined.contains("--no-open"));
        assert!(joined.contains("--port 3081"));
    }

    #[test]
    fn trusted_hosts_are_repeatable_flags() {
        let mut c = cfg();
        c.trusted_hosts = vec!["192.168.1.50".into(), "dev.local".into()];
        let argv = c.command_argv();
        let joined = argv.join(" ");
        assert!(joined.contains("--trusted-host 192.168.1.50"));
        assert!(joined.contains("--trusted-host dev.local"));
    }

    #[test]
    fn argv_is_a_vec_and_cannot_be_injected_through_string_concatenation() {
        // Arguments are separate elements, so a hostname containing spaces or
        // shell metacharacters cannot become an extra flag.
        let mut c = cfg();
        c.trusted_hosts = vec!["evil; rm -rf /".into()];
        let argv = c.command_argv();
        let idx = argv.iter().position(|a| a == "--trusted-host").unwrap();
        assert_eq!(argv[idx + 1], "evil; rm -rf /");
        // Still exactly one element, never split into flags.
        assert!(!argv.iter().any(|a| a == "rm"));
    }

    #[test]
    fn start_error_codes_are_stable() {
        assert_eq!(StartError::Spawn("x".into()).code(), "DSH_NOT_INSTALLED");
        assert_eq!(
            StartError::ExitedEarly {
                code: Some(1),
                tail: vec![]
            }
            .code(),
            "DSH_EXITED"
        );
        assert_eq!(
            StartError::Timeout(Duration::from_secs(1), vec![]).code(),
            "DSH_READY_TIMEOUT"
        );
    }

    #[test]
    fn exited_early_failure_carries_the_stderr_tail() {
        let e = StartError::ExitedEarly {
            code: Some(1),
            tail: vec!["Error: missing dist".into()],
        };
        let f = e.to_failure();
        assert_eq!(f.exit_code, Some(1));
        assert!(f.user_message().contains("missing dist"));
    }

    #[tokio::test]
    async fn missing_binary_fails_with_a_named_code() {
        let mut c = cfg_on_a_free_port();
        c.binary = "/definitely/not/a/real/binary".into();
        let sup = Supervisor::new(c);
        let err = sup.start().await.unwrap_err();
        assert_eq!(err.code(), "DSH_NOT_INSTALLED");
        assert_eq!(
            sup.status().await.state,
            RuntimeState::Starting,
            "a spawn failure happens before the state can advance to Failed"
        );
    }

    #[tokio::test]
    async fn a_port_already_answering_is_refused_not_adopted() {
        // The regression this guards: a listener the router did not start was
        // reported as this instance's readiness. The router then claimed an
        // instance it had never launched — one it could not stop, whose sessions
        // were not the ones the user believed they were talking to.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        // Held open for the duration, so the port genuinely answers.
        let _held = listener;

        let sup = Supervisor::new(SupervisorConfig {
            internal_port: port,
            ..cfg()
        });
        let err = sup.start().await.unwrap_err();
        assert_eq!(err.code(), "DSH_PORT_IN_USE");
        assert_eq!(sup.status().await.state, RuntimeState::Failed);
    }

    #[tokio::test]
    async fn a_refused_port_names_the_port_in_its_message() {
        // The message is the whole point of the failure: "it did not start" is
        // useless without saying what is in the way.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let _held = listener;

        let sup = Supervisor::new(SupervisorConfig {
            internal_port: port,
            ..cfg()
        });
        let err = sup.start().await.unwrap_err();
        assert!(
            err.to_string().contains(&port.to_string()),
            "the refusal must name the port, got: {err}"
        );
    }

    #[tokio::test]
    async fn initial_status_is_absent_and_not_usable() {
        let sup = Supervisor::new(cfg());
        let s = sup.status().await;
        assert_eq!(s.state, RuntimeState::Absent);
        assert!(!s.state.is_usable());
        assert_eq!(s.port, 3081);
    }
}
