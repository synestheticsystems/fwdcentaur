//! Projects, the checkouts that belong to them, and the directories the daemon and CLI share.
//!
//! A project is the task graph and its database: a UUID and a name, kept under the data
//! directory, independent of any folder on disk. A project is a set of repositories, each
//! identified by its upstream remote, never by a clone or a worktree (Daniel, 2026-09-25). A
//! checkout is any clone or worktree of a member repository on this machine: local, dynamic, and
//! never in the graph. Resolution goes from a directory to its git upstream, and from the
//! upstream to the project through the local index (`projects.toml` in the data directory). A
//! committed `.fwd/project.toml` marker is optional: it tells a fresh machine which project the
//! repository belongs to, and teaches that machine's index on first sight. See
//! docs/design/daemon.md, "Projects, repositories, checkouts".

use crate::anchoring::git;
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub const MARKER: &str = ".fwd/project.toml";
/// The marker's name before 2026-09-25: still read, never written.
const LEGACY_MARKER: &str = ".fwd/workspace.toml";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Marker {
    id: Uuid,
    name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: Uuid,
    pub name: String,
}

/// A clone or worktree on this machine, as git describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkout {
    /// The git toplevel.
    pub root: PathBuf,
    /// `ops::repo_key` of the origin remote; `None` when the repository has no remote.
    pub upstream: Option<String>,
    /// The branch checked out, or `None` on a detached HEAD.
    pub branch: Option<String>,
    pub head: Option<String>,
}

/// What `resolve` fails with when nothing names a project for the directory: no marker, and no
/// indexed project claims the checkout's upstream (or there is no checkout). Commands that also
/// work without a project (`fwd ui`, `fwd daemon status`) tell it apart from a resolution that
/// went wrong (a broken worktree, a marker or index that does not parse), which they report.
#[derive(Debug, thiserror::Error)]
#[error("no project for {}: run `fwd init` in the repository, or pass --project <name|id>", .0.display())]
pub struct NoProject(pub PathBuf);

/// A project resolved from a directory, with the checkout it was reached through.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub project: Project,
    /// `None` outside any git repository (a marker alone can name a project).
    pub checkout: Option<Checkout>,
    /// The checkout's root, else the directory holding the marker, else the start directory.
    pub root: PathBuf,
}

/// The two directories everything else hangs off. Explicit rather than global so tests can
/// isolate themselves; the CLI builds one from `FWD_RUNTIME_DIR` / `FWD_DATA_DIR` or the defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// The daemon's lock files and its port file. Owner-only. Default: `$TMPDIR/fwd-<uid>/`.
    pub runtime_dir: PathBuf,
    /// The project index, identity, the daemon's settings and log, and per-project storage
    /// (`projects/<id>/project.sqlite`, the checkout registry). Default: the OS data directory
    /// plus `fwd`.
    pub data_dir: PathBuf,
}

impl Paths {
    pub fn from_env() -> Result<Self> {
        let runtime_dir = match std::env::var_os("FWD_RUNTIME_DIR") {
            Some(v) => PathBuf::from(v),
            None => {
                // SAFETY: getuid has no preconditions and cannot fail.
                let uid = unsafe { libc::getuid() };
                std::env::temp_dir().join(format!("fwd-{uid}"))
            }
        };
        let data_dir = match std::env::var_os("FWD_DATA_DIR") {
            Some(v) => PathBuf::from(v),
            None => dirs::data_dir().ok_or_else(|| anyhow!("no data directory for this platform"))?.join("fwd"),
        };
        Ok(Self { runtime_dir, data_dir })
    }

