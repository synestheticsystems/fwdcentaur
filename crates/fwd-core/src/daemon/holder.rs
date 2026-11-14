//! What the daemon writes about itself into the daemon lock (`daemon.lock`), and how a reader
//! verifies it. The record names the process that holds the lock, by its pid and when it started,
//! which tell it apart from a later process the system gave the same pid; only the lock itself says
//! whether anyone holds it ([`super::find`]). docs/design/daemon.md says why the daemon names itself.

use crate::project::Paths;
use anyhow::{anyhow, Context as _, Result};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::path::Path;

/// What the daemon writes into the daemon lock once it holds it, and again once it listens: its
/// pid, and when its process started, so that a record is never taken for whatever has its pid now.
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
    /// This process, which has just taken the daemon lock and does not listen yet; `None` when it
    /// cannot tell itself apart in the system's view of processes (on Linux, a `/proc` that belongs
    /// to another pid namespace), and so must write no record.
    #[cfg(not(target_os = "linux"))]
    pub fn this_process() -> Result<Option<LockHolder>> {
        let pid = std::process::id();
        let process_started =
            process_started(pid)?.ok_or_else(|| anyhow!("the system knows no process {pid}, which is this one"))?;
        Ok(Some(LockHolder { pid, process_started, listening_on: None }))
    }

    /// This process, which has just taken the daemon lock and does not listen yet; `None` when
    /// `/proc/self` describes another pid than this process's own, as a `/proc` that belongs to
    /// another pid namespace does, and so the record would name another process.
    #[cfg(target_os = "linux")]
    pub fn this_process() -> Result<Option<LockHolder>> {
        let pid = std::process::id();
        let stat = std::fs::read_to_string("/proc/self/stat").context("reading /proc/self/stat")?;
        Ok(match Stat::parse(&stat, pid)? {
            Stat::Started(process_started) => Some(LockHolder { pid, process_started, listening_on: None }),
            Stat::AnotherPid(_) => None,
            Stat::Zombie => anyhow::bail!("/proc/self/stat says this process has ended"),
        })
    }

    /// Writes the record at the start of `lock`, the daemon lock, which this process holds, as one
    /// line ended by a newline. The file is never emptied first, so it keeps its inode, and so its
    /// lock; a reader finds the record before, this one, or bytes that are no whole line. The
    /// system does not promise that a write this small is never split, and a split write that then
    /// failed could read as this record's pid and start time with an older record's port: a reader
    /// verifies the pid and the start time, and uses the port only in the reason it prints.
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
    /// A whole record: the first line.
    Whole(LockHolder),
    /// Bytes that are no whole record: a write caught or left midway, or a record no build of
    /// this kind writes.
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

/// When the process with this pid started, as the system tells it: a number only ever compared
/// with itself. `None` when the system describes no process of this pid to this one: none has it,
/// it has ended and waits for its parent to collect it, or it is one the system will not describe.
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
    // ESRCH: no process has the pid, or it has ended and waits to be collected; EPERM: the system
    // will not describe that process to this one.
    let error = std::io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ESRCH | libc::EPERM) => Ok(None),
        _ => Err(error).with_context(|| format!("asking the system when process {pid} started")),
    }
}

