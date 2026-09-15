//! The authenticated URL an instance's browser interface is reachable at.
//!
//! # What this solves
//!
//! The harness gates its browser interface behind a token: 32 random bytes,
//! generated in memory when the process starts, printed **once** to its own
//! stdout, and never written anywhere else:
//!
//! ```text
//! dsh web: http://127.0.0.1:3082/?token=<32 random bytes>
//! ```
//!
//! A URL built from the port alone — `http://127.0.0.1:3082` — is answered with
//! `dsh web authentication required; reopen the URL printed by dsh web`. That
//! message is not a broken instance. It is a URL missing its only credential,
//! and it was what `router open` produced.
//!
//! # Why this is a file, and not a field in the registry
//!
//! Two reasons, and both matter:
//!
//! 1. **`router start` and `router open` are different processes.** The
//!    supervisor that reads the token holds it in memory; the command that
//!    needs it does not. Something has to cross that boundary, and a file is the
//!    simplest thing that can.
//!
//! 2. **The registry holds configuration; this is runtime state.** The registry
//!    describes what the user asked for and is written once per change. The URL
//!    changes every time the process starts and is worthless the moment it
//!    stops. Mixing the two would mean rewriting the user's configuration every
//!    time an instance restarts, and would leave a stale credential in a file
//!    they might commit or share.
//!
//! # Lifetime
//!
//! The token is valid exactly as long as the process that minted it. A file left
//! behind by a crash is therefore stale, and [`read`] is only ever consulted
//! after confirming something is genuinely answering on the port — a live
//! listener plus a token that does not match still fails, but a live listener
//! plus no token fails for a reason the user can understand.

use std::path::{Path, PathBuf};

/// Where an instance's current URL is recorded.
///
/// Deliberately *not* under the harness's own state root: this belongs to the
/// router, and writing router bookkeeping into a directory the harness owns
/// would be the kind of mixing this project avoids everywhere else.
#[must_use]
pub fn url_path(instance_dir: &Path) -> PathBuf {
    instance_dir.join("browser-url")
}

/// Record the URL the harness announced.
///
/// # Errors
///
/// Returns the operating system error when the file cannot be written. A caller
/// may reasonably ignore this — the instance still works, the URL is just not
/// discoverable from another process — but it should not be silent about a
/// filesystem that has stopped accepting writes.
pub fn write(instance_dir: &Path, url: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(instance_dir)?;
    std::fs::write(url_path(instance_dir), url.trim())
}

/// Read a previously recorded URL, if one is present and non-empty.
#[must_use]
pub fn read(instance_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(url_path(instance_dir)).ok()?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Remove a recorded URL.
///
/// Called when an instance stops, because the token died with the process and a
/// leftover file would let a later command offer a link that cannot work —
/// which reads as a router bug rather than a stale link.
///
/// A missing file is success: the goal is that no usable URL remains.
pub fn clear(instance_dir: &Path) {
    let _ = std::fs::remove_file(url_path(instance_dir));
}

/// Whether a URL carries the token the harness requires.
///
/// Used to decide whether a recorded URL is worth offering at all. A URL without
/// a token is not a worse link, it is a link that always fails, and offering one
/// would waste the user's time in a way that looks like a fault.
#[must_use]
pub fn has_token(url: &str) -> bool {
    // Matched as a query parameter rather than a substring, so a path or host
    // that happens to contain the word "token" does not count.
    url.split(['?', '&'])
        .skip(1)
        .any(|part| part.split('=').next().is_some_and(|k| k == "token"))
        && url.contains("token=")
        && !url.ends_with("token=")
}

/// Where the PID of the harness this instance started is recorded.
///
/// # Why a PID file, and not a command-line check
///
/// To adopt a listener on a restart, the router must prove the process is the one
/// *it* started. The obvious test — does the command line mention this instance's
/// state root? — does not work: the harness receives its state root through the
/// `DSH_HOME` environment variable, and one process's environment is not readable
/// from another on Windows at all. The command line is therefore identical for
/// every harness on the machine, and cannot distinguish them.
///
/// What the router *can* do is remember. It spawns the child, so it knows the PID
/// at the moment of creation, and a file carries that across process boundaries
/// for the same reason [`url_path`] does.
///
/// A recorded PID is only trusted after being checked for liveness *and* for
/// looking like a harness, so a recycled PID cannot make the router adopt an
/// unrelated process. This is a safety check, not authentication: it prevents an
/// accident, which is the realistic risk.
#[must_use]
pub fn pid_path(instance_dir: &Path) -> PathBuf {
    instance_dir.join("harness-pid")
}

/// Record the PID of the harness this instance started.
///
/// # Errors
///
/// Returns the operating system error when the file cannot be written. A caller
/// may ignore this without harm: the instance still runs, and the only
/// consequence is that a later restart refuses to adopt it rather than adopting
/// it wrongly — which is the safe direction to fail in.
pub fn write_pid(instance_dir: &Path, pid: u32) -> std::io::Result<()> {
    std::fs::create_dir_all(instance_dir)?;
    std::fs::write(pid_path(instance_dir), pid.to_string())
}

/// Read a previously recorded PID, if one is present and parses.
#[must_use]
pub fn read_pid(instance_dir: &Path) -> Option<u32> {
    std::fs::read_to_string(pid_path(instance_dir))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Remove a recorded PID.
///
/// A missing file is success, for the same reason as [`clear`]: the goal is that
/// no usable claim remains.
pub fn clear_pid(instance_dir: &Path) {
    let _ = std::fs::remove_file(pid_path(instance_dir));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_url_is_round_tripped() {
        let dir = tempfile::tempdir().unwrap();
        let url = "http://127.0.0.1:3082/?token=abc123";
        write(dir.path(), url).unwrap();
        assert_eq!(read(dir.path()).as_deref(), Some(url));
    }

    #[test]
    fn surrounding_whitespace_is_ignored() {
        // The URL is scraped from a log line, so a stray newline is the normal
        // case rather than an exceptional one.
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "  http://127.0.0.1:3082/?token=abc  \n").unwrap();
        assert_eq!(
            read(dir.path()).as_deref(),
            Some("http://127.0.0.1:3082/?token=abc")
        );
    }

    #[test]
    fn clearing_removes_the_url() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "http://127.0.0.1:3082/?token=abc").unwrap();
        clear(dir.path());
        assert_eq!(read(dir.path()), None);
    }

    #[test]
    fn clearing_something_absent_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        clear(dir.path()); // must not panic
        assert_eq!(read(dir.path()), None);
    }

    #[test]
    fn an_empty_file_reads_as_no_url() {
        // A crash mid-write can leave an empty file. Treating that as a URL
        // would offer the user a link to nothing.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(url_path(dir.path()), "   \n").unwrap();
        assert_eq!(read(dir.path()), None);
    }

    #[test]
    fn tokens_are_recognised() {
        assert!(has_token("http://127.0.0.1:3082/?token=abc123"));
        assert!(has_token("http://127.0.0.1:3082/?x=1&token=abc"));
    }

    #[test]
    fn urls_without_a_token_are_rejected() {
        // The exact link `router open` used to build, and the reason the
        // browser answered "authentication required".
        assert!(!has_token("http://127.0.0.1:3082"));
        assert!(!has_token("http://127.0.0.1:3082/"));
        assert!(!has_token("http://127.0.0.1:3082/?token="));
        // A path that merely contains the word must not count.
        assert!(!has_token("http://127.0.0.1:3082/token/abc"));
    }
}
