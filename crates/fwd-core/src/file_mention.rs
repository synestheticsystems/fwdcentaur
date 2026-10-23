//! A whole file named in a message's text, at a version, so that a writer can point a reader at a
//! file as the writer saw it (Daniel, 2026-09-30: "do we have a way of referencing a whole file
//! (for a given commit / unstaged state) and having that appear as a lozenge? we should."; task
//! nVE7KTeSqiX0). Such a reference is a mention like any other (docs/DECISIONS.md, D125,
//! "Anything a message names in its text is a mention, whatever its kind"): a `mention` relation
//! from the message to what it names, beside the text (D46, "Inline references are shadowed by
//! `mention` edges").
//!
//! A writer names a file in double brackets only, since an at-sign names people, agents and ids
//! (D121): `[[docs/cli.md]]` names the file as it stands on disk, unstaged changes included;
//! `[[docs/cli.md@index]]` its staged copy; `[[docs/cli.md@e13b8e2]]` the file at that commit,
//! and any revision git reads names one too (`HEAD`, a branch, a tag); `[[docs/cli.md@worktree]]`
//! names the file on disk outright, for a path with an at-sign of its own. The version follows the
//! last at-sign. A path is told from an id by its shape ([`read`]): it holds a slash or a dot,
//! which no id and no `D<number>` does, so a file at the root with neither is written with `./`
//! before it (`[[./Makefile]]`).
//!
//! Posting reads the file in the checkout the command runs in ([`reading_files_in`], which the CLI
//! wraps around each command), at the version named: the path must be a file there, on disk, in
//! the index or in the commit, or the message is refused with a sentence saying what was not
//! found. The file as read becomes a whole-file highlight (D43, "A whole-file reference is an
//! anchor with no line range"), the one made already for the same content or a new one, and the
//! text stores the reference as that highlight's full id, an at-sign and the version, in double
//! brackets (`[[<id>@<version>]]`, the version `worktree`, `index` or the full commit id): the
//! highlight is keyed by its content, so one highlight serves every version that holds the same
//! bytes, and the version the writer named goes with the mention (the lead's ruling on this task,
//! 2026-09-30). A symbolic revision is stored as the commit it named when the message was posted,
//! so that the reference does not drift as `HEAD` or a branch moves. That stored form reads again
//! with no checkout ([`Bracketed::Highlight`]), as a forward re-posts a quoted text and an edit
//! box writes the text back. The page and the extension host's node read no checkout, so a path
//! written there is refused with a sentence saying where to write it ([`NO_CHECKOUT`]).

use anyhow::{anyhow, bail, Result};
use std::fmt;

/// The version of a file a mention names, as the stored mention keeps it after the highlight's id.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Version {
    /// The file as it stood on disk, unstaged changes included: `worktree`.
    Worktree,
    /// The file as the index held it: `index`.
    Index,
    /// The file at a commit, by the commit's id in lower-case hexadecimal.
    Commit(String),
}

/// How a stored mention writes [`Version::Worktree`].
pub const WORKTREE: &str = "worktree";
/// How a stored mention writes [`Version::Index`].
pub const INDEX: &str = "index";

impl Version {
    /// A version as a stored mention writes it: `worktree`, `index`, or a commit's id, which the
    /// posting path writes whole and a writer may give from its first seven characters; none for
    /// anything else, a symbolic revision among them, which needs a checkout to read.
    pub fn parse(written: &str) -> Option<Version> {
        match written {
            WORKTREE => Some(Version::Worktree),
            INDEX => Some(Version::Index),
            _ if is_commit_id(written) => Some(Version::Commit(written.to_ascii_lowercase())),
            _ => None,
        }
    }

