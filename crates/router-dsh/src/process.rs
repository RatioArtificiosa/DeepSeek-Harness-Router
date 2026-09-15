//! Stopping a harness, including the process it actually runs in.
//!
//! # The problem this solves
//!
//! On Windows the installed `dsh` is a `.cmd` shim, not an executable. Spawning
//! it starts `cmd.exe`, which starts `node`, which is the process that holds the
//! port and the state root. The child handle the supervisor holds belongs to the
//! *shim*.
//!
//! Killing that handle therefore stops nothing that matters. The shim exits, the
//! supervisor reports success, and the harness keeps running — holding its port
//! and its state root. The next `router start` then fails with `EADDRINUSE`, or
//! silently talks to the previous process, which is worse.
//!
//! This was not hypothetical: `router stop notes` printed "Stopped notes" while
//! the node process carried on answering on its port.
//!
//! # The approach
//!
//! Ask the operating system to stop the whole tree. On Windows that is
//! `taskkill /T /F`, which walks parent-to-child and cannot miss a
//! grandchild. On Unix the direct child *is* the process, so the ordinary kill
//! is correct and a tree walk would be ceremony.
//!
//! After a tree kill the direct child is still waited on, so the supervisor does
//! not leak a zombie and can honestly report that the child it owns is gone.

use std::path::Path;
use std::process::Stdio;
use tokio::process::Child;

/// Whether a harness path is a script the OS will run through an interpreter.
///
/// A `.cmd` or `.bat` is executed by `cmd.exe`, so the process the supervisor
/// spawns is not the process that runs the harness. This predicate is what
/// decides whether a tree kill is necessary — and it is deliberately based on
/// the file the user pointed us at, because that is what determines the spawn
/// behaviour.
#[must_use]
pub fn is_command_script(binary: &Path) -> bool {
    binary
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| {
            let lower = ext.to_ascii_lowercase();
            lower == "cmd" || lower == "bat"
        })
}

/// Stop a child and everything it started.
///
/// Always waits for the child afterwards, so a caller can rely on the process
/// being gone rather than merely asked to leave.
pub async fn kill_tree(child: &mut Child, grace: std::time::Duration) {
    // A graceful signal first: a harness that can still flush its session state
    // should be allowed to, and taking that away to save a few milliseconds
    // would be trading the user's data for our convenience.
    let _ = child.start_kill();
    if tokio::time::timeout(grace, child.wait()).await.is_ok() {
        return;
    }

    // Still alive. On Windows the direct child is a shim whose real work is in a
    // grandchild, so walking the tree is the only way to stop the harness.
    #[cfg(windows)]
    {
        let _ = tokio::process::Command::new("taskkill")
            .arg("/PID")
            .arg(child.id().unwrap_or(0).to_string())
            .arg("/T")
            .arg("/F")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    }

    // Forced, for the Unix case and as a backstop if `taskkill` could not help.
    let _ = child.kill().await;
}

/// Find the process listening on a loopback port and stop it.
///
/// # Why this is needed at all
///
/// `router stop` runs in its own process. The supervisor that started the
/// harness lives in a *different* process — the one running `router start` — so
/// the stopping process holds no child handle. A supervisor that only knew how
/// to kill its own child would therefore do nothing, and report success anyway.
///
/// That was the actual behaviour: `router stop` printed "Stopped" while the
/// harness kept the port. A command that reports work it did not do is worse
/// than one that fails, because the user stops looking.
///
/// So the port is the address of the process. It is the one fact both processes
/// agree on, and the harness's whole purpose is to hold it.
///
/// Returns `true` when nothing is listening afterwards — which includes the case
/// where nothing was listening to begin with, because "make it not run" is
/// already satisfied.
pub async fn stop_listener_on(port: u16, grace: std::time::Duration) -> bool {
    let Some(pids) = listeners_on(port) else {
        // Could not determine the owner. Reporting a failure is right: claiming
        // success here is the exact bug this function exists to fix.
        return false;
    };

    for pid in pids {
        terminate_pid(pid, grace).await;
    }

    // Confirm by probing rather than by trusting the signal. The process may
    // take a moment to release the socket, and a stale answer here is the whole
    // failure mode.
    let deadline = std::time::Instant::now() + grace.max(std::time::Duration::from_secs(5));
    while std::time::Instant::now() < deadline {
        if !is_listening(port) {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    }
    !is_listening(port)
}

/// Whether anything accepts a connection on a loopback port.
#[must_use]
pub fn is_listening(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        std::time::Duration::from_millis(300),
    )
    .is_ok()
}

