//! Anchors through the graph: one entity for one key from any clone (R1 of
//! docs/research/anchoring-robust-proposal.md), the resolution cache in the sidecar rather than
//! on the entity, and anchors of the first cut gaining a key and basis.

use ankurah::property::{Json, Ref};
use ankurah::{policy::DEFAULT_CONTEXT, Context, EntityId, Node, PermissiveAgent};
use ankurah_storage_sqlite::SqliteStorageEngine;
use anyhow::Result;
use fwd_core::anchoring::{self, git, Label, Method};
use fwd_core::model::*;
use fwd_core::ops;
use fwd_core::query::with_string;
use fwd_core::sidecar::Sidecar;
use std::path::{Path, PathBuf};
use std::sync::Arc;

async fn context() -> Result<Context> {
    let storage = SqliteStorageEngine::open_in_memory().await?;
    let node = Node::new_durable(Arc::new(storage), PermissiveAgent::new());
    node.system.create().await?;
    Ok(node.context_async(DEFAULT_CONTEXT).await?)
}

const V1: &str = "a\nb\nc\nfn reconnect() {\n    loop {\n        try_once();\n    }\n}\nx\ny\nz\n";

/// An upstream with `ws.rs` committed, and a clone of it that fetched only what branches reach.
fn upstream(dir: &Path) -> Result<PathBuf> {
    let origin = dir.join("origin");
    std::fs::create_dir_all(&origin)?;
    git(&origin, &["init", "-q", "-b", "main"])?;
    git(&origin, &["config", "user.email", "t@example.com"])?;
    git(&origin, &["config", "user.name", "t"])?;
    std::fs::write(origin.join("ws.rs"), V1)?;
    git(&origin, &["add", "ws.rs"])?;
    git(&origin, &["commit", "-q", "-m", "init"])?;
    Ok(origin)
}

fn clone(dir: &Path, origin: &Path, name: &str) -> Result<PathBuf> {
    git(dir, &["clone", "-q", "--no-local", &format!("file://{}", origin.display()), name])?;
    let root = dir.join(name);
    git(&root, &["config", "user.email", "t@example.com"])?;
    git(&root, &["config", "user.name", "t"])?;
    Ok(root)
}

async fn anchor_in(ctx: &Context, me: EntityId, root: &Path, lines: Option<(i64, i64)>) -> Result<EntityId> {
    let spec = match lines {
        Some((start, end)) => anchoring::form(root, "ws.rs", start, end)?,
        None => anchoring::form_whole_file(root, "ws.rs")?,
    };
    let repo = ops::ensure_repo(ctx, root).await?;
    ops::create_anchor(ctx, me, repo, &spec).await
}

#[tokio::test]
async fn r1_two_clones_make_one_anchor_for_the_same_lines() -> Result<()> {
    let ctx = context().await?;
    let me = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let dir = tempfile::tempdir()?;
    let origin = upstream(dir.path())?;
    let one = clone(dir.path(), &origin, "one")?;
    let two = clone(dir.path(), &origin, "two")?;

    let first = anchor_in(&ctx, me, &one, Some((3, 8))).await?;
    let second = anchor_in(&ctx, me, &two, Some((3, 8))).await?;
    assert_eq!(first, second, "the same lines of the same content are one anchor");
    let view: CodeAnchorView = ctx.get(first).await?;
    let key = view.anchor_key()?.expect("a key");
    let same: Vec<CodeAnchorView> = ctx.fetch(with_string("anchor_key = ?", &key)?).await?;
    assert_eq!(same.len(), 1);

    // Uncommitted, identical edits in both clones: still one anchor, apart from the first.
    let edited = V1.replace("try_once();", "try_once().await?;");
    std::fs::write(one.join("ws.rs"), &edited)?;
    std::fs::write(two.join("ws.rs"), &edited)?;
    let dirty = anchor_in(&ctx, me, &one, Some((3, 8))).await?;
    assert_eq!(anchor_in(&ctx, me, &two, Some((3, 8))).await?, dirty);
    assert_ne!(dirty, first, "other content is another basis");

    // The whole file is an anchor of its own, made once too.
    let whole = anchor_in(&ctx, me, &one, None).await?;
    assert_eq!(anchor_in(&ctx, me, &two, None).await?, whole);
    let view: CodeAnchorView = ctx.get(whole).await?;
    let json = ops::json_of_anchor(&view)?;
    assert_eq!(json["whole_file"], true);
    assert_eq!((json["start_line"].as_i64(), json["end_line"].as_i64()), (Some(0), Some(0)));
    assert_eq!(json["anchored_text"], "");
    assert_eq!(json["span_key"], serde_json::Value::Null);
    assert!(json.get("resolution").is_none(), "no resolution cache on the entity");
    Ok(())
}

