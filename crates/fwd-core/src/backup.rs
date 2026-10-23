//! Backups of this machine's data folder, taken while the daemon runs, and restores of them.
//!
//! The data folder holds the only copy of every project's graph on this machine (`Paths` in
//! `project.rs`). `take` copies it into a new dated folder: each SQLite database through SQLite's
//! online backup (`copy_database` says why never as a file), every other file as it is, leaving
//! out logs, locks and files still being written. It checks each copied database with SQLite's
//! integrity check, writes a manifest naming every file with its size and SHA-256 last, and then
//! removes the oldest backups of the same data folder beyond the number to keep. `restore` checks
//! a backup against its manifest and copies it into a data folder that is empty or does not exist
//! yet, so it never overwrites a graph, and never into the data folder while this environment's
//! daemon serves it. See docs/design/daemon.md, "Backups".

use crate::daemon::{read_port_file, try_lock};
use crate::project::{fsync_dir, Index, Paths, Project};
use anyhow::{anyhow, bail, Context as _, Result};
use rusqlite::backup::{Backup, StepResult};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

/// The manifest in each backup folder, written last: a folder without one is not a finished
/// backup, so neither pruning nor a restore takes it for one.
pub const MANIFEST: &str = "backup.json";

/// The manifest's layout; a restore refuses a backup whose format is later than this build's.
const FORMAT: u32 = 1;

/// What a backup holds, as its manifest records it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    /// When the backup was taken, in milliseconds since the Unix epoch.
    pub taken: i64,
    /// The machine that took it, by host name, and the data folder it copied: pruning removes
    /// only the backups of this data folder taken on this machine, so machines that back up into
    /// one synced folder never remove each other's. A machine whose host name changes leaves the
    /// backups it took under the old name for the person to remove.
    pub host: String,
    pub data_dir: PathBuf,
    /// The projects the data folder's index listed.
    pub projects: Vec<Project>,
    pub files: Vec<Entry>,
}

/// One file of a backup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    /// Where the file lies, in the backup and in the data folder alike: relative, with `/`
    /// between its parts.
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
    /// A SQLite database: copied through SQLite, and checked with its integrity check.
    pub database: bool,
}

/// What `take` did.
#[derive(Debug, Serialize)]
pub struct Taken {
    /// The dated folder it wrote.
    pub backup: PathBuf,
    #[serde(flatten)]
    pub manifest: Manifest,
    /// How many backups of this data folder the backups folder keeps.
    pub keep: usize,
    /// The backups of this data folder taken on this machine that remain, newest first: the new
    /// one, then the older ones kept.
    pub kept: Vec<PathBuf>,
    /// The older ones removed so that no more than `keep` remain.
    pub removed: Vec<PathBuf>,
}

/// What `restore` did.
#[derive(Debug, Serialize)]
pub struct Restored {
    /// The data folder it wrote.
    pub into: PathBuf,
    /// The backup it copied.
    pub backup: PathBuf,
    #[serde(flatten)]
    pub manifest: Manifest,
}

/// Where backups go unless the person names another folder: beside the data folder, named after
/// it (`~/Library/Application Support/fwd-backups` beside `.../fwd`). Outside the data folder, so
/// the backups outlive the folder's deletion and the next backup does not copy them.
pub fn default_dir(paths: &Paths) -> PathBuf {
    let name = paths.data_dir.file_name().unwrap_or_default().to_string_lossy();
    paths.data_dir.with_file_name(format!("{name}-backups"))
}

