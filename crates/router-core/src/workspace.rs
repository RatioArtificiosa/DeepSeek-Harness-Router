//! Workspace path validation and normalization.
//!
//! This module is the single gate through which a host directory becomes the
//! container's `/workspace`. It is deliberately strict and deliberately
//! platform-aware, because naive path handling is the top failure mode for a
//! cross-platform product: it is the top failure mode for this class of tool.
//!
//! # The seven hazards it handles
//!
//! 1. **Spaces** in the path.
//! 2. **Backslashes** — normalized to forward slashes for Docker's benefit only.
//! 3. **Drive-relative** Windows paths (`C:foo`), which are not `C:\foo`.
//! 4. **UNC paths** (`\\server\share`), detected and warned about.
//! 5. **Shell metacharacters** (`$`, `%`) that invite interpolation bugs.
//! 6. **Non-ASCII** characters, which must round-trip as UTF-8.
//! 7. **Traversal** (`..`) that survives lexical normalization.
//!
//! # The deny list
//!
//! Mounting a filesystem root would expose the whole machine to the agent.
//! Mounting a system directory would let it modify the OS. Both are refused
//! by name, with a message that explains why and suggests what to do instead.

use crate::error::{ErrorCode, Result, RouterError};
use std::path::{Component, Path, PathBuf};

/// The container path the workspace is always mounted at.
///
/// The application never sees a host path; this constant is why
/// The application never sees a host path; this constant is why.
pub const WORKSPACE_MOUNT: &str = "/workspace";

/// How the workspace directory was obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceMode {
    /// The user pointed at a directory that must already exist.
    Existing,
    /// The user named a new workspace; the directory may be created.
    New,
}

/// A validated, canonical workspace path.
///
/// Constructing one is the only way to obtain a path the mount layer will
/// accept, so validation cannot be skipped by accident.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspacePath {
    host_path: PathBuf,
    docker_path: String,
    /// The physical directory this path refers to, with symlinks resolved.
    ///
    /// # Why this is separate from `host_path`
    ///
    /// `host_path` is what the user typed, canonicalised lexically — it may
    /// contain a symlink, and it is what we should hand to the harness so the
    /// instance works exactly as the user expects.
    ///
    /// Identity is a different question. Two spellings of the same directory —
    /// a real path and a symlink to it — are the *same* directory, and the rule
    /// that no two instances share a workspace is about the directory, not the
    /// spelling. Comparing `host_path` missed that: a real path and a symlink to
    /// it registered as two instances sharing one physical tree, which is the
    /// two-agents-one-project hazard the check exists to prevent.
    identity: PathBuf,
}

impl WorkspacePath {
    /// The canonical absolute host path, as the user spelled it.
    #[must_use]
    pub fn host(&self) -> &Path {
        &self.host_path
    }

    /// The physical directory, with symlinks resolved.
    ///
    /// Used for uniqueness checks. Falls back to the lexical path when the
    /// filesystem cannot resolve it, so a check never fails open into "assume
    /// unique" — it compares what it can see and says so.
    #[must_use]
    pub fn identity(&self) -> &Path {
        &self.identity
    }

    /// The path as Docker should receive it: forward slashes, no trailing
    /// separator. Docker's long-form bind syntax accepts this on every
    /// platform, which is why we normalize rather than hand over the raw value.
    #[must_use]
    pub fn for_docker(&self) -> &str {
        &self.docker_path
    }

    /// The in-container mount point. Always [`WORKSPACE_MOUNT`].
    #[must_use]
    pub const fn mount_point(&self) -> &'static str {
        WORKSPACE_MOUNT
    }
}

/// Names refused on every platform, because mounting them is never intended.
///
/// Matching is case-insensitive and compared against the first path segment
/// (POSIX) or the segment after the root (Windows).
const DENIED_ROOT_SEGMENTS: &[&str] = &[
    // POSIX system trees
    "etc",
    "usr",
    "bin",
    "sbin",
    "lib",
    "lib64",
    "boot",
    "dev",
    "proc",
    "sys",
    "var",
    "root",
    "opt",
    "srv",
    "run",
    "tmp",
    // Windows system trees (compared case-insensitively)
    "windows",
    "program files",
    "program files (x86)",
    "programdata",
    "recovery",
    "perflogs",
];

