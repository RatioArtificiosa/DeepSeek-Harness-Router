//! Runtime supervision states and transitions.
//!
//! The supervisor owns one child process and reports its lifecycle. The state
//! machine is small and explicit so that every surface — `/health`, the UI
//! banner, the launcher — reads the same truth.

use router_core::Status;
use serde::{Deserialize, Serialize};

/// The lifecycle state of the supervised runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeState {
    /// Not started yet.
    Absent,
    /// Spawned, but not yet accepting requests.
    Starting,
    /// Listening and accepting.
    Ready,
    /// Asked to stop; winding down.
    Stopping,
    /// Exited cleanly.
    Stopped,
    /// Exited unexpectedly, or never became ready.
    Failed,
}

impl RuntimeState {
    /// Whether the runtime is usable right now.
    #[must_use]
    pub const fn is_usable(self) -> bool {
        matches!(self, Self::Ready)
    }

    /// Whether the supervisor may attempt another start.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Stopped | Self::Failed)
    }

    /// How this state maps onto the health model.
    ///
    /// `Starting` is deliberately **not** unhealthy: a runtime that is coming
    /// up is normal, and reporting it as a failure would make the launcher
    /// give up during a healthy boot.
    #[must_use]
    pub const fn health(self) -> Status {
        match self {
            Self::Ready => Status::Healthy,
            Self::Absent | Self::Starting | Self::Stopping | Self::Stopped => Status::Degraded,
            Self::Failed => Status::Unhealthy,
        }
    }

    /// The wire form, for `/health`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::Starting => "starting",
            Self::Ready => "ready",
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
        }
    }
}

/// A snapshot of the supervisor's view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeStatus {
    /// Current state.
    pub state: RuntimeState,
    /// Harness version, once discovered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Process id while running.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Loopback port the harness listens on.
    pub port: u16,
    /// How many times the runtime has been (re)started.
    pub restarts: u32,
    /// How long readiness took, once achieved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ready_in_ms: Option<u64>,
    /// The last error, if the runtime failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<RuntimeFailure>,
}

impl Default for RuntimeStatus {
    fn default() -> Self {
        Self {
            state: RuntimeState::Absent,
            version: None,
            pid: None,
            port: 0,
            restarts: 0,
            ready_in_ms: None,
            last_error: None,
        }
    }
}

impl RuntimeStatus {
    /// A status in the given state on the given port.
    #[must_use]
    pub fn new(state: RuntimeState, port: u16) -> Self {
        Self {
            state,
            port,
            ..Self::default()
        }
    }

    /// The health status this runtime state implies.
    #[must_use]
    pub const fn health(&self) -> Status {
        self.state.health()
    }
}

/// Why the runtime stopped.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeFailure {
    /// Stable code.
    pub code: String,
    /// Human-readable detail.
    pub detail: String,
    /// The process exit code, when there was one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// The last few stderr lines, which are usually the real explanation.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub stderr_tail: Vec<String>,
}

impl RuntimeFailure {
    /// Build a failure with no captured output.
    #[must_use]
    pub fn new(code: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            detail: detail.into(),
            exit_code: None,
            stderr_tail: Vec::new(),
        }
    }

    /// Attach the exit code.
    #[must_use]
    pub fn with_exit_code(mut self, code: i32) -> Self {
        self.exit_code = Some(code);
        self
    }

    /// Attach captured stderr lines.
    #[must_use]
    pub fn with_stderr_tail(mut self, lines: Vec<String>) -> Self {
        self.stderr_tail = lines;
        self
    }

    /// Render the failure the way a user should see it.
    ///
    /// Always names the code and the detail, and includes the stderr tail when
    /// there is one, because the tail is usually the actual explanation.
    #[must_use]
    pub fn user_message(&self) -> String {
        use std::fmt::Write as _;

        let mut out = format!("{}: {}", self.code, self.detail);
        if let Some(code) = self.exit_code {
            let _ = write!(out, "\n  exit code: {code}");
        }
        if !self.stderr_tail.is_empty() {
            out.push_str("\n  last output:");
            for line in &self.stderr_tail {
                let _ = write!(out, "\n    {line}");
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_ready_is_usable() {
        assert!(RuntimeState::Ready.is_usable());
        for s in [
            RuntimeState::Absent,
            RuntimeState::Starting,
            RuntimeState::Stopping,
            RuntimeState::Stopped,
            RuntimeState::Failed,
        ] {
            assert!(!s.is_usable(), "{s:?} must not be usable");
        }
    }

    #[test]
    fn starting_is_not_a_failure() {
        // A runtime coming up is normal. Reporting it as unhealthy would make
        // the launcher abandon a healthy boot.
        assert_eq!(RuntimeState::Starting.health(), Status::Degraded);
        assert_ne!(RuntimeState::Starting.health(), Status::Unhealthy);
    }

    #[test]
    fn failed_is_the_only_unhealthy_state() {
        for s in [
            RuntimeState::Absent,
            RuntimeState::Starting,
            RuntimeState::Ready,
            RuntimeState::Stopping,
            RuntimeState::Stopped,
        ] {
            assert_ne!(s.health(), Status::Unhealthy, "{s:?}");
        }
        assert_eq!(RuntimeState::Failed.health(), Status::Unhealthy);
    }

    #[test]
    fn terminal_states_are_exactly_the_ended_ones() {
        assert!(RuntimeState::Stopped.is_terminal());
        assert!(RuntimeState::Failed.is_terminal());
        assert!(!RuntimeState::Ready.is_terminal());
        assert!(!RuntimeState::Starting.is_terminal());
    }

    #[test]
    fn failure_message_includes_exit_code_and_output() {
        let f = RuntimeFailure::new("DSH_EXITED", "the runtime stopped")
            .with_exit_code(1)
            .with_stderr_tail(vec!["Error: missing frontend dist".to_string()]);
        let msg = f.user_message();
        assert!(msg.contains("DSH_EXITED"));
        assert!(msg.contains("exit code: 1"));
        assert!(msg.contains("missing frontend dist"));
    }

    #[test]
    fn failure_message_without_output_still_names_the_cause() {
        let f = RuntimeFailure::new("DSH_READY_TIMEOUT", "no readiness within 120s");
        let msg = f.user_message();
        assert!(msg.contains("DSH_READY_TIMEOUT"));
        assert!(msg.contains("120s"));
        assert!(!msg.contains("exit code"));
    }
}
