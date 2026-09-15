//! Configuration: what the container reads from its environment.
//!
//! The launcher owns the host-specific values (PROPOSAL.md §P-13); this module
//! is the container's typed view of them. Every field has a documented default
//! so the container can start with nothing but a workspace mounted.

use crate::error::{ErrorCode, Result, RouterError};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Default port the UI is published on, matching the harness default.
pub const DEFAULT_APP_PORT: u16 = 3080;

/// Default loopback port the harness listens on *inside* the container.
///
/// Deliberately different from [`DEFAULT_APP_PORT`]: the relay owns the
/// published port and forwards to this one, which lets the harness keep its
/// safe loopback bind (PROPOSAL.md §P-09).
pub const DEFAULT_DSH_INTERNAL_PORT: u16 = 3081;

/// Where the harness keeps its state inside the container.
///
/// This is what isolates us from any harness installation on the host
/// (PROPOSAL.md §P-05.2).
pub const DEFAULT_DSH_HOME: &str = "/data/dsh";

/// Log verbosity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    /// Errors only.
    Error,
    /// Warnings and above.
    Warn,
    /// Informational and above.
    Info,
    /// Debug detail.
    Debug,
    /// Everything, including per-request traces.
    Trace,
}

impl LogLevel {
    /// Parse from the environment, defaulting to `info`.
    #[must_use]
    pub fn parse(raw: Option<&str>) -> Self {
        match raw.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            Some("error") => Self::Error,
            Some("warn" | "warning") => Self::Warn,
            Some("debug") => Self::Debug,
            Some("trace") => Self::Trace,
            _ => Self::Info,
        }
    }

    /// The `tracing` filter directive for this level.
    #[must_use]
    pub const fn as_filter(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }
}

/// Runtime configuration, assembled from the environment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Host directory mounted at `/workspace`.
    pub workspace_path: PathBuf,
    /// Port the relay publishes inside the container.
    pub app_port: u16,
    /// Loopback port the harness listens on.
    pub dsh_internal_port: u16,
    /// Harness state directory inside the container.
    pub dsh_home: PathBuf,
    /// Path to the `dsh` executable.
    pub dsh_binary: PathBuf,
    /// Log verbosity.
    pub log_level: LogLevel,
    /// Extra authorities the harness should accept (from `TRUSTED_HOSTS`).
    pub trusted_hosts: Vec<String>,
    /// How long to wait for the harness to become ready.
    pub ready_timeout_secs: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            workspace_path: PathBuf::from(crate::workspace::WORKSPACE_MOUNT),
            app_port: DEFAULT_APP_PORT,
            dsh_internal_port: DEFAULT_DSH_INTERNAL_PORT,
            dsh_home: PathBuf::from(DEFAULT_DSH_HOME),
            dsh_binary: PathBuf::from("/opt/dsh/bin/dsh"),
            log_level: LogLevel::Info,
            trusted_hosts: Vec::new(),
            ready_timeout_secs: 120,
        }
    }
}

