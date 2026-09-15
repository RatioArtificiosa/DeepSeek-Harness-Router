//! The `/health` contract.
//!
//! The launcher blocks on this endpoint before telling the user the system is
//! ready (PROPOSAL.md §P-25.9), so the shape here is a public contract, not an
//! internal detail.
//!
//! # Why `degraded` returns HTTP 200
//!
//! A first-run user has no model credential yet. If that made the service
//! `unhealthy`, the launcher would refuse to declare success and the user could
//! never reach the UI to enter a key. `degraded` is the honest state there.

use serde::{Deserialize, Serialize};

/// Overall service status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Every critical check passed.
    Healthy,
    /// Critical checks passed; a non-critical check failed.
    Degraded,
    /// A critical check failed.
    Unhealthy,
}

impl Status {
    /// The HTTP status code this state should be served with.
    #[must_use]
    pub const fn http_code(self) -> u16 {
        match self {
            Self::Healthy | Self::Degraded => 200,
            Self::Unhealthy => 503,
        }
    }

    /// The string form used on the wire.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Degraded => "degraded",
            Self::Unhealthy => "unhealthy",
        }
    }
}

/// One named check with its outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Check {
    /// Stable check identifier, e.g. `dsh-binary`.
    pub name: String,
    /// Whether the check passed.
    pub ok: bool,
    /// Whether a failure here makes the whole service unhealthy.
    ///
    /// Non-critical checks (a missing model key, unavailable confinement)
    /// degrade rather than fail.
    pub critical: bool,
    /// How long the check took, in milliseconds.
    pub ms: u64,
    /// Human-readable detail, present when something needs explaining.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl Check {
    /// A passing check.
    #[must_use]
    pub fn pass(name: impl Into<String>, critical: bool, ms: u64) -> Self {
        Self {
            name: name.into(),
            ok: true,
            critical,
            ms,
            detail: None,
        }
    }

    /// A failing check, with the reason a user would need.
    #[must_use]
    pub fn fail(
        name: impl Into<String>,
        critical: bool,
        ms: u64,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            ok: false,
            critical,
            ms,
            detail: Some(detail.into()),
        }
    }
}

/// Application-level facts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplicationInfo {
    /// Status of the router process itself.
    pub status: Status,
    /// Published port.
    pub port: u16,
    /// Router version.
    pub version: String,
}

/// Harness runtime facts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeInfo {
    /// Status of the supervised runtime.
    pub status: Status,
    /// Engine name.
    pub engine: String,
    /// Supervision state (`absent`, `starting`, `ready`, `stopping`, `stopped`, `failed`).
    pub state: String,
    /// Harness version, once known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Process id, while running.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Restarts since boot.
    pub restarts: u32,
    /// How long readiness took, once achieved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ready_in_ms: Option<u64>,
}

/// Harness availability facts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DshInfo {
    /// Whether the binary was found and is executable.
    pub available: bool,
    /// Path to the binary.
    pub binary: String,
    /// Harness home directory.
    pub home: String,
    /// Whether the home directory is writable.
    pub home_writable: bool,
    /// Whether the web frontend assets are present.
    pub frontend_built: bool,
}

/// Workspace availability facts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceInfo {
    /// Whether the workspace is mounted and readable.
    pub available: bool,
    /// The in-container path (always `/workspace`).
    pub path: String,
    /// Whether a real write succeeded.
    pub writable: bool,
    /// The mount source as seen from the host, for display only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mount_source: Option<String>,
}

/// The complete `/health` payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthReport {
    /// Overall status.
    pub status: Status,
    /// Router version.
    pub version: String,
    /// Seconds since start.
    pub uptime_seconds: u64,
    /// ISO-8601 timestamp.
    pub time: String,
    /// Application facts.
    pub application: ApplicationInfo,
    /// Runtime facts.
    pub runtime: RuntimeInfo,
    /// Harness facts.
    pub dsh: DshInfo,
    /// Workspace facts.
    pub workspace: WorkspaceInfo,
    /// Every individual check.
    pub checks: Vec<Check>,
}

impl HealthReport {
    /// Derive the overall status from the check set.
    ///
    /// Unhealthy if any **critical** check failed; degraded if any
    /// non-critical check failed; healthy otherwise. This is the single place
    /// the rule lives, so `/health`, `/health/ready`, and the UI banner can
    /// never disagree.
    #[must_use]
    pub fn derive_status(checks: &[Check]) -> Status {
        if checks.iter().any(|c| !c.ok && c.critical) {
            Status::Unhealthy
        } else if checks.iter().any(|c| !c.ok) {
            Status::Degraded
        } else {
            Status::Healthy
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_pass_is_healthy() {
        let checks = vec![
            Check::pass("dsh-binary", true, 1),
            Check::pass("workspace-rw", true, 2),
        ];
        assert_eq!(HealthReport::derive_status(&checks), Status::Healthy);
    }

    #[test]
    fn non_critical_failure_degrades() {
        // The first-run case: no model key must not block the UI.
        let checks = vec![
            Check::pass("dsh-binary", true, 1),
            Check::fail("model-credential", false, 1, "no API key configured"),
        ];
        assert_eq!(HealthReport::derive_status(&checks), Status::Degraded);
    }

    #[test]
    fn critical_failure_is_unhealthy() {
        let checks = vec![
            Check::fail("dsh-binary", true, 1, "not found"),
            Check::pass("workspace-rw", true, 2),
        ];
        assert_eq!(HealthReport::derive_status(&checks), Status::Unhealthy);
    }

    #[test]
    fn critical_failure_outranks_a_gentle_one() {
        let checks = vec![
            Check::fail("model-credential", false, 1, "no key"),
            Check::fail("dsh-binary", true, 1, "not found"),
        ];
        assert_eq!(HealthReport::derive_status(&checks), Status::Unhealthy);
    }

    #[test]
    fn empty_check_set_is_healthy() {
        assert_eq!(HealthReport::derive_status(&[]), Status::Healthy);
    }

    #[test]
    fn degraded_is_served_as_200_but_unhealthy_is_not() {
        // This is the rule the launcher depends on.
        assert_eq!(Status::Healthy.http_code(), 200);
        assert_eq!(Status::Degraded.http_code(), 200);
        assert_eq!(Status::Unhealthy.http_code(), 503);
    }
}
