//! The multi-instance supervisor.
//!
//! One router, many harness processes. Each runs with its own state root, its
//! own port, and its own working directory, and none of them can reach into the
//! others.
//!
//! # Why a map of children rather than a list
//!
//! Instances are addressed by name, and the name outlives any particular
//! process. A supervisor keyed by name can report on a stopped instance, which
//! is what the control surface needs: `router list` shows what exists, not only
//! what is currently running.
//!
//! # Independence
//!
//! Failure is contained. A child that crashes, or fails to start, changes
//! nothing about its siblings — they hold no shared handles, no shared files,
//! and no shared locks. That property is the reason this design works at all,
//! and it is asserted by a test rather than assumed.

use crate::provisioning::{provision, ProvisionReport};
use crate::readiness::{classify_line, is_fatal, push_bounded, OutputSignal, DEFAULT_TAIL_LINES};
use crate::state::{RuntimeFailure, RuntimeState, RuntimeStatus};
use router_core::registry::Instance;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{watch, Mutex, RwLock};

/// Everything the supervisor needs to start one instance.
#[derive(Debug, Clone)]
pub struct InstanceSpec {
    /// The instance's name, which is also its address in the registry.
    pub name: String,
    /// The project directory the agent works in.
    pub workspace: PathBuf,
    /// The instance's own state root, passed as `DSH_HOME`.
    pub state_root: PathBuf,
    /// The port this instance listens on.
    pub port: u16,
    /// The model to provision on first start, if any.
    pub model: Option<String>,
    /// Whether credentials are shared with the host installation.
    pub share_credentials: bool,
    /// Extra environment variables.
    pub env: BTreeMap<String, String>,
}

impl InstanceSpec {
    /// Build a spec from a registry entry.
    #[must_use]
    pub fn from_registry(name: &str, instance: &Instance, home: &Path) -> Self {
        Self {
            name: name.to_string(),
            workspace: instance.workspace.clone(),
            state_root: router_core::registry::Registry::state_root(home, name),
            port: instance.port,
            model: instance.model.clone(),
            share_credentials: instance.share_credentials,
            env: instance.env.clone(),
        }
    }
}

/// How the supervisor behaves.
#[derive(Debug, Clone)]
pub struct MultiConfig {
    /// Path to the `dsh` executable.
    pub binary: PathBuf,
    /// How long to wait for any one instance to become ready.
    pub ready_timeout: Duration,
    /// How long to wait for a graceful stop before forcing.
    pub stop_grace: Duration,
    /// Extra authorities every instance should accept.
    pub trusted_hosts: Vec<String>,
    /// The host's credential file, used only when an instance shares it.
    pub host_credentials: Option<PathBuf>,
}

impl Default for MultiConfig {
    fn default() -> Self {
        Self {
            binary: PathBuf::from("dsh"),
            ready_timeout: Duration::from_secs(120),
            stop_grace: Duration::from_secs(15),
            trusted_hosts: Vec::new(),
            host_credentials: None,
        }
    }
}

/// Why starting one instance failed.
#[derive(Debug, thiserror::Error)]
pub enum InstanceError {
    /// The instance's state root could not be prepared.
    #[error("cannot prepare the instance home: {0}")]
    Provision(String),
    /// The harness executable could not be launched.
    #[error("cannot launch the harness: {0}")]
    Spawn(String),
    /// The harness exited before it was ready.
    #[error("the harness exited before becoming ready (code {code:?})")]
    ExitedEarly {
        /// Exit code, when the process reported one.
        code: Option<i32>,
        /// Captured stderr, which usually explains why.
        tail: Vec<String>,
    },
    /// The harness never became reachable in time.
    #[error("the harness did not become ready within {0:?}")]
    Timeout(Duration, Vec<String>),
    /// No instance by that name exists.
    #[error("no instance named '{0}'")]
    Unknown(String),
}

