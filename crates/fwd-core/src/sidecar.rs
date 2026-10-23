//! The anchors' local store: a SQLite file beside the project's database in the data directory
//! (`projects/<id>/anchors.sqlite`), for what one machine learns about anchors and never syncs.
//! Today it holds the resolution cache: where an anchor was found in a given version of its file,
//! keyed by the anchor, the file's path there and its blob id, so switching back to a branch, or
//! popping a stash, answers from the cache. A resolution is knowledge about one checkout that
//! changes with every commit; kept on the anchor entity it was a write synced to every peer, and
//! two machines' answers fought over one field (docs/research/anchoring-robust-proposal.md,
//! section 3.3 and open question 2). Nothing here is needed for correctness: a missing or
//! unreadable file only costs recomputing.

use crate::anchoring::Resolution;
use anyhow::{anyhow, Context as _, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

/// The resolver's version: answers cached by another version are recomputed, since a change to
/// the cascade can change the answer for the same anchor and blob.
pub const RESOLVER_VERSION: i64 = 1;

pub struct Sidecar {
    conn: Mutex<Connection>,
}

impl Sidecar {
    /// The file beside a project's database, given the project's storage directory
    /// (`Paths::storage_dir`).
    pub fn path(storage_dir: &Path) -> PathBuf {
        storage_dir.join("anchors.sqlite")
    }

    /// Opens the store, creating the file and its table on first use.
    pub fn open(path: &Path) -> Result<Sidecar> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        // Several commands may resolve at once; each waits its turn rather than failing.
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS resolutions (
                anchor TEXT NOT NULL,
                path TEXT NOT NULL,
                target_blob TEXT NOT NULL,
                resolver INTEGER NOT NULL,
                record TEXT NOT NULL,
                at INTEGER NOT NULL,
                PRIMARY KEY (anchor, path, target_blob)
            )",
        )?;
        Ok(Sidecar { conn: Mutex::new(conn) })
    }

    /// An in-memory store, for tests.
    pub fn in_memory() -> Result<Sidecar> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(
            "CREATE TABLE resolutions (
                anchor TEXT NOT NULL, path TEXT NOT NULL, target_blob TEXT NOT NULL,
                resolver INTEGER NOT NULL, record TEXT NOT NULL, at INTEGER NOT NULL,
                PRIMARY KEY (anchor, path, target_blob)
            )",
        )?;
        Ok(Sidecar { conn: Mutex::new(conn) })
    }

    fn conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.conn.lock().map_err(|_| anyhow!("the anchors' store was poisoned by a panic"))
    }

    /// The cached answer for `anchor` (its key, or `id:<entity id>` before it has one) against
    /// the blob `target_blob` at `path`, when this resolver made it.
    pub fn resolution(&self, anchor: &str, path: &str, target_blob: &str) -> Result<Option<Resolution>> {
        let record: Option<String> = self
            .conn()?
            .query_row(
                "SELECT record FROM resolutions WHERE anchor = ?1 AND path = ?2 AND target_blob = ?3 AND resolver = ?4",
                params![anchor, path, target_blob, RESOLVER_VERSION],
                |row| row.get(0),
            )
            .optional()?;
        record.map(|r| serde_json::from_str(&r).context("reading a cached resolution")).transpose()
    }

    /// Keeps an answer, without its target, which depends on how it was asked.
    pub fn remember(&self, anchor: &str, path: &str, target_blob: &str, resolution: &Resolution) -> Result<()> {
        let record = Resolution { target: None, ..resolution.clone() };
        self.conn()?.execute(
            "INSERT OR REPLACE INTO resolutions (anchor, path, target_blob, resolver, record, at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![anchor, path, target_blob, RESOLVER_VERSION, serde_json::to_string(&record)?, resolution.at],
        )?;
        Ok(())
    }
}
