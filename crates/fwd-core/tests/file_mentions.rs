//! A whole file named in a message's text, at a version, through the posting path
//! (`fwd_core::file_mention`; task nVE7KTeSqiX0): read in the checkout the command runs in, on
//! disk, in the index or at a commit; refused when it is not there, or where no checkout is read;
//! stored as its whole-file highlight's id and the version it names, the highlight found when it
//! exists; and read again with no checkout, as a forward and an edit read a stored text.

use ankurah::{policy::DEFAULT_CONTEXT, Context, EntityId, Node, PermissiveAgent};
use ankurah_storage_sqlite::SqliteStorageEngine;
use anyhow::Result;
use fwd_core::anchoring::{self, git};
use fwd_core::file_mention::reading_files_in;
use fwd_core::model::*;
use fwd_core::ops::{self, MessageContext};
use fwd_core::references::{self, Named};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

async fn context() -> Result<Context> {
    let storage = SqliteStorageEngine::open_in_memory().await?;
    let node = Node::new_durable(Arc::new(storage), PermissiveAgent::new());
    node.system.create().await?;
    Ok(node.context_async(DEFAULT_CONTEXT).await?)
}

const PRIMER: &str = "# Using fwd\n\nThe primer.\n";

/// A clone of an upstream whose first commit holds `docs/using-fwd.md` and `src/lib.rs`, with the
/// commit's id.
fn checkout(dir: &Path) -> Result<(PathBuf, String)> {
    let origin = dir.join("origin");
    std::fs::create_dir_all(origin.join("docs"))?;
    std::fs::create_dir_all(origin.join("src"))?;
    git(&origin, &["init", "-q", "-b", "main"])?;
    git(&origin, &["config", "user.email", "t@example.com"])?;
    git(&origin, &["config", "user.name", "t"])?;
    std::fs::write(origin.join("docs/using-fwd.md"), PRIMER)?;
    std::fs::write(origin.join("src/lib.rs"), "pub fn one() {}\n")?;
    git(&origin, &["add", "."])?;
    git(&origin, &["commit", "-q", "-m", "init"])?;
    git(dir, &["clone", "-q", "--no-local", &format!("file://{}", origin.display()), "work"])?;
    let root = dir.join("work");
    git(&root, &["config", "user.email", "t@example.com"])?;
    git(&root, &["config", "user.name", "t"])?;
    let head = git(&root, &["rev-parse", "HEAD"])?.trim().to_string();
    Ok((root, head))
}

/// Posts `text` by `by` as a comment on nothing in particular, reading files in `root`.
async fn post_in(ctx: &Context, root: &Path, by: EntityId, text: &str) -> Result<EntityId> {
    in_checkout(root, ops::post_message(ctx, by, text, Priority::Next, MessageContext::default())).await
}

async fn in_checkout<F: Future>(root: &Path, work: F) -> F::Output {
    reading_files_in(Some(root.to_path_buf()), work).await
}

/// What the stored text of `message` names, in order.
async fn named(ctx: &Context, message: EntityId) -> Result<Vec<Named>> {
    let m: MessageView = ctx.get(message).await?;
    Ok(references::stored_named(&ops::text(m.content())?).into_iter().map(|(_, n)| n).collect())
}

/// The highlight a file mention names, with what it holds.
async fn highlight(ctx: &Context, id: EntityId) -> Result<CodeAnchorView> {
    Ok(ctx.get(id).await?)
}

async fn whole_files(ctx: &Context) -> Result<usize> {
    Ok(ctx.fetch::<CodeAnchorView>("end_line = 0").await?.len())
}