impl Config {
    /// Read configuration from the process environment.
    ///
    /// Only `WORKSPACE_PATH` is required, and even that has the container
    /// default so the image can be smoke-tested without a launcher.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::ConfigInvalid`] when a value is present but
    /// unparseable. An absent value is never an error — it takes the default —
    /// because a missing variable should not fail a boot that could succeed.
    pub fn from_env() -> Result<Self> {
        let mut cfg = Self::default();

        if let Ok(v) = std::env::var("WORKSPACE_PATH") {
            if !v.trim().is_empty() {
                cfg.workspace_path = PathBuf::from(v);
            }
        }
        if let Ok(v) = std::env::var("DSH_HOME") {
            if !v.trim().is_empty() {
                cfg.dsh_home = PathBuf::from(v);
            }
        }
        if let Ok(v) = std::env::var("DSH_BINARY") {
            if !v.trim().is_empty() {
                cfg.dsh_binary = PathBuf::from(v);
            }
        }
        if let Some(v) = std::env::var("APP_PORT")
            .ok()
            .filter(|s| !s.trim().is_empty())
        {
            cfg.app_port = v.trim().parse().map_err(|_| {
                RouterError::new(
                    ErrorCode::ConfigInvalid,
                    format!("APP_PORT is not a valid port number: '{v}'"),
                )
            })?;
        }
        if let Some(v) = std::env::var("DSH_INTERNAL_PORT")
            .ok()
            .filter(|s| !s.trim().is_empty())
        {
            cfg.dsh_internal_port = v.trim().parse().map_err(|_| {
                RouterError::new(
                    ErrorCode::ConfigInvalid,
                    format!("DSH_INTERNAL_PORT is not a valid port number: '{v}'"),
                )
            })?;
        }
        if let Some(v) = std::env::var("READY_TIMEOUT_SECS")
            .ok()
            .filter(|s| !s.trim().is_empty())
        {
            cfg.ready_timeout_secs = v.trim().parse().map_err(|_| {
                RouterError::new(
                    ErrorCode::ConfigInvalid,
                    format!("READY_TIMEOUT_SECS is not a valid number: '{v}'"),
                )
            })?;
        }

        cfg.log_level = LogLevel::parse(std::env::var("LOG_LEVEL").ok().as_deref());
        cfg.trusted_hosts = std::env::var("TRUSTED_HOSTS")
            .ok()
            .map(|raw| {
                raw.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();

        cfg.validate()?;
        Ok(cfg)
    }

    /// Reject configurations that cannot work.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::ConfigInvalid`] for a zero port, a port collision
    /// between the published and internal listeners, or an empty harness home.
    pub fn validate(&self) -> Result<()> {
        if self.app_port == 0 {
            return Err(RouterError::new(
                ErrorCode::ConfigInvalid,
                "APP_PORT must not be zero",
            ));
        }
        if self.dsh_internal_port == 0 {
            return Err(RouterError::new(
                ErrorCode::ConfigInvalid,
                "DSH_INTERNAL_PORT must not be zero",
            ));
        }
        if self.app_port == self.dsh_internal_port {
            return Err(RouterError::new(
                ErrorCode::ConfigInvalid,
                format!(
                    "APP_PORT and DSH_INTERNAL_PORT are both {}; the relay needs its own port",
                    self.app_port
                ),
            ));
        }
        if self.dsh_home.as_os_str().is_empty() {
            return Err(RouterError::new(
                ErrorCode::ConfigInvalid,
                "DSH_HOME must not be empty",
            ));
        }
        if self.workspace_path.as_os_str().is_empty() {
            return Err(RouterError::new(
                ErrorCode::ConfigInvalid,
                "WORKSPACE_PATH must not be empty",
            ));
        }
        Ok(())
    }

    /// The loopback URL the relay forwards to.
    #[must_use]
    pub fn dsh_loopback_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.dsh_internal_port)
    }

    /// The address the relay binds inside the container.
    #[must_use]
    pub fn relay_bind_addr(&self) -> String {
        format!("0.0.0.0:{}", self.app_port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_internally_consistent() {
        let c = Config::default();
        assert!(c.validate().is_ok());
        assert_ne!(c.app_port, c.dsh_internal_port);
    }

    #[test]
    fn log_level_parsing_is_forgiving() {
        assert_eq!(LogLevel::parse(Some("DEBUG")), LogLevel::Debug);
        assert_eq!(LogLevel::parse(Some(" warn ")), LogLevel::Warn);
        assert_eq!(LogLevel::parse(Some("warning")), LogLevel::Warn);
        assert_eq!(LogLevel::parse(None), LogLevel::Info);
        assert_eq!(LogLevel::parse(Some("nonsense")), LogLevel::Info);
    }

    #[test]
    fn rejects_port_collision_between_relay_and_harness() {
        let c = Config {
            app_port: 5000,
            dsh_internal_port: 5000,
            ..Config::default()
        };
        let e = c.validate().unwrap_err();
        assert_eq!(e.code, ErrorCode::ConfigInvalid);
        assert!(e.detail.contains("5000"));
    }

    #[test]
    fn rejects_zero_ports() {
        let c = Config {
            app_port: 0,
            ..Config::default()
        };
        assert_eq!(c.validate().unwrap_err().code, ErrorCode::ConfigInvalid);
    }

    #[test]
    fn loopback_url_uses_loopback_not_wildcard() {
        // The harness must keep its loopback bind; the relay bridges it.
        let c = Config::default();
        assert!(c.dsh_loopback_url().contains("127.0.0.1"));
        assert!(!c.dsh_loopback_url().contains("0.0.0.0"));
    }

    #[test]
    fn relay_binds_all_interfaces_inside_the_container() {
        let c = Config::default();
        assert!(c.relay_bind_addr().starts_with("0.0.0.0:"));
    }
}
