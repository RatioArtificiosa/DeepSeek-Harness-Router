//! The acceptance test for the whole design.
//!
//! # What is being proven
//!
//! This project exists because a second DeepSeek Harness on one machine, using
//! the default state root, shares mutable state with the first — and the harness
//! has **no cross-process write locking**. Its own documentation states that two
//! processes writing the same unit produce *"last-completion wins"* behaviour
//! and that a cross-process session lease is not yet implemented.
//!
//! The product claim is therefore narrow and testable: **each instance gets its
//! own state root, so nothing one instance does can be observed in another.**
//!
//! The test below states that claim as an assertion about bytes. It is
//! deliberately phrased as byte equality rather than "the file looks unchanged",
//! because a JSON file can be rewritten with identical content and still have
//! been written — which is proof the lock is missing, not proof of isolation.
//! Byte equality after a *write to a different instance* is the strongest
//! statement available at this level, and it is the one that would fail if a
//! future change made two instances share a root.
//!
//! # Why this test needs a real harness
//!
//! A unit test with a fake harness would assert that our own code writes to two
//! directories — true by construction, and worthless. The risk being guarded
//! against lives in the harness: an environment variable it ignores, a state
//! root it resolves differently than expected, a path it caches at startup.
//! Only the real binary can be wrong in those ways.
//!
//! The test skips itself when no harness is on `PATH`, so a checkout without
//! one still runs green. A skipped test is honest; a silently-passing fake is
//! not.

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;
use support::sandbox::{TEST_PORT_BASE, TEST_PORT_SPAN};

/// Locate the harness, or report that this test cannot run here.
///
/// `DSH_BIN` overrides, which is how CI points at a specific build.
///
/// On Windows the installed `dsh` is a `.cmd` shim, so the discovered path is
/// returned as-is and [`run_harness_against`] handles the indirection. Looking
/// for the real executable here would mean guessing at npm's layout.
fn harness() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("DSH_BIN") {
        let path = PathBuf::from(explicit);
        return path.is_file().then_some(path);
    }
    let names: &[&str] = if cfg!(windows) {
        &["dsh.cmd", "dsh.exe", "dsh"]
    } else {
        &["dsh"]
    };
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .flat_map(|dir| names.iter().map(move |n| dir.join(n)))
        .find(|candidate| candidate.is_file())
}

/// A throwaway directory that cleans itself up.
///
/// Guarded by a `Drop` rather than cleaned at the end of the test body, so a
/// failing assertion still tidies up. A test that leaks a directory on failure
/// is a test that fills a disk.
struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let unique = format!(
            "router-isolation-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        );
        let path = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&path).expect("cannot create temp dir");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // Best effort: a leftover temp directory is untidy, not a failure.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The bytes of a file, or `None` if it does not exist.
///
/// Absence and emptiness are different states — an absent file can behave
/// differently from an empty one — so they are reported separately rather than
/// both collapsing to an empty vector.
fn bytes(path: &Path) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

/// Every file under `root`, with its bytes, as a stable snapshot.
///
/// The comparison is over the whole tree rather than one named file because the
/// interesting failure is not "this file changed" but "this root was touched at
/// all". A snapshot cannot be fooled by a change the test author did not think
/// to check.
fn snapshot(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    collect(root, root, &mut out);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            collect(root, &path, out);
        } else {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            // A file that vanished between listing and reading is a change, and
            // is recorded as such rather than skipped.
            out.push((rel, bytes(&path).unwrap_or_default()));
        }
    }
}

