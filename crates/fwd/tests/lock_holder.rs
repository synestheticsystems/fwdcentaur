//! A process that holds the daemon lock but that no command reaches is never taken for no daemon.
//! The daemon names itself in its lock. When its port file is gone, as macOS's temp cleaner left the
//! daemon of 2026-11-11, `fwd daemon status`, a command that gives up and `fwd daemon stop` name the
//! daemon by its pid, say why no command reaches it and give the `kill` line that ends it; stop
//! sends no signal and fails, and once the person has run that line the next command starts a
//! daemon that answers. A process that holds the lock and names no daemon there is reported as such.

mod common;

use anyhow::{anyhow, Result};
use common::Scratch;
use fwd_core::daemon::{self, Autostart};
use fwd_core::project::{self, Paths};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::Stdio;

/// `fwd <args>` against the daemon of `paths`, in the checkout `repo`, with its output piped.
fn fwd(repo: &Path, paths: &Paths, args: &[&str]) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_fwd"));
    command
        .args(args)
        .current_dir(repo)
        .env("FWD_RUNTIME_DIR", &paths.runtime_dir)
        .env("FWD_DATA_DIR", &paths.data_dir)
        .env_remove("FWD_HANDLE")
        .env_remove("FWD_PROJECT")
        .env_remove("FWD_AS")
        .env_remove("FWD_SCOPE")
        .env_remove("FWD_DAEMON_BIN")
        .env_remove("FWD_CLAUDE_SESSION")
        .env_remove("FWD_CLAUDE_AGENT")
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("RUST_LOG")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

/// macOS's temp cleaner removed the port file of a daemon that went on serving (2026-11-11). With
/// its port file gone, the daemon is named, with the `kill` line that ends it, by `fwd daemon
/// status`, by a command without autostart at once and by `fwd daemon start` at the start
/// deadline; `fwd daemon stop` stops nothing and fails saying the same. Once that line has run, a
/// new daemon starts.
#[tokio::test]
async fn a_daemon_whose_port_file_is_gone_is_named_with_the_line_that_ends_it() -> Result<()> {
    let dir = Scratch::new()?;
    let paths = dir.paths("run", "data");
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo)?;
    let ws = project::init(&paths, &repo, project::InitOptions { name: Some("held"), ..Default::default() })?.0.project;
    let spawn = Autostart::Spawn { daemon_bin: PathBuf::from(env!("CARGO_BIN_EXE_fwd")) };
    let pf = daemon::ensure_daemon(&paths, &ws, spawn.clone()).await?.pf.clone();
    assert_eq!(
        daemon::lock_holder(&paths)?.map(|holder| (holder.pid, holder.listening_on)),
        Some((pf.pid, Some(pf.port))),
        "the daemon names itself in its lock, with the port it listens on"
    );

    std::fs::remove_file(paths.port_file())?;
    let kill = format!("  kill {}", pf.pid);
    let named = format!(
        "no command reaches the daemon (pid {}), which holds the daemon lock: it has listened on port {}, but its port file is gone; end it, and the next command starts a daemon that answers:\n{kill}\n",
        pf.pid, pf.port
    );
    let status = fwd(&repo, &paths, &["daemon", "status"]).output().await?;
    assert_eq!((status.status.code(), String::from_utf8(status.stderr)?), (Some(7), named.clone()));
    for args in [&["--no-autostart", "task", "list"][..], &["daemon", "start"]] {
        let out = fwd(&repo, &paths, args).output().await?;
        let stderr = String::from_utf8(out.stderr)?;
        assert!(stderr.contains(&format!("Error: {named}")), "fwd {args:?} said:\n{stderr}");
        assert!(!out.status.success(), "fwd {args:?} succeeded");
    }

    let stopped = fwd(&repo, &paths, &["daemon", "stop"]).output().await?;
    let stderr = String::from_utf8(stopped.stderr)?;
    assert_eq!(stopped.status.code(), Some(1), "fwd daemon stop said:\n{stderr}");
    assert_eq!(stderr, format!("Error: `fwd daemon stop` stopped nothing, as it sends no signal: {named}"));
    assert!(stopped.stdout.is_empty(), "fwd daemon stop printed {:?}", String::from_utf8_lossy(&stopped.stdout));
    assert!(daemon::daemon_alive(&paths)?, "the daemon still runs: stop signalled nothing");

    // The person runs the line, as printed.
    assert!(std::process::Command::new("sh").args(["-c", kill.trim()]).status()?.success(), "{kill}");
    let again = daemon::ensure_daemon(&paths, &ws, spawn).await?;
    assert_ne!(again.pf.pid, pf.pid, "a new daemon serves");
    drop(again);
    assert!(daemon::stop(&paths).await?);
    Ok(())
}