/// Backs up the data folder into a new dated folder in `to`, then removes the oldest backups of
/// this data folder taken on this machine there, so that `keep` remain, the new one among them.
/// The backup is built in a partial folder and named only once every database in it has passed
/// the integrity check and its manifest is written; on any failure the partial folder goes and no
/// older backup is removed, so a run that fails never costs a good backup.
pub fn take(paths: &Paths, to: &Path, keep: usize) -> Result<Taken> {
    let data_dir = resolved(&paths.data_dir)?;
    let to = resolved(to)?;
    // A data folder without an index holds no project. Backing it up anyway would let a
    // scheduled run with the wrong FWD_DATA_DIR replace the good backups with empty ones.
    if !data_dir.join("projects.toml").is_file() {
        bail!(
            "there is nothing to back up in {}: it has no projects.toml (does FWD_DATA_DIR name the data folder you meant?)",
            data_dir.display()
        );
    }
    if to.starts_with(&data_dir) {
        bail!(
            "{} is inside the data folder {}: a backup there is lost with the folder, and the next backup would copy it; name a folder outside it with --to",
            to.display(),
            data_dir.display()
        );
    }
    create_private_dir(&to)?;
    let taken = crate::now_ms();
    let name = LocalTime::at(taken).folder_name();
    let partial = to.join(format!("{name}.{}.partial", std::process::id()));
    create_new_private_dir(&partial)?;
    let finished = copy_out(&data_dir, &partial, taken).and_then(|manifest| {
        let backup = settle(&to, &partial, &name)?;
        Ok((manifest, backup))
    });
    let (manifest, backup) = match finished {
        Ok(done) => done,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&partial);
            return Err(e.context(format!("nothing was backed up into {}, and no backup was removed", to.display())));
        }
    };
    let (kept, removed) = prune(&to, &backup, &manifest, keep)
        .with_context(|| format!("the backup {} is written, but removing the older ones failed", backup.display()))?;
    Ok(Taken { backup, manifest, keep, kept, removed })
}

/// Copies every file of the data folder into `partial`, checks each database copied, and writes
/// the manifest last.
fn copy_out(data_dir: &Path, partial: &Path, taken: i64) -> Result<Manifest> {
    let mut files = Vec::new();
    for relative in files_under(data_dir)? {
        let (from, to) = (data_dir.join(&relative), partial.join(&relative));
        if let Some(folder) = to.parent() {
            std::fs::create_dir_all(folder).with_context(|| format!("creating {}", folder.display()))?;
        }
        let database = is_database(&from)?;
        if database {
            copy_database(&from, &to).with_context(|| format!("copying the database {}", from.display()))?;
            check_integrity(&to).with_context(|| format!("checking the copy of {}", from.display()))?;
        } else {
            std::fs::copy(&from, &to).with_context(|| format!("copying {}", from.display()))?;
        }
        File::open(&to)?.sync_all()?;
        let bytes = std::fs::metadata(&to)?.len();
        files.push(Entry { path: slashed(&relative), bytes, sha256: sha256(&to)?, database });
    }
    let index_path = partial.join("projects.toml");
    let index: Index = toml::from_str(&std::fs::read_to_string(&index_path)?)
        .with_context(|| format!("projects.toml in {} does not parse", data_dir.display()))?;
    let manifest = Manifest {
        format: FORMAT,
        taken,
        host: host_name(),
        data_dir: data_dir.to_path_buf(),
        projects: index.projects.iter().map(Project::from).collect(),
        files,
    };
    let manifest_path = partial.join(MANIFEST);
    std::fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest)?)
        .with_context(|| format!("writing {}", manifest_path.display()))?;
    File::open(&manifest_path)?.sync_all()?;
    sync_folders(partial)?;
    Ok(manifest)
}

/// Gives the finished backup its dated name, or, when another backup took that name in the same
/// second, the name followed by `-2`, `-3` and so on.
fn settle(to: &Path, partial: &Path, name: &str) -> Result<PathBuf> {
    let mut n = 1;
    loop {
        let candidate = to.join(if n == 1 { name.to_string() } else { format!("{name}-{n}") });
        n += 1;
        if candidate.exists() {
            continue;
        }
        match std::fs::rename(partial, &candidate) {
            Ok(()) => {
                fsync_dir(to)?;
                return Ok(candidate);
            }
            // Another backup took the name between the look and the rename.
            Err(_) if candidate.exists() => continue,
            Err(e) => return Err(e).with_context(|| format!("naming the backup {}", candidate.display())),
        }
    }
}