/// When the process with this pid started, in clock ticks since the machine booted: a number only
/// ever compared with itself. `None` when no process has the pid, it has ended and waits for its
/// parent to collect it, or `/proc` describes another process under that number (a `/proc` of
/// another pid namespace).
#[cfg(target_os = "linux")]
fn process_started(pid: u32) -> Result<Option<u64>> {
    let path = format!("/proc/{pid}/stat");
    let stat = match std::fs::read_to_string(&path) {
        Ok(stat) => stat,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {path}")),
    };
    Ok(match Stat::parse(&stat, pid)? {
        Stat::Started(started) => Some(started),
        Stat::Zombie | Stat::AnotherPid(_) => None,
    })
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn process_started(pid: u32) -> Result<Option<u64>> {
    anyhow::bail!(
        "fwd cannot ask this system when process {pid} started, so it cannot name the daemon that holds its lock"
    )
}

/// What a Linux `/proc/<pid>/stat` says of the process it was read for.
#[cfg(any(target_os = "linux", test))]
#[derive(Debug, PartialEq, Eq)]
enum Stat {
    /// It runs, and started at this many clock ticks since boot.
    Started(u64),
    /// It has ended and waits for its parent to collect it.
    Zombie,
    /// The file describes the process with this other pid.
    AnotherPid(u32),
}

#[cfg(any(target_os = "linux", test))]
impl Stat {
    /// Reads `stat`, read for the process `pid`. The first field is the pid the file describes; the
    /// second, the command's name in parentheses, may hold spaces and parentheses itself, so the
    /// fields after it are counted from the last `)`: the state is the first after it, and the
    /// start time, the file's 22nd field, the twentieth.
    fn parse(stat: &str, pid: u32) -> Result<Stat> {
        let malformed = || anyhow!("a process's stat names no pid, state or start time: {stat}");
        let (head, rest) = stat.rsplit_once(')').ok_or_else(malformed)?;
        let described: u32 = head.split_whitespace().next().and_then(|p| p.parse().ok()).ok_or_else(malformed)?;
        if described != pid {
            return Ok(Stat::AnotherPid(described));
        }
        let fields: Vec<&str> = rest.split_whitespace().collect();
        let (Some(state), Some(started)) = (fields.first(), fields.get(19)) else { return Err(malformed()) };
        if *state == "Z" {
            return Ok(Stat::Zombie);
        }
        Ok(Stat::Started(started.parse().with_context(|| format!("reading the start time in {stat}"))?))
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::daemon::try_lock;

    /// A runtime and a data folder in `dir`, created.
    pub(in crate::daemon) fn paths(dir: &Path) -> Paths {
        let paths = Paths { runtime_dir: dir.join("run"), data_dir: dir.join("data") };
        paths.ensure().expect("the folders");
        paths
    }

    /// A pid no process has: a child's, once it has exited and been waited for.
    pub(in crate::daemon) fn ended_pid() -> u32 {
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

        let this = LockHolder::this_process().expect("this process").expect("named");
        let listening = LockHolder { listening_on: Some(4242), ..this };
        listening.write(&lock).expect("written");
        assert_eq!(lock_holder(&paths).expect("a record"), Some(listening));
        this.write(&lock).expect("written again");
        assert_eq!(lock_holder(&paths).expect("a record"), Some(this), "a shorter record replaces a longer one");

        // The same pid, given to a later process: the start time differs.
        LockHolder { process_started: this.process_started + 1, ..this }.write(&lock).expect("written");
        assert_eq!(lock_holder(&paths).expect("a record"), None, "a pid the system gave another process");
        LockHolder { pid: ended_pid(), ..this }.write(&lock).expect("written");
        assert_eq!(lock_holder(&paths).expect("a record"), None, "a process that has ended");
    }

    /// What a reader can find in the lock: nothing; a line a write left without its newline; a
    /// shorter record written over a longer one, of which it takes the first line; bytes that are
    /// no record.
    #[test]
    fn a_reader_takes_only_a_whole_first_line_for_a_record() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let lock = dir.path().join("daemon.lock");
        let read = |bytes: &[u8]| {
            std::fs::write(&lock, bytes).expect("written");
            read_record(&lock).expect("read")
        };
        let short = LockHolder { pid: 7, process_started: 8, listening_on: None };
        assert_eq!(read(b""), Record::Empty);
        assert_eq!(read(br#"{"pid":7,"process_started":8"#), Record::Unreadable);
        assert_eq!(read(b"{\"pid\":7,\"process_started\":8}\n,\"listening_on\":4242}\n"), Record::Whole(short));
        assert_eq!(read(b"\x00\x01not a record\n"), Record::Unreadable);
    }

    /// The stat of a process, read for its pid or for another, with a command name holding spaces
    /// and parentheses, and of one that has ended.
    #[test]
    fn a_stat_names_its_process_its_state_and_when_it_started() {
        let stat = |pid: u32, name: &str, state: &str| {
            format!("{pid} ({name}) {state} 1 1 1 0 -1 4194560 100 0 0 0 1 1 0 0 20 0 1 0 98765 1000 100 0")
        };
        assert_eq!(Stat::parse(&stat(42, "fwd", "S"), 42).expect("read"), Stat::Started(98765));
        assert_eq!(Stat::parse(&stat(7, "fwd", "S"), 42).expect("read"), Stat::AnotherPid(7));
        assert_eq!(Stat::parse(&stat(42, "a (b) c) d", "R"), 42).expect("read"), Stat::Started(98765));
        assert_eq!(Stat::parse(&stat(42, "fwd", "Z"), 42).expect("read"), Stat::Zombie);
    }
}
