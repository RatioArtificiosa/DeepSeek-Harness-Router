//! The instance registry: what the router knows about the instances it manages.
//!
//! Stored as a single human-readable YAML document under the router home. It is
//! deliberately inspectable and hand-editable, because a supervisor that hides
//! its own state is a supervisor you cannot debug at 2am.
//!
//! # Why writes are atomic
//!
//! The harness this project supervises has no cross-process write locking, and
//! its own documentation is blunt about the consequence: two processes writing
//! the same file produce *"last-completion wins"*. The router must not repeat
//! that mistake. Every write goes to a temporary file, is flushed to disk, and
//! is then renamed over the target, so a crash leaves either the old document
//! or the new one — never a half-written one.

use crate::error::{ErrorCode, Result, RouterError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The current registry schema version.
///
/// Bumped when the shape changes in a way an older build could not read. The
/// version is checked on load so a future change can migrate rather than
/// silently misreading.
pub const REGISTRY_VERSION: u32 = 1;

/// The lowest port the router will allocate.
///
/// 3080 is the harness default and is very likely held by the installation this
/// router is meant to coexist with, so allocation starts one above it.
pub const DEFAULT_BASE_PORT: u16 = 3081;

/// One managed instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instance {
    /// The project directory this instance's agent works in.
    ///
    /// Stored canonicalized, so two entries differing only by a symlink or a
    /// trailing slash compare equal.
    pub workspace: PathBuf,

    /// The port this instance has been assigned.
    ///
    /// Remembered across restarts so a bookmarked URL keeps working. The router
    /// re-verifies the port is free before use and reassigns only when it is
    /// not, reporting that it did.
    pub port: u16,

    /// The model route this instance should start on.
    ///
    /// Written into the instance's own settings before its first boot, so the
    /// model is set without anyone opening a GUI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Whether this instance starts with the router.
    #[serde(default)]
    pub autostart: bool,

    /// Whether this instance shares the host's credential file.
    ///
    /// Off by default: a shared credential file is a whole-file rewrite, and it
    /// is the file a user would least like to lose to a race.
    #[serde(default)]
    pub share_credentials: bool,

    /// Extra environment variables for the instance's process.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,

    /// The process id of the running harness, when known.
    ///
    /// Recorded so the router can find orphans after an unclean shutdown. A
    /// pid is a hint, never proof — the process may have exited and its id been
    /// reused — so it is always verified against the running process's command
    /// line before being acted on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_pid: Option<u32>,
}

impl Instance {
    /// A new instance on the given workspace and port.
    #[must_use]
    pub fn new(workspace: PathBuf, port: u16) -> Self {
        Self {
            workspace,
            port,
            model: None,
            autostart: false,
            share_credentials: false,
            env: BTreeMap::new(),
            last_pid: None,
        }
    }
}

/// The registry document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registry {
    /// Schema version, checked on load.
    pub version: u32,

    /// The lowest port allocation may use.
    #[serde(default = "default_base_port")]
    pub base_port: u16,

    /// Instances, keyed by name.
    ///
    /// A `BTreeMap` rather than a `HashMap` so the serialized document has a
    /// stable key order: a registry that reshuffles itself on every write would
    /// produce meaningless diffs.
    #[serde(default)]
    pub instances: BTreeMap<String, Instance>,
}

const fn default_base_port() -> u16 {
    DEFAULT_BASE_PORT
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            version: REGISTRY_VERSION,
            base_port: DEFAULT_BASE_PORT,
            instances: BTreeMap::new(),
        }
    }
}