    /// Creates both directories, owner-only.
    pub fn ensure(&self) -> Result<()> {
        for dir in [&self.runtime_dir, &self.data_dir] {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
                    .with_context(|| format!("restricting {} to its owner", dir.display()))?;
            }
        }
        Ok(())
    }

    pub fn index_path(&self) -> PathBuf {
        self.data_dir.join("projects.toml")
    }
    pub fn storage_dir(&self, project: &Project) -> PathBuf {
        self.data_dir.join("projects").join(project.id.to_string())
    }
    pub fn db_path(&self, project: &Project) -> PathBuf {
        self.storage_dir(project).join("project.sqlite")
    }
    pub fn checkouts_path(&self, project: &Project) -> PathBuf {
        self.storage_dir(project).join("checkouts.json")
    }
    /// The one daemon's log, for every project it hosts.
    pub fn log_path(&self) -> PathBuf {
        self.data_dir.join("daemon.log")
    }
    /// The daemon's settings: `bind` (loopback or the LAN) and the port it last listened on.
    pub fn daemon_config_path(&self) -> PathBuf {
        self.data_dir.join("daemon.toml")
    }
    /// The port file: the daemon writes it once it is listening and removes it when it exits.
    pub fn port_file(&self) -> PathBuf {
        self.runtime_dir.join("daemon.ws")
    }
    /// Held by the daemon for its whole life.
    pub fn lifetime_lock(&self) -> PathBuf {
        self.runtime_dir.join("daemon.lock")
    }
    /// Taken by a launcher about to spawn the daemon, so only one spawns.
    pub fn bootstrap_lock(&self) -> PathBuf {
        self.runtime_dir.join("daemon.bootstrap.lock")
    }

    /// Moves a project's storage from the pre-2026-09-25 layout (`workspaces/<id>/workspace.sqlite`)
    /// to the current one. Call it only while holding the daemon's lifetime lock: nothing may
    /// have the database open while its files move. The id did not change, so the graph is the
    /// same graph.
    ///
    /// SQLite runs in WAL mode, so a database is three files (`.sqlite`, `-wal`, `-shm`) that
    /// must move together, main file last: while `workspace.sqlite` still exists the completion
    /// step below reruns on the next start, so an interrupted move finishes instead of leaving
    /// the daemon to open an empty database. A directory that already exists on the new side
    /// (earlier launchers created it for the project's log) is merged into; a database on both
    /// sides is a conflict this refuses to guess about.
    pub fn migrate_legacy(&self, project: &Project) -> Result<()> {
        let old = self.data_dir.join("workspaces").join(project.id.to_string());
        let new = self.storage_dir(project);
        if old.is_dir() {
            let both_have_databases = old.join("workspace.sqlite").exists() && self.db_path(project).exists();
            if both_have_databases {
                bail!(
                    "{} and {} both hold a database; keep one by hand, then start the daemon again",
                    old.join("workspace.sqlite").display(),
                    self.db_path(project).display()
                );
            }
            std::fs::create_dir_all(&new)?;
            for entry in std::fs::read_dir(&old)? {
                let entry = entry?;
                let name = entry.file_name();
                let to = new.join(&name);
                if to.exists() {
                    // Not a database file (that case was refused above): keep the legacy copy
                    // beside the new one rather than overwrite either.
                    let kept = new.join(format!("{}.legacy", name.to_string_lossy()));
                    std::fs::rename(entry.path(), &kept)?;
                } else {
                    std::fs::rename(entry.path(), &to)?;
                }
            }
            std::fs::remove_dir(&old).with_context(|| format!("removing the emptied {}", old.display()))?;
            fsync_dir(&new)?;
            if let Some(parent) = old.parent() {
                fsync_dir(parent)?;
            }
        }
        // Completion step, on every start: the database files take their new names, main file last.
        if new.join("workspace.sqlite").exists() && !self.db_path(project).exists() {
            for suffix in ["-wal", "-shm", ""] {
                let from = new.join(format!("workspace.sqlite{suffix}"));
                if from.exists() {
                    std::fs::rename(&from, new.join(format!("project.sqlite{suffix}")))?;
                }
            }
            fsync_dir(&new)?;
        }
        Ok(())
    }
}