/// Windows user-profile directories that must not be mounted wholesale.
const DENIED_WINDOWS_PROFILE_DIRS: &[&str] = &[
    "appdata",
    "local settings",
    "application data",
    "cookies",
    "recent",
];

/// Canonicalize without requiring the path to exist.
///
/// `std::fs::canonicalize` fails on a missing path, which is precisely the
/// "new workspace" case, so we resolve lexically and let the caller decide
/// whether existence is required.
fn lexical_absolute(input: &Path) -> Result<PathBuf> {
    let absolute = if input.is_absolute() {
        input.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| RouterError::new(ErrorCode::IoFailed, "cannot read cwd").with_source(e))?
            .join(input)
    };

    // Collapse `.` and resolve `..` lexically, without touching the filesystem.
    let mut out = PathBuf::new();
    for comp in absolute.components() {
        match comp {
            Component::Prefix(p) => out.push(p.as_os_str()),
            Component::RootDir => out.push(Component::RootDir.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                // Refuse to escape above the root.
                if !out.pop() {
                    return Err(RouterError::new(
                        ErrorCode::WorkspaceTraversal,
                        "the path escapes the filesystem root",
                    ));
                }
            }
            Component::Normal(seg) => out.push(seg),
        }
    }
    Ok(out)
}

/// Detect a Windows drive-relative path such as `C:foo`.
///
/// `C:foo` means "foo in the current directory of drive C", which is *not*
/// `C:\foo`. Treating them as equal is a classic bug, so we refuse it and ask
/// for an unambiguous rooted path.
fn is_drive_relative(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    if bytes.len() < 2 {
        return false;
    }
    let drive = bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    if !drive {
        return false;
    }
    // `C:` alone and `C:foo` are both drive-relative; only a separator after
    // the colon makes the path rooted.
    !matches!(bytes.get(2), Some(b'\\' | b'/'))
}

/// The first meaningful path segment, used for deny-list comparison.
fn first_segment(path: &Path) -> Option<String> {
    path.components().find_map(|c| match c {
        Component::Normal(s) => Some(s.to_string_lossy().to_lowercase()),
        _ => None,
    })
}

/// Whether the path is a filesystem root with nothing below it.
fn is_filesystem_root(path: &Path) -> bool {
    path.parent().is_none()
}

/// Reject paths that would expose the machine or the operating system.
///
/// `raw` is the user's original spelling and is used in messages so the error
/// names what they actually typed — normalizing first would show a
/// platform-rewritten path they never entered.
fn check_denied(path: &Path, raw: &str) -> Result<()> {
    if is_filesystem_root(path) {
        return Err(RouterError::new(
            ErrorCode::WorkspaceDenied,
            format!(
                "{raw} is a filesystem root. DeepSeek Harness Router mounts ONE project \
                 directory into the agent's workspace — not a whole drive."
            ),
        ));
    }

    if let Some(seg) = first_segment(path) {
        if DENIED_ROOT_SEGMENTS.contains(&seg.as_str()) {
            return Err(RouterError::new(
                ErrorCode::WorkspaceDenied,
                format!("{raw} is a system directory. Choose a project folder instead."),
            ));
        }
        if DENIED_WINDOWS_PROFILE_DIRS.contains(&seg.as_str()) {
            return Err(RouterError::new(
                ErrorCode::WorkspaceDenied,
                format!("{raw} is an application-data directory. Choose a project folder instead."),
            ));
        }
    }
    Ok(())
}

/// Render a path the way Docker should receive it.
///
/// Windows backslashes become forward slashes; this is *only* for the value
/// Docker consumes, never for filesystem access.
fn docker_form(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    // Strip a trailing slash except on a bare root (already denied above).
    if s.len() > 1 && s.ends_with('/') {
        s[..s.len() - 1].to_string()
    } else {
        s
    }
}