/// Removes the oldest backups of `manifest`'s data folder taken on its machine from `to`, so that
/// `keep` remain with `new` among them, and returns the ones kept, newest first, and the ones
/// removed. A folder counts only when its manifest names the same machine and data folder: nothing
/// else in `to`, whether another machine's backups, another data folder's, a partial backup or a
/// folder of the person's, is counted or removed.
fn prune(to: &Path, new: &Path, manifest: &Manifest, keep: usize) -> Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    let mut older = Vec::new();
    for entry in std::fs::read_dir(to).with_context(|| format!("listing {}", to.display()))? {
        let entry = entry?;
        let path = entry.path();
        let partial = path.extension().is_some_and(|e| e == "partial");
        if path == new || partial || !entry.file_type()?.is_dir() {
            continue;
        }
        let Ok(other) = read_manifest(&path) else { continue };
        if other.host == manifest.host && other.data_dir == manifest.data_dir {
            older.push((other.taken, path));
        }
    }
    older.sort_by(|a, b| b.cmp(a));
    let removed: Vec<PathBuf> =
        older.split_off(keep.saturating_sub(1).min(older.len())).into_iter().map(|(_, path)| path).collect();
    for path in &removed {
        match std::fs::remove_dir_all(path) {
            Ok(()) => {}
            // A backup run at the same moment removed it first.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("removing the old backup {}", path.display())),
        }
    }
    let kept = std::iter::once(new.to_path_buf()).chain(older.into_iter().map(|(_, path)| path)).collect();
    Ok((kept, removed))
}

