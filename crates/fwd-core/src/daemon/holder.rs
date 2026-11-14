//! The daemon that holds the daemon lock (`daemon.lock`) but that no command reaches, named by the
//! record it writes into the lock, so that `fwd daemon status`, `fwd daemon stop` and a command
//! that gives up waiting say which process it is, why no command reaches it, and the `kill` line
//! that ends it, instead of that no daemon runs. Nothing in fwd signals it: `fwd daemon stop`
//! stops only a daemon that answers (docs/DECISIONS.md, D173).
//!
//! Every command finds the daemon through the port file alone, and a launcher that finds the lock
//! held waits for its holder rather than start a daemon that would find the lock held and exit. So
//! a daemon that holds the lock while no command reaches it (its port file gone, no answer on its
//! port, or stuck before it listens) leaves every command waiting and giving up until it goes. On
//! 2026-11-11 macOS's temp cleaner removed the port file of a daemon that went on serving, and for
//! nine hours `fwd daemon status` and `fwd daemon stop` said that no daemon ran while every command
//! gave up, until its process was ended by hand.

use super::read_port_file;
use crate::project::Paths;
use anyhow::{anyhow, Context as _, Result};
use serde::{Deserialize, Serialize};
use std::fs::File;

/// What the daemon writes into the daemon lock once it holds it, and again once it listens: its
/// pid, and when its process started, which tells that process apart from a later one the system
/// gave the same pid, so that a record is never taken for whatever has its pid now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockHolder {
    pub pid: u32,
    /// When the process started, as the system tells it: only ever compared with itself.
    pub process_started: u64,
    /// The port the daemon listens on, written once it listens and its port file is in place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listening_on: Option<u16>,
}

impl LockHolder {
    /// This process, which has just taken the daemon lock and does not listen yet.
    pub fn this_process() -> Result<LockHolder> {
        let pid = std::process::id();
        let process_started =
            process_started(pid)?.ok_or_else(|| anyhow!("the system knows no process {pid}, which is this one"))?;
        Ok(LockHolder { pid, process_started, listening_on: None })
    }

    /// Writes the record into `lock`, the daemon lock, which this process holds. It is emptied first
    /// and written whole from its start, so a reader finds the record before, none, or this one.
    pub fn write(&self, lock: &File) -> Result<()> {
        use std::os::unix::fs::FileExt;
        lock.set_len(0).context("emptying the daemon lock")?;
        lock.write_all_at(&serde_json::to_vec(self)?, 0).context("writing the daemon's record into its lock")?;
        Ok(())
    }

    /// Whether the recorded process still runs, as that process and not another given its pid.
    fn runs(&self) -> Result<bool> {
        Ok(process_started(self.pid)? == Some(self.process_started))
    }
}

/// The daemon process that holds the daemon lock, by the record it wrote there; `None` when the lock
/// carries no record (no daemon of this build has held it, or one has just taken it and not written
/// yet) or the record of a process that has ended. A recorded process that still runs holds the
/// lock: the daemon writes the record only once it holds the lock, and keeps the lock until its
/// process ends, when the system frees it.
pub fn lock_holder(paths: &Paths) -> Result<Option<LockHolder>> {
    let path = paths.lifetime_lock();
    let record = match std::fs::read(&path) {
        Ok(record) => record,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    if record.is_empty() {
        return Ok(None);
    }
    let holder: LockHolder = serde_json::from_slice(&record)
        .with_context(|| format!("reading the record of the daemon that holds {}", path.display()))?;
    Ok(holder.runs()?.then_some(holder))
}

/// A daemon that holds the daemon lock, by its record, and that no command reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("no command reaches the daemon (pid {pid}), which holds the daemon lock: {why}; end it, and the next command starts a daemon that answers:\n  kill {pid}", pid = .holder.pid)]
pub struct NotAnswering {
    pub holder: LockHolder,
    pub why: Unreached,
}

/// Why no command reaches the daemon that holds the daemon lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unreached {
    /// It has not begun to listen: it is still opening its projects, or stuck before it listens.
    NotListening,
    /// It has listened, and its port file is gone: removed by something else while it listens, or
    /// by the daemon itself once it stopped listening, on its way out.
    PortFileGone { port: u16 },
    /// Its port file names its port, where it gave no answer.
    NoAnswer { port: u16 },
}