    /// The version as a stored mention writes it.
    pub fn as_str(&self) -> &str {
        match self {
            Version::Worktree => WORKTREE,
            Version::Index => INDEX,
            Version::Commit(commit) => commit,
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Whether `s` reads as a commit's id: seven hexadecimal digits or more, up to the 64 of a
/// SHA-256 repository's.
fn is_commit_id(s: &str) -> bool {
    (7..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Whether `s` has the shape of a path: a slash or a dot, which no id (URL-safe base64) and no
/// `D<number>` holds.
fn is_path(s: &str) -> bool {
    s.contains('/') || s.contains('.')
}

/// What stands between a pair of double brackets, read as a file mention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bracketed<'a> {
    /// A file by its path from the repository's root, and the version named after its last
    /// at-sign: none for the file on disk, else `worktree`, `index` or a revision.
    File { path: &'a str, revision: Option<&'a str> },
    /// A whole-file highlight by its id, or the start of it, and the version of the file the
    /// mention names: the form a posted file mention is stored in, which a forward re-posts and an
    /// edit box writes back, and which reads again with no checkout.
    Highlight { id: &'a str, version: &'a str },
}

/// `inner`, the text between a pair of double brackets, read as a file mention by its shape; none
/// for what names no file, which is read as an id or a decision's number. A path before the last
/// at-sign names a file at the version after it; a path with no at-sign, or whose at-sign has no
/// path before it (`a@b.md`), names the file on disk; and an id before the last at-sign names a
/// whole-file highlight at the version after it.
pub fn read(inner: &str) -> Option<Bracketed<'_>> {
    let inner = inner.trim();
    if let Some((before, after)) = inner.rsplit_once('@') {
        let before = before.trim();
        if is_path(before) {
            return Some(Bracketed::File { path: before, revision: Some(after.trim()) });
        }
        if !is_path(inner) {
            return Some(Bracketed::Highlight { id: before, version: after.trim() });
        }
    }
    is_path(inner).then_some(Bracketed::File { path: inner, revision: None })
}

/// The sentence that refuses a file named where no checkout is read, after "names a file, ": the
/// page and the extension host's node have none, and the CLI has none outside a checkout of the
/// project's repository.
#[cfg(feature = "native")]
pub const NO_CHECKOUT: &str = "which is read in the checkout a command runs in, and this one runs in no checkout of the project's repository: run it in one";
#[cfg(not(feature = "native"))]
pub const NO_CHECKOUT: &str = "which only a command run in a checkout reads, and the page has none: write it from the CLI (fwd comment, fwd reply) or in a comment in VS Code's editor";

/// Why a file named in a message's text was not read: no checkout is read here ([`NO_CHECKOUT`]),
/// so that the posting path can say so apart from a file that is not there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoCheckout;

impl fmt::Display for NoCheckout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(NO_CHECKOUT)
    }
}

impl std::error::Error for NoCheckout {}

/// Why a highlight named by its id and a version is refused as a file mention, before the id is
/// looked up: the version must be one a stored mention writes ([`Version::parse`]).
pub fn stored_version(version: &str) -> Result<Version> {
    Version::parse(version).ok_or_else(|| {
        anyhow!("after a highlight's id the version is worktree, index or a commit's id; to name a file at `{version}`, write its path")
    })
}

/// The whole-file highlight named by `id` (all of it, or its start) at `version`, as a stored file
/// mention names it ([`Bracketed::Highlight`]): refused unless `id` names a highlight on a whole
/// file and `version` is one a stored mention writes.
pub async fn highlight_at(ctx: &ankurah::Context, id: &str, version: &str) -> Result<(ankurah::EntityId, Version)> {
    use crate::model::CodeAnchorView;
    use crate::ops::{self, Kind};
    let version = stored_version(version)?;
    if id.is_empty() {
        bail!("no highlight's id comes before the at-sign");
    }
    let id = ops::resolve_id(ctx, id, &[Kind::Anchor]).await?;
    let anchor: CodeAnchorView = ctx.get(id).await.map_err(|_| anyhow!("no highlight has that id"))?;
    if !anchor.whole_file()?.unwrap_or(false) {
        bail!("a version goes with a whole file, and this highlight is on lines of one");
    }
    Ok((id, version))
}

#[cfg(feature = "native")]
pub use native::{highlight, read_file, reading_files_in, Read};

#[cfg(not(feature = "native"))]
pub use elsewhere::{highlight, read_file, Read};

#[cfg(feature = "native")]
mod native {
    use super::{NoCheckout, Version, INDEX, WORKTREE};
    use crate::anchoring::{self, Against, AnchorSpec};
    use crate::model::DiffSide;
    use ankurah::{Context, EntityId};
    use anyhow::{bail, Result};
    use std::future::Future;
    use std::path::{Path, PathBuf};

    tokio::task_local! {
        /// The checkout a command reads the files its texts name in, while [`reading_files_in`]
        /// runs it.
        static CHECKOUT: Option<PathBuf>;
    }