/// Copies the backup in `from` into the data folder `into`, which must be empty or not exist, so
/// a restore never overwrites a graph. It checks every file against the manifest before it writes
/// anything, copies the files into a folder beside `into`, checks each database there with SQLite's
/// integrity check, and only then renames that folder to `into`, so `into` holds the whole backup
/// or nothing. When `into` is the data folder of this environment, the restore holds that folder's
/// daemon lifetime lock while it writes: a daemon holds the lock for its whole life, so taking it
/// proves that no daemon serves the folder and keeps one from starting there meanwhile.
pub fn restore(paths: &Paths, from: &Path, into: &Path) -> Result<Restored> {
    let from = resolved(from)?;
    let into = resolved(into)?;
    let manifest = read_backup(&from)?;
    let files = manifest.files.iter().map(|entry| Ok((inside(&entry.path)?, entry))).collect::<Result<Vec<_>>>()?;
    if into.starts_with(&from) {
        bail!("{} is inside the backup {}; restore into a folder outside it", into.display(), from.display());
    }
    let _lifetime = if into == resolved(&paths.data_dir)? { Some(hold_lifetime_lock(paths, &into)?) } else { None };
    if into.exists() {
        if !into.is_dir() {
            bail!("{} is a file, not a data folder; name a folder that is empty or does not exist yet", into.display());
        }
        if std::fs::read_dir(&into)?.next().is_some() {
            bail!(
                "{} is not empty: a restore writes only into an empty or new folder, so it never overwrites a graph. To replace the one there, stop its daemon (`fwd daemon stop`), move the folder aside (mv '{}' '{}.before-restore') and restore again; or restore into a new folder with --into",
                into.display(),
                into.display(),
                into.display()
            );
        }
    }
    verify(&from, &files)?;
    let parent = into.parent().ok_or_else(|| anyhow!("{} has no folder to restore it in", into.display()))?;
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let staging = parent.join(format!(
        ".{}.{}.restoring",
        into.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    create_new_private_dir(&staging)?;
    let written = (|| -> Result<()> {
        for (relative, entry) in &files {
            let to = staging.join(relative);
            if let Some(folder) = to.parent() {
                std::fs::create_dir_all(folder)?;
            }
            std::fs::copy(from.join(relative), &to).with_context(|| format!("copying {}", entry.path))?;
            File::open(&to)?.sync_all()?;
            if entry.database {
                check_integrity(&to).with_context(|| format!("checking the restored {}", entry.path))?;
            }
        }
        sync_folders(&staging)?;
        if into.exists() {
            std::fs::remove_dir(&into)
                .with_context(|| format!("{} is no longer empty: something wrote into it meanwhile", into.display()))?;
        }
        std::fs::rename(&staging, &into)
            .with_context(|| format!("renaming {} to {}", staging.display(), into.display()))?;
        fsync_dir(parent)
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e.context(format!("nothing was restored into {}", into.display())));
    }
    Ok(Restored { into, backup: from, manifest })
}

/// This environment's daemon lifetime lock, held for as long as a restore writes into the data
/// folder that daemon serves. When a daemon holds it, the restore stops and says how to stop it.
fn hold_lifetime_lock(paths: &Paths, into: &Path) -> Result<File> {
    create_private_dir(&paths.runtime_dir)?;
    if let Some(lock) = try_lock(&paths.lifetime_lock())? {
        return Ok(lock);
    }
    let pid = match read_port_file(&paths.port_file()) {
        Ok(Some(port_file)) => format!(" (pid {})", port_file.pid),
        _ => String::new(),
    };
    bail!(
        "a daemon{pid} is serving the data folder {}: stop it with `fwd daemon stop` and restore again; stop the agent sessions that use fwd first, since their next command starts it again",
        into.display()
    )
}

/// The manifest of the backup in `folder`, or a sentence a person can act on when the folder is
/// not a backup; a folder of backups names the newest one in it.
fn read_backup(folder: &Path) -> Result<Manifest> {
    if !folder.is_dir() {
        bail!("{} is not a folder; name one of the dated folders `fwd admin backup` wrote", folder.display());
    }
    if !folder.join(MANIFEST).is_file() {
        let newest = std::fs::read_dir(folder)?
            .flatten()
            .filter_map(|entry| Some((read_manifest(&entry.path()).ok()?.taken, entry.path())))
            .max();
        bail!(match newest {
            Some((_, newest)) => format!(
                "{} holds backups rather than being one; name one of them, such as the newest, {}",
                folder.display(),
                newest.display()
            ),
            None => format!(
                "{} is not a backup: it has no {MANIFEST}, which `fwd admin backup` writes into each backup it finishes",
                folder.display()
            ),
        });
    }
    let manifest = read_manifest(folder)?;
    if manifest.format > FORMAT {
        bail!(
            "the backup {} was made by a later fwd (format {}; this build reads format {FORMAT}); restore it with that build",
            folder.display(),
            manifest.format
        );
    }
    Ok(manifest)
}

fn read_manifest(folder: &Path) -> Result<Manifest> {
    let path = folder.join(MANIFEST);
    let text = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_slice(&text).with_context(|| format!("parsing {}", path.display()))
}

/// Checks each file of the backup against its entry in the manifest before a restore writes
/// anything: present, of its recorded size, with its recorded SHA-256. A backup can sit for weeks
/// in a synced folder; the files that changed since are named, and nothing is restored.
fn verify(from: &Path, files: &[(PathBuf, &Entry)]) -> Result<()> {
    let mut problems = Vec::new();
    for (relative, entry) in files {
        let path = from.join(relative);
        let Ok(metadata) = std::fs::metadata(&path) else {
            problems.push(format!("{} is missing", entry.path));
            continue;
        };
        if metadata.len() != entry.bytes {
            problems.push(format!(
                "{} has {} bytes where the manifest says {}",
                entry.path,
                metadata.len(),
                entry.bytes
            ));
        } else if sha256(&path)? != entry.sha256 {
            problems.push(format!("{} changed since the backup (its SHA-256 differs)", entry.path));
        }
    }
    if !problems.is_empty() {
        bail!("the backup {} is damaged, so nothing was restored: {}", from.display(), problems.join("; "));
    }
    Ok(())
}

/// Copies the SQLite database at `from` into a new file at `to` through SQLite's online backup,
/// never as a file. The daemon keeps the database open and may write to it at any moment, in WAL
/// mode: a committed write stays in `-wal` beside the main file until a checkpoint moves it in,
/// so a file copy can take the two files at different moments, or a page halfway through being
/// written, and yield a copy that lacks writes or does not open. The backup reads inside one read
/// transaction, which sees one committed state of both files and does not hold up the daemon's
/// writes, and it copies every page in one step, since a write by another process between steps
/// would start it over. It copies page by page, so the copy holds every row as the daemon had it,
/// down to the row ids; `VACUUM INTO` would rebuild the database instead, and SQLite documents
/// that a vacuum may renumber the rows of a table without an integer primary key, as Ankurah's
/// tables are (their keys are text). Last, the copy switches to a rollback journal, so it is one
/// file with nothing beside it that must travel with it into a synced folder; the daemon switches
/// it back to WAL when it opens it.
fn copy_database(from: &Path, to: &Path) -> Result<()> {
    // Opened as the daemon opens it (to read and write, never created), so that closing it leaves
    // the files as SQLite leaves them; the backup only reads through it.
    let source =
        Connection::open_with_flags(from, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    let mut copy = Connection::open(to)?;
    {
        let backup = Backup::new(&source, &mut copy)?;
        let mut waits = 0;
        loop {
            match backup.step(-1)? {
                StepResult::Done => break,
                // The source was locked for a moment (another connection recovering its log, say).
                _ if waits < 50 => {
                    waits += 1;
                    std::thread::sleep(Duration::from_millis(100));
                }
                _ => bail!("{} stayed locked for five seconds", from.display()),
            }
        }
    }
    let mode: String = copy.pragma_update_and_check(None, "journal_mode", "DELETE", |row| row.get(0))?;
    if mode != "delete" {
        bail!("the copy of {} stayed in journal mode {mode}", from.display());
    }
    Ok(())
}

/// Opens the database at `path` to read only and runs SQLite's integrity check, which reads every
/// page and index; anything but its one answer `ok` is an error that says what it found.
fn check_integrity(path: &Path) -> Result<()> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .with_context(|| format!("opening {}", path.display()))?;
    let found = conn
        .prepare("PRAGMA integrity_check")?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    if found != ["ok"] {
        bail!("{} does not pass SQLite's integrity check: {}", path.display(), found.join("; "));
    }
    Ok(())
}

/// True when the file begins with the header every SQLite database begins with.
fn is_database(path: &Path) -> Result<bool> {
    let mut head = [0u8; 16];
    let mut file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    match file.read_exact(&mut head) {
        Ok(()) => Ok(&head == b"SQLite format 3\0"),
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Every file under `root` a backup copies, relative to `root`, in order. Folders are walked;
/// symbolic links and what `left_out` names are skipped.
fn files_under(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut folders = vec![root.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).with_context(|| format!("listing {}", folder.display()))? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                folders.push(entry.path());
            } else if kind.is_file() && !left_out(&entry.file_name().to_string_lossy()) {
                files.push(entry.path().strip_prefix(root)?.to_path_buf());
            }
        }
    }
    files.sort();
    Ok(files)
}

/// The files a backup leaves out: logs (`daemon.log`, and the per-project logs of earlier
/// builds), lock files, files a writer is still writing (`.tmp`), and a database's `-wal`,
/// `-shm` and `-journal`, whose committed content the database's own copy already holds.
fn left_out(name: &str) -> bool {
    name.ends_with(".log")
        || name.contains(".log.")
        || [".lock", ".tmp", "-wal", "-shm", "-journal"].iter().any(|suffix| name.ends_with(suffix))
}

/// `relative` with `/` between its parts, as the manifest records paths.
fn slashed(relative: &Path) -> String {
    relative.components().map(|part| part.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

/// A path from the manifest as a relative path that stays inside the folder it is joined to: a
/// damaged or crafted manifest must not make a restore write anywhere else.
fn inside(path: &str) -> Result<PathBuf> {
    let relative = Path::new(path);
    if path.is_empty() || !relative.components().all(|part| matches!(part, Component::Normal(_))) {
        bail!("the backup's manifest names {path:?}, which is not a path inside the backup");
    }
    Ok(relative.to_path_buf())
}

/// `path` made absolute, with the part of it that exists resolved to its real path (symbolic links
/// followed), so that two spellings of one folder compare equal whether or not it exists yet.
fn resolved(path: &Path) -> Result<PathBuf> {
    let absolute = std::path::absolute(path).with_context(|| format!("making {} absolute", path.display()))?;
    let mut existing = absolute.as_path();
    let mut missing = Vec::new();
    loop {
        if let Ok(real) = existing.canonicalize() {
            return Ok(missing.iter().rev().fold(real, |path, part| path.join(part)));
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                missing.push(name.to_os_string());
                existing = parent;
            }
            _ => return Ok(absolute),
        }
    }
}

/// Creates `folder` and the folders above it when missing; one this creates is owner-only, as the
/// data folder is.
fn create_private_dir(folder: &Path) -> Result<()> {
    if folder.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(folder).with_context(|| format!("creating {}", folder.display()))?;
    restrict(folder)
}

/// Creates `folder`, which must not exist yet, owner-only.
fn create_new_private_dir(folder: &Path) -> Result<()> {
    std::fs::create_dir(folder).with_context(|| format!("creating {}", folder.display()))?;
    restrict(folder)
}

fn restrict(folder: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("restricting {} to its owner", folder.display()))?;
    }
    Ok(())
}