impl std::fmt::Display for Unreached {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unreached::NotListening => {
                f.write_str("it has not begun to listen: it is still opening its projects, or stuck before it listens")
            }
            Unreached::PortFileGone { port } => write!(f, "it has listened on port {port}, but its port file is gone"),
            Unreached::NoAnswer { port } => write!(f, "it gives no answer on port {port}, which its port file names"),
        }
    }
}

/// The daemon that holds the daemon lock, for a caller that found no daemon answering as the port
/// file names one (`status`); `None` when the lock has no recorded holder that still runs.
pub fn not_answering(paths: &Paths) -> Result<Option<NotAnswering>> {
    let Some(holder) = lock_holder(paths)? else { return Ok(None) };
    let its_port_file = read_port_file(&paths.port_file())?.filter(|pf| pf.pid == holder.pid);
    let why = match (its_port_file, holder.listening_on) {
        (Some(pf), _) => Unreached::NoAnswer { port: pf.port },
        (None, Some(port)) => Unreached::PortFileGone { port },
        (None, None) => Unreached::NotListening,
    };
    Ok(Some(NotAnswering { holder, why }))
}

/// Why `fwd daemon stop` stopped nothing although a process holds the daemon lock: it stops only a
/// daemon that answers, and sends no signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StoppedNothing {
    /// The daemon the lock names, which no command reaches.
    #[error("`fwd daemon stop` stopped nothing, as it sends no signal: {0}")]
    Unreached(NotAnswering),
}

/// When the process with this pid started, as the system tells it: a number only ever compared
/// with itself. `None` when no process of this user's has the pid: none has it, it has ended and
/// waits for its parent to collect it, or it is another user's, of which the system tells this one
/// nothing, and which is never this user's daemon.
#[cfg(target_os = "macos")]
fn process_started(pid: u32) -> Result<Option<u64>> {
    let Ok(pid) = libc::c_int::try_from(pid) else { return Ok(None) };
    // SAFETY: proc_bsdinfo is plain integers and byte arrays, for which all zeroes is a value.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let buffer = (&mut info as *mut libc::proc_bsdinfo).cast();
    // SAFETY: `buffer` is valid for writes of `size` bytes, and proc_pidinfo writes at most that many.
    let written = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, buffer, size) };
    if written == size {
        return Ok(Some(info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec));
    }
    // ESRCH: no process has the pid, or it has ended and waits to be collected; EPERM: another
    // user's process has it.
    let error = std::io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ESRCH | libc::EPERM) => Ok(None),
        _ => Err(error).with_context(|| format!("asking the system when process {pid} started")),
    }
}