    /// Runs `work` reading the files its messages' texts name in `checkout`, the root of the
    /// checkout the command runs in (none outside one), so that every command that writes a
    /// message's text, a comment, a question, a reply, a forward's note, an edit or a decision's
    /// note, reads them where it runs without each passing its checkout down to the posting path.
    /// The CLI runs each command so; without it, as in the daemon, a file named is refused
    /// ([`NoCheckout`]).
    pub async fn reading_files_in<F: Future>(checkout: Option<PathBuf>, work: F) -> F::Output {
        CHECKOUT.scope(checkout, work).await
    }

    fn checkout() -> Result<PathBuf> {
        CHECKOUT.try_with(|c| c.clone()).ok().flatten().ok_or_else(|| NoCheckout.into())
    }

    /// A file a mention names, read in the checkout: the whole-file highlight it becomes, not made
    /// yet, and the version the writer named.
    #[derive(Debug, Clone)]
    pub struct Read {
        root: PathBuf,
        spec: AnchorSpec,
        pub version: Version,
    }

    /// Reads the file at `path` (from the repository's root, or absolute inside the checkout) at
    /// `revision` (none for the file on disk) in the checkout this command reads files in, and
    /// writes nothing to the graph: the posting path reads every reference of a text before it
    /// makes any highlight, so that a text refused leaves none behind. Git keeps the content read,
    /// pinned under `refs/fwd/basis/`, as it keeps every highlight's. Refused, with a sentence for
    /// the refusal to quote, when no checkout is read, when the revision is none this checkout
    /// has, and when the path is no file at that version.
    pub fn read_file(path: &str, revision: Option<&str>) -> Result<Read> {
        let root = checkout()?;
        let version = version_named(&root, revision)?;
        let spec = match &version {
            Version::Worktree => on_disk(&root, path)?,
            Version::Index => {
                let rel = anchoring::revision_path(&root, path)?;
                if anchoring::index_blob(&root, &rel)?.is_none() {
                    bail!("{rel} is not in the index");
                }
                anchoring::form_on_revision(&root, &rel, None, INDEX, DiffSide::Base)?
            }
            Version::Commit(commit) => {
                let rel = anchoring::revision_path(&root, path)?;
                let short: String = commit.chars().take(7).collect();
                match anchoring::git(&root, &["cat-file", "-t", &format!("{commit}:{rel}")])
                    .map(|t| t.trim().to_string())
                {
                    Ok(kind) if kind == "blob" => {}
                    Ok(kind) if kind == "tree" => bail!("{rel} is a directory at {short}, and a mention names a file"),
                    _ => bail!("{rel} is not in commit {short}"),
                }
                anchoring::form_on_revision(&root, &rel, None, commit, DiffSide::Base)?
            }
        };
        Ok(Read { root, spec, version })
    }

    /// The version `revision` names in the checkout at `root`: none, or `worktree`, the file on
    /// disk; `index`; else the commit a revision git reads names now, so that the mention keeps
    /// naming it as `HEAD` or a branch moves on.
    fn version_named(root: &Path, revision: Option<&str>) -> Result<Version> {
        let revision = match revision {
            None | Some(WORKTREE) => return Ok(Version::Worktree),
            Some(INDEX) => return Ok(Version::Index),
            Some("") => bail!("no version follows the at-sign: give worktree, index, or a commit, a branch or a tag"),
            Some(revision) => revision,
        };
        match Against::parse(root, revision) {
            Ok(Some(Against::Commit { commit, .. })) => Ok(Version::Commit(commit.to_ascii_lowercase())),
            Ok(Some(Against::Blob(_))) => bail!("`{revision}` names a blob, not a commit"),
            _ => bail!("this checkout has no revision `{revision}`"),
        }
    }

    /// The file at `path` as it stands on disk, or why it cannot be read: not there, or a directory.
    fn on_disk(root: &Path, path: &str) -> Result<AnchorSpec> {
        let given = Path::new(path);
        let file = if given.is_absolute() { given.to_path_buf() } else { root.join(given) };
        if !file.exists() {
            bail!("{path} is not on disk in this checkout");
        }
        if file.is_dir() {
            bail!("{path} is a directory, and a mention names a file");
        }
        anchoring::form_whole_file(root, path)
    }

