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
//!
//! The record names the holder; only the lock says whether anyone holds it. A record that names no
//! daemon that runs is therefore never taken for "no daemon": [`find`] asks the lock itself.

use super::{answering, read_port_file, take_bootstrap, try_lock, Running};
use crate::project::Paths;
use anyhow::{anyhow, Context as _, Result};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::path::Path;

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

    /// Publishes the record in `lock`, the daemon lock, which this process holds: one line, ended by
    /// a newline, put in place by one write at the start of the file, which is never emptied first.
    /// A reader finds the record before or this one whole, or, if its read caught the write midway,
    /// bytes that are no whole line, which it cannot take for an empty lock. A record shorter than
    /// the one before leaves that one's end after its newline, where readers do not look. The file
    /// keeps its inode, and so its lock.
    pub fn write(&self, lock: &File) -> Result<()> {
        use std::os::unix::fs::FileExt;
        let mut line = serde_json::to_vec(self)?;
        line.push(b'\n');
        lock.write_all_at(&line, 0).context("writing the daemon's record into its lock")
    }

    /// Whether the recorded process still runs, as that process and not another given its pid.
    fn runs(&self) -> Result<bool> {
        Ok(process_started(self.pid)? == Some(self.process_started))
    }
}

/// What a reader finds in the daemon lock.
#[derive(Debug, PartialEq, Eq)]
enum Record {
    /// Nothing: no daemon of this build has written there.
    Empty,
    /// A whole record.
    Whole(LockHolder),
    /// Bytes that are no whole record: a read that caught a write midway, or a record no build
    /// of this kind writes.
    Unreadable,
}

fn read_record(path: &Path) -> Result<Record> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Record::Empty),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    if bytes.is_empty() {
        return Ok(Record::Empty);
    }
    let Some(end) = bytes.iter().position(|byte| *byte == b'\n') else { return Ok(Record::Unreadable) };
    Ok(serde_json::from_slice(&bytes[..end]).map_or(Record::Unreadable, Record::Whole))
}