impl InstanceError {
    /// The stable code for this failure.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Provision(_) => "INSTANCE_PROVISION_FAILED",
            Self::Spawn(_) => "DSH_NOT_INSTALLED",
            Self::ExitedEarly { .. } => "DSH_EXITED",
            Self::Timeout(..) => "DSH_READY_TIMEOUT",
            Self::Unknown(_) => "INSTANCE_UNKNOWN",
        }
    }

    /// Convert into the health model's failure shape.
    #[must_use]
    pub fn to_failure(&self) -> RuntimeFailure {
        match self {
            Self::Provision(d) | Self::Spawn(d) => RuntimeFailure::new(self.code(), d.clone()),
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
            Self::Unknown(name) => {
                RuntimeFailure::new(self.code(), format!("no instance named '{name}'"))
            }
        }
    }
}

/// The live state of one supervised instance.
struct Slot {
    spec: InstanceSpec,
    child: Option<Child>,
    status: RuntimeStatus,
    stderr_tail: Vec<String>,
    started_at: Option<Instant>,
}

/// Supervises every instance the router manages.
pub struct MultiSupervisor {
    config: MultiConfig,
    slots: RwLock<BTreeMap<String, Arc<Mutex<Slot>>>>,
    status_tx: watch::Sender<BTreeMap<String, RuntimeStatus>>,
}

impl MultiSupervisor {
    /// Create an empty supervisor.
    #[must_use]
    pub fn new(config: MultiConfig) -> Self {
        let (status_tx, _) = watch::channel(BTreeMap::new());
        Self {
            config,
            slots: RwLock::new(BTreeMap::new()),
            status_tx,
        }
    }

    /// Subscribe to status changes for every instance.
    #[must_use]
    pub fn watch(&self) -> watch::Receiver<BTreeMap<String, RuntimeStatus>> {
        self.status_tx.subscribe()
    }

    /// Register an instance without starting it.
    ///
    /// Idempotent: registering a name that already exists leaves the existing
    /// slot alone, so a running instance is never disturbed by a re-register.
    pub async fn register(&self, spec: InstanceSpec) {
        let mut slots = self.slots.write().await;
        slots.entry(spec.name.clone()).or_insert_with(|| {
            let status = RuntimeStatus::new(RuntimeState::Absent, spec.port);
            Arc::new(Mutex::new(Slot {
                spec,
                child: None,
                status,
                stderr_tail: Vec::new(),
                started_at: None,
            }))
        });
        drop(slots);
        self.publish().await;
    }

    /// The names of every registered instance.
    pub async fn names(&self) -> Vec<String> {
        self.slots.read().await.keys().cloned().collect()
    }

    /// A snapshot of every instance's status.
    pub async fn statuses(&self) -> BTreeMap<String, RuntimeStatus> {
        let slots = self.slots.read().await;
        let mut out = BTreeMap::new();
        for (name, slot) in slots.iter() {
            out.insert(name.clone(), slot.lock().await.status.clone());
        }
        out
    }

    /// One instance's status.
    pub async fn status_of(&self, name: &str) -> Option<RuntimeStatus> {
        let slot = {
            let slots = self.slots.read().await;
            slots.get(name).cloned()
        }?;
        let guard = slot.lock().await;
        let status = guard.status.clone();
        drop(guard);
        Some(status)
    }

    /// The captured stderr tail for an instance.
    pub async fn stderr_tail(&self, name: &str) -> Vec<String> {
        let slot = {
            let slots = self.slots.read().await;
            slots.get(name).cloned()
        };
        let Some(slot) = slot else {
            return Vec::new();
        };
        let guard = slot.lock().await;
        let tail = guard.stderr_tail.clone();
        drop(guard);
        tail
    }

    async fn publish(&self) {
        let snapshot = self.statuses().await;
        let _ = self.status_tx.send(snapshot);
    }

    async fn set_state(&self, name: &str, state: RuntimeState) {
        {
            let slots = self.slots.read().await;
            let Some(slot) = slots.get(name) else { return };
            let mut guard = slot.lock().await;
            guard.status.state = state;
        }
        self.publish().await;
    }