/// PIDs listening on a port, or `None` if the query itself failed.
///
/// `None` and an empty list mean different things: "I could not find out" versus
/// "nobody is there". Collapsing them would let a failed query read as a
/// successful stop.
#[cfg(windows)]
fn listeners_on(port: u16) -> Option<Vec<u32>> {
    // `netstat -ano` is the only port-to-PID mapping available without a
    // dependency. It is parsed rather than trusted, and a line that does not
    // parse is skipped instead of aborting the whole answer.
    let output = std::process::Command::new("netstat")
        .args(["-ano", "-p", "TCP"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let needle = format!(":{port}");
    let mut pids = Vec::new();
    for line in text.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        // Proto, Local Address, Foreign Address, State, PID
        if cols.len() < 5 || !cols[0].eq_ignore_ascii_case("TCP") {
            continue;
        }
        if !cols[1].ends_with(&needle) || !cols[3].eq_ignore_ascii_case("LISTENING") {
            continue;
        }
        if let Ok(pid) = cols[4].parse::<u32>() {
            if pid != 0 && !pids.contains(&pid) {
                pids.push(pid);
            }
        }
    }
    Some(pids)
}

#[cfg(unix)]
fn listeners_on(port: u16) -> Option<Vec<u32>> {
    // `lsof` is not guaranteed present, so it is tried and its absence reported
    // as a failed query rather than as "nothing is listening".
    let output = std::process::Command::new("lsof")
        .args(["-t", "-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let pids: Vec<u32> = text
        .lines()
        .filter_map(|l| l.trim().parse::<u32>().ok())
        .collect();
    Some(pids)
}

#[cfg(not(any(windows, unix)))]
fn listeners_on(_port: u16) -> Option<Vec<u32>> {
    None
}

/// Stop one process by id, including its children.
#[cfg(windows)]
async fn terminate_pid(pid: u32, grace: std::time::Duration) -> bool {
    let _ = grace;
    // `/T` walks the tree: the harness may be a grandchild of the process that
    // holds the port, and stopping the holder alone can leave an orphan that
    // reclaims it.
    tokio::process::Command::new("taskkill")
        .arg("/PID")
        .arg(pid.to_string())
        .arg("/T")
        .arg("/F")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .is_ok_and(|s| s.success())
}

#[cfg(unix)]
async fn terminate_pid(pid: u32, grace: std::time::Duration) -> bool {
    // A polite request first, then the certainty.
    let term = std::process::Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status()
        .is_ok_and(|s| s.success());
    if term {
        tokio::time::sleep(grace.min(std::time::Duration::from_secs(2))).await;
    }
    let _ = std::process::Command::new("kill")
        .args(["-KILL", &pid.to_string()])
        .status();
    true
}

#[cfg(not(any(windows, unix)))]
async fn terminate_pid(_pid: u32, _grace: std::time::Duration) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn command_scripts_are_recognised() {
        // The shim is what makes a tree kill necessary, so recognising it is the
        // load-bearing decision in this module.
        assert!(is_command_script(&PathBuf::from("dsh.cmd")));
        assert!(is_command_script(&PathBuf::from("DSH.CMD")));
        assert!(is_command_script(&PathBuf::from("dsh.bat")));
        assert!(is_command_script(&PathBuf::from("/opt/bin/dsh.Cmd")));
    }

    #[test]
    fn real_executables_are_not_command_scripts() {
        // A false positive here would run `taskkill` for a process that needs no
        // tree walk; harmless, but it would also mean the predicate no longer
        // says what it claims to.
        assert!(!is_command_script(&PathBuf::from("dsh")));
        assert!(!is_command_script(&PathBuf::from("dsh.exe")));
        assert!(!is_command_script(&PathBuf::from("/usr/local/bin/dsh")));
        assert!(!is_command_script(&PathBuf::from("dsh.ps1")));
    }

    #[tokio::test]
    async fn killing_a_child_that_already_exited_is_not_an_error() {
        // `stop` can be called twice, and a harness can die on its own. Neither
        // is a failure, and neither may panic.
        #[cfg(windows)]
        let mut child = tokio::process::Command::new("cmd")
            .args(["/C", "exit", "0"])
            .spawn()
            .expect("cannot spawn cmd");
        #[cfg(not(windows))]
        let mut child = tokio::process::Command::new("true")
            .spawn()
            .expect("cannot spawn true");

        // Let it finish first, so this exercises the already-exited path.
        let _ = child.wait().await;
        kill_tree(&mut child, std::time::Duration::from_millis(200)).await;
    }

    #[tokio::test]
    async fn stopping_a_port_nobody_holds_reports_success() {
        // "Make it not run" is already true, so this must not be an error — a
        // second `router stop` would otherwise look like a failure.
        let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("cannot bind");
        let port = probe.local_addr().expect("bound").port();
        drop(probe);

        assert!(!is_listening(port), "the port must start free");
        assert!(
            stop_listener_on(port, std::time::Duration::from_millis(500)).await,
            "stopping a free port must report success"
        );
    }

    #[tokio::test]
    async fn stopping_a_held_port_actually_frees_it() {
        // The regression this guards: a stop that reports success while the
        // listener runs on.
        //
        // # Why the listener is a separate process
        //
        // The first version of this test opened the socket in the test process
        // itself, and `stop_listener_on` — quite correctly — found the test
        // runner's own PID and killed it. A self-destructing test proves nothing
        // and takes the suite down with it, so the listener runs in a child that
        // may safely be terminated.
        //
        // The child picks the port and reports it, so the test still never
        // hardcodes a port.
        #[cfg(windows)]
        let mut listener = tokio::process::Command::new("pwsh")
            .args([
                "-NoProfile",
                "-Command",
                "$l=[System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback,0);\
                 $l.Start();\
                 [Console]::Out.WriteLine($l.LocalEndpoint.Port);\
                 [Console]::Out.Flush();\
                 Start-Sleep -Seconds 60",
            ])
            .stdout(Stdio::piped())
            .spawn()
            .expect("cannot spawn a listener process");
        #[cfg(not(windows))]
        let mut listener = tokio::process::Command::new("python3")
            .args([
                "-c",
                "import socket,sys,time\n\
                 s=socket.socket()\n\
                 s.bind(('127.0.0.1',0))\n\
                 s.listen(1)\n\
                 print(s.getsockname()[1], flush=True)\n\
                 time.sleep(60)",
            ])
            .stdout(Stdio::piped())
            .spawn()
            .expect("cannot spawn a listener process");

        // Read the port the child chose.
        let port = {
            use tokio::io::AsyncBufReadExt;
            let stdout = listener.stdout.take().expect("piped stdout");
            let mut lines = tokio::io::BufReader::new(stdout).lines();
            tokio::time::timeout(std::time::Duration::from_secs(20), lines.next_line())
                .await
                .ok()
                .and_then(Result::ok)
                .flatten()
                .expect("the listener must report the port it bound")
                .trim()
                .parse::<u16>()
                .expect("a port number")
        };

        assert!(
            is_listening(port),
            "the listener must be up before we stop it"
        );
        let result = stop_listener_on(port, std::time::Duration::from_secs(3)).await;
        let free = !is_listening(port);

        // Clean up whatever survived, so a failure here leaks no process.
        let _ = listener.kill().await;
        let _ = listener.wait().await;

        assert!(result, "stopping a listener on port {port} must succeed");
        assert!(
            free,
            "the port must actually be free afterwards, not merely reported so"
        );
    }
}
