//! Code anchors: forming one from what a person is looking at, and finding it again in another
//! version of the file. See docs/design/anchoring.md, and for the reasons behind the shape,
//! docs/research/anchoring-robust-proposal.md (sections 3.1 to 3.3 and 3.5).
//!
//! An anchor's version, its basis, is git's object id of the content the person saw, whether that
//! content was committed, staged or only in the working tree. The content is written to the
//! repository and pinned under `refs/fwd/basis/`, and the anchor carries a patch from a blob other
//! machines are likely to hold, so any checkout can rebuild the basis and check its id. Finding
//! the lines in another version follows the edits between the two blobs with a Myers line diff
//! (the `similar` crate), then falls back to searching for the anchored text. Every answer states
//! three facts apart: where the lines are, whether their text changed, and whether they are there
//! at all. Git is driven through the `git` executable.

use crate::model::{DiffSide, Piece};
use anyhow::{anyhow, bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use similar::{Algorithm, ChangeTag, DiffOp, TextDiff};
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const CONTEXT_LINES: usize = 3;
pub const FUZZY_THRESHOLD: f32 = 0.6;
/// The largest basis patch an anchor carries (the proposal's section 5): past it only the pinned
/// blob holds the basis, and a checkout without that blob resolves by searching for the text.
pub const PATCH_CAP: usize = 256 * 1024;
const KEY_PREFIX: &str = "fwd-anchor:1:";
const SPAN_KEY_PREFIX: &str = "fwd-span:1:";
/// How long one line diff may run before `similar` settles for a coarser answer.
const DIFF_DEADLINE: Duration = Duration::from_secs(2);
/// How many untracked files are hashed when looking for a file moved without git being told.
const UNTRACKED_SCAN_LIMIT: usize = 500;
/// How many renames are followed back through history before giving up.
const RENAME_HOPS: usize = 8;

/// What an anchor holds, as this module reads and writes it; see the `CodeAnchor` model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnchorSpec {
    pub path: String,
    /// The anchored lines, 0-based with the end exclusive; `None` for a whole-file anchor.
    pub lines: Option<(i64, i64)>,
    /// git's id of the content the anchor was made on. `None` on an anchor of the first cut until
    /// a checkout supplies one (`backfill`).
    pub basis_blob: Option<String>,
    pub head_blob: Option<String>,
    pub index_blob: Option<String>,
    pub nearest_blob: Option<String>,
    pub basis_patch: Option<String>,
    pub pieces: Vec<Piece>,
    pub commit_sha: Option<String>,
    /// The first cut's basis (the model's `blob_sha`): the HEAD blob a selection unchanged since
    /// HEAD was mapped onto. Only anchors made before `basis_blob` existed have one.
    pub legacy_blob: Option<String>,
    pub anchored_text: String,
    pub context_before: String,
    pub context_after: String,
    /// The file on disk, or the side of a diff showing the revision the anchor was made on.
    pub side: DiffSide,
}

impl AnchorSpec {
    pub fn whole_file(&self) -> bool {
        self.lines.is_none()
    }

    /// The blob whose lines the anchor's range counts: its basis, or the first cut's HEAD blob.
    fn numbered_in(&self) -> Option<&str> {
        self.basis_blob.as_deref().or(self.legacy_blob.as_deref())
    }

    /// The versions recorded beside the basis that are older than it, so that an anchored line
    /// one of them lacks came after it, and a diff from the basis is the whole truth about it:
    /// HEAD's and the index's blobs for an anchor made on the file on disk, and HEAD's for one
    /// made on the index. The HEAD and index recorded beside an anchor made on HEAD or on another
    /// commit may be newer than its basis: a line they lack may have been deleted or moved since,
    /// which only a search tells apart.
    fn older_versions(&self) -> Vec<&str> {
        let made_on_index = self.basis_blob.is_some() && self.basis_blob == self.index_blob;
        let older = match self.side {
            DiffSide::Worktree => vec![&self.head_blob, &self.index_blob],
            DiffSide::Base | DiffSide::Modified if made_on_index => vec![&self.head_blob],
            DiffSide::Base | DiffSide::Modified => Vec::new(),
        };
        older.into_iter().filter_map(|b| b.as_deref()).collect()
    }
}

// ---------------------------------------------------------------------------------------------
// Git

fn git_command(repo_root: &Path, args: &[&str]) -> Command {
    let mut command = Command::new("git");
    command.arg("-C").arg(repo_root).args(args);
    command
}

pub fn git(repo_root: &Path, args: &[&str]) -> Result<String> {
    Ok(String::from_utf8_lossy(&git_bytes(repo_root, args)?).into_owned())
}

fn git_bytes(repo_root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = git_command(repo_root, args).output().with_context(|| format!("running git {}", args.join(" ")))?;
    if !out.status.success() {
        bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(out.stdout)
}

/// Runs git with `input` on its standard input.
fn git_with_input(repo_root: &Path, args: &[&str], input: &[u8]) -> Result<String> {
    let mut child = git_command(repo_root, args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("running git {}", args.join(" ")))?;
    // Written from a thread, so a large input never waits on output that nothing reads yet.
    let mut stdin = child.stdin.take().ok_or_else(|| anyhow!("git's standard input was not piped"))?;
    let input = input.to_vec();
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let out = child.wait_with_output().with_context(|| format!("running git {}", args.join(" ")))?;
    if !out.status.success() {
        bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    writer
        .join()
        .map_err(|_| anyhow!("writing to git {} panicked", args.join(" ")))?
        .with_context(|| format!("writing to git {}", args.join(" ")))?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Git's yes (exit 0) or no (exit 1) to a question such as `diff --quiet`; any other exit is an
/// error.
fn git_answers(repo_root: &Path, args: &[&str]) -> Result<bool> {
    let out = git_command(repo_root, args).output().with_context(|| format!("running git {}", args.join(" ")))?;
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()),
    }
}

pub fn head_commit(repo_root: &Path) -> Result<Option<String>> {
    Ok(git(repo_root, &["rev-parse", "--verify", "HEAD"]).ok().map(|s| s.trim().to_string()))
}

/// The blob sha of `path` at `rev`, if it exists there.
pub fn blob_at(repo_root: &Path, rev: &str, path: &str) -> Result<Option<String>> {
    Ok(git(repo_root, &["rev-parse", "--verify", "--quiet", &format!("{rev}:{path}")])
        .ok()
        .map(|s| s.trim().to_string()))
}

/// The blob the index holds for `path`, if it holds one (stage 0: not a conflict).
pub fn index_blob(repo_root: &Path, path: &str) -> Result<Option<String>> {
    Ok(git(repo_root, &["rev-parse", "--verify", "--quiet", &format!(":0:{path}")]).ok().map(|s| s.trim().to_string()))
}

pub fn blob_text(repo_root: &Path, blob_sha: &str) -> Result<String> {
    Ok(String::from_utf8_lossy(&blob_bytes(repo_root, blob_sha)?).into_owned())
}

fn blob_bytes(repo_root: &Path, blob: &str) -> Result<Vec<u8>> {
    git_bytes(repo_root, &["cat-file", "blob", blob])
}

/// True when `rev` names a commit in this checkout (a branch that was never pushed here does
/// not, and that is an `absent` answer, not an orphan).
pub fn rev_exists(repo_root: &Path, rev: &str) -> bool {
    git(repo_root, &["rev-parse", "--verify", "--quiet", &format!("{rev}^{{commit}}")]).is_ok()
}

/// True when the object database here holds the blob.
pub fn blob_exists(repo_root: &Path, sha: &str) -> bool {
    git(repo_root, &["cat-file", "-e", &format!("{sha}^{{blob}}")]).is_ok()
}

/// git's id for `content` as `git hash-object --path` gives it: the repository's clean filters
/// (line endings, `.gitattributes`) apply, as they do when the file is added. With `write` the
/// object is written too.
fn hash_object(repo_root: &Path, path: &str, content: &[u8], write: bool) -> Result<String> {
    let path_arg = format!("--path={path}");
    let mut args = vec!["hash-object", "--stdin", path_arg.as_str()];
    if write {
        args.insert(1, "-w");
    }
    Ok(git_with_input(repo_root, &args, content)?.trim().to_string())
}

/// git's id for bytes that are already a blob's content (a rebuilt basis): no filters.
fn hash_blob_content(repo_root: &Path, content: &[u8]) -> Result<String> {
    Ok(git_with_input(repo_root, &["hash-object", "--stdin", "--no-filters"], content)?.trim().to_string())
}

/// Keeps `blob` from `git gc` with a ref naming it (a ref may name a blob): `refs/fwd/basis/<id>`,
/// never a branch, the index or the working tree. Writing it again changes nothing.
pub fn pin(repo_root: &Path, blob: &str) -> Result<()> {
    git(repo_root, &["update-ref", &format!("refs/fwd/basis/{blob}"), blob]).map(|_| ())
}

/// True when the working-tree file matches the index by git's own comparison, which reads the
/// index and so applies its filters exactly as `git add` would.
fn worktree_matches_index(repo_root: &Path, path: &str) -> bool {
    git_answers(repo_root, &["diff", "--quiet", "--no-ext-diff", "--", path]).unwrap_or(false)
}

/// The id the working-tree `content` of `path` has as a blob: the index blob when the file
/// matches the index, else `git hash-object --path`. The index blob comes first because
/// `hash-object` does not read the index, and git keeps CRLF line endings in a file whose index
/// copy already has them where `hash-object` would convert (experiment E1 in the proposal's
/// section 6). With `write` a hashed object is written too.
fn worktree_blob(repo_root: &Path, path: &str, content: &[u8], write: bool) -> Result<String> {
    if let Some(blob) = index_blob(repo_root, path)? {
        if worktree_matches_index(repo_root, path) {
            return Ok(blob);
        }
    }
    hash_object(repo_root, path, content, write)
}

/// A path as anchors store it: relative to the checkout root, so an anchor never names a file
/// outside its repository and the same string names the same file in every checkout. A relative
/// path is taken from the checkout root and may not climb (`..`); an absolute path inside the
/// checkout is converted to the relative form (Daniel, 2026-09-26: "is there a good reason why
/// absolute paths would be problematic? I don't see an issue"). Either way the file is found with
/// symlinks followed, and refused when it lies outside the checkout root, or inside another
/// checkout nested in this one (a worktree under `.claude/worktrees/`, a submodule): its files
/// belong to that checkout.
pub fn normalize_path(repo_root: &Path, path: &str) -> Result<String> {
    use std::path::Component;
    let given = Path::new(path);
    let root = repo_root.canonicalize().with_context(|| format!("reading the checkout at {}", repo_root.display()))?;
    let file = if given.is_absolute() {
        given.canonicalize().with_context(|| format!("reading {path}"))?
    } else {
        if !given.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir)) {
            bail!("`{path}` leaves the checkout (`..` or a prefix); anchor paths stay inside the repository");
        }
        root.join(given).canonicalize().with_context(|| format!("reading {path}"))?
    };
    let relative = file.strip_prefix(&root).map_err(|_| {
        anyhow!(
            "`{path}` is outside this checkout ({}), at {}; anchor paths name files inside it",
            root.display(),
            file.display()
        )
    })?;
    // Any directory between the file and the root that holds a `.git` of its own is another
    // checkout.
    let mut dir = file.parent();
    while let Some(d) = dir.filter(|d| *d != root && d.starts_with(&root)) {
        if d.join(".git").exists() {
            bail!(
                "`{path}` is in another checkout ({}) inside this one ({}); anchor paths name files of this checkout",
                d.display(),
                root.display()
            );
        }
        dir = d.parent();
    }
    Ok(relative.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/"))
}

/// A stored path this code will read: the form `normalize_path` produces.
pub fn stored_path_is_safe(path: &str) -> bool {
    use std::path::Component;
    let p = Path::new(path);
    !path.is_empty() && !p.is_absolute() && p.components().all(|c| matches!(c, Component::Normal(_)))
}

// ---------------------------------------------------------------------------------------------
// Lines, diffs and patches

/// A text's lines as a person counts them: split at each `\n`, a `\r` before it dropped, so a
/// file's CRLF and LF forms have the same lines.
fn lines(text: &str) -> Vec<&str> {
    text.lines().collect()
}

fn slice(lines: &[&str], start: i64, end: i64) -> String {
    let start = start.max(0) as usize;
    let end = (end.max(0) as usize).min(lines.len());
    if start >= end {
        return String::new();
    }
    lines[start..end].join("\n")
}

fn diff_ops(old: &[&str], new: &[&str]) -> Vec<DiffOp> {
    similar::capture_diff_slices_deadline(Algorithm::Myers, old, new, Some(Instant::now() + DIFF_DEADLINE))
}

/// For each line of `old`, the line of `new` a Myers diff over whole lines pairs it with, if any.
fn line_map(old: &[&str], new: &[&str]) -> Vec<Option<usize>> {
    let mut map = vec![None; old.len()];
    for op in diff_ops(old, new) {
        if let DiffOp::Equal { old_index, new_index, len } = op {
            for k in 0..len {
                map[old_index + k] = Some(new_index + k);
            }
        }
    }
    map
}

/// How many lines a diff from `old` to `new` removes and adds.
fn diff_size(old: &[&str], new: &[&str]) -> usize {
    diff_ops(old, new)
        .iter()
        .map(|op| match *op {
            DiffOp::Equal { .. } => 0,
            DiffOp::Delete { old_len, .. } => old_len,
            DiffOp::Insert { new_len, .. } => new_len,
            DiffOp::Replace { old_len, new_len, .. } => old_len + new_len,
        })
        .sum()
}