#[tokio::test]
async fn a_file_is_named_on_disk_in_the_index_and_at_a_commit() -> Result<()> {
    let ctx = context().await?;
    let me = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let dir = tempfile::tempdir()?;
    let (root, head) = checkout(dir.path())?;

    // On disk, with an unstaged change: the file as it stands, whatever HEAD holds.
    let edited = format!("{PRIMER}One more line, not staged.\n");
    std::fs::write(root.join("docs/using-fwd.md"), &edited)?;
    let m = post_in(&ctx, &root, me, "The primer is [[docs/using-fwd.md]], the how-to.").await?;
    let [on_disk] = named(&ctx, m).await?.try_into().expect("one reference");
    assert_eq!(on_disk.version, Some(fwd_core::file_mention::Version::Worktree));
    let a = highlight(&ctx, on_disk.id).await?;
    assert_eq!((a.path()?, a.whole_file()?), ("docs/using-fwd.md".to_string(), Some(true)));
    let blob = git(&root, &["hash-object", "docs/using-fwd.md"])?.trim().to_string();
    assert_eq!(a.basis_blob()?, Some(blob), "the highlight holds the file as it stood on disk");
    let stored: MessageView = ctx.get(m).await?;
    assert_eq!(
        ops::text(stored.content())?,
        format!("The primer is [[{}@worktree]], the how-to.", on_disk.id.to_base64()),
        "stored as the highlight's id and the version named"
    );
    let mentions = ops::relations_from(&ctx, m, Some(RelationKind::Mention)).await?;
    assert_eq!(mentions.iter().map(|r| r.to_id().unwrap()).collect::<Vec<_>>(), vec![on_disk.id], "a mention edge");

    // At a commit, named by a revision: stored as the commit it named, so it does not drift.
    let m = post_in(&ctx, &root, me, "Before: [[docs/using-fwd.md@HEAD]].").await?;
    let [at_head] = named(&ctx, m).await?.try_into().expect("one reference");
    assert_eq!(at_head.version, Some(fwd_core::file_mention::Version::Commit(head.clone())));
    assert_ne!(at_head.id, on_disk.id, "other content is another highlight");
    let head_blob = git(&root, &["rev-parse", "HEAD:docs/using-fwd.md"])?.trim().to_string();
    assert_eq!(highlight(&ctx, at_head.id).await?.basis_blob()?, Some(head_blob));
    // The same commit by its short id, or by the branch, names the same highlight.
    let short: String = head.chars().take(7).collect();
    let m = post_in(&ctx, &root, me, &format!("[[docs/using-fwd.md@{short}]] and [[docs/using-fwd.md@main]]")).await?;
    assert_eq!(
        named(&ctx, m).await?,
        vec![at_head.clone(), at_head.clone()],
        "one highlight, and the version as the commit's whole id"
    );

    // In the index: the staged copy, apart from a change made on disk after it.
    let staged = format!("{PRIMER}A staged line.\n");
    std::fs::write(root.join("docs/using-fwd.md"), &staged)?;
    git(&root, &["add", "docs/using-fwd.md"])?;
    std::fs::write(root.join("docs/using-fwd.md"), format!("{staged}And an unstaged one.\n"))?;
    let m = post_in(&ctx, &root, me, "Staged: [[docs/using-fwd.md@index]]").await?;
    let [in_index] = named(&ctx, m).await?.try_into().expect("one reference");
    assert_eq!(in_index.version, Some(fwd_core::file_mention::Version::Index));
    let index_blob = git(&root, &["rev-parse", ":docs/using-fwd.md"])?.trim().to_string();
    assert_eq!(highlight(&ctx, in_index.id).await?.basis_blob()?, Some(index_blob));

    // A file at the root is written with `./`, and an absolute path inside the checkout is taken.
    std::fs::write(root.join("Makefile"), "all:\n")?;
    let absolute = root.join("src/lib.rs");
    let m = post_in(&ctx, &root, me, &format!("[[./Makefile]] and [[{}@HEAD]]", absolute.display())).await?;
    let paths: Vec<String> = {
        let mut out = Vec::new();
        for n in named(&ctx, m).await? {
            out.push(highlight(&ctx, n.id).await?.path()?);
        }
        out
    };
    assert_eq!(paths, vec!["Makefile".to_string(), "src/lib.rs".to_string()]);
    Ok(())
}

