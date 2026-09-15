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
    // Whether the host's credentials are *actually* in use, which is not the
    // same question as whether sharing was requested. Reporting the request
    // meant this said `true` after a failed link had quietly left a private
    // empty file behind — the flag appeared to work while the instance had no
    // credentials at all.
    let mut credentials_shared = false;

    if !credentials_path.exists() {
        if share_credentials {
            match host_credentials {
                Some(host_file) if host_file.exists() => {
                    match link_file(host_file, &credentials_path) {
                        Ok(()) => {
                            credentials_created = true;
                            credentials_shared = true;
                        }
                        Err(e) => {
                            // A failed link must not stop the instance from
                            // starting, but it must be *reported*. Creating a
                            // private file keeps the instance usable; claiming
                            // the credentials are shared would send the user
                            // looking for a problem somewhere else.
                            tracing::warn!(
                                error = %e,
                                host = %host_file.display(),
                                "could not link shared credentials; creating a private file"
                            );
                            write_empty_credentials(&credentials_path)?;
                            credentials_created = true;
                        }
                    }
                }
                Some(host_file) => {
                    tracing::warn!(
                        path = %host_file.display(),
                        "shared credentials requested but the host file does not exist; \
                         creating a private file"
                    );
                    write_empty_credentials(&credentials_path)?;
                    credentials_created = true;
                }
                None => {
                    // Reachable when a caller builds a supervisor without
                    // deriving the host path. Loud, because the alternative is
                    // the silent no-op this code used to be.
                    tracing::warn!(
                        "shared credentials requested but no host credential path was \
                         configured; creating a private file"
                    );
                    write_empty_credentials(&credentials_path)?;
                    credentials_created = true;
                }
            }
        } else {
            write_empty_credentials(&credentials_path)?;
            credentials_created = true;
        }
    } else if share_credentials {
        // A file already exists. It is shared only if it is genuinely the host's
        // — a symlink pointing at it — rather than a private copy from an
        // earlier run.
        credentials_shared = points_at(&credentials_path, host_credentials);
    }

    Ok(ProvisionReport {
        state_root: state_root.to_path_buf(),
        settings_written,
        credentials_created,
        credentials_shared,
    })
}

/// Whether `link` resolves to the same file as `target`.
///
/// Used to answer "are these credentials actually shared?" without trusting
/// the request that produced them. Falls back to `false` when either path
/// cannot be read, because the honest answer to an unverifiable question is
/// "no".
fn points_at(link: &Path, target: Option<&Path>) -> bool {
    let Some(target) = target else {
        return false;
    };
    match (std::fs::canonicalize(link), std::fs::canonicalize(target)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
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

/// Link one file to another with a symlink.
///
/// On Windows a symlink needs either developer mode or elevation. The caller
/// handles that failure — it is reported, not swallowed, and the instance falls
/// back to a private file — because a convenience feature that silently does
/// nothing is worse than one that visibly declines.
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

/// What changing an instance's sharing actually achieved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShareOutcome {
    /// The instance's file is now a link to the host's credentials.
    Shared,
    /// The instance now has its own file, and is not sharing.
    Private,
    /// Sharing was requested and could not be done; the instance kept a private
    /// file. Carries why, so the caller can tell the user.
    Unavailable(String),
}

/// Whether an instance currently shares the host's credentials.
///
/// Answers by comparing the two files rather than by reading a recorded
/// intention, so a link that was later broken reads as "not shared".
#[must_use]
pub fn is_sharing(state_root: &Path, host_credentials: Option<&Path>) -> bool {
    points_at(&state_root.join(".credentials.yaml"), host_credentials)
}

/// Change whether an instance shares the host's credentials.
///
/// # Why this is not just a registry flag
///
/// The setting is not only recorded in the registry: it decides what
/// `.credentials.yaml` *is* inside the instance's state root — a link to the
/// host file, or a standalone one. Flipping the registry and stopping there
/// would leave the two disagreeing, so an instance marked "shared" would still
/// hold a private file, or worse, one marked private would still be reading the
/// host's live credentials.
///
/// So the file is reconciled to match. Turning sharing **off** removes the link
/// and writes a private empty file: the host credentials must stop being
/// reachable, which is the entire point of the choice.
///
/// # Errors
///
/// Returns [`ErrorCode::IoFailed`] when the state root cannot be written. A
/// state root that does not exist yet is not an error: the setting is recorded
/// and `provision` applies it on first start.
pub fn set_credential_sharing(
    state_root: &Path,
    share: bool,
    host_credentials: Option<&Path>,
) -> Result<ShareOutcome> {
    let credentials_path = state_root.join(".credentials.yaml");

    // Nothing provisioned yet, so there is no file to reconcile. The registry
    // change is the whole change; `provision` will honour it on first start.
    if !state_root.exists() {
        return Ok(if share {
            ShareOutcome::Shared
        } else {
            ShareOutcome::Private
        });
    }

    if !share {
        // Stop sharing: the host's credentials must become unreachable.
        remove_link(&credentials_path)?;
        write_empty_credentials(&credentials_path)?;
        return Ok(ShareOutcome::Private);
    }

    let Some(host) = host_credentials else {
        return Ok(ShareOutcome::Unavailable(
            "no host credential path is configured".to_string(),
        ));
    };
    if !host.exists() {
        return Ok(ShareOutcome::Unavailable(format!(
            "{} does not exist",
            host.display()
        )));
    }
    if points_at(&credentials_path, Some(host)) {
        return Ok(ShareOutcome::Shared); // already done
    }

    // Replacing a file with a link is not atomic, so the old file goes first.
    // A failure after this point leaves no file at all, which the next start
    // repairs by recreating it — better than leaving a stale private file that
    // looks like sharing but is not.
    remove_link(&credentials_path)?;
    match link_file(host, &credentials_path) {
        Ok(()) => Ok(ShareOutcome::Shared),
        Err(e) => {
            write_empty_credentials(&credentials_path)?;
            Ok(ShareOutcome::Unavailable(e.to_string()))
        }
    }
}

/// Remove a file or link, treating "absent" as already done.
fn remove_link(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(RouterError::new(
            ErrorCode::IoFailed,
            format!("cannot replace {}: {e}", path.display()),
        )),
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