    /// Start one instance.
    ///
    /// # Errors
    ///
    /// Returns [`InstanceError`] when the name is unknown, provisioning fails,
    /// or the harness does not become ready.
    pub async fn start(&self, name: &str) -> Result<ProvisionReport, InstanceError> {
        let slot = self
            .slots
            .read()
            .await
            .get(name)
            .cloned()
            .ok_or_else(|| InstanceError::Unknown(name.to_string()))?;

        let spec = {
            let guard = slot.lock().await;
            if guard.status.state.is_usable() {
                // Already running: report the state root without re-provisioning.
                return Ok(ProvisionReport {
                    state_root: guard.spec.state_root.clone(),
                    settings_written: false,
                    credentials_created: false,
                    credentials_shared: guard.spec.share_credentials,
                });
            }
            guard.spec.clone()
        };

        self.set_state(name, RuntimeState::Starting).await;

        // Prepare the instance's own home first. This is the step that makes
        // the instance independent, so it happens before anything is spawned.
        let report = provision(
            &spec.state_root,
            spec.model.as_deref(),
            spec.share_credentials,
            self.config.host_credentials.as_deref(),
        )
        .map_err(|e| InstanceError::Provision(e.detail))?;

        {
            let mut guard = slot.lock().await;
            guard.status.restarts += 1;
            guard.stderr_tail.clear();
            guard.started_at = Some(Instant::now());
        }

        let argv = self.command_argv(&spec);
        let (executable, arguments) = argv
            .split_first()
            .ok_or_else(|| InstanceError::Spawn("the command line is empty".to_string()))?;

        let mut cmd = Command::new(executable);
        cmd.args(arguments)
            .current_dir(&spec.workspace)
            // The isolation lever: this instance's own state root.
            .env("DSH_HOME", &spec.state_root)
            .env("HOME", spec.state_root.parent().unwrap_or(&spec.state_root))
            // A router instance must never try to open a browser; the router
            // owns that decision.
            .env("BROWSER", "false")
            .envs(&spec.env)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd
            .spawn()
            .map_err(|e| InstanceError::Spawn(format!("{executable}: {e}")))?;

        let pid = child.id();
        {
            let mut guard = slot.lock().await;
            guard.status.pid = pid;
        }

        let (line_tx, line_rx) = tokio::sync::mpsc::channel::<String>(256);
        pump(child.stdout.take(), line_tx.clone(), None);
        pump(
            child.stderr.take(),
            line_tx.clone(),
            Some(Arc::clone(&slot)),
        );
        drop(line_tx);

        self.await_ready(name, &slot, child, line_rx).await?;
        Ok(report)
    }

    /// The command line for one instance.
    #[must_use]
    pub fn command_argv(&self, spec: &InstanceSpec) -> Vec<String> {
        let mut argv = vec![
            self.config.binary.to_string_lossy().to_string(),
            "web".to_string(),
            "--host".to_string(),
            "127.0.0.1".to_string(),
            "--port".to_string(),
            spec.port.to_string(),
            "--no-open".to_string(),
        ];
        for host in &self.config.trusted_hosts {
            argv.push("--trusted-host".to_string());
            argv.push(host.clone());
        }
        argv
    }

    async fn await_ready(
        &self,
        name: &str,
        slot: &Arc<Mutex<Slot>>,
        mut child: Child,
        mut line_rx: tokio::sync::mpsc::Receiver<String>,
    ) -> Result<(), InstanceError> {
        let port = { slot.lock().await.spec.port };
        let deadline = Instant::now() + self.config.ready_timeout;
        let mut announced = false;

        loop {
            if deadline.saturating_duration_since(Instant::now()).is_zero() {
                let tail = slot.lock().await.stderr_tail.clone();
                self.set_state(name, RuntimeState::Failed).await;
                return Err(InstanceError::Timeout(self.config.ready_timeout, tail));
            }

            tokio::select! {
                biased;

                status = child.wait() => {
                    let code = status.ok().and_then(|s| s.code());
                    let tail = {
                        let mut guard = slot.lock().await;
                        guard.status.pid = None;
                        guard.stderr_tail.clone()
                    };
                    {
                        let mut guard = slot.lock().await;
                        guard.status.last_error =
                            Some(InstanceError::ExitedEarly { code, tail: tail.clone() }.to_failure());
                    }
                    self.set_state(name, RuntimeState::Failed).await;
                    return Err(InstanceError::ExitedEarly { code, tail });
                }

                line = line_rx.recv() => {
                    if let Some(l) = line {
                        if matches!(classify_line(&l), OutputSignal::Ready { .. }) {
                            announced = true;
                        }
                        log_signal(&l);
                    }
                }

                () = tokio::time::sleep(Duration::from_millis(250)), if announced => {
                    if self.confirm_ready(slot, port).await {
                        return Ok(());
                    }
                }

                // Probe even before the announcement, because an operator may
                // run a harness build that does not print the readiness line.
                // The line remains the fast path.
                () = tokio::time::sleep(Duration::from_millis(500)) => {
                    if self.confirm_ready(slot, port).await {
                        return Ok(());
                    }
                }
            }
        }
    }