/// A unified diff taking `old` to `new`, exact to the byte so the result can be checked against a
/// blob id: lines are split after each `\n` (a `\r` stays part of its line, where `similar`'s own
/// line splitting would end a line at a lone `\r`), and a last line without a newline is marked
/// as git marks it. Empty when the texts are equal.
pub fn unified_patch(path: &str, old: &str, new: &str) -> String {
    let old_lines: Vec<&str> = old.split_inclusive('\n').collect();
    let new_lines: Vec<&str> = new.split_inclusive('\n').collect();
    let ops = similar::capture_diff_slices_deadline(
        Algorithm::Myers,
        &old_lines,
        &new_lines,
        Some(Instant::now() + DIFF_DEADLINE),
    );
    if ops.iter().all(|op| matches!(op, DiffOp::Equal { .. })) {
        return String::new();
    }
    // A side with no lines names the line before it; otherwise its first line, counted from 1.
    let side = |start: usize, end: usize| {
        if end == start {
            format!("{start},0")
        } else {
            format!("{},{}", start + 1, end - start)
        }
    };
    let mut out = format!("--- a/{path}\n+++ b/{path}\n");
    for group in similar::group_diff_ops(ops, 3) {
        let (Some(first), Some(last)) = (group.first(), group.last()) else { continue };
        out.push_str(&format!(
            "@@ -{} +{} @@\n",
            side(first.old_range().start, last.old_range().end),
            side(first.new_range().start, last.new_range().end)
        ));
        for op in &group {
            for change in op.iter_changes(&old_lines, &new_lines) {
                out.push(match change.tag() {
                    ChangeTag::Equal => ' ',
                    ChangeTag::Delete => '-',
                    ChangeTag::Insert => '+',
                });
                let line = change.value();
                out.push_str(line);
                if !line.ends_with('\n') {
                    out.push_str("\n\\ No newline at end of file\n");
                }
            }
        }
    }
    out
}

/// Applies a patch `unified_patch` made to the text it was made from. Every context and removed
/// line is checked, so a patch applied to any other text is refused rather than half applied.
pub fn apply_patch(old: &str, patch: &str) -> Result<String> {
    let old_lines: Vec<&str> = old.split_inclusive('\n').collect();
    let mut out = String::with_capacity(old.len() + patch.len());
    let mut next = 0usize;
    let mut rows = patch.split_inclusive('\n').peekable();
    while let Some(row) = rows.next() {
        // File headers, and anything else outside a hunk, carry nothing to apply.
        let Some(header) = row.strip_prefix("@@ -") else { continue };
        let (old_start, old_len, new_len) = hunk_header(header)?;
        let at = if old_len == 0 { Some(old_start) } else { old_start.checked_sub(1) };
        let Some(at) = at.filter(|&at| at >= next && at <= old_lines.len()) else {
            bail!("the patch's hunks are out of order or reach past the text's end");
        };
        for line in &old_lines[next..at] {
            out.push_str(line);
        }
        next = at;
        let (mut old_seen, mut new_seen) = (0, 0);
        while old_seen < old_len || new_seen < new_len {
            let row = rows.next().ok_or_else(|| anyhow!("the patch ends inside a hunk"))?;
            let mut chars = row.chars();
            let tag = chars.next().ok_or_else(|| anyhow!("an empty line inside a hunk"))?;
            let mut line = chars.as_str();
            // The marker after a line says that line has no newline at its end.
            if rows.peek().is_some_and(|r| r.starts_with('\\')) {
                rows.next();
                line = line.strip_suffix('\n').unwrap_or(line);
            }
            match tag {
                ' ' | '-' => {
                    if old_lines.get(next) != Some(&line) {
                        bail!("the patch does not fit this text at line {}", next + 1);
                    }
                    next += 1;
                    old_seen += 1;
                    if tag == ' ' {
                        out.push_str(line);
                        new_seen += 1;
                    }
                }
                '+' => {
                    out.push_str(line);
                    new_seen += 1;
                }
                _ => bail!("a line inside a hunk starts with {tag:?}"),
            }
        }
    }
    for line in &old_lines[next..] {
        out.push_str(line);
    }
    Ok(out)
}

/// `a,b +c,d @@...` (what follows `@@ -`) as the old start, the old length and the new length.
fn hunk_header(header: &str) -> Result<(usize, usize, usize)> {
    let bad = || anyhow!("a hunk header that does not parse: @@ -{}", header.trim_end());
    let (old, rest) = header.split_once(" +").ok_or_else(bad)?;
    let (new, _) = rest.split_once(" @@").ok_or_else(bad)?;
    let range = |r: &str| -> Result<(usize, usize)> {
        match r.split_once(',') {
            Some((start, len)) => Ok((start.parse().map_err(|_| bad())?, len.parse().map_err(|_| bad())?)),
            None => Ok((r.parse().map_err(|_| bad())?, 1)),
        }
    };
    let (old_start, old_len) = range(old)?;
    let (_, new_len) = range(new)?;
    Ok((old_start, old_len, new_len))
}

// ---------------------------------------------------------------------------------------------
// Identity

/// The anchor's identity (the proposal's section 3.1): `fwd-anchor:1:` and the SHA-256 of the
/// repository entity's id, the path, the basis blob, the line range (empty for a whole-file
/// anchor) and the columns (empty until columns exist), NUL-separated. The same lines of the same
/// content in the same repository give the same key on any machine.
pub fn anchor_key(repo: &str, path: &str, basis_blob: &str, lines: Option<(i64, i64)>) -> String {
    let range = lines.map(|(start, end)| format!("{start}-{end}")).unwrap_or_default();
    let columns = "";
    let mut hash = Sha256::new();
    for (i, part) in [repo, path, basis_blob, range.as_str(), columns].into_iter().enumerate() {
        if i > 0 {
            hash.update([0u8]);
        }
        hash.update(part.as_bytes());
    }
    format!("{KEY_PREFIX}{}", hex(&hash.finalize()))
}

/// The anchored text's own fingerprint, for recognising two anchors on the same text:
/// `fwd-span:1:` and the SHA-256 of the text with trailing whitespace stripped from each line and
/// every other run of whitespace made one space. Identifiers and literals are left alone, so the
/// same span survives reformatting but not a rename.
pub fn span_key(anchored_text: &str) -> String {
    let normalized: Vec<String> = anchored_text
        .lines()
        .map(|line| {
            let mut out = String::with_capacity(line.len());
            let mut in_space = false;
            for c in line.trim_end().chars() {
                if c.is_whitespace() {
                    if !in_space {
                        out.push(' ');
                    }
                    in_space = true;
                } else {
                    out.push(c);
                    in_space = false;
                }
            }
            out
        })
        .collect();
    format!("{SPAN_KEY_PREFIX}{}", hex(&Sha256::digest(normalized.join("\n").as_bytes())))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ---------------------------------------------------------------------------------------------
// Forming an anchor

/// Forms an anchor for lines `[start, end)` (0-based, end exclusive) of the working-tree file.
pub fn form(repo_root: &Path, path: &str, start: i64, end: i64) -> Result<AnchorSpec> {
    check_range(start, end)?;
    form_basis(repo_root, path, Some((start, end)), &Against::Worktree, DiffSide::Worktree)
}

/// Forms an anchor on the whole working-tree file: no line range, and any content, text or not.
pub fn form_whole_file(repo_root: &Path, path: &str) -> Result<AnchorSpec> {
    form_basis(repo_root, path, None, &Against::Worktree, DiffSide::Worktree)
}

/// Forms an anchor on lines `[start, end)` of the file as a revision holds it, or with no lines
/// on the whole file there, so that lines shown on a side of a diff other than the working
/// tree's can take a comment: `target` names the revision as `Against::parse` reads it (`index`,
/// `HEAD`, a branch or a commit), and `side` the side of the diff that showed it. The revision's
/// blob of the file is the basis, pinned as a working-tree basis is, and the HEAD and index
/// blobs, the nearest blob and its patch, and the pieces are recorded as for an anchor made on
/// the file on disk. The file need not be on disk: HEAD holds a file the working tree deleted.
pub fn form_on_revision(
    repo_root: &Path,
    path: &str,
    lines: Option<(i64, i64)>,
    target: &str,
    side: DiffSide,
) -> Result<AnchorSpec> {
    if side == DiffSide::Worktree {
        bail!("lines of a revision are on the base or the modified side of a diff, never the working tree's");
    }
    if let Some((start, end)) = lines {
        check_range(start, end)?;
    }
    let revision = match Against::parse(repo_root, target)? {
        Some(Against::Worktree) => bail!("`worktree` is the file on disk, which is anchored without --against"),
        Some(Against::Blob(_)) => {
            bail!("`{target}` names a blob, not a version of a file: give index, HEAD, or a branch or commit")
        }
        Some(revision) => revision,
        None => bail!("this checkout has no `{target}`: give index, HEAD, or a branch or commit it has"),
    };
    form_basis(repo_root, path, lines, &revision, side)
}

/// Refuses a line range with no lines in it.
fn check_range(start: i64, end: i64) -> Result<()> {
    if start < 0 || end <= start {
        bail!("an anchor needs a non-empty line range; got {start}..{end}");
    }
    Ok(())
}

/// Records what the person is looking at, in the working tree or in a revision (`version`) shown
/// on `side`: the content's blob id (written and pinned), the HEAD and index blobs of the file at
/// this moment, the nearest of them and a patch from it to the basis, and for a line anchor, its
/// lines' pieces and text. In the working tree the lines count the working-tree text, which is
/// the basis content with the repository's filters undone; in a revision, the blob's own text,
/// which is what a diff's side shows.
fn form_basis(
    repo_root: &Path,
    path: &str,
    wanted: Option<(i64, i64)>,
    version: &Against,
    side: DiffSide,
) -> Result<AnchorSpec> {
    let (path, seen, revision_blob) = match version {
        Against::Worktree => {
            let path = normalize_path(repo_root, path)?;
            let seen = std::fs::read(repo_root.join(&path)).with_context(|| format!("reading {path}"))?;
            (path, seen, None)
        }
        revision => {
            let path = revision_path(repo_root, path)?;
            let Some((Some(blob), content)) = content_at(repo_root, revision, &path)? else {
                bail!("`{path}` is not in {}, so it has nothing there to anchor", revision.named());
            };
            (path, content, Some(blob))
        }
    };
    let (anchored_text, context_before, context_after) = match wanted {
        None => Default::default(),
        Some((start, end)) => {
            let text = std::str::from_utf8(&seen).map_err(|_| {
                anyhow!("`{path}` is not UTF-8 text, so it has no lines to anchor; anchor the whole file instead (no --lines)")
            })?;
            let all = lines(text);
            if end as usize > all.len() {
                bail!("the file has {} lines; {start}..{end} is out of range", all.len());
            }
            let context = CONTEXT_LINES as i64;
            (slice(&all, start, end), slice(&all, start - context, start), slice(&all, end, end + context))
        }
    };
    let head_blob = blob_at(repo_root, "HEAD", &path)?;
    let index_blob = index_blob(repo_root, &path)?;
    let basis_blob = match revision_blob {
        Some(blob) => blob,
        None => worktree_blob(repo_root, &path, &seen, true)?,
    };
    // A revision's blob is in the object database already, but the index's is kept only while the
    // index holds it, and a commit's only while a branch reaches it.
    pin(repo_root, &basis_blob)?;
    let text_of = |blob: &str| -> Result<Option<String>> { Ok(String::from_utf8(blob_bytes(repo_root, blob)?).ok()) };
    let basis_text = text_of(&basis_blob)?;
    let head_text = head_blob.as_deref().map(text_of).transpose()?.flatten();
    let index_text = index_blob.as_deref().map(text_of).transpose()?.flatten();
    let (nearest_blob, basis_patch) = nearest_and_patch(
        &path,
        &basis_blob,
        basis_text.as_deref(),
        [(head_blob.as_deref(), head_text.as_deref()), (index_blob.as_deref(), index_text.as_deref())],
    );
    let pieces = match (wanted, basis_text.as_deref()) {
        (Some((start, end)), Some(basis_text)) => {
            let basis = lines(basis_text);
            let head_map = head_text.as_deref().map(|t| line_map(&basis, &lines(t)));
            let index_map = index_text.as_deref().map(|t| line_map(&basis, &lines(t)));
            pieces_of(start as usize, end as usize, head_map.as_deref(), index_map.as_deref())
        }
        _ => Vec::new(),
    };
    Ok(AnchorSpec {
        path,
        lines: wanted,
        basis_blob: Some(basis_blob),
        head_blob,
        index_blob,
        nearest_blob,
        basis_patch,
        pieces,
        commit_sha: head_commit(repo_root)?,
        legacy_blob: None,
        anchored_text,
        context_before,
        context_after,
        side,
    })
}

/// A path as anchors store it, for a file as a revision holds it, which the working tree may no
/// longer have (deleted, or moved since): `normalize_path`'s answer while the file is on disk,
/// and otherwise the path as written, from the checkout root (an absolute path inside the
/// checkout made relative), refused as `normalize_path` refuses a path that leaves the checkout.
/// Git has no file at a path through a symlink or into another checkout, so the revision holding
/// a file at the path is the rest of the check.
pub(crate) fn revision_path(repo_root: &Path, path: &str) -> Result<String> {
    use std::path::Component;
    let given = Path::new(path);
    let on_disk = if given.is_absolute() { given.to_path_buf() } else { repo_root.join(given) };
    if on_disk.exists() {
        return normalize_path(repo_root, path);
    }
    let root = repo_root.canonicalize().with_context(|| format!("reading the checkout at {}", repo_root.display()))?;
    let relative = if given.is_absolute() {
        given.strip_prefix(&root).or_else(|_| given.strip_prefix(repo_root)).map_err(|_| {
            anyhow!("`{path}` is outside this checkout ({}); anchor paths name files inside it", root.display())
        })?
    } else {
        given
    };
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::CurDir => {}
            _ => bail!("`{path}` leaves the checkout (`..` or a prefix); anchor paths stay inside the repository"),
        }
    }
    if parts.is_empty() {
        bail!("`{path}` names no file of the checkout");
    }
    Ok(parts.join("/"))
}

