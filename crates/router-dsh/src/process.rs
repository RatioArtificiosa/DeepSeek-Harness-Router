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

/// Why a port could not be released.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopRefusal {
    /// Something is listening, but it is not this project's harness.
    ///
    /// Carries the process ids and their command lines, so the message can name
    /// what is actually holding the port instead of leaving the user to guess.
    NotOurs {
        /// The offending process ids.
        pids: Vec<u32>,
        /// Their command lines, for display.
        commands: Vec<String>,
    },
    /// The owner could not be determined at all.
    Unknown,
}

/// Find the process listening on a loopback port and stop it, if it is ours.
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
/// # Why identity is verified before killing
///
/// The port alone is **not** proof of ownership. An earlier revision killed
/// whatever held the port, on the reasoning that the port is the one fact both
/// processes agree on. It is — but agreement is not identity: any unrelated
/// program that happened to bind that port was killed, and `router stop` still
/// reported success. A user testing two things at once could lose an unrelated
/// process with no warning and no way to know why.
///
/// So the listener is identified first. A process is ours when its command line
/// shows this project's harness invocation. If it is not ours, nothing is
/// killed and the caller is told what is holding the port.
///
/// Returns `Ok(true)` when nothing is listening afterwards — which includes the
/// case where nothing was listening to begin with, because "make it not run" is
/// already satisfied.
///
/// # Errors
///
/// Returns [`StopRefusal`] when the listener is not ours, or its owner cannot be
/// determined. Both are refusals rather than failures to avoid killing the wrong
/// thing.
pub async fn stop_listener_on(port: u16, grace: std::time::Duration) -> Result<bool, StopRefusal> {
    if !is_listening(port) {
        return Ok(true);
    }

    let Some(pids) = listeners_on(port) else {
        // Could not determine the owner. Refusing is right: killing a process
        // we cannot identify is exactly the hazard this check exists for.
        return Err(StopRefusal::Unknown);
    };
    if pids.is_empty() {
        // A connect succeeded but no owning process was found — possible when
        // another user's process holds the port, or the lookup raced. Refuse
        // rather than guess.
        return Err(StopRefusal::Unknown);
    }

    // Every listener must look like our harness. If any does not, nothing is
    // killed: stopping some of them would leave a half-stopped state that is
    // harder to reason about than refusing outright.
    let mut foreign_pids = Vec::new();
    let mut foreign_commands = Vec::new();
    for pid in &pids {
        match process_command_line(*pid) {
            Some(cmd) if looks_like_our_harness(&cmd) => {}
            Some(cmd) => {
                foreign_pids.push(*pid);
                foreign_commands.push(cmd);
            }
            None => {
                foreign_pids.push(*pid);
                foreign_commands.push("<command line unavailable>".to_string());
            }
        }
    }
    if !foreign_pids.is_empty() {
        return Err(StopRefusal::NotOurs {
            pids: foreign_pids,
            commands: foreign_commands,
        });
    }

    for pid in pids {
        terminate_pid(pid, grace).await;
    }

    // Confirm by probing rather than by trusting the signal. The process may
    // take a moment to release the socket, and a stale answer here is the whole
    // failure mode.
    let deadline = std::time::Instant::now() + grace.max(std::time::Duration::from_secs(5));
    while std::time::Instant::now() < deadline {
        if !is_listening(port) {
            return Ok(true);
        }
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    }
    Ok(!is_listening(port))
}

/// Whether a command line looks like a harness this router started.
///
/// Deliberately permissive about the path and strict about the shape: the
/// binary may be `dsh`, `dsh.cmd`, `dsh.exe`, `node …/dsh/bin.js`, or a full
/// path, and on Windows the actual listener is a `node` process whose command
/// line contains the harness's script path. What must hold is that the line
/// mentions a `dsh` executable or a path through the harness package — enough
/// to distinguish it from an unrelated program that merely binds the right port.
///
/// This is a safety check, not authentication. It is not defending against a
/// hostile process deliberately imitating the harness; it is preventing an
/// accident, which is the realistic risk.
#[must_use]
pub fn looks_like_our_harness(command_line: &str) -> bool {
    let lower = command_line.to_ascii_lowercase();

    // The harness binary itself, with or without an extension.
    if lower.contains("dsh ")
        || lower.contains("dsh.exe")
        || lower.contains("dsh.cmd")
        || lower.ends_with("dsh")
        || lower.contains("\\dsh\"")
        || lower.contains("/dsh\"")
    {
        return true;
    }

    // The npm-installed layout: node running a script inside the package.
    if lower.contains("@deepseek-ai") && lower.contains("dsh") {
        return true;
    }
    // A path that goes through the harness package directory.
    if lower.contains("deepseek-harness") {
        return true;
    }

    false
}