/// Run the harness briefly against a state root and let it initialise.
///
/// The harness is asked to serve, then stopped. The point is not the serving —
/// it is that starting up is what creates and mutates the state root. A harness
/// that never starts writes nothing, and a test that never writes anything
/// proves nothing.
///
/// # Waiting for evidence, not for a duration
///
/// The function does not sleep and then *assume* the harness came up. It watches
/// the state root until the harness has demonstrably written something, and only
/// then stops it. A fixed sleep would be both slower and wrong: too short on a
/// cold machine, and indistinguishable from "the harness crashed instantly",
/// which is the failure this test most needs to report clearly.
///
/// # Why stderr is captured
///
/// When the harness refuses to start it says exactly why, and that sentence is
/// the difference between a five-minute fix and an hour of guessing. Discarding
/// it turns `EADDRINUSE` or a permissions problem into a bare "it did not
/// start". The tail is kept and printed only on failure, so a passing run stays
/// quiet.
///
/// On Windows the discovered `dsh` is a `.cmd` shim that starts a separate node
/// process and exits. Killing the shim alone would leave the harness running, so
/// the process tree is killed.
///
/// Returns `true` once the harness has written to its state root.
fn run_harness_against(binary: &Path, state_root: &Path, workspace: &Path, port: u16) -> bool {
    let before = snapshot(state_root);

    let mut command = Command::new(binary);
    command
        .arg("web")
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string())
        .arg("--no-open")
        .arg("--trusted-host")
        .arg("127.0.0.1")
        // The isolation lever, and the only thing this test is about.
        .env("DSH_HOME", state_root)
        .current_dir(workspace)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());

    let Ok(mut child) = command.spawn() else {
        return false;
    };

    // Drained on a thread: a harness that logs steadily would otherwise fill the
    // pipe buffer and block on its own stderr, which looks exactly like a hang.
    let stderr = child.stderr.take();
    let log = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let sink = std::sync::Arc::clone(&log);
    let reader = stderr.map(|mut pipe| {
        std::thread::spawn(move || {
            use std::io::Read;
            let mut buf = [0u8; 4096];
            let mut tail = String::new();
            while let Ok(n) = pipe.read(&mut buf) {
                if n == 0 {
                    break;
                }
                tail.push_str(&String::from_utf8_lossy(&buf[..n]));
                // Keep it bounded; the interesting part is always at the end.
                if tail.len() > 8192 {
                    let cut = tail.len() - 8192;
                    tail = tail[cut..].to_string();
                }
            }
            if let Ok(mut guard) = sink.lock() {
                *guard = tail;
            }
        })
    });

    // Watch for the harness to touch its own root. The bound is generous
    // because a first run unpacks a profile tree; the loop exits as soon as
    // there is evidence, so a warm run costs a fraction of the timeout.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut wrote = false;
    while std::time::Instant::now() < deadline {
        if snapshot(state_root) != before {
            wrote = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    // Give a run that has already started writing a moment to settle, so the
    // snapshot taken afterwards is of a quiesced tree rather than of a file
    // mid-write. This is the one place a delay is honest: it is not standing in
    // for a condition, it is letting a known-active writer finish.
    if wrote {
        std::thread::sleep(std::time::Duration::from_secs(2));
    }

    kill_tree(&mut child);
    if let Some(handle) = reader {
        let _ = handle.join();
    }

    if !wrote {
        let captured = log.lock().map(|g| g.clone()).unwrap_or_default();
        let tail: String = captured
            .lines()
            .rev()
            .take(12)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        eprintln!(
            "harness did not start against {} on port {port}.\n\
             Its own output was:\n{tail}",
            state_root.display()
        );
    }
    wrote
}

/// Stop a process and anything it started.
///
/// The harness is a node process behind a shim, and on Windows killing the shim
/// leaves the child running — which would leave a harness holding a port and a
/// state root for the rest of the suite.
fn kill_tree(child: &mut std::process::Child) {
    #[cfg(windows)]
    {
        // `/T` includes the process tree, `/F` forces it. Errors are ignored:
        // a process that already exited is not a problem to report.
        let _ = Command::new("taskkill")
            .arg("/PID")
            .arg(child.id().to_string())
            .arg("/T")
            .arg("/F")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Claim a free port for a test instance.
///
/// # Why not a fixed port
///
/// The obvious choice — hardcode 33081 and 33082 — is exactly the mistake this
/// project exists to prevent. A fixed port is a claim on a machine-wide resource
/// that the test does not actually hold: a previous run that leaked a process,
/// an unrelated service, or a parallel test all take it, and the harness then
/// dies with `EADDRINUSE`. That turned out to be a real failure here, and it
/// presented as "the harness will not start", which reads like a broken build
/// rather than a busy port.
///
/// So the port is taken from the test range declared in `support::sandbox`,
/// which is private to tests and sits far above both the harness default (3080)
/// and the router's own allocation range (3081+). A test must never start a
/// harness on a port a person's own instance could already be using.
///
/// If the chosen port is somehow busy anyway, the next one in the range is
/// tried, so a stray process from an earlier run cannot fail the suite.
fn free_port() -> u16 {
    use std::sync::atomic::{AtomicU16, Ordering};
    static NEXT: AtomicU16 = AtomicU16::new(TEST_PORT_BASE);

    for _ in 0..TEST_PORT_SPAN {
        let port = NEXT.fetch_add(1, Ordering::Relaxed);
        if port >= TEST_PORT_BASE + TEST_PORT_SPAN {
            NEXT.store(TEST_PORT_BASE, Ordering::Relaxed);
            continue;
        }
        // Bound and released: on Windows a just-closed socket can linger, so a
        // failure here means "try the next one", not "the test is broken".
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }
    panic!(
        "no free port in the test range {TEST_PORT_BASE}..{}",
        TEST_PORT_BASE + TEST_PORT_SPAN
    );
}

/// **The acceptance test.**
///
/// Start instance A. Snapshot instance B's state root. Start instance A again,
/// doing real work against its own root. Assert instance B's root is
/// **byte-for-byte identical**.
///
/// If the two roots were the same directory — the failure this project exists to
/// prevent — B's tree would move, and the assertion would fail with the file and
/// size that changed.
#[test]
fn a_session_in_one_instance_leaves_another_untouched() {
    let Some(binary) = harness() else {
        eprintln!(
            "skipping: no `dsh` on PATH and DSH_BIN is unset. \
             This test needs a real harness — a fake one cannot be wrong in the \
             ways this test is looking for."
        );
        return;
    };

    let sandbox = TempDir::new("acceptance");
    let root_a = sandbox.path().join("instances/api/dsh");
    let root_b = sandbox.path().join("instances/notes/dsh");
    let ws_a = sandbox.path().join("projects/api");
    let ws_b = sandbox.path().join("projects/notes");
    for dir in [&root_a, &root_b, &ws_a, &ws_b] {
        std::fs::create_dir_all(dir).expect("cannot create sandbox dir");
    }

    // Two roots, two ports, two workspaces — the arrangement the router makes.
    assert_ne!(root_a, root_b, "the two instances must not share a root");
    let port_a = free_port();
    let port_b = free_port();
    assert_ne!(port_a, port_b, "the two instances must not share a port");

    assert!(
        run_harness_against(&binary, &root_a, &ws_a, port_a),
        "instance A's harness never wrote to its state root, so this test would \
         prove nothing. If this is a port or permission problem, the message \
         above the failure is the harness's own."
    );
    assert!(
        run_harness_against(&binary, &root_b, &ws_b, port_b),
        "instance B's harness never wrote to its state root, so there is \
         nothing to protect and the assertion below would pass vacuously"
    );

    // Everything B owns, frozen.
    let before_b = snapshot(&root_b);
    assert!(
        !before_b.is_empty(),
        "instance B's root is empty after a run, so there is nothing to protect \
         and this test would pass vacuously"
    );

    // A is now started a second time. Startup is when the harness reads and
    // rewrites its storage — `workspace.json` among it — which is exactly the
    // write that would land in B's root if the roots were shared.
    assert!(
        run_harness_against(&binary, &root_a, &ws_a, port_a),
        "instance A's second run never wrote anything, so no write happened to \
         observe and the isolation assertion below means nothing"
    );

    let after_b = snapshot(&root_b);

    // Compare by name first, so a file that only A wrote is reported as an
    // addition rather than as a confusing mismatch on a later index.
    let names_before: Vec<&String> = before_b.iter().map(|(n, _)| n).collect();
    let names_after: Vec<&String> = after_b.iter().map(|(n, _)| n).collect();
    assert_eq!(
        names_before, names_after,
        "instance B's state root gained or lost files while instance A ran. \
         The two roots are not isolated."
    );

    for ((name, before), (_, after)) in before_b.iter().zip(after_b.iter()) {
        assert_eq!(
            before.len(),
            after.len(),
            "`{name}` changed size in instance B's root ({before_len} -> {after_len} bytes) \
             while instance A was running. Shared mutable state — this is the bug \
             the product exists to prevent.",
            before_len = before.len(),
            after_len = after.len()
        );
        assert!(
            before == after,
            "`{name}` changed content in instance B's root while instance A ran, \
             despite matching size. Two processes wrote the same file."
        );
    }

    // Byte equality was asserted per file above; this is the whole-tree
    // statement in one line, so the intent survives a careless edit to the loop.
    assert_eq!(
        before_b, after_b,
        "instance B's root is not byte-identical after instance A ran"
    );

    // The control: A's own root should have been written. Without this, a
    // harness that writes nothing at all would make the assertion above pass
    // for the wrong reason.
    let a_after = snapshot(&root_a);
    assert!(
        !a_after.is_empty(),
        "instance A's root is empty, so the harness did not write anything and \
         the isolation assertion above proves nothing"
    );
}

/// The leak this test would catch, demonstrated against a shared root.
///
/// Without this, a green acceptance test could mean "the harness writes
/// nothing". This shows the same measurement *does* detect a change when the
/// roots are genuinely shared, so the assertion above has teeth.
#[test]
fn the_measurement_detects_a_change_when_roots_are_shared() {
    let sandbox = TempDir::new("control");
    let shared = sandbox.path().join("instances/shared/dsh");
    std::fs::create_dir_all(&shared).expect("cannot create sandbox dir");
    std::fs::write(shared.join("workspace.json"), b"{\"workspaces\":{}}").expect("cannot seed");

    let before = snapshot(&shared);

    // A second instance writing into the same root — the bug, reproduced.
    std::fs::write(
        shared.join("workspace.json"),
        b"{\"workspaces\":{\"a\":{\"path\":\"/projects/api\"}}}",
    )
    .expect("cannot write");

    let after = snapshot(&shared);

    assert_ne!(
        before, after,
        "the snapshot comparison must notice a file changing in a shared root; \
         if it does not, the acceptance test above cannot fail and means nothing"
    );
}

/// Two instances provisioned independently must not produce the same root.
///
/// A weaker, always-runnable companion to the acceptance test: it needs no
/// harness, and it fails if the state-root derivation ever collapses two names
/// onto one directory.
#[test]
fn distinct_instances_derive_distinct_state_roots() {
    let home = PathBuf::from("/router-home");
    let a = router_core::registry::Registry::state_root(&home, "api");
    let b = router_core::registry::Registry::state_root(&home, "notes");
    assert_ne!(a, b, "two instance names must not resolve to one root");

    // And the root must be *inside* the router home, never the harness default.
    // Pointing an instance at `~/.dsh` would be pointing it at the very
    // installation this project must not disturb.
    for root in [&a, &b] {
        assert!(
            root.starts_with(&home),
            "an instance root escaped the router home: {}",
            root.display()
        );
        assert!(
            !root.ends_with(".dsh"),
            "an instance root must never be the harness default: {}",
            root.display()
        );
    }
}