/// When the process with this pid started, in clock ticks since the machine booted (the 22nd field
/// of `/proc/<pid>/stat`): a number only ever compared with itself. `None` when no process has the
/// pid, or it has ended and waits for its parent to collect it.
#[cfg(target_os = "linux")]
fn process_started(pid: u32) -> Result<Option<u64>> {
    let path = format!("/proc/{pid}/stat");
    let stat = match std::fs::read_to_string(&path) {
        Ok(stat) => stat,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {path}")),
    };
    // The second field, the command's name in parentheses, may hold spaces and parentheses itself,
    // so the fields are counted from the last `)`: the third, the state, is the first after it.
    let fields: Vec<&str> =
        stat.rsplit_once(')').map(|(_, rest)| rest.split_whitespace().collect()).unwrap_or_default();
    let (Some(state), Some(started)) = (fields.first(), fields.get(19)) else {
        anyhow::bail!("{path} has no state or start time: {stat}");
    };
    if *state == "Z" {
        return Ok(None);
    }
    Ok(Some(started.parse().with_context(|| format!("reading the start time in {path}"))?))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn process_started(pid: u32) -> Result<Option<u64>> {
    anyhow::bail!(
        "fwd cannot ask this system when process {pid} started, so it cannot name the daemon that holds its lock"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::{try_lock, write_port_file, Bind, PortFile};

    /// A runtime and a data folder in `dir`, created.
    fn paths(dir: &std::path::Path) -> Paths {
        let paths = Paths { runtime_dir: dir.join("run"), data_dir: dir.join("data") };
        paths.ensure().expect("the folders");
        paths
    }

    /// A pid no process has: a child's, once it has exited and been waited for.
    fn ended_pid() -> u32 {
        let mut child = std::process::Command::new("true").spawn().expect("a child");
        child.wait().expect("the child exits");
        child.id()
    }

    #[test]
    fn the_record_names_the_process_that_wrote_it_only_while_that_process_runs() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let paths = paths(dir.path());
        assert_eq!(lock_holder(&paths).expect("no lock yet"), None);
        let lock = try_lock(&paths.lifetime_lock()).expect("the lock").expect("free");
        assert_eq!(lock_holder(&paths).expect("an empty lock"), None, "a lock with no record names no one");

        let this = LockHolder::this_process().expect("this process");
        this.write(&lock).expect("written");
        assert_eq!(lock_holder(&paths).expect("a record"), Some(this));
        let listening = LockHolder { listening_on: Some(4242), ..this };
        listening.write(&lock).expect("written again");
        assert_eq!(lock_holder(&paths).expect("a record"), Some(listening), "the second record replaces the first");

        // The same pid, given to a later process: the start time differs.
        LockHolder { process_started: this.process_started + 1, ..this }.write(&lock).expect("written");
        assert_eq!(lock_holder(&paths).expect("a record"), None, "a pid the system gave another process");
        LockHolder { pid: ended_pid(), ..this }.write(&lock).expect("written");
        assert_eq!(lock_holder(&paths).expect("a record"), None, "a process that has ended");
    }

    #[test]
    fn a_daemon_no_command_reaches_is_named_with_the_reason() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let paths = paths(dir.path());
        let lock = try_lock(&paths.lifetime_lock()).expect("the lock").expect("free");
        let this = LockHolder::this_process().expect("this process");
        this.write(&lock).expect("written");
        let why = || not_answering(&paths).expect("the holder").map(|daemon| daemon.why);
        assert_eq!(why(), Some(Unreached::NotListening));

        let listening = LockHolder { listening_on: Some(4242), ..this };
        listening.write(&lock).expect("written");
        assert_eq!(why(), Some(Unreached::PortFileGone { port: 4242 }));
        // A port file another daemon left is not this one's.
        let other =
            PortFile { port: 4343, pid: ended_pid(), instance_id: "other".into(), started: 0, bind: Bind::Loopback };
        write_port_file(&paths.port_file(), &other).expect("written");
        assert_eq!(why(), Some(Unreached::PortFileGone { port: 4242 }));
        write_port_file(&paths.port_file(), &PortFile { port: 4242, pid: this.pid, ..other }).expect("written");
        assert_eq!(why(), Some(Unreached::NoAnswer { port: 4242 }));
        assert_eq!(
            not_answering(&paths).expect("the holder").expect("named").to_string(),
            format!(
                "no command reaches the daemon (pid {pid}), which holds the daemon lock: it gives no answer on port 4242, which its port file names; end it, and the next command starts a daemon that answers:\n  kill {pid}",
                pid = this.pid
            )
        );

        LockHolder { pid: ended_pid(), ..listening }.write(&lock).expect("written");
        assert_eq!(why(), None, "the record of a daemon that has ended names no one");
    }
}