/// Which of the HEAD and index blobs (each with its text, when it is text) a patch to the basis
/// should start from, and that patch: the blob the basis already is, with an empty patch;
/// otherwise the one with the smaller line diff (HEAD on a tie, being the likelier to exist on
/// another machine); with neither, an empty file, so the patch is the whole basis. No patch when
/// the basis is not text or the patch would pass `PATCH_CAP`.
fn nearest_and_patch(
    path: &str,
    basis_blob: &str,
    basis_text: Option<&str>,
    candidates: [(Option<&str>, Option<&str>); 2],
) -> (Option<String>, Option<String>) {
    if let Some(blob) = candidates.iter().find_map(|(blob, _)| blob.filter(|b| *b == basis_blob)) {
        return (Some(blob.to_string()), Some(String::new()));
    }
    let Some(basis_text) = basis_text else { return (None, None) };
    let basis_lines: Vec<&str> = basis_text.split_inclusive('\n').collect();
    let mut best: Option<(usize, &str, &str)> = None;
    for (blob, text) in candidates {
        let (Some(blob), Some(text)) = (blob, text) else { continue };
        let size = diff_size(&text.split_inclusive('\n').collect::<Vec<_>>(), &basis_lines);
        if best.is_none_or(|(smallest, _, _)| size < smallest) {
            best = Some((size, blob, text));
        }
    }
    let (nearest, from) = match best {
        Some((_, blob, text)) => (Some(blob.to_string()), text),
        None => (None, ""),
    };
    let patch = unified_patch(path, from, basis_text);
    (nearest, (patch.len() <= PATCH_CAP).then_some(patch))
}

/// The lines `[start, end)` of the basis as maximal runs that stand the same way against HEAD
/// and the index (committed, staged only, or in neither) with each version's lines consecutive.
fn pieces_of(start: usize, end: usize, head: Option<&[Option<usize>]>, index: Option<&[Option<usize>]>) -> Vec<Piece> {
    let at = |map: Option<&[Option<usize>]>, line: usize| map.and_then(|m| m.get(line).copied().flatten());
    let continues = |run: Option<[i64; 2]>, next: Option<usize>| match (run, next) {
        (None, None) => true,
        (Some(run), Some(line)) => run[1] == line as i64,
        _ => false,
    };
    let mut pieces: Vec<Piece> = Vec::new();
    for line in start..end {
        let (h, x) = (at(head, line), at(index, line));
        match pieces.last_mut() {
            Some(p) if continues(p.head_lines, h) && continues(p.index_lines, x) => {
                p.basis_lines[1] += 1;
                for run in [&mut p.head_lines, &mut p.index_lines].into_iter().flatten() {
                    run[1] += 1;
                }
            }
            _ => pieces.push(Piece {
                basis_lines: [line as i64, line as i64 + 1],
                head_lines: h.map(|l| [l as i64, l as i64 + 1]),
                index_lines: x.map(|l| [l as i64, l as i64 + 1]),
            }),
        }
    }
    pieces
}

// ---------------------------------------------------------------------------------------------
// The basis, wherever it can be had

/// The basis text as this checkout can supply it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BasisText {
    /// The basis blob (or the first cut's HEAD blob) is in this object database.
    Available(String),
    /// Rebuilt from the nearest blob and the patch, and its id checked against the basis.
    Rebuilt(String),
    /// The anchor names a basis this checkout cannot supply or rebuild.
    Unavailable,
    /// An anchor of the first cut made on a file that differed from HEAD: it never had one.
    Unrecorded,
}

/// The basis text: the blob itself when this object database holds it, else the nearest blob
/// with the patch applied, trusted only when it hashes to the basis id.
pub fn basis_text(repo_root: &Path, spec: &AnchorSpec) -> BasisText {
    let text = |blob: &str| blob_bytes(repo_root, blob).ok().and_then(|b| String::from_utf8(b).ok());
    let Some(basis) = spec.basis_blob.as_deref() else {
        return match spec.legacy_blob.as_deref() {
            Some(blob) => text(blob).map(BasisText::Available).unwrap_or(BasisText::Unavailable),
            None => BasisText::Unrecorded,
        };
    };
    if blob_exists(repo_root, basis) {
        if let Some(t) = text(basis) {
            return BasisText::Available(t);
        }
    }
    let Some(patch) = spec.basis_patch.as_deref() else { return BasisText::Unavailable };
    let from = match spec.nearest_blob.as_deref() {
        Some(nearest) => match text(nearest) {
            Some(t) => t,
            None => return BasisText::Unavailable,
        },
        None => String::new(),
    };
    match apply_patch(&from, patch) {
        Ok(rebuilt) if hash_blob_content(repo_root, rebuilt.as_bytes()).ok().as_deref() == Some(basis) => {
            BasisText::Rebuilt(rebuilt)
        }
        _ => BasisText::Unavailable,
    }
}

// ---------------------------------------------------------------------------------------------
// Targets

/// The version an anchor is resolved against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Against {
    /// The file on disk in the checkout.
    Worktree,
    /// What the index holds for the file.
    Index,
    /// The file in a commit: `name` as asked (`HEAD`, a branch), `commit` its id.
    Commit { name: String, commit: String },
    /// A blob named by its id, read as the anchor's own file.
    Blob(String),
}

impl Against {
    /// `worktree`, `index`, a revision, or a blob id, as this checkout has it; `None` when it has
    /// no such commit or blob (a branch never fetched here), which is an `absent` answer.
    pub fn parse(repo_root: &Path, target: &str) -> Result<Option<Against>> {
        match target {
            "worktree" => return Ok(Some(Against::Worktree)),
            "index" => return Ok(Some(Against::Index)),
            _ => {}
        }
        if target.is_empty() || target.starts_with('-') {
            bail!("`{target}` is not a target: give worktree, index, HEAD, a branch or commit, or a blob id");
        }
        let peel = |kind: &str| {
            git(repo_root, &["rev-parse", "--verify", "--quiet", &format!("{target}^{{{kind}}}")])
                .ok()
                .map(|s| s.trim().to_string())
        };
        if let Some(commit) = peel("commit") {
            return Ok(Some(Against::Commit { name: target.to_string(), commit }));
        }
        Ok(peel("blob").map(Against::Blob))
    }

    /// The version as a sentence names it: "the working tree", "the index", "`HEAD`".
    fn named(&self) -> String {
        match self {
            Against::Worktree => "the working tree".into(),
            Against::Index => "the index".into(),
            Against::Commit { name, .. } => format!("`{name}`"),
            Against::Blob(blob) => format!("blob `{blob}`"),
        }
    }

    fn describe(&self, located: &Located) -> Target {
        let (kind, rev, commit) = match self {
            Against::Worktree => ("worktree", None, None),
            Against::Index => ("index", None, None),
            Against::Commit { name, commit } => ("commit", Some(name.clone()), Some(commit.clone())),
            Against::Blob(_) => ("blob", None, None),
        };
        Target { kind: kind.into(), rev, commit, blob: located.blob.clone(), path: located.path.clone() }
    }
}

/// True when this checkout has the target, so an anchor can be resolved against it here; an
/// error when the target is not a target's name at all.
pub fn has_target(repo_root: &Path, target: &str) -> Result<bool> {
    Ok(Against::parse(repo_root, target)?.is_some())
}

/// Where the anchor's file is at a target, and what it holds there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located {
    /// The anchor's path, or the path the file was renamed to.
    pub path: String,
    pub renamed: bool,
    /// The file's blob at the target; for the working tree, the id its content has, unwritten.
    pub blob: Option<String>,
    /// The file's content at the target; `None` when the file is not there.
    pub content: Option<Vec<u8>>,
}

/// Finds the anchor's file at the target: at its own path, else where a rename took it (a rename
/// committed in the target's history, a rename staged in the index, or for the working tree, an
/// untracked file whose content is one the anchor knew).
pub fn locate(repo_root: &Path, spec: &AnchorSpec, against: &Against) -> Result<Located> {
    let missing = || Located { path: spec.path.clone(), renamed: false, blob: None, content: None };
    // A stored path that could reach outside the checkout is never read; the anchor keeps its
    // snapshot and reports its file missing.
    if !stored_path_is_safe(&spec.path) {
        return Ok(missing());
    }
    if let Some((blob, content)) = content_at(repo_root, against, &spec.path)? {
        return Ok(Located { path: spec.path.clone(), renamed: false, blob, content: Some(content) });
    }
    if let Some(path) = renamed_to(repo_root, spec, against)?.filter(|p| stored_path_is_safe(p)) {
        if let Some((blob, content)) = content_at(repo_root, against, &path)? {
            return Ok(Located { path, renamed: true, blob, content: Some(content) });
        }
    }
    Ok(missing())
}

/// The file at `path` in the target: its blob id and content.
fn content_at(repo_root: &Path, against: &Against, path: &str) -> Result<Option<(Option<String>, Vec<u8>)>> {
    let from_blob = |blob: Option<String>| -> Result<Option<(Option<String>, Vec<u8>)>> {
        match blob {
            Some(blob) => Ok(Some((Some(blob.clone()), blob_bytes(repo_root, &blob)?))),
            None => Ok(None),
        }
    };
    match against {
        Against::Worktree => {
            let file = repo_root.join(path);
            if !file.is_file() {
                return Ok(None);
            }
            let content = std::fs::read(&file).with_context(|| format!("reading {path}"))?;
            // Without its id the file still resolves, by diff or search; only the same-blob
            // answer and the cache are lost.
            let blob = worktree_blob(repo_root, path, &content, false)
                .map_err(|e| tracing::warn!("finding the blob id of {path} in the working tree: {e}"))
                .ok();
            Ok(Some((blob, content)))
        }
        Against::Index => from_blob(index_blob(repo_root, path)?),
        Against::Commit { commit, .. } => from_blob(blob_at(repo_root, commit, path)?),
        Against::Blob(blob) => from_blob(Some(blob.clone())),
    }
}

/// Where the anchor's file went when it is not at its own path in the target.
fn renamed_to(repo_root: &Path, spec: &AnchorSpec, against: &Against) -> Result<Option<String>> {
    let path = spec.path.as_str();
    match against {
        Against::Blob(_) => Ok(None),
        Against::Commit { commit, .. } => renamed_in_history(repo_root, commit, path),
        Against::Index => {
            if let Some(to) = staged_rename(repo_root, path)? {
                return Ok(Some(to));
            }
            match renamed_in_history(repo_root, "HEAD", path)? {
                Some(to) if index_blob(repo_root, &to)?.is_some() => Ok(Some(to)),
                _ => Ok(None),
            }
        }
        Against::Worktree => {
            for to in
                [staged_rename(repo_root, path)?, renamed_in_history(repo_root, "HEAD", path)?].into_iter().flatten()
            {
                if repo_root.join(&to).is_file() {
                    return Ok(Some(to));
                }
            }
            untracked_copy(repo_root, spec)
        }
    }
}

/// The path `path` became in `rev`'s history: the last commit there that touched it, when that
/// commit renamed it, followed through up to `RENAME_HOPS` renames.
fn renamed_in_history(repo_root: &Path, rev: &str, path: &str) -> Result<Option<String>> {
    if !rev_exists(repo_root, rev) {
        return Ok(None);
    }
    let mut current = path.to_string();
    for _ in 0..RENAME_HOPS {
        if blob_at(repo_root, rev, &current)?.is_some() {
            return Ok((current != path).then_some(current));
        }
        let last = git(repo_root, &["log", "-1", "--format=%H", rev, "--", &current])?.trim().to_string();
        if last.is_empty() {
            return Ok(None);
        }
        // A root commit has no parent to diff against, so it renamed nothing.
        let Ok(renames) =
            git(repo_root, &["diff", "-M", "--name-status", "-z", "--diff-filter=R", &format!("{last}^"), &last])
        else {
            return Ok(None);
        };
        match rename_from(&renames, &current) {
            Some(to) => current = to,
            None => return Ok(None),
        }
    }
    Ok(None)
}

/// A rename of `path` staged in the index (HEAD against the index).
fn staged_rename(repo_root: &Path, path: &str) -> Result<Option<String>> {
    if head_commit(repo_root)?.is_none() {
        return Ok(None);
    }
    let renames = git(repo_root, &["diff", "--cached", "-M", "--name-status", "-z", "--diff-filter=R"])?;
    Ok(rename_from(&renames, path))
}

/// In `git diff --name-status -z` output, the new path of a rename from `old`.
fn rename_from(listing: &str, old: &str) -> Option<String> {
    let mut fields = listing.split('\0');
    while let Some(status) = fields.next() {
        if status.is_empty() {
            break;
        }
        if status.starts_with('R') || status.starts_with('C') {
            let (from, to) = (fields.next()?, fields.next()?);
            if status.starts_with('R') && from == old {
                return Some(to.to_string());
            }
        } else {
            fields.next();
        }
    }
    None
}