/// Makes a directory's entries durable after renames (a no-op where the platform cannot).
pub(crate) fn fsync_dir(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        if let Ok(f) = std::fs::File::open(dir) {
            let _ = f.sync_all();
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// The local index: which projects this machine knows, and which upstreams each one claims.

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Index {
    #[serde(default, rename = "project")]
    pub projects: Vec<IndexEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexEntry {
    pub id: Uuid,
    pub name: String,
    /// `ops::repo_key`s of the member repositories' upstreams.
    #[serde(default)]
    pub upstreams: Vec<String>,
}

impl From<&IndexEntry> for Project {
    fn from(e: &IndexEntry) -> Self {
        Project { id: e.id, name: e.name.clone() }
    }
}

impl Index {
    pub fn load(paths: &Paths) -> Result<Index> {
        match std::fs::read_to_string(paths.index_path()) {
            Ok(text) => toml::from_str(&text).with_context(|| format!("parsing {}", paths.index_path().display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Index::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Changes the index under an exclusive lock: reload, apply `edit`, write a per-process temp
    /// file, sync it, rename it into place, sync the directory. Concurrent `fwd` commands (a
    /// hook per agent session, say) therefore never lose each other's registrations.
    pub fn update<R>(paths: &Paths, edit: impl FnOnce(&mut Index) -> R) -> Result<R> {
        use fs4::fs_std::FileExt;
        paths.ensure()?;
        let lock_path = paths.index_path().with_extension("toml.lock");
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .with_context(|| format!("opening {}", lock_path.display()))?;
        lock.lock_exclusive().with_context(|| format!("locking {}", lock_path.display()))?;
        let result = (|| {
            let mut index = Index::load(paths)?;
            let out = edit(&mut index);
            index.save(paths)?;
            Ok::<R, anyhow::Error>(out)
        })();
        let _ = lock.unlock();
        result
    }

    /// Atomic: a per-process temp file synced and renamed into place. Prefer `update`, which
    /// also serializes writers.
    pub fn save(&self, paths: &Paths) -> Result<()> {
        paths.ensure()?;
        let path = paths.index_path();
        let tmp = path.with_extension(format!("toml.{}.tmp", std::process::id()));
        let text = format!(
            "# fwd's local project index: the projects this machine knows and the upstream repositories each\n# one claims. `fwd init` and every resolution through a committed marker keep it current.\n{}",
            toml::to_string_pretty(self)?
        );
        {
            let mut f = std::fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
            use std::io::Write;
            f.write_all(text.as_bytes())?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &path).with_context(|| format!("replacing {}", path.display()))?;
        if let Some(dir) = path.parent() {
            fsync_dir(dir)?;
        }
        Ok(())
    }

    pub fn by_upstream(&self, key: &str) -> Vec<&IndexEntry> {
        self.projects.iter().filter(|p| p.upstreams.iter().any(|u| u == key)).collect()
    }

    /// By id, else by exact name. Several projects with one name is an error naming them all,
    /// never a silent first match.
    pub fn get(&self, id_or_name: &str) -> Result<Option<&IndexEntry>> {
        if let Ok(id) = Uuid::parse_str(id_or_name) {
            return Ok(self.projects.iter().find(|p| p.id == id));
        }
        let named: Vec<&IndexEntry> = self.projects.iter().filter(|p| p.name == id_or_name).collect();
        match named.as_slice() {
            [] => Ok(None),
            [one] => Ok(Some(one)),
            many => bail!(
                "{} projects are named `{id_or_name}`; pass an id instead: {}",
                many.len(),
                many.iter().map(|p| p.id.to_string()).collect::<Vec<_>>().join(", ")
            ),
        }
    }

    /// Records that `project` exists and claims `upstream`; true when anything changed.
    pub fn register(&mut self, project: &Project, upstream: Option<&str>) -> bool {
        let mut changed = false;
        let entry = match self.projects.iter_mut().find(|p| p.id == project.id) {
            Some(e) => e,
            None => {
                self.projects.push(IndexEntry { id: project.id, name: project.name.clone(), upstreams: Vec::new() });
                changed = true;
                self.projects.last_mut().expect("just pushed")
            }
        };
        if let Some(u) = upstream {
            if !entry.upstreams.iter().any(|x| x == u) {
                entry.upstreams.push(u.to_string());
                changed = true;
            }
        }
        changed
    }

    /// Withdraws a project's claim on an upstream; true when it was claimed.
    pub fn unclaim(&mut self, project: Uuid, upstream: &str) -> bool {
        match self.projects.iter_mut().find(|p| p.id == project) {
            Some(entry) => {
                let before = entry.upstreams.len();
                entry.upstreams.retain(|u| u != upstream);
                entry.upstreams.len() != before
            }
            None => false,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Checkouts

/// What git says about the checkout containing `dir`: `Ok(None)` outside any repository, and
/// an error when a `.git` is there but git cannot read it (a linked worktree whose main checkout
/// was deleted, say), so nothing mistakes a broken checkout for a fresh directory.
pub fn checkout_at(dir: &Path) -> Result<Option<Checkout>> {
    let root = match git(dir, &["rev-parse", "--show-toplevel"]) {
        Ok(out) => PathBuf::from(out.trim()),
        Err(e) => {
            return match dir.ancestors().find(|d| d.join(".git").exists()) {
                Some(marked) => Err(anyhow!(
                    "git cannot read the checkout at {} (was its main checkout deleted? `git worktree prune` there may help): {e}",
                    marked.display()
                )),
                None => Ok(None),
            };
        }
    };
    let upstream = git(&root, &["remote", "get-url", "origin"])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(|u| crate::ops::repo_key(&u));
    let branch = git(&root, &["symbolic-ref", "--short", "HEAD"]).ok().map(|s| s.trim().to_string());
    let head = git(&root, &["rev-parse", "--verify", "HEAD"]).ok().map(|s| s.trim().to_string());
    Ok(Some(Checkout { root, upstream, branch, head }))
}

/// Every worktree of the repository `root` belongs to, `root` included.
pub fn worktrees_of(root: &Path) -> Vec<PathBuf> {
    git(root, &["worktree", "list", "--porcelain"])
        .map(|out| out.lines().filter_map(|l| l.strip_prefix("worktree ")).map(PathBuf::from).collect())
        .unwrap_or_default()
}

/// The directory holding the objects and refs of the checkout at `root`, which every worktree of
/// one clone shares (git's common directory), so a question about objects asked of one worktree
/// is answered for all of them. It is read from files rather than asked of git, costing no
/// process: a clone's `.git` directory is its own; a linked worktree's `.git` file names the
/// worktree's git directory, whose `commondir` file names the shared one (a submodule's git
/// directory has no `commondir` and is its own). `None` when those files cannot be read, as in a
/// directory that is no checkout.
pub fn git_common_dir(root: &Path) -> Option<PathBuf> {
    let dot_git = root.join(".git");
    if dot_git.is_dir() {
        return dot_git.canonicalize().ok();
    }
    let git_dir = root.join(std::fs::read_to_string(&dot_git).ok()?.strip_prefix("gitdir:")?.trim());
    let common = match std::fs::read_to_string(git_dir.join("commondir")) {
        Ok(relative) => git_dir.join(relative.trim()),
        Err(_) => git_dir,
    };
    common.canonicalize().ok()
}

// ---------------------------------------------------------------------------------------------
// Markers

fn read_marker(dir: &Path) -> Result<Option<Marker>> {
    for name in [MARKER, LEGACY_MARKER] {
        let path = dir.join(name);
        if path.is_file() {
            let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
            return toml::from_str(&text).with_context(|| format!("parsing {}", path.display())).map(Some);
        }
    }
    Ok(None)
}

/// The marker that may name `start`'s project. Inside a checkout only its root counts: a marker
/// above the root belongs to whatever contains this repository, not to it. Outside git the
/// walk up is the whole mechanism.
fn marker_for(start: &Path, checkout: Option<&Checkout>) -> Result<Option<(PathBuf, Marker)>> {
    if let Some(c) = checkout {
        return Ok(read_marker(&c.root)?.map(|m| (c.root.clone(), m)));
    }
    let mut dir = Some(start.to_path_buf());
    while let Some(d) = dir {
        if let Some(m) = read_marker(&d)? {
            return Ok(Some((d, m)));
        }
        dir = d.parent().map(Path::to_path_buf);
    }
    Ok(None)
}

/// The project the marker at `root` names, if there is one.
pub fn marker_project(root: &Path) -> Result<Option<Project>> {
    Ok(read_marker(root)?.map(|m| Project { id: m.id, name: m.name }))
}

fn write_marker(root: &Path, project: &Project) -> Result<()> {
    let marker = root.join(MARKER);
    std::fs::create_dir_all(marker.parent().expect(".fwd"))?;
    let text = toml::to_string_pretty(&Marker { id: project.id, name: project.name.clone() })?;
    std::fs::write(
        &marker,
        format!(
            "# fwd project marker: the project this repository belongs to. Commit it so a fresh clone knows;\n# on a machine where any checkout of this repository's upstream has joined the project, it is\n# not needed (the local index knows the upstream).\n{text}"
        ),
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Resolution

/// The project a selector names: an id, a project name, or (when it contains a path separator,
/// or nothing is named that way and it is a directory) a directory inside one of the project's
/// checkouts. Names and ids come first so a project called `docs` is not shadowed by a folder.
pub fn select(paths: &Paths, index: &Index, selector: &str) -> Result<Project> {
    let looks_like_a_path = selector.contains(std::path::MAIN_SEPARATOR) || selector.contains('/');
    if !looks_like_a_path {
        if let Some(entry) = index.get(selector)? {
            return Ok(entry.into());
        }
    }
    if Path::new(selector).is_dir() {
        return Ok(resolve(paths, Path::new(selector), None)?.project);
    }
    bail!("no project is named `{selector}`, has that id, or has a checkout there; `fwd project list` shows the ones this machine knows")
}

/// The project for `start`: `wanted` (see `select`) names it outright; otherwise a marker at the
/// checkout's root (or, outside git, above `start`) names it and is recorded in the index;
/// otherwise the checkout's upstream must be claimed by exactly one indexed project. The
/// checkout is always the one containing `start`, whatever named the project.
pub fn resolve(paths: &Paths, start: &Path, wanted: Option<&str>) -> Result<Resolved> {
    let start = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    let checkout = checkout_at(&start)?;
    let root = checkout.as_ref().map(|c| c.root.clone()).unwrap_or_else(|| start.clone());
    let index = Index::load(paths)?;
    if let Some(w) = wanted {
        let project = select(paths, &index, w)?;
        return Ok(Resolved { project, checkout, root });
    }
    let upstream = checkout.as_ref().and_then(|c| c.upstream.clone());
    if let Some((dir, m)) = marker_for(&start, checkout.as_ref())? {
        let project = Project { id: m.id, name: m.name };
        // A marker teaches this machine's index about the project and this upstream, so every
        // other worktree of the repository resolves without one.
        let known = index.get(&project.id.to_string())?.is_some_and(|e| match &upstream {
            Some(u) => e.upstreams.iter().any(|x| x == u),
            None => true,
        });
        if !known {
            Index::update(paths, |idx| idx.register(&project, upstream.as_deref()))
                .with_context(|| index_not_written(paths, &project, upstream.as_deref(), &dir))?;
        }
        let root = if checkout.is_some() { root } else { dir };
        return Ok(Resolved { project, checkout, root });
    }
    let candidates = upstream.as_deref().map(|u| index.by_upstream(u)).unwrap_or_default();
    match candidates.as_slice() {
        [one] => Ok(Resolved { project: (*one).into(), checkout, root }),
        [] => Err(NoProject(start).into()),
        many => bail!(
            "this repository belongs to several projects ({}); pass --project <name|id>",
            many.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ")
        ),
    }
}

/// The sentence a resolution stops with when it cannot record in the index what a checkout's
/// marker taught it. A command that only reads does not expect to write, so the sentence names the
/// file it could not write, the directory the file lives in and why the command was writing it
/// (docs/DECISIONS.md, D86, "A read-only command that cannot write the project index stops with an
/// error that names projects.toml", 2026-09-28); the error from the write follows as its cause.
fn index_not_written(paths: &Paths, project: &Project, upstream: Option<&str>, marker_dir: &Path) -> String {
    let doing = match upstream {
        Some(u) => format!(
            "joining the checkout's upstream {u} to the project \"{}\", which the checkout's marker names and this machine's index did not yet list for that upstream",
            project.name
        ),
        None => format!(
            "adding the project \"{}\", which the marker in {} names, to this machine's index",
            project.name,
            marker_dir.display()
        ),
    };
    format!(
        "could not write projects.toml in the data directory {}: this command was {doing}; let a command write that directory once (a sandbox may forbid it), and from then on commands here only read the index",
        paths.data_dir.display()
    )
}

#[derive(Debug, Default, Clone)]
pub struct InitOptions<'a> {
    /// The new project's name; the directory's name otherwise.
    pub name: Option<&'a str>,
    /// Create a new project even when this repository's upstream already belongs to one.
    pub new: bool,
    /// Join this existing project (name or id) instead of creating one.
    pub join: Option<&'a str>,
}

/// `fwd init`: joins the project this repository already belongs to (by marker at the checkout
/// root, by upstream in the index, or by `join`), otherwise creates one. Writes the marker in
/// the checkout's root and records the upstream in the index. The bool is true when a project
/// was created. A new project may not take a name this machine already knows; a project a
/// teammate's marker names is never refused for its name.
pub fn init(paths: &Paths, start: &Path, opts: InitOptions) -> Result<(Resolved, bool)> {
    paths.ensure()?;
    let start = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    let checkout = checkout_at(&start)?;
    let root = checkout.as_ref().map(|c| c.root.clone()).unwrap_or_else(|| start.clone());
    let upstream = checkout.as_ref().and_then(|c| c.upstream.clone());
    let index = Index::load(paths)?;
    let fresh = |index: &Index| -> Result<Project> {
        let name = opts
            .name
            .map(str::to_string)
            .unwrap_or_else(|| root.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or("project".into()));
        if index.projects.iter().any(|p| p.name == name) {
            bail!("a project named `{name}` already exists on this machine; pass --name <another>, or --project {name} to join it");
        }
        Ok(Project { id: Uuid::new_v4(), name })
    };
    let (project, created) = if let Some(j) = opts.join {
        (select(paths, &index, j)?, false)
    } else if opts.new {
        (fresh(&index)?, true)
    } else if let Some((_, m)) = marker_for(&start, checkout.as_ref())? {
        (Project { id: m.id, name: m.name }, false)
    } else {
        let candidates = upstream.as_deref().map(|u| index.by_upstream(u)).unwrap_or_default();
        match candidates.as_slice() {
            [one] => (Project::from(*one), false),
            [] => (fresh(&index)?, true),
            many => bail!(
                "this repository's upstream belongs to several projects ({}); pass --project <name|id> or --new",
                many.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ")
            ),
        }
    };
    Index::update(paths, |idx| idx.register(&project, upstream.as_deref()))?;
    write_marker(&root, &project)?;
    Ok((Resolved { project, checkout, root }, created))
}

/// `fwd project rename`: gives `project` a new name in this machine's index and in the marker at
/// `root` when that marker names the project, and returns the marker it rewrote. The name is how
/// people select a project (`--project`, the page's selector), so a blank name, or one another
/// project on this machine already has, is refused. Markers in other checkouts keep the old name
/// until the rewritten marker is committed and reaches them.
pub fn rename(paths: &Paths, project: &Project, root: &Path, name: &str) -> Result<Option<PathBuf>> {
    if name.trim().is_empty() {
        bail!("a project's name cannot be blank");
    }
    Index::update(paths, |index| {
        if let Some(other) = index.projects.iter().find(|p| p.name == name && p.id != project.id) {
            bail!("a project named `{name}` already exists on this machine ({}); choose another name", other.id);
        }
        let entry = index
            .projects
            .iter_mut()
            .find(|p| p.id == project.id)
            .ok_or_else(|| anyhow!("project {} is not in this machine's index", project.id))?;
        entry.name = name.to_string();
        Ok(())
    })??;
    match marker_project(root)? {
        Some(marked) if marked.id == project.id => {
            write_marker(root, &Project { id: project.id, name: name.to_string() })?;
            Ok(Some(root.join(MARKER)))
        }
        _ => Ok(None),
    }
}

/// `fwd project unclaim`: withdraws a project's claim on an upstream this machine recorded by
/// mistake (a repository cloned inside another project's checkout, say).
pub fn unclaim(paths: &Paths, project: &str, upstream: &str) -> Result<bool> {
    let index = Index::load(paths)?;
    let id =
        index.get(project)?.map(|e| e.id).ok_or_else(|| anyhow!("no project is named `{project}` or has that id"))?;
    let key = crate::ops::repo_key(upstream);
    Index::update(paths, |idx| idx.unclaim(id, &key))
}