    async fn confirm_ready(&self, slot: &Arc<Mutex<Slot>>, port: u16) -> bool {
        if !probe_once(port).await {
            return false;
        }

        let elapsed = {
            let guard = slot.lock().await;
            guard
                .started_at
                .map(|t| u64::try_from(t.elapsed().as_millis()).unwrap_or(u64::MAX))
        };
        let version = read_version(&self.config.binary).await;

        {
            let mut guard = slot.lock().await;
            guard.status.ready_in_ms = elapsed;
            guard.status.version = version;
            guard.status.state = RuntimeState::Ready;
        }
        self.publish().await;
        true
    }

    /// Stop one instance.
    ///
    /// # Errors
    ///
    /// Returns [`InstanceError::Unknown`] when no such instance exists.
    pub async fn stop(&self, name: &str) -> Result<(), InstanceError> {
        let slot = self
            .slots
            .read()
            .await
            .get(name)
            .cloned()
            .ok_or_else(|| InstanceError::Unknown(name.to_string()))?;

        self.set_state(name, RuntimeState::Stopping).await;

        let grace = self.config.stop_grace;
        let mut guard = slot.lock().await;
        if let Some(child) = guard.child.as_mut() {
            let _ = child.start_kill();
            if tokio::time::timeout(grace, child.wait()).await.is_err() {
                let _ = child.kill().await;
            }
        }
        guard.child = None;
        guard.status.pid = None;
        guard.status.state = RuntimeState::Stopped;
        drop(guard);

        self.publish().await;
        Ok(())
    }

    /// Stop every running instance.
    ///
    /// Failures are collected rather than short-circuiting, because a shutdown
    /// that abandons the remaining instances because one refused to stop is
    /// worse than one that reports the stragglers.
    pub async fn stop_all(&self) -> Vec<(String, InstanceError)> {
        let names = self.names().await;
        let mut failures = Vec::new();
        for name in names {
            let running = self
                .status_of(&name)
                .await
                .is_some_and(|s| s.state.is_usable());
            if running {
                if let Err(e) = self.stop(&name).await {
                    failures.push((name, e));
                }
            }
        }
        failures
    }

    /// Every instance that reports ready.
    pub async fn running(&self) -> Vec<String> {
        self.statuses()
            .await
            .into_iter()
            .filter(|(_, s)| s.state.is_usable())
            .map(|(n, _)| n)
            .collect()
    }

    /// The count of ready instances.
    pub async fn running_count(&self) -> usize {
        self.running().await.len()
    }
}

/// Forward one child stream into the supervisor's line channel.
fn pump<R>(stream: Option<R>, tx: tokio::sync::mpsc::Sender<String>, slot: Option<Arc<Mutex<Slot>>>)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let Some(stream) = stream else { return };

    tokio::spawn(async move {
        let mut lines = BufReader::new(stream).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(slot) = &slot {
                let mut guard = slot.lock().await;
                push_bounded(&mut guard.stderr_tail, line.clone(), DEFAULT_TAIL_LINES);
            }
            if tx.send(line).await.is_err() {
                break;
            }
        }
    });
}