/// The daemon process that holds the daemon lock, by the record it wrote there; `None` when the
/// record names no daemon that runs: there is none, it cannot be read, or it names a process that
/// has ended. `None` does not mean that no daemon runs: a daemon of an earlier build writes no
/// record, and one that has just taken the lock has not written yet. A recorded process that runs
/// holds the lock: the daemon writes the record only once it holds the lock, and keeps the lock
/// until its process ends, when the system frees it.
pub fn lock_holder(paths: &Paths) -> Result<Option<LockHolder>> {
    match read_record(&paths.lifetime_lock())? {
        Record::Whole(holder) => Ok(holder.runs()?.then_some(holder)),
        Record::Empty | Record::Unreadable => Ok(None),
    }
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
/// file names one; `None` when the lock's record names no daemon that runs ([`lock_holder`]).
pub(super) fn not_answering(paths: &Paths) -> Result<Option<NotAnswering>> {
    let Some(holder) = lock_holder(paths)? else { return Ok(None) };
    let its_port_file = read_port_file(&paths.port_file())?.filter(|pf| pf.pid == holder.pid);
    let why = match (its_port_file, holder.listening_on) {
        (Some(pf), _) => Unreached::NoAnswer { port: pf.port },
        (None, Some(port)) => Unreached::PortFileGone { port },
        (None, None) => Unreached::NotListening,
    };
    Ok(Some(NotAnswering { holder, why }))
}

/// A process holds the daemon lock and names no daemon there that runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("a process holds the daemon lock but names no daemon there that runs: a daemon of an earlier build of fwd, one that has only just taken the lock, or `fwd admin restore`; `pgrep -fl 'fwd daemon serve'` lists the daemons that run")]
pub struct Unnamed;

/// This runtime directory's daemon, as `fwd daemon status` and `fwd daemon stop` find it.
#[derive(Debug)]
pub enum Found {
    /// A daemon answers as the port file names it.
    Answering(Running),
    /// The lifetime lock names the daemon that holds it, which no command reaches.
    Unreached(NotAnswering),
    /// A process holds the lifetime lock and names no daemon there that runs.
    Unnamed,
    /// No process holds the lifetime lock: no daemon runs.
    Absent,
}

/// Finds this runtime directory's daemon; never autostarts. Whether a daemon that no record names
/// holds the lifetime lock only the lock tells, by being taken for a moment, and a daemon starting
/// in that moment would find it held and exit. Launchers spawn a daemon only under the bootstrap
/// lock, and keep it until the daemon answers, so the lock is tried under the bootstrap lock too,
/// after asking once more for an answer from a daemon that may have started meanwhile.
pub async fn find(paths: &Paths) -> Result<Found> {
    if let Some(running) = answering(paths).await? {
        return Ok(Found::Answering(running));
    }
    if let Some(daemon) = not_answering(paths)? {
        return Ok(Found::Unreached(daemon));
    }
    // A lifetime lock that was never created was never held; nothing in fwd removes it.
    if !paths.lifetime_lock().exists() {
        return Ok(Found::Absent);
    }
    let _bootstrap = match take_bootstrap(paths).await {
        Ok(lock) => lock,
        Err(error) => return error.downcast::<NotAnswering>().map(Found::Unreached),
    };
    if let Some(running) = answering(paths).await? {
        return Ok(Found::Answering(running));
    }
    if let Some(daemon) = not_answering(paths)? {
        return Ok(Found::Unreached(daemon));
    }
    Ok(match try_lock(&paths.lifetime_lock())? {
        Some(_) => Found::Absent,
        None => Found::Unnamed,
    })
}

/// Why `fwd daemon stop` stopped nothing although a process holds the daemon lock: it stops only a
/// daemon that answers, and sends no signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StoppedNothing {
    /// The daemon the lock names, which no command reaches.
    #[error("`fwd daemon stop` stopped nothing, as it sends no signal: {0}")]
    Unreached(NotAnswering),
    /// A process that names no daemon there.
    #[error("`fwd daemon stop` stopped nothing: {0}")]
    Unnamed(Unnamed),
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
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Barrier;

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

        let listening = LockHolder { listening_on: Some(4242), ..LockHolder::this_process().expect("this process") };
        listening.write(&lock).expect("written");
        assert_eq!(lock_holder(&paths).expect("a record"), Some(listening));
        let this = LockHolder { listening_on: None, ..listening };
        this.write(&lock).expect("written again");
        assert_eq!(lock_holder(&paths).expect("a record"), Some(this), "a shorter record replaces a longer one");

        // The same pid, given to a later process: the start time differs.
        LockHolder { process_started: this.process_started + 1, ..this }.write(&lock).expect("written");
        assert_eq!(lock_holder(&paths).expect("a record"), None, "a pid the system gave another process");
        LockHolder { pid: ended_pid(), ..this }.write(&lock).expect("written");
        assert_eq!(lock_holder(&paths).expect("a record"), None, "a process that has ended");
    }

    /// A reader running while the daemon publishes its record again and again never finds the lock
    /// empty, which it would take for a lock no daemon has written: it finds a whole record, or,
    /// should its read catch a write midway, bytes it cannot read. The two start together at a
    /// barrier, and the reader reads until the writer says it has written them all.
    #[test]
    fn a_reader_never_finds_the_record_being_published_empty() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let paths = paths(dir.path());
        let lock = try_lock(&paths.lifetime_lock()).expect("the lock").expect("free");
        let this = LockHolder::this_process().expect("this process");
        this.write(&lock).expect("the first record");
        let records = [this, LockHolder { listening_on: Some(4242), ..this }];
        let start = Barrier::new(2);
        let written = AtomicBool::new(false);
        let found = std::thread::scope(|scope| {
            scope.spawn(|| {
                start.wait();
                for record in records.iter().cycle().take(20_000) {
                    record.write(&lock).expect("written");
                }
                written.store(true, Ordering::Release);
            });
            let reader = scope.spawn(|| {
                start.wait();
                let mut found = Vec::new();
                while !written.load(Ordering::Acquire) {
                    found.push(read_record(&paths.lifetime_lock()).expect("read"));
                }
                found
            });
            reader.join().expect("the reader")
        });
        assert!(!found.contains(&Record::Empty), "a reader found the lock empty while a record was published");
        assert!(found.iter().any(|record| *record == Record::Whole(records[1])), "the reader read during the writes");
    }

    /// Only the lock says whether a process holds it: one whose record names no daemon that runs,
    /// or that wrote none, is told apart from no daemon at all.
    #[tokio::test]
    async fn a_lock_held_by_a_process_that_names_no_daemon_is_not_taken_for_no_daemon() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let never_made = Paths { runtime_dir: dir.path().join("never made"), data_dir: dir.path().join("data") };
        assert!(matches!(find(&never_made).await.expect("found"), Found::Absent), "no runtime folder, so no lock");
        let paths = paths(dir.path());
        drop(try_lock(&paths.lifetime_lock()).expect("the lock").expect("free"));
        assert!(matches!(find(&paths).await.expect("found"), Found::Absent), "a lock no one holds");

        let lock = try_lock(&paths.lifetime_lock()).expect("the lock").expect("free");
        assert!(matches!(find(&paths).await.expect("found"), Found::Unnamed), "a held lock with no record");
        let this = LockHolder::this_process().expect("this process");
        LockHolder { pid: ended_pid(), ..this }.write(&lock).expect("written");
        assert!(matches!(find(&paths).await.expect("found"), Found::Unnamed), "a held lock naming an ended process");
        this.write(&lock).expect("written");
        match find(&paths).await.expect("found") {
            Found::Unreached(daemon) => assert_eq!((daemon.holder, daemon.why), (this, Unreached::NotListening)),
            other => panic!("found {other:?} where the lock names this process"),
        }
    }

    /// The lifetime lock is tried only under the bootstrap lock, so that a daemon a launcher has just
    /// spawned never finds it taken for that moment and exits: while another holds the bootstrap
    /// lock, a free lifetime lock is not taken for no daemon, and the wait for the bootstrap lock
    /// ends in its own error.
    #[tokio::test]
    async fn the_lifetime_lock_is_tried_only_under_the_bootstrap_lock() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let paths = paths(dir.path());
        drop(try_lock(&paths.lifetime_lock()).expect("the lock").expect("free"));
        let _bootstrap = try_lock(&paths.bootstrap_lock()).expect("the bootstrap lock").expect("free");
        let error = find(&paths).await.expect_err("the bootstrap lock stays held");
        assert!(error.to_string().starts_with("timed out waiting for the bootstrap lock"), "{error:#}");
    }

    /// A launcher that gives up waiting for the bootstrap lock, which another launcher holds while
    /// it waits on the daemon that holds the lifetime lock, names that daemon when no command reaches
    /// it, as the other launcher does. This process holds both locks before the launcher starts.
    #[tokio::test]
    async fn a_launcher_that_gives_up_on_the_bootstrap_lock_names_the_daemon_no_command_reaches() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let paths = paths(dir.path());
        let lock = try_lock(&paths.lifetime_lock()).expect("the lock").expect("free");
        let holder = LockHolder { listening_on: Some(4242), ..LockHolder::this_process().expect("this process") };
        holder.write(&lock).expect("written");
        let _bootstrap = try_lock(&paths.bootstrap_lock()).expect("the bootstrap lock").expect("free");

        let spawn = crate::daemon::Autostart::Spawn { daemon_bin: "/nonexistent/fwd".into() };
        let error = crate::daemon::ensure_running(&paths, &spawn).await.expect_err("no daemon answers");
        assert_eq!(
            error.downcast_ref::<NotAnswering>(),
            Some(&NotAnswering { holder, why: Unreached::PortFileGone { port: 4242 } }),
            "{error:#}"
        );
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
