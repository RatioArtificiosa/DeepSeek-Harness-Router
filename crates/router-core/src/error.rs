//! Typed errors with stable codes.
//!
//! Every failure the user can encounter has a stable `code`, a plain-language
//! message, and — where one exists — a remediation hint. This is the
//! "every failure names its cause and its fix" rule from PROPOSAL.md §P-26.1.
//!
//! Codes are part of the public contract: `/health` reports them, the launcher
//! greps for them, and `docs/troubleshooting.md` documents each one.

use std::fmt;

/// A stable, machine-readable error code.
///
/// The string form is what appears in logs, in `/health`, and in the
/// troubleshooting documentation. Renaming one is a breaking change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    // -- Startup and environment ------------------------------------------
    /// Docker is not installed.
    DockerMissing,
    /// Docker is installed but the daemon is unreachable.
    DockerDaemonDown,
    /// Compose v2 is unavailable.
    ComposeMissing,

    // -- Workspace --------------------------------------------------------
    /// No workspace was supplied.
    WorkspaceMissing,
    /// The supplied path does not exist.
    WorkspaceNotFound,
    /// The supplied path exists but is not a directory.
    WorkspaceNotDirectory,
    /// The path is a filesystem root or a system directory.
    WorkspaceDenied,
    /// The path contains a `..` segment after normalization.
    WorkspaceTraversal,
    /// The path is drive-relative (Windows, e.g. `C:foo`).
    WorkspaceDriveRelative,
    /// The path is relative where an absolute path is required.
    WorkspaceRelative,
    /// The workspace exists but is not writable.
    WorkspaceReadOnly,

    // -- Harness runtime --------------------------------------------------
    /// The `dsh` binary is not present in the image.
    DshNotInstalled,
    /// The frontend assets are missing from the image.
    DshFrontendMissing,
    /// The harness home directory is not writable.
    DshHomeUnwritable,
    /// The harness did not become ready within the timeout.
    DshReadyTimeout,
    /// The harness exited unexpectedly.
    DshExited,
    /// No model credential is configured.
    DshMissingCredential,
    /// Process confinement is unavailable on this host.
    DshSandboxUnavailable,
    /// The internal port is already in use.
    DshPortInUse,

    // -- Router -----------------------------------------------------------
    /// Configuration could not be parsed or is invalid.
    ConfigInvalid,
    /// A required environment variable is absent.
    ConfigMissingEnv,
    /// The published port could not be bound.
    PortUnavailable,
    /// The relay could not reach the harness.
    RelayUpstreamUnreachable,
    /// A file or directory operation failed.
    IoFailed,
    /// A required path failed validation.
    PathInvalid,
}

impl ErrorCode {
    /// The stable string form used on the wire and in logs.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DockerMissing => "DOCKER_MISSING",
            Self::DockerDaemonDown => "DOCKER_DAEMON_DOWN",
            Self::ComposeMissing => "COMPOSE_MISSING",
            Self::WorkspaceMissing => "WORKSPACE_MISSING",
            Self::WorkspaceNotFound => "WORKSPACE_NOT_FOUND",
            Self::WorkspaceNotDirectory => "WORKSPACE_NOT_DIRECTORY",
            Self::WorkspaceDenied => "WORKSPACE_DENIED",
            Self::WorkspaceTraversal => "WORKSPACE_TRAVERSAL",
            Self::WorkspaceDriveRelative => "WORKSPACE_DRIVE_RELATIVE",
            Self::WorkspaceRelative => "WORKSPACE_RELATIVE",
            Self::WorkspaceReadOnly => "WORKSPACE_READ_ONLY",
            Self::DshNotInstalled => "DSH_NOT_INSTALLED",
            Self::DshFrontendMissing => "DSH_FRONTEND_MISSING",
            Self::DshHomeUnwritable => "DSH_HOME_UNWRITABLE",
            Self::DshReadyTimeout => "DSH_READY_TIMEOUT",
            Self::DshExited => "DSH_EXITED",
            Self::DshMissingCredential => "DSH_MISSING_CREDENTIAL",
            Self::DshSandboxUnavailable => "DSH_SANDBOX_UNAVAILABLE",
            Self::DshPortInUse => "DSH_PORT_IN_USE",
            Self::ConfigInvalid => "CONFIG_INVALID",
            Self::ConfigMissingEnv => "CONFIG_MISSING_ENV",
            Self::PortUnavailable => "PORT_UNAVAILABLE",
            Self::RelayUpstreamUnreachable => "RELAY_UPSTREAM_UNREACHABLE",
            Self::IoFailed => "IO_FAILED",
            Self::PathInvalid => "PATH_INVALID",
        }
    }

    /// A plain-language remediation shown to the user, when one exists.
    ///
    /// A message without a remedy is an incomplete message (§P-26.1 rule 4).
    #[must_use]
    pub const fn remediation(self) -> Option<&'static str> {
        match self {
            Self::DockerMissing => Some("Install Docker: https://docs.docker.com/get-docker/"),
            Self::DockerDaemonDown => Some(
                "Start Docker Desktop (macOS/Windows) or `sudo systemctl start docker` (Linux).",
            ),
            Self::ComposeMissing => {
                Some("Docker Compose v2 is required. Check with `docker compose version`.")
            }
            Self::WorkspaceMissing => Some("Pass a workspace: ./start.sh /path/to/project"),
            Self::WorkspaceNotFound => Some("Check the path, or create the directory first."),
            Self::WorkspaceNotDirectory => Some("Choose a directory, not a file."),
            Self::WorkspaceDenied => Some(
                "Choose a project folder inside it, not a filesystem root or system directory.",
            ),
            Self::WorkspaceTraversal => Some("The path must not contain '..' after normalization."),
            Self::WorkspaceDriveRelative => {
                Some("Use a rooted path such as C:\\Users\\you\\projects\\app.")
            }
            Self::WorkspaceRelative => Some("Use an absolute path."),
            Self::WorkspaceReadOnly => Some(
                "The workspace is not writable. On Docker Desktop, check file sharing settings.",
            ),
            Self::DshNotInstalled => Some("Rebuild the image: docker compose build --no-cache"),
            Self::DshFrontendMissing => {
                Some("The image is incomplete. Please report this with the image digest.")
            }
            Self::DshHomeUnwritable => {
                Some("The data volume is not writable. See docs/permissions.md.")
            }
            Self::DshReadyTimeout => Some("Run ./start.sh --doctor and check the container logs."),
            Self::DshExited => {
                Some("The runtime stopped unexpectedly. Check `docker compose logs`.")
            }
            Self::DshMissingCredential => Some("Open Settings > Models and add an API key."),
            Self::DshSandboxUnavailable => Some(
                "Confined tools are disabled. See docs/security.md#if-confinement-is-unavailable",
            ),
            Self::DshPortInUse => Some("The next free internal port will be tried."),
            Self::ConfigInvalid => Some("Check .env against .env.example."),
            Self::ConfigMissingEnv => Some("The launcher sets this; re-run it."),
            Self::PortUnavailable => Some("Choose another port with --port."),
            Self::RelayUpstreamUnreachable => {
                Some("The runtime is still starting. Wait, or run --doctor.")
            }
            Self::IoFailed | Self::PathInvalid => None,
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The crate-wide error type.
#[derive(Debug, thiserror::Error)]
pub struct RouterError {
    /// Stable machine-readable code.
    pub code: ErrorCode,
    /// Human-readable detail, including the offending value where relevant.
    pub detail: String,
    /// The underlying cause, when one exists.
    #[source]
    pub source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl RouterError {
    /// Construct an error from a code and a detail message.
    pub fn new(code: ErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
            source: None,
        }
    }

    /// Attach an underlying cause.
    #[must_use]
    pub fn with_source(mut self, source: impl std::error::Error + Send + Sync + 'static) -> Self {
        self.source = Some(Box::new(source));
        self
    }

    /// The remediation hint for this error's code, if any.
    #[must_use]
    pub fn remediation(&self) -> Option<&'static str> {
        self.code.remediation()
    }

    /// Render the full user-facing message: cause, then remedy.
    ///
    /// This is the single place that decides how an error is presented, so
    /// every surface — launcher, log, `/health`, UI banner — stays consistent.
    #[must_use]
    pub fn user_message(&self) -> String {
        match self.remediation() {
            Some(remedy) => format!("{}  ({})", self.detail, remedy),
            None => self.detail.clone(),
        }
    }
}

