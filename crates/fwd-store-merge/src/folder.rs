//! The data folder: what the move finds in the source, which it only reads, and the staging folder
//! it fills and names as the destination only once the new store is judged.

use std::fs;
use std::path::{Component, Path, PathBuf};

use uuid::Uuid;

use crate::old_store::open_immutable;

/// One thing in the source folder, by its path inside it.
#[derive(Debug, PartialEq)]
pub enum Item {
    Dir(PathBuf),
    /// `projects/<project>/project.sqlite`: moved into the one store.
    Store {
        project: Uuid,
        path: PathBuf,
    },
    /// `projects/<project>/checkouts.json`: carried with its scope bindings re-pointed.
    Checkouts {
        project: Uuid,
        path: PathBuf,
    },
    /// `projects/<project>/anchors.sqlite`: the anchors' cache, which rebuilds; left behind.
    Cache(PathBuf),
    /// Anything else fwd keeps: carried byte for byte.
    File(PathBuf),
}

#[derive(Debug, thiserror::Error)]
pub enum FolderError {
    #[error("{0} has no projects.toml, so it is not a fwd data folder")]
    NotDataFolder(PathBuf),
    #[error("{0} holds the one store already: this folder's projects were moved")]
    Moved(PathBuf),
    #[error("{0} is beside a database: a daemon is serving the folder, or the last process to open that database did not close it; stop the daemon (`fwd daemon stop`) and run again")]
    Busy(PathBuf),
    #[error("{0} is an Ankurah store outside projects/<project>/project.sqlite, which this move does not carry")]
    StrayStore(PathBuf),
    #[error("{0} is neither a file nor a folder")]
    Special(PathBuf),
    #[error("{to} is {relation} {from}")]
    Overlap { from: PathBuf, to: PathBuf, relation: &'static str },
    #[error("{0} already exists")]
    Exists(PathBuf),
    #[error("{path}: {error}")]
    Io { path: PathBuf, error: std::io::Error },
    #[error("reading {path}: {error}")]
    Sqlite { path: PathBuf, error: rusqlite::Error },
}

/// Everything in the data folder at `from`, folders before what they hold, once it is plain that
/// nothing is writing to it and its projects were not moved already.
pub fn survey(from: &Path) -> Result<Vec<Item>, FolderError> {
    if !from.join("projects.toml").is_file() {
        return Err(FolderError::NotDataFolder(from.to_owned()));
    }
    if from.join("store.sqlite").exists() {
        return Err(FolderError::Moved(from.join("store.sqlite")));
    }
    let mut items = Vec::new();
    walk(from, Path::new(""), &mut items)?;
    Ok(items)
}

fn walk(root: &Path, inside: &Path, items: &mut Vec<Item>) -> Result<(), FolderError> {
    let folder = root.join(inside);
    let mut entries =
        fs::read_dir(&folder).map_err(io(&folder))?.collect::<Result<Vec<_>, _>>().map_err(io(&folder))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let relative = inside.join(entry.file_name());
        let path = root.join(&relative);
        let kind = entry.file_type().map_err(io(&path))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if ["-wal", "-shm", "-journal"].iter().any(|suffix| name.ends_with(suffix)) {
            return Err(FolderError::Busy(path));
        }
        if kind.is_dir() {
            items.push(Item::Dir(relative.clone()));
            walk(root, &relative, items)?;
            continue;
        }
        if !kind.is_file() {
            return Err(FolderError::Special(path));
        }
        items.push(match in_project(&relative) {
            Some((project, "project.sqlite")) => Item::Store { project, path: relative },
            Some((project, "checkouts.json")) => Item::Checkouts { project, path: relative },
            Some((_, "anchors.sqlite")) => Item::Cache(relative),
            _ if is_ankurah_store(&path)? => return Err(FolderError::StrayStore(path)),
            _ => Item::File(relative),
        });
    }
    Ok(())
}