/// An untracked working-tree file whose content is the anchor's basis, or its HEAD or index blob
/// of the moment it was made: the file moved without git being told.
fn untracked_copy(repo_root: &Path, spec: &AnchorSpec) -> Result<Option<String>> {
    let known: Vec<&str> = [&spec.basis_blob, &spec.head_blob, &spec.index_blob, &spec.legacy_blob]
        .into_iter()
        .filter_map(|b| b.as_deref())
        .collect();
    if known.is_empty() {
        return Ok(None);
    }
    let listed = git(repo_root, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    let paths: Vec<&str> =
        listed.split('\0').filter(|p| !p.is_empty() && !p.contains('\n')).take(UNTRACKED_SCAN_LIMIT).collect();
    if paths.is_empty() {
        return Ok(None);
    }
    let ids = git_with_input(repo_root, &["hash-object", "--stdin-paths"], paths.join("\n").as_bytes())?;
    Ok(paths.iter().zip(ids.lines()).find(|(_, id)| known.contains(&id.trim())).map(|(path, _)| path.to_string()))
}

// ---------------------------------------------------------------------------------------------
// Resolution: three facts and a label

/// The first cut's single state, kept for readers that predate the three facts (the VS Code
/// extension reads `state`, `start_line`, `end_line` and `path`): the label, with `partial`
/// reported as `modified`, the first cut's word for lines that partly survive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display, strum::EnumString)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ResolutionState {
    Exact,
    Shifted,
    Modified,
    Moved,
    Recovered,
    Orphaned,
    Absent,
}

/// The three facts summed up in one word, for lists, glyphs and hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display, strum::EnumString)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum Label {
    /// The same text at the same lines (for a whole file: the same content at the same path).
    Exact,
    /// The same text, at other lines.
    Shifted,
    /// All the lines are there, with other text among or in them (for a whole file: its
    /// content changed).
    Modified,
    /// Found elsewhere by its text, or in a renamed file.
    Moved,
    /// A likely place found by similarity, not a certain one.
    Recovered,
    /// Some of the lines are there and some are not.
    Partial,
    /// Nothing of it is there; the anchor keeps its identity, its snapshot and its messages.
    Orphaned,
    /// Nothing was compared: no checkout here has the target.
    Absent,
}

impl Label {
    fn first_cut(self) -> ResolutionState {
        match self {
            Label::Exact => ResolutionState::Exact,
            Label::Shifted => ResolutionState::Shifted,
            Label::Modified | Label::Partial => ResolutionState::Modified,
            Label::Moved => ResolutionState::Moved,
            Label::Recovered => ResolutionState::Recovered,
            Label::Orphaned => ResolutionState::Orphaned,
            Label::Absent => ResolutionState::Absent,
        }
    }
}

/// Whether we know where the lines (or the file) are in the target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum Location {
    /// At the anchor's own lines; for a whole-file anchor, at its own path.
    Exact,
    /// At other lines, where following the edits between the basis and the target put them.
    Shifted,
    /// Elsewhere, found by searching for the text; or in the file the anchor's file was renamed to.
    Moved,
    /// A likely place, found by similarity.
    Recovered,
    /// Not found, or not looked for.
    Unknown,
}

/// Whether the text at the resolved place is the anchored text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Content {
    /// The anchored text (for a whole file: the basis blob itself).
    Same,
    /// Different text: `similarity` from 0 to 1 (absent for a whole file), and the resolved lines
    /// (0-based, in the target) that are not the anchored lines.
    Changed { similarity: Option<f32>, changed_lines: Vec<i64> },
    /// None of the anchored text is there.
    Gone,
    /// Nothing was compared.
    Unknown,
}

/// Whether the anchored lines (or the file) are in the target at all.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Availability {
    Present,
    /// Some lines are there and some are not, run by run.
    Partial {
        pieces: Vec<PieceAt>,
    },
    Absent {
        reason: AbsentReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum AbsentReason {
    /// The anchored text is gone from the target; for a whole-file anchor, the file is.
    Deleted,
    /// The anchored lines' file is not in the target, and no rename of it was found.
    FileMissing,
    /// The target is the HEAD or index version the anchor was made beside, older than its basis,
    /// which never held these lines: they were staged or only in the working tree then.
    BeforeCreation,
    /// No checkout on this machine has the target, so nothing was compared.
    NotInCheckout,
}

/// A run of the anchored lines in a partial answer: where it is in the target, if anywhere.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PieceAt {
    /// 0-based, end exclusive, in the basis.
    pub basis_lines: [i64; 2],
    /// The same run in the target; `None` when it is not there.
    pub target_lines: Option<[i64; 2]>,
    /// Where the run stood when the anchor was made (`head`, `index`, `worktree` or `revision`,
    /// from its pieces: `Piece::stage`); `None` for an anchor made before pieces existed.
    #[serde(rename = "in")]
    pub stood: Option<String>,
}

/// How the answer was reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum Method {
    /// The target is the basis itself.
    SameBlob,
    /// The lines were followed through a line diff from the basis to the target.
    Diff,
    /// The target was searched for the anchored text (a moved block, or a similar window).
    Fingerprint,
    /// A whole-file anchor, or a file that is not there: the file's presence and blob only.
    File,
    /// Nothing was compared.
    NotCompared,
}

/// Where the basis text came from, when the answer needed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum BasisState {
    /// Read from this object database.
    Available,
    /// Rebuilt from the nearest blob and the patch, its id checked.
    Rebuilt,
    /// The anchor has a basis this checkout cannot supply: the answer comes from searching for
    /// the text, and never claims more than `recovered`.
    Unavailable,
    /// An anchor of the first cut made on a file that differed from HEAD, which recorded none: the
    /// answer comes from searching for the text, as it always did.
    Unrecorded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineRange {
    /// 0-based, inclusive.
    pub start_line: i64,
    /// 0-based, exclusive.
    pub end_line: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hint {
    /// Where the lines were last known to be: the anchor's own first line.
    pub last_seen_line: Option<i64>,
    /// How many places hold the anchored text equally well, when more than one does.
    pub ambiguity: u32,
}

/// The version resolved against, as found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    /// `worktree`, `index`, `commit` or `blob`.
    pub kind: String,
    /// For a commit: the name it was asked by (`HEAD`, a branch).
    pub rev: Option<String>,
    pub commit: Option<String>,
    /// The file's blob there; for the working tree, the id its content has.
    pub blob: Option<String>,
    /// Where the file is there.
    pub path: String,
}

/// Where an anchor is in a target. The first six fields are the first cut's and keep their
/// meaning for the readers written against it; the rest are the three facts, apart, and what
/// they come to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Resolution {
    pub state: ResolutionState,
    /// Where the file is in the target (the renamed path when it moved).
    pub path: String,
    /// The resolved lines; the anchor's own when nothing was found; 0 and 0 for a whole file.
    pub start_line: i64,
    pub end_line: i64,
    /// 1.0 for exact and shifted; lower for modified, partial, moved and recovered; 0 when
    /// nothing was found.
    pub confidence: f32,
    pub at: i64,
    pub label: Label,
    pub location: Location,
    pub content: Content,
    pub availability: Availability,
    /// The resolved lines, when there are any.
    pub range: Option<LineRange>,
    pub method: Method,
    pub basis: Option<BasisState>,
    pub hint: Option<Hint>,
    pub whole_file: bool,
    /// Filled by the caller, which knows how the target was asked for.
    pub target: Option<Target>,
}

impl Resolution {
    /// Whether this answer may be kept for the same anchor, file path and target blob: not one
    /// made without the basis, which a later fetch or pin could improve.
    pub fn cacheable(&self) -> bool {
        !matches!(self.basis, Some(BasisState::Unavailable))
            && !matches!(self.availability, Availability::Absent { reason: AbsentReason::NotInCheckout })
    }
}

/// The facts one strategy establishes, before the label and the first cut's fields are derived.
struct Facts {
    location: Location,
    content: Content,
    availability: Availability,
    range: Option<(i64, i64)>,
    method: Method,
    basis: Option<BasisState>,
    hint: Option<Hint>,
    confidence: f32,
    path: String,
}

fn label_of(location: Location, content: &Content, availability: &Availability) -> Label {
    match availability {
        Availability::Absent { reason: AbsentReason::NotInCheckout } => Label::Absent,
        Availability::Absent { .. } => Label::Orphaned,
        Availability::Partial { .. } => Label::Partial,
        Availability::Present => match location {
            Location::Moved => Label::Moved,
            Location::Recovered => Label::Recovered,
            Location::Unknown => Label::Orphaned,
            Location::Exact | Location::Shifted if matches!(content, Content::Changed { .. }) => Label::Modified,
            Location::Shifted => Label::Shifted,
            Location::Exact => Label::Exact,
        },
    }
}

fn finish(spec: &AnchorSpec, facts: Facts) -> Resolution {
    let label = label_of(facts.location, &facts.content, &facts.availability);
    let (start_line, end_line) = facts.range.or(spec.lines).unwrap_or((0, 0));
    Resolution {
        state: label.first_cut(),
        path: facts.path,
        start_line,
        end_line,
        confidence: facts.confidence,
        at: crate::now_ms(),
        label,
        location: facts.location,
        content: facts.content,
        availability: facts.availability,
        range: facts.range.map(|(start_line, end_line)| LineRange { start_line, end_line }),
        method: facts.method,
        basis: facts.basis,
        hint: facts.hint,
        whole_file: spec.whole_file(),
        target: None,
    }
}

/// Resolves against a target named as `Against::parse` reads it, at the anchor's own path or
/// where a rename took the file. A target this checkout lacks is an `absent` answer.
pub fn resolve_against(repo_root: &Path, spec: &AnchorSpec, target: &str) -> Result<Resolution> {
    let Some(against) = Against::parse(repo_root, target)? else { return Ok(absent(spec)) };
    let located = locate(repo_root, spec, &against)?;
    let mut resolution = resolve_located(repo_root, spec, &located);
    resolution.target = Some(against.describe(&located));
    Ok(resolution)
}

/// Describes the target for a resolution the caller computed or took from its cache.
pub fn target_of(against: &Against, located: &Located) -> Target {
    against.describe(located)
}

/// Where the anchor is in the located file: a whole-file anchor by the file's presence and blob
/// alone, a line anchor by the cascade in `span_facts`.
pub fn resolve_located(repo_root: &Path, spec: &AnchorSpec, located: &Located) -> Resolution {
    let facts = match spec.lines {
        None => file_facts(spec, located),
        Some(lines) => span_facts(repo_root, spec, lines, located),
    };
    finish(spec, facts)
}

/// The answer when no checkout on this machine can show the target: nothing compared, the
/// anchor's own lines and text standing (never cached; it is knowledge about this machine).
pub fn absent(spec: &AnchorSpec) -> Resolution {
    finish(
        spec,
        Facts {
            location: Location::Unknown,
            content: Content::Unknown,
            availability: Availability::Absent { reason: AbsentReason::NotInCheckout },
            range: None,
            method: Method::NotCompared,
            basis: None,
            hint: None,
            confidence: 0.0,
            path: spec.path.clone(),
        },
    )
}

/// A whole-file anchor: present at its path or a renamed one, or gone; its content the same when
/// the blob is the basis. No line is claimed.
fn file_facts(spec: &AnchorSpec, located: &Located) -> Facts {
    if located.content.is_none() {
        return Facts {
            location: Location::Unknown,
            content: Content::Gone,
            availability: Availability::Absent { reason: AbsentReason::Deleted },
            range: None,
            method: Method::File,
            basis: None,
            hint: None,
            confidence: 0.0,
            path: spec.path.clone(),
        };
    }
    let same = located.blob.is_some() && located.blob.as_deref() == spec.numbered_in();
    Facts {
        location: if located.renamed { Location::Moved } else { Location::Exact },
        content: if same { Content::Same } else { Content::Changed { similarity: None, changed_lines: Vec::new() } },
        availability: Availability::Present,
        range: None,
        method: if same { Method::SameBlob } else { Method::File },
        basis: None,
        hint: None,
        confidence: 1.0,
        path: located.path.clone(),
    }
}

/// A line anchor, cheap first: the same blob; else the lines followed through a diff from the
/// basis; else a search for the text. After any strategy the text at the resolved lines is
/// compared with the anchored text, and a difference is reported as changed content.
fn span_facts(repo_root: &Path, spec: &AnchorSpec, (start, end): (i64, i64), located: &Located) -> Facts {
    let Some(content) = &located.content else {
        return Facts {
            location: Location::Unknown,
            content: Content::Gone,
            availability: Availability::Absent { reason: AbsentReason::FileMissing },
            range: None,
            method: Method::File,
            basis: None,
            hint: Some(Hint { last_seen_line: Some(start), ambiguity: 0 }),
            confidence: 0.0,
            path: spec.path.clone(),
        };
    };
    let text = String::from_utf8_lossy(content);
    let target = lines(&text);
    let search = Search { spec, target: &target, start, end, renamed: located.renamed, path: &located.path };
    if located.blob.is_some() && located.blob.as_deref() == spec.numbered_in() {
        return search.at((start, end), Method::SameBlob, None, 1.0);
    }
    // A HEAD or index version the anchor was made beside, older than its basis: the diff from the
    // basis is the whole truth about it, so no search second-guesses a partial or empty answer.
    let beside = !located.renamed && located.blob.as_deref().is_some_and(|blob| spec.older_versions().contains(&blob));
    match basis_text(repo_root, spec) {
        BasisText::Available(basis) => search.by_diff(&basis, BasisState::Available, beside),
        BasisText::Rebuilt(basis) => search.by_diff(&basis, BasisState::Rebuilt, beside),
        BasisText::Unavailable => search.by_text(BasisState::Unavailable),
        BasisText::Unrecorded => search.by_text(BasisState::Unrecorded),
    }
}

