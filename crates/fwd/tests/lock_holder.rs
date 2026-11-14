//! A process that holds the daemon lock but that no command reaches is never taken for no daemon.
//! The daemon names itself in its lock. When its port file is gone, as macOS's temp cleaner left the
//! daemon of 2026-11-11, `fwd daemon status`, a command that gives up and `fwd daemon stop` name the
//! daemon by its pid, say why no command reaches it and give the `kill` line that ends it; stop
//! sends no signal and fails, and once the person has run that line the next command starts a
//! daemon that answers. A daemon stuck before it listens is named too, and a process that holds the
//! lock naming no daemon there is reported as such.

mod common;

use anyhow::{anyhow, Result};
use common::Scratch;
use fwd_core::daemon::{self, Autostart};
use fwd_core::project::{self, Paths};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

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
/// stops nothing and fails saying so, rather than that no daemon was running. This test's process
/// plays that daemon, holding the lock with no record in it, which the commands it runs find held.
#[tokio::test]
async fn a_process_that_holds_the_lock_naming_no_daemon_is_not_taken_for_none() -> Result<()> {
    let dir = Scratch::new()?;
    let paths = dir.paths("run", "data");
    paths.ensure()?;
    let _held = daemon::try_lock(&paths.lifetime_lock())?.ok_or_else(|| anyhow!("the daemon lock is held"))?;
    let unnamed = "a process holds the daemon lock but names no daemon there that runs: a daemon of an earlier build of fwd, one that has only just taken the lock, or `fwd admin restore`; `pgrep -fl 'fwd daemon serve'` lists the daemons that run";

    let status = fwd(dir.path(), &paths, &["daemon", "status"]).output().await?;
    assert_eq!((status.status.code(), String::from_utf8(status.stderr)?), (Some(7), format!("{unnamed}\n")));
    let stopped = fwd(dir.path(), &paths, &["daemon", "stop"]).output().await?;
    let stderr = String::from_utf8(stopped.stderr)?;
    assert_eq!(stopped.status.code(), Some(1), "fwd daemon stop said:\n{stderr}");
    assert_eq!(stderr, format!("Error: `fwd daemon stop` stopped nothing: {unnamed}\n"));
    Ok(())
}

/// A daemon stuck before it listens is named as one that has not begun to listen, since it names
/// itself in the lock before it reads its settings. Here its settings file is a pipe this test holds
/// open for writing and never writes, so the daemon waits there, holding the lock; opening the pipe
/// returns only once the daemon has opened it to read.
#[tokio::test]
async fn a_daemon_stuck_before_it_listens_is_named() -> Result<()> {
    let dir = Scratch::new()?;
    let paths = dir.paths("run", "data");
    paths.ensure()?;
    let settings = paths.daemon_config_path();
    let pipe = std::ffi::CString::new(settings.as_os_str().as_bytes())?;
    // SAFETY: `pipe` is a path ended by a NUL, which mkfifo only reads.
    if unsafe { libc::mkfifo(pipe.as_ptr(), 0o600) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut stuck = tokio::process::Command::new(env!("CARGO_BIN_EXE_fwd"))
        .args(["daemon", "serve", "--runtime-dir"])
        .arg(&paths.runtime_dir)
        .arg("--data-dir")
        .arg(&paths.data_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    // On a thread of its own, so that a daemon that fails before it reads its settings fails the
    // test rather than leave it waiting on the pipe.
    let (opened, settings_opened) = std::sync::mpsc::channel();
    std::thread::spawn(move || opened.send(std::fs::OpenOptions::new().write(true).open(settings)));
    let _held_open = settings_opened.recv_timeout(Duration::from_secs(60))??;

    let pid = stuck.id().ok_or_else(|| anyhow!("the daemon has exited"))?;
    let status = fwd(dir.path(), &paths, &["daemon", "status"]).output().await?;
    assert_eq!(
        (status.status.code(), String::from_utf8(status.stderr)?),
        (Some(7), format!("no command reaches the daemon (pid {pid}), which holds the daemon lock: it has not begun to listen: it is still opening its projects, or stuck before it listens; end it, and the next command starts a daemon that answers:\n  kill {pid}\n"))
    );
    stuck.kill().await?;
    Ok(())
}