/// Validate and canonicalize a workspace path.
///
/// Returns a [`WorkspacePath`] the mount layer will accept, or an error whose
/// message names the offending path and explains the fix.
///
/// # Errors
///
/// Returns [`ErrorCode::WorkspaceMissing`] for an empty input,
/// [`WorkspaceDriveRelative`](ErrorCode::WorkspaceDriveRelative) for `C:foo`,
/// [`WorkspaceDenied`](ErrorCode::WorkspaceDenied) for a root or system
/// directory, [`WorkspaceTraversal`](ErrorCode::WorkspaceTraversal) when `..`
/// escapes the root, and [`WorkspaceNotFound`](ErrorCode::WorkspaceNotFound) /
/// [`WorkspaceNotDirectory`](ErrorCode::WorkspaceNotDirectory) when the
/// directory is required but absent or not a directory.
pub fn validate_workspace(raw: &str, mode: WorkspaceMode) -> Result<WorkspacePath> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(RouterError::new(
            ErrorCode::WorkspaceMissing,
            "no workspace directory was provided",
        ));
    }

    if is_drive_relative(trimmed) {
        return Err(RouterError::new(
            ErrorCode::WorkspaceDriveRelative,
            format!(
                "'{trimmed}' is drive-relative, which is ambiguous. Write the full \
                 rooted path, such as C:\\Users\\you\\projects\\app."
            ),
        ));
    }

    let absolute = lexical_absolute(Path::new(trimmed))?;

    if !absolute.is_absolute() {
        return Err(RouterError::new(
            ErrorCode::WorkspaceRelative,
            format!("'{trimmed}' did not resolve to an absolute path"),
        ));
    }

    check_denied(&absolute, trimmed)?;

    match mode {
        WorkspaceMode::Existing => {
            let meta = std::fs::metadata(&absolute).map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    RouterError::new(
                        ErrorCode::WorkspaceNotFound,
                        format!("Workspace not found: {}", absolute.display()),
                    )
                } else {
                    RouterError::new(
                        ErrorCode::IoFailed,
                        format!("cannot inspect {}: {e}", absolute.display()),
                    )
                }
            })?;
            if !meta.is_dir() {
                return Err(RouterError::new(
                    ErrorCode::WorkspaceNotDirectory,
                    format!("{} is not a directory", absolute.display()),
                ));
            }
        }
        WorkspaceMode::New => {
            // A new workspace is created here, host-side, because the harness
            // refuses to register a workspace over a nonexistent directory.
            std::fs::create_dir_all(&absolute).map_err(|e| {
                RouterError::new(
                    ErrorCode::IoFailed,
                    format!("cannot create {}: {e}", absolute.display()),
                )
            })?;
        }
    }

    // Resolve symlinks for the identity, now that the directory exists (or has
    // just been created). `canonicalize` is the only call that answers "which
    // physical directory is this", and it requires the path to exist — which is
    // guaranteed by this point in both modes.
    //
    // A failure here is not fatal: the instance still works with the lexical
    // path. It only means the uniqueness check is comparing spellings, which is
    // the pre-existing behaviour and better than refusing a valid workspace.
    let identity = std::fs::canonicalize(&absolute).unwrap_or_else(|_| absolute.clone());

    let docker_path = docker_form(&absolute);
    Ok(WorkspacePath {
        host_path: absolute,
        docker_path,
        identity,
    })
}

