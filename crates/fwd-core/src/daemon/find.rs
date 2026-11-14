//! What this runtime directory's daemon is, for a command that asks: one that answers; one that
//! holds the lifetime lock and that no command reaches, named by its record ([`super::holder`]); a
//! process that holds the lock and names no daemon there; another command busy starting the
//! daemon or looking for it; or none. `fwd daemon status`, `fwd daemon stop`, a command run with
//! `--no-autostart` and a launcher at its deadline all classify here, so that none of them says
//! that no daemon runs while a process holds the lock, and a `kill` line is printed only for a
//! process just verified, behind a probe that has just failed.

use super::holder::{lock_holder, LockHolder};
use super::{probe, try_lock, Probe, Running, Unanswered};
use crate::project::Paths;
use anyhow::Result;
use std::fs::File;

/// This runtime directory's daemon, as [`find`] finds it.
#[derive(Debug)]
pub enum Found {
    /// A daemon answers as the port file names it.
    Answering(Running),
    /// The lifetime lock names the daemon that holds it, which no command reaches.
    Unreached(NotAnswering),
    /// A process holds the lifetime lock and names no daemon there that runs.
    Unnamed,
    /// Another command holds the bootstrap lock, starting the daemon or looking for it.
    Busy,
    /// No process holds the lifetime lock: no daemon runs.
    Absent,
}

/// Finds this runtime directory's daemon, without waiting and without starting one. Whether a
/// process that no record names holds the lifetime lock only the lock tells, by being taken for a
/// moment, so that is asked only under the bootstrap lock ([`classify`]); while another command
/// holds that, the answer is [`Found::Busy`].
pub async fn find(paths: &Paths) -> Result<Found> {
    let unanswered = match probe(paths).await? {
        Probe::Answering(running) => return Ok(Found::Answering(running)),
        Probe::Unanswered(unanswered) => unanswered,
    };
    if let Some(daemon) = not_answering(paths, &unanswered)? {
        return Ok(Found::Unreached(daemon));
    }
    // A lifetime lock that was never created was never held; nothing in fwd removes it.
    if !paths.lifetime_lock().exists() {
        return Ok(Found::Absent);
    }
    let Some(bootstrap) = try_lock(&paths.bootstrap_lock())? else { return Ok(Found::Busy) };
    let unanswered = match probe(paths).await? {
        Probe::Answering(running) => return Ok(Found::Answering(running)),
        Probe::Unanswered(unanswered) => unanswered,
    };
    Ok(match classify(paths, &bootstrap, &unanswered)? {
        LockHeld::Named(daemon) => Found::Unreached(daemon),
        LockHeld::Unnamed => Found::Unnamed,
        LockHeld::Free => Found::Absent,
    })
}

/// Who holds the lifetime lock, as [`classify`] tells.
#[derive(Debug)]
pub(super) enum LockHeld {
    /// The daemon its record names, verified.
    Named(NotAnswering),
    /// A process that names no daemon there.
    Unnamed,
    /// No process.
    Free,
}

/// Who holds the lifetime lock, for a caller that holds the bootstrap lock (`_bootstrap`, the
/// proof) and whose probe has just found no daemon answering (`unanswered`): the daemon the lock's
/// record names, verified, or else whoever the lock itself says, tried once. Trying it takes it for
/// a moment, and a daemon starting in that moment would find it held and exit; launchers spawn only
/// under the bootstrap lock, so under it none is starting.
pub(super) fn classify(paths: &Paths, _bootstrap: &File, unanswered: &Unanswered) -> Result<LockHeld> {
    if let Some(daemon) = not_answering(paths, unanswered)? {
        return Ok(LockHeld::Named(daemon));
    }
    Ok(match try_lock(&paths.lifetime_lock())? {
        Some(_) => LockHeld::Free,
        None => LockHeld::Unnamed,
    })
}

