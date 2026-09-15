//! # router-core
//!
//! Core types for `DeepSeek Harness Router`: typed errors, configuration, the
//! health contract, and workspace path validation.
//!
//! This crate has no dependency on `DeepSeek Harness` itself. Coupling to the
//! harness is confined to `router-dsh`, so a breaking upstream change stays a
//! localized edit rather than a rewrite.
//!
//! ```
//! use router_core::workspace::{validate_workspace, WorkspaceMode};
//!
//! // A filesystem root is refused with an explanation.
//! let err = validate_workspace("/", WorkspaceMode::Existing).unwrap_err();
//! assert_eq!(err.code.as_str(), "WORKSPACE_DENIED");
//! assert!(err.remediation().is_some());
//! ```
#![deny(missing_docs)]
#![deny(clippy::all)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

pub mod config;
pub mod error;
pub mod health;
pub mod ports;
pub mod registry;
pub mod workspace;

pub use config::Config;
pub use error::{ErrorCode, Result, RouterError};
pub use health::{Check, HealthReport, Status};
pub use ports::{allocate, AllocationOutcome, PortClaims};
pub use registry::{Instance, Registry, DEFAULT_BASE_PORT, REGISTRY_VERSION};
pub use workspace::{validate_workspace, WorkspaceMode, WorkspacePath, WORKSPACE_MOUNT};

/// The version of this build, from `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Render an ISO-8601 UTC timestamp for a duration since the Unix epoch.
///
/// Implemented directly rather than pulled in as a dependency: the health
/// endpoint needs one timestamp, and the civil-date algorithm is short enough
/// to test exhaustively.
///
/// Values beyond the representable `i64` day range are clamped rather than
/// wrapping, so an absurd uptime cannot produce a nonsense date.
#[must_use]
pub fn iso8601_from_unix(secs: u64) -> String {
    let days = secs / 86_400;
    let rem_secs = secs % 86_400;
    let (hour, minute, second) = (rem_secs / 3600, (rem_secs % 3600) / 60, rem_secs % 60);

    // Howard Hinnant's civil-from-days algorithm.
    let z = i64::try_from(days)
        .unwrap_or(i64::MAX)
        .saturating_add(719_468);
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year_est = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year_est + 1 } else { year_est };

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso8601_epoch() {
        assert_eq!(iso8601_from_unix(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn iso8601_known_instants() {
        // Values cross-checked against the Unix epoch definition.
        assert_eq!(iso8601_from_unix(1), "1970-01-01T00:00:01Z");
        assert_eq!(iso8601_from_unix(86_400), "1970-01-02T00:00:00Z");
        // 2000-01-01T00:00:00Z
        assert_eq!(iso8601_from_unix(946_684_800), "2000-01-01T00:00:00Z");
        // 2021-01-01T00:00:00Z
        assert_eq!(iso8601_from_unix(1_609_459_200), "2021-01-01T00:00:00Z");
    }

    #[test]
    fn iso8601_handles_leap_day() {
        // 2020-02-29T00:00:00Z — a leap year the naive algorithm gets wrong.
        assert_eq!(iso8601_from_unix(1_582_934_400), "2020-02-29T00:00:00Z");
    }

    #[test]
    fn iso8601_handles_century_non_leap() {
        // 1900 was not a leap year; 2100 is not either.
        // 2100-03-01T00:00:00Z
        assert_eq!(iso8601_from_unix(4_107_542_400), "2100-03-01T00:00:00Z");
    }

    #[test]
    fn version_is_populated() {
        assert!(!VERSION.is_empty());
        assert!(VERSION.contains('.'));
    }
}