/// One line anchor's search in one target text.
struct Search<'a> {
    spec: &'a AnchorSpec,
    target: &'a [&'a str],
    start: i64,
    end: i64,
    renamed: bool,
    path: &'a str,
}

impl Search<'_> {
    fn len(&self) -> i64 {
        self.end - self.start
    }

    /// Where the resolved lines are, as a location: at the anchor's own lines, elsewhere, or in
    /// a renamed file.
    fn location(&self, range: (i64, i64)) -> Location {
        if self.renamed {
            Location::Moved
        } else if range == (self.start, self.end) {
            Location::Exact
        } else {
            Location::Shifted
        }
    }

    /// Every line at `range` is an anchored line in order: present, with the text compared.
    fn at(&self, range: (i64, i64), method: Method, basis: Option<BasisState>, confidence: f32) -> Facts {
        Facts {
            location: self.location(range),
            content: self.content(range, &[]),
            availability: Availability::Present,
            range: Some(range),
            method,
            basis,
            hint: None,
            confidence,
            path: self.path.to_string(),
        }
    }

    /// The text at `range` against the anchored text; `kept` are the target lines known to be
    /// anchored lines, so the others in the range are the changed ones.
    fn content(&self, (first, last): (i64, i64), kept: &[i64]) -> Content {
        let found = slice(self.target, first, last);
        if found == self.spec.anchored_text {
            return Content::Same;
        }
        let changed_lines = if kept.is_empty() {
            let anchored = lines(&self.spec.anchored_text);
            let here: Vec<&str> = self.target[first as usize..(last as usize).min(self.target.len())].to_vec();
            let same = line_map(&here, &anchored);
            same.iter().enumerate().filter(|(_, m)| m.is_none()).map(|(i, _)| first + i as i64).collect()
        } else {
            (first..last).filter(|l| !kept.contains(l)).collect()
        };
        Content::Changed {
            similarity: Some(TextDiff::from_lines(self.spec.anchored_text.as_str(), found.as_str()).ratio()),
            changed_lines,
        }
    }

    fn gone(&self, reason: AbsentReason, basis: Option<BasisState>) -> Facts {
        Facts {
            location: Location::Unknown,
            content: Content::Gone,
            availability: Availability::Absent { reason },
            range: None,
            method: Method::Diff,
            basis,
            hint: Some(Hint { last_seen_line: Some(self.start), ambiguity: 0 }),
            confidence: 0.0,
            path: self.path.to_string(),
        }
    }

    /// Follows the anchored lines through a line diff from the basis. All of them surviving in
    /// order is a present answer; some surviving is partial (or, with every line there but
    /// others among them, modified); too few, far from the basis version, are only a candidate
    /// that a search for the text may beat.
    fn by_diff(&self, basis: &str, state: BasisState, beside: bool) -> Facts {
        let basis_lines = lines(basis);
        let map = line_map(&basis_lines, self.target);
        let span = self.start as usize..(self.end as usize).min(map.len());
        let kept: Vec<(usize, usize)> = span.clone().filter_map(|i| map[i].map(|t| (i, t))).collect();
        let mut candidate = None;
        if let (Some(&(_, first)), Some(&(_, last))) = (kept.first(), kept.last()) {
            let range = (first as i64, last as i64 + 1);
            let every = kept.len() as i64 == self.len();
            let contiguous = kept.windows(2).all(|w| w[1].1 == w[0].1 + 1);
            if every && contiguous {
                return self.at(range, Method::Diff, Some(state), 1.0);
            }
            let coverage = kept.len() as f32 / self.len().max(1) as f32;
            let followed = self.followed(range, &kept, &map, every, coverage, state);
            if beside || coverage >= 0.5 {
                return followed;
            }
            candidate = Some(followed);
        }
        if beside && candidate.is_none() {
            return self.gone(AbsentReason::BeforeCreation, Some(state));
        }
        if let Some(found) = self.search(Some(state)) {
            return found;
        }
        candidate.unwrap_or_else(|| self.gone(AbsentReason::Deleted, Some(state)))
    }

    /// The answer when some anchored lines were followed to `range`: modified when every line
    /// is there with others among them, partial when some are missing, run by run.
    fn followed(
        &self,
        range: (i64, i64),
        kept: &[(usize, usize)],
        map: &[Option<usize>],
        every: bool,
        coverage: f32,
        state: BasisState,
    ) -> Facts {
        let kept_lines: Vec<i64> = kept.iter().map(|&(_, t)| t as i64).collect();
        let availability =
            if every { Availability::Present } else { Availability::Partial { pieces: self.pieces_at(map) } };
        // The lines that survive are where the anchor put them when each is at its own number,
        // however the range around them was cut or widened.
        let location = if self.renamed {
            Location::Moved
        } else if kept.iter().all(|&(basis, target)| basis == target) {
            Location::Exact
        } else {
            Location::Shifted
        };
        Facts {
            location,
            content: self.content(range, &kept_lines),
            availability,
            range: Some(range),
            method: Method::Diff,
            basis: Some(state),
            hint: None,
            confidence: coverage,
            path: self.path.to_string(),
        }
    }

    /// The anchored lines as runs of the same standing when the anchor was made (its pieces) and
    /// the same fate in the target, each with its target lines when it survived.
    fn pieces_at(&self, map: &[Option<usize>]) -> Vec<PieceAt> {
        let stood = |line: i64| {
            self.spec
                .pieces
                .iter()
                .find(|p| p.basis_lines[0] <= line && line < p.basis_lines[1])
                .map(|p| p.stage(self.spec.side).to_string())
        };
        let mut runs: Vec<PieceAt> = Vec::new();
        for line in self.start..self.end {
            let target = map.get(line as usize).copied().flatten().map(|t| t as i64);
            let here = stood(line);
            match runs.last_mut() {
                Some(run)
                    if run.stood == here
                        && match (run.target_lines, target) {
                            (None, None) => true,
                            (Some(r), Some(t)) => r[1] == t,
                            _ => false,
                        } =>
                {
                    run.basis_lines[1] += 1;
                    if let Some(r) = run.target_lines.as_mut() {
                        r[1] += 1;
                    }
                }
                _ => runs.push(PieceAt {
                    basis_lines: [line, line + 1],
                    target_lines: target.map(|t| [t, t + 1]),
                    stood: here,
                }),
            }
        }
        runs
    }

    /// Without the basis: a search for the text alone, as the first cut always did.
    fn by_text(&self, state: BasisState) -> Facts {
        self.search(Some(state))
            .unwrap_or_else(|| Facts { method: Method::Fingerprint, ..self.gone(AbsentReason::Deleted, Some(state)) })
    }

    /// The anchored text as a whole block (the place nearest the anchor's own lines when it occurs
    /// more than once, which is then only `recovered`), else the most similar window of the same
    /// length with the context as a tie-breaker. With a basis this checkout cannot supply, no
    /// location better than `recovered` is claimed.
    fn search(&self, state: Option<BasisState>) -> Option<Facts> {
        let len = self.len();
        if len <= 0 || (self.target.len() as i64) < len {
            return None;
        }
        let unsure = state == Some(BasisState::Unavailable);
        let starts = 0..=(self.target.len() as i64 - len);
        let blocks: Vec<i64> =
            starts.clone().filter(|&s| slice(self.target, s, s + len) == self.spec.anchored_text).collect();
        if let Some(&nearest) = blocks.iter().min_by_key(|&&s| (s - self.start).unsigned_abs()) {
            let range = (nearest, nearest + len);
            let ambiguity = blocks.len() as u32;
            let location = if unsure || (ambiguity > 1 && range != (self.start, self.end)) {
                Location::Recovered
            } else if self.renamed || range != (self.start, self.end) {
                Location::Moved
            } else {
                Location::Exact
            };
            let confidence = match location {
                Location::Exact => 1.0,
                Location::Moved => 0.9,
                _ => 0.8,
            };
            return Some(Facts {
                location,
                content: Content::Same,
                availability: Availability::Present,
                range: Some(range),
                method: Method::Fingerprint,
                basis: state,
                hint: (ambiguity > 1).then_some(Hint { last_seen_line: Some(self.start), ambiguity }),
                confidence,
                path: self.path.to_string(),
            });
        }
        let context = CONTEXT_LINES as i64;
        let mut best: Option<(f32, i64)> = None;
        for s in starts {
            let window = slice(self.target, s, s + len);
            let mut score = TextDiff::from_chars(self.spec.anchored_text.as_str(), window.as_str()).ratio();
            if !self.spec.context_before.is_empty() && slice(self.target, s - context, s) == self.spec.context_before {
                score += 0.05;
            }
            if !self.spec.context_after.is_empty()
                && slice(self.target, s + len, s + len + context) == self.spec.context_after
            {
                score += 0.05;
            }
            score -= ((s - self.start).unsigned_abs() as f32 / 10_000.0).min(0.02);
            if best.is_none_or(|(b, _)| score > b) {
                best = Some((score, s));
            }
        }
        let (score, s) = best.filter(|(score, _)| *score >= FUZZY_THRESHOLD)?;
        let range = (s, s + len);
        let content = match self.content(range, &[]) {
            Content::Changed { changed_lines, .. } => {
                Content::Changed { similarity: Some(score.min(1.0)), changed_lines }
            }
            same => same,
        };
        Some(Facts {
            location: Location::Recovered,
            content,
            availability: Availability::Present,
            range: Some(range),
            method: Method::Fingerprint,
            basis: state,
            hint: None,
            confidence: score.min(0.89),
            path: self.path.to_string(),
        })
    }
}

// ---------------------------------------------------------------------------------------------
// Anchors of the first cut

/// What an anchor made before content-addressed bases can gain from this checkout: a basis to
/// resolve by diff from, the nearest blob and patch that rebuild it elsewhere, and its text's
/// fingerprint. The caller adds the key, which needs the repository's entity id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backfill {
    pub basis_blob: String,
    pub nearest_blob: Option<String>,
    pub basis_patch: Option<String>,
    pub span_key: String,
}

/// The basis an anchor of the first cut can take here, if any: the HEAD blob it was mapped onto
/// when this object database holds it; else, for one made on a file that differed from HEAD, a
/// version in which its text and both context blocks stand exactly at its lines (the answer just
/// given was `exact`), which then is the content its lines count. The blob is pinned.
pub fn backfill(
    repo_root: &Path,
    spec: &AnchorSpec,
    against: &Against,
    located: &Located,
    resolution: &Resolution,
) -> Result<Option<Backfill>> {
    let Some((start, end)) = spec.lines else { return Ok(None) };
    if spec.basis_blob.is_some() {
        return Ok(None);
    }
    let basis = match spec.legacy_blob.as_deref() {
        Some(blob) if blob_exists(repo_root, blob) => blob.to_string(),
        Some(_) => return Ok(None),
        None => {
            let (Some(content), Some(blob)) = (&located.content, &located.blob) else { return Ok(None) };
            if located.renamed || resolution.label != Label::Exact {
                return Ok(None);
            }
            let text = String::from_utf8_lossy(content);
            let all = lines(&text);
            let context = CONTEXT_LINES as i64;
            if slice(&all, start - context, start) != spec.context_before
                || slice(&all, end, end + context) != spec.context_after
            {
                return Ok(None);
            }
            // The working tree's id was computed without writing the object; write it now.
            if *against == Against::Worktree && worktree_blob(repo_root, &located.path, content, true)? != *blob {
                return Ok(None);
            }
            blob.clone()
        }
    };
    pin(repo_root, &basis)?;
    let text_of = |blob: &str| -> Result<Option<String>> { Ok(String::from_utf8(blob_bytes(repo_root, blob)?).ok()) };
    let basis_text = text_of(&basis)?;
    let head = blob_at(repo_root, "HEAD", &spec.path)?;
    let index = index_blob(repo_root, &spec.path)?;
    let head_text = head.as_deref().map(text_of).transpose()?.flatten();
    let index_text = index.as_deref().map(text_of).transpose()?.flatten();
    let (nearest_blob, basis_patch) = nearest_and_patch(
        &spec.path,
        &basis,
        basis_text.as_deref(),
        [(head.as_deref(), head_text.as_deref()), (index.as_deref(), index_text.as_deref())],
    );
    Ok(Some(Backfill { basis_blob: basis, nearest_blob, basis_patch, span_key: span_key(&spec.anchored_text) }))
}