/// The daemon the lifetime lock's record names, verified, and why no command reaches it, given what
/// the probe that has just failed found; `None` when the record names no daemon that runs
/// ([`lock_holder`]). The port file counts as the holder's when it names the holder's pid and, once
/// the holder has recorded its port, that port.
pub(super) fn not_answering(paths: &Paths, unanswered: &Unanswered) -> Result<Option<NotAnswering>> {
    let Some(holder) = lock_holder(paths)? else { return Ok(None) };
    let why = match (unanswered, holder.listening_on) {
        (Unanswered::Failed { pf, why }, recorded)
            if pf.pid == holder.pid && recorded.is_none_or(|port| port == pf.port) =>
        {
            Unreached::NoValidAnswer { port: pf.port, why: why.clone() }
        }
        (_, None) => Unreached::NotListening,
        (Unanswered::NoPortFile, Some(port)) => Unreached::PortFileMissing { port },
        (Unanswered::Failed { pf, .. }, Some(port)) => {
            Unreached::PortFileNamesAnother { port, other_pid: pf.pid, other_port: pf.port }
        }
    };
    Ok(Some(NotAnswering { holder, why }))
}

/// A daemon that holds the daemon lock, by its record, and that no command reaches.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("no command reaches the daemon (pid {pid}), which holds the daemon lock: {why}; ending it lets the next command start a fresh daemon:\n  kill {pid}", pid = .holder.pid)]
pub struct NotAnswering {
    pub holder: LockHolder,
    pub why: Unreached,
}

/// Why no command reaches the daemon that holds the daemon lock, as the probe found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unreached {
    /// It has not begun to listen: it is still opening its projects, or stuck before it listens.
    NotListening,
    /// It has listened on `port`, and its port file is missing.
    PortFileMissing { port: u16 },
    /// It has listened on `port`, and the port file names another process.
    PortFileNamesAnother { port: u16, other_pid: u32, other_port: u16 },
    /// Its port file names it, on `port`, and the probe there failed for `why`.
    NoValidAnswer { port: u16, why: String },
}

impl std::fmt::Display for Unreached {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unreached::NotListening => {
                f.write_str("it has not begun to listen: it is still opening its projects, or stuck before it listens")
            }
            Unreached::PortFileMissing { port } => write!(f, "it has listened on port {port}, but its port file is gone"),
            Unreached::PortFileNamesAnother { port, other_pid, other_port } => write!(
                f,
                "it has listened on port {port}, but the port file now names another process, pid {other_pid}, port {other_port}"
            ),
            Unreached::NoValidAnswer { port, why } => {
                write!(f, "it gives no valid answer on port {port}, which its port file names: {why}")
            }
        }
    }
}

/// A process holds the daemon lock and names no daemon there that runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("a process holds the daemon lock but names no daemon there that runs: a daemon of an earlier build of fwd, one that has only just taken the lock or cannot name itself, or `fwd admin restore`; `pgrep -fl 'fwd daemon serve'` lists the daemons that run")]
pub struct Unnamed;

/// Another command holds the bootstrap lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("another fwd command is starting the daemon, or looking for it as this one is; ask again")]
pub struct Busy;

/// Why `fwd daemon stop` stopped nothing although a daemon or a process holds the daemon lock: it
/// stops only a daemon that answers and takes the stop, and sends no signal.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoppedNothing {
    /// The daemon the lock names, which no command reaches.
    #[error("`fwd daemon stop` stopped nothing, as it sends no signal: {0}")]
    Unreached(NotAnswering),
    /// A process that names no daemon there.
    #[error("`fwd daemon stop` stopped nothing: {0}")]
    Unnamed(Unnamed),
    /// Another command busy starting the daemon or looking for it.
    #[error("`fwd daemon stop` stopped nothing: {0}")]
    Busy(Busy),
    /// A daemon that answers but did not take the stop.
    #[error(
        "`fwd daemon stop` stopped nothing: the daemon (pid {pid}) answers on port {port} but did not take the stop"
    )]
    DidNotTakeTheStop { pid: u32, port: u16 },
}