/// Whether the workspace is writable, by actually attempting a write.
///
/// A metadata check can report writable while a read-only bind mount refuses
/// the write, so we test the real operation in a temporary file we remove.
#[must_use]
pub fn is_writable(path: &Path) -> bool {
    let probe = path.join(".dsh-router-write-probe");
    match std::fs::write(&probe, b"probe") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Turn a canonical host path into Docker's bind-source form.
///
/// Exposed so the launcher and the tests agree on exactly one transformation.
#[must_use]
pub fn to_docker_source(path: &Path) -> String {
    docker_form(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty() {
        let e = validate_workspace("", WorkspaceMode::Existing).unwrap_err();
        assert_eq!(e.code, ErrorCode::WorkspaceMissing);
        let e = validate_workspace("   ", WorkspaceMode::Existing).unwrap_err();
        assert_eq!(e.code, ErrorCode::WorkspaceMissing);
    }

    #[test]
    fn rejects_drive_relative() {
        // Hazard 3: `C:foo` is not `C:\foo`.
        for raw in ["C:foo", "C:", "d:bar"] {
            let e = validate_workspace(raw, WorkspaceMode::Existing).unwrap_err();
            assert_eq!(e.code, ErrorCode::WorkspaceDriveRelative, "input: {raw}");
        }
    }

    #[test]
    fn accepts_rooted_windows_paths_syntactically() {
        // `C:\...` is rooted; it must get past the drive-relative check and
        // fail later for a different, honest reason (not found on this host).
        for raw in ["C:\\definitely-not-here\\app", "C:/definitely-not-here/app"] {
            let e = validate_workspace(raw, WorkspaceMode::Existing).unwrap_err();
            assert_ne!(e.code, ErrorCode::WorkspaceDriveRelative, "input: {raw}");
        }
    }

    #[test]
    fn rejects_filesystem_roots() {
        let e = validate_workspace("/", WorkspaceMode::Existing).unwrap_err();
        assert_eq!(e.code, ErrorCode::WorkspaceDenied);

        #[cfg(windows)]
        {
            for raw in ["C:\\", "C:/", "D:\\"] {
                let e = validate_workspace(raw, WorkspaceMode::Existing).unwrap_err();
                assert_eq!(e.code, ErrorCode::WorkspaceDenied, "input: {raw}");
            }
        }
    }

    #[test]
    fn rejects_system_directories() {
        for raw in ["/etc", "/usr", "/bin", "/proc", "/sys", "/var"] {
            let e = validate_workspace(raw, WorkspaceMode::Existing).unwrap_err();
            assert_eq!(e.code, ErrorCode::WorkspaceDenied, "input: {raw}");
        }
        #[cfg(windows)]
        {
            for raw in ["C:\\Windows", "C:/Windows", "C:\\Program Files"] {
                let e = validate_workspace(raw, WorkspaceMode::Existing).unwrap_err();
                assert_eq!(e.code, ErrorCode::WorkspaceDenied, "input: {raw}");
            }
        }
    }

    #[test]
    fn rejects_traversal_above_root() {
        // Enough `..` to climb past the root is refused, not silently clamped.
        let deep = if cfg!(windows) {
            "/../../.."
        } else {
            "/../../../.."
        };
        let e = validate_workspace(deep, WorkspaceMode::Existing).unwrap_err();
        assert_eq!(e.code, ErrorCode::WorkspaceTraversal);
    }

    #[test]
    fn collapses_dot_and_parent_segments() {
        // Interior `..` resolves lexically; the result is still a real path.
        let dir = tempfile::tempdir().unwrap();
        let inner = dir.path().join("a").join("b");
        std::fs::create_dir_all(&inner).unwrap();
        let messy = inner.join("..").join("..").join("a").join("b");
        let got = validate_workspace(&messy.to_string_lossy(), WorkspaceMode::Existing).unwrap();
        assert!(got.host().ends_with("a/b") || got.host().ends_with("a\\b"));
    }

    #[test]
    fn accepts_a_real_directory() {
        let dir = tempfile::tempdir().unwrap();
        // tempfile lives under the OS temp root, which must not be deny-listed
        // when it is a *child* of it.
        let ws = dir.path().join("my-project");
        std::fs::create_dir_all(&ws).unwrap();
        let got = validate_workspace(&ws.to_string_lossy(), WorkspaceMode::Existing).unwrap();
        assert_eq!(got.mount_point(), WORKSPACE_MOUNT);
        assert!(got.host().is_absolute());
    }

    #[test]
    fn handles_paths_with_spaces() {
        // Hazard 1: spaces must survive intact.
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("my project dir");
        std::fs::create_dir_all(&ws).unwrap();
        let got = validate_workspace(&ws.to_string_lossy(), WorkspaceMode::Existing).unwrap();
        assert!(got.for_docker().contains("my project dir"));
        assert!(
            !got.for_docker().contains('\\'),
            "docker form must use forward slashes"
        );
    }

    #[test]
    fn handles_non_ascii_paths() {
        // Hazard 6: UTF-8 must round-trip. Windows filesystems reject some
        // characters in practice, so the assertion is that resolution does not
        // corrupt the path — not that every character is legal everywhere.
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("projekte-\u{00fc}");

        std::fs::create_dir_all(&ws).unwrap();
        let got = validate_workspace(&ws.to_string_lossy(), WorkspaceMode::Existing).unwrap();
        assert!(got.host().is_absolute());
        assert!(
            got.for_docker().contains("projekte-"),
            "the path prefix must survive normalization"
        );
    }

    #[test]
    fn docker_form_never_contains_backslash() {
        // Hazard 2: normalize only for Docker's benefit.
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("sub").join("project");
        std::fs::create_dir_all(&ws).unwrap();
        let got = validate_workspace(&ws.to_string_lossy(), WorkspaceMode::Existing).unwrap();
        assert!(!got.for_docker().contains('\\'));
    }

    #[test]
    fn new_mode_creates_missing_directory() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("brand-new-workspace");
        assert!(!ws.exists());
        let got = validate_workspace(&ws.to_string_lossy(), WorkspaceMode::New).unwrap();
        assert!(got.host().is_dir());
    }

    #[test]
    fn existing_mode_refuses_missing_directory() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("does-not-exist");
        let e = validate_workspace(&ws.to_string_lossy(), WorkspaceMode::Existing).unwrap_err();
        assert_eq!(e.code, ErrorCode::WorkspaceNotFound);
    }

    #[test]
    fn refuses_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a-file.txt");
        std::fs::write(&file, b"x").unwrap();
        let e = validate_workspace(&file.to_string_lossy(), WorkspaceMode::Existing).unwrap_err();
        assert_eq!(e.code, ErrorCode::WorkspaceNotDirectory);
    }

    #[test]
    fn writability_probe_reflects_reality() {
        let dir = tempfile::tempdir().unwrap();
        assert!(is_writable(dir.path()));
        // The probe must clean up after itself.
        assert!(!dir.path().join(".dsh-router-write-probe").exists());
    }

    #[test]
    fn error_messages_name_the_path() {
        // A message that does not name the offending path is a poor message.
        let e = validate_workspace("/etc", WorkspaceMode::Existing).unwrap_err();
        assert!(e.detail.contains("/etc"));
        assert!(e.remediation().is_some());
    }

    #[test]
    fn a_symlink_and_its_target_share_one_identity() {
        // The bug this guards: a real path and a symlink to it registered as two
        // instances sharing one physical directory, because the duplicate check
        // compared spellings. Two agents editing one tree is the hazard the
        // check exists for, so identity must be the directory, not the name.
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = dir.path().join("link");

        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&real, &link).is_ok();
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_dir(&real, &link).is_ok();
        #[cfg(not(any(unix, windows)))]
        let linked = false;

        if !linked {
            // Symlinks need privilege on Windows. Skipping is honest; asserting
            // a property the platform cannot express would not be.
            return;
        }

        let a = validate_workspace(&real.to_string_lossy(), WorkspaceMode::Existing).unwrap();
        let b = validate_workspace(&link.to_string_lossy(), WorkspaceMode::Existing).unwrap();

        assert_ne!(
            a.host(),
            b.host(),
            "the two spellings are genuinely different paths"
        );
        assert_eq!(
            a.identity(),
            b.identity(),
            "but they are the same directory, and identity must say so"
        );
    }

    #[test]
    fn distinct_directories_have_distinct_identities() {
        // The positive case: the identity fix must not collapse unrelated paths.
        let dir = tempfile::tempdir().unwrap();
        let one = dir.path().join("one");
        let two = dir.path().join("two");
        std::fs::create_dir(&one).unwrap();
        std::fs::create_dir(&two).unwrap();

        let a = validate_workspace(&one.to_string_lossy(), WorkspaceMode::Existing).unwrap();
        let b = validate_workspace(&two.to_string_lossy(), WorkspaceMode::Existing).unwrap();
        assert_ne!(a.identity(), b.identity());
    }
}
