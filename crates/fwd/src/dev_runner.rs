//! The daemon's entry in the dev-runner registry, so that a program watching this machine can
//! tell whether the daemon runs, on which port, and how many projects it has open. The dev-runner
//! protocol is files and nothing else. Each runner keeps an entry in the registry,
//! `~/.dev-runner/`: a meta file, and a status file for each of its units, a unit being a named
//! part of a runner whose state it publishes in a status file of its own. It removes them when it
//! exits. fwd names the protocol, never a program that reads the registry (docs/DECISIONS.md,
//! D154). The daemon is one process, so its entry has one unit, `daemon`, and the projects it
//! hosts are not units of their own (Daniel, 2026-09-26, recorded in D22). See
//! docs/design/daemon.md, "The dev-runner registry".

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::warn;

/// The dev-runner registry: `~/.dev-runner/`, unless `FWD_DEV_RUNNER_DIR` names another directory
/// (the tests, the smoke script, or any run on another data folder, such as a restored backup,
/// point it at one of their own, so that those daemons stay out of the machine's registry).
pub fn registry() -> Option<PathBuf> {
    match std::env::var_os("FWD_DEV_RUNNER_DIR") {
        Some(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
        _ => dirs::home_dir().map(|home| home.join(".dev-runner")),
    }
}

/// The daemon's entry in the registry: its meta file `<id>`, and the status file of its one unit,
/// `<id>.daemon.status`, a single line (`<state>` or `<state>: <detail>`) rewritten on each
/// change. Each is written to `<name>.tmp` and renamed into place: a reader may watch the
/// directory, and a file rewritten in place can be read empty.
pub struct Entry {
    /// `None` when there is nowhere to write: no home directory, or a registry that could not be
    /// created. The daemon then runs without an entry.
    registry: Option<PathBuf>,
    /// The first 16 hex digits of the SHA-256 of `dir`, so the same data directory has the same
    /// entry at every start, and a start finds the files a crash left.
    id: String,
    /// The data directory, absolute; a reader may name the entry after its last component (`fwd`
    /// for the default data directory).
    dir: PathBuf,
    /// Set by the first write that fails, after its one warning. The daemon then writes nothing
    /// more and leaves what it wrote before as it was (once announced, the meta file and the
    /// status file as last written). `clear` still tries to remove it at exit, and cannot when the
    /// directory no longer accepts changes, which leaves the entry until a later start for this
    /// data directory clears it.
    failed: AtomicBool,
}

impl Entry {
    /// The entry of the daemon for `data_dir`, kept in `registry`, which is created when missing.
    /// When there is none, or it cannot be created, the daemon says so once and runs without an
    /// entry.
    pub fn new(registry: Option<PathBuf>, data_dir: &Path) -> Entry {
        let dir = std::path::absolute(data_dir).unwrap_or_else(|_| data_dir.to_path_buf());
        let id = format!("{:x}", Sha256::digest(dir.as_os_str().as_encoded_bytes()))[..16].to_string();
        let registry = match registry {
            None => {
                warn!("no home directory for ~/.dev-runner/; running without a dev-runner entry");
                None
            }
            Some(registry) => match std::fs::create_dir_all(&registry) {
                Ok(()) => Some(registry),
                Err(e) => {
                    warn!("could not create {}: {e}; running without a dev-runner entry", registry.display());
                    None
                }
            },
        };
        Entry { registry, id, dir, failed: AtomicBool::new(false) }
    }

    /// Writes the status file `starting` while the daemon opens its projects, before its listener
    /// is bound and before its meta file exists: `announce` writes the status file `ready` before
    /// the meta file, so the meta file never appears beside a status file reading `starting`. An
    /// entry exists once its meta file does, so the daemon's entry first appears with its status
    /// file already reading `ready`.
    pub fn starting(&self) {
        self.write_status("starting");
    }

    /// Once the listener is bound on `port`, with `projects` open: writes the status file `ready`
    /// with that number, then the meta file. The meta file holds the data directory, the daemon's
    /// pid (a reader may check that it is alive, and may stop the daemon by sending it SIGTERM),
    /// when the listener was bound, and its port as `DAEMON_PORT`: a key ending in `_PORT`
    /// supplies the port for the unit named by its lowercased prefix, so a reader can offer to
    /// open the page from the `daemon` unit.
    pub fn announce(&self, port: u16, projects: usize) {
        self.ready(projects);
        let started = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let meta =
            format!("DIR={}\nPID={}\nSTARTED={started}\nDAEMON_PORT={port}\n", self.dir.display(), std::process::id());
        self.write(&self.id, &meta);
    }

    /// Writes the status file once the listener is bound: `ready` with the number of projects
    /// open, `ready: 2 projects` or `ready: 1 project`. The daemon writes it again when a project
    /// opens on request.
    pub fn ready(&self, projects: usize) {
        let noun = if projects == 1 { "project" } else { "projects" };
        self.write_status(&format!("ready: {projects} {noun}"));
    }

    /// Removes the meta file and every `<id>.*` beside it: the status file, and a temporary file a
    /// crash left mid-write. At start this removes what a crashed daemon for the same data
    /// directory left; at exit, the daemon's own files.
    pub fn clear(&self) {
        let Some(registry) = &self.registry else { return };
        // The protocol's guard: an empty or short id would match other runners' files.
        if self.id.len() < 8 {
            return;
        }
        let Ok(files) = std::fs::read_dir(registry) else { return };
        let prefix = format!("{}.", self.id);
        for file in files.flatten() {
            let name = file.file_name();
            let name = name.to_string_lossy();
            if name == self.id || name.starts_with(&prefix) {
                let _ = std::fs::remove_file(file.path());
            }
        }
    }

    fn write_status(&self, state: &str) {
        self.write(&format!("{}.daemon.status", self.id), &format!("{state}\n"));
    }

    fn write(&self, name: &str, text: &str) {
        let Some(registry) = &self.registry else { return };
        if self.failed.load(Ordering::Relaxed) {
            return;
        }
        let path = registry.join(name);
        let tmp = registry.join(format!("{name}.tmp"));
        if let Err(e) = std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, &path)) {
            if !self.failed.swap(true, Ordering::Relaxed) {
                warn!(
                    "could not write {}: {e}; no further dev-runner updates, leaving what was already written",
                    path.display()
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn the_entry_has_one_unit_and_publishes_its_project_count_and_port() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let registry = dir.path().join("dev-runner");
        let entry = Entry::new(Some(registry.clone()), &dir.path().join("data"));
        let files = || -> Vec<String> {
            let mut names: Vec<String> = std::fs::read_dir(&registry)
                .expect("the registry")
                .map(|e| e.expect("a directory entry").file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        };
        let status_file = format!("{}.daemon.status", entry.id);
        let status = || std::fs::read_to_string(registry.join(&status_file)).expect("the status file");

        entry.starting();
        assert_eq!(files(), [status_file.as_str()], "no meta file before the listener is bound");
        assert_eq!(status(), "starting\n");

        entry.announce(4242, 0);
        assert_eq!(files(), [entry.id.as_str(), status_file.as_str()], "the meta file and its one status file");
        assert_eq!(status(), "ready: 0 projects\n");
        // A key ending in `_PORT` supplies the port for the unit named by its lowercased prefix.
        let ports: BTreeMap<String, String> = std::fs::read_to_string(registry.join(&entry.id))
            .expect("the meta file")
            .lines()
            .filter_map(|line| line.split_once('='))
            .filter_map(|(key, port)| Some((key.strip_suffix("_PORT")?.to_lowercase(), port.to_string())))
            .collect();
        assert_eq!(ports, BTreeMap::from([("daemon".to_string(), "4242".to_string())]));

        entry.ready(1);
        assert_eq!(status(), "ready: 1 project\n");
        entry.ready(2);
        assert_eq!(status(), "ready: 2 projects\n");
    }

    #[test]
    fn clearing_leaves_other_runners_files() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let registry = dir.path().join("dev-runner");
        let entry = Entry::new(Some(registry.clone()), &dir.path().join("data"));
        entry.announce(4242, 1);
        // Another runner's entry, and the status file of a runner whose longer id starts with
        // this one.
        let other = "0123456789abcdef";
        let mut others = vec![other.to_string(), format!("{other}.server.status"), format!("{}0.web.status", entry.id)];
        for name in &others {
            std::fs::write(registry.join(name), "").expect("a file");
        }
        entry.clear();
        let mut left: Vec<String> = std::fs::read_dir(&registry)
            .expect("the registry")
            .map(|e| e.expect("a directory entry").file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        others.sort();
        assert_eq!(left, others);
    }
}