/// The command line of a process, or `None` when it cannot be read.
#[cfg(windows)]
fn process_command_line(pid: u32) -> Option<String> {
    let output = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!(
                "(Get-CimInstance Win32_Process -Filter \"ProcessId={pid}\" \
                 -ErrorAction SilentlyContinue).CommandLine"
            ),
        ])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// The command line of a process, or `None` when it cannot be read.
#[cfg(unix)]
fn process_command_line(pid: u32) -> Option<String> {
    let output = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "args="])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

#[cfg(not(any(unix, windows)))]
fn process_command_line(_pid: u32) -> Option<String> {
    None
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

/// Describe what is listening on a port, for a message a person can act on.
///
/// Returns `None` when nothing is listening. When something is, the description
/// names the process and says whether it looks like a harness — because the two
/// cases need different advice, and "port in use" alone tells a user neither
/// what to stop nor whether stopping it is even the right move.
///
/// This exists so a refusal can explain itself. A bare "port 3082 is in use"
/// sends the reader to `netstat`; naming the process lets them decide.
#[must_use]
pub fn describe_listener(port: u16) -> Option<String> {
    let pids = listeners_on(port)?;
    if pids.is_empty() {
        return None;
    }

    let mut described = Vec::new();
    for pid in pids {
        match process_command_line(pid) {
            Some(cmd) if looks_like_our_harness(&cmd) => {
                described.push(format!("pid {pid} (another DeepSeek Harness)"));
            }
            Some(cmd) => {
                // Only the program name is shown, and only the first path
                // segment of it. The full command line can contain a workspace
                // path or a token, and a diagnostic is not a reason to print
                // either.
                let program = cmd.split_whitespace().next().unwrap_or("a process");
                let name = Path::new(program)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(program);
                described.push(format!("pid {pid} ({name})"));
            }
            None => described.push(format!("pid {pid}")),
        }
    }

    Some(described.join(", "))
}

/// Whether `pid` holds `port`, directly or through a descendant.
///
/// # Why descendants count
///
/// On Windows the router spawns the `dsh.cmd` shim, so the PID it records at
/// spawn time belongs to `cmd.exe` — while the process that actually binds the
/// port is a `node` grandchild. Checking only the recorded PID would therefore
/// never recognise the router's own harness, and every restart would be refused
/// as a foreign conflict. This is the same shim-versus-harness gap that
/// [`kill_tree`] exists to solve, met from the other direction.
///
/// The walk is bounded: it stops at a fixed depth and only ever follows children
/// of the recorded process, so it cannot wander into unrelated parts of the
/// process tree.
#[must_use]
pub fn pid_or_descendant_listens_on(pid: u32, port: u16) -> bool {
    let Some(listeners) = listeners_on(port) else {
        return false;
    };
    if listeners.is_empty() {
        return false;
    }

    let family = descendants_of(pid, MAX_DESCENDANT_DEPTH);
    listeners.iter().any(|listener| family.contains(listener))
}

/// How deep the shim-to-harness walk may go.
///
/// A `.cmd` shim adds one `cmd.exe`, which adds one `node`; a launcher script
/// could add a third. Four is generous for a known chain while still bounding
/// the walk on a machine running unrelated processes.
const MAX_DESCENDANT_DEPTH: u32 = 4;

/// `pid` and every descendant of it, up to `depth` levels.
///
/// Returns an empty list when the process is gone, which callers read as "not
/// ours" — the safe direction.
#[cfg(windows)]
fn descendants_of(pid: u32, depth: u32) -> Vec<u32> {
    let mut found = vec![pid];
    let mut frontier = vec![pid];

    for _ in 0..depth {
        if frontier.is_empty() {
            break;
        }
        // One query per level rather than per process: `Get-CimInstance` is a
        // PowerShell startup, and doing it once per node would dominate the cost
        // of the check.
        let filter = frontier
            .iter()
            .map(|p| format!("ParentProcessId={p}"))
            .collect::<Vec<_>>()
            .join(" OR ");
        let Some(output) = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &format!(
                    "Get-CimInstance Win32_Process -Filter \"{filter}\" \
                     -ErrorAction SilentlyContinue | \
                     Select-Object -ExpandProperty ProcessId"
                ),
            ])
            .output()
            .ok()
        else {
            break;
        };

        let mut next = Vec::new();
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            if let Ok(child) = line.trim().parse::<u32>() {
                if !found.contains(&child) {
                    found.push(child);
                    next.push(child);
                }
            }
        }
        frontier = next;
    }

    found
}