/// Whether a name is safe to use as an instance identifier.
///
/// The name becomes a directory under the router home, so it must not be able
/// to escape that home or collide with a reserved filesystem entry.
#[must_use]
pub fn is_valid_instance_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 64 {
        return false;
    }
    // Reject anything that is only dots, which would mean `.` or `..`.
    if name.chars().all(|c| c == '.') {
        return false;
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap_or(' ');
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

impl Registry {
    /// The router home that a registry path implies.
    ///
    /// The registry lives directly inside the router home, so everything else
    /// the router owns is derived from the same root.
    #[must_use]
    pub fn home_for(registry_path: &Path) -> PathBuf {
        registry_path
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
    }

    /// Where an instance's state root lives.
    ///
    /// This is the directory passed to the harness as `DSH_HOME`, and it is the
    /// single mechanism that keeps instances from sharing state.
    #[must_use]
    pub fn state_root(home: &Path, name: &str) -> PathBuf {
        home.join("instances").join(name).join("dsh")
    }

    /// Load a registry from disk.
    ///
    /// A missing file is an empty registry rather than an error, because the
    /// absence of a registry and a registry with no instances describe the same
    /// situation and callers should not have to distinguish them.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::ConfigInvalid`] when the document is malformed,
    /// carries an unsupported version, or contains an invalid instance name.
    pub fn load(path: &Path) -> Result<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => {
                return Err(RouterError::new(
                    ErrorCode::IoFailed,
                    format!("cannot read {}: {e}", path.display()),
                ))
            }
        };

        if text.trim().is_empty() {
            return Ok(Self::default());
        }

        let registry: Self = serde_yaml::from_str(&text).map_err(|e| {
            RouterError::new(
                ErrorCode::ConfigInvalid,
                format!("{} is not a valid registry: {e}", path.display()),
            )
        })?;

        registry.validate()?;
        Ok(registry)
    }

    /// Save the registry atomically.
    ///
    /// Writes to a sibling temporary file, flushes it, then renames over the
    /// target. A reader therefore always sees a complete document.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::IoFailed`] if the document cannot be written, and
    /// [`ErrorCode::ConfigInvalid`] if the registry is internally inconsistent.
    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;

        let home = Self::home_for(path);
        std::fs::create_dir_all(&home).map_err(|e| {
            RouterError::new(
                ErrorCode::IoFailed,
                format!("cannot create {}: {e}", home.display()),
            )
        })?;

        let text = serde_yaml::to_string(self).map_err(|e| {
            RouterError::new(ErrorCode::ConfigInvalid, format!("cannot serialize: {e}"))
        })?;

        // A sibling temp file guarantees the rename stays on one filesystem.
        let temp = path.with_extension("yaml.tmp");
        {
            use std::io::Write as _;
            let mut file = std::fs::File::create(&temp).map_err(|e| {
                RouterError::new(
                    ErrorCode::IoFailed,
                    format!("cannot create {}: {e}", temp.display()),
                )
            })?;
            file.write_all(text.as_bytes()).map_err(|e| {
                RouterError::new(
                    ErrorCode::IoFailed,
                    format!("cannot write {}: {e}", temp.display()),
                )
            })?;
            // Flush to the device before the rename, so the rename cannot
            // publish a name whose contents have not landed yet.
            file.sync_all().map_err(|e| {
                RouterError::new(
                    ErrorCode::IoFailed,
                    format!("cannot flush {}: {e}", temp.display()),
                )
            })?;
        }

        std::fs::rename(&temp, path).map_err(|e| {
            let _ = std::fs::remove_file(&temp);
            RouterError::new(
                ErrorCode::IoFailed,
                format!("cannot replace {}: {e}", path.display()),
            )
        })?;

        Ok(())
    }

    /// Check that the registry is internally consistent.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::ConfigInvalid`] for an unsupported version, an
    /// invalid instance name, a zero port, or a duplicated port or workspace.
    pub fn validate(&self) -> Result<()> {
        if self.version != REGISTRY_VERSION {
            return Err(RouterError::new(
                ErrorCode::ConfigInvalid,
                format!(
                    "registry version {} is not supported by this build (expected {})",
                    self.version, REGISTRY_VERSION
                ),
            ));
        }

        let mut ports: BTreeMap<u16, &str> = BTreeMap::new();
        let mut workspaces: BTreeMap<&Path, &str> = BTreeMap::new();

        for (name, instance) in &self.instances {
            if !is_valid_instance_name(name) {
                return Err(RouterError::new(
                    ErrorCode::ConfigInvalid,
                    format!(
                        "'{name}' is not a valid instance name: use letters, digits, \
                         '-', '_' or '.', starting with a letter or digit"
                    ),
                ));
            }
            if instance.port == 0 {
                return Err(RouterError::new(
                    ErrorCode::ConfigInvalid,
                    format!("instance '{name}' has no port"),
                ));
            }

            if let Some(other) = ports.insert(instance.port, name) {
                return Err(RouterError::new(
                    ErrorCode::ConfigInvalid,
                    format!(
                        "instances '{other}' and '{name}' both claim port {}; \
                         each instance needs its own port",
                        instance.port
                    ),
                ));
            }

            // Two instances on one workspace means two agents editing one tree.
            // That is a real hazard, so it is refused at load rather than
            // discovered later by a confused user.
            if let Some(other) = workspaces.insert(instance.workspace.as_path(), name) {
                return Err(RouterError::new(
                    ErrorCode::ConfigInvalid,
                    format!(
                        "instances '{other}' and '{name}' both point at {}; \
                         two agents editing one project will conflict",
                        instance.workspace.display()
                    ),
                ));
            }
        }

        Ok(())
    }

    /// Whether an instance by this name exists.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.instances.contains_key(name)
    }

    /// Look up an instance.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Instance> {
        self.instances.get(name)
    }

    /// Every instance name, in stable order.
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        self.instances.keys().map(String::as_str).collect()
    }

    /// Add an instance, rejecting a duplicate name.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::ConfigInvalid`] for an invalid or duplicate name.
    pub fn insert(&mut self, name: &str, instance: Instance) -> Result<()> {
        if !is_valid_instance_name(name) {
            return Err(RouterError::new(
                ErrorCode::ConfigInvalid,
                format!("'{name}' is not a valid instance name"),
            ));
        }
        if self.instances.contains_key(name) {
            return Err(RouterError::new(
                ErrorCode::ConfigInvalid,
                format!("an instance named '{name}' already exists"),
            ));
        }
        self.instances.insert(name.to_string(), instance);
        Ok(())
    }

    /// Remove an instance.
    ///
    /// Returns the removed entry, or `None` if the name was unknown.
    ///
    /// **This never touches the workspace directory.** A path a user handed us
    /// is not ours to delete.
    pub fn remove(&mut self, name: &str) -> Option<Instance> {
        self.instances.remove(name)
    }

    /// The set of ports currently claimed by registered instances.
    #[must_use]
    pub fn claimed_ports(&self) -> Vec<u16> {
        self.instances.values().map(|i| i.port).collect()
    }

    /// The workspace paths currently in use.
    #[must_use]
    pub fn claimed_workspaces(&self) -> Vec<&Path> {
        self.instances
            .values()
            .map(|i| i.workspace.as_path())
            .collect()
    }

    /// Find an instance by workspace path.
    ///
    /// Used to detect a second instance being pointed at a directory that is
    /// already in use.
    #[must_use]
    pub fn find_by_workspace(&self, workspace: &Path) -> Option<(&str, &Instance)> {
        self.instances
            .iter()
            .find(|(_, i)| i.workspace == workspace)
            .map(|(n, i)| (n.as_str(), i))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_registry() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("router.yaml");
        (dir, path)
    }

    #[test]
    fn default_starts_at_3081_never_3080() {
        // The owner's existing install holds 3080; the router must not take it.
        let r = Registry::default();
        assert_eq!(r.base_port, DEFAULT_BASE_PORT);
        assert_eq!(r.base_port, 3081);
    }

    #[test]
    fn missing_file_loads_as_empty_registry() {
        let (_d, path) = tmp_registry();
        let r = Registry::load(&path).unwrap();
        assert!(r.instances.is_empty());
        assert_eq!(r.version, REGISTRY_VERSION);
    }

    #[test]
    fn round_trips_through_disk() {
        let (_d, path) = tmp_registry();
        let mut r = Registry::default();
        let mut inst = Instance::new(PathBuf::from("/tmp/project-a"), 3081);
        inst.model = Some("deepseek-v4-pro".into());
        r.insert("alpha", inst).unwrap();
        r.save(&path).unwrap();

        let back = Registry::load(&path).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn save_is_atomic_leaving_no_temp_file() {
        let (_d, path) = tmp_registry();
        let mut r = Registry::default();
        r.insert("a", Instance::new(PathBuf::from("/tmp/a"), 3081))
            .unwrap();
        r.save(&path).unwrap();

        assert!(path.exists());
        assert!(
            !path.with_extension("yaml.tmp").exists(),
            "the temporary file must not survive a successful save"
        );
    }

    #[test]
    fn save_creates_the_home_directory() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("brand-new").join("router.yaml");
        let r = Registry::default();
        r.save(&nested).unwrap();
        assert!(nested.exists());
    }

    #[test]
    fn malformed_document_fails_loudly() {
        let (_d, path) = tmp_registry();
        std::fs::write(&path, "this: [is: not, a: registry").unwrap();
        let e = Registry::load(&path).unwrap_err();
        assert_eq!(e.code, ErrorCode::ConfigInvalid);
    }

    #[test]
    fn future_version_is_refused_not_misread() {
        let (_d, path) = tmp_registry();
        std::fs::write(&path, "version: 99\ninstances: {}\n").unwrap();
        let e = Registry::load(&path).unwrap_err();
        assert_eq!(e.code, ErrorCode::ConfigInvalid);
        assert!(e.detail.contains("99"));
    }

    #[test]
    fn duplicate_port_is_rejected() {
        let mut r = Registry::default();
        r.insert("a", Instance::new(PathBuf::from("/tmp/a"), 3081))
            .unwrap();
        r.insert("b", Instance::new(PathBuf::from("/tmp/b"), 3081))
            .unwrap();
        let e = r.validate().unwrap_err();
        assert_eq!(e.code, ErrorCode::ConfigInvalid);
        assert!(e.detail.contains("3081"));
    }

    #[test]
    fn two_instances_on_one_workspace_are_rejected() {
        // The whole point of the product: two agents on one tree must not
        // happen silently.
        let mut r = Registry::default();
        r.insert("a", Instance::new(PathBuf::from("/tmp/shared"), 3081))
            .unwrap();
        r.insert("b", Instance::new(PathBuf::from("/tmp/shared"), 3082))
            .unwrap();
        let e = r.validate().unwrap_err();
        assert_eq!(e.code, ErrorCode::ConfigInvalid);
        assert!(e.detail.contains("shared"));
    }

    #[test]
    fn find_by_workspace_detects_a_repeat() {
        let mut r = Registry::default();
        r.insert("a", Instance::new(PathBuf::from("/tmp/one"), 3081))
            .unwrap();
        assert!(r.find_by_workspace(Path::new("/tmp/one")).is_some());
        assert!(r.find_by_workspace(Path::new("/tmp/two")).is_none());
    }

    #[test]
    fn duplicate_name_is_rejected() {
        let mut r = Registry::default();
        r.insert("a", Instance::new(PathBuf::from("/tmp/a"), 3081))
            .unwrap();
        let e = r
            .insert("a", Instance::new(PathBuf::from("/tmp/b"), 3082))
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::ConfigInvalid);
    }

    #[test]
    fn removing_never_touches_the_workspace() {
        // A path the user handed us is not ours to delete.
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("project");
        std::fs::create_dir_all(&ws).unwrap();

        let mut r = Registry::default();
        r.insert("a", Instance::new(ws.clone(), 3081)).unwrap();
        let removed = r.remove("a").unwrap();

        assert_eq!(removed.workspace, ws);
        assert!(ws.is_dir(), "the workspace must survive deregistration");
    }

    #[test]
    fn state_root_is_per_instance() {
        let home = Path::new("/router");
        let a = Registry::state_root(home, "alpha");
        let b = Registry::state_root(home, "beta");
        assert_ne!(a, b, "each instance needs its own state root");
        assert!(a.starts_with(home));
        // Compared by components rather than a string suffix, because the
        // separator differs between platforms.
        assert_eq!(
            a.components().next_back(),
            Some(std::path::Component::Normal("dsh".as_ref()))
        );
        assert!(a.to_string_lossy().contains("alpha"));
    }

    #[test]
    fn instance_names_are_restricted() {
        for good in ["a", "my-project", "proj_1", "a.b", "Alpha9"] {
            assert!(is_valid_instance_name(good), "should accept {good}");
        }
        for bad in [
            "", "..", ".", "-leading", "_leading", ".hidden", "a/b", "a\\b", "a b", "café",
        ] {
            assert!(!is_valid_instance_name(bad), "should reject {bad:?}");
        }
    }

    #[test]
    fn name_length_is_bounded() {
        assert!(is_valid_instance_name(&"a".repeat(64)));
        assert!(!is_valid_instance_name(&"a".repeat(65)));
    }

    #[test]
    fn invented_name_in_document_is_rejected_on_load() {
        let (_d, path) = tmp_registry();
        std::fs::write(
            &path,
            "version: 1\ninstances:\n  \"../escape\":\n    workspace: /tmp/x\n    port: 3081\n",
        )
        .unwrap();
        let e = Registry::load(&path).unwrap_err();
        assert_eq!(e.code, ErrorCode::ConfigInvalid);
    }

    #[test]
    fn serialized_order_is_stable() {
        // A registry that reshuffles on every write produces meaningless diffs.
        let mut r = Registry::default();
        for name in ["zeta", "alpha", "mid"] {
            let port = 3081 + u16::try_from(r.instances.len()).unwrap_or(0);
            r.insert(
                name,
                Instance::new(PathBuf::from(format!("/tmp/{name}")), port),
            )
            .unwrap();
        }
        let first = serde_yaml::to_string(&r).unwrap();
        let second = serde_yaml::to_string(&r).unwrap();
        assert_eq!(first, second);
        let alpha_at = first.find("alpha").unwrap();
        let zeta_at = first.find("zeta").unwrap();
        assert!(alpha_at < zeta_at, "keys must be in stable sorted order");
    }

    #[test]
    fn home_for_derives_from_the_registry_path() {
        let p = Path::new("/home/user/.deepseek-router/router.yaml");
        assert_eq!(
            Registry::home_for(p),
            PathBuf::from("/home/user/.deepseek-router")
        );
    }
}