/// Makes the entries of `root` and of every folder under it durable.
fn sync_folders(root: &Path) -> Result<()> {
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            sync_folders(&entry.path())?;
        }
    }
    fsync_dir(root)
}

fn sha256(path: &Path) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).with_context(|| format!("reading {}", path.display()))?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// This machine's host name, which the manifest records.
fn host_name() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: the buffer is valid for writes of its whole length, and gethostname writes at most
    // that many bytes.
    if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } != 0 {
        return String::new();
    }
    let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

impl Manifest {
    /// What the backup holds, for people: a line per project naming its files with their sizes,
    /// then a line for the files beside the projects.
    pub fn contents(&self) -> Vec<String> {
        let mut rest: Vec<&Entry> = self.files.iter().collect();
        let mut lines = Vec::new();
        for project in &self.projects {
            let folder = format!("projects/{}/", project.id);
            let (its, others): (Vec<&Entry>, Vec<&Entry>) =
                rest.into_iter().partition(|entry| entry.path.starts_with(&folder));
            rest = others;
            lines.push(format!("{} ({}): {}", project.name, project.id, listed(&its, &folder)));
        }
        if !rest.is_empty() {
            lines.push(format!("beside them: {}", listed(&rest, "")));
        }
        lines
    }

    /// When the backup was taken, in this machine's time zone: `2026-09-28 20:40:12 -0700`.
    pub fn taken_at(&self) -> String {
        LocalTime::at(self.taken).readable()
    }
}