impl StoppedNothing {
    /// What `fwd daemon stop` reports for `found`: `Ok(false)` when no daemon runs.
    pub(super) fn for_found(found: Found) -> Result<bool> {
        Err(match found {
            Found::Absent => return Ok(false),
            Found::Answering(running) => {
                StoppedNothing::DidNotTakeTheStop { pid: running.pf.pid, port: running.pf.port }
            }
            Found::Unreached(daemon) => StoppedNothing::Unreached(daemon),
            Found::Unnamed => StoppedNothing::Unnamed(Unnamed),
            Found::Busy => StoppedNothing::Busy(Busy),
        }
        .into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::holder::tests::{ended_pid, paths};
    use crate::daemon::{Bind, PortFile};

    fn port_file(pid: u32, port: u16) -> PortFile {
        PortFile { port, pid, instance_id: "a-daemon".into(), started: 0, bind: Bind::Loopback }
    }

    /// One reason for each state of the record and of what the probe found, each saying what was
    /// observed.
    #[test]
    fn the_reason_says_what_the_probe_found() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let paths = paths(dir.path());
        let lock = try_lock(&paths.lifetime_lock()).expect("the lock").expect("free");
        let this = LockHolder::this_process().expect("this process").expect("named");
        this.write(&lock).expect("written");
        let failed = |pf: PortFile| Unanswered::Failed { pf, why: "nothing is listening on port 4242".into() };
        let why = |unanswered: &Unanswered| not_answering(&paths, unanswered).expect("the holder").map(|d| d.why);
        assert_eq!(why(&Unanswered::NoPortFile), Some(Unreached::NotListening));

        let listening = LockHolder { listening_on: Some(4242), ..this };
        listening.write(&lock).expect("written");
        assert_eq!(why(&Unanswered::NoPortFile), Some(Unreached::PortFileMissing { port: 4242 }));
        let other = ended_pid();
        assert_eq!(
            why(&failed(port_file(other, 4343))),
            Some(Unreached::PortFileNamesAnother { port: 4242, other_pid: other, other_port: 4343 })
        );
        assert_eq!(
            why(&failed(port_file(this.pid, 4343))),
            Some(Unreached::PortFileNamesAnother { port: 4242, other_pid: this.pid, other_port: 4343 }),
            "a port file with this pid but another port than the record's is not this daemon's"
        );
        let named = not_answering(&paths, &failed(port_file(this.pid, 4242))).expect("the holder").expect("named");
        assert_eq!(
            named.to_string(),
            format!(
                "no command reaches the daemon (pid {pid}), which holds the daemon lock: it gives no valid answer on port 4242, which its port file names: nothing is listening on port 4242; ending it lets the next command start a fresh daemon:\n  kill {pid}",
                pid = this.pid
            )
        );

        LockHolder { pid: ended_pid(), ..listening }.write(&lock).expect("written");
        assert_eq!(why(&Unanswered::NoPortFile), None, "the record of a daemon that has ended names no one");
    }

    /// Only the lock says whether a process holds it: one whose record names no daemon that runs,
    /// or that wrote none, is told apart from no daemon at all.
    #[tokio::test]
    async fn a_lock_held_by_a_process_that_names_no_daemon_is_not_taken_for_no_daemon() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let never_made = Paths { runtime_dir: dir.path().join("never made"), data_dir: dir.path().join("data") };
        assert!(matches!(find(&never_made).await.expect("found"), Found::Absent), "no runtime folder, so no lock");
        // Made, never locked: a lock this process has let go of can stay held for a moment by a
        // child another test is spawning, which starts with a copy of this process's open files.
        let free = paths(&dir.path().join("free"));
        File::create(free.lifetime_lock()).expect("the lock file");
        assert!(matches!(find(&free).await.expect("found"), Found::Absent), "a lock no one holds");

        let paths = paths(&dir.path().join("held"));
        let lock = try_lock(&paths.lifetime_lock()).expect("the lock").expect("free");
        assert!(matches!(find(&paths).await.expect("found"), Found::Unnamed), "a held lock with no record");
        let this = LockHolder::this_process().expect("this process").expect("named");
        LockHolder { pid: ended_pid(), ..this }.write(&lock).expect("written");
        assert!(matches!(find(&paths).await.expect("found"), Found::Unnamed), "a held lock naming an ended process");
        this.write(&lock).expect("written");
        match find(&paths).await.expect("found") {
            Found::Unreached(daemon) => assert_eq!((daemon.holder, daemon.why), (this, Unreached::NotListening)),
            other => panic!("found {other:?} where the lock names this process"),
        }
    }

    /// While another command holds the bootstrap lock, `find` neither waits for it nor tries the
    /// lifetime lock, which is free here: trying it would answer that no daemon runs.
    #[tokio::test]
    async fn another_command_holding_the_bootstrap_lock_is_busy() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let paths = paths(dir.path());
        File::create(paths.lifetime_lock()).expect("the lock file");
        let _bootstrap = try_lock(&paths.bootstrap_lock()).expect("the bootstrap lock").expect("free");
        assert!(matches!(find(&paths).await.expect("found"), Found::Busy));
    }
}
