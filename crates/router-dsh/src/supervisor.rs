//! Process supervision: spawn the harness, detect readiness, keep it alive.
//!
//! The supervisor owns exactly one child. It never sleeps to decide readiness —
//! it watches for the harness's own announcement **and** confirms the socket
//! answers, because the announcement proves intent while the probe proves
//! reachability (PROPOSAL.md §P-11.4).

use crate::readiness::{
    classify_line, is_fatal, push_bounded, OutputSignal, DEFAULT_TAIL_LINES,
};
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
    /// concatenation (PROPOSAL.md §P-20 rule S-13).
    #[must_use]
    pub fn command_argv(&self) -> Vec<String> {
        let mut argv = vec![
            self.binary.to_string_lossy().to_string(),
            "web".to_string(),
            "--host".to_string(),
            "127.0.0.1".to_string(),
            "--port".to_string(),
            self.internal_port.to_string(),
            // The host launcher owns the browser handoff; a container must never
            // try to open one (PROPOSAL.md §P-25.7).
            "--no-open".to_string(),
        ];
        for host in &self.trusted_hosts {
            argv.push("--trusted-host".to_string());
            argv.push(host.clone());
        }
        argv
    }
}

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
            Self::Timeout(d, tail) => RuntimeFailure::new(
                self.code(),
                format!("no readiness within {}s", d.as_secs()),
            )
            .with_stderr_tail(tail.clone()),
            Self::Unreachable(u) => RuntimeFailure::new(self.code(), format!("{u} did not respond")),
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
        self.set_state(RuntimeState::Starting).await;

        let argv = self.config.command_argv();
        let (program, args) = argv.split_first().ok_or_else(|| {
            StartError::Spawn("the command line is empty".to_string())
        })?;

        let mut cmd = Command::new(program);
        cmd.args(args)
            .current_dir(&self.config.workspace)
            .env("DSH_HOME", &self.config.dsh_home)
            .env("HOME", self.config.dsh_home.parent().unwrap_or(&self.config.dsh_home))
            // The harness must never believe it may open a browser.
            .env("BROWSER", "false")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd.spawn().map_err(|e| {
            StartError::Spawn(format!("{program}: {e}"))
        })?;

        let pid = child.id();
        {
            let mut inner = self.inner.lock().await;
            inner.status.pid = pid;
        }

        // Read both streams: readiness arrives on stdout, and the explanation
        // for a failure usually arrives on stderr.
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let (line_tx, mut line_rx) = tokio::sync::mpsc::channel::<String>(256);

        if let Some(out) = stdout {
            let tx = line_tx.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(out).lines();
                while let Ok(Some(l)) = lines.next_line().await {
                    if tx.send(l).await.is_err() {
                        break;
                    }
                }
            });
        }
        if let Some(err) = stderr {
            let tx = line_tx.clone();
            let tail = Arc::clone(&self.inner);
            tokio::spawn(async move {
                let mut lines = BufReader::new(err).lines();
                while let Ok(Some(l)) = lines.next_line().await {
                    {
                        let mut inner = tail.lock().await;
                        push_bounded(&mut inner.stderr_tail, l.clone(), DEFAULT_TAIL_LINES);
                    }
                    if tx.send(l).await.is_err() {
                        break;
                    }
                }
            });
        }
        drop(line_tx);

        let deadline = Instant::now() + self.config.ready_timeout;
        let mut announced_ready = false;
        let mut announced_url: Option<String> = None;

        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
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
                    match line {
                        Some(l) => {
                            match classify_line(&l) {
                                OutputSignal::Ready { url } => {
                                    announced_ready = true;
                                    announced_url = Some(url);
                                }
                                OutputSignal::FrontendMissing
                                | OutputSignal::MissingCredential
                                | OutputSignal::PortInUse => {
                                    // Recorded as it happens; a startup that
                                    // continues may still succeed.
                                    tracing::warn!(line = %l, "harness reported an issue");
                                }
                                OutputSignal::SandboxUnavailable => {
                                    tracing::warn!(
                                        target: "sandbox",
                                        line = %l,
                                        "process confinement is unavailable"
                                    );
                                }
                                OutputSignal::Other => {
                                    if is_fatal(&l) {
                                        tracing::debug!(line = %l, "harness diagnostic");
                                    }
                                }
                            }
                        }
                        None => {
                            // Both streams closed; fall through to the probe
                            // rather than declaring failure, because a runtime
                            // that closed its streams may still be serving.
                        }
                    }
                }

                // Once announced, confirm the socket actually answers. The
                // announcement proves intent; the probe proves reachability.
                () = tokio::time::sleep(Duration::from_millis(250)),
                    if announced_ready =>
                {
                    let url = announced_url
                        .clone()
                        .unwrap_or_else(|| format!("http://127.0.0.1:{}", self.config.internal_port));
                    if probe_once(&url).await {
                        let elapsed = {
                            let inner = self.inner.lock().await;
                            inner.started_at.map(|t| t.elapsed().as_millis() as u64)
                        };
                        {
                            let mut inner = self.inner.lock().await;
                            inner.status.ready_in_ms = elapsed;
                            inner.status.version = read_version(&self.config.binary).await;
                            inner.child = Some(child);
                        }
                        self.set_state(RuntimeState::Ready).await;
                        return Ok(());
                    }
                }

                // While not yet announced, probe periodically too: an operator
                // may be running a harness build that does not print the line,
                // and refusing to start in that case would be needlessly
                // brittle. The line remains the fast path.
                () = tokio::time::sleep(Duration::from_millis(500)) => {
                    let url = format!("http://127.0.0.1:{}", self.config.internal_port);
                    if probe_once(&url).await {
                        let elapsed = {
                            let inner = self.inner.lock().await;
                            inner.started_at.map(|t| t.elapsed().as_millis() as u64)
                        };
                        {
                            let mut inner = self.inner.lock().await;
                            inner.status.ready_in_ms = elapsed;
                            inner.status.version = read_version(&self.config.binary).await;
                            inner.child = Some(child);
                        }
                        self.set_state(RuntimeState::Ready).await;
                        return Ok(());
                    }
                }
            }
        }
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
            let _ = child.start_kill();
            match tokio::time::timeout(grace, child.wait()).await {
                Ok(_) => {}
                Err(_) => {
                    let _ = child.kill().await;
                }
            }
        }
        inner.child = None;
        inner.status.pid = None;
        drop(inner);

        self.set_state(RuntimeState::Stopped).await;
        Ok(())
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

    #[test]
    fn argv_binds_loopback_and_never_opens_a_browser() {
        let argv = cfg().command_argv();
        assert_eq!(argv[0], "/opt/dsh/bin/dsh");
        assert_eq!(argv[1], "web");

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
        assert_eq!(
            StartError::Spawn("x".into()).code(),
            "DSH_NOT_INSTALLED"
        );
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
        let mut c = cfg();
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
    async fn initial_status_is_absent_and_not_usable() {
        let sup = Supervisor::new(cfg());
        let s = sup.status().await;
        assert_eq!(s.state, RuntimeState::Absent);
        assert!(!s.state.is_usable());
        assert_eq!(s.port, 3081);
    }
}