impl fmt::Display for RouterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.detail)
    }
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, RouterError>;

impl From<std::io::Error> for RouterError {
    fn from(e: std::io::Error) -> Self {
        Self::new(ErrorCode::IoFailed, e.to_string()).with_source(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unique_and_stable() {
        // Every code string must be distinct; a collision would make logs and
        // /health ambiguous.
        let codes = [
            ErrorCode::DockerMissing,
            ErrorCode::DockerDaemonDown,
            ErrorCode::ComposeMissing,
            ErrorCode::WorkspaceMissing,
            ErrorCode::WorkspaceNotFound,
            ErrorCode::WorkspaceNotDirectory,
            ErrorCode::WorkspaceDenied,
            ErrorCode::WorkspaceTraversal,
            ErrorCode::WorkspaceDriveRelative,
            ErrorCode::WorkspaceRelative,
            ErrorCode::WorkspaceReadOnly,
            ErrorCode::DshNotInstalled,
            ErrorCode::DshFrontendMissing,
            ErrorCode::DshHomeUnwritable,
            ErrorCode::DshReadyTimeout,
            ErrorCode::DshExited,
            ErrorCode::DshMissingCredential,
            ErrorCode::DshSandboxUnavailable,
            ErrorCode::DshPortInUse,
            ErrorCode::ConfigInvalid,
            ErrorCode::ConfigMissingEnv,
            ErrorCode::PortUnavailable,
            ErrorCode::RelayUpstreamUnreachable,
            ErrorCode::IoFailed,
            ErrorCode::PathInvalid,
        ];
        let mut seen = std::collections::HashSet::new();
        for c in codes {
            assert!(seen.insert(c.as_str()), "duplicate code string: {c}");
        }
        assert_eq!(seen.len(), codes.len());
    }

    #[test]
    fn user_message_includes_remediation_when_present() {
        let e = RouterError::new(ErrorCode::WorkspaceMissing, "no workspace given");
        let msg = e.user_message();
        assert!(msg.contains("no workspace given"));
        assert!(msg.contains("Pass a workspace"));
    }

    #[test]
    fn user_message_is_detail_only_without_remediation() {
        let e = RouterError::new(ErrorCode::IoFailed, "disk on fire");
        assert_eq!(e.user_message(), "disk on fire");
    }

    #[test]
    fn every_workspace_code_has_a_remediation() {
        // Workspace errors are the ones users hit most; each must guide them.
        for c in [
            ErrorCode::WorkspaceMissing,
            ErrorCode::WorkspaceNotFound,
            ErrorCode::WorkspaceNotDirectory,
            ErrorCode::WorkspaceDenied,
            ErrorCode::WorkspaceTraversal,
            ErrorCode::WorkspaceDriveRelative,
            ErrorCode::WorkspaceRelative,
            ErrorCode::WorkspaceReadOnly,
        ] {
            assert!(c.remediation().is_some(), "no remediation for {c}");
        }
    }
}