#[tokio::test]
async fn resolutions_are_cached_in_the_sidecar_not_on_the_entity() -> Result<()> {
    let ctx = context().await?;
    let me = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let dir = tempfile::tempdir()?;
    let origin = upstream(dir.path())?;
    let root = clone(dir.path(), &origin, "work")?;
    let id = anchor_in(&ctx, me, &root, Some((3, 8))).await?;
    let store = Sidecar::in_memory()?;

    let r = ops::resolve_anchor(&ctx, &root, id, "HEAD", Some(&store)).await?;
    assert_eq!(r.label, Label::Exact);
    let target = r.target.clone().expect("the target is described");
    assert_eq!((target.kind.as_str(), target.rev.as_deref()), ("commit", Some("HEAD")));
    let view: CodeAnchorView = ctx.get(id).await?;
    let key = view.anchor_key()?.unwrap();
    let blob = target.blob.clone().unwrap();
    let cached = store.resolution(&key, "ws.rs", &blob)?.expect("kept in the sidecar");
    assert_eq!((cached.label, cached.target.clone()), (Label::Exact, None));

    // A second answer for the same blob comes from the store: a doctored record proves it.
    let doctored = anchoring::Resolution { confidence: 0.25, ..cached };
    store.remember(&key, "ws.rs", &blob, &doctored)?;
    let again = ops::resolve_anchor(&ctx, &root, id, "HEAD", Some(&store)).await?;
    assert_eq!(again.confidence, 0.25);
    assert_eq!(again.target, Some(target), "the target is described anew for each question");

    // The same blob reached through the working tree answers from the store too.
    let worktree = ops::resolve_anchor(&ctx, &root, id, "worktree", Some(&store)).await?;
    assert_eq!(worktree.confidence, 0.25);
    Ok(())
}

/// What the first cut stored for an anchor: its lines, its text and context, and `blob_sha` for
/// a selection unchanged since HEAD; no key and no basis.
struct FirstCut<'a> {
    lines: (i64, i64),
    blob_sha: Option<String>,
    text: &'a str,
    before: &'a str,
    after: &'a str,
}

/// Creates an anchor the way the first cut stored one.
async fn first_cut(ctx: &Context, me: EntityId, repo: EntityId, old: FirstCut<'_>) -> Result<EntityId> {
    let FirstCut { lines, blob_sha, text, before, after } = old;
    let trx = ctx.begin();
    let id = trx
        .create(&CodeAnchor {
            repo: Ref::new(repo),
            path: "ws.rs".into(),
            anchor_key: None,
            span_key: None,
            basis_blob: None,
            head_blob: None,
            index_blob: None,
            nearest_blob: None,
            basis_patch: None,
            blob_sha,
            commit_sha: None,
            side: DiffSide::Worktree,
            start_line: lines.0,
            end_line: lines.1,
            whole_file: None,
            anchored_text: text.into(),
            context_before: before.into(),
            context_after: after.into(),
            pieces: Json::new(serde_json::json!([])),
            symbol_path: None,
            created_by: Ref::new(me),
            created: fwd_core::now_ms(),
        })
        .await?
        .id();
    trx.commit().await?;
    Ok(id)
}