/// Log one line of harness output at the level its meaning deserves.
fn log_signal(line: &str) {
    match classify_line(line) {
        OutputSignal::Ready { .. } => tracing::debug!(line = %line, "harness announced readiness"),
        OutputSignal::FrontendMissing
        | OutputSignal::MissingCredential
        | OutputSignal::PortInUse => {
            tracing::warn!(line = %line, "harness reported an issue");
        }
        OutputSignal::SandboxUnavailable => {
            tracing::warn!(target: "sandbox", line = %line, "process confinement is unavailable");
        }
        OutputSignal::Other => {
            if is_fatal(line) {
                tracing::debug!(line = %line, "harness diagnostic");
            }
        }
    }
}

/// One reachability probe on loopback.
///
/// Any accepted connection counts, including one the harness will later answer
/// with 401 or 403 — those prove it is listening and applying its trust fence,
/// which is exactly what readiness means.
async fn probe_once(port: u16) -> bool {
    matches!(
        tokio::time::timeout(
            Duration::from_secs(2),
            tokio::net::TcpStream::connect(("127.0.0.1", port)),
        )
        .await,
        Ok(Ok(_))
    )
}

/// Read the harness version, best-effort.
async fn read_version(binary: &Path) -> Option<String> {
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
    (!s.is_empty()).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use router_core::registry::Registry;

    fn spec(name: &str, port: u16) -> InstanceSpec {
        InstanceSpec {
            name: name.to_string(),
            workspace: PathBuf::from(format!("/tmp/{name}")),
            state_root: PathBuf::from(format!("/tmp/router/instances/{name}/dsh")),
            port,
            model: None,
            share_credentials: false,
            env: BTreeMap::new(),
        }
    }

    #[tokio::test]
    async fn starts_empty() {
        let sup = MultiSupervisor::new(MultiConfig::default());
        assert!(sup.names().await.is_empty());
        assert_eq!(sup.running_count().await, 0);
    }

    #[tokio::test]
    async fn register_is_idempotent() {
        let sup = MultiSupervisor::new(MultiConfig::default());
        sup.register(spec("alpha", 3081)).await;
        sup.register(spec("alpha", 3081)).await;
        assert_eq!(sup.names().await, vec!["alpha"]);
    }

    #[tokio::test]
    async fn instances_are_addressed_by_name_and_stay_ordered() {
        let sup = MultiSupervisor::new(MultiConfig::default());
        for n in ["zeta", "alpha", "mid"] {
            sup.register(spec(n, 3081)).await;
        }
        assert_eq!(sup.names().await, vec!["alpha", "mid", "zeta"]);
    }

    #[tokio::test]
    async fn unknown_instance_fails_with_a_named_error() {
        let sup = MultiSupervisor::new(MultiConfig::default());
        let e = sup.start("ghost").await.unwrap_err();
        assert_eq!(e.code(), "INSTANCE_UNKNOWN");
        assert!(e.to_failure().user_message().contains("ghost"));

        assert!(sup.stop("ghost").await.is_err());
        assert!(sup.status_of("ghost").await.is_none());
    }

    #[tokio::test]
    async fn reported_statuses_are_independent() {
        // The property the whole product depends on: one instance's state
        // must never be another's.
        let sup = MultiSupervisor::new(MultiConfig::default());
        sup.register(spec("alpha", 3081)).await;
        sup.register(spec("beta", 3082)).await;

        let all = sup.statuses().await;
        assert_eq!(all.len(), 2);
        assert_eq!(all["alpha"].port, 3081);
        assert_eq!(all["beta"].port, 3082);
        assert_eq!(all["alpha"].state, RuntimeState::Absent);
    }

    #[tokio::test]
    async fn stop_all_tolerates_nothing_running() {
        let sup = MultiSupervisor::new(MultiConfig::default());
        sup.register(spec("alpha", 3081)).await;
        let failures = sup.stop_all().await;
        assert!(
            failures.is_empty(),
            "stopping an idle instance is not a failure"
        );
    }

    #[tokio::test]
    async fn argv_binds_loopback_for_every_instance() {
        let sup = MultiSupervisor::new(MultiConfig::default());
        let a = sup.command_argv(&spec("alpha", 3081));
        let joined = a.join(" ");

        assert!(joined.contains("--host 127.0.0.1"));
        assert!(
            !joined.contains("0.0.0.0"),
            "the harness rejects this anyway"
        );
        assert!(joined.contains("--port 3081"));
        assert!(
            joined.contains("--no-open"),
            "the router owns the browser handoff"
        );
    }

    #[tokio::test]
    async fn argv_gives_each_instance_its_own_port() {
        let sup = MultiSupervisor::new(MultiConfig::default());
        let a = sup.command_argv(&spec("alpha", 3081)).join(" ");
        let b = sup.command_argv(&spec("beta", 3082)).join(" ");
        assert!(a.contains("--port 3081"));
        assert!(b.contains("--port 3082"));
        assert_ne!(a, b);
    }

    #[tokio::test]
    async fn argv_cannot_be_injected_through_trusted_hosts() {
        let sup = MultiSupervisor::new(MultiConfig {
            trusted_hosts: vec!["evil; rm -rf /".into()],
            ..MultiConfig::default()
        });
        let argv = sup.command_argv(&spec("alpha", 3081));
        let idx = argv.iter().position(|a| a == "--trusted-host").unwrap();
        assert_eq!(argv[idx + 1], "evil; rm -rf /");
        assert!(!argv.iter().any(|a| a == "rm"));
    }

    #[tokio::test]
    async fn missing_binary_fails_the_instance_without_touching_siblings() {
        let sup = MultiSupervisor::new(MultiConfig {
            binary: PathBuf::from("/definitely/not/a/harness"),
            ready_timeout: Duration::from_millis(300),
            ..MultiConfig::default()
        });

        let dir = tempfile::tempdir().unwrap();
        let mut a = spec("alpha", 3081);
        a.state_root = dir.path().join("alpha");
        a.workspace = dir.path().to_path_buf();
        let mut b = spec("beta", 3082);
        b.state_root = dir.path().join("beta");
        b.workspace = dir.path().to_path_buf();
        sup.register(a).await;
        sup.register(b).await;

        let err = sup.start("alpha").await.unwrap_err();
        assert_eq!(err.code(), "DSH_NOT_INSTALLED");

        // Beta is untouched by alpha's failure.
        let beta = sup.status_of("beta").await.unwrap();
        assert_eq!(beta.state, RuntimeState::Absent);
        assert_eq!(beta.port, 3082);
    }

    #[tokio::test]
    async fn starting_provisions_the_instance_home() {
        let sup = MultiSupervisor::new(MultiConfig {
            binary: PathBuf::from("/definitely/not/a/harness"),
            ready_timeout: Duration::from_millis(200),
            ..MultiConfig::default()
        });

        let dir = tempfile::tempdir().unwrap();
        let mut s = spec("alpha", 3081);
        s.state_root = dir.path().join("state");
        s.workspace = dir.path().to_path_buf();
        s.model = Some("deepseek-v4-pro".into());
        sup.register(s).await;

        // Provisioning happens before the spawn, so it must have succeeded even
        // though the launch then failed.
        let _ = sup.start("alpha").await;

        assert!(dir.path().join("state").is_dir());
        assert!(dir.path().join("state/.credentials.yaml").exists());
        let settings = std::fs::read_to_string(dir.path().join("state/settings.yaml")).unwrap();
        assert!(settings.contains("deepseek-v4-pro"));
    }

    #[test]
    fn spec_from_registry_derives_a_per_instance_state_root() {
        let home = Path::new("/router");
        let inst = Instance::new(PathBuf::from("/projects/a"), 3081);
        let a = InstanceSpec::from_registry("alpha", &inst, home);
        let b = InstanceSpec::from_registry("beta", &inst, home);

        assert_ne!(a.state_root, b.state_root, "state roots must differ");
        assert_eq!(a.state_root, Registry::state_root(home, "alpha"));
        assert_eq!(a.port, 3081);
    }

    #[test]
    fn error_codes_are_stable() {
        assert_eq!(
            InstanceError::Provision("x".into()).code(),
            "INSTANCE_PROVISION_FAILED"
        );
        assert_eq!(InstanceError::Spawn("x".into()).code(), "DSH_NOT_INSTALLED");
        assert_eq!(
            InstanceError::Timeout(Duration::from_secs(1), vec![]).code(),
            "DSH_READY_TIMEOUT"
        );
    }
}