/// The project and file name, when `relative` is `projects/<project uuid>/<file>`.
fn in_project(relative: &Path) -> Option<(Uuid, &str)> {
    let parts: Vec<_> = relative.components().collect();
    match parts.as_slice() {
        [Component::Normal(projects), Component::Normal(project), Component::Normal(file)]
            if *projects == "projects" =>
        {
            Some((project.to_str()?.parse().ok()?, file.to_str()?))
        }
        _ => None,
    }
}

fn is_ankurah_store(path: &Path) -> Result<bool, FolderError> {
    use std::io::Read;
    let mut header = [0u8; 16];
    let read = fs::File::open(path).and_then(|mut file| file.read(&mut header)).map_err(io(path))?;
    if read < header.len() || &header != b"SQLite format 3\0" {
        return Ok(false);
    }
    let sqlite = |error| FolderError::Sqlite { path: path.to_owned(), error };
    let db = open_immutable(path).map_err(sqlite)?;
    db.query_row("SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = '_ankurah_event'", [], |row| {
        row.get::<_, i64>(0)
    })
    .map(|count| count > 0)
    .map_err(sqlite)
}

/// The folder the move is written into: beside the destination, under a name no fwd reads, until
/// [`Staging::settle`] gives it the destination's name. Until then, dropping it removes it, so an
/// ordinary failure leaves no partial folder; only a process that ends without unwinding (a kill,
/// an abort) can leave `<destination>.partial-<pid>` behind, which no fwd reads.
pub struct Staging {
    pub path: PathBuf,
    to: PathBuf,
    settled: bool,
}

impl Staging {
    pub fn create(from: &Path, to: &Path) -> Result<Self, FolderError> {
        let from = fs::canonicalize(from).map_err(io(from))?;
        let (parent, name) = match (to.parent(), to.file_name()) {
            (Some(parent), Some(name)) => (if parent.as_os_str().is_empty() { Path::new(".") } else { parent }, name),
            _ => return Err(FolderError::Exists(to.to_owned())),
        };
        let to = fs::canonicalize(parent).map_err(io(parent))?.join(name);
        if to.starts_with(&from) {
            return Err(FolderError::Overlap { from, to, relation: "inside" });
        }
        if from.starts_with(&to) {
            return Err(FolderError::Overlap { from, to, relation: "around" });
        }
        if to.exists() {
            return Err(FolderError::Exists(to));
        }
        let name = to.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let path = to.with_file_name(format!("{name}.partial-{}", std::process::id()));
        fs::create_dir(&path).map_err(io(&path))?;
        Ok(Self { path, to, settled: false })
    }

    /// Recreates a folder of the source with its permissions.
    pub fn add_dir(&self, from: &Path, relative: &Path) -> Result<(), FolderError> {
        let (source, target) = (from.join(relative), self.path.join(relative));
        fs::create_dir(&target).map_err(io(&target))?;
        let permissions = fs::metadata(&source).map_err(io(&source))?.permissions();
        fs::set_permissions(&target, permissions).map_err(io(&target))
    }

    /// Copies a file of the source as it is, permissions included.
    pub fn add_file(&self, from: &Path, relative: &Path) -> Result<(), FolderError> {
        let target = self.path.join(relative);
        fs::copy(from.join(relative), &target).map_err(io(&target))?;
        Ok(())
    }

    /// Gives the staging folder the destination's name, with the source's permissions (fwd keeps
    /// the data folder private).
    pub fn settle(mut self, from: &Path) -> Result<PathBuf, FolderError> {
        let permissions = fs::metadata(from).map_err(io(from))?.permissions();
        fs::set_permissions(&self.path, permissions).map_err(io(&self.path))?;
        fs::rename(&self.path, &self.to).map_err(io(&self.to))?;
        self.settled = true;
        Ok(self.to.clone())
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if !self.settled {
            if let Err(error) = fs::remove_dir_all(&self.path) {
                eprintln!("error: could not remove the partial folder {}: {error}", self.path.display());
            }
        }
    }
}

pub fn io(path: &Path) -> impl Fn(std::io::Error) -> FolderError + '_ {
    move |error| FolderError::Io { path: path.to_owned(), error }
}