#[tokio::test]
async fn anchors_of_the_first_cut_gain_a_key_and_basis_when_a_checkout_can_supply_them() -> Result<()> {
    let ctx = context().await?;
    let me = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let dir = tempfile::tempdir()?;
    let origin = upstream(dir.path())?;
    let root = clone(dir.path(), &origin, "work")?;
    let repo = ops::ensure_repo(&ctx, &root).await?;
    let head = anchoring::blob_at(&root, "HEAD", "ws.rs")?;
    let block = "fn reconnect() {\n    loop {\n        try_once();\n    }\n}";

    // Mapped onto the HEAD blob: that blob becomes the basis, and the key follows.
    let old = FirstCut { lines: (3, 8), blob_sha: head.clone(), text: block, before: "a\nb\nc", after: "x\ny\nz" };
    let on_blob = first_cut(&ctx, me, repo, old).await?;
    let r = ops::resolve_anchor(&ctx, &root, on_blob, "HEAD", None).await?;
    assert_eq!((r.label, r.method), (Label::Exact, Method::SameBlob));
    let view: CodeAnchorView = ctx.get(on_blob).await?;
    assert_eq!(view.basis_blob()?, head);
    assert_eq!(view.nearest_blob()?, head);
    assert_eq!(view.basis_patch()?.as_deref(), Some(""));
    let key = anchoring::anchor_key(&repo.to_base64(), "ws.rs", head.as_deref().unwrap(), Some((3, 8)));
    assert_eq!(view.anchor_key()?, Some(key.clone()));
    // Anchoring the same lines now finds it rather than making another.
    assert_eq!(anchor_in(&ctx, me, &root, Some((3, 8))).await?, on_blob);

    // Made on a dirty file: its text alone, until a version holds it exactly at its lines with
    // its context around it.
    let added = format!("{V1}fn added() {{}}\n");
    let old = FirstCut { lines: (11, 12), blob_sha: None, text: "fn added() {}", before: "x\ny\nz", after: "" };
    let on_text = first_cut(&ctx, me, repo, old).await?;
    let r = ops::resolve_anchor(&ctx, &root, on_text, "worktree", None).await?;
    assert_eq!(r.label, Label::Orphaned, "the working tree does not hold it yet");
    assert_eq!(ctx.get::<CodeAnchorView>(on_text).await?.anchor_key()?, None);
    std::fs::write(root.join("ws.rs"), &added)?;
    let r = ops::resolve_anchor(&ctx, &root, on_text, "worktree", None).await?;
    assert_eq!((r.label, r.method), (Label::Exact, Method::Fingerprint));
    let view: CodeAnchorView = ctx.get(on_text).await?;
    let basis = view.basis_blob()?.expect("the working tree's content became its basis");
    assert!(anchoring::blob_exists(&root, &basis), "and was written to the repository");
    assert!(view.anchor_key()?.is_some());
    // From now on it resolves by diff from that basis.
    git(&root, &["add", "ws.rs"])?;
    git(&root, &["commit", "-q", "-m", "added"])?;
    std::fs::write(root.join("ws.rs"), format!("top\n{added}"))?;
    let r = ops::resolve_anchor(&ctx, &root, on_text, "worktree", None).await?;
    assert_eq!((r.label, r.method, r.start_line), (Label::Shifted, Method::Diff, 12));
    Ok(())
}

/// What a command puts on the lines of `ws.rs` in `root` (the whole file without `lines`): the
/// highlight made already for them, or the one to make with it.
async fn highlight_in(ctx: &Context, me: EntityId, root: &Path, lines: Option<(i64, i64)>) -> Result<ops::Highlight> {
    let spec = match lines {
        Some((start, end)) => anchoring::form(root, "ws.rs", start, end)?,
        None => anchoring::form_whole_file(root, "ws.rs")?,
    };
    let repo = ops::ensure_repo(ctx, root).await?;
    ops::highlight(ctx, me, repo, &spec).await
}