#[tokio::test]
async fn an_existing_whole_file_highlight_is_the_one_a_mention_names() -> Result<()> {
    let ctx = context().await?;
    let me = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let dir = tempfile::tempdir()?;
    let (root, head) = checkout(dir.path())?;
    // `fwd anchor create --path docs/using-fwd.md` made it before.
    let spec = anchoring::form_whole_file(&root, "docs/using-fwd.md")?;
    let repo = ops::ensure_repo(&ctx, &root).await?;
    let made = ops::create_anchor(&ctx, me, repo, &spec).await?;
    let before = whole_files(&ctx).await?;

    // The file on disk, clean, and the file at HEAD hold the same bytes: one highlight, named at
    // the version each mention names.
    let m = post_in(&ctx, &root, me, "[[docs/using-fwd.md]] is [[docs/using-fwd.md@HEAD]]").await?;
    let names = named(&ctx, m).await?;
    assert_eq!(names.iter().map(|n| n.id).collect::<Vec<_>>(), vec![made, made], "the highlight made already");
    assert_eq!(
        names.iter().map(|n| n.version.clone()).collect::<Vec<_>>(),
        vec![
            Some(fwd_core::file_mention::Version::Worktree),
            Some(fwd_core::file_mention::Version::Commit(head.clone()))
        ],
        "each with the version its writer named"
    );
    assert_eq!(whole_files(&ctx).await?, before, "no highlight made");

    // Each is named with its version when the text is read.
    let stored: MessageView = ctx.get(m).await?;
    let text = ops::text(stored.content())?;
    let short: String = head.chars().take(7).collect();
    assert_eq!(
        ops::render_references(&ctx, &text).await,
        format!("[docs/using-fwd.md on disk] is [docs/using-fwd.md {short}]")
    );
    let listed = ops::references_json(&ctx, &text).await;
    assert_eq!(listed[1]["kind"], "anchor");
    assert_eq!(listed[1]["name"], format!("docs/using-fwd.md {short}"));
    assert_eq!(listed[1]["version"], head);
    assert_eq!(listed[0]["version"], "worktree");
    Ok(())
}

#[tokio::test]
async fn a_file_not_there_is_refused_naming_it_and_the_text_writes_nothing() -> Result<()> {
    let ctx = context().await?;
    let me = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let dir = tempfile::tempdir()?;
    let (root, head) = checkout(dir.path())?;
    let short: String = head.chars().take(7).collect();
    std::fs::write(root.join("notes.md"), "untracked\n")?;
    let refused = |text: &'static str| post_in(&ctx, &root, me, text);

    for (text, why) in [
        ("see [[docs/nope.md]]", "docs/nope.md is not on disk in this checkout".to_string()),
        ("see [[docs/]]", "docs/ is a directory, and a mention names a file".to_string()),
        ("see [[notes.md@index]]", "notes.md is not in the index".to_string()),
        ("see [[notes.md@HEAD]]", format!("notes.md is not in commit {short}")),
        ("see [[./docs@HEAD]]", format!("docs is a directory at {short}, and a mention names a file")),
        ("see [[docs/using-fwd.md@nope]]", "this checkout has no revision `nope`".to_string()),
        ("see [[docs/using-fwd.md@]]", "no version follows the at-sign".to_string()),
    ] {
        let err = refused(text).await.expect_err(text).to_string();
        assert!(err.starts_with("refused: "), "{err}");
        assert!(err.contains(&why), "{text}: {err}");
        assert!(err.contains("names nothing"), "{err}");
    }

    // A text that names a file and something that names nothing is refused whole, with no
    // highlight left behind for the file it read.
    let before = whole_files(&ctx).await?;
    let err = refused("[[docs/using-fwd.md]] and [[zzzzzzzz]]").await.expect_err("a typo").to_string();
    assert!(err.contains("`[[zzzzzzzz]]`"), "{err}");
    assert_eq!(whole_files(&ctx).await?, before, "a refused text makes no highlight");
    assert!(ctx.fetch::<MessageView>("created > 0").await?.is_empty(), "nor a message");
    Ok(())
}

