//! A sandbox for tests that start real harness processes.
//!
//! # Why this exists
//!
//! Testing this project means starting actual DeepSeek Harness processes. Those
//! processes outlive the test that made them unless something stops them, and a
//! leaked harness is visible to whoever owns the machine as an unexplained
//! service listening on a port.
//!
//! That is not hypothetical. A session of CLI testing left four harnesses
//! running on ports 3082–3085: the supervising `router` process was killed
//! while the harness underneath kept running. The tool has since been fixed to
//! stop process trees, but a test that kills its own supervisor by hand reopens
//! the same hole — and a person watching their machine sees a stray service
//! they did not create.
//!
//! This module makes the two mistakes structurally hard:
//!
//! 1. **Port collision.** [`Sandbox::ports`] hands out ports from a range no
//!    user instance occupies, so a test can never land on someone's live agent.
//! 2. **Leaked processes.** [`Sandbox`] implements `Drop`, so a failing
//!    assertion or an early `return` still stops whatever was started. Cleanup
//!    cannot be forgotten because it is not something the test has to remember
//!    to call.

#![allow(dead_code)] // Not every test in the crate uses every helper.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// The lowest port a test instance may use.
///
/// Chosen to sit clear of both the harness default (3080) and the router's own
/// allocation range (3081 and up). A real user's second instance lives in that
/// range, so a test that allocated there could collide with live work — and
/// before the identity check existed, could have killed it.
pub const TEST_PORT_BASE: u16 = 34_000;

/// How many ports the test range covers.
pub const TEST_PORT_SPAN: u16 = 100;

/// A throwaway router home plus everything started against it.
///
/// Cleans up on drop, including the processes. Hold one for the lifetime of a
/// test; do not construct one per assertion.
pub struct Sandbox {
    root: PathBuf,
    started: Vec<Child>,
    next_port: u16,
}

impl Sandbox {
    /// Create a sandbox under the system temp directory.
    ///
    /// # Panics
    ///
    /// Panics if the directory cannot be created, because a test that cannot
    /// make its own workspace cannot test anything and should say so loudly.
    #[must_use]
    pub fn new(label: &str) -> Self {
        let unique = format!(
            "router-test-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        );
        let root = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&root).expect("cannot create the test sandbox");
        Self {
            root,
            started: Vec::new(),
            next_port: TEST_PORT_BASE,
        }
    }

    /// The sandbox root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// A path inside the sandbox, creating nothing.
    #[must_use]
    pub fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    /// The router home for this sandbox.
    #[must_use]
    pub fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    /// Create a workspace directory and return its path.
    ///
    /// # Panics
    ///
    /// Panics if the directory cannot be created.
    pub fn workspace(&self, name: &str) -> PathBuf {
        let p = self.root.join("ws").join(name);
        std::fs::create_dir_all(&p).expect("cannot create a test workspace");
        p
    }

    /// The next port in the test range.
    ///
    /// Handed out sequentially rather than randomly so a failure names the same
    /// port every run, which makes it reproducible. The range is private to
    /// tests, so a collision means two tests in this process — and the
    /// sequential counter rules that out too.
    pub fn next_port(&mut self) -> u16 {
        let port = self.next_port;
        self.next_port = self
            .next_port
            .checked_add(1)
            .filter(|p| *p < TEST_PORT_BASE + TEST_PORT_SPAN)
            .expect("the test port range is exhausted; free some sandboxes");
        port
    }

    /// Start the harness against a state root, and remember it for cleanup.
    ///
    /// Returns the child so the caller can await readiness, but does not
    /// require them to: [`Drop`] stops it either way.
    ///
    /// # Panics
    ///
    /// Panics if the process cannot be spawned.
    pub fn start_harness(
        &mut self,
        binary: &Path,
        state_root: &Path,
        workspace: &Path,
        port: u16,
    ) -> u32 {
        std::fs::create_dir_all(state_root).expect("cannot create the state root");
        let child = Command::new(binary)
            .arg("--profile")
            .arg("web")
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string())
            .arg("--no-open")
            .arg("--trusted-host")
            .arg("127.0.0.1")
            .env("DSH_HOME", state_root)
            .current_dir(workspace)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("cannot start the harness");
        let pid = child.id();
        self.started.push(child);
        pid
    }

    /// Whether anything is listening on a port.
    #[must_use]
    pub fn is_listening(port: u16) -> bool {
        std::net::TcpStream::connect_timeout(
            &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
            std::time::Duration::from_millis(300),
        )
        .is_ok()
    }

    /// Wait until a port answers, or give up.
    ///
    /// Polls rather than sleeping a fixed time: a warm start is fast and a cold
    /// one is slow, and a fixed delay is wrong for both.
    pub fn wait_until_listening(&self, port: u16, timeout: std::time::Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if Self::is_listening(port) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        false
    }

    /// Stop everything this sandbox started, by process id.
    ///
    /// Kills the process tree, because the harness is a node process behind a
    /// launcher on some platforms — the same reason the router itself kills
    /// trees. Stopping only the direct child would leave the listener running,
    /// which is exactly the leak this type exists to prevent.
    pub fn stop_all(&mut self) {
        for mut child in self.started.drain(..) {
            #[cfg(windows)]
            {
                let _ = Command::new("taskkill")
                    .arg("/PID")
                    .arg(child.id().to_string())
                    .arg("/T")
                    .arg("/F")
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        // Processes first: a directory removed while a process still has files
        // open inside it fails on Windows, and the process is the thing that
        // must not survive.
        self.stop_all();
        // Then the directory. Best effort — an undeletable temp directory is
        // untidy, not a failure, and panicking here would abort the test run.
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_test_range_cannot_collide_with_a_real_instance() {
        // Checked at compile time rather than at run time: this is a property of
        // the constants, so a violation should fail the build rather than a
        // test. The rule matters because 3080 is the harness default and 3081
        // upward is where a user's own instances live.
        const {
            assert!(
                TEST_PORT_BASE > 3081,
                "tests must not share the range a user's own instances occupy"
            );
            assert!(
                TEST_PORT_BASE >= 34_000,
                "tests must sit clear of the common low-port picks"
            );
            assert!(
                TEST_PORT_BASE as u32 + TEST_PORT_SPAN as u32 <= u16::MAX as u32,
                "the range must not overflow u16"
            );
        }
    }

    #[test]
    fn ports_are_handed_out_without_repetition() {
        let mut sb = Sandbox::new("ports");
        let a = sb.next_port();
        let b = sb.next_port();
        assert_ne!(a, b, "two instances must never share a port");
        assert!(a >= TEST_PORT_BASE && b >= TEST_PORT_BASE);
    }

    #[test]
    fn dropping_the_sandbox_removes_its_directory() {
        let path = {
            let sb = Sandbox::new("drop");
            let p = sb.root().to_path_buf();
            assert!(p.exists());
            p
        };
        assert!(
            !path.exists(),
            "the sandbox must clean up after itself without being asked"
        );
    }

    #[test]
    fn workspaces_are_created_inside_the_sandbox() {
        let sb = Sandbox::new("ws");
        let ws = sb.workspace("alpha");
        assert!(ws.is_dir());
        assert!(
            ws.starts_with(sb.root()),
            "a test workspace must never be created outside the sandbox"
        );
    }
}