/// A message on lines no highlight holds yet is written with its new highlight, and a step's
/// with the step too, in one transaction (`fwd ask --path`, `fwd comment --path`, `fwd step add
/// --path`): refused for a reference that names nothing, it leaves no highlight and no step
/// behind; accepted, it is on the highlight made with it. On a highlight made already, a refused
/// comment leaves no step either, and an accepted one comes beside the step.
#[tokio::test]
async fn a_message_refused_on_new_lines_leaves_no_highlight_and_no_step() -> Result<()> {
    let ctx = context().await?;
    let me = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let task = ops::create_task(&ctx, me, ops::CreateTask { title: "Tidy ws.rs".into(), ..Default::default() }).await?;
    let dir = tempfile::tempdir()?;
    let origin = upstream(dir.path())?;
    let root = clone(dir.path(), &origin, "work")?;
    let typo = "is @nobody-here watching this loop?";
    let asked = || ops::MessageContext { asks_project: true, ..Default::default() };
    let refused = |e: anyhow::Error| assert!(e.to_string().contains("names nothing"), "refused for the name: {e}");

    let on = highlight_in(&ctx, me, &root, Some((3, 8))).await?;
    assert!(matches!(on, ops::Highlight::New(_)), "no highlight holds the lines yet");
    refused(ops::post_on_highlight(&ctx, me, typo, Priority::Now, asked(), on).await.expect_err("a question"));
    let on = highlight_in(&ctx, me, &root, Some((3, 8))).await?;
    refused(
        ops::post_on_highlight(&ctx, me, typo, Priority::Next, Default::default(), on).await.expect_err("a comment"),
    );
    let on = highlight_in(&ctx, me, &root, Some((3, 8))).await?;
    refused(ops::add_highlight_step(&ctx, me, task, on, Some(typo)).await.expect_err("a step's comment"));
    assert!(ops::anchors_in_path(&ctx, "ws.rs").await?.is_empty(), "no highlight is left behind");
    assert!(ops::steps(&ctx, task).await?.is_empty(), "nor a step");

    let on = highlight_in(&ctx, me, &root, Some((3, 8))).await?;
    let (question, lines) = ops::post_on_highlight(&ctx, me, "why a loop?", Priority::Now, asked(), on).await?;
    let anchors: Vec<EntityId> = ops::anchors_in_path(&ctx, "ws.rs").await?.iter().map(|a| a.id()).collect();
    assert_eq!(anchors, vec![lines], "the question's highlight");
    let m: MessageView = ctx.get(question).await?;
    assert_eq!(m.anchor()?.map(EntityId::from), Some(lines), "the question is on it");

    let on = highlight_in(&ctx, me, &root, Some((3, 8))).await?;
    assert!(matches!(on, ops::Highlight::Made(id) if id == lines), "the lines are that highlight now");
    refused(ops::add_highlight_step(&ctx, me, task, on, Some(typo)).await.expect_err("a step's comment"));
    assert!(ops::steps(&ctx, task).await?.is_empty(), "a refused comment adds no step");
    let on = highlight_in(&ctx, me, &root, Some((3, 8))).await?;
    let (step, edge, comment) = ops::add_highlight_step(&ctx, me, task, on, Some("start here")).await?;
    assert_eq!(step, lines);
    let c: MessageView = ctx.get(comment.expect("a comment")).await?;
    assert_eq!(c.anchor()?.map(EntityId::from), Some(lines), "the comment beside the step");

    // A new highlight, its step and its comment.
    let on = highlight_in(&ctx, me, &root, None).await?;
    let (whole, second, comment) = ops::add_highlight_step(&ctx, me, task, on, Some("and the rest")).await?;
    assert_ne!(whole, lines);
    let steps: Vec<EntityId> = ops::steps(&ctx, task).await?.iter().map(|r| r.id()).collect();
    assert_eq!(steps, vec![edge, second], "the new highlight is the next step");
    let c: MessageView = ctx.get(comment.expect("a comment")).await?;
    assert_eq!(c.anchor()?.map(EntityId::from), Some(whole));
    Ok(())
}
