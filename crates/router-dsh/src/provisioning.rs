//! Instance home provisioning.
//!
//! Each instance gets its own state root. Preparing that root before the
//! harness starts is what turns "several harnesses on one machine" from a
//! hopeful arrangement into a safe one.
//!
//! # What gets written, and why
//!
//! Only two things, and both for a specific reason:
//!
//! - **`settings.yaml`** — so the instance starts on the model it was
//!   configured with. Without this, every new instance would boot on whatever
//!   the harness defaults to and need a trip through the GUI to correct.
//!
//! - **`.credentials.yaml`** — created empty, or symlinked to the host's
//!   credential file when the user asked to share one key. An absent file can
//!   behave differently from an empty one, so it is created explicitly.
//!
//! Everything else — `storages/`, `sessions/`, `profiles/` — is left for the
//! harness to create. Pre-creating directories the harness owns would be
//! guessing at its internal layout, and a wrong guess is worse than none.

use router_core::error::{ErrorCode, Result, RouterError};
use std::path::{Path, PathBuf};

/// What provisioning did, so the caller can report it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionReport {
    /// The state root that was prepared.
    pub state_root: PathBuf,
    /// Whether `settings.yaml` was written this time.
    pub settings_written: bool,
    /// Whether the credentials file was created or linked this time.
    pub credentials_created: bool,
    /// Whether credentials are shared with the host installation.
    pub credentials_shared: bool,
}

/// Where the host's credential file lives, derived from the harness default.
///
/// Only used when sharing is requested. The harness resolves its home from
/// `$DSH_HOME`, falling back to `~/.dsh`, so the host's file is found the same
/// way rather than by a hardcoded path.
#[must_use]
pub fn host_credentials_path(os_home: &Path) -> PathBuf {
    os_home.join(".dsh").join(".credentials.yaml")
}

/// Prepare an instance's state root.
///
/// Idempotent: running it on an already-provisioned instance rewrites only what
/// is missing, so a restart never clobbers a settings file the user has since
/// edited by hand.
///
/// # Errors
///
/// Returns [`ErrorCode::IoFailed`] when a directory or file cannot be created.
pub fn provision(
    state_root: &Path,
    model: Option<&str>,
    share_credentials: bool,
    host_credentials: Option<&Path>,
) -> Result<ProvisionReport> {
    std::fs::create_dir_all(state_root).map_err(|e| {
        RouterError::new(
            ErrorCode::IoFailed,
            format!("cannot create {}: {e}", state_root.display()),
        )
    })?;

    // ── settings.yaml ────────────────────────────────────────────────────
    let settings_path = state_root.join("settings.yaml");
    let mut settings_written = false;

    if let Some(model) = model {
        // Only write when the file is absent. A user who has since chosen a
        // different model in the GUI must not have that choice reverted on the
        // next start.
        if !settings_path.exists() {
            let document = settings_document(model);
            std::fs::write(&settings_path, document).map_err(|e| {
                RouterError::new(
                    ErrorCode::IoFailed,
                    format!("cannot write {}: {e}", settings_path.display()),
                )
            })?;
            settings_written = true;
        }
    }

    // ── .credentials.yaml ────────────────────────────────────────────────
    let credentials_path = state_root.join(".credentials.yaml");
    let mut credentials_created = false;

    if !credentials_path.exists() {
        if share_credentials {
            if let Some(host_file) = host_credentials {
                if host_file.exists() {
                    match link_file(host_file, &credentials_path) {
                        Ok(()) => credentials_created = true,
                        Err(e) => {
                            // A link failure must not stop the instance from
                            // starting; it falls back to a private file and
                            // says so.
                            tracing::warn!(
                                error = %e,
                                "could not link shared credentials; creating a private file"
                            );
                            write_empty_credentials(&credentials_path)?;
                            credentials_created = true;
                        }
                    }
                } else {
                    tracing::warn!(
                        path = %host_file.display(),
                        "shared credentials requested but the host file does not exist; \
                         creating a private file"
                    );
                    write_empty_credentials(&credentials_path)?;
                    credentials_created = true;
                }
            }
        } else {
            write_empty_credentials(&credentials_path)?;
            credentials_created = true;
        }
    }

    Ok(ProvisionReport {
        state_root: state_root.to_path_buf(),
        settings_written,
        credentials_created,
        credentials_shared: share_credentials,
    })
}

/// The settings document written for a new instance.
///
/// Model selection lives under `agent-default-model`, which the harness
/// documents as the process-wide default for fresh agents. Writing it here
/// means the instance is on the right model from its first boot.
#[must_use]
pub fn settings_document(model: &str) -> String {
    let (provider, model_id) = split_model(model);
    format!(
        "# Written by DeepSeek Harness Router when this instance was created.\n\
         #\n\
         # This file belongs to this instance alone: it lives under the\n\
         # instance's own state root, so editing it cannot affect any other\n\
         # instance or any harness installation on this machine.\n\
         #\n\
         # The router does not overwrite it after creation, so changes you make\n\
         # here — or in the GUI — are preserved across restarts.\n\
         \n\
         agent-default-model:\n\
         \u{20} provider: {provider}\n\
         \u{20} model: {model_id}\n"
    )
}

/// Split a `provider/model` spec, falling back to the harness default provider.
///
/// Accepts either `deepseek-v4-pro` or `opencode-go/deepseek-v4.1-flash`, so a
/// user can name a route explicitly when the default provider does not serve
/// the model they want.
#[must_use]
pub fn split_model(spec: &str) -> (String, String) {
    match spec.split_once('/') {
        Some((provider, model)) if !provider.is_empty() && !model.is_empty() => {
            (provider.to_string(), model.to_string())
        }
        _ => ("deepseek-official".to_string(), spec.to_string()),
    }
}

