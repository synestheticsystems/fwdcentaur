//! The daemon's card in the dev-runner registry, so that a program watching this machine can tell
//! whether the daemon runs, on which port, and how many projects it has open. The dev-runner
//! protocol is files and nothing else: each runner keeps a meta file and a status file per part in
//! the registry, `~/.dev-runner/`, and removes them when it exits, and a reader of the registry
//! builds a card from each meta file. The daemon is one process, so its card has one row, `daemon`,
//! and the projects it hosts have no rows of their own (Daniel, 2026-09-26, in docs/DECISIONS.md,
//! D22). See docs/design/daemon.md, "The dev-runner registry".

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::warn;

/// The dev-runner registry: `~/.dev-runner/`, unless `FWD_DEV_RUNNER_DIR` names another directory
/// (the tests and the smoke script point it at a scratch one, so their daemons stay out of the
/// machine's registry).
pub fn registry() -> Option<PathBuf> {
    match std::env::var_os("FWD_DEV_RUNNER_DIR") {
        Some(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
        _ => dirs::home_dir().map(|home| home.join(".dev-runner")),
    }
}

/// The daemon's card, two files in the registry named after `id`: the meta file `<id>`, which a
/// reader builds the card from, and the row `<id>.daemon.status`, a single line (`<state>` or
/// `<state>: <detail>`) rewritten on each change. Each is written to `<name>.tmp` and renamed
/// into place: a reader may watch the directory, and a file rewritten in place can be read empty.
pub struct Card {
    /// `None` when there is nowhere to write: no home directory, or a registry that could not be
    /// created. The daemon then runs without a card.
    registry: Option<PathBuf>,
    /// The first 16 hex digits of the SHA-256 of `dir`, so the same data directory has the same
    /// card at every start, and a start finds the files a crash left.
    id: String,
    /// The data directory, absolute; a reader names the card after its last component, `fwd`.
    dir: PathBuf,
    /// Set by the first write that fails, after its one warning; the daemon then runs without a
    /// card, though `clear` still removes what it wrote before.
    failed: AtomicBool,
}

impl Card {
    /// The card of the daemon for `data_dir`, kept in `registry`, which is created when missing.
    /// When there is none, or it cannot be created, the daemon says so once and runs without a
    /// card.
    pub fn new(registry: Option<PathBuf>, data_dir: &Path) -> Card {
        let dir = std::path::absolute(data_dir).unwrap_or_else(|_| data_dir.to_path_buf());
        let id = format!("{:x}", Sha256::digest(dir.as_os_str().as_encoded_bytes()))[..16].to_string();
        let registry = match registry {
            None => {
                warn!("no home directory for ~/.dev-runner/; running without a dev-runner card");
                None
            }
            Some(registry) => match std::fs::create_dir_all(&registry) {
                Ok(()) => Some(registry),
                Err(e) => {
                    warn!("could not create {}: {e}; running without a dev-runner card", registry.display());
                    None
                }
            },
        };
        Card { registry, id, dir, failed: AtomicBool::new(false) }
    }

    /// Writes the row while the daemon opens its projects, before its listener is bound. A reader
    /// never shows this state: it builds a card from the meta file only, and `announce` writes
    /// that once the row reads `ready`.
    pub fn starting(&self) {
        self.row("starting");
    }

    /// Once the listener is bound on `port`, with `projects` open: writes the row `ready` with
    /// that number, then the meta file, so the card appears with its row reading `ready`. The meta
    /// file holds the data directory, the daemon's pid (a reader checks that it lives, and may
    /// stop the daemon by sending it SIGTERM), when the listener was bound, and its port as
    /// `DAEMON_PORT`: the protocol gives the port of a key that ends in `_PORT` to the row named
    /// like the rest of the key, lowercased, so a reader can offer to open the page from that row.
    pub fn announce(&self, port: u16, projects: usize) {
        self.ready(projects);
        let started = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let meta =
            format!("DIR={}\nPID={}\nSTARTED={started}\nDAEMON_PORT={port}\n", self.dir.display(), std::process::id());
        self.write(&self.id, &meta);
    }

    /// Writes the row once the listener is bound: `ready` with the number of projects open,
    /// `ready: 2 projects` or `ready: 1 project`. The daemon writes it again when a project opens
    /// on request.
    pub fn ready(&self, projects: usize) {
        let noun = if projects == 1 { "project" } else { "projects" };
        self.row(&format!("ready: {projects} {noun}"));
    }

    /// Removes the meta file and every `<id>.*` beside it: the row, and a temporary file a crash
    /// left mid-write. At start this removes what a crashed daemon for the same data directory
    /// left; at exit, the daemon's own files.
    pub fn clear(&self) {
        let Some(registry) = &self.registry else { return };
        // The protocol's guard: an empty or short id would match other runners' files.
        if self.id.len() < 8 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(registry) else { return };
        let rows = format!("{}.", self.id);
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == self.id || name.starts_with(&rows) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    fn row(&self, state: &str) {
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
                warn!("could not write {}: {e}; running without a dev-runner card", path.display());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn the_card_has_one_row_which_counts_the_projects_and_carries_the_daemons_port() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let registry = dir.path().join("dev-runner");
        let card = Card::new(Some(registry.clone()), &dir.path().join("data"));
        let files = || -> Vec<String> {
            let mut names: Vec<String> = std::fs::read_dir(&registry)
                .expect("the registry")
                .map(|e| e.expect("an entry").file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        };
        let status = format!("{}.daemon.status", card.id);
        let row = || std::fs::read_to_string(registry.join(&status)).expect("the row");

        card.starting();
        assert_eq!(files(), [status.as_str()], "no card before the listener is bound");
        assert_eq!(row(), "starting\n");

        card.announce(4242, 0);
        assert_eq!(files(), [card.id.as_str(), status.as_str()], "the card and its one row");
        assert_eq!(row(), "ready: 0 projects\n");
        // The ports as the protocol reads them from the meta file: a key that ends in `_PORT` gives
        // its port to the row named like the rest of the key, lowercased.
        let ports: BTreeMap<String, String> = std::fs::read_to_string(registry.join(&card.id))
            .expect("the meta file")
            .lines()
            .filter_map(|line| line.split_once('='))
            .filter_map(|(key, port)| Some((key.strip_suffix("_PORT")?.to_lowercase(), port.to_string())))
            .collect();
        assert_eq!(ports, BTreeMap::from([("daemon".to_string(), "4242".to_string())]));

        card.ready(1);
        assert_eq!(row(), "ready: 1 project\n");
        card.ready(2);
        assert_eq!(row(), "ready: 2 projects\n");
    }

    #[test]
    fn clearing_leaves_other_runners_files() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let registry = dir.path().join("dev-runner");
        let card = Card::new(Some(registry.clone()), &dir.path().join("data"));
        card.announce(4242, 1);
        // Another runner's card, and one whose longer id starts with this one.
        let other = "0123456789abcdef";
        let mut others = vec![other.to_string(), format!("{other}.server.status"), format!("{}0.web.status", card.id)];
        for name in &others {
            std::fs::write(registry.join(name), "").expect("a file");
        }
        card.clear();
        let mut left: Vec<String> = std::fs::read_dir(&registry)
            .expect("the registry")
            .map(|e| e.expect("an entry").file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        others.sort();
        assert_eq!(left, others);
    }
}
