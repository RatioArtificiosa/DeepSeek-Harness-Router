//! Where the router keeps its files.
//!
//! One root, resolved once, with a documented precedence so it can be pointed
//! somewhere else for testing or for a second router instance.

use router_core::error::{ErrorCode, Result, RouterError};
use std::path::{Path, PathBuf};

/// The environment variable that overrides the router home.
pub const HOME_ENV: &str = "DSH_ROUTER_HOME";

/// The directory name used under the operating system's home.
pub const DEFAULT_HOME_DIR: &str = ".deepseek-router";

/// The registry filename inside the router home.
pub const REGISTRY_FILE: &str = "router.yaml";

/// The resolved router home.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouterHome {
    root: PathBuf,
}

impl RouterHome {
    /// Resolve the router home.
    ///
    /// Precedence: an explicit argument, then `DSH_ROUTER_HOME`, then
    /// `<os home>/.deepseek-router`.
    ///
    /// This deliberately mirrors how the harness itself resolves its own home
    /// (`$DSH_HOME`, else `~/.dsh`), because a user who has learned one should
    /// not have to learn a second convention.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::ConfigInvalid`] when no home can be determined —
    /// neither an argument, nor the environment variable, nor an operating
    /// system home directory is available.
    pub fn resolve(explicit: Option<PathBuf>) -> Result<Self> {
        if let Some(path) = explicit {
            return Ok(Self {
                root: normalise(path)?,
            });
        }

        if let Some(value) = std::env::var_os(HOME_ENV) {
            let text = value.to_string_lossy().trim().to_string();
            // A blank override is treated as unset rather than as "the current
            // directory", which would silently scatter router state.
            if !text.is_empty() {
                return Ok(Self {
                    root: normalise(PathBuf::from(text))?,
                });
            }
        }

        let os_home = os_home_dir().ok_or_else(|| {
            RouterError::new(
                ErrorCode::ConfigInvalid,
                "cannot determine your home directory".to_string(),
            )
        })?;

        Ok(Self {
            root: os_home.join(DEFAULT_HOME_DIR),
        })
    }

    /// The router home directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The registry file.
    #[must_use]
    pub fn registry_path(&self) -> PathBuf {
        self.root.join(REGISTRY_FILE)
    }

    /// Where one instance's files live.
    #[must_use]
    pub fn instance_dir(&self, name: &str) -> PathBuf {
        self.root.join("instances").join(name)
    }
}

/// The operating system's home directory.
///
/// `USERPROFILE` first on Windows, `HOME` elsewhere, with a fallback to the
/// other so an unusual environment still resolves.
fn os_home_dir() -> Option<PathBuf> {
    let candidates: &[&str] = if cfg!(windows) {
        &["USERPROFILE", "HOME"]
    } else {
        &["HOME", "USERPROFILE"]
    };

    for name in candidates {
        if let Some(value) = std::env::var_os(name) {
            let text = value.to_string_lossy().trim().to_string();
            if !text.is_empty() {
                return Some(PathBuf::from(text));
            }
        }
    }
    None
}

/// Make a path absolute and collapse `.` and `..` lexically.
///
/// Lexical rather than `realpath`, because the router home may not exist yet —
/// `router init` creates it — and canonicalization would fail on a path that is
/// about to become valid.
fn normalise(path: PathBuf) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map_err(|e| {
                RouterError::new(
                    ErrorCode::IoFailed,
                    format!("cannot read the working directory: {e}"),
                )
            })?
            .join(path)
    };

    use std::path::Component;
    let mut out = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(p) => out.push(p.as_os_str()),
            Component::RootDir => out.push(Component::RootDir.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(seg) => out.push(seg),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_path_wins() {
        let home = RouterHome::resolve(Some(PathBuf::from("/tmp/somewhere"))).unwrap();
        assert!(home.root().ends_with("somewhere"));
    }

    #[test]
    fn default_is_under_the_os_home() {
        if os_home_dir().is_none() {
            return; // nothing to assert on this machine
        }
        let home = RouterHome::resolve(None).unwrap();
        assert!(home.root().is_absolute());
        assert!(home.root().to_string_lossy().contains(DEFAULT_HOME_DIR));
    }

    #[test]
    fn registry_lives_directly_in_the_home() {
        // Registry::home_for derives everything else from this, so the two must
        // agree.
        let home = RouterHome::resolve(Some(PathBuf::from("/tmp/router"))).unwrap();
        assert_eq!(home.registry_path(), home.root().join(REGISTRY_FILE));
        assert_eq!(
            router_core::registry::Registry::home_for(&home.registry_path()),
            home.root().to_path_buf()
        );
    }

    #[test]
    fn instance_dir_is_namespaced_by_name() {
        let home = RouterHome::resolve(Some(PathBuf::from("/tmp/router"))).unwrap();
        assert_ne!(home.instance_dir("a"), home.instance_dir("b"));
        assert!(home.instance_dir("a").starts_with(home.root()));
    }

    #[test]
    fn relative_paths_are_made_absolute() {
        let home = RouterHome::resolve(Some(PathBuf::from("relative-dir"))).unwrap();
        assert!(home.root().is_absolute());
    }

    #[test]
    fn dot_segments_are_collapsed() {
        // `/tmp/a/./b/../c` collapses to `/tmp/a/c`: the `.` vanishes and the
        // `..` cancels `b`.
        let home = RouterHome::resolve(Some(PathBuf::from("/tmp/a/./b/../c"))).unwrap();
        let text = home.root().to_string_lossy().replace('\\', "/");
        assert!(text.ends_with("/tmp/a/c"), "got {text}");
        assert!(!text.contains(".."), "parent segments must be resolved");
        assert!(
            !text.contains("./"),
            "current-directory segments must be dropped"
        );
    }

    #[test]
    fn a_blank_override_falls_back_rather_than_using_the_cwd() {
        // A blank value must never resolve to the current directory, which
        // would scatter router state wherever the user happened to be.
        let previous = std::env::var_os(HOME_ENV);
        std::env::set_var(HOME_ENV, "   ");
        let resolved = RouterHome::resolve(None);
        match previous {
            Some(v) => std::env::set_var(HOME_ENV, v),
            None => std::env::remove_var(HOME_ENV),
        }
        // Either it resolved to the OS home default, or there was no OS home.
        if let Ok(home) = resolved {
            assert!(
                home.root().to_string_lossy().contains(DEFAULT_HOME_DIR),
                "a blank override must not become the cwd: {}",
                home.root().display()
            );
        }
    }
}