    /// The whole-file highlight holding what `read` read: the one made already for the same
    /// content of the same file, else a new one made by `by`, alone in its transaction, as `fwd
    /// anchor create` makes one (`ops::create_anchor`).
    pub async fn highlight(ctx: &Context, by: EntityId, read: &Read) -> Result<EntityId> {
        let repo = crate::ops::ensure_repo(ctx, &read.root).await?;
        crate::ops::create_anchor(ctx, by, repo, &read.spec).await
    }
}

/// Where no checkout can be read (the page, the extension host's node): every file named is
/// refused ([`NoCheckout`]).
#[cfg(not(feature = "native"))]
mod elsewhere {
    use super::{NoCheckout, Version};
    use ankurah::{Context, EntityId};
    use anyhow::Result;

    /// A file a mention names, read in a checkout; never made where none is read.
    #[derive(Debug, Clone)]
    pub struct Read {
        pub version: Version,
    }

    /// Refuses the file: no checkout is read here.
    pub fn read_file(_path: &str, _revision: Option<&str>) -> Result<Read> {
        Err(NoCheckout.into())
    }

    /// Refuses the file: no checkout is read here, so nothing was read to make a highlight of.
    pub async fn highlight(_ctx: &Context, _by: EntityId, _read: &Read) -> Result<EntityId> {
        Err(NoCheckout.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_names_a_file_at_the_version_after_its_last_at_sign() {
        use Bracketed::*;
        assert_eq!(read("docs/cli.md"), Some(File { path: "docs/cli.md", revision: None }));
        assert_eq!(read(" README.md "), Some(File { path: "README.md", revision: None }), "trimmed");
        assert_eq!(read("docs/cli.md@e13b8e2"), Some(File { path: "docs/cli.md", revision: Some("e13b8e2") }));
        assert_eq!(read("docs/cli.md@index"), Some(File { path: "docs/cli.md", revision: Some("index") }));
        assert_eq!(read("src/x.rs@feat/retry"), Some(File { path: "src/x.rs", revision: Some("feat/retry") }));
        assert_eq!(
            read("node_modules/@types/node/index.d.ts@worktree"),
            Some(File { path: "node_modules/@types/node/index.d.ts", revision: Some("worktree") }),
            "the version follows the last at-sign"
        );
        assert_eq!(read("./Makefile"), Some(File { path: "./Makefile", revision: None }));
        assert_eq!(read("a.md@"), Some(File { path: "a.md", revision: Some("") }), "an empty version, refused later");
        assert_eq!(read("mail@example.com"), Some(File { path: "mail@example.com", revision: None }));
    }

    #[test]
    fn an_id_before_an_at_sign_names_a_highlight_at_a_version_and_an_id_alone_names_no_file() {
        use Bracketed::*;
        assert_eq!(read("vwugaN8h@index"), Some(Highlight { id: "vwugaN8h", version: "index" }));
        let id = "nVE7KTeSqiX0C2586UTkE-DJ9ggQg1TUYFQI-BzZRMw";
        let commit = "e13b8e216257b10dc4f3df4908ab34c26ed5497b";
        assert_eq!(read(&format!("{id}@{commit}")), Some(Highlight { id, version: commit }));
        assert_eq!(read("@worktree"), Some(Highlight { id: "", version: "worktree" }), "no id, refused later");
        for not_a_file in ["vwugaN8h", "D12", "Makefile", "two words", ""] {
            assert_eq!(read(not_a_file), None, "{not_a_file}");
        }
    }

    #[test]
    fn a_stored_version_is_worktree_index_or_a_commits_id() {
        assert_eq!(Version::parse("worktree"), Some(Version::Worktree));
        assert_eq!(Version::parse("index"), Some(Version::Index));
        let commit = "E13B8E216257B10DC4F3DF4908AB34C26ED5497B";
        assert_eq!(Version::parse(commit), Some(Version::Commit(commit.to_ascii_lowercase())), "lower case");
        assert_eq!(Version::parse("e13b8e2"), Some(Version::Commit("e13b8e2".into())), "its first seven characters");
        for refused in ["HEAD", "main", "e13b8e", "", "g13b8e2", &"a".repeat(65)] {
            assert_eq!(Version::parse(refused), None, "{refused}");
        }
        assert_eq!(Version::Commit("abcdef0".into()).to_string(), "abcdef0");
        assert_eq!(Version::Worktree.to_string(), "worktree");
        let refused = stored_version("HEAD").unwrap_err().to_string();
        assert!(refused.contains("write its path"), "{refused}");
    }
}