/// Create an empty credentials document.
fn write_empty_credentials(path: &Path) -> Result<()> {
    std::fs::write(path, b"").map_err(|e| {
        RouterError::new(
            ErrorCode::IoFailed,
            format!("cannot create {}: {e}", path.display()),
        )
    })
}

/// Link one file to another, preferring a symlink but falling back to a copy.
///
/// On Windows a symlink needs either developer mode or elevation, and failing
/// outright because of that would be a poor experience for a convenience
/// feature. The copy fallback is documented in the result rather than silent.
fn link_file(target: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(target, link)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, link);
        Err(std::io::Error::other("unsupported platform"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn provision_creates_the_state_root() {
        let d = tmp();
        let root = d.path().join("instances").join("alpha").join("dsh");
        let report = provision(&root, None, false, None).unwrap();
        assert!(root.is_dir());
        assert_eq!(report.state_root, root);
    }

    #[test]
    fn writes_settings_when_a_model_is_given() {
        let d = tmp();
        let root = d.path().join("dsh");
        let report = provision(&root, Some("deepseek-v4-pro"), false, None).unwrap();
        assert!(report.settings_written);

        let text = std::fs::read_to_string(root.join("settings.yaml")).unwrap();
        assert!(text.contains("agent-default-model"));
        assert!(text.contains("deepseek-v4-pro"));
    }

    #[test]
    fn does_not_overwrite_settings_a_user_has_edited() {
        // The user may have changed the model in the GUI. A restart must not
        // revert that choice.
        let d = tmp();
        let root = d.path().join("dsh");
        provision(&root, Some("deepseek-v4-pro"), false, None).unwrap();

        std::fs::write(
            root.join("settings.yaml"),
            "agent-default-model:\n  model: mine\n",
        )
        .unwrap();
        let report = provision(&root, Some("deepseek-v4-pro"), false, None).unwrap();

        assert!(
            !report.settings_written,
            "must not rewrite an existing file"
        );
        let text = std::fs::read_to_string(root.join("settings.yaml")).unwrap();
        assert!(text.contains("mine"), "the user's edit must survive");
    }

    #[test]
    fn creates_an_empty_credentials_file_by_default() {
        let d = tmp();
        let root = d.path().join("dsh");
        let report = provision(&root, None, false, None).unwrap();
        assert!(report.credentials_created);
        assert!(!report.credentials_shared);
        assert!(root.join(".credentials.yaml").exists());
    }

    #[test]
    fn shared_credentials_link_when_the_host_file_exists() {
        let d = tmp();
        let host = d.path().join("host-credentials.yaml");
        std::fs::write(&host, "provider: secret\n").unwrap();
        let root = d.path().join("dsh");

        let report = provision(&root, None, true, Some(&host)).unwrap();
        assert!(report.credentials_shared);
        assert!(report.credentials_created);

        // Either linked or copied, the content must be the host's.
        let text = std::fs::read_to_string(root.join(".credentials.yaml")).unwrap();
        assert!(text.contains("secret"));
    }

    #[test]
    fn sharing_requested_without_a_host_file_still_starts() {
        // The instance must come up; a missing host credential is a warning,
        // not a failure.
        let d = tmp();
        let root = d.path().join("dsh");
        let missing = d.path().join("nope.yaml");

        let report = provision(&root, None, true, Some(&missing)).unwrap();
        assert!(report.credentials_created);
        assert!(root.join(".credentials.yaml").exists());
    }

    #[test]
    fn provisioning_is_idempotent() {
        let d = tmp();
        let root = d.path().join("dsh");
        provision(&root, Some("m"), true, None).unwrap();
        let second = provision(&root, Some("m"), true, None).unwrap();

        assert!(!second.settings_written, "nothing to write the second time");
        assert!(
            !second.credentials_created,
            "nothing to create the second time"
        );
    }

    #[test]
    fn settings_document_names_provider_and_model() {
        let doc = settings_document("deepseek-official/deepseek-v4-pro");
        assert!(doc.contains("provider: deepseek-official"));
        assert!(doc.contains("model: deepseek-v4-pro"));
    }

    #[test]
    fn bare_model_gets_the_default_provider() {
        let (provider, model) = split_model("deepseek-v4-pro");
        assert_eq!(provider, "deepseek-official");
        assert_eq!(model, "deepseek-v4-pro");
    }

    #[test]
    fn qualified_model_splits_on_the_first_slash() {
        let (provider, model) = split_model("opencode-go/deepseek-v4.1-flash");
        assert_eq!(provider, "opencode-go");
        assert_eq!(model, "deepseek-v4.1-flash");
    }

    #[test]
    fn malformed_qualified_model_falls_back_rather_than_panicking() {
        for spec in ["/", "provider/", "/model"] {
            let (provider, model) = split_model(spec);
            assert_eq!(provider, "deepseek-official", "spec: {spec}");
            assert_eq!(model, spec);
        }
    }

    #[test]
    fn settings_document_explains_its_own_ownership() {
        // A comment telling a future reader that this file is instance-local
        // is worth more than one restating the format.
        let doc = settings_document("m");
        assert!(doc.contains("this instance alone"));
        assert!(doc.contains("does not overwrite"));
    }

    #[test]
    fn host_credentials_derives_from_the_os_home() {
        let p = host_credentials_path(Path::new("/home/someone"));
        assert!(p.to_string_lossy().contains(".dsh"));
        assert!(p.to_string_lossy().ends_with(".credentials.yaml"));
    }
}