fn listed(entries: &[&Entry], folder: &str) -> String {
    if entries.is_empty() {
        return "no files yet".into();
    }
    let named = entries
        .iter()
        .map(|entry| format!("{} {}", entry.path.strip_prefix(folder).unwrap_or(&entry.path), size(entry.bytes)));
    named.collect::<Vec<_>>().join(", ")
}

fn size(bytes: u64) -> String {
    match bytes {
        0..1024 => format!("{bytes} bytes"),
        1024..1_048_576 => format!("{:.1} KB", bytes as f64 / 1024.0),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.0),
    }
}

/// A moment in this machine's time zone, for the names of backup folders and for people.
struct LocalTime {
    tm: libc::tm,
}

impl LocalTime {
    fn at(ms: i64) -> LocalTime {
        let seconds = ms.div_euclid(1000) as libc::time_t;
        // SAFETY: `tm` holds integers and a pointer to a zone name, for which zero is a valid
        // value; localtime_r and gmtime_r read `seconds` and write `tm`, both valid for the call.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        if unsafe { libc::localtime_r(&seconds, &mut tm) }.is_null() {
            unsafe { libc::gmtime_r(&seconds, &mut tm) };
        }
        LocalTime { tm }
    }

    /// `2026-09-28T204012-0700`: in time order within one zone, and naming the zone, so the
    /// backups of machines in two zones never read as one sequence.
    fn folder_name(&self) -> String {
        let t = &self.tm;
        format!(
            "{:04}-{:02}-{:02}T{:02}{:02}{:02}{}",
            t.tm_year + 1900,
            t.tm_mon + 1,
            t.tm_mday,
            t.tm_hour,
            t.tm_min,
            t.tm_sec,
            self.offset()
        )
    }

    /// `2026-09-28 20:40:12 -0700`.
    fn readable(&self) -> String {
        let t = &self.tm;
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02} {}",
            t.tm_year + 1900,
            t.tm_mon + 1,
            t.tm_mday,
            t.tm_hour,
            t.tm_min,
            t.tm_sec,
            self.offset()
        )
    }

    /// The zone's distance from UTC, `-0700`.
    fn offset(&self) -> String {
        let minutes = self.tm.tm_gmtoff / 60;
        let sign = if minutes < 0 { '-' } else { '+' };
        format!("{sign}{:02}{:02}", minutes.abs() / 60, minutes.abs() % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A data folder with one project whose database has committed rows still in its write-ahead
    /// log, held open the way the daemon holds it, beside the files a backup leaves out.
    fn data_folder(root: &Path) -> Result<(Paths, Connection)> {
        let paths = Paths { runtime_dir: root.join("run"), data_dir: root.join("data") };
        let project = Project { id: uuid::Uuid::new_v4(), name: "widgets".into() };
        Index::update(&paths, |index| index.register(&project, Some("github.com/acme/widgets")))?;
        std::fs::create_dir_all(paths.storage_dir(&project))?;
        let daemon = Connection::open(paths.db_path(&project))?;
        daemon.pragma_update(None, "journal_mode", "WAL")?;
        // Nothing moves the log into the main file while this connection stays open.
        daemon.pragma_update(None, "wal_autocheckpoint", 0)?;
        daemon.execute_batch("CREATE TABLE task (id TEXT PRIMARY KEY, title TEXT)")?;
        for n in 0..100 {
            daemon.execute("INSERT INTO task VALUES (?1, ?2)", (format!("t{n}"), format!("task {n}")))?;
        }
        std::fs::write(paths.log_path(), "a log\n")?;
        std::fs::write(paths.checkouts_path(&project), "[]")?;
        std::fs::write(paths.storage_dir(&project).join("checkouts.json.tmp"), "half")?;
        Ok((paths, daemon))
    }

    #[test]
    fn a_backup_holds_the_writes_still_in_the_log_and_leaves_out_logs_and_temporary_files() -> Result<()> {
        let root = tempfile::tempdir()?;
        let (paths, _daemon) = data_folder(root.path())?;
        let storage = std::fs::read_dir(paths.data_dir.join("projects"))?.next().expect("a project")?.path();
        assert!(storage.join("project.sqlite-wal").metadata()?.len() > 0, "the rows are in the log");

        let taken = take(&paths, &root.path().join("backups"), 7)?;
        let names: Vec<&str> = taken.manifest.files.iter().map(|entry| entry.path.as_str()).collect();
        let id = &taken.manifest.projects[0].id;
        assert_eq!(
            names,
            [format!("projects/{id}/checkouts.json"), format!("projects/{id}/project.sqlite"), "projects.toml".into()]
        );
        let copy = Connection::open(taken.backup.join(format!("projects/{id}/project.sqlite")))?;
        let rows: i64 = copy.query_row("SELECT count(*) FROM task", [], |row| row.get(0))?;
        assert_eq!(rows, 100, "the copy holds what the log held");
        let files: Vec<String> = std::fs::read_dir(taken.backup.join(format!("projects/{id}")))?
            .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
            .collect::<Result<_>>()?;
        assert!(!files.iter().any(|f| f.ends_with("-wal") || f.ends_with("-shm")), "the copy is one file: {files:?}");
        Ok(())
    }

    #[test]
    fn a_damaged_backup_is_refused_before_anything_is_written() -> Result<()> {
        let root = tempfile::tempdir()?;
        let (paths, _daemon) = data_folder(root.path())?;
        let taken = take(&paths, &root.path().join("backups"), 7)?;
        std::fs::write(taken.backup.join("projects.toml"), "# edited\n")?;
        let into = root.path().join("restored");
        let refused = restore(&paths, &taken.backup, &into).expect_err("a damaged backup is refused");
        assert!(format!("{refused:#}").contains("projects.toml has"), "{refused:#}");
        assert!(!into.exists(), "nothing was written");
        Ok(())
    }

    #[test]
    fn a_manifest_path_that_leaves_the_backup_is_refused() {
        for path in ["../elsewhere", "/etc/passwd", "projects/../../x", ""] {
            assert!(inside(path).is_err(), "{path:?} is refused");
        }
        assert_eq!(inside("projects/a/project.sqlite").ok(), Some(PathBuf::from("projects/a/project.sqlite")));
    }

    #[test]
    fn logs_locks_temporary_files_and_a_databases_companions_are_left_out() {
        for name in ["daemon.log", "daemon.log.legacy", "fwdd.log", "projects.toml.lock", "checkouts.json.tmp"] {
            assert!(left_out(name), "{name}");
        }
        for name in ["project.sqlite-wal", "project.sqlite-shm", "anchors.sqlite-journal"] {
            assert!(left_out(name), "{name}");
        }
        for name in ["project.sqlite", "anchors.sqlite", "checkouts.json", "checkouts.json.corrupt", "identity.toml"] {
            assert!(!left_out(name), "{name}");
        }
    }
}