#[tokio::test]
async fn a_file_named_where_no_checkout_is_read_is_refused_saying_where_to_write_it() -> Result<()> {
    let ctx = context().await?;
    let me = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let post = || ops::post_message(&ctx, me, "see [[docs/cli.md]]", Priority::Next, MessageContext::default());
    // The daemon, and any caller that reads no checkout; then the CLI outside a checkout of the
    // project's repository.
    let unscoped = post().await.expect_err("no checkout").to_string();
    let outside = reading_files_in(None, post()).await.expect_err("no checkout").to_string();
    for err in [unscoped, outside] {
        assert!(
            err.contains("`[[docs/cli.md]]` names a file, which is read in the checkout a command runs in"),
            "{err}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_stored_file_mention_reads_again_with_no_checkout_keeping_its_version() -> Result<()> {
    let ctx = context().await?;
    let me = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let dir = tempfile::tempdir()?;
    let (root, head) = checkout(dir.path())?;
    let m = post_in(&ctx, &root, me, "The primer is [[docs/using-fwd.md@HEAD]].").await?;
    let [file] = named(&ctx, m).await?.try_into().expect("one reference");
    let stored: MessageView = ctx.get(m).await?;
    let text = ops::text(stored.content())?;

    // A forward quotes the stored text, and posts it with no checkout to read.
    let forward = ops::post_message(&ctx, me, &format!("> {text}"), Priority::Next, MessageContext::default()).await?;
    assert_eq!(named(&ctx, forward).await?, vec![file.clone()]);

    // An edit box writes it back by the start of the highlight's id and its version, which an
    // edit reads again with no checkout, changing nothing.
    let editable = references::editable(&text, |_| None);
    let start: String = file.id.to_base64().chars().take(8).collect();
    assert_eq!(editable, format!("The primer is [[{start}@{head}]]."));
    ops::edit_message(&ctx, me, m, &editable).await?;
    let after: MessageView = ctx.get(m).await?;
    assert_eq!(ops::text(after.content())?, text, "the same text");
    assert_eq!(after.edited()?, None, "an edit that changes nothing marks nothing");
    ops::edit_message(&ctx, me, m, &format!("The staged primer is [[{start}@index]].")).await?;
    assert_eq!(
        named(&ctx, m).await?,
        vec![Named { id: file.id, version: Some(fwd_core::file_mention::Version::Index) }]
    );
    let mentions = ops::relations_from(&ctx, m, Some(RelationKind::Mention)).await?;
    let standing: Vec<_> = mentions.iter().filter(|r| r.status().ok() == Some(RelationStatus::Accepted)).collect();
    assert_eq!(standing.len(), 1, "the mention edge to the highlight stays");

    // After an id, the version is one a stored mention writes; a revision needs the path.
    for (text, why) in [
        (format!("[[{start}@HEAD]]"), "after a highlight's id the version is worktree, index or a commit's id"),
        (format!("[[@{head}]]"), "no highlight's id comes before the at-sign"),
    ] {
        let err = ops::post_message(&ctx, me, &text, Priority::Next, MessageContext::default()).await.unwrap_err();
        assert!(err.to_string().contains(why), "{text}: {err}");
    }
    // A version goes with a whole file, never with lines of one.
    let spec = anchoring::form(&root, "src/lib.rs", 0, 1)?;
    let repo = ops::ensure_repo(&ctx, &root).await?;
    let lines = ops::create_anchor(&ctx, me, repo, &spec).await?;
    let err = ops::post_message(
        &ctx,
        me,
        &format!("[[{}@index]]", lines.to_base64()),
        Priority::Next,
        MessageContext::default(),
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(err.contains("a version goes with a whole file"), "{err}");
    Ok(())
}