/// A process that holds the daemon lock and names no daemon there, as a daemon of an earlier build
/// does, is not taken for no daemon: `fwd daemon status` says so and exits 7, and `fwd daemon stop`
/// stops nothing and fails saying so, rather than that no daemon was running.
#[tokio::test]
async fn a_process_that_holds_the_lock_naming_no_daemon_is_not_taken_for_none() -> Result<()> {
    let dir = Scratch::new()?;
    let paths = dir.paths("run", "data");
    let here = dir.path().join("here");
    std::fs::create_dir_all(&here)?;
    let stand_in = StandIn::holding(&paths)?;
    let unnamed = "a process holds the daemon lock but names no daemon there that runs: a daemon of an earlier build of fwd, one that has only just taken the lock, or `fwd admin restore`; `pgrep -fl 'fwd daemon serve'` lists the daemons that run";

    let status = fwd(&here, &paths, &["daemon", "status"]).output().await?;
    assert_eq!((status.status.code(), String::from_utf8(status.stderr)?), (Some(7), format!("{unnamed}\n")));
    let stopped = fwd(&here, &paths, &["daemon", "stop"]).output().await?;
    let stderr = String::from_utf8(stopped.stderr)?;
    assert_eq!(stopped.status.code(), Some(1), "fwd daemon stop said:\n{stderr}");
    assert_eq!(stderr, format!("Error: `fwd daemon stop` stopped nothing: {unnamed}\n"));
    assert!(daemon::daemon_alive(&paths)?, "the stand-in (pid {}) still holds the lock", stand_in.pid);
    Ok(())
}

/// A process that holds the daemon lock and names no daemon there, as a daemon of an earlier build
/// does, played by a process of its own: this test binary run again as `STAND_IN`. It holds the
/// lock until its standard input ends, which dropping it closes, and is collected as soon as it
/// exits.
struct StandIn {
    pid: u32,
    /// Its standard input, whose end ends it.
    input: Option<std::process::ChildStdin>,
    /// Its standard output, kept open until it exits, so that the test harness in it, which reports
    /// there, never writes into a closed pipe.
    output: std::io::Lines<std::io::BufReader<std::process::ChildStdout>>,
}

impl Drop for StandIn {
    /// Closes its standard input, and reads its output to the end, which comes when it has exited.
    fn drop(&mut self) {
        drop(self.input.take());
        for _ in self.output.by_ref() {}
    }
}

/// The test `StandIn` runs in its process.
const STAND_IN: &str = "hold_the_daemon_lock_naming_no_daemon";
/// The variable that tells that process the lock to hold.
const STAND_IN_LOCK: &str = "FWD_TEST_STAND_IN_LOCK";
/// The line that process prints once it holds the lock.
const HOLDING: &str = "holding the daemon lock";

impl StandIn {
    /// Starts the process, and returns once it holds the daemon lock of `paths`.
    fn holding(paths: &Paths) -> Result<StandIn> {
        std::fs::create_dir_all(&paths.runtime_dir)?;
        let mut process = std::process::Command::new(std::env::current_exe()?)
            .args([STAND_IN, "--exact", "--ignored", "--nocapture"])
            .env(STAND_IN_LOCK, paths.lifetime_lock())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        let pid = process.id();
        let input = process.stdin.take();
        let mut output = std::io::BufReader::new(process.stdout.take().expect("piped")).lines();
        std::thread::spawn(move || process.wait());
        loop {
            match output.next().transpose()? {
                Some(line) if line.ends_with(HOLDING) => return Ok(StandIn { pid, input, output }),
                Some(_) => {}
                None => return Err(anyhow!("the stand-in ended before it held {}", paths.lifetime_lock().display())),
            }
        }
    }
}

/// The process `StandIn` runs: takes the lock `STAND_IN_LOCK` names, writes nothing there, says so,
/// and holds the lock until its standard input ends. Run any other way, it does nothing.
#[test]
#[ignore = "the test above runs it in a process of its own, as a daemon of an earlier build"]
fn hold_the_daemon_lock_naming_no_daemon() -> Result<()> {
    let Some(lock) = std::env::var_os(STAND_IN_LOCK) else { return Ok(()) };
    let _held = daemon::try_lock(Path::new(&lock))?.ok_or_else(|| anyhow!("something else holds {lock:?}"))?;
    println!("{HOLDING}");
    std::io::Read::read_to_end(&mut std::io::stdin(), &mut Vec::new())?;
    Ok(())
}