/// `pid` and every descendant of it, up to `depth` levels.
///
/// Unix has no equivalent ambiguity: the spawned child *is* the process, so the
/// recorded PID is the listener and there is nothing to walk.
#[cfg(not(windows))]
fn descendants_of(pid: u32, _depth: u32) -> Vec<u32> {
    vec![pid]
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
            matches!(
                stop_listener_on(port, std::time::Duration::from_millis(500)).await,
                Ok(true)
            ),
            "stopping a free port must report success"
        );
    }

    #[test]
    fn harness_command_lines_are_recognised() {
        // The safety check that stops the router killing an unrelated process.
        // Permissive about the path, strict about the shape, because what must
        // hold is only that the line names a harness rather than some program
        // that merely bound the right port.
        for good in [
            "dsh web",
            "/usr/local/bin/dsh --profile web",
            "C:\\Users\\x\\.local\\bin\\dsh.cmd web",
            "dsh.exe --profile web --port 3081",
            "node C:\\Users\\x\\AppData\\Roaming\\npm\\node_modules\\@deepseek-ai\\dsh\\lib\\bin.js",
            "/usr/lib/node_modules/@deepseek-ai/dsh/bin.js --profile web",
            "node /opt/deepseek-harness/lib/bin.js",
        ] {
            assert!(
                looks_like_our_harness(good),
                "{good:?} should be recognised as a harness"
            );
        }
    }

    #[test]
    fn unrelated_command_lines_are_not_recognised() {
        // The regression this guards: `router stop` killed an unrelated process
        // that happened to hold the instance's port, and reported success.
        for bad in [
            "python -c import socket",
            "pwsh -NoProfile -Command $l.Start()",
            "C:\\Windows\\System32\\svchost.exe -k netsvcs",
            "nginx: worker process",
            "node server.js",
            "java -jar app.jar",
            "postgres -D /var/lib/postgresql/data",
            "",
        ] {
            assert!(
                !looks_like_our_harness(bad),
                "{bad:?} must NOT be treated as a harness"
            );
        }
    }

    #[tokio::test]
    async fn a_foreign_listener_is_refused_not_killed() {
        // The bug, as a test. A child process that is plainly not the harness
        // binds a port; the function must refuse and leave it alive.
        //
        // # Why this test is worth its cost
        //
        // Killing the wrong process is unrecoverable for the user — they lose
        // work with no warning and no way to know what happened. A refusal they
        // can read and act on is strictly better, so the safety property is
        // asserted rather than assumed.
        #[cfg(windows)]
        let mut holder = tokio::process::Command::new("pwsh")
            .args([
                "-NoProfile",
                "-Command",
                "$l=[System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback,0);\
                 $l.Start();\
                 [Console]::Out.WriteLine($l.LocalEndpoint.Port);\
                 [Console]::Out.Flush();\
                 Start-Sleep -Seconds 120",
            ])
            .stdout(Stdio::piped())
            .spawn()
            .expect("cannot spawn a foreign listener");
        #[cfg(not(windows))]
        let mut holder = tokio::process::Command::new("python3")
            .args([
                "-c",
                "import socket,sys,time\n\
                 s=socket.socket()\n\
                 s.bind(('127.0.0.1',0))\n\
                 s.listen(1)\n\
                 print(s.getsockname()[1], flush=True)\n\
                 time.sleep(120)",
            ])
            .stdout(Stdio::piped())
            .spawn()
            .expect("cannot spawn a foreign listener");

        let port = {
            use tokio::io::AsyncBufReadExt;
            let stdout = holder.stdout.take().expect("piped stdout");
            let mut lines = tokio::io::BufReader::new(stdout).lines();
            tokio::time::timeout(std::time::Duration::from_secs(20), lines.next_line())
                .await
                .ok()
                .and_then(Result::ok)
                .flatten()
                .expect("the listener must report its port")
                .trim()
                .parse::<u16>()
                .expect("a port number")
        };

        assert!(is_listening(port), "the foreign listener must be up");
        let outcome = stop_listener_on(port, std::time::Duration::from_secs(2)).await;

        // Checked *before* the test tears its own listener down, or the
        // assertion would be about the cleanup rather than about the refusal.
        let still_alive = holder.try_wait().ok().flatten().is_none();
        let still_listening = is_listening(port);

        let _ = holder.kill().await;
        let _ = holder.wait().await;

        match outcome {
            Err(StopRefusal::NotOurs { pids, .. }) => {
                assert!(
                    !pids.is_empty(),
                    "the refusal must name what holds the port"
                );
            }
            other => panic!("expected a refusal for a foreign listener, got {other:?}"),
        }
        assert!(
            still_alive,
            "the foreign process must still be running — refusing means not killing"
        );
        assert!(
            still_listening,
            "the foreign listener must still hold its port"
        );
    }
}