pub fn parse_lines(arg: &str) -> Result<(i64, i64)> {
    // Human form: 1-based, inclusive ("120-134" or "120"); stored 0-based, exclusive.
    let (a, b) = match arg.split_once('-') {
        Some((a, b)) => (a.trim().parse::<i64>()?, b.trim().parse::<i64>()?),
        None => {
            let n = arg.trim().parse::<i64>()?;
            (n, n)
        }
    };
    if a < 1 || b < a {
        return Err(anyhow!("lines must be like `120-134` (1-based, inclusive)"));
    }
    Ok((a - 1, b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn repo() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        init(&root);
        (dir, root)
    }

    fn init(root: &Path) {
        fs::create_dir_all(root).unwrap();
        git(root, &["init", "-q", "-b", "main"]).unwrap();
        git(root, &["config", "user.email", "t@example.com"]).unwrap();
        git(root, &["config", "user.name", "t"]).unwrap();
    }

    fn commit(root: &Path, path: &str, text: &str) -> String {
        fs::write(root.join(path), text).unwrap();
        git(root, &["add", path]).unwrap();
        git(root, &["commit", "-q", "-m", "c"]).unwrap();
        head_commit(root).unwrap().unwrap()
    }

    /// A clone that fetched only what branches reach, as a teammate's machine has it: none of the
    /// objects written for anchors, and none of the pins.
    fn fresh_clone(origin: &Path) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("clone");
        let url = format!("file://{}", origin.display());
        git(dir.path(), &["clone", "-q", "--no-local", &url, "clone"]).unwrap();
        (dir, root)
    }

    /// Two answers alike in everything but when they were made.
    fn same_answer(a: &Resolution, b: &Resolution) -> bool {
        Resolution { at: 0, ..a.clone() } == Resolution { at: 0, ..b.clone() }
    }

    const V1: &str = "a\nb\nc\nfn reconnect() {\n    loop {\n        try_once();\n    }\n}\nx\ny\nz\n";

    /// Twenty-four distinct lines: `line 1` to `line 24`.
    fn numbered() -> String {
        (1..=24).map(|n| format!("line {n}\n")).collect()
    }

    #[test]
    fn a_clean_selection_has_the_head_blob_as_its_basis() {
        let (_d, root) = repo();
        let c = commit(&root, "ws.rs", V1);
        let spec = form(&root, "ws.rs", 3, 8).unwrap();
        let head = blob_at(&root, "HEAD", "ws.rs").unwrap();
        assert_eq!(spec.basis_blob, head);
        assert_eq!(spec.index_blob, head);
        assert_eq!((spec.nearest_blob.clone(), spec.basis_patch.as_deref()), (head.clone(), Some("")));
        assert_eq!(spec.commit_sha.as_deref(), Some(c.as_str()));
        assert_eq!(spec.anchored_text, "fn reconnect() {\n    loop {\n        try_once();\n    }\n}");
        assert_eq!(spec.context_before, "a\nb\nc");
        assert_eq!(spec.context_after, "x\ny\nz");
        assert_eq!(
            spec.pieces,
            vec![Piece { basis_lines: [3, 8], head_lines: Some([3, 8]), index_lines: Some([3, 8]) }]
        );
        let pinned = git(&root, &["rev-parse", &format!("refs/fwd/basis/{}", head.unwrap())]).unwrap();
        assert_eq!(Some(pinned.trim().to_string()), spec.basis_blob, "the basis is pinned by a ref");
    }

    #[test]
    fn a_dirty_selection_has_its_own_basis_and_its_changed_line_is_a_worktree_piece() {
        let (_d, root) = repo();
        commit(&root, "ws.rs", V1);
        let dirty = V1.replace("try_once();", "try_once().await?;");
        fs::write(root.join("ws.rs"), &dirty).unwrap();
        let spec = form(&root, "ws.rs", 3, 8).unwrap();
        let head = blob_at(&root, "HEAD", "ws.rs").unwrap();
        let basis = spec.basis_blob.clone().unwrap();
        assert_ne!(Some(&basis), head.as_ref());
        assert_eq!(spec.index_blob, head, "nothing is staged");
        assert!(blob_exists(&root, &basis), "the content seen is written to the repository");
        assert_eq!(blob_text(&root, &basis).unwrap(), dirty);
        assert_eq!(spec.nearest_blob, head);
        assert!(spec.basis_patch.as_deref().is_some_and(|p| p.contains("+        try_once().await?;")));
        let stages: Vec<(&str, [i64; 2])> = spec.pieces.iter().map(|p| (p.stage(spec.side), p.basis_lines)).collect();
        assert_eq!(stages, vec![("head", [3, 5]), ("worktree", [5, 6]), ("head", [6, 8])]);
        // Lines above the edit share the file's basis: the basis is the whole file as seen.
        let above = form(&root, "ws.rs", 0, 2).unwrap();
        assert_eq!(above.basis_blob.as_deref(), Some(basis.as_str()));
        assert_eq!(above.pieces.len(), 1);
    }

    #[test]
    fn resolution_cascade() {
        let (_d, root) = repo();
        commit(&root, "ws.rs", V1);
        let spec = form(&root, "ws.rs", 3, 8).unwrap();

        // Exact: same file.
        let r = resolve_against(&root, &spec, "HEAD").unwrap();
        assert_eq!((r.state, r.start_line, r.end_line), (ResolutionState::Exact, 3, 8));

        // Shifted: two lines inserted above.
        commit(&root, "ws.rs", &format!("new1\nnew2\n{V1}"));
        let r = resolve_against(&root, &spec, "HEAD").unwrap();
        assert_eq!((r.state, r.start_line, r.end_line), (ResolutionState::Shifted, 5, 10));

        // Modified: one line inside the range changed.
        commit(&root, "ws.rs", &V1.replace("try_once();", "try_once().await?;"));
        let r = resolve_against(&root, &spec, "HEAD").unwrap();
        assert_eq!(r.state, ResolutionState::Modified);
        assert_eq!((r.start_line, r.end_line), (3, 8));
        assert!(r.confidence > 0.5 && r.confidence < 1.0);

        // Moved: the whole block relocated below other code, with the original lines gone.
        commit(&root, "ws.rs", "a\nb\nc\nx\ny\nz\nfn reconnect() {\n    loop {\n        try_once();\n    }\n}\n");
        let r = resolve_against(&root, &spec, "HEAD").unwrap();
        assert!(matches!(r.state, ResolutionState::Moved | ResolutionState::Shifted), "{r:?}");
        assert_eq!((r.start_line, r.end_line), (6, 11));

        // Recovered: reformatted, nothing identical line by line.
        commit(&root, "ws.rs", "a\nb\nc\nfn reconnect() -> R {\n  loop {\n    try_once()?;\n  }\n}\nx\ny\nz\n");
        let r = resolve_against(&root, &spec, "HEAD").unwrap();
        assert!(matches!(r.state, ResolutionState::Recovered | ResolutionState::Modified), "{r:?}");
        assert_eq!((r.start_line, r.end_line), (3, 8));

        // Orphaned: the function is gone.
        commit(&root, "ws.rs", "a\nb\nc\nx\ny\nz\nfn something_else() { 1 }\n");
        let r = resolve_against(&root, &spec, "HEAD").unwrap();
        assert_eq!(r.state, ResolutionState::Orphaned);

        // Missing file.
        git(&root, &["rm", "-q", "ws.rs"]).unwrap();
        git(&root, &["commit", "-q", "-m", "rm"]).unwrap();
        let r = resolve_against(&root, &spec, "HEAD").unwrap();
        assert_eq!(r.state, ResolutionState::Orphaned);
    }

    #[test]
    fn the_cascade_states_three_facts_apart() {
        let (_d, root) = repo();
        commit(&root, "ws.rs", V1);
        let spec = form(&root, "ws.rs", 3, 8).unwrap();

        let r = resolve_against(&root, &spec, "HEAD").unwrap();
        assert_eq!((r.label, r.location, r.method), (Label::Exact, Location::Exact, Method::SameBlob));
        assert_eq!((r.content.clone(), r.availability.clone()), (Content::Same, Availability::Present));
        assert_eq!(r.range, Some(LineRange { start_line: 3, end_line: 8 }));

        commit(&root, "ws.rs", &format!("new1\nnew2\n{V1}"));
        let r = resolve_against(&root, &spec, "HEAD").unwrap();
        assert_eq!((r.label, r.location, r.method), (Label::Shifted, Location::Shifted, Method::Diff));
        assert_eq!((r.content, r.basis), (Content::Same, Some(BasisState::Available)));

        // A line edited inside the span: the old line is gone, so the span is partly there.
        commit(&root, "ws.rs", &V1.replace("try_once();", "try_once().await?;"));
        let r = resolve_against(&root, &spec, "HEAD").unwrap();
        assert_eq!((r.label, r.state), (Label::Partial, ResolutionState::Modified));
        let Content::Changed { changed_lines, similarity } = &r.content else { panic!("{r:?}") };
        assert_eq!(changed_lines, &vec![5]);
        assert!(similarity.is_some_and(|s| s > 0.5 && s < 1.0));
        let Availability::Partial { pieces } = &r.availability else { panic!("{r:?}") };
        let fates: Vec<Option<[i64; 2]>> = pieces.iter().map(|p| p.target_lines).collect();
        assert_eq!(fates, vec![Some([3, 5]), None, Some([6, 8])]);

        // A line inserted inside the span: every anchored line is there, with another among them.
        commit(&root, "ws.rs", &V1.replace("        try_once();\n", "        try_once();\n        log();\n"));
        let r = resolve_against(&root, &spec, "HEAD").unwrap();
        assert_eq!((r.label, r.availability.clone()), (Label::Modified, Availability::Present));
        assert_eq!(r.range, Some(LineRange { start_line: 3, end_line: 9 }));

        // Gone: nothing found, the anchor's own lines kept as where it was last seen.
        commit(&root, "ws.rs", "a\nb\nc\nx\ny\nz\nfn something_else() { 1 }\n");
        let r = resolve_against(&root, &spec, "HEAD").unwrap();
        assert_eq!((r.label, r.location, r.content.clone()), (Label::Orphaned, Location::Unknown, Content::Gone));
        assert_eq!(r.availability, Availability::Absent { reason: AbsentReason::Deleted });
        assert_eq!(r.hint.and_then(|h| h.last_seen_line), Some(3));

        // A target this checkout does not have: nothing compared.
        let r = resolve_against(&root, &spec, "no-such-branch").unwrap();
        assert_eq!((r.label, r.state, r.content), (Label::Absent, ResolutionState::Absent, Content::Unknown));
    }

    /// An anchor as the first cut stored one made on a dirty file: no basis, only its text.
    fn first_cut(path: &str, lines: (i64, i64), text: &str, before: &str, after: &str) -> AnchorSpec {
        AnchorSpec {
            path: path.into(),
            lines: Some(lines),
            basis_blob: None,
            head_blob: None,
            index_blob: None,
            nearest_blob: None,
            basis_patch: None,
            pieces: Vec::new(),
            commit_sha: None,
            legacy_blob: None,
            anchored_text: text.into(),
            context_before: before.into(),
            context_after: after.into(),
            side: DiffSide::Worktree,
        }
    }

    #[test]
    fn an_anchor_of_the_first_cut_without_a_basis_resolves_by_text_only() {
        let (_d, root) = repo();
        commit(&root, "ws.rs", V1);
        let text = format!("{V1}fn added() {{}}\n");
        fs::write(root.join("ws.rs"), &text).unwrap();
        let spec = first_cut("ws.rs", (11, 12), "fn added() {}", "x\ny\nz", "");
        let r = resolve_against(&root, &spec, "worktree").unwrap();
        assert_eq!((r.state, r.basis), (ResolutionState::Exact, Some(BasisState::Unrecorded)));
        let r = resolve_against(&root, &spec, "HEAD").unwrap();
        assert_eq!(r.state, ResolutionState::Orphaned, "not in HEAD yet");
        commit(&root, "ws.rs", &format!("first\n{text}"));
        let r = resolve_against(&root, &spec, "HEAD").unwrap();
        assert_eq!((r.state, r.start_line), (ResolutionState::Moved, 12));
    }

    #[test]
    fn an_anchor_of_the_first_cut_on_a_head_blob_resolves_by_diff_from_it() {
        let (_d, root) = repo();
        commit(&root, "ws.rs", V1);
        let mut spec = first_cut("ws.rs", (3, 8), &lines(V1)[3..8].join("\n"), "a\nb\nc", "x\ny\nz");
        spec.legacy_blob = blob_at(&root, "HEAD", "ws.rs").unwrap();
        let r = resolve_against(&root, &spec, "HEAD").unwrap();
        assert_eq!((r.label, r.method), (Label::Exact, Method::SameBlob));
        commit(&root, "ws.rs", &format!("new1\n{V1}"));
        let r = resolve_against(&root, &spec, "HEAD").unwrap();
        assert_eq!((r.label, r.method, r.start_line), (Label::Shifted, Method::Diff, 4));
    }

    #[test]
    fn a_new_dirty_anchor_resolves_by_diff_where_the_first_cut_searched() {
        let (_d, root) = repo();
        commit(&root, "ws.rs", V1);
        let text = format!("{V1}fn added() {{}}\n");
        fs::write(root.join("ws.rs"), &text).unwrap();
        let spec = form(&root, "ws.rs", 11, 12).unwrap();
        assert_eq!(spec.pieces.iter().map(|p| p.stage(spec.side)).collect::<Vec<_>>(), vec!["worktree"]);
        let r = resolve_against(&root, &spec, "worktree").unwrap();
        assert_eq!((r.label, r.method), (Label::Exact, Method::SameBlob));
        // HEAD is the version the anchor was made beside, which never held the line.
        let r = resolve_against(&root, &spec, "HEAD").unwrap();
        assert_eq!(r.availability, Availability::Absent { reason: AbsentReason::BeforeCreation });
        assert_eq!(r.label, Label::Orphaned);
        commit(&root, "ws.rs", &format!("first\n{text}"));
        let r = resolve_against(&root, &spec, "HEAD").unwrap();
        assert_eq!((r.label, r.method, r.start_line), (Label::Shifted, Method::Diff, 12));
    }

    #[test]
    fn line_arguments_are_one_based_inclusive() {
        assert_eq!(parse_lines("120-134").unwrap(), (119, 134));
        assert_eq!(parse_lines("7").unwrap(), (6, 7));
        assert!(parse_lines("0-3").is_err());
        assert!(parse_lines("9-3").is_err());
    }

    #[test]
    fn anchor_paths_stay_inside_the_checkout() {
        let (_d, root) = repo();
        std::fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/a.rs"), "a\nb\n").unwrap();
        assert_eq!(form(&root, "./src/a.rs", 0, 1).unwrap().path, "src/a.rs");
        assert!(form(&root, "../outside.rs", 0, 1).is_err(), "`..` leaves the checkout");
        let absolute = root.join("src/a.rs");
        assert_eq!(
            form(&root, absolute.to_str().unwrap(), 0, 1).unwrap().path,
            "src/a.rs",
            "an absolute path inside the checkout is converted"
        );
        let other = tempfile::tempdir().unwrap();
        let outside = other.path().join("outside.rs");
        fs::write(&outside, "x\ny\n").unwrap();
        let refused = form(&root, outside.to_str().unwrap(), 0, 1).unwrap_err().to_string();
        assert!(refused.contains("is outside this checkout"), "an absolute path outside it is refused: {refused}");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, root.join("link.rs")).unwrap();
            assert!(form(&root, "link.rs", 0, 1).is_err(), "a symlink that escapes the checkout is refused");
            let through = root.join("link.rs");
            assert!(
                form(&root, through.to_str().unwrap(), 0, 1).is_err(),
                "so is an absolute path through that symlink"
            );
        }
        // Another worktree (or a submodule) inside this checkout has a `.git` of its own: its files
        // are that checkout's, whether named relatively or absolutely.
        std::fs::create_dir_all(root.join(".claude/worktrees/other/src")).unwrap();
        fs::write(root.join(".claude/worktrees/other/.git"), "gitdir: elsewhere\n").unwrap();
        fs::write(root.join(".claude/worktrees/other/src/a.rs"), "a\nb\n").unwrap();
        let nested = root.join(".claude/worktrees/other/src/a.rs");
        let refused = form(&root, nested.to_str().unwrap(), 0, 1).unwrap_err().to_string();
        assert!(refused.contains("another checkout"), "{refused}");
        assert!(form(&root, ".claude/worktrees/other/src/a.rs", 0, 1).is_err());
        assert!(stored_path_is_safe("src/a.rs"));
        assert!(!stored_path_is_safe("/etc/hosts"));
        assert!(!stored_path_is_safe("../x"));
        assert!(!stored_path_is_safe(""));
    }

    /// R1 below the graph: two clones at one commit form the same key for the same lines, of
    /// committed content and of identical uncommitted content alike.
    #[test]
    fn two_clones_form_the_same_key_for_the_same_lines() {
        let (_d, origin) = repo();
        commit(&origin, "ws.rs", V1);
        let (_a, one) = fresh_clone(&origin);
        let (_b, two) = fresh_clone(&origin);
        let key = |root: &Path| {
            let spec = form(root, "ws.rs", 3, 8).unwrap();
            anchor_key("repo-id", &spec.path, spec.basis_blob.as_deref().unwrap(), spec.lines)
        };
        assert_eq!(key(&one), key(&two));
        let edited = V1.replace("try_once();", "try_once().await?;");
        fs::write(one.join("ws.rs"), &edited).unwrap();
        fs::write(two.join("ws.rs"), &edited).unwrap();
        assert_eq!(key(&one), key(&two), "uncommitted content has the same id everywhere");
        let whole = form_whole_file(&one, "ws.rs").unwrap();
        let spec = form(&one, "ws.rs", 3, 8).unwrap();
        assert_eq!(whole.basis_blob, spec.basis_blob);
        assert_ne!(
            anchor_key("repo-id", "ws.rs", whole.basis_blob.as_deref().unwrap(), whole.lines),
            key(&one),
            "the whole file is another anchor than lines of it"
        );
        assert!(key(&one).starts_with("fwd-anchor:1:"));
        assert_eq!(span_key("  fn f() {\t x }  \n"), span_key(" fn f() { x }"), "whitespace runs are one space");
        assert_ne!(span_key("fn f()"), span_key("fn g()"));
    }

    /// R2: a file changed in two hunks with the first staged, and a span over both and the lines
    /// between: three blobs, and three pieces.
    fn partly_staged() -> (tempfile::TempDir, std::path::PathBuf, AnchorSpec) {
        let (d, root) = repo();
        commit(&root, "f.txt", &numbered());
        let staged = numbered().replace("line 5\n", "line 5 staged\n");
        fs::write(root.join("f.txt"), &staged).unwrap();
        git(&root, &["add", "f.txt"]).unwrap();
        fs::write(root.join("f.txt"), staged.replace("line 20\n", "line 20 unstaged\n")).unwrap();
        let spec = form(&root, "f.txt", 4, 20).unwrap();
        (d, root, spec)
    }

    #[test]
    fn r2_a_partly_staged_span_has_three_blobs_and_three_pieces() {
        let (_d, root, spec) = partly_staged();
        let (basis, head, index) = (spec.basis_blob.clone(), spec.head_blob.clone(), spec.index_blob.clone());
        assert!(basis.is_some() && head.is_some() && index.is_some());
        assert!(basis != head && basis != index && head != index, "three distinct blobs");
        assert_eq!(head, blob_at(&root, "HEAD", "f.txt").unwrap());
        assert_eq!(index, index_blob(&root, "f.txt").unwrap());
        assert_eq!(
            spec.pieces,
            vec![
                Piece { basis_lines: [4, 5], head_lines: None, index_lines: Some([4, 5]) },
                Piece { basis_lines: [5, 19], head_lines: Some([5, 19]), index_lines: Some([5, 19]) },
                Piece { basis_lines: [19, 20], head_lines: None, index_lines: None },
            ]
        );
        assert_eq!(
            spec.pieces.iter().map(|p| p.stage(spec.side)).collect::<Vec<_>>(),
            vec!["index", "head", "worktree"]
        );
        // The index is nearer the basis than HEAD (one line apart, not two).
        assert_eq!(spec.nearest_blob, index);
    }

    /// R3: after the index is committed, the staged piece and the unchanged lines are there and
    /// the unstaged piece is not; a stash and pop between changes nothing; after the rest is
    /// committed, the span is exact.
    #[test]
    fn r3_committing_the_index_leaves_the_span_partial_then_exact() {
        let (_d, root, spec) = partly_staged();
        let before = resolve_against(&root, &spec, "HEAD").unwrap();
        assert_eq!(before.label, Label::Partial, "HEAD holds only the unchanged lines: {before:?}");

        git(&root, &["commit", "-q", "-m", "the staged hunk"]).unwrap();
        let head = resolve_against(&root, &spec, "HEAD").unwrap();
        assert_eq!((head.label, head.state), (Label::Partial, ResolutionState::Modified));
        assert_eq!(head.range, Some(LineRange { start_line: 4, end_line: 19 }));
        let Availability::Partial { pieces } = &head.availability else { panic!("{head:?}") };
        let fates: Vec<(Option<&str>, Option<[i64; 2]>)> =
            pieces.iter().map(|p| (p.stood.as_deref(), p.target_lines)).collect();
        assert_eq!(
            fates,
            vec![(Some("index"), Some([4, 5])), (Some("head"), Some([5, 19])), (Some("worktree"), None)],
            "the staged piece and the unchanged lines are there, the unstaged piece is not"
        );
        let worktree = resolve_against(&root, &spec, "worktree").unwrap();
        assert_eq!(worktree.label, Label::Exact);

        git(&root, &["stash", "push", "-q"]).unwrap();
        let stashed = resolve_against(&root, &spec, "worktree").unwrap();
        assert_eq!(stashed.label, Label::Partial, "the working tree is HEAD while the change is stashed");
        git(&root, &["stash", "pop", "-q"]).unwrap();
        assert!(same_answer(&resolve_against(&root, &spec, "HEAD").unwrap(), &head));
        assert!(same_answer(&resolve_against(&root, &spec, "worktree").unwrap(), &worktree));

        git(&root, &["commit", "-q", "-am", "the rest"]).unwrap();
        let r = resolve_against(&root, &spec, "HEAD").unwrap();
        assert_eq!((r.label, r.method), (Label::Exact, Method::SameBlob));
        let r = resolve_against(&root, &spec, "index").unwrap();
        assert_eq!(r.label, Label::Exact);
    }

    /// R6: amending, squashing and cherry-picking keep the file's blob, so an unchanged span stays
    /// exact.
    #[test]
    fn r6_rewritten_history_leaves_an_unchanged_span_exact() {
        let (_d, root) = repo();
        let base = commit(&root, "other.txt", "one\n");
        commit(&root, "ws.rs", V1);
        let spec = form(&root, "ws.rs", 3, 8).unwrap();
        let exact = |rev: &str| {
            let r = resolve_against(&root, &spec, rev).unwrap();
            assert_eq!((r.label, r.method), (Label::Exact, Method::SameBlob), "against {rev}: {r:?}");
        };

        fs::write(root.join("notes.txt"), "n\n").unwrap();
        git(&root, &["add", "notes.txt"]).unwrap();
        git(&root, &["commit", "-q", "--amend", "-m", "amended"]).unwrap();
        exact("HEAD");

        // Squashed as an interactive rebase would: the commit that added the file and the two
        // after it become one.
        commit(&root, "other.txt", "one\ntwo\n");
        commit(&root, "more.txt", "m\n");
        git(&root, &["reset", "-q", "--soft", "HEAD~3"]).unwrap();
        git(&root, &["commit", "-q", "-m", "squashed"]).unwrap();
        exact("HEAD");

        let pick = head_commit(&root).unwrap().unwrap();
        git(&root, &["checkout", "-q", "-b", "side", &base]).unwrap();
        commit(&root, "other.txt", "one\nside\n");
        git(&root, &["cherry-pick", "--strategy-option=theirs", &pick]).unwrap();
        exact("side");
        exact("HEAD");
    }

    /// The patch on an anchor rebuilds its basis on a machine that never saw it, and nothing but
    /// the exact basis is trusted.
    #[test]
    fn the_basis_is_rebuilt_from_the_nearest_blob_and_the_patch() {
        let (_d, root) = repo();
        commit(&root, "ws.rs", V1);
        let edited = format!("top\n{}", V1.replace("try_once();", "try_once().await?;"));
        fs::write(root.join("ws.rs"), &edited).unwrap();
        let spec = form(&root, "ws.rs", 4, 9).unwrap();
        let basis = spec.basis_blob.clone().unwrap();
        let nearest = blob_text(&root, spec.nearest_blob.as_deref().unwrap()).unwrap();
        let rebuilt = apply_patch(&nearest, spec.basis_patch.as_deref().unwrap()).unwrap();
        assert_eq!(rebuilt, edited);
        assert_eq!(hash_blob_content(&root, rebuilt.as_bytes()).unwrap(), basis);

        let (_c, clone) = fresh_clone(&root);
        assert!(!blob_exists(&clone, &basis), "the basis was never committed, so a clone lacks it");
        assert_eq!(basis_text(&clone, &spec), BasisText::Rebuilt(edited.clone()));
        let r = resolve_against(&clone, &spec, "HEAD").unwrap();
        assert_eq!((r.basis, r.label), (Some(BasisState::Rebuilt), Label::Partial), "{r:?}");

        let mut tampered = spec.clone();
        tampered.basis_patch = Some(spec.basis_patch.clone().unwrap().replace("+top", "+pot"));
        assert_eq!(basis_text(&clone, &tampered), BasisText::Unavailable, "a rebuild that does not hash to the basis");
        let r = resolve_against(&clone, &tampered, "HEAD").unwrap();
        assert_eq!(r.basis, Some(BasisState::Unavailable));
        assert!(!matches!(r.location, Location::Exact | Location::Shifted | Location::Moved), "{r:?}");
    }

    #[test]
    fn patches_are_exact_to_the_byte() {
        for (old, new) in [
            ("a\nb\nc\n", "a\nB\nc\n"),
            ("a\nb", "a\nb\nc"),
            ("a\nb\n", "a\nb"),
            ("", "x\ny"),
            ("one\r\ntwo\r\n", "one\ntwo\n"),
            ("a\rb\nc\n", "a\rB\nc\n"),
            ("same\n", "same\n"),
        ] {
            let patch = unified_patch("f", old, new);
            assert_eq!(apply_patch(old, &patch).unwrap(), new, "{old:?} to {new:?} by {patch:?}");
        }
        assert!(apply_patch("other\ntext\n", &unified_patch("f", "a\nb\nc\n", "a\nB\nc\n")).is_err());
    }

    #[test]
    fn an_untracked_file_carries_its_whole_content_as_the_patch() {
        let (_d, root) = repo();
        commit(&root, "ws.rs", V1);
        fs::write(root.join("new.rs"), "fn fresh() {}\nfn other() {}\n").unwrap();
        let spec = form(&root, "new.rs", 0, 1).unwrap();
        assert_eq!(
            (spec.head_blob.as_deref(), spec.index_blob.as_deref(), spec.nearest_blob.as_deref()),
            (None, None, None)
        );
        assert!(spec.basis_patch.as_deref().is_some_and(|p| p.contains("@@ -0,0 +1,2 @@")));
        assert_eq!(spec.pieces.iter().map(|p| p.stage(spec.side)).collect::<Vec<_>>(), vec!["worktree"]);
        let r = resolve_against(&root, &spec, "worktree").unwrap();
        assert_eq!((r.label, r.method), (Label::Exact, Method::SameBlob));
        let r = resolve_against(&root, &spec, "HEAD").unwrap();
        assert_eq!(r.availability, Availability::Absent { reason: AbsentReason::FileMissing });

        let (_c, clone) = fresh_clone(&root);
        assert_eq!(basis_text(&clone, &spec), BasisText::Rebuilt("fn fresh() {}\nfn other() {}\n".into()));
    }

    #[test]
    fn a_whole_file_anchor_is_present_renamed_or_deleted() {
        let (_d, root) = repo();
        commit(&root, "doc.md", "# Title\n\nBody.\n");
        let spec = form_whole_file(&root, "doc.md").unwrap();
        assert_eq!(spec.lines, None);
        assert_eq!((spec.anchored_text.as_str(), spec.pieces.len()), ("", 0));
        assert_eq!(spec.basis_blob, blob_at(&root, "HEAD", "doc.md").unwrap());

        let r = resolve_against(&root, &spec, "worktree").unwrap();
        assert_eq!(
            (r.label, r.location, r.availability.clone()),
            (Label::Exact, Location::Exact, Availability::Present)
        );
        assert_eq!((r.range, r.whole_file, r.start_line, r.end_line), (None, true, 0, 0), "no line is claimed");

        fs::write(root.join("doc.md"), "# Title\n\nBody, edited.\n").unwrap();
        let r = resolve_against(&root, &spec, "worktree").unwrap();
        assert_eq!((r.label, r.method), (Label::Modified, Method::File));
        git(&root, &["checkout", "-q", "--", "doc.md"]).unwrap();

        // Moved without git being told: found by its content among untracked files.
        fs::rename(root.join("doc.md"), root.join("moved.md")).unwrap();
        let r = resolve_against(&root, &spec, "worktree").unwrap();
        assert_eq!((r.label, r.path.as_str()), (Label::Moved, "moved.md"));
        fs::rename(root.join("moved.md"), root.join("doc.md")).unwrap();

        // Renamed in the index, then in a commit.
        git(&root, &["mv", "doc.md", "guide.md"]).unwrap();
        for target in ["index", "worktree"] {
            let r = resolve_against(&root, &spec, target).unwrap();
            assert_eq!((r.label, r.path.as_str()), (Label::Moved, "guide.md"), "against {target}");
        }
        git(&root, &["commit", "-q", "-m", "rename"]).unwrap();
        let r = resolve_against(&root, &spec, "HEAD").unwrap();
        assert_eq!((r.label, r.location, r.path.as_str()), (Label::Moved, Location::Moved, "guide.md"));
        assert_eq!(r.content, Content::Same);

        git(&root, &["rm", "-q", "guide.md"]).unwrap();
        git(&root, &["commit", "-q", "-m", "delete"]).unwrap();
        let r = resolve_against(&root, &spec, "HEAD").unwrap();
        assert_eq!(r.label, Label::Orphaned);
        assert_eq!(r.availability, Availability::Absent { reason: AbsentReason::Deleted });
    }

    #[test]
    fn a_whole_file_anchor_takes_any_content() {
        let (_d, root) = repo();
        fs::write(root.join("logo.bin"), [0u8, 159, 146, 150, 255, 0, 10]).unwrap();
        let spec = form_whole_file(&root, "logo.bin").unwrap();
        assert!(spec.basis_blob.is_some());
        assert_eq!(spec.basis_patch, None, "no text patch for bytes that are not text");
        assert!(form(&root, "logo.bin", 0, 1).unwrap_err().to_string().contains("not UTF-8 text"));
        let r = resolve_against(&root, &spec, "worktree").unwrap();
        assert_eq!(r.label, Label::Exact);
    }

    #[test]
    fn a_span_in_a_renamed_file_is_moved_with_its_path() {
        let (_d, root) = repo();
        commit(&root, "ws.rs", V1);
        let spec = form(&root, "ws.rs", 3, 8).unwrap();
        git(&root, &["mv", "ws.rs", "net.rs"]).unwrap();
        git(&root, &["commit", "-q", "-m", "rename"]).unwrap();
        commit(&root, "net.rs", &format!("// moved\n{V1}"));
        let r = resolve_against(&root, &spec, "HEAD").unwrap();
        assert_eq!((r.label, r.path.as_str(), r.start_line), (Label::Moved, "net.rs", 4));
    }

    /// E1: the same CRLF content hashes to one id through `--path` under `core.autocrlf=true`
    /// and under a `text=auto` attribute, the id of its LF form; and a file whose committed copy
    /// already has CRLF keeps it, as `git add` does, because its basis is then the index blob.
    #[test]
    fn e1_line_endings_give_one_id() {
        let crlf = b"one\r\ntwo\r\n";
        let (_a, autocrlf) = repo();
        git(&autocrlf, &["config", "core.autocrlf", "true"]).unwrap();
        let (_b, text_auto) = repo();
        fs::write(text_auto.join(".gitattributes"), "* text=auto\n").unwrap();
        let (_c, plain) = repo();
        let lf_id = hash_blob_content(&plain, b"one\ntwo\n").unwrap();
        assert_eq!(hash_object(&autocrlf, "f.txt", crlf, false).unwrap(), lf_id);
        assert_eq!(hash_object(&text_auto, "f.txt", crlf, false).unwrap(), lf_id);
        assert_ne!(hash_object(&plain, "f.txt", crlf, false).unwrap(), lf_id, "no filters, no conversion");

        fs::write(plain.join("f.txt"), crlf).unwrap();
        git(&plain, &["add", "f.txt"]).unwrap();
        git(&plain, &["commit", "-q", "-m", "crlf"]).unwrap();
        git(&plain, &["config", "core.autocrlf", "true"]).unwrap();
        let committed = blob_at(&plain, "HEAD", "f.txt").unwrap();
        let spec = form_whole_file(&plain, "f.txt").unwrap();
        assert_eq!(spec.basis_blob, committed, "the file matches its index copy, so that blob is the basis");
        assert_eq!(hash_object(&plain, "f.txt", crlf, false).unwrap(), lf_id, "where hash-object alone would convert");
    }

    /// `ws.rs` committed as `V1`, then its loop's call changed and staged, then a line added
    /// above everything and not staged: HEAD, the index and the working tree all differ.
    fn staged_and_unstaged() -> (tempfile::TempDir, std::path::PathBuf) {
        let (d, root) = repo();
        commit(&root, "ws.rs", V1);
        let staged = V1.replace("try_once();", "try_once().await?;");
        fs::write(root.join("ws.rs"), &staged).unwrap();
        git(&root, &["add", "ws.rs"]).unwrap();
        fs::write(root.join("ws.rs"), format!("// top\n{staged}")).unwrap();
        (d, root)
    }

    /// Lines on the index's side of a diff (task Iz0HMyk0): the anchor counts the index's lines
    /// and has the index's blob, pinned, for its basis, and its pieces say which of its lines
    /// were staged. It is exact in the index and one line down in the working tree; HEAD, older
    /// than its basis, never held the staged line.
    #[test]
    fn an_anchor_on_the_index_has_the_indexs_lines_and_blob() {
        let (_d, root) = staged_and_unstaged();
        let index = index_blob(&root, "ws.rs").unwrap();
        let spec = form_on_revision(&root, "ws.rs", Some((3, 8)), "index", DiffSide::Base).unwrap();
        assert_eq!(
            (spec.side, spec.basis_blob.clone(), spec.index_blob.clone()),
            (DiffSide::Base, index.clone(), index.clone())
        );
        assert_eq!(spec.head_blob, blob_at(&root, "HEAD", "ws.rs").unwrap());
        assert_eq!((spec.nearest_blob.clone(), spec.basis_patch.as_deref()), (index.clone(), Some("")));
        assert_eq!(spec.anchored_text, "fn reconnect() {\n    loop {\n        try_once().await?;\n    }\n}");
        assert_eq!((spec.context_before.as_str(), spec.context_after.as_str()), ("a\nb\nc", "x\ny\nz"));
        let stages: Vec<(&str, [i64; 2])> = spec.pieces.iter().map(|p| (p.stage(spec.side), p.basis_lines)).collect();
        assert_eq!(stages, vec![("head", [3, 5]), ("index", [5, 6]), ("head", [6, 8])]);
        let pinned = git(&root, &["rev-parse", &format!("refs/fwd/basis/{}", index.clone().unwrap())]).unwrap();
        assert_eq!(Some(pinned.trim().to_string()), index, "the basis is pinned by a ref");

        let at = |target: &str| resolve_against(&root, &spec, target).unwrap();
        let in_index = at("index");
        assert_eq!((in_index.label, in_index.range), (Label::Exact, Some(LineRange { start_line: 3, end_line: 8 })));
        let in_worktree = at("worktree");
        assert_eq!(
            (in_worktree.label, in_worktree.range),
            (Label::Shifted, Some(LineRange { start_line: 4, end_line: 9 }))
        );
        let in_head = at("HEAD");
        let Availability::Partial { pieces } = &in_head.availability else { panic!("partial in HEAD: {in_head:?}") };
        assert_eq!(
            pieces.iter().map(|p| (p.basis_lines, p.target_lines, p.stood.as_deref())).collect::<Vec<_>>(),
            vec![
                ([3, 5], Some([3, 5]), Some("head")),
                ([5, 6], None, Some("index")),
                ([6, 8], Some([6, 8]), Some("head"))
            ]
        );
    }

    /// Lines on HEAD's side of the staged changes: the anchor has HEAD's lines and blob. The index
    /// and the working tree are newer than that basis, so the line the staged change rewrote is
    /// looked for there, as in any other version, rather than ruled never there.
    #[test]
    fn an_anchor_on_head_is_looked_for_in_the_newer_index() {
        let (_d, root) = staged_and_unstaged();
        let spec = form_on_revision(&root, "ws.rs", Some((5, 6)), "HEAD", DiffSide::Base).unwrap();
        assert_eq!(
            (spec.basis_blob.clone(), spec.anchored_text.as_str()),
            (blob_at(&root, "HEAD", "ws.rs").unwrap(), "        try_once();")
        );
        assert_eq!(spec.pieces.iter().map(|p| p.stage(spec.side)).collect::<Vec<_>>(), vec!["head"]);
        let in_index = resolve_against(&root, &spec, "index").unwrap();
        assert_eq!(
            (in_index.label, in_index.range),
            (Label::Recovered, Some(LineRange { start_line: 5, end_line: 6 })),
            "{in_index:?}"
        );
        let in_worktree = resolve_against(&root, &spec, "worktree").unwrap();
        assert_eq!(
            (in_worktree.label, in_worktree.range),
            (Label::Recovered, Some(LineRange { start_line: 6, end_line: 7 }))
        );
    }

    /// Lines of an older commit, on the right side of its changes, that a later commit deleted:
    /// its pieces say the lines stood in that revision alone, and HEAD, newer than the basis,
    /// reports them deleted.
    #[test]
    fn an_anchor_on_an_older_commit_finds_its_lines_deleted_since() {
        let (_d, root) = repo();
        let first = commit(&root, "ws.rs", V1);
        commit(&root, "ws.rs", &V1.replace("fn reconnect() {\n    loop {\n        try_once();\n    }\n}\n", ""));
        let spec = form_on_revision(&root, "ws.rs", Some((3, 8)), &first, DiffSide::Modified).unwrap();
        assert_eq!(
            (spec.side, spec.basis_blob.clone()),
            (DiffSide::Modified, blob_at(&root, &first, "ws.rs").unwrap())
        );
        assert_eq!(spec.pieces.iter().map(|p| p.stage(spec.side)).collect::<Vec<_>>(), vec!["revision"]);
        let in_head = resolve_against(&root, &spec, "HEAD").unwrap();
        assert_eq!(in_head.availability, Availability::Absent { reason: AbsentReason::Deleted }, "{in_head:?}");
    }

    /// A revision holds files the working tree may not have: lines of a file deleted from the
    /// working tree are anchored on HEAD, and so is the whole file, named by its absolute path. A
    /// blob, a revision this checkout lacks, a file the revision lacks, a path out of the checkout
    /// and the working tree are each refused with a sentence saying why.
    #[test]
    fn a_revision_is_anchored_without_the_file_on_disk_and_only_a_revision_is() {
        let (_d, root) = repo();
        commit(&root, "ws.rs", V1);
        let head = blob_at(&root, "HEAD", "ws.rs").unwrap();
        fs::remove_file(root.join("ws.rs")).unwrap();
        let spec = form_on_revision(&root, "ws.rs", Some((3, 8)), "HEAD", DiffSide::Base).unwrap();
        assert_eq!((spec.basis_blob.clone(), spec.anchored_text.lines().count()), (head.clone(), 5));
        let whole =
            form_on_revision(&root, &root.join("ws.rs").display().to_string(), None, "HEAD", DiffSide::Base).unwrap();
        assert_eq!((whole.path.as_str(), whole.whole_file(), whole.basis_blob), ("ws.rs", true, head.clone()));
        let refused = |path: &str, target: &str| {
            form_on_revision(&root, path, Some((0, 1)), target, DiffSide::Base).unwrap_err().to_string()
        };
        assert!(refused("ws.rs", &head.unwrap()).contains("names a blob, not a version of a file"));
        assert!(refused("ws.rs", "no-such-branch").contains("this checkout has no `no-such-branch`"));
        assert!(refused("other.rs", "HEAD").contains("`other.rs` is not in `HEAD`"));
        assert!(refused("../ws.rs", "HEAD").contains("leaves the checkout"));
        assert!(refused("ws.rs", "worktree").contains("anchored without --against"));
    }
}
