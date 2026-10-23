//! Operations and routing against an in-process durable node: the graph behaves as
//! docs/design/data-model.md and docs/design/agents-and-inbox.md say.

use ankurah::EntityId;
use ankurah::{policy::DEFAULT_CONTEXT, Context, Node, PermissiveAgent};
use ankurah_storage_sqlite::SqliteStorageEngine;
use anyhow::Result;
use fwd_core::model::*;
use fwd_core::ops::{self, CreateDecision, CreateScope, CreateTask, EditScope, MessageContext};
use fwd_core::routing;
use fwd_core::rules::{DecisionMove, NotReadyReason, Readiness, Verb};
use std::sync::Arc;

async fn context() -> Result<Context> {
    let storage = SqliteStorageEngine::open_in_memory().await?;
    let node = Node::new_durable(Arc::new(storage), PermissiveAgent::new());
    node.system.create().await?;
    Ok(node.context_async(DEFAULT_CONTEXT).await?)
}

struct Cast {
    daniel: ankurah::EntityId,
    implementer: ankurah::EntityId,
    triage: ankurah::EntityId,
    scope: ankurah::EntityId,
}

async fn cast(ctx: &Context) -> Result<Cast> {
    let daniel = ops::ensure_principal(ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    // Daniel belongs to the project, as `fwd init` makes the person who runs it belong.
    ops::join(ctx, daniel).await?;
    let implementer = ops::ensure_principal(
        ctx,
        "impl-alice",
        "Alice (implementer)",
        PrincipalKind::Agent,
        Some(AgentRole::Implementer),
    )
    .await?;
    // The agent that answers first for the whole project names itself, selects triage and is
    // endorsed for the whole project, as a small project with small scopes may endorse one.
    let triage = ops::add_principal(
        ctx,
        "triage-bot",
        Some(AgentRole::Triage),
        Some(ops::Serves::ProjectWide),
        None,
        None,
        None,
    )
    .await?;
    assert_eq!(ops::triage_for(ctx, None).await?.map(|f| f.agent), Some(triage), "it serves the whole project");
    assert_eq!(
        ops::ensure_principal(ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?,
        daniel,
        "ensure is idempotent"
    );
    let intention = "Get comments to agents mid-task".to_string();
    let scope = ops::create_scope(ctx, implementer, CreateScope { intention, ..named_scope("Inbox delivery") }).await?;
    Ok(Cast { daniel, implementer, triage, scope })
}

fn task(title: &str) -> CreateTask {
    CreateTask { title: title.into(), needs_review: true, ..Default::default() }
}

/// An active scope with a name and nothing else.
fn named_scope(title: &str) -> CreateScope {
    CreateScope { title: title.into(), ..Default::default() }
}

/// `ids` in one order whatever order they came in, to compare as a set.
fn sorted(mut ids: Vec<EntityId>) -> Vec<EntityId> {
    ids.sort();
    ids
}

/// Who a message notifies, and how, in recipient order.
async fn recipients(ctx: &Context, message: ankurah::EntityId) -> Result<Vec<(ankurah::EntityId, NotificationType)>> {
    let view: MessageView = ctx.get(message).await?;
    Ok(routing::plan_for_message(ctx, &view).await?.iter().map(|p| (p.recipient, p.notification_type)).collect())
}

#[tokio::test]
async fn dependencies_readiness_claims_and_review() -> Result<()> {
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let a = ops::create_task(&ctx, c.daniel, task("A: connector")).await?;
    let b = ops::create_task(&ctx, c.daniel, CreateTask { after: vec![a], ..task("B: retry") }).await?;

    let bv: TaskView = ctx.get(b).await?;
    assert_eq!(ops::readiness(&ctx, &bv).await?, Readiness::NotReady(vec![NotReadyReason::BlockedBy(a)]));
    let av: TaskView = ctx.get(a).await?;
    assert!(ops::readiness(&ctx, &av).await?.is_ready());

    // A cycle is refused with the cycle named.
    let err =
        ops::link(&ctx, c.daniel, RelationKind::DependsOn, a, b, None, false, serde_json::json!({})).await.unwrap_err();
    assert!(err.to_string().contains("cycle"), "{err}");

    // The implementer's frontier shows A only.
    let ready = ops::ready_tasks(&ctx, Some(c.implementer)).await?;
    assert_eq!(ready.iter().map(|t| t.id()).collect::<Vec<_>>(), vec![a]);

    // The implementer starts A in her scope: she is its assignee and it is worked there; no one
    // starts it again.
    ops::claim(&ctx, c.implementer, a, c.scope).await?;
    let other_scope = ops::create_scope(&ctx, c.daniel, named_scope("Daniel by hand")).await?;
    let err = ops::claim(&ctx, c.daniel, a, other_scope).await.unwrap_err();
    assert!(err.to_string().contains("only an open task"), "{err}");
    assert_eq!(ops::worked_in(&ctx, a).await?, vec![c.scope]);
    assert_eq!(ops::assignee_of(&ctx.get::<TaskView>(a).await?)?, Some(c.implementer));
    assert!(ops::ready_tasks(&ctx, Some(c.daniel)).await?.is_empty(), "A is taken, B is blocked");

    // Submit with evidence; the submitter cannot accept; Daniel can; B becomes ready.
    assert_eq!(
        ops::transition(&ctx, c.implementer, a, Verb::Submit, Some("commit 123, tests green")).await?,
        TaskStatus::InReview
    );
    let err = ops::transition(&ctx, c.implementer, a, Verb::Accept, None).await.unwrap_err();
    assert!(err.to_string().contains("own submission"), "{err}");
    assert_eq!(ops::transition(&ctx, c.daniel, a, Verb::Accept, Some("looks right")).await?, TaskStatus::Done);
    let av: TaskView = ctx.get(a).await?;
    assert!(av.closed()?.is_some());
    let bv: TaskView = ctx.get(b).await?;
    assert!(ops::readiness(&ctx, &bv).await?.is_ready());

    // Amend appends to why without losing the original.
    ops::amend(&ctx, b, ops::Amend { why: Some("first reason".into()), ..Default::default() }).await?;
    ops::amend(&ctx, b, ops::Amend { why_append: Some("second reason".into()), ..Default::default() }).await?;
    let bv: TaskView = ctx.get(b).await?;
    assert_eq!(bv.why()?, "first reason\n\nsecond reason");

    // Proposed tasks and relations are approved or rejected, never started directly.
    let p = ops::create_task(&ctx, c.implementer, CreateTask { propose: true, ..task("P: proposed") }).await?;
    let err = ops::claim(&ctx, c.implementer, p, c.scope).await.unwrap_err();
    assert!(err.to_string().contains("proposed"), "{err}");
    assert_eq!(ops::transition(&ctx, c.triage, p, Verb::Approve, None).await?, TaskStatus::Open);
    let rel = ops::link(
        &ctx,
        c.implementer,
        RelationKind::DependsOn,
        p,
        b,
        Some("needs retry first"),
        true,
        serde_json::json!({}),
    )
    .await?;
    ops::decide_relation(&ctx, c.triage, rel, true, None).await?;
    let pv: TaskView = ctx.get(p).await?;
    assert_eq!(ops::readiness(&ctx, &pv).await?, Readiness::NotReady(vec![NotReadyReason::BlockedBy(b)]));
    assert_eq!(ops::unlink(&ctx, c.daniel, RelationKind::DependsOn, p, b, "not needed").await?, 1);
    assert!(ops::readiness(&ctx, &pv).await?.is_ready());
    Ok(())
}

/// The comments on `task` that keep why it moved, oldest first: each one's author, its text and
/// the move its `context.move` names.
async fn move_comments(ctx: &Context, task: EntityId) -> Result<Vec<(EntityId, String, String)>> {
    let mut found = Vec::new();
    for m in ops::messages_about(ctx, "task", task).await? {
        let context: serde_json::Value = m.context()?.into();
        if let Some(mv) = context.get(ops::MOVE_KEY).and_then(|v| v.as_str()) {
            found.push((EntityId::from(m.author()?), ops::text(m.content())?, mv.to_string()));
        }
    }
    Ok(found)
}

#[tokio::test]
async fn a_task_moves_reason_is_kept_as_a_comment_by_whoever_moved_it() -> Result<()> {
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let a = ops::create_task(&ctx, c.daniel, task("A: connector")).await?;
    ops::claim(&ctx, c.implementer, a, c.scope).await?;
    // A submission keeps its evidence in the task's context, and no comment repeats it.
    ops::transition(&ctx, c.implementer, a, Verb::Submit, Some("commit 123, tests green")).await?;
    assert!(move_comments(&ctx, a).await?.is_empty(), "the evidence is not a comment");
    // A return says what should change, as a comment on the task by its reviewer, which reaches
    // the task's assignee and its triage agent as any comment on it does, and not its author.
    ops::transition(&ctx, c.daniel, a, Verb::Return, Some("cover the retry too")).await?;
    let returned = move_comments(&ctx, a).await?;
    assert_eq!(returned, vec![(c.daniel, "Returned: cover the retry too".to_string(), "returned".to_string())]);
    let comment = ops::messages_about(&ctx, "task", a).await?.pop().expect("the comment").id();
    let mut told = recipients(&ctx, comment).await?;
    told.sort();
    let mut expected =
        vec![(c.implementer, NotificationType::TaskCommented), (c.triage, NotificationType::TaskCommented)];
    expected.sort();
    assert_eq!(told, expected);
    // A cancel's reason is a comment and still the task's closed reason; a reopen's is a comment,
    // and the closed reason goes with the close.
    ops::transition(&ctx, c.daniel, a, Verb::Cancel, Some("folded into B")).await?;
    let av: TaskView = ctx.get(a).await?;
    assert_eq!(av.closed_reason()?.as_deref(), Some("folded into B"));
    ops::transition(&ctx, c.daniel, a, Verb::Reopen, Some("B fell through")).await?;
    let av: TaskView = ctx.get(a).await?;
    assert_eq!(av.closed_reason()?, None);
    // A rejustify's new reason, then a reject's, each a comment by whoever made the move.
    ops::transition(&ctx, c.implementer, a, Verb::Rejustify, Some("not sure it is needed now")).await?;
    ops::transition(&ctx, c.triage, a, Verb::Reject, Some("B covers it")).await?;
    let moves: Vec<(EntityId, String)> =
        move_comments(&ctx, a).await?.into_iter().skip(1).map(|(author, _, mv)| (author, mv)).collect();
    assert_eq!(
        moves,
        vec![
            (c.daniel, "cancelled".to_string()),
            (c.daniel, "reopened".to_string()),
            (c.implementer, "rejustified".to_string()),
            (c.triage, "rejected".to_string()),
        ]
    );
    let av: TaskView = ctx.get(a).await?;
    assert_eq!(av.closed_reason()?.as_deref(), Some("B covers it"));
    // A reason naming nothing the project has refuses the move, as a post does; the task stays.
    let b = ops::create_task(&ctx, c.daniel, task("B: retry")).await?;
    let err = ops::transition(&ctx, c.daniel, b, Verb::Cancel, Some("see [[zzzzzzzz]]")).await.unwrap_err();
    assert!(!err.to_string().is_empty());
    let bv: TaskView = ctx.get(b).await?;
    assert_eq!(bv.status()?, TaskStatus::Open, "the move was not made");
    assert!(move_comments(&ctx, b).await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn messages_route_to_the_people_who_need_to_know() -> Result<()> {
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let a = ops::create_task(&ctx, c.daniel, task("A")).await?;
    ops::claim(&ctx, c.implementer, a, c.scope).await?;

    // Daniel comments on A: the implementer (scope owner) and the triage agent hear, Daniel does not.
    let m1 = ops::post_message(
        &ctx,
        c.daniel,
        "what if we retried on disconnect?",
        Priority::Next,
        MessageContext { task_id: Some(a), ..Default::default() },
    )
    .await?;
    let m1v: MessageView = ctx.get(m1).await?;
    let planned = routing::plan_for_message(&ctx, &m1v).await?;
    let mut recipients: Vec<_> = planned.iter().map(|p| (p.recipient, p.notification_type)).collect();
    recipients.sort();
    let mut expected =
        vec![(c.implementer, NotificationType::TaskCommented), (c.triage, NotificationType::TaskCommented)];
    expected.sort();
    assert_eq!(recipients, expected);
    routing::apply(&ctx, &planned).await?;
    let inbox = ops::inbox(&ctx, c.implementer, true).await?;
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].notification_type()?, NotificationType::TaskCommented);
    assert_eq!(serde_json::Value::from(inbox[0].target()?)["id"], a.to_base64());
    assert!(ops::inbox(&ctx, c.daniel, true).await?.is_empty());

    // A question asked of the project reaches its triage agent only.
    let q = ops::post_message(
        &ctx,
        c.implementer,
        "should B wait for A?",
        Priority::Now,
        MessageContext { asks_project: true, task_id: Some(a), ..Default::default() },
    )
    .await?;
    let qv: MessageView = ctx.get(q).await?;
    let planned = routing::plan_for_message(&ctx, &qv).await?;
    assert_eq!(planned.len(), 1);
    assert_eq!((planned[0].recipient, planned[0].notification_type), (c.triage, NotificationType::QuestionAsked));

    // A reply reaches the thread's other participants; mentions reach the mentioned.
    let r = ops::post_message(
        &ctx,
        c.implementer,
        "added backoff; @dnorman please look",
        Priority::Next,
        MessageContext { reply_to: Some(m1), ..Default::default() },
    )
    .await?;
    let rv: MessageView = ctx.get(r).await?;
    assert_eq!(rv.thread()?.map(ankurah::EntityId::from), Some(m1));
    assert_eq!(rv.task()?.map(ankurah::EntityId::from), Some(a), "the subject is carried into the reply");
    let planned = routing::plan_for_message(&ctx, &rv).await?;
    let mut recipients: Vec<_> = planned.iter().map(|p| (p.recipient, p.notification_type)).collect();
    recipients.sort();
    let mut expected = vec![(c.daniel, NotificationType::Replied), (c.triage, NotificationType::TaskCommented)];
    expected.sort();
    assert_eq!(
        recipients, expected,
        "Daniel is a participant, so Replied wins over Mentioned; the triage agent sees a task comment"
    );

    // The triage agent creates a proposed task from Daniel's message: Daniel is told the task exists.
    let t =
        ops::create_task(&ctx, c.triage, CreateTask { propose: true, from: Some(m1), ..task("Retry on disconnect") })
            .await?;
    let tv: TaskView = ctx.get(t).await?;
    let planned = routing::plan_for_task_created(&ctx, &tv).await?;
    assert_eq!(planned.len(), 1);
    assert_eq!((planned[0].recipient, planned[0].notification_type), (c.daniel, NotificationType::TaskCreated));
    assert_eq!(planned[0].originator["message_id"], m1.to_base64());

    // Submit gives the review to the triage agent (the project has one); closing A unblocks its
    // dependents' owners.
    let b = ops::create_task(&ctx, c.daniel, CreateTask { after: vec![a], ..task("B") }).await?;
    // Views are live: capture the previous status as a value before transitioning.
    let av: TaskView = ctx.get(a).await?;
    let previous = av.status()?;
    ops::transition(&ctx, c.implementer, a, Verb::Submit, Some("done")).await?;
    let av2: TaskView = ctx.get(a).await?;
    assert_eq!(av.status()?, TaskStatus::InReview, "a view reflects the entity's current state");
    let planned = routing::plan_for_task_status(&ctx, &av2, previous, Some(c.implementer)).await?;
    assert_eq!(
        planned.iter().map(|p| (p.recipient, p.notification_type)).collect::<Vec<_>>(),
        vec![(c.triage, NotificationType::ReviewRequested)]
    );
    ops::transition(&ctx, c.daniel, a, Verb::Accept, None).await?;
    let av3: TaskView = ctx.get(a).await?;
    let planned = routing::plan_for_task_status(&ctx, &av3, TaskStatus::InReview, Some(c.daniel)).await?;
    assert_eq!(
        planned.iter().map(|p| (p.recipient, p.notification_type)).collect::<Vec<_>>(),
        vec![(c.triage, NotificationType::TaskUnblocked)],
        "B is unassigned, so the triage agent hears"
    );
    assert_eq!(planned[0].target["id"], b.to_base64());

    // Reading and dismissing.
    let ids: Vec<_> = ops::inbox(&ctx, c.implementer, true).await?.iter().map(|n| n.id()).collect();
    ops::set_notification_status(&ctx, &ids, NotificationStatus::Read).await?;
    assert!(ops::inbox(&ctx, c.implementer, true).await?.is_empty());
    assert_eq!(ops::inbox(&ctx, c.implementer, false).await?.len(), ids.len());
    Ok(())
}

/// The notifications a client holds are marked in one transaction, without fetching each again
/// (task taRWSL2-): the page's Mark all read fetched each of Daniel's 212 from the daemon first,
/// one round trip at a time, and they stayed unread. A live query on the inbox hears each
/// transaction as one change set, so the whole inbox marked read is heard once, and only the
/// notifications given change.
#[tokio::test]
async fn the_notifications_a_client_holds_are_marked_in_one_transaction() -> Result<()> {
    use ankurah::changes::ChangeSet;
    use ankurah::signals::Subscribe;
    use std::sync::Mutex;

    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let notify = |recipient: EntityId, n: usize| {
        let ctx = ctx.clone();
        async move {
            ops::create_notification(
                &ctx,
                recipient,
                NotificationType::QuestionAsked,
                Priority::Next,
                serde_json::json!({"type": "Message", "id": recipient.to_base64(), "n": n}),
                serde_json::json!({"actor_id": recipient.to_base64(), "why": "addressee"}),
            )
            .await
        }
    };
    for n in 0..220 {
        notify(c.daniel, n).await?;
    }
    let elsewhere = notify(c.implementer, 0).await?;
    let held = ops::inbox(&ctx, c.daniel, true).await?;
    assert_eq!(held.len(), 220);

    let inbox = ctx.query::<NotificationView>(fwd_core::query::with_id("recipient = ?", c.daniel)?)?;
    inbox.wait_initialized().await?;
    let heard = Arc::new(Mutex::new(Vec::new()));
    let _listening = inbox.subscribe({
        let heard = heard.clone();
        move |changes: ChangeSet<NotificationView>| heard.lock().unwrap().push(changes.changes.len())
    });
    ops::set_status_of(&ctx, &held, NotificationStatus::Read).await?;
    assert_eq!(*heard.lock().unwrap(), vec![220], "one transaction, changing each notification given");
    assert!(ops::inbox(&ctx, c.daniel, true).await?.is_empty());
    assert_eq!(ops::inbox(&ctx, c.daniel, false).await?.len(), 220, "read, not dismissed");
    let other: NotificationView = ctx.get(elsewhere).await?;
    assert_eq!(other.status()?, NotificationStatus::Unread, "a notification not given stays as it was");
    Ok(())
}

/// An anchor on `lines` of `path` (0-based, the end exclusive), or on the whole file with none.
/// Routing reads only where an anchor is, so the content it was made on is made up.
async fn anchor(
    ctx: &Context,
    by: EntityId,
    repo: EntityId,
    path: &str,
    lines: Option<(i64, i64)>,
) -> Result<EntityId> {
    ops::create_anchor(ctx, by, repo, &anchor_spec(path, lines)).await
}

/// What [`anchor`] makes an anchor from.
fn anchor_spec(path: &str, lines: Option<(i64, i64)>) -> fwd_core::anchoring::AnchorSpec {
    fwd_core::anchoring::AnchorSpec {
        path: path.into(),
        lines,
        basis_blob: Some("5f1c0a1be4f2d1fbd1f3c1b0e0f1a2b3c4d5e6f7".into()),
        head_blob: None,
        index_blob: None,
        nearest_blob: None,
        basis_patch: None,
        pieces: Vec::new(),
        commit_sha: None,
        legacy_blob: None,
        anchored_text: String::new(),
        context_before: String::new(),
        context_after: String::new(),
        side: DiffSide::Worktree,
    }
}

/// A comment on a whole file is a comment on every line of it (docs/design/agents-and-inbox.md,
/// rule 1, a comment on code): it reaches the assignee of a task anchored anywhere in the file,
/// however deep, and a comment on any line reaches the assignee of a task anchored on the whole
/// file. Lines further apart than `routing::NEARBY_LINES` still do not
/// reach each other. A whole-file anchor stores its lines as 0 and 0, which used to put it at the
/// top of the file.
#[tokio::test]
async fn a_comment_on_a_whole_file_reaches_whoever_works_any_part_of_it() -> Result<()> {
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let bob =
        ops::ensure_principal(&ctx, "impl-bob", "Bob", PrincipalKind::Agent, Some(AgentRole::Implementer)).await?;
    let bobs = ops::create_scope(&ctx, bob, named_scope("Parser docs")).await?;
    let repo = EntityId::random();
    let anchored = |ctx: Context, task: EntityId, anchor: EntityId| async move {
        ops::link(&ctx, c.daniel, RelationKind::AnchoredAt, task, anchor, None, false, serde_json::json!({})).await
    };
    // The implementer works a task anchored 400 lines into the file, Bob one on the whole file.
    let deep = anchor(&ctx, c.daniel, repo, "src/parser.rs", Some((400, 410))).await?;
    let split = ops::create_task(&ctx, c.daniel, task("Split the parser")).await?;
    anchored(ctx.clone(), split, deep).await?;
    ops::claim(&ctx, c.implementer, split, c.scope).await?;
    let whole = anchor(&ctx, c.daniel, repo, "src/parser.rs", None).await?;
    let document = ops::create_task(&ctx, c.daniel, task("Document the parser")).await?;
    anchored(ctx.clone(), document, whole).await?;
    ops::claim(&ctx, bob, document, bobs).await?;

    // Who hears a comment by Daniel on `anchor` because a task near it is assigned to them, and
    // which task that is.
    let near = |ctx: Context, anchor: EntityId| async move {
        let on = MessageContext { anchor_id: Some(anchor), ..Default::default() };
        let m = ops::post_message(&ctx, c.daniel, "this file is getting long", Priority::Next, on).await?;
        let view: MessageView = ctx.get(m).await?;
        let mut heard: Vec<(EntityId, String)> = routing::plan_for_message(&ctx, &view)
            .await?
            .into_iter()
            .filter(|p| p.originator["why"] == "assignee")
            .map(|p| (p.recipient, p.originator["near_task_id"].as_str().unwrap_or_default().to_string()))
            .collect();
        heard.sort();
        anyhow::Ok(heard)
    };
    let both = {
        let mut both = vec![(c.implementer, split.to_base64()), (bob, document.to_base64())];
        both.sort();
        both
    };
    assert_eq!(near(ctx.clone(), whole).await?, both, "the whole file reaches the task 400 lines down");
    let top = anchor(&ctx, c.daniel, repo, "src/parser.rs", Some((0, 3))).await?;
    assert_eq!(
        near(ctx.clone(), top).await?,
        vec![(bob, document.to_base64())],
        "the top of the file is part of the whole, and 397 lines from the task below"
    );
    let beside = anchor(&ctx, c.daniel, repo, "src/parser.rs", Some((380, 385))).await?;
    assert_eq!(near(ctx.clone(), beside).await?, both);
    let elsewhere = anchor(&ctx, c.daniel, repo, "src/lexer.rs", None).await?;
    assert!(near(ctx.clone(), elsewhere).await?.is_empty(), "another file is not part of this one");
    Ok(())
}

/// A scope has no owner (docs/DECISIONS.md, D108, "A scope has no owner: scopes are not assigned to
/// principals"): a comment on code near a task's lines reaches whoever is assigned the task, not
/// whoever made the scope it is worked in, and reaches no one that way while the task has no
/// assignee; a comment on a scope reaches the project's triage agent, not whoever made the scope.
#[tokio::test]
async fn a_comment_near_a_task_reaches_its_assignee_and_one_on_a_scope_the_triage_agent() -> Result<()> {
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let bob =
        ops::ensure_principal(&ctx, "impl-bob", "Bob", PrincipalKind::Agent, Some(AgentRole::Implementer)).await?;
    let repo = EntityId::random();
    let retried = anchor(&ctx, c.daniel, repo, "src/retry.rs", Some((10, 20))).await?;
    let backoff = ops::create_task(&ctx, c.daniel, task("Back off between retries")).await?;
    ops::link(&ctx, c.daniel, RelationKind::AnchoredAt, backoff, retried, None, false, serde_json::json!({})).await?;
    // The task is worked in the implementer's scope, but no one is assigned it yet.
    ops::link(&ctx, c.daniel, RelationKind::WorkedIn, backoff, c.scope, None, false, serde_json::json!({})).await?;

    let on_code = |ctx: Context| async move {
        let near = anchor(&ctx, c.daniel, repo, "src/retry.rs", Some((12, 14))).await?;
        let on = MessageContext { anchor_id: Some(near), ..Default::default() };
        let m = ops::post_message(&ctx, c.daniel, "this loop never sleeps", Priority::Next, on).await?;
        let view: MessageView = ctx.get(m).await?;
        let mut heard: Vec<(EntityId, String, String)> = routing::plan_for_message(&ctx, &view)
            .await?
            .into_iter()
            .map(|p| {
                let near = p.originator["near_task_id"].as_str().unwrap_or_default().to_string();
                (p.recipient, p.originator["why"].as_str().unwrap_or_default().to_string(), near)
            })
            .collect();
        heard.sort();
        anyhow::Ok(heard)
    };
    assert_eq!(
        on_code(ctx.clone()).await?,
        vec![(c.triage, "triage".to_string(), String::new())],
        "an unassigned task nearby reaches no one, whoever made its scope"
    );
    // Bob starts it in the implementer's scope, which anyone may work in: he hears the next
    // comment, and the scope's maker does not.
    ops::claim(&ctx, bob, backoff, c.scope).await?;
    let mut expected =
        vec![(c.triage, "triage".to_string(), String::new()), (bob, "assignee".to_string(), backoff.to_base64())];
    expected.sort();
    assert_eq!(on_code(ctx.clone()).await?, expected);

    let on_scope = MessageContext { scope_id: Some(c.scope), ..Default::default() };
    let m = ops::post_message(&ctx, c.daniel, "is this scope still active?", Priority::Next, on_scope).await?;
    assert_eq!(
        reasons(&ctx, m).await?,
        vec![(c.triage, NotificationType::TaskCommented, "triage".to_string())],
        "a comment on a scope reaches the triage agent, not whoever made the scope"
    );
    Ok(())
}

/// A message always notifies the party to which it was directed, even when that party wrote it
/// (Daniel, 2026-09-26: "it should always notify the party to which the question was directed");
/// every other rule still leaves the author out.
#[tokio::test]
async fn a_question_always_notifies_the_party_it_was_directed_to() -> Result<()> {
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let a = ops::create_task(&ctx, c.daniel, task("A")).await?;
    ops::claim(&ctx, c.implementer, a, c.scope).await?;

    // Daniel asks himself a question, addressed the way `fwd ask --to @dnorman` and the page's Ask
    // box address one (his id in `to`): it waits in his own inbox.
    let q = ops::post_message(
        &ctx,
        c.daniel,
        "does a question asked of myself reach my inbox?",
        Priority::Later,
        MessageContext { to: Some(c.daniel.to_base64()), ..Default::default() },
    )
    .await?;
    assert_eq!(recipients(&ctx, q).await?, vec![(c.daniel, NotificationType::QuestionAsked)]);

    // Alice comments on her own task: the triage agent hears; she does not, though she owns the scope
    // the task is assigned to.
    let m = ops::post_message(
        &ctx,
        c.implementer,
        "starting with the backoff",
        Priority::Next,
        MessageContext { task_id: Some(a), ..Default::default() },
    )
    .await?;
    assert_eq!(recipients(&ctx, m).await?, vec![(c.triage, NotificationType::TaskCommented)]);

    // Daniel answers his own question: the reply reaches no one, since he is the thread's only
    // participant.
    let r = ops::post_message(
        &ctx,
        c.daniel,
        "it does",
        Priority::Next,
        MessageContext { reply_to: Some(q), ..Default::default() },
    )
    .await?;
    assert!(recipients(&ctx, r).await?.is_empty(), "the author of a reply is not notified of it");

    // The triage agent asking itself hears its own question, as anyone does (D31).
    let s = ops::post_message(
        &ctx,
        c.triage,
        "who approves relations while Daniel is away?",
        Priority::Next,
        MessageContext { to: Some(c.triage.to_base64()), ..Default::default() },
    )
    .await?;
    assert_eq!(recipients(&ctx, s).await?, vec![(c.triage, NotificationType::QuestionAsked)]);
    Ok(())
}

/// A question about lines of code (`fwd ask --path --lines`, which the editor's Ask About These
/// Lines… runs) asks the project, whose triage agent hears it, and no one else: not the assignee of
/// a task anchored nearby, whom a comment on that code reaches (task _4rjtRz9).
/// The triage agent hears it once, as the question.
#[tokio::test]
async fn a_question_on_lines_asks_the_project_alone() -> Result<()> {
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let repo = EntityId::random();
    let retried = anchor(&ctx, c.daniel, repo, "src/retry.rs", Some((10, 20))).await?;
    let backoff = ops::create_task(&ctx, c.daniel, task("Back off between retries")).await?;
    ops::link(&ctx, c.daniel, RelationKind::AnchoredAt, backoff, retried, None, false, serde_json::json!({})).await?;
    ops::claim(&ctx, c.implementer, backoff, c.scope).await?;

    let asked_about = anchor(&ctx, c.daniel, repo, "src/retry.rs", Some((12, 14))).await?;
    let question = ops::post_message(
        &ctx,
        c.daniel,
        "why does this loop never sleep?",
        Priority::Now,
        MessageContext { asks_project: true, anchor_id: Some(asked_about), ..Default::default() },
    )
    .await?;
    assert_eq!(reasons(&ctx, question).await?, vec![(c.triage, NotificationType::QuestionAsked, "triage".into())]);
    // A comment on the same lines reaches whoever works nearby, as the question did not.
    let comment = ops::post_message(
        &ctx,
        c.daniel,
        "this loop never sleeps",
        Priority::Next,
        MessageContext { anchor_id: Some(asked_about), ..Default::default() },
    )
    .await?;
    let mut heard = reasons(&ctx, comment).await?;
    heard.sort();
    let mut expected = vec![
        (c.triage, NotificationType::CodeCommented, "triage".to_string()),
        (c.implementer, NotificationType::CodeCommented, "assignee".to_string()),
    ];
    expected.sort();
    assert_eq!(heard, expected);
    Ok(())
}

/// A message reaches only whom it is addressed to, and whom it mentions (docs/DECISIONS.md, D111,
/// "A message reaches only whom it is addressed to; the triage agent is the front line for questions
/// directed to it, without distracting the implementer"; task _4rjtRz9): a question to a person
/// about a task reaches that person only, and a question asked of the project about a task its
/// triage agent only, neither reaching the task's assignee; a comment on the task that asks no one
/// reaches its assignee and the triage agent. With no agent holding triage, a question asked of the
/// project reaches no one, and waits.
#[tokio::test]
async fn a_message_reaches_only_whom_it_is_addressed_to() -> Result<()> {
    assert_eq!(
        routing::ADDRESSED_GIVES,
        &[NotificationType::QuestionAsked, NotificationType::Mentioned],
        "a message addressed to someone gives the question to them and a mention to whom it names, nothing else"
    );
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let a = ops::create_task(&ctx, c.daniel, task("Retry on disconnect")).await?;
    ops::claim(&ctx, c.implementer, a, c.scope).await?;
    let bob =
        ops::ensure_principal(&ctx, "impl-bob", "Bob", PrincipalKind::Agent, Some(AgentRole::Implementer)).await?;
    let asked = MessageContext { to: Some(c.daniel.to_base64()), task_id: Some(a), ..Default::default() };
    let question = ops::post_message(&ctx, bob, "is Windows in scope?", Priority::Now, asked.clone()).await?;
    assert_eq!(reasons(&ctx, question).await?, vec![(c.daniel, NotificationType::QuestionAsked, "addressee".into())]);
    // A question asked of no one in particular about the task: it asks the scope the task is worked
    // in, which no agent serves, so the agent that serves the whole project hears it alone, not
    // Alice, whose task it is.
    let of_project = MessageContext { asks_project: true, task_id: Some(a), ..Default::default() };
    let triaged =
        ops::post_message(&ctx, bob, "who should take the Windows part?", Priority::Now, of_project.clone()).await?;
    assert_eq!(reasons(&ctx, triaged).await?, vec![(c.triage, NotificationType::QuestionAsked, "triage".into())]);
    let stored: MessageView = ctx.get(triaged).await?;
    assert_eq!(
        serde_json::Value::from(stored.context()?),
        serde_json::json!({"asks": "project", "asks_scope": c.scope.to_base64()}),
        "as a reader sees it"
    );
    // A message asks one principal or the project, never both.
    let both = MessageContext { to: Some(c.daniel.to_base64()), ..of_project.clone() };
    let refused = ops::post_message(&ctx, bob, "both?", Priority::Now, both).await.unwrap_err().to_string();
    assert_eq!(refused, "a message asks one principal or the project, not both");
    // Whom it mentions hears of it too, and no one else.
    let naming = ops::post_message(&ctx, bob, "is Windows in scope, @triage-bot?", Priority::Now, asked).await?;
    let mut heard = reasons(&ctx, naming).await?;
    heard.sort();
    let mut expected = vec![
        (c.daniel, NotificationType::QuestionAsked, "addressee".to_string()),
        (c.triage, NotificationType::Mentioned, "mentioned".to_string()),
    ];
    expected.sort();
    assert_eq!(heard, expected);
    // A plain comment on the task still reaches its assignee and the triage agent.
    let comment = ops::post_message(
        &ctx,
        bob,
        "Windows needs its own reconnect path",
        Priority::Next,
        MessageContext { task_id: Some(a), ..Default::default() },
    )
    .await?;
    let mut heard = reasons(&ctx, comment).await?;
    heard.sort();
    let mut expected = vec![
        (c.implementer, NotificationType::TaskCommented, "assignee".to_string()),
        (c.triage, NotificationType::TaskCommented, "triage".to_string()),
    ];
    expected.sort();
    assert_eq!(heard, expected);
    // With the triage agent retired, a question asked of the project reaches no one: it waits,
    // listed on the Questions page as asked of the project.
    ops::retire_principal(&ctx, "triage-bot").await?;
    let waiting = ops::post_message(&ctx, bob, "anyone?", Priority::Now, of_project).await?;
    assert!(reasons(&ctx, waiting).await?.is_empty(), "no one holds triage");
    Ok(())
}

/// The questions that waited reach the triage agent that comes to answer first for them (rule 5;
/// task 5RoOJZl9; per scope since docs/DECISIONS.md, D122): `routing::plan_for_waiting_questions`
/// plans, for each active question asked of no one in particular that no triage agent has heard of,
/// a `QuestionAsked` with why triage to the triage agent that answers first for the scope it asks
/// now, oldest first, recorded as `plan_for_message` records one; nothing for a question whose
/// scope no agent answers for, nor for a question asked of someone, a comment, or a question
/// resolved. An agent that takes a scope hears of that scope's questions, and one endorsed for the
/// whole project of those asked of no scope and of scopes no agent serves. A question told of is
/// heard, and not planned again, one routed before routing recorded why among them; one that a
/// triage agent retired with no successor left unread is heard by no one, and is planned for the
/// next agent that answers for it.
#[tokio::test]
async fn the_questions_that_waited_reach_the_agent_that_comes_to_answer_for_their_scope() -> Result<()> {
    let ctx = context().await?;
    let daniel = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    ops::add_principal(&ctx, "impl-alice", Some(AgentRole::Implementer), None, None, None, None).await?;
    let bob = ops::add_principal(&ctx, "impl-bob", Some(AgentRole::Implementer), None, None, None, None).await?;
    let replication = ops::create_scope(&ctx, daniel, named_scope("Replication")).await?;
    let question = |id: EntityId| serde_json::json!({"type": "Message", "id": id.to_base64()});
    let retry = ops::create_task(&ctx, daniel, task("Retry on disconnect")).await?;
    let of_project = MessageContext { asks_project: true, ..Default::default() };
    let about = MessageContext { task_id: Some(retry), ..of_project.clone() };
    let first = ops::post_message(&ctx, daniel, "who takes the TLS part?", Priority::Now, about).await?;
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let second = ops::post_message(&ctx, bob, "is Windows in scope?", Priority::Later, of_project.clone()).await?;
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let of_replication = MessageContext { asks_scope: Some(replication), ..of_project.clone() };
    let scoped = ops::post_message(&ctx, bob, "does the relay replicate?", Priority::Next, of_replication).await?;
    // None of these waits for triage: a question asked of Daniel, a comment, a question resolved.
    let to_daniel = MessageContext { to: Some(daniel.to_base64()), ..Default::default() };
    ops::post_message(&ctx, bob, "can I take it?", Priority::Next, to_daniel).await?;
    let on_task = MessageContext { task_id: Some(retry), ..Default::default() };
    ops::post_message(&ctx, daniel, "retries back off", Priority::Next, on_task).await?;
    let settled = ops::post_message(&ctx, bob, "does it build?", Priority::Next, of_project.clone()).await?;
    ops::resolve_message(&ctx, daniel, settled).await?;
    // Nor does a question to the supervisor role, asked of the project since `fwd admin migrate`,
    // that reached a supervisor, still active, before routing recorded why.
    let migrated = ops::post_message(&ctx, bob, "who owns the relay?", Priority::Next, of_project).await?;
    let supervisor =
        ops::add_principal(&ctx, "supervisor-1", Some(AgentRole::Reviewer), None, None, None, None).await?;
    let before_why = serde_json::json!({"message_id": migrated.to_base64(), "actor_id": bob.to_base64()});
    let reached = question(migrated);
    ops::create_notification(&ctx, supervisor, NotificationType::QuestionAsked, Priority::Next, reached, before_why)
        .await?;
    assert!(routing::plan_for_waiting_questions(&ctx).await?.is_empty(), "no triage agent answers for any of them");

    // An agent takes Replication: it hears of Replication's question, and of no other.
    let repl = ops::add_principal(
        &ctx,
        "triage-repl",
        Some(AgentRole::Triage),
        Some(ops::Serves::Scope(replication)),
        None,
        None,
        None,
    )
    .await?;
    let planned = routing::plan_for_waiting_questions(&ctx).await?;
    assert_eq!(
        planned.iter().map(|p| (p.recipient, p.target.clone())).collect::<Vec<_>>(),
        vec![(repl, question(scoped))]
    );
    assert_eq!(planned[0].originator["triage_scope_id"], replication.to_base64(), "naming the scope it triages");
    routing::apply(&ctx, &planned).await?;
    // An agent endorsed for the whole project hears of those asked of no scope.
    let triage = ops::add_principal(
        &ctx,
        "triage-bot",
        Some(AgentRole::Triage),
        Some(ops::Serves::ProjectWide),
        None,
        None,
        None,
    )
    .await?;
    let planned = routing::plan_for_waiting_questions(&ctx).await?;
    assert_eq!(
        planned.iter().map(|p| (p.recipient, p.notification_type, p.priority, p.target.clone())).collect::<Vec<_>>(),
        vec![
            (triage, NotificationType::QuestionAsked, Priority::Now, question(first)),
            (triage, NotificationType::QuestionAsked, Priority::Later, question(second)),
        ],
        "oldest first, each at the priority it was asked at"
    );
    assert_eq!(
        planned[0].originator,
        serde_json::json!({
            "message_id": first.to_base64(),
            "actor_id": daniel.to_base64(),
            "task_id": retry.to_base64(),
            "why": "triage",
        }),
        "who asked it, what about, and why the triage agent hears it"
    );

    // Told of them, the triage agents have heard them: nothing is planned again.
    routing::apply(&ctx, &planned).await?;
    assert!(routing::plan_for_waiting_questions(&ctx).await?.is_empty());
    // triage-bot reads the first, and is retired with no successor: the second, unread in an inbox
    // no agent reads now, waits, since no agent answers for what concerns no scope, and reaches the
    // agent endorsed for the whole project next; the first does not.
    let told = ops::inbox(&ctx, triage, true).await?;
    let read: Vec<EntityId> = told
        .iter()
        .filter(|n| serde_json::Value::from(n.originator().unwrap())["message_id"] == first.to_base64())
        .map(|n| n.id())
        .collect();
    ops::set_notification_status(&ctx, &read, NotificationStatus::Read).await?;
    ops::retire_principal(&ctx, "triage-bot").await?;
    assert!(routing::plan_for_waiting_questions(&ctx).await?.is_empty(), "no agent answers for it now");
    let alice = ops::set_role(&ctx, "impl-alice", AgentRole::Triage, Some(ops::Serves::ProjectWide)).await?;
    let planned = routing::plan_for_waiting_questions(&ctx).await?;
    assert_eq!(
        planned.iter().map(|p| (p.recipient, p.target.clone())).collect::<Vec<_>>(),
        vec![(alice, question(second))]
    );
    Ok(())
}

/// What is asked of no one reaches the triage agent of the scope it concerns, else the one that
/// serves the whole project (docs/DECISIONS.md, D122): a question asked of no one in particular
/// asks, as it is posted, the scope `--scope` names, else the scope of what it is about, else the
/// scope its asker wrote it in; a comment on a task reaches the triage agent of the task's scope, a
/// comment on code no task is near that of the scope its author wrote it in; a proposal, a review
/// and a proposed decision that of the scope of their task, or the scope the decision governs; a
/// triage agent never takes the review of its own submission, which goes to the agent that serves
/// the whole project instead.
#[tokio::test]
async fn what_is_asked_of_no_one_reaches_the_triage_agent_of_the_scope_it_concerns() -> Result<()> {
    let ctx = context().await?;
    let daniel = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    ops::join(&ctx, daniel).await?;
    let bob = ops::add_principal(&ctx, "impl-bob", Some(AgentRole::Implementer), None, None, None, None).await?;
    let (a, b) = (
        ops::create_scope(&ctx, daniel, named_scope("Replication")).await?,
        ops::create_scope(&ctx, daniel, named_scope("Storage")).await?,
    );
    let of_a =
        ops::add_principal(&ctx, "triage-a", Some(AgentRole::Triage), Some(ops::Serves::Scope(a)), None, None, None)
            .await?;
    let whole = ops::add_principal(
        &ctx,
        "triage-all",
        Some(AgentRole::Triage),
        Some(ops::Serves::ProjectWide),
        None,
        None,
        None,
    )
    .await?;
    let in_a = ops::create_task(&ctx, daniel, CreateTask { scope: Some(a), ..task("Replicate the relay") }).await?;
    let nowhere = ops::create_task(&ctx, daniel, task("Write the primer")).await?;
    let triaged = |who: EntityId, kind: NotificationType| vec![(who, kind, "triage".to_string())];
    let asked = |mc: MessageContext| {
        let ctx = ctx.clone();
        async move {
            let q = ops::post_message(
                &ctx,
                bob,
                "who takes it?",
                Priority::Now,
                MessageContext { asks_project: true, ..mc },
            )
            .await?;
            let scope = ops::MessageContext::from_view(&ctx.get::<MessageView>(q).await?)?.asks_scope;
            anyhow::Ok((q, scope))
        }
    };
    // A question about a task made in Replication asks Replication, whose agent hears it alone.
    let (about_a, scope) = asked(MessageContext { task_id: Some(in_a), ..Default::default() }).await?;
    assert_eq!(scope, Some(a));
    assert_eq!(reasons(&ctx, about_a).await?, triaged(of_a, NotificationType::QuestionAsked));
    // Named, another scope is asked, whatever the question is about: Storage has no agent, so the
    // one that serves the whole project hears it.
    let (named, scope) =
        asked(MessageContext { task_id: Some(in_a), asks_scope: Some(b), ..Default::default() }).await?;
    assert_eq!(scope, Some(b));
    assert_eq!(reasons(&ctx, named).await?, triaged(whole, NotificationType::QuestionAsked));
    // About nothing, it asks the scope its asker wrote it in; with none, the project.
    let (written, scope) = asked(MessageContext { written_in: Some(a), ..Default::default() }).await?;
    assert_eq!((scope, reasons(&ctx, written).await?), (Some(a), triaged(of_a, NotificationType::QuestionAsked)));
    let (bare, scope) = asked(MessageContext::default()).await?;
    assert_eq!((scope, reasons(&ctx, bare).await?), (None, triaged(whole, NotificationType::QuestionAsked)));

    // A comment on the task in Replication reaches Replication's agent, naming the scope it triages;
    // one on the task made in no scope, the agent that serves the whole project.
    let comment = |mc: MessageContext| {
        let ctx = ctx.clone();
        async move { ops::post_message(&ctx, bob, "retries back off", Priority::Next, mc).await }
    };
    let on_a = comment(MessageContext { task_id: Some(in_a), ..Default::default() }).await?;
    let planned = routing::plan_for_message(&ctx, &ctx.get::<MessageView>(on_a).await?).await?;
    assert_eq!(
        planned.iter().map(|p| (p.recipient, p.originator["triage_scope_id"].clone())).collect::<Vec<_>>(),
        vec![(of_a, serde_json::json!(a.to_base64()))]
    );
    let on_nowhere = comment(MessageContext { task_id: Some(nowhere), ..Default::default() }).await?;
    assert_eq!(reasons(&ctx, on_nowhere).await?, triaged(whole, NotificationType::TaskCommented));
    // A comment on code no task is near reaches the agent of the scope its author wrote it in.
    let highlight = EntityId::random();
    let on_code =
        comment(MessageContext { anchor_id: Some(highlight), written_in: Some(a), ..Default::default() }).await?;
    assert_eq!(reasons(&ctx, on_code).await?, triaged(of_a, NotificationType::CodeCommented));

    // A task proposed in Replication reaches its agent; its review too, unless that agent submitted
    // it, when the agent that serves the whole project takes it.
    let proposed = ops::create_task(&ctx, bob, CreateTask { scope: Some(a), propose: true, ..task("Jitter") }).await?;
    let planned = routing::plan_for_task_created(&ctx, &ctx.get::<TaskView>(proposed).await?).await?;
    assert_eq!(
        planned.iter().map(|p| (p.recipient, p.notification_type)).collect::<Vec<_>>(),
        vec![(of_a, NotificationType::TaskProposed)]
    );
    for (submitter, reviewer) in [(bob, of_a), (of_a, whole)] {
        let t = ops::create_task(&ctx, daniel, task("Retry on disconnect")).await?;
        ops::claim(&ctx, submitter, t, a).await?;
        ops::transition(&ctx, submitter, t, Verb::Submit, Some("tests green")).await?;
        let submitted: TaskView = ctx.get(t).await?;
        let planned = routing::plan_for_task_status(&ctx, &submitted, TaskStatus::InProgress, Some(submitter)).await?;
        assert_eq!(
            planned.iter().map(|p| (p.recipient, p.notification_type)).collect::<Vec<_>>(),
            vec![(reviewer, NotificationType::ReviewRequested)]
        );
    }
    // A decision proposed for Replication's work reaches its agent and the project's people.
    let ruling = CreateDecision { title: "Cap retries".into(), propose: true, scope: Some(a), ..Default::default() };
    let d = ops::create_decision(&ctx, bob, ruling).await?;
    let planned = routing::plan_for_decision_created(&ctx, &ctx.get::<DecisionView>(d).await?).await?;
    let mut told: Vec<EntityId> = planned.iter().map(|p| p.recipient).collect();
    told.sort();
    let mut expected = vec![of_a, daniel];
    expected.sort();
    assert_eq!(told, expected);
    Ok(())
}

/// What concerns a task worked in two scopes reaches the triage agent of each (docs/DECISIONS.md,
/// D136, "A task may be worked in several scopes": "A proposal reaches the triage agent of each of
/// its scopes"; the lead, 2026-10-03, for reviews, tasks that became ready and proposed
/// relations), as a comment on it does, each scope's own, else the one that serves the whole
/// project, each agent once and naming the scope it triages, so that an agent that triages two of
/// the task's scopes is told once; a review reaches neither the submitter nor, where every agent
/// that could take it submitted it, anyone but the approver or creator. A question asked of no one
/// asks one scope, its asker's when the task is worked in it, else the first the task was worked
/// in.
#[tokio::test]
async fn what_concerns_a_task_in_two_scopes_reaches_the_triage_agent_of_each() -> Result<()> {
    let ctx = context().await?;
    let daniel = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    ops::join(&ctx, daniel).await?;
    let bob = ops::add_principal(&ctx, "impl-bob", Some(AgentRole::Implementer), None, None, None, None).await?;
    let (a, b, c) = (
        ops::create_scope(&ctx, daniel, named_scope("Replication")).await?,
        ops::create_scope(&ctx, daniel, named_scope("Storage")).await?,
        ops::create_scope(&ctx, daniel, named_scope("Docs")).await?,
    );
    let serving = |name: &'static str, serves: ops::Serves| {
        let ctx = ctx.clone();
        async move { ops::add_principal(&ctx, name, Some(AgentRole::Triage), Some(serves), None, None, None).await }
    };
    let of_a = serving("triage-a", ops::Serves::Scope(a)).await?;
    let of_b = serving("triage-b", ops::Serves::Scope(b)).await?;
    let link_scopes = |task: EntityId, scopes: Vec<EntityId>| {
        let ctx = ctx.clone();
        async move {
            for scope in scopes {
                ops::link(&ctx, daniel, RelationKind::WorkedIn, task, scope, None, false, serde_json::json!({}))
                    .await?;
            }
            anyhow::Ok(())
        }
    };
    let told = |planned: Vec<routing::Planned>| {
        let mut told: Vec<(EntityId, NotificationType, serde_json::Value)> = planned
            .iter()
            .map(|p| (p.recipient, p.notification_type, p.originator["triage_scope_id"].clone()))
            .collect();
        told.sort_by_key(|(recipient, ..)| *recipient);
        told
    };
    let each = |kind: NotificationType| {
        let mut each =
            vec![(of_a, kind, serde_json::json!(a.to_base64())), (of_b, kind, serde_json::json!(b.to_base64()))];
        each.sort_by_key(|(recipient, ..)| *recipient);
        each
    };

    // A task proposed in Replication, and worked in Storage too, reaches both scopes' agents.
    let proposed = ops::create_task(&ctx, bob, CreateTask { scope: Some(a), propose: true, ..task("Jitter") }).await?;
    link_scopes(proposed, vec![b]).await?;
    let planned = routing::plan_for_task_created(&ctx, &ctx.get::<TaskView>(proposed).await?).await?;
    assert_eq!(told(planned), each(NotificationType::TaskProposed));

    // A comment on it reaches both scopes' agents too, each naming the scope it triages.
    let comment = MessageContext { task_id: Some(proposed), ..Default::default() };
    let on_both = ops::post_message(&ctx, bob, "jitter of a tenth", Priority::Next, comment).await?;
    let planned = routing::plan_for_message(&ctx, &ctx.get::<MessageView>(on_both).await?).await?;
    assert_eq!(told(planned), each(NotificationType::TaskCommented));

    // Its review reaches both, and one submitted by one scope's agent reaches the other alone.
    let of_b_alone = vec![(of_b, NotificationType::ReviewRequested, serde_json::json!(b.to_base64()))];
    for (submitter, reviewers) in [(bob, each(NotificationType::ReviewRequested)), (of_a, of_b_alone)] {
        let t = ops::create_task(&ctx, daniel, task("Retry on disconnect")).await?;
        link_scopes(t, vec![a, b]).await?;
        ops::claim(&ctx, submitter, t, a).await?;
        ops::transition(&ctx, submitter, t, Verb::Submit, Some("tests green")).await?;
        let submitted: TaskView = ctx.get(t).await?;
        let planned = routing::plan_for_task_status(&ctx, &submitted, TaskStatus::InProgress, Some(submitter)).await?;
        assert_eq!(told(planned), reviewers, "submitted by {submitter}");
    }

    // A task nobody holds that became ready reaches both scopes' agents.
    let first = ops::create_task(&ctx, daniel, CreateTask { needs_review: false, ..task("Cut the release") }).await?;
    let then = ops::create_task(&ctx, daniel, CreateTask { after: vec![first], ..task("Announce it") }).await?;
    link_scopes(then, vec![a, b]).await?;
    ops::start(&ctx, daniel, first, None).await?;
    ops::transition(&ctx, daniel, first, Verb::Done, None).await?;
    let done: TaskView = ctx.get(first).await?;
    let planned = routing::plan_for_task_status(&ctx, &done, TaskStatus::InProgress, Some(daniel)).await?;
    assert_eq!(told(planned), each(NotificationType::TaskUnblocked));

    // A proposed relation from it reaches both too.
    let other = ops::create_task(&ctx, daniel, task("Write the notes")).await?;
    let proposal =
        ops::link(&ctx, bob, RelationKind::DependsOn, then, other, None, true, serde_json::json!({})).await?;
    let planned = routing::plan_for_relation(&ctx, &ctx.get::<RelationView>(proposal).await?).await?;
    assert_eq!(told(planned), each(NotificationType::TaskProposed));

    // Where one scope has no agent of its own, the agent that serves the whole project answers for
    // it, beside the other scope's own.
    let whole = serving("triage-all", ops::Serves::ProjectWide).await?;
    let docs = ops::create_task(&ctx, bob, CreateTask { scope: Some(a), propose: true, ..task("Document it") }).await?;
    link_scopes(docs, vec![c]).await?;
    let planned = routing::plan_for_task_created(&ctx, &ctx.get::<TaskView>(docs).await?).await?;
    let mut expected = vec![
        (of_a, NotificationType::TaskProposed, serde_json::json!(a.to_base64())),
        (whole, NotificationType::TaskProposed, serde_json::Value::Null),
    ];
    expected.sort_by_key(|(recipient, ..)| *recipient);
    assert_eq!(told(planned), expected);

    // One agent that triages two of a task's scopes answers first for both as one, told once, as
    // the scope first among them (`ops::front_lines`): Storage's agent comes to serve Releases too.
    let releases = ops::create_scope(&ctx, daniel, named_scope("Releases")).await?;
    ops::serve(&ctx, "triage-b", ops::Serves::Scope(releases)).await?;
    let principals = ops::principals(&ctx).await?;
    assert_eq!(
        ops::front_lines(&principals, &[b, releases]),
        vec![ops::FrontLine { agent: of_b, scope: Some(b) }],
        "one front line for the agent of both scopes"
    );
    let shipped =
        ops::create_task(&ctx, bob, CreateTask { scope: Some(b), propose: true, ..task("Ship the store") }).await?;
    link_scopes(shipped, vec![releases]).await?;
    let planned = routing::plan_for_task_created(&ctx, &ctx.get::<TaskView>(shipped).await?).await?;
    // The scope first among them by when the task came to be worked in each, which two writes in
    // one millisecond leave to their relations' ids: read back, not assumed.
    let first = ops::worked_in(&ctx, shipped).await?[0];
    assert_eq!(told(planned), vec![(of_b, NotificationType::TaskProposed, serde_json::json!(first.to_base64()))]);

    // A question about the task in both scopes, asked of no one, asks one scope: the asker's when
    // the task is worked in it, else the scope the task came to be worked in first.
    let asked = |written_in: Option<EntityId>| {
        let ctx = ctx.clone();
        async move {
            let mc = MessageContext { asks_project: true, task_id: Some(proposed), written_in, ..Default::default() };
            let q = ops::post_message(&ctx, bob, "who takes it?", Priority::Now, mc).await?;
            anyhow::Ok(ops::MessageContext::from_view(&ctx.get::<MessageView>(q).await?)?.asks_scope)
        }
    };
    let first = ops::worked_in(&ctx, proposed).await?[0];
    for own in [a, b] {
        assert_eq!(asked(Some(own)).await?, Some(own), "the asker's own scope");
    }
    assert_eq!(asked(Some(c)).await?, Some(first), "the asker's scope is not the task's: its first");
    assert_eq!(asked(None).await?, Some(first));
    Ok(())
}

/// Who a message notifies, how, and why routing chose them, in recipient order.
async fn reasons(ctx: &Context, message: EntityId) -> Result<Vec<(EntityId, NotificationType, String)>> {
    let view: MessageView = ctx.get(message).await?;
    Ok(routing::plan_for_message(ctx, &view)
        .await?
        .iter()
        .map(|p| (p.recipient, p.notification_type, p.originator["why"].as_str().unwrap_or("").to_string()))
        .collect())
}

/// A reply addressed to someone (a forward) is for them and for anyone it mentions: not for the
/// author of what it answers, the thread's author or anyone else in the thread, nor for the people
/// of the task it is about (Daniel, 2026-09-26, of the triage agent, then called the supervisor: "it
/// didn't say to: dnorman though. it said from supervisor to claude. this is why I was confused to
/// see it in my inbox."). A reply addressed to
/// no one still reaches the thread's people. Each notification records why its recipient got it.
#[tokio::test]
async fn a_reply_addressed_to_someone_else_is_not_for_the_thread() -> Result<()> {
    let sorted = |mut v: Vec<(EntityId, NotificationType, String)>| {
        v.sort();
        v
    };
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let bob =
        ops::ensure_principal(&ctx, "impl-bob", "Bob", PrincipalKind::Agent, Some(AgentRole::Implementer)).await?;
    // Task A is Alice's: comments on it reach her.
    let a = ops::create_task(&ctx, c.daniel, task("A")).await?;
    ops::claim(&ctx, c.implementer, a, c.scope).await?;
    let question = ops::post_message(
        &ctx,
        c.daniel,
        "can the retry cap be configurable?",
        Priority::Now,
        MessageContext { asks_project: true, task_id: Some(a), ..Default::default() },
    )
    .await?;
    // It asks the project, and reaches its triage agent and no one else: not Alice, whose task it
    // is about.
    assert_eq!(reasons(&ctx, question).await?, vec![(c.triage, NotificationType::QuestionAsked, "triage".into())]);

    // The triage agent forwards Daniel's question to Bob: Bob is asked; Daniel, whose question it
    // answers and whose thread it is in, hears nothing, and neither does Alice, whose task it is.
    let forward = ops::post_message(
        &ctx,
        c.triage,
        "Bob: does the connector read settings yet?",
        Priority::Now,
        MessageContext { reply_to: Some(question), to: Some(bob.to_base64()), ..Default::default() },
    )
    .await?;
    assert_eq!(reasons(&ctx, forward).await?, vec![(bob, NotificationType::QuestionAsked, "forwarded".into())]);
    // A forward that mentions someone reaches them too.
    let mentioning = ops::post_message(
        &ctx,
        c.triage,
        "Bob: settings? @impl-alice for context",
        Priority::Now,
        MessageContext { reply_to: Some(question), to: Some(bob.to_base64()), ..Default::default() },
    )
    .await?;
    assert_eq!(
        sorted(reasons(&ctx, mentioning).await?),
        sorted(vec![
            (bob, NotificationType::QuestionAsked, "forwarded".into()),
            (c.implementer, NotificationType::Mentioned, "mentioned".into()),
        ])
    );

    // The triage agent answers Daniel, addressed to no one: Daniel hears, as the question's author,
    // and Alice, as a comment on her task.
    let answer = ops::post_message(
        &ctx,
        c.triage,
        "a constant is enough for now",
        Priority::Next,
        MessageContext { reply_to: Some(question), ..Default::default() },
    )
    .await?;
    assert_eq!(
        sorted(reasons(&ctx, answer).await?),
        sorted(vec![
            (c.daniel, NotificationType::Replied, "parent_author".into()),
            (c.implementer, NotificationType::TaskCommented, "assignee".into()),
        ])
    );

    // Bob answers the forward: the triage agent hears as the forward's author, Daniel as the
    // thread's, Alice as the task's, and none twice.
    let reply = ops::post_message(
        &ctx,
        bob,
        "not yet",
        Priority::Next,
        MessageContext { reply_to: Some(forward), ..Default::default() },
    )
    .await?;
    assert_eq!(
        sorted(reasons(&ctx, reply).await?),
        sorted(vec![
            (c.daniel, NotificationType::Replied, "thread_author".into()),
            (c.triage, NotificationType::Replied, "parent_author".into()),
            (c.implementer, NotificationType::TaskCommented, "assignee".into()),
        ])
    );
    Ok(())
}

/// Review goes to the project's triage agent first, which settles only what a person's words or a
/// ruling settle and brings the rest to its lead (2026-09-26, after nine review requests reached
/// Daniel's inbox; docs/DECISIONS.md, D129); without a triage agent that could take it, to whoever
/// approved the task, else its creator; never to the submitter.
#[tokio::test]
async fn review_goes_to_the_triage_agent_first() -> Result<()> {
    let submit = |ctx: Context, task: EntityId, by: EntityId| async move {
        let before: TaskView = ctx.get(task).await?;
        let previous = before.status()?;
        ops::transition(&ctx, by, task, Verb::Submit, Some("commit 1a2b3c; tests green")).await?;
        let after: TaskView = ctx.get(task).await?;
        let planned = routing::plan_for_task_status(&ctx, &after, previous, Some(by)).await?;
        anyhow::Ok(
            planned
                .iter()
                .map(|p| (p.recipient, p.notification_type, p.originator["why"].as_str().unwrap_or("").to_string()))
                .collect::<Vec<_>>(),
        )
    };
    let ctx = context().await?;
    let c = cast(&ctx).await?;

    // Daniel's task, done by Alice: the triage agent takes its review, not Daniel.
    let a = ops::create_task(&ctx, c.daniel, task("A")).await?;
    ops::claim(&ctx, c.implementer, a, c.scope).await?;
    assert_eq!(
        submit(ctx.clone(), a, c.implementer).await?,
        vec![(c.triage, NotificationType::ReviewRequested, "triage".into())]
    );

    // The triage agent submits work of its own: it never takes its own review, so the task's
    // creator is asked.
    let b = ops::create_task(&ctx, c.daniel, task("B")).await?;
    let own = ops::create_scope(&ctx, c.triage, named_scope("Grooming")).await?;
    ops::claim(&ctx, c.triage, b, own).await?;
    assert_eq!(
        submit(ctx.clone(), b, c.triage).await?,
        vec![(c.daniel, NotificationType::ReviewRequested, "creator".into())]
    );

    // A project with no triage agent: whoever approved the task, else its creator.
    let ctx = context().await?;
    let daniel = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let alice =
        ops::ensure_principal(&ctx, "impl-alice", "Alice", PrincipalKind::Agent, Some(AgentRole::Implementer)).await?;
    let scope = ops::create_scope(&ctx, alice, named_scope("Retry")).await?;
    let filed = ops::create_task(&ctx, alice, CreateTask { propose: true, ..task("C") }).await?;
    ops::transition(&ctx, daniel, filed, Verb::Approve, None).await?;
    ops::claim(&ctx, alice, filed, scope).await?;
    assert_eq!(
        submit(ctx.clone(), filed, alice).await?,
        vec![(daniel, NotificationType::ReviewRequested, "approver".into())]
    );
    let plain = ops::create_task(&ctx, daniel, task("D")).await?;
    ops::claim(&ctx, alice, plain, scope).await?;
    assert_eq!(
        submit(ctx.clone(), plain, alice).await?,
        vec![(daniel, NotificationType::ReviewRequested, "creator".into())]
    );
    Ok(())
}

#[tokio::test]
async fn a_scope_takes_any_name_but_a_blank_one() -> Result<()> {
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    ops::rename_scope(&ctx, c.scope, "Comments reach agents mid-task").await?;
    let scope: ScopeView = ctx.get(c.scope).await?;
    assert_eq!(scope.title()?, "Comments reach agents mid-task");
    assert!(ops::rename_scope(&ctx, c.scope, "  ").await.is_err(), "a blank name is refused");
    assert_eq!(ctx.get::<ScopeView>(c.scope).await?.title()?, "Comments reach agents mid-task");
    Ok(())
}

/// A scope is made with any status and edited a field at a time, as the Scopes page's panel does
/// it, under the rules the CLI runs: a blank name and branches without their repository are
/// refused, a remote URL is stored as its upstream's key, a field left out keeps what it had, and
/// the status records when the scope started and when it ended.
#[tokio::test]
async fn a_scope_is_made_and_edited_a_field_at_a_time() -> Result<()> {
    use serde_json::json;
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    assert!(ops::create_scope(&ctx, c.daniel, named_scope(" ")).await.is_err(), "a blank name is refused");
    let alone = CreateScope { branches: vec!["main".into()], ..named_scope("Retry") };
    let err = ops::create_scope(&ctx, c.daniel, alone).await.unwrap_err();
    assert!(err.to_string().contains("needs the repository"), "{err}");

    let spec = CreateScope {
        title: "Cloud hosting".into(),
        intention: "Run the daemon for a team".into(),
        repo: Some("git@github.com:Synesthetic/FwdCentaur.git".into()),
        branches: vec!["main".into(), " feat/hosting ".into(), "main".into(), String::new()],
        status: ScopeStatus::Planned,
    };
    let id = ops::create_scope(&ctx, c.daniel, spec).await?;
    let s: ScopeView = ctx.get(id).await?;
    assert_eq!((s.status()?, s.started()?, EntityId::from(s.created_by()?)), (ScopeStatus::Planned, None, c.daniel));
    let shown = ops::json_of_scope(&s)?;
    assert_eq!(shown["context"]["repo"], "github.com/synesthetic/fwdcentaur", "a remote URL is stored as its key");
    assert_eq!(shown["branches"], json!(["main", "feat/hosting"]), "each branch once, trimmed, no blanks");

    ops::edit_scope(&ctx, id, EditScope { title: Some("Hosting".into()), ..Default::default() }).await?;
    ops::edit_scope(&ctx, id, EditScope { intention: Some("Host a team's graph".into()), ..Default::default() })
        .await?;
    ops::edit_scope(&ctx, id, EditScope { branches: Some(vec!["feat/cloud".into()]), ..Default::default() }).await?;
    let shown = ops::json_of_scope(&ctx.get(id).await?)?;
    assert_eq!((&shown["title"], &shown["intention"]), (&json!("Hosting"), &json!("Host a team's graph")));
    assert_eq!(shown["branches"], json!(["feat/cloud"]));
    assert_eq!(shown["context"]["repo"], "github.com/synesthetic/fwdcentaur", "the repository stays");
    assert_eq!(shown["status"], "planned", "the status stays");

    // Neither refusal changes anything, not even the fields beside the refused one.
    let err = ops::edit_scope(&ctx, id, EditScope { repo: Some(None), ..Default::default() }).await.unwrap_err();
    assert!(err.to_string().contains("needs the repository"), "{err}");
    let blank = EditScope { title: Some(" ".into()), status: Some(ScopeStatus::Active), ..Default::default() };
    assert!(ops::edit_scope(&ctx, id, blank).await.is_err());
    let s: ScopeView = ctx.get(id).await?;
    assert_eq!((s.title()?, s.status()?), ("Hosting".to_string(), ScopeStatus::Planned));
    // With its branches gone, the repository may go too.
    ops::edit_scope(&ctx, id, EditScope { repo: Some(None), branches: Some(vec![]), ..Default::default() }).await?;
    assert_eq!(ops::json_of_scope(&ctx.get(id).await?)?["context"], json!({"repo": null, "branches": []}));

    // Becoming active starts it; done ends it; reopening clears the end and keeps the start.
    ops::set_scope_status(&ctx, id, ScopeStatus::Active).await?;
    let started = ctx.get::<ScopeView>(id).await?.started()?;
    assert!(started.is_some(), "the first status past planned starts it");
    ops::set_scope_status(&ctx, id, ScopeStatus::Done).await?;
    assert!(ctx.get::<ScopeView>(id).await?.ended()?.is_some(), "done ends it");
    ops::edit_scope(&ctx, id, EditScope { status: Some(ScopeStatus::Paused), ..Default::default() }).await?;
    let s: ScopeView = ctx.get(id).await?;
    assert_eq!((s.status()?, s.ended()?, s.started()?), (ScopeStatus::Paused, None, started));

    // `fwd scope intention --append` adds a paragraph.
    let more = EditScope { intention_append: Some("Then bill for it".into()), ..Default::default() };
    ops::edit_scope(&ctx, id, more).await?;
    assert_eq!(ops::text(ctx.get::<ScopeView>(id).await?.intention())?, "Host a team's graph\n\nThen bill for it");

    // A scope from before 2026-09-25 names one `branch` and no repository: a rename leaves that
    // alone, and naming the repository folds the branch into the list.
    let old = ops::create_scope(&ctx, c.daniel, named_scope("Old")).await?;
    ops::update_scope_context(&ctx, old, |context| *context = json!({"branch": "legacy"})).await?;
    ops::rename_scope(&ctx, old, "Older").await?;
    assert_eq!(ops::json_of_scope(&ctx.get(old).await?)?["context"], json!({"branch": "legacy"}));
    ops::edit_scope(&ctx, old, EditScope { repo: Some(Some("github.com/o/r".into())), ..Default::default() }).await?;
    let context = &ops::json_of_scope(&ctx.get(old).await?)?["context"];
    assert_eq!(context, &json!({"repo": "github.com/o/r", "branches": ["legacy"]}));
    Ok(())
}

/// Rules 2 and 9 reach a task's assignee (Daniel, 2026-09-28); a comment on a highlight that is a
/// step reaches the creator and the assignee of each task it is a step of, naming the task, and
/// neither a reply on the highlight nor a comment on it that names a task uses that rule.
#[tokio::test]
async fn an_assignee_and_the_people_of_a_step_hear_what_is_theirs() -> Result<()> {
    let sorted = |mut v: Vec<(EntityId, NotificationType, String)>| {
        v.sort();
        v
    };
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let bob =
        ops::ensure_principal(&ctx, "impl-bob", "Bob", PrincipalKind::Agent, Some(AgentRole::Implementer)).await?;

    // Rule 9: B waits on A and is Bob's; A done, Bob hears B is ready.
    let a = ops::create_task(&ctx, c.daniel, CreateTask { needs_review: false, ..task("A") }).await?;
    let b = ops::create_task(&ctx, c.daniel, CreateTask { after: vec![a], ..task("B") }).await?;
    ops::assign(&ctx, c.daniel, b, Some(bob)).await?;
    ops::transition(&ctx, c.daniel, a, Verb::Done, None).await?;
    let av: TaskView = ctx.get(a).await?;
    let planned = routing::plan_for_task_status(&ctx, &av, TaskStatus::Open, Some(c.daniel)).await?;
    let unblocked: Vec<_> =
        planned.iter().map(|p| (p.recipient, p.notification_type, p.originator["why"].clone())).collect();
    assert_eq!(unblocked, vec![(bob, NotificationType::TaskUnblocked, serde_json::json!("assignee"))]);

    // Rule 2: a comment on B reaches Bob as its assignee, and the triage agent.
    let m = ops::post_message(
        &ctx,
        c.daniel,
        "B next?",
        Priority::Next,
        MessageContext { task_id: Some(b), ..Default::default() },
    )
    .await?;
    assert_eq!(
        sorted(reasons(&ctx, m).await?),
        sorted(vec![
            (bob, NotificationType::TaskCommented, "assignee".into()),
            (c.triage, NotificationType::TaskCommented, "triage".into()),
        ])
    );

    // A review Daniel created and assigned to Bob, with a highlight as a step (a highlight's own
    // fields play no part in the rule, so the test names one by its id alone).
    let review = ops::create_task(&ctx, c.daniel, task("Review: ws.rs")).await?;
    ops::assign(&ctx, c.daniel, review, Some(bob)).await?;
    let highlight = EntityId::random();
    ops::add_step(&ctx, c.implementer, review, highlight).await?;
    let on_highlight = MessageContext { anchor_id: Some(highlight), ..Default::default() };
    let comment =
        ops::post_message(&ctx, c.implementer, "this mutex serializes X and Y", Priority::Next, on_highlight.clone())
            .await?;
    assert_eq!(
        sorted(reasons(&ctx, comment).await?),
        sorted(vec![
            (c.daniel, NotificationType::CodeCommented, "step_party".into()),
            (bob, NotificationType::CodeCommented, "step_party".into()),
            (c.triage, NotificationType::CodeCommented, "triage".into()),
        ])
    );
    let view: MessageView = ctx.get(comment).await?;
    let planned = routing::plan_for_message(&ctx, &view).await?;
    assert!(planned
        .iter()
        .filter(|p| p.recipient != c.triage)
        .all(|p| p.originator["step_task_id"] == review.to_base64()));
    // The commenter is left out: Bob's own comment reaches Daniel and the triage agent.
    let own = ops::post_message(&ctx, bob, "agreed", Priority::Next, on_highlight).await?;
    assert_eq!(
        sorted(reasons(&ctx, own).await?),
        sorted(vec![
            (c.daniel, NotificationType::CodeCommented, "step_party".into()),
            (c.triage, NotificationType::CodeCommented, "triage".into()),
        ])
    );
    // A reply on the highlight's thread reaches its thread, not the step's people.
    let reply = ops::post_message(
        &ctx,
        c.triage,
        "noted",
        Priority::Next,
        MessageContext { reply_to: Some(comment), ..Default::default() },
    )
    .await?;
    assert_eq!(reasons(&ctx, reply).await?, vec![(c.implementer, NotificationType::Replied, "parent_author".into())]);
    // A comment on the highlight that names a task is a comment on that task.
    let on_task = ops::post_message(
        &ctx,
        c.triage,
        "for B",
        Priority::Next,
        MessageContext { anchor_id: Some(highlight), task_id: Some(b), ..Default::default() },
    )
    .await?;
    assert_eq!(reasons(&ctx, on_task).await?, vec![(bob, NotificationType::TaskCommented, "assignee".into())]);
    Ok(())
}

/// A whole file made a step is a highlight like lines of it (docs/DECISIONS.md, D43, "A whole-file
/// reference is an anchor with no line range"): its first comment, posted once the file is a step,
/// reaches the creator and the assignee of the task, which is why `fwd step add --path` makes the
/// step before it posts the comment, with `--lines` or without (task wmNoucjv). Routing reads the
/// graph as it stands when a comment is posted, so a comment posted on a whole file before the
/// file became a step reached neither.
#[tokio::test]
async fn a_whole_file_steps_first_comment_reaches_the_tasks_creator_and_assignee() -> Result<()> {
    let sorted = |mut v: Vec<(EntityId, NotificationType, String)>| {
        v.sort();
        v
    };
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let bob =
        ops::ensure_principal(&ctx, "impl-bob", "Bob", PrincipalKind::Agent, Some(AgentRole::Implementer)).await?;
    let review = ops::create_task(&ctx, c.daniel, task("Review: the connector")).await?;
    ops::assign(&ctx, c.daniel, review, Some(bob)).await?;
    let repo = EntityId::random();
    let net = anchor(&ctx, c.implementer, repo, "src/net.rs", None).await?;
    let retry = anchor(&ctx, c.implementer, repo, "src/retry.rs", None).await?;
    let on = |anchor: EntityId| MessageContext { anchor_id: Some(anchor), ..Default::default() };
    // Named as a step and in an inbox's list by its path alone, since it names no line.
    assert_eq!(ops::describe(&ctx, net).await, Some((ops::Kind::Anchor, "src/net.rs".to_string())));

    // The step first, then its first comment: the review's creator and assignee hear of it.
    ops::add_step(&ctx, c.implementer, review, net).await?;
    let first =
        ops::post_message(&ctx, c.implementer, "nothing here changes; read it last", Priority::Next, on(net)).await?;
    assert_eq!(
        sorted(reasons(&ctx, first).await?),
        sorted(vec![
            (c.daniel, NotificationType::CodeCommented, "step_party".into()),
            (bob, NotificationType::CodeCommented, "step_party".into()),
            (c.triage, NotificationType::CodeCommented, "triage".into()),
        ])
    );

    // A comment on a whole file that is no one's step yet, as the comment was when a whole file
    // became a step only by commenting on it and adding its anchor after: only the triage agent hears.
    let early = ops::post_message(&ctx, c.implementer, "and this one first", Priority::Next, on(retry)).await?;
    assert_eq!(reasons(&ctx, early).await?, vec![(c.triage, NotificationType::CodeCommented, "triage".into())]);
    Ok(())
}

/// Inline references (Daniel, 2026-09-28: each reference in a comment "should shadow/be shadowed
/// by an edge"): resolved when posted, stored as full ids, each shadowed by a `mention` relation
/// that says it was inline and where; one that names nothing refuses the post; code is text.
#[tokio::test]
async fn an_inline_reference_is_stored_as_an_id_and_shadowed_by_a_mention() -> Result<()> {
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let task = ops::create_task(&ctx, c.daniel, task("Retry on disconnect")).await?;
    let d = ops::create_decision(&ctx, c.daniel, ruling("Retries back off exponentially")).await?;
    let prefix = &task.to_base64()[..8];
    let m = ops::post_message(
        &ctx,
        c.triage,
        &format!("see [[{prefix}]] under [[D1]], @impl-alice; not `@nobody` in code"),
        Priority::Next,
        MessageContext::default(),
    )
    .await?;
    let stored = ctx.get::<MessageView>(m).await?.content()?;
    assert_eq!(
        stored,
        format!(
            "see [[{}]] under [[{}]], [[{}]]; not `@nobody` in code",
            task.to_base64(),
            d.to_base64(),
            c.implementer.to_base64()
        )
    );
    let mut edges = ops::relations_from(&ctx, m, Some(RelationKind::Mention)).await?;
    edges.sort_by_key(|r| serde_json::Value::from(r.via().unwrap())["ordinal"].as_i64());
    let shadowed: Vec<_> = edges
        .iter()
        .map(|r| (r.to_id().unwrap(), r.status().unwrap(), serde_json::Value::from(r.via().unwrap())))
        .collect();
    assert_eq!(
        shadowed,
        vec![
            (task, RelationStatus::Accepted, serde_json::json!({"inline": true, "ordinal": 1})),
            (d, RelationStatus::Accepted, serde_json::json!({"inline": true, "ordinal": 2})),
            (c.implementer, RelationStatus::Accepted, serde_json::json!({"inline": true, "ordinal": 3})),
        ]
    );
    // Rendered back for a terminal, and listed for JSON.
    assert_eq!(
        ops::render_references(&ctx, &stored).await,
        "see [Retry on disconnect] under [D1 (Retries back off exponentially)], @impl-alice; not `@nobody` in code"
    );
    let listed: Vec<(String, String)> = ops::references_json(&ctx, &stored)
        .await
        .iter()
        .map(|r| (r["kind"].as_str().unwrap().to_string(), r["name"].as_str().unwrap().to_string()))
        .collect();
    assert_eq!(
        listed,
        vec![
            ("task".into(), "Retry on disconnect".into()),
            ("decision".into(), "D1 (Retries back off exponentially)".into()),
            ("principal".into(), "@impl-alice".into()),
        ]
    );
    // The person named hears of it as a mention.
    assert_eq!(recipients(&ctx, m).await?, vec![(c.implementer, NotificationType::Mentioned)]);

    // A typo is caught when posted, naming the reference; nothing is written.
    let before = ctx.fetch::<MessageView>("created > 0").await?.len();
    let err =
        ops::post_message(&ctx, c.daniel, "ask @impl-alicee and [[D9]]", Priority::Next, MessageContext::default())
            .await
            .unwrap_err()
            .to_string();
    assert!(err.contains("2 references name nothing"), "{err}");
    assert!(err.contains("`@impl-alicee` (no one has the handle `impl-alicee`, and no id starts with it)"), "{err}");
    assert!(err.contains("`[[D9]]` (there is no decision D9)"), "{err}");
    let err = ops::post_message(&ctx, c.daniel, "see [[zzzzzzzz]]", Priority::Next, MessageContext::default())
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("`[[zzzzzzzz]]` (no task, message, decision"), "{err}");
    assert_eq!(ctx.fetch::<MessageView>("created > 0").await?.len(), before, "a refused post writes nothing");

    // Text that only looks like a mention (in a word, or in code) is posted as written.
    let plain = ops::post_message(
        &ctx,
        c.daniel,
        "mail daniel@example.com about this:\n```\n@Override\n```",
        Priority::Next,
        MessageContext::default(),
    )
    .await?;
    assert!(ops::relations_from(&ctx, plain, Some(RelationKind::Mention)).await?.is_empty());
    Ok(())
}

/// Routing reads the mention relations a message was posted with, not its text: a message that
/// holds `@handle` with no relation behind it (as one written before mentions were relations)
/// tells no one it names.
#[tokio::test]
async fn a_mention_reaches_its_person_through_its_relation_only() -> Result<()> {
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let trx = ctx.begin();
    let old = trx
        .create(&Message {
            author: ankurah::property::Ref::new(c.daniel),
            created: fwd_core::now_ms(),
            content: "@impl-alice have a look".into(),
            priority: Priority::Next,
            status: MessageStatus::Active,
            task: None,
            anchor: None,
            scope: None,
            decision: None,
            reply_to: None,
            thread: None,
            context: ankurah::property::Json::new(serde_json::json!({})),
            resolved_by: None,
            resolved: None,
            edited: None,
            edited_by: None,
        })
        .await?
        .id();
    trx.commit().await?;
    assert!(recipients(&ctx, old).await?.is_empty());
    let posted =
        ops::post_message(&ctx, c.daniel, "@impl-alice have a look", Priority::Next, MessageContext::default()).await?;
    assert_eq!(recipients(&ctx, posted).await?, vec![(c.implementer, NotificationType::Mentioned)]);
    Ok(())
}

/// Who a new relation notifies, how, and why routing chose them.
async fn told_by(ctx: &Context, rel: &RelationView) -> Result<Vec<(EntityId, NotificationType, String)>> {
    Ok(routing::plan_for_relation(ctx, rel)
        .await?
        .iter()
        .map(|p| (p.recipient, p.notification_type, p.originator["why"].as_str().unwrap_or("").to_string()))
        .collect())
}

/// An author edits a message they sent where it stands (task rRHro34D, after Daniel asked of a
/// question an agent had sent him "Are you able to edit this question?" and the only fix was a
/// second question): only its author may, no agent having succeeded its author (an agent that did
/// edits it too: tests/agents.rs); its id, thread and place stay while its text changes, and
/// `edited` and `edited_by` say when and by whom; its mentions follow the new text, one retracted
/// with the reason and one added, which tells the one newly named and no one else; a name that
/// fits nothing refuses the edit as it refuses a post; and a notification's sentence quotes the
/// text as it reads now.
#[tokio::test]
async fn an_author_edits_a_message_in_place_and_its_mentions_follow_the_text() -> Result<()> {
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let bob =
        ops::ensure_principal(&ctx, "impl-bob", "Bob", PrincipalKind::Agent, Some(AgentRole::Implementer)).await?;
    let t = ops::create_task(&ctx, c.daniel, task("Retry on disconnect")).await?;
    let short = &t.to_base64()[..8];
    let post = |by: EntityId, text: String, reply_to: Option<EntityId>| {
        let ctx = ctx.clone();
        async move {
            let mc = MessageContext { reply_to, ..Default::default() };
            let id = ops::post_message(&ctx, by, &text, Priority::Next, mc).await?;
            let view: MessageView = ctx.get(id).await?;
            routing::apply(&ctx, &routing::plan_for_message(&ctx, &view).await?).await?;
            Ok::<_, anyhow::Error>(id)
        }
    };
    // A thread: Daniel's comment, the triage agent's question beneath it, Daniel's answer.
    let root = post(c.daniel, "should reconnects back off?".into(), None).await?;
    let question = post(c.triage, format!("@impl-alice, see [[{short}]]: can you take it?"), Some(root)).await?;
    let answer = post(c.daniel, "yes, exponentially".into(), Some(question)).await?;
    let thread_order = || {
        let ctx = ctx.clone();
        async move {
            let found = ops::messages_about(&ctx, "thread", root).await?;
            Ok::<_, anyhow::Error>(found.iter().map(|m| m.id()).collect::<Vec<_>>())
        }
    };
    assert_eq!(thread_order().await?, vec![question, answer]);
    let before: MessageView = ctx.get(question).await?;
    let place = |m: &MessageView| -> Result<(i64, Option<EntityId>, Option<EntityId>)> {
        Ok((m.created()?, m.reply_to()?.map(EntityId::from), m.thread()?.map(EntityId::from)))
    };
    let (created, reply_to, thread) = place(&before)?;
    assert_eq!((reply_to, thread), (Some(root), Some(root)));
    let posted_text = ops::text(before.content())?;
    let posted_edges = ops::relations_from(&ctx, question, Some(RelationKind::Mention)).await?;
    let edge_to = |to: EntityId| posted_edges.iter().find(|r| r.to_id().ok() == Some(to)).map(|r| r.id()).unwrap();
    let (to_alice, to_task) = (edge_to(c.implementer), edge_to(t));

    // Only its author edits it, no agent having succeeded its author; anyone else is told to reply.
    let refused = ops::edit_message(&ctx, c.daniel, question, "a better question").await.unwrap_err().to_string();
    assert_eq!(
        refused,
        "only its author edits a message, or an agent that succeeded its author, and this one is @triage-bot's; reply to it instead, or ask @triage-bot to edit it"
    );
    // A name that fits nothing refuses the edit, as it refuses a post, and a blank text is refused.
    let typo = ops::edit_message(&ctx, c.triage, question, "@impl-bobb, can you?").await.unwrap_err().to_string();
    assert!(
        typo.contains("`@impl-bobb` (no one has the handle `impl-bobb`, and no id starts with it) names nothing"),
        "{typo}"
    );
    let blank = ops::edit_message(&ctx, c.triage, question, "  \n").await.unwrap_err().to_string();
    assert_eq!(blank, "a message's text cannot be blank; write what it should say instead");
    let untouched: MessageView = ctx.get(question).await?;
    assert_eq!((ops::text(untouched.content())?, untouched.edited()?), (posted_text.clone(), None), "nothing written");
    assert_eq!(ops::relations_from(&ctx, question, Some(RelationKind::Mention)).await?.len(), 2);

    // The triage agent asks Bob instead of Alice, the task named second as before.
    ops::edit_message(&ctx, c.triage, question, &format!("@impl-bob, see [[{short}]]: can you take it?")).await?;
    let after: MessageView = ctx.get(question).await?;
    assert_eq!(
        ops::text(after.content())?,
        format!("[[{}]], see [[{}]]: can you take it?", bob.to_base64(), t.to_base64()),
        "the new text's references are stored as full ids"
    );
    assert_eq!(after.id(), question);
    assert_eq!(place(&after)?, (created, reply_to, thread));
    assert_eq!(thread_order().await?, vec![question, answer], "it keeps its place among the replies");
    let edited = after.edited()?.expect("an edit records when it was made");
    assert!(edited >= created);
    assert_eq!(ops::json_of_message(&after)?["edited"], edited);
    assert_eq!(after.edited_by()?.map(EntityId::from), Some(c.triage), "and who made it, the author here");
    assert_eq!(ops::json_of_message(&after)?["edited_by"], c.triage.to_base64());

    let edges = ops::relations_from(&ctx, question, Some(RelationKind::Mention)).await?;
    let edge = |id: EntityId| edges.iter().find(|r| r.id() == id).unwrap();
    assert_eq!(edge(to_task).status()?, RelationStatus::Accepted, "the task, still named, keeps its relation");
    let gone = edge(to_alice);
    assert_eq!(gone.status()?, RelationStatus::Retracted, "Alice, no longer named, has hers retracted");
    assert_eq!(gone.closed_by()?.map(EntityId::from), Some(c.triage));
    assert_eq!(
        gone.closed_reason()?.as_deref(),
        Some("the message's text no longer names it: an edit of the text removed the reference")
    );
    let added: Vec<&RelationView> = edges.iter().filter(|r| r.id() != to_task && r.id() != to_alice).collect();
    assert_eq!(added.len(), 1);
    assert_eq!((added[0].to_id()?, added[0].status()?), (bob, RelationStatus::Accepted), "Bob, newly named, has one");
    assert_eq!(
        serde_json::Value::from(added[0].via()?),
        serde_json::json!({"inline": true, "ordinal": 1, "edited": true}),
        "which says the edit wrote it"
    );

    // Bob hears that the message names him now; the relations kept and retracted tell no one.
    assert_eq!(told_by(&ctx, added[0]).await?, vec![(bob, NotificationType::Mentioned, "mentioned".into())]);
    assert!(told_by(&ctx, edge(to_task)).await?.is_empty());
    assert!(told_by(&ctx, gone).await?.is_empty());
    // The post's relations are the post's to tell: routing the question again tells Bob nothing.
    assert!(!reasons(&ctx, question).await?.iter().any(|(who, ..)| *who == bob));
    routing::apply(&ctx, &routing::plan_for_relation(&ctx, added[0]).await?).await?;

    // Named again, named twice, or named by the author: no one hears a second time. Daniel, named
    // newly, heard of the question already (it answers his comment), so he is not told again.
    ops::edit_message(&ctx, c.triage, question, "@impl-bob and @impl-bob, or @dnorman? @triage-bot asks").await?;
    for r in ops::relations_from(&ctx, question, Some(RelationKind::Mention)).await? {
        assert!(told_by(&ctx, &r).await?.is_empty(), "{:?}", serde_json::Value::from(r.via()?));
    }

    // A text that stores as the message's own changes nothing, not even when it was edited.
    let stamped = ctx.get::<MessageView>(question).await?.edited()?;
    let same = ops::text(ctx.get::<MessageView>(question).await?.content())?;
    ops::edit_message(&ctx, c.triage, question, &same).await?;
    assert_eq!(ctx.get::<MessageView>(question).await?.edited()?, stamped);

    // Daniel's inbox names the thread by its first words, as its first message reads now.
    let replied = ops::inbox(&ctx, c.daniel, false).await?;
    let replied = replied.iter().find(|n| n.notification_type().ok() == Some(NotificationType::Replied)).unwrap();
    let sentence = || async {
        Ok::<_, anyhow::Error>(fwd_core::reason::plain(&fwd_core::reason::gather(&ctx, replied, Some(c.daniel)).await?))
    };
    let reply = format!("; reply with fwd reply {}", &replied.id().to_base64()[..8]);
    assert_eq!(sentence().await?, format!("triage-bot replied to your comment “should reconnects back off?”{reply}"));
    ops::edit_message(&ctx, c.daniel, root, "should reconnects back off, and how far?").await?;
    assert_eq!(
        sentence().await?,
        format!("triage-bot replied to your comment “should reconnects back off, and how far?”{reply}")
    );
    Ok(())
}

/// A message's standing `mention` relations in the order of their places among its references,
/// each as the relation, what it names and its `via`; and its retracted relations.
async fn mentions_of(
    ctx: &Context,
    message: EntityId,
) -> Result<(Vec<(EntityId, EntityId, serde_json::Value)>, Vec<EntityId>)> {
    let (mut standing, mut retracted) = (Vec::new(), Vec::new());
    for r in ops::relations_from(ctx, message, Some(RelationKind::Mention)).await? {
        match r.status()? {
            RelationStatus::Accepted => standing.push((r.id(), r.to_id()?, serde_json::Value::from(r.via()?))),
            _ => retracted.push(r.id()),
        }
    }
    standing.sort_by_key(|(.., via)| via["ordinal"].as_u64());
    retracted.sort();
    Ok((standing, retracted))
}

/// An edit that moves a message's references moves their `mention` relations with them (task
/// FF8U7XgY, which found no test moving one): each thing the new text still names keeps its
/// relation, which takes the place of the reference that keeps it and keeps the rest of its `via`,
/// the mark of the edit that wrote it among them; a second reference to one thing gets a relation
/// of its own, marked as the edit's; and what the text no longer names has its relation retracted.
#[tokio::test]
async fn an_edit_keeps_the_mentions_of_what_the_text_still_names_in_their_new_places() -> Result<()> {
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let retry = ops::create_task(&ctx, c.daniel, task("Retry on disconnect")).await?;
    let cap = ops::create_task(&ctx, c.daniel, task("Cap the retry delay")).await?;
    let jitter = ops::create_task(&ctx, c.daniel, task("Jitter the retries")).await?;
    let by_post = |ordinal: u64| serde_json::json!({"inline": true, "ordinal": ordinal});
    let by_edit = |ordinal: u64| serde_json::json!({"inline": true, "ordinal": ordinal, "edited": true});

    let text = format!("[[{}]], [[{}]], @impl-alice", retry.to_base64(), cap.to_base64());
    let m = ops::post_message(&ctx, c.triage, &text, Priority::Next, MessageContext::default()).await?;
    let (standing, _) = mentions_of(&ctx, m).await?;
    let [to_retry, to_cap, to_alice] = [0, 1, 2].map(|n| standing[n].0);
    assert_eq!(
        standing,
        vec![(to_retry, retry, by_post(1)), (to_cap, cap, by_post(2)), (to_alice, c.implementer, by_post(3))]
    );

    // Alice first now, the second task second still, a third task last and the first task gone:
    // Alice's relation moves to the first place, the second task keeps its own where it was, the
    // third task gets one that the edit marks, and the first task's is retracted.
    let text = format!("@impl-alice, [[{}]], [[{}]]", cap.to_base64(), jitter.to_base64());
    ops::edit_message(&ctx, c.triage, m, &text).await?;
    let (standing, retracted) = mentions_of(&ctx, m).await?;
    let to_jitter = standing[2].0;
    assert_eq!(
        standing,
        vec![(to_alice, c.implementer, by_post(1)), (to_cap, cap, by_post(2)), (to_jitter, jitter, by_edit(3))]
    );
    assert_eq!(retracted, vec![to_retry]);

    // The third task first and the second twice: the relation the last edit wrote moves to the
    // first place with its mark, the second task's first reference keeps its relation and its
    // second gets a new one, and Alice's is retracted.
    let text = format!("[[{}]], [[{}]] and [[{}]] again", jitter.to_base64(), cap.to_base64(), cap.to_base64());
    ops::edit_message(&ctx, c.triage, m, &text).await?;
    let (standing, retracted) = mentions_of(&ctx, m).await?;
    let again = standing[2].0;
    assert_eq!(standing, vec![(to_jitter, jitter, by_edit(1)), (to_cap, cap, by_post(2)), (again, cap, by_edit(3))]);
    let mut gone = vec![to_retry, to_alice];
    gone.sort();
    assert_eq!(retracted, gone);
    Ok(())
}

/// Steps (docs/DECISIONS.md, 2026-09-28): a `step_of` relation from anything to a task, ordered by
/// its position, checked by moving the relation to Resolved and unchecked by moving it back; made
/// once; acyclic between tasks; and nothing but a step is ever checked.
#[tokio::test]
async fn steps_are_added_listed_checked_and_unchecked() -> Result<()> {
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let review = ops::create_task(&ctx, c.daniel, task("Review: the connector")).await?;
    let part = ops::create_task(&ctx, c.daniel, task("Read the retry loop")).await?;
    let note = ops::post_message(&ctx, c.daniel, "start with ws.rs", Priority::Next, MessageContext::default()).await?;
    let first = ops::add_step(&ctx, c.triage, review, note).await?;
    let second = ops::add_step(&ctx, c.triage, review, part).await?;
    assert_eq!(ops::add_step(&ctx, c.triage, review, note).await?, first, "a step is made once");
    let listed = |ctx: Context| async move {
        let steps = ops::steps(&ctx, review).await?;
        anyhow::Ok(steps.iter().map(|r| (r.id(), r.status().unwrap())).collect::<Vec<_>>())
    };
    assert_eq!(
        listed(ctx.clone()).await?,
        vec![(first, RelationStatus::Accepted), (second, RelationStatus::Accepted)],
        "in the order they were added"
    );
    let positions: Vec<String> =
        ops::steps(&ctx, review).await?.iter().map(|r| r.position().unwrap().unwrap()).collect();
    assert!(positions[0] < positions[1], "{positions:?}");

    // Checked: Resolved, with who and when; unchecked: Accepted, with neither.
    ops::check_step(&ctx, c.daniel, first, true).await?;
    let checked: RelationView = ctx.get(first).await?;
    assert_eq!(checked.status()?, RelationStatus::Resolved);
    assert_eq!(checked.closed_by()?.map(EntityId::from), Some(c.daniel));
    assert!(checked.closed()?.is_some());
    ops::check_step(&ctx, c.daniel, first, true).await?;
    assert_eq!(listed(ctx.clone()).await?[0], (first, RelationStatus::Resolved), "checking twice changes nothing");
    ops::check_step(&ctx, c.daniel, first, false).await?;
    let unchecked: RelationView = ctx.get(first).await?;
    assert_eq!(
        (unchecked.status()?, unchecked.closed_by()?.map(EntityId::from), unchecked.closed()?),
        (RelationStatus::Accepted, None, None)
    );

    // A task is never a step of itself, however far round, checked or not.
    ops::check_step(&ctx, c.daniel, second, true).await?;
    let err = ops::add_step(&ctx, c.daniel, part, review).await.unwrap_err();
    assert!(err.to_string().contains("would close a cycle"), "{err}");
    assert!(ops::add_step(&ctx, c.daniel, review, review).await.is_err());
    // Only a step is checked, and a step is a step of a task.
    let dependency =
        ops::link(&ctx, c.daniel, RelationKind::DependsOn, part, review, None, false, serde_json::json!({})).await?;
    let err = ops::check_step(&ctx, c.daniel, dependency, true).await.unwrap_err();
    assert!(err.to_string().contains("only a step is checked"), "{err}");
    assert!(ops::add_step(&ctx, c.daniel, note, part).await.is_err(), "a message has no checklist");
    // A retracted step leaves the checklist, checked or not.
    assert_eq!(ops::unlink(&ctx, c.daniel, RelationKind::StepOf, part, review, "not needed").await?, 1);
    assert_eq!(listed(ctx.clone()).await?, vec![(first, RelationStatus::Accepted)]);
    Ok(())
}

/// A message's status and who resolved it, and a step's status and who checked it, as the tests
/// below compare them: the time each was moved is there exactly when the person is.
async fn thread_state(ctx: &Context, message: EntityId) -> Result<(MessageStatus, Option<EntityId>)> {
    let m: MessageView = ctx.get(message).await?;
    assert_eq!(m.resolved()?.is_some(), m.resolved_by()?.is_some(), "who resolved it and when go together");
    Ok((m.status()?, m.resolved_by()?.map(EntityId::from)))
}

async fn step_state(ctx: &Context, edge: EntityId) -> Result<(RelationStatus, Option<EntityId>)> {
    let r: RelationView = ctx.get(edge).await?;
    assert_eq!(r.closed()?.is_some(), r.closed_by()?.is_some(), "who checked it and when go together");
    Ok((r.status()?, r.closed_by()?.map(EntityId::from)))
}

/// How many messages, relations and notifications the project holds: moving a step and its thread
/// adds none, so routing, which reacts to what is added, tells no one, as a check never did.
async fn entities(ctx: &Context) -> Result<(usize, usize, usize)> {
    Ok((
        ctx.fetch::<MessageView>("created > 0").await?.len(),
        ctx.fetch::<RelationView>("created > 0").await?.len(),
        ctx.fetch::<NotificationView>("created > 0").await?.len(),
    ))
}

/// D99 (docs/DECISIONS.md, "Checking a step resolves its highlight's first thread, and resolving
/// that thread checks the step; each control says it does both"): resolving the first message of
/// a highlight's first thread checks the steps the highlight is, and setting it back to active
/// unchecks them; checking a step (the page's box and `fwd step check` both call `check_step`)
/// resolves that message, and unchecking sets it back; each move records who and when on the
/// message and on every step, and adds nothing, so tells no one. A later thread and a reply move
/// alone.
#[tokio::test]
async fn a_highlights_first_thread_and_its_steps_move_together() -> Result<()> {
    use MessageStatus::{Active, Resolved as Settled};
    use RelationStatus::{Accepted, Resolved as Checked};
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let review = ops::create_task(&ctx, c.daniel, task("Review: the retry loop")).await?;
    let again = ops::create_task(&ctx, c.daniel, task("Review: the retry loop, again")).await?;
    let highlight = anchor(&ctx, c.implementer, EntityId::random(), "src/ws.rs", Some((3, 8))).await?;
    let on_highlight = MessageContext { anchor_id: Some(highlight), ..Default::default() };
    let first =
        ops::post_message(&ctx, c.implementer, "the loop never backs off", Priority::Next, on_highlight.clone())
            .await?;
    // Two threads started within the same millisecond would be ordered by id.
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let later = ops::post_message(&ctx, c.daniel, "and why no jitter?", Priority::Next, on_highlight).await?;
    let reply = ops::post_message(
        &ctx,
        c.daniel,
        "agreed",
        Priority::Next,
        MessageContext { reply_to: Some(first), ..Default::default() },
    )
    .await?;
    let step = ops::add_step(&ctx, c.implementer, review, highlight).await?;
    let other = ops::add_step(&ctx, c.implementer, again, highlight).await?;
    assert_eq!(ops::first_thread(&ctx, highlight).await?.map(|m| m.id()), Some(first));
    let before = entities(&ctx).await?;

    // Resolving the first thread checks the highlight's step on both checklists, saying so.
    let moved = ops::resolve_message(&ctx, c.daniel, first).await?;
    assert_eq!(moved.message, Some((first, Settled)));
    assert_eq!(moved.steps.len(), 2, "{moved:?}");
    assert_eq!(thread_state(&ctx, first).await?, (Settled, Some(c.daniel)));
    assert_eq!(step_state(&ctx, step).await?, (Checked, Some(c.daniel)));
    assert_eq!(step_state(&ctx, other).await?, (Checked, Some(c.daniel)));
    // Setting it back to active unchecks them, and forgets who and when on all three.
    ops::unresolve_message(&ctx, c.daniel, first).await?;
    assert_eq!(thread_state(&ctx, first).await?, (Active, None));
    assert_eq!(step_state(&ctx, step).await?, (Accepted, None));
    assert_eq!(step_state(&ctx, other).await?, (Accepted, None));

    // Checking the step resolves the first thread, and the other checklist's step moves too.
    let moved = ops::check_step(&ctx, c.triage, step, true).await?;
    assert_eq!(moved.message, Some((first, Settled)));
    assert_eq!(thread_state(&ctx, first).await?, (Settled, Some(c.triage)));
    assert_eq!(step_state(&ctx, step).await?, (Checked, Some(c.triage)));
    assert_eq!(step_state(&ctx, other).await?, (Checked, Some(c.triage)));
    // Unchecking the other checklist's step sets the thread back to active, and unchecks this one.
    ops::check_step(&ctx, c.daniel, other, false).await?;
    assert_eq!(thread_state(&ctx, first).await?, (Active, None));
    assert_eq!(step_state(&ctx, step).await?, (Accepted, None));
    assert_eq!(step_state(&ctx, other).await?, (Accepted, None));

    // A later thread and a reply move alone.
    for alone in [later, reply] {
        let moved = ops::resolve_message(&ctx, c.daniel, alone).await?;
        assert_eq!((moved.message, moved.steps), (Some((alone, Settled)), vec![]));
        assert_eq!(thread_state(&ctx, alone).await?, (Settled, Some(c.daniel)));
    }
    assert_eq!(thread_state(&ctx, first).await?, (Active, None));
    assert_eq!(step_state(&ctx, step).await?, (Accepted, None));
    assert_eq!(entities(&ctx).await?, before, "nothing was added, so routing tells no one");
    Ok(())
}

/// A step whose highlight no one has started a thread on just checks (D99, above), and a step that
/// is a message or a task moves alone: D99 speaks of highlights, so the message stays active.
#[tokio::test]
async fn a_step_with_no_thread_or_no_highlight_moves_alone() -> Result<()> {
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let review = ops::create_task(&ctx, c.daniel, task("Review: the connector")).await?;
    let bare = anchor(&ctx, c.implementer, EntityId::random(), "src/ws.rs", Some((20, 25))).await?;
    let step = ops::add_step(&ctx, c.implementer, review, bare).await?;
    let moved = ops::check_step(&ctx, c.daniel, step, true).await?;
    assert_eq!((moved.message, moved.steps), (None, vec![(step, RelationStatus::Resolved)]));
    assert_eq!(ops::first_thread(&ctx, bare).await?.map(|m| m.id()), None);

    let note = ops::post_message(&ctx, c.daniel, "start with ws.rs", Priority::Next, MessageContext::default()).await?;
    let noted = ops::add_step(&ctx, c.daniel, review, note).await?;
    let moved = ops::check_step(&ctx, c.daniel, noted, true).await?;
    assert_eq!((moved.message, moved.steps), (None, vec![(noted, RelationStatus::Resolved)]));
    assert_eq!(thread_state(&ctx, note).await?, (MessageStatus::Active, None), "a message that is a step stays active");
    Ok(())
}

/// The first message of each thread on the highlight `anchor`, as its page lists them: the
/// messages on it that answer no other message (`anchor_row::listing` in crates/fwd-web).
async fn threads_on(ctx: &Context, anchor: EntityId) -> Result<Vec<EntityId>> {
    let on = ops::messages_about(ctx, "anchor", anchor).await?;
    Ok(on.iter().filter(|m| m.reply_to().ok().flatten().is_none()).map(|m| m.id()).collect())
}

/// Taking a step off a checklist (task 9dvGW0EI, "Take a step off a checklist and retire its
/// highlight in one command: fwd step remove"): its relation is retracted with who, when and why,
/// so the checklist no longer lists it, and what the step is stays as it was, a highlight's threads
/// open while it is not retired. Only the task's creator and its assignee take a step off; a step
/// not on that task's checklist, a blank reason and a retire of anything but a highlight are
/// refused, writing nothing; and nothing is added, so nobody is told.
#[tokio::test]
async fn a_step_comes_off_its_checklist_for_the_tasks_creator_and_assignee_and_tells_no_one() -> Result<()> {
    use RelationStatus::{Accepted, Resolved as Checked, Retracted};
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let review = ops::create_task(&ctx, c.daniel, task("Review: the connector")).await?;
    ops::assign(&ctx, c.daniel, review, Some(c.implementer)).await?;
    let other = ops::create_task(&ctx, c.daniel, task("Review: the backoff")).await?;
    let part = ops::create_task(&ctx, c.daniel, task("Read the retry loop")).await?;
    let highlight = anchor(&ctx, c.implementer, EntityId::random(), "src/ws.rs", Some((3, 8))).await?;
    let on_highlight = MessageContext { anchor_id: Some(highlight), ..Default::default() };
    let thread =
        ops::post_message(&ctx, c.implementer, "the loop never backs off", Priority::Next, on_highlight).await?;
    let first = ops::add_step(&ctx, c.implementer, review, highlight).await?;
    let second = ops::add_step(&ctx, c.implementer, review, part).await?;
    let elsewhere = ops::add_step(&ctx, c.daniel, other, part).await?;
    ops::check_step(&ctx, c.daniel, second, true).await?;
    let before = entities(&ctx).await?;
    let listed = |ctx: Context, task: EntityId| async move {
        anyhow::Ok(ops::steps(&ctx, task).await?.iter().map(|r| r.id()).collect::<Vec<_>>())
    };

    // Refused, writing nothing: a step of another task's checklist, anyone but the task's creator
    // and its assignee, a blank reason, and a retire of a step that is a task.
    let refusals = [
        (c.daniel, elsewhere, "moved", false, "that is not a step on the checklist of \"Review: the connector\""),
        (
            c.triage,
            first,
            "moved",
            false,
            "only the creator and the assignee of \"Review: the connector\" take a step off its checklist; ask @dnorman or @impl-alice",
        ),
        (c.implementer, first, "  ", false, "say why the step comes off the checklist: the reason is kept on the step"),
        (
            c.implementer,
            second,
            "moved",
            true,
            "only a highlight is retired, and this step is a task (Read the retry loop); take it off without retiring it",
        ),
    ];
    for (by, edge, reason, retire, refusal) in refusals {
        let err = ops::remove_step(&ctx, by, review, edge, reason, retire).await.unwrap_err();
        assert_eq!(err.to_string(), refusal);
    }
    assert_eq!(listed(ctx.clone(), review).await?, vec![first, second]);
    assert_eq!(step_state(&ctx, second).await?, (Checked, Some(c.daniel)));

    // The assignee takes the checked task off: its relation is retracted with who, when and why,
    // the task stays, and so does its step on the other checklist.
    let retired = ops::remove_step(&ctx, c.implementer, review, second, " moved to its own review ", false).await?;
    assert!(retired.is_empty(), "{retired:?}");
    let off: RelationView = ctx.get(second).await?;
    assert_eq!(step_state(&ctx, second).await?, (Retracted, Some(c.implementer)));
    assert_eq!(off.closed_reason()?.as_deref(), Some("moved to its own review"));
    assert_eq!(listed(ctx.clone(), review).await?, vec![first]);
    assert_eq!(listed(ctx.clone(), other).await?, vec![elsewhere]);
    assert!(ctx.get::<TaskView>(part).await.is_ok());
    let err = ops::remove_step(&ctx, c.implementer, review, second, "again", false).await.unwrap_err();
    assert_eq!(err.to_string(), "that step was taken off the checklist of \"Review: the connector\" already");

    // The creator takes the highlight off without retiring it: the highlight and its thread stay,
    // the thread open.
    ops::remove_step(&ctx, c.daniel, review, first, "added by mistake", false).await?;
    assert!(listed(ctx.clone(), review).await?.is_empty());
    assert!(ctx.get::<CodeAnchorView>(highlight).await.is_ok());
    assert_eq!(threads_on(&ctx, highlight).await?, vec![thread]);
    assert_eq!(thread_state(&ctx, thread).await?, (MessageStatus::Active, None));
    assert_eq!(step_state(&ctx, elsewhere).await?.0, Accepted);

    // Nothing was added, so routing, which reacts to what is added, tells no one; nor would it of a
    // step taken off.
    assert_eq!(entities(&ctx).await?, before, "nothing was added");
    assert!(routing::plan_for_relation(&ctx, &ctx.get::<RelationView>(first).await?).await?.is_empty());
    Ok(())
}

/// Retiring a highlight as its step comes off (task 9dvGW0EI, above): every thread on it is
/// resolved, with who and when, so the editor stops drawing it, and its page still lists them;
/// its step on another task's checklist is checked with its first thread, as resolving that thread
/// checks it (docs/DECISIONS.md, D99, "Checking a step resolves its highlight's first thread, and
/// resolving that thread checks the step; each control says it does both"), and the result says so;
/// a reply stays as it is; nothing is added, so nobody is told.
#[tokio::test]
async fn retiring_a_highlight_resolves_its_threads_and_its_page_still_lists_them() -> Result<()> {
    use MessageStatus::{Active, Resolved as Settled};
    use RelationStatus::{Resolved as Checked, Retracted};
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let review = ops::create_task(&ctx, c.daniel, task("Review: the retry loop")).await?;
    let smaller = ops::create_task(&ctx, c.daniel, task("Review: the retry loop, part one")).await?;
    let highlight = anchor(&ctx, c.implementer, EntityId::random(), "src/ws.rs", Some((3, 8))).await?;
    let on_highlight = MessageContext { anchor_id: Some(highlight), ..Default::default() };
    let first =
        ops::post_message(&ctx, c.implementer, "the loop never backs off", Priority::Next, on_highlight.clone())
            .await?;
    // Two threads started within the same millisecond would be ordered by id.
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let later = ops::post_message(&ctx, c.daniel, "and why no jitter?", Priority::Next, on_highlight).await?;
    let answer = MessageContext { reply_to: Some(first), ..Default::default() };
    let reply = ops::post_message(&ctx, c.daniel, "agreed", Priority::Next, answer).await?;
    let step = ops::add_step(&ctx, c.implementer, review, highlight).await?;
    let there = ops::add_step(&ctx, c.implementer, smaller, highlight).await?;
    let before = entities(&ctx).await?;

    let retired = ops::remove_step(&ctx, c.daniel, review, step, "replaced by smaller reviews", true).await?;
    assert_eq!(retired.messages, vec![first, later], "each thread's first message, oldest first");
    assert_eq!(retired.steps, vec![(there, Checked)], "the step on the other checklist moves with the first thread");
    assert_eq!(thread_state(&ctx, first).await?, (Settled, Some(c.daniel)));
    assert_eq!(thread_state(&ctx, later).await?, (Settled, Some(c.daniel)));
    assert_eq!(thread_state(&ctx, reply).await?, (Active, None), "a reply starts no thread");
    assert_eq!(step_state(&ctx, step).await?, (Retracted, Some(c.daniel)));
    assert_eq!(step_state(&ctx, there).await?, (Checked, Some(c.daniel)));
    assert!(ops::steps(&ctx, review).await?.is_empty());
    // The highlight stays, and its page lists both threads, resolved now.
    assert!(ctx.get::<CodeAnchorView>(highlight).await.is_ok());
    assert_eq!(threads_on(&ctx, highlight).await?, vec![first, later]);
    assert_eq!(entities(&ctx).await?, before, "nothing was added, so routing tells no one");
    Ok(())
}

/// A task's kind is a label (docs/DECISIONS.md, D97, "A task's kind, such as bug or feature, is a
/// label, not a task type"): a task is filed with labels and labelled and unlabelled afterwards,
/// each label stored in one form (trimmed, single spaces, lower case) and once; a blank label, one
/// holding a comma, or one both added and taken off is refused, and the refusal writes nothing.
#[tokio::test]
async fn a_task_is_labelled_and_unlabelled_after_it_is_filed() -> Result<()> {
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let labels = |view: TaskView| ops::labels_of(&view).unwrap();
    let spec =
        CreateTask { labels: vec!["Bug".into(), " bug ".into(), "Page  Forms".into()], ..task("Fix the preview") };
    let t = ops::create_task(&ctx, c.daniel, spec).await?;
    assert_eq!(labels(ctx.get(t).await?), ["bug", "page forms"], "each once, in the one form");
    let blank = CreateTask { labels: vec!["bug".into(), " ".into()], ..task("Filed with a blank label") };
    let err = ops::create_task(&ctx, c.daniel, blank).await.unwrap_err();
    assert_eq!(err.to_string(), "a label cannot be blank");
    let filed: Vec<TaskView> = ctx.fetch("created > 0").await?;
    assert!(filed.iter().all(|f| f.title().unwrap() != "Filed with a blank label"), "a refused task is not filed");

    ops::label_task(&ctx, t, "Feature").await?;
    let tv: TaskView = ctx.get(t).await?;
    assert_eq!(labels(tv.clone()), ["bug", "page forms", "feature"], "added after those it had");
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    ops::label_task(&ctx, t, " FEATURE").await?;
    ops::unlabel_task(&ctx, t, "ui").await?;
    let again: TaskView = ctx.get(t).await?;
    assert_eq!(
        (labels(again.clone()), again.updated()?),
        (labels(tv.clone()), tv.updated()?),
        "a label it has, or one it lacks, changes nothing, not even when the task changed"
    );
    ops::unlabel_task(&ctx, t, "page forms").await?;
    assert_eq!(labels(ctx.get(t).await?), ["bug", "feature"]);
    assert_eq!(ops::label_task(&ctx, t, "  ").await.unwrap_err().to_string(), "a label cannot be blank");
    assert_eq!(
        ops::label_task(&ctx, t, "bug, ui").await.unwrap_err().to_string(),
        "a label cannot hold a comma, which separates labels: give `bug, ui` as separate labels"
    );

    // A label written before the rules compares in the one form.
    let written_earlier: TaskView = ctx.get(t).await?;
    let trx = ctx.begin();
    written_earlier.edit(&trx)?.labels()?.set(&ankurah::property::Json::new(serde_json::json!(["Web", "bug"])))?;
    trx.commit().await?;
    ops::label_task(&ctx, t, "web").await?;
    assert_eq!(labels(ctx.get(t).await?), ["Web", "bug"], "`web` is the label `Web`");
    ops::unlabel_task(&ctx, t, "WEB").await?;
    assert_eq!(labels(ctx.get(t).await?), ["bug"]);

    // Amending adds and takes off labels with the other fields, in one transaction.
    let amend = ops::Amend {
        title: Some("Fix the hover preview".into()),
        label: vec!["ui".into(), "feature".into()],
        unlabel: vec!["Bug".into()],
        ..Default::default()
    };
    ops::amend(&ctx, t, amend).await?;
    let tv: TaskView = ctx.get(t).await?;
    assert_eq!(
        (tv.title()?, labels(tv.clone())),
        ("Fix the hover preview".into(), vec!["ui".into(), "feature".into()])
    );
    let contradiction = ops::Amend {
        title: Some("Never written".into()),
        label: vec!["UI".into()],
        unlabel: vec!["ui".into()],
        ..Default::default()
    };
    let err = ops::amend(&ctx, t, contradiction).await.unwrap_err();
    assert_eq!(err.to_string(), "`ui` is both added and taken off; give it once");
    assert_eq!(ctx.get::<TaskView>(t).await?.title()?, "Fix the hover preview", "the refusal writes nothing");
    Ok(())
}

/// Adds `step` to `task`'s checklist as `by` and routes the new relation, as the daemon would.
async fn routed_step(ctx: &Context, by: EntityId, task: EntityId, step: EntityId) -> Result<EntityId> {
    let edge = ops::add_step(ctx, by, task, step).await?;
    let view: RelationView = ctx.get(edge).await?;
    routing::apply(ctx, &routing::plan_for_relation(ctx, &view).await?).await?;
    Ok(edge)
}

/// Posts `text` as `by` about `mc` and routes it, as the daemon would.
async fn routed_message(ctx: &Context, by: EntityId, text: &str, mc: MessageContext) -> Result<EntityId> {
    let message = ops::post_message(ctx, by, text, Priority::Next, mc).await?;
    let view: MessageView = ctx.get(message).await?;
    routing::apply(ctx, &routing::plan_for_message(ctx, &view).await?).await?;
    Ok(message)
}

/// `who`'s unread notifications, each as its type and the batch it holds, if any.
async fn unread_batches(ctx: &Context, who: EntityId) -> Result<Vec<(NotificationType, Option<routing::Batch>)>> {
    let mut out = Vec::new();
    for n in ops::inbox(ctx, who, true).await? {
        out.push((n.notification_type()?, routing::batch_of(&n.originator()?.into())));
    }
    Ok(out)
}

/// Rule 10 (D95, "A burst of comments and additions from one author on one task reaches a person
/// as one notification"): what one author does on one task reaches each recipient as one
/// notification while it stays unread, listing its items; reading it ends it; a reply, another
/// author's comment and a mention keep notifications of their own, and an item routed twice is
/// held once.
#[tokio::test]
async fn a_burst_from_one_author_on_one_task_is_one_notification_per_person() -> Result<()> {
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let alice = c.implementer;
    // Alice makes a review for Daniel and adds three highlights to it, each with a first comment.
    let review = ops::create_task(&ctx, alice, task("Review: the retry loop")).await?;
    ops::assign(&ctx, alice, review, Some(c.daniel)).await?;
    let mut comments = Vec::new();
    for n in 1..=3 {
        let highlight = EntityId::random();
        routed_step(&ctx, alice, review, highlight).await?;
        let on = MessageContext { anchor_id: Some(highlight), ..Default::default() };
        comments.push(routed_message(&ctx, alice, &format!("look at part {n}"), on).await?);
    }
    let heard = unread_batches(&ctx, c.daniel).await?;
    assert_eq!(heard.len(), 1, "one notification for the burst: {heard:?}");
    let (kind, batch) = heard[0].clone();
    let batch = batch.expect("it lists what it holds");
    assert_eq!(kind, NotificationType::Digest);
    assert_eq!((batch.task, batch.steps(), batch.comments()), (review, 3, 3));
    let kinds: Vec<bool> = batch.items.iter().map(|i| matches!(i, routing::BatchItem::Step { .. })).collect();
    assert_eq!(kinds, vec![true, false, true, false, true, false], "each step, then its first comment");
    let digest = &ops::inbox(&ctx, c.daniel, true).await?[0];
    let originator: serde_json::Value = digest.originator()?.into();
    assert_eq!(originator["message_id"], comments[2].to_base64(), "it names the latest comment");
    assert_eq!(originator["why"], "step_party");
    assert_eq!(serde_json::Value::from(digest.target()?)["id"], review.to_base64(), "it is about the task");
    assert!(digest.updated()?.is_some(), "it grew");
    assert_eq!(
        fwd_core::reason::plain(&fwd_core::reason::gather(&ctx, digest, Some(c.daniel)).await?),
        "impl-alice added 3 steps and commented 3 times on your task Review: the retry loop"
    );
    // The triage agent hears the comments on code as one notification too; Alice hears nothing.
    let triaged = unread_batches(&ctx, c.triage).await?;
    assert_eq!(triaged.len(), 1, "{triaged:?}");
    assert_eq!(triaged[0].1.as_ref().map(|b| (b.steps(), b.comments())), Some((0, 3)));
    assert!(ops::inbox(&ctx, alice, true).await?.is_empty(), "a person's own items never notify them");

    // An item routed twice (the event delivered again) is held once.
    let view: MessageView = ctx.get(comments[0]).await?;
    routing::apply(&ctx, &routing::plan_for_message(&ctx, &view).await?).await?;
    assert_eq!(unread_batches(&ctx, c.daniel).await?, heard);

    // Reading it ends the batch: the next step and its comment start another.
    ops::set_notification_status(&ctx, &[digest.id()], NotificationStatus::Read).await?;
    let fourth = EntityId::random();
    routed_step(&ctx, alice, review, fourth).await?;
    let on_fourth = MessageContext { anchor_id: Some(fourth), ..Default::default() };
    routed_message(&ctx, alice, "and part 4", on_fourth).await?;
    let heard = unread_batches(&ctx, c.daniel).await?;
    assert_eq!(heard.len(), 1, "{heard:?}");
    assert_eq!(heard[0].1.as_ref().map(|b| (b.steps(), b.comments())), Some((1, 1)));
    assert_eq!(ops::inbox(&ctx, c.daniel, false).await?.len(), 2, "the one read, and the new one");
    // An item of the batch that was read, routed again, is still held once.
    routing::apply(&ctx, &routing::plan_for_message(&ctx, &view).await?).await?;
    assert_eq!(unread_batches(&ctx, c.daniel).await?, heard);

    // A reply in a step's thread is its own notification: Daniel asks under the first comment,
    // and Alice's answer reaches him as a reply, leaving the batch as it was.
    let question = routed_message(
        &ctx,
        c.daniel,
        "why a loop?",
        MessageContext { reply_to: Some(comments[0]), ..Default::default() },
    )
    .await?;
    routed_message(&ctx, alice, "it retries", MessageContext { reply_to: Some(question), ..Default::default() })
        .await?;
    let heard = unread_batches(&ctx, c.daniel).await?;
    let mut kinds: Vec<NotificationType> = heard.iter().map(|(t, _)| *t).collect();
    kinds.sort();
    assert_eq!(kinds, vec![NotificationType::Replied, NotificationType::Digest]);
    assert!(heard.iter().any(|(_, b)| b.as_ref().is_some_and(|b| b.items.len() == 2)), "the batch did not grow");

    // Another author's comment on the task is theirs: Bob's makes a notification of its own.
    let bob =
        ops::ensure_principal(&ctx, "impl-bob", "Bob", PrincipalKind::Agent, Some(AgentRole::Implementer)).await?;
    let on_review = MessageContext { task_id: Some(review), ..Default::default() };
    routed_message(&ctx, bob, "I can take part 2", on_review.clone()).await?;
    let heard = unread_batches(&ctx, c.daniel).await?;
    assert_eq!(heard.len(), 3, "{heard:?}");
    assert!(heard
        .iter()
        .any(|(t, b)| *t == NotificationType::TaskCommented && b.as_ref().is_some_and(|b| b.items.len() == 1)));
    // Alice's own comment on the task joins her batch; a mention of Daniel in the next is his.
    routed_message(&ctx, alice, "all four are in", on_review.clone()).await?;
    routed_message(&ctx, alice, "@dnorman over to you", on_review).await?;
    let heard = unread_batches(&ctx, c.daniel).await?;
    let mut kinds: Vec<NotificationType> = heard.iter().map(|(t, _)| *t).collect();
    kinds.sort();
    assert_eq!(
        kinds,
        vec![
            NotificationType::TaskCommented,
            NotificationType::Mentioned,
            NotificationType::Replied,
            NotificationType::Digest
        ]
    );
    let alices: Vec<_> = heard.iter().filter_map(|(_, b)| b.clone()).filter(|b| b.items.len() > 1).collect();
    assert_eq!(alices.iter().map(|b| (b.steps(), b.comments())).collect::<Vec<_>>(), vec![(1, 2)]);
    Ok(())
}

/// Rule 13 for an edit beside rule 10: a person who heard of a comment as an item of a batch has
/// heard of it, though the batch's `message_id` names only its latest comment, so an edit that
/// newly names them in it tells them nothing more; someone it never reached is told.
#[tokio::test]
async fn an_edit_naming_someone_told_of_the_comment_in_a_batch_tells_them_nothing_more() -> Result<()> {
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let alice = c.implementer;
    let bob =
        ops::ensure_principal(&ctx, "impl-bob", "Bob", PrincipalKind::Agent, Some(AgentRole::Implementer)).await?;
    // Alice puts two highlights on Daniel's review, each with a first comment: Daniel holds one
    // notification, which names the second comment and holds the first among its items.
    let review = ops::create_task(&ctx, alice, task("Review: the retry loop")).await?;
    ops::assign(&ctx, alice, review, Some(c.daniel)).await?;
    let mut comments = Vec::new();
    for n in 1..=2 {
        let highlight = EntityId::random();
        routed_step(&ctx, alice, review, highlight).await?;
        let on = MessageContext { anchor_id: Some(highlight), ..Default::default() };
        comments.push(routed_message(&ctx, alice, &format!("look at part {n}"), on).await?);
    }
    let heard = ops::inbox(&ctx, c.daniel, true).await?;
    assert_eq!(heard.len(), 1, "{heard:?}");
    assert_eq!(serde_json::Value::from(heard[0].originator()?)["message_id"], comments[1].to_base64());

    // Alice's edit of the first comment names Daniel and Bob: only Bob hears of it.
    ops::edit_message(&ctx, alice, comments[0], "look at part 1, @dnorman and @impl-bob").await?;
    let mut told = Vec::new();
    for r in ops::relations_from(&ctx, comments[0], Some(RelationKind::Mention)).await? {
        told.extend(told_by(&ctx, &r).await?);
    }
    assert_eq!(told, vec![(bob, NotificationType::Mentioned, "mentioned".to_string())]);
    Ok(())
}

/// One unread notification as its recipient reads it: its id and type, what its batch holds (a
/// thread a step came with and a comment by their message, a step by what became the step), the
/// message a reply given its id answers, its priority and its sentence.
#[derive(Debug)]
struct Heard {
    id: EntityId,
    kind: NotificationType,
    items: Vec<(&'static str, EntityId)>,
    message: Option<EntityId>,
    priority: Priority,
    said: String,
}

/// `who`'s unread notifications, in their inbox's order, as [`Heard`].
async fn heard(ctx: &Context, who: EntityId) -> Result<Vec<Heard>> {
    let mut out = Vec::new();
    for n in ops::inbox(ctx, who, true).await? {
        let originator: serde_json::Value = n.originator()?.into();
        let batch = routing::batch_of(&originator).map(|b| b.items).unwrap_or_default();
        let items = batch
            .iter()
            .map(|i| match *i {
                routing::BatchItem::Thread { message, .. } => ("thread", message),
                routing::BatchItem::Comment { message, .. } => ("comment", message),
                routing::BatchItem::Step { step, .. } => ("step", step),
            })
            .collect();
        out.push(Heard {
            id: n.id(),
            kind: n.notification_type()?,
            items,
            message: originator["message_id"].as_str().and_then(|s| s.parse().ok()),
            priority: n.priority()?,
            said: fwd_core::reason::plain(&fwd_core::reason::gather(ctx, &n, Some(who)).await?),
        });
    }
    Ok(out)
}

/// Marks what `who` holds unread read, as reading their inbox does.
async fn read_all(ctx: &Context, who: EntityId) -> Result<()> {
    let ids: Vec<EntityId> = ops::inbox(ctx, who, true).await?.iter().map(|n| n.id()).collect();
    ops::set_notification_status(ctx, &ids, NotificationStatus::Read).await
}

/// How the sentence of the notification `n` ends for its recipient when it names the one message a
/// reply given its id answers.
fn reply_with(n: EntityId) -> String {
    format!("; reply with fwd reply {}", &n.to_base64()[..8])
}

/// Lets a moment pass, so that what is made next is made later by the clock.
async fn moment() {
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
}

/// How many of `who`'s notifications name `message`: as the message behind them, or among what
/// their batch lists.
async fn naming(ctx: &Context, who: EntityId, message: EntityId) -> Result<usize> {
    let mut count = 0;
    for n in ops::inbox(ctx, who, false).await? {
        let originator: serde_json::Value = n.originator()?.into();
        let listed = routing::batch_of(&originator).is_some_and(|b| b.messages().contains(&message));
        if listed || originator["message_id"] == message.to_base64() {
            count += 1;
        }
    }
    Ok(count)
}

/// Stamps `message` as written at `at`, as a device whose clock is off stamps what it writes.
async fn stamp_message(ctx: &Context, message: EntityId, at: i64) -> Result<()> {
    let view: MessageView = ctx.get(message).await?;
    let trx = ctx.begin();
    view.edit(&trx)?.created()?.set(&at)?;
    trx.commit().await?;
    Ok(())
}

/// Stamps the step `edge` as made at `at`, as a device whose clock is off stamps what it makes.
async fn stamp_step(ctx: &Context, edge: EntityId, at: i64) -> Result<()> {
    let view: RelationView = ctx.get(edge).await?;
    let trx = ctx.begin();
    view.edit(&trx)?.created()?.set(&at)?;
    trx.commit().await?;
    Ok(())
}

/// Routes the posting of `message` and the adding of the step `edge`, the message's first or the
/// step's, as the daemon's routing service does whichever of their events it reaches first.
async fn route_both(ctx: &Context, message: EntityId, edge: EntityId, message_first: bool) -> Result<()> {
    let (said, step): (MessageView, RelationView) = (ctx.get(message).await?, ctx.get(edge).await?);
    for message_now in [message_first, !message_first] {
        let planned = match message_now {
            true => routing::plan_for_message(ctx, &said).await?,
            false => routing::plan_for_relation(ctx, &step).await?,
        };
        routing::apply(ctx, &planned).await?;
    }
    Ok(())
}

/// A comment or a question written on lines before they become a step reaches the task's people
/// with the step (task WTCAd9la). On 2026-09-30 Daniel, reviewing in a scope with no triage agent,
/// commented on lines of the code under review and then added them to the review's checklist, and
/// asked the project about other lines and added those: the review's creator heard of two steps
/// with nothing in them, the comment reached no one, and the question waited for a triage agent.
/// Now the step added lists the threads open on its lines, naming the one it lists as the message
/// a reply given its id answers, and is as urgent as they are; the question still waits; Daniel
/// hears nothing of his own step.
#[tokio::test]
async fn a_thread_written_on_lines_before_they_become_a_step_reaches_the_tasks_people_with_it() -> Result<()> {
    let ctx = context().await?;
    // A project with no triage agent: Daniel, and the lead that made the review he works.
    let daniel = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    ops::join(&ctx, daniel).await?;
    let lead = ops::ensure_principal(&ctx, "lead-storage", "Storage lead", PrincipalKind::Agent, Some(AgentRole::Lead))
        .await?;
    let scope = ops::create_scope(&ctx, lead, named_scope("Storage engine")).await?;
    assert!(ops::triage_for(&ctx, Some(scope)).await?.is_none(), "no triage agent answers for the scope");
    let review = ops::create_task(&ctx, lead, task("Review: the policy context")).await?;
    ops::claim(&ctx, daniel, review, scope).await?;
    let repo = EntityId::random();
    let on = |anchor: EntityId| MessageContext { anchor_id: Some(anchor), ..Default::default() };

    // Daniel comments on lines no step holds yet, and no one hears of it.
    let lines = anchor(&ctx, daniel, repo, "core/src/policy/context.rs", Some((25, 27))).await?;
    let comment = routed_message(&ctx, daniel, "this clones the whole context", on(lines)).await?;
    assert!(ops::inbox(&ctx, lead, false).await?.is_empty(), "the comment reaches no one");
    // He adds the lines to the review's checklist: the lead hears of the step with the comment,
    // which a reply given the notification's id answers.
    moment().await;
    routed_step(&ctx, daniel, review, lines).await?;
    let told = heard(&ctx, lead).await?;
    assert_eq!(told.len(), 1, "{told:?}");
    assert_eq!((told[0].kind, told[0].priority), (NotificationType::StepAdded, Priority::Next));
    assert_eq!(told[0].items, vec![("thread", comment), ("step", lines)]);
    assert_eq!(told[0].message, Some(comment));
    assert_eq!(
        told[0].said,
        format!(
            "dnorman added a step with a comment to your task Review: the policy context{}",
            reply_with(told[0].id)
        )
    );
    assert!(ops::inbox(&ctx, daniel, false).await?.is_empty(), "his own comment and step tell him nothing");
    read_all(&ctx, lead).await?;

    // He asks the project about other lines, as urgently as a person's question asks by default,
    // and adds them: the question reaches no one, waiting for a triage agent, and the lead hears of
    // the step with it, as urgently. Whom it asks does not change once its lines are a step.
    let other = anchor(&ctx, daniel, repo, "core/src/policy.rs", Some((55, 57))).await?;
    let asked = MessageContext { asks_project: true, ..on(other) };
    let question = ops::post_message(&ctx, daniel, "why is this not cached?", Priority::Now, asked).await?;
    let view: MessageView = ctx.get(question).await?;
    assert!(routing::plan_for_message(&ctx, &view).await?.is_empty(), "the question waits");
    moment().await;
    routed_step(&ctx, daniel, review, other).await?;
    let told = heard(&ctx, lead).await?;
    assert_eq!(told.len(), 1, "{told:?}");
    assert_eq!((told[0].kind, told[0].priority), (NotificationType::StepAdded, Priority::Now));
    assert_eq!(told[0].items, vec![("thread", question), ("step", other)]);
    assert_eq!(
        told[0].said,
        format!(
            "dnorman added a step with a question to your task Review: the policy context{}",
            reply_with(told[0].id)
        )
    );
    assert!(routing::plan_for_message(&ctx, &view).await?.is_empty(), "the question waits still");

    // Two comments on other lines, which he adds while the lead has not read that: the step joins
    // it with both, and it names no message, since a reply given its id could not say which of
    // the comments and the question it answers.
    let third = anchor(&ctx, daniel, repo, "core/src/policy.rs", Some((80, 82))).await?;
    let first = routed_message(&ctx, daniel, "a lock here", on(third)).await?;
    moment().await;
    let second = routed_message(&ctx, daniel, "and one here", on(third)).await?;
    moment().await;
    routed_step(&ctx, daniel, review, third).await?;
    let told = heard(&ctx, lead).await?;
    assert_eq!(told.len(), 1, "{told:?}");
    assert_eq!((told[0].kind, told[0].priority), (NotificationType::Digest, Priority::Now));
    assert_eq!(
        told[0].items,
        vec![("thread", question), ("step", other), ("thread", first), ("thread", second), ("step", third)]
    );
    assert_eq!(told[0].message, None);
    assert_eq!(
        told[0].said,
        "dnorman added 2 steps with 2 comments and a question to your task Review: the policy context"
    );
    Ok(())
}

/// A project with no triage agent, as on 2026-09-30: Daniel, the lead that made a review, and the
/// review, which Daniel works; with the repository its lines are in.
async fn no_triage_review(ctx: &Context) -> Result<(EntityId, EntityId, EntityId, EntityId)> {
    let daniel = ops::ensure_principal(ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    ops::join(ctx, daniel).await?;
    let lead =
        ops::ensure_principal(ctx, "lead-storage", "Storage lead", PrincipalKind::Agent, Some(AgentRole::Lead)).await?;
    let scope = ops::create_scope(ctx, lead, named_scope("Storage engine")).await?;
    let review = ops::create_task(ctx, lead, task("Review: the policy context")).await?;
    ops::claim(ctx, daniel, review, scope).await?;
    Ok((daniel, lead, review, EntityId::random()))
}

/// Each of a task's people hears of a comment on a step's lines once, whichever of the comment and
/// the step routing reaches first, with both written by then, and whatever the clocks that stamped
/// them say (the lead's ruling on task WTCAd9la, 2026-10-02): each rule acts on what exists when
/// routing reaches its event and tells nothing more of a message to a party told of it already.
/// So the same two events read as a step with a comment, when routing reaches the step first, or
/// as a step and a comment on it, when it reaches the comment first; both are true and complete.
/// An event delivered again adds nothing. A comment stamped earlier than a step that existed before
/// it, by a device whose clock runs behind, still reaches the lead, and so does one a step stamped
/// earlier than it came after.
#[tokio::test]
async fn each_party_hears_of_a_comment_on_a_steps_lines_once_whichever_routing_reaches_first() -> Result<()> {
    let ctx = context().await?;
    let (daniel, lead, review, repo) = no_triage_review(&ctx).await?;
    let on = |anchor: EntityId| MessageContext { anchor_id: Some(anchor), ..Default::default() };
    let with_a_comment = "dnorman added a step with a comment to your task Review: the policy context";
    let and_commented = "dnorman added a step and commented on your task Review: the policy context";

    for (n, message_first) in [(0, true), (1, false)] {
        // The comment written before the step, both existing when routing reaches each.
        read_all(&ctx, lead).await?;
        let lines = anchor(&ctx, daniel, repo, "core/src/retry.rs", Some((10 * n, 10 * n + 2))).await?;
        let comment = ops::post_message(&ctx, daniel, "written first", Priority::Next, on(lines)).await?;
        moment().await;
        let edge = ops::add_step(&ctx, daniel, review, lines).await?;
        route_both(&ctx, comment, edge, message_first).await?;
        route_both(&ctx, comment, edge, !message_first).await?;
        let told = heard(&ctx, lead).await?;
        assert_eq!(told.len(), 1, "message first: {message_first}: {told:?}");
        assert_eq!(naming(&ctx, lead, comment).await?, 1, "message first: {message_first}");
        let (kind, items, said) = match message_first {
            true => (NotificationType::Digest, vec![("comment", comment), ("step", lines)], and_commented),
            false => (NotificationType::StepAdded, vec![("thread", comment), ("step", lines)], with_a_comment),
        };
        assert_eq!((told[0].kind, &told[0].items), (kind, &items), "message first: {message_first}");
        assert!(told[0].said.starts_with(said), "{}", told[0].said);

        // The comment written after the step, both existing when routing reaches each.
        read_all(&ctx, lead).await?;
        let lines = anchor(&ctx, daniel, repo, "core/src/backoff.rs", Some((10 * n, 10 * n + 2))).await?;
        let edge = ops::add_step(&ctx, daniel, review, lines).await?;
        moment().await;
        let comment = ops::post_message(&ctx, daniel, "written second", Priority::Next, on(lines)).await?;
        route_both(&ctx, comment, edge, message_first).await?;
        route_both(&ctx, comment, edge, !message_first).await?;
        let told = heard(&ctx, lead).await?;
        assert_eq!(told.len(), 1, "message first: {message_first}: {told:?}");
        assert_eq!(naming(&ctx, lead, comment).await?, 1, "message first: {message_first}");
        let (kind, items, said) = match message_first {
            true => (NotificationType::Digest, vec![("step", lines), ("comment", comment)], and_commented),
            false => (NotificationType::StepAdded, vec![("step", lines), ("thread", comment)], with_a_comment),
        };
        assert_eq!((told[0].kind, &told[0].items), (kind, &items), "message first: {message_first}");
        assert!(told[0].said.starts_with(said), "{}", told[0].said);

        // A comment that names the lead, which a mention tells in a notification of its own: the
        // lead hears of it once all the same, as a mention or with the step.
        read_all(&ctx, lead).await?;
        let lines = anchor(&ctx, daniel, repo, "core/src/cache.rs", Some((10 * n, 10 * n + 2))).await?;
        let comment = ops::post_message(&ctx, daniel, "@lead-storage, see here", Priority::Next, on(lines)).await?;
        moment().await;
        let edge = ops::add_step(&ctx, daniel, review, lines).await?;
        route_both(&ctx, comment, edge, message_first).await?;
        assert_eq!(naming(&ctx, lead, comment).await?, 1, "message first: {message_first}");
        let kinds: Vec<NotificationType> = heard(&ctx, lead).await?.iter().map(|h| h.kind).collect();
        let expected = match message_first {
            true => vec![NotificationType::Mentioned, NotificationType::StepAdded],
            false => vec![NotificationType::StepAdded],
        };
        assert_eq!(kinds, expected, "message first: {message_first}");
    }

    // A step exists and routing has reached it; a minute later Daniel comments on its lines from a
    // device whose clock runs two minutes behind: the comment, stamped before the step, reaches
    // the lead all the same, joining the notification of the step.
    read_all(&ctx, lead).await?;
    let lines = anchor(&ctx, daniel, repo, "core/src/lock.rs", Some((1, 3))).await?;
    let edge = routed_step(&ctx, daniel, review, lines).await?;
    let comment = ops::post_message(&ctx, daniel, "stamped early", Priority::Next, on(lines)).await?;
    let step_made = ctx.get::<RelationView>(edge).await?.created()?;
    stamp_message(&ctx, comment, step_made - 120_000).await?;
    let view: MessageView = ctx.get(comment).await?;
    routing::apply(&ctx, &routing::plan_for_message(&ctx, &view).await?).await?;
    let told = heard(&ctx, lead).await?;
    assert_eq!(told.len(), 1, "{told:?}");
    assert_eq!(
        told[0].items,
        vec![("comment", comment), ("step", lines)],
        "ordered by the stamps, which decide nothing"
    );
    assert_eq!(naming(&ctx, lead, comment).await?, 1);

    // Daniel comments on lines and routing reaches the comment; he adds them from a device whose
    // clock runs two minutes behind: the step, stamped before the comment, still lists it.
    read_all(&ctx, lead).await?;
    let lines = anchor(&ctx, daniel, repo, "core/src/lock.rs", Some((20, 22))).await?;
    let comment = routed_message(&ctx, daniel, "written before, stamped after", on(lines)).await?;
    let edge = ops::add_step(&ctx, daniel, review, lines).await?;
    let written = ctx.get::<MessageView>(comment).await?.created()?;
    stamp_step(&ctx, edge, written - 120_000).await?;
    let step: RelationView = ctx.get(edge).await?;
    routing::apply(&ctx, &routing::plan_for_relation(&ctx, &step).await?).await?;
    let told = heard(&ctx, lead).await?;
    assert_eq!(told.len(), 1, "{told:?}");
    assert_eq!(told[0].items, vec![("step", lines), ("thread", comment)]);
    assert_eq!(naming(&ctx, lead, comment).await?, 1);
    Ok(())
}

/// Whom a step's threads are listed for (task WTCAd9la): each of the task's creator and assignee
/// is listed the threads open on the step's lines that they did not write in, are not asked by and
/// were not told of, a thread being a first message that names no task and is not resolved;
/// whoever added the step, and anyone neither, hears nothing of it. A question asked of no one on
/// the lines reaches the triage agent alone, as before, and is listed with the step for the task's
/// people all the same; one asked of a party is left out of that party's list, since it reaches
/// them as a question whichever of it and the step routing reaches first, and is listed for the
/// other party.
#[tokio::test]
async fn a_step_lists_for_each_of_its_people_the_threads_they_did_not_write_and_were_not_told_of() -> Result<()> {
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let (alice, triage) = (c.implementer, c.triage);
    let bob =
        ops::ensure_principal(&ctx, "impl-bob", "Bob", PrincipalKind::Agent, Some(AgentRole::Implementer)).await?;
    // Alice made the review, and Bob is assigned it; Bob works a task anchored near its lines too,
    // so that a comment on them reaches him as that task's assignee.
    let review = ops::create_task(&ctx, alice, task("Review: the retry loop")).await?;
    ops::assign(&ctx, alice, review, Some(bob)).await?;
    let repo = EntityId::random();
    let backoff = ops::create_task(&ctx, c.daniel, task("Back off between retries")).await?;
    let anchored = anchor(&ctx, c.daniel, repo, "src/retry.rs", Some((10, 20))).await?;
    ops::link(&ctx, c.daniel, RelationKind::AnchoredAt, backoff, anchored, None, false, serde_json::json!({})).await?;
    ops::claim(&ctx, bob, backoff, c.scope).await?;
    let lines = anchor(&ctx, c.daniel, repo, "src/retry.rs", Some((12, 14))).await?;
    let on = MessageContext { anchor_id: Some(lines), ..Default::default() };

    // On the lines: Daniel's comment, which reaches Bob, and in whose thread Alice replies;
    // Daniel's question asked of no one, which reaches the triage agent alone; Alice's comment;
    // the triage agent's comment naming Bob's task, a comment on that task; Daniel's comment,
    // resolved since; and Daniel's comment of a moment ago, which routing has not reached yet.
    let comment = routed_message(&ctx, c.daniel, "this loop never sleeps", on.clone()).await?;
    moment().await;
    let of_no_one = MessageContext { asks_project: true, ..on.clone() };
    let question = ops::post_message(&ctx, c.daniel, "why no backoff?", Priority::Next, of_no_one).await?;
    assert_eq!(reasons(&ctx, question).await?, vec![(triage, NotificationType::QuestionAsked, "triage".into())]);
    let view: MessageView = ctx.get(question).await?;
    routing::apply(&ctx, &routing::plan_for_message(&ctx, &view).await?).await?;
    moment().await;
    routed_message(&ctx, alice, "it is on purpose", on.clone()).await?;
    let answering = MessageContext { reply_to: Some(comment), ..Default::default() };
    routed_message(&ctx, alice, "it sleeps upstream", answering).await?;
    routed_message(&ctx, triage, "for the backoff", MessageContext { task_id: Some(backoff), ..on.clone() }).await?;
    let settled = routed_message(&ctx, c.daniel, "never mind the name", on.clone()).await?;
    ops::resolve_message(&ctx, c.daniel, settled).await?;
    for who in [c.daniel, alice, bob, triage] {
        read_all(&ctx, who).await?;
    }
    moment().await;
    let of_bob = MessageContext { to: Some(bob.to_base64()), ..on.clone() };
    let to_bob = ops::post_message(&ctx, c.daniel, "can the loop take a budget?", Priority::Next, of_bob).await?;
    moment().await;

    // Daniel adds the lines to the review's checklist. Alice, who made the review, is listed his
    // question asked of no one and his question to Bob, not his comment, in whose thread she wrote,
    // nor her own comment.
    routed_step(&ctx, c.daniel, review, lines).await?;
    let told = heard(&ctx, alice).await?;
    assert_eq!(told.len(), 1, "{told:?}");
    assert_eq!(told[0].items, vec![("thread", question), ("thread", to_bob), ("step", lines)]);
    assert_eq!(told[0].message, None, "a reply given its id could not say which it answers");
    assert_eq!(told[0].said, "dnorman added a step with 2 questions to your task Review: the retry loop");
    // Bob, its assignee, heard of the comments as the assignee of the task near the lines, and the
    // question to him is his through rule 5: he is listed the question asked of no one alone,
    // which a reply given the notification's id answers.
    let told = heard(&ctx, bob).await?;
    assert_eq!(told.len(), 1, "{told:?}");
    assert_eq!(told[0].items, vec![("thread", question), ("step", lines)]);
    assert_eq!(told[0].message, Some(question));
    assert_eq!(
        told[0].said,
        format!("dnorman added a step with a question to your task Review: the retry loop{}", reply_with(told[0].id))
    );
    // Routing reaches the question to Bob after the step: it asks Bob, and tells no one else.
    let view: MessageView = ctx.get(to_bob).await?;
    routing::apply(&ctx, &routing::plan_for_message(&ctx, &view).await?).await?;
    assert_eq!((naming(&ctx, bob, to_bob).await?, naming(&ctx, alice, to_bob).await?), (1, 1));
    // The triage agent, neither the review's creator nor its assignee, and Daniel, who added the
    // step, hear nothing of it.
    assert!(heard(&ctx, triage).await?.iter().all(|h| h.kind != NotificationType::StepAdded));
    assert!(heard(&ctx, c.daniel).await?.is_empty());

    // The triage agent's own task: a question asked of no one on other lines, which asks the
    // triage agent, is left out of its list when routing reaches the step first, and reaches it as
    // a question once routing reaches the question.
    let filed = ops::create_task(&ctx, triage, task("Retries need a budget")).await?;
    let budget = anchor(&ctx, c.daniel, repo, "src/budget.rs", Some((1, 4))).await?;
    let asked = MessageContext { asks_project: true, anchor_id: Some(budget), ..Default::default() };
    let asked = ops::post_message(&ctx, c.daniel, "who sets the budget?", Priority::Next, asked).await?;
    moment().await;
    read_all(&ctx, triage).await?;
    routed_step(&ctx, c.daniel, filed, budget).await?;
    let told = heard(&ctx, triage).await?;
    assert_eq!(told.len(), 1, "{told:?}");
    assert_eq!(told[0].items, vec![("step", budget)]);
    let view: MessageView = ctx.get(asked).await?;
    routing::apply(&ctx, &routing::plan_for_message(&ctx, &view).await?).await?;
    assert_eq!(naming(&ctx, triage, asked).await?, 1);
    assert!(heard(&ctx, triage).await?.iter().any(|h| h.kind == NotificationType::QuestionAsked));
    Ok(())
}

/// A step's first comment, written by the command that adds the step (`fwd step add <task> --path
/// ... <text>`, `ops::add_highlight_step`), is that step's own (the lead's ruling on task WTCAd9la,
/// 2026-10-02): the comment names its step (`ops::FIRST_OF_STEP_KEY`), the step's list leaves it out
/// and rule 14 reports it, so the task's people read that a step was added and commented on
/// whichever of the two routing reaches first, both on a new highlight, which the command writes
/// with its step and its comment in one transaction, and on one made before. Another task's step
/// made later of the same lines lists it as any thread.
#[tokio::test]
async fn a_steps_first_comment_written_with_it_is_the_steps_own_whichever_routing_reaches_first() -> Result<()> {
    let ctx = context().await?;
    let (daniel, lead, review, repo) = no_triage_review(&ctx).await?;
    let said = "dnorman added a step and commented on your task Review: the policy context";
    let mut lines = 0;
    for message_first in [true, false] {
        for made_before in [false, true] {
            read_all(&ctx, lead).await?;
            lines += 3;
            let spec = anchor_spec("core/src/first.rs", Some((lines, lines + 2)));
            if made_before {
                ops::create_anchor(&ctx, daniel, repo, &spec).await?;
            }
            let on = ops::highlight(&ctx, daniel, repo, &spec).await?;
            assert_eq!(matches!(on, ops::Highlight::Made(_)), made_before);
            let (highlight, edge, comment) =
                ops::add_highlight_step(&ctx, daniel, review, on, Some("the first thing to see")).await?;
            let comment = comment.expect("the step's first comment");
            let context = serde_json::Value::from(ctx.get::<MessageView>(comment).await?.context()?);
            assert_eq!(context[ops::FIRST_OF_STEP_KEY], edge.to_base64(), "it names its step");
            route_both(&ctx, comment, edge, message_first).await?;
            let told = heard(&ctx, lead).await?;
            let case = format!("message first: {message_first}, made before: {made_before}");
            assert_eq!(told.len(), 1, "{case}: {told:?}");
            assert_eq!((told[0].kind, told[0].said.as_str()), (NotificationType::Digest, said), "{case}");
            let mut items = told[0].items.clone();
            items.sort();
            assert_eq!(items, vec![("comment", comment), ("step", highlight)], "{case}");
            assert_eq!(naming(&ctx, lead, comment).await?, 1, "{case}");
        }
    }

    // A second review, the lead's too and Bob's to do, takes the last lines as a step: the comment
    // written with the first review's step is a thread on them like any other, which Bob is listed,
    // while the lead heard of it with the first review.
    read_all(&ctx, lead).await?;
    let bob =
        ops::ensure_principal(&ctx, "impl-bob", "Bob", PrincipalKind::Agent, Some(AgentRole::Implementer)).await?;
    let other = ops::create_task(&ctx, lead, task("Review: the policy cache")).await?;
    ops::assign(&ctx, lead, other, Some(bob)).await?;
    let spec = anchor_spec("core/src/first.rs", Some((lines, lines + 2)));
    let ops::Highlight::Made(highlight) = ops::highlight(&ctx, daniel, repo, &spec).await? else {
        anyhow::bail!("the lines are a highlight already");
    };
    let first = ops::messages_about(&ctx, "anchor", highlight).await?.first().map(|m| m.id());
    routed_step(&ctx, daniel, other, highlight).await?;
    let told = heard(&ctx, bob).await?;
    assert_eq!(told.len(), 1, "{told:?}");
    assert_eq!(told[0].items, vec![("thread", first.expect("the first review's comment")), ("step", highlight)]);
    let told = heard(&ctx, lead).await?;
    assert_eq!(told.len(), 1, "{told:?}");
    assert_eq!(told[0].items, vec![("step", highlight)], "the lead heard of the comment with the first review");
    Ok(())
}

/// An agent that succeeded a thread's writer is listed nothing of that thread (task WTCAd9la;
/// docs/DECISIONS.md, D114): the lead that made a review comments on its lines and is relaunched
/// as a successor, and Daniel adds the lines to the review's checklist: the successor, which hears
/// for the lead, is listed nothing of its predecessor's comment, which the review's assignee is.
#[tokio::test]
async fn a_step_lists_nothing_to_the_successor_of_a_threads_writer() -> Result<()> {
    let ctx = context().await?;
    let (daniel, lead, review, repo) = no_triage_review(&ctx).await?;
    let bob =
        ops::ensure_principal(&ctx, "impl-bob", "Bob", PrincipalKind::Agent, Some(AgentRole::Implementer)).await?;
    ops::assign(&ctx, lead, review, Some(bob)).await?;
    let lines = anchor(&ctx, daniel, repo, "core/src/policy.rs", Some((5, 7))).await?;
    let on = MessageContext { anchor_id: Some(lines), ..Default::default() };
    let comment = routed_message(&ctx, lead, "this is where the context is cloned", on).await?;
    let heir =
        ops::ensure_principal(&ctx, "lead-storage-2", "Storage lead", PrincipalKind::Agent, Some(AgentRole::Lead))
            .await?;
    ops::succeed_principal(&ctx, "lead-storage", heir).await?;
    moment().await;
    routed_step(&ctx, daniel, review, lines).await?;
    let told = heard(&ctx, heir).await?;
    assert_eq!(told.len(), 1, "{told:?}");
    assert_eq!((told[0].kind, &told[0].items), (NotificationType::StepAdded, &vec![("step", lines)]));
    let told = heard(&ctx, bob).await?;
    assert_eq!(told.len(), 1, "{told:?}");
    assert_eq!(told[0].items, vec![("thread", comment), ("step", lines)]);
    assert!(told[0].said.starts_with("dnorman added a step with a comment to your task"), "{}", told[0].said);
    Ok(())
}

/// Assignment is a field on the task (Daniel, 2026-09-28): set and cleared by `assign`, set by
/// starting; a task assigned to someone is started by them alone; a person with no scope starts a
/// task all the same; each scope a task is worked in is one `worked_in` relation, and a task may be
/// worked in several (docs/DECISIONS.md, D136, "A task may be worked in several scopes"; until
/// 2026-10-03 a second was refused).
#[tokio::test]
async fn a_task_is_assigned_started_and_worked_in_its_scopes() -> Result<()> {
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let assignee = |ctx: Context, t: EntityId| async move { ops::assignee_of(&ctx.get::<TaskView>(t).await?) };
    let a = ops::create_task(&ctx, c.daniel, task("A")).await?;
    assert_eq!(assignee(ctx.clone(), a).await?, None, "a new task is unassigned");
    ops::assign(&ctx, c.daniel, a, Some(c.implementer)).await?;
    assert_eq!(assignee(ctx.clone(), a).await?, Some(c.implementer));
    let task_context = serde_json::Value::from(ctx.get::<TaskView>(a).await?.context()?);
    assert_eq!(task_context[ops::ASSIGNED_BY_KEY], c.daniel.to_base64(), "who assigned it is kept");
    // Her frontier lists it first; Daniel's leaves it out.
    let b = ops::create_task(&ctx, c.daniel, task("B")).await?;
    let ids = |ts: Vec<TaskView>| ts.iter().map(|t| t.id()).collect::<Vec<_>>();
    assert_eq!(ids(ops::ready_tasks(&ctx, Some(c.implementer)).await?), vec![a, b]);
    assert_eq!(ids(ops::ready_tasks(&ctx, Some(c.daniel)).await?), vec![b]);
    // Someone else cannot start it; she can, in her scope.
    let err = ops::start(&ctx, c.daniel, a, None).await.unwrap_err();
    assert!(err.to_string().contains("assigned to @impl-alice"), "{err}");
    ops::start(&ctx, c.implementer, a, Some(c.scope)).await?;
    assert_eq!(ops::worked_in(&ctx, a).await?, vec![c.scope]);
    // Daniel has no scope and starts B all the same: he is its assignee; it is worked nowhere.
    ops::start(&ctx, c.daniel, b, None).await?;
    assert_eq!(assignee(ctx.clone(), b).await?, Some(c.daniel));
    assert_eq!(ops::worked_in(&ctx, b).await?, Vec::<EntityId>::new());
    // Unassigned again.
    ops::assign(&ctx, c.daniel, b, None).await?;
    assert_eq!(assignee(ctx.clone(), b).await?, None);
    assert!(ops::assign(&ctx, c.daniel, b, Some(a)).await.is_err(), "only a principal is an assignee");

    // Several scopes: a second `worked_in` adds a scope; linking a scope the task is worked in
    // already answers the relation there; and `assigned_to` is made no more. (Which of two scopes
    // linked within one millisecond comes first is their relations' ids' to say, so the order is
    // left to the migration's test, whose relations are made at times apart.)
    let other = ops::create_scope(&ctx, c.daniel, named_scope("Daniel by hand")).await?;
    let second_link =
        ops::link(&ctx, c.daniel, RelationKind::WorkedIn, a, other, None, false, serde_json::json!({})).await?;
    let both = sorted(vec![c.scope, other]);
    assert_eq!(sorted(ops::worked_in(&ctx, a).await?), both);
    for scope in [c.scope, other] {
        let edges = ops::relations_from(&ctx, a, Some(RelationKind::WorkedIn)).await?;
        let again =
            ops::link(&ctx, c.daniel, RelationKind::WorkedIn, a, scope, None, false, serde_json::json!({})).await?;
        let there: Vec<EntityId> = edges.iter().filter(|r| r.to_id().ok() == Some(scope)).map(|r| r.id()).collect();
        assert_eq!(vec![again], there);
    }
    assert_eq!(ops::relations_from(&ctx, a, Some(RelationKind::WorkedIn)).await?.len(), 2);
    let err = ops::link(&ctx, c.daniel, RelationKind::AssignedTo, b, other, None, false, serde_json::json!({}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("no longer made"), "{err}");

    // A reopened task is still worked in both its scopes, and a start never moves it: from a third
    // scope it is refused, naming the two and offering to bind the checkout to either; from either
    // it starts, and nothing is retracted.
    ops::transition(&ctx, c.implementer, a, Verb::Submit, Some("commit 1")).await?;
    ops::transition(&ctx, c.daniel, a, Verb::Accept, None).await?;
    ops::transition(&ctx, c.daniel, a, Verb::Reopen, Some("one more case")).await?;
    let third = ops::create_scope(&ctx, c.implementer, named_scope("Retry, again")).await?;
    let err = ops::start(&ctx, c.implementer, a, Some(third)).await.unwrap_err();
    let order = ops::worked_in(&ctx, a).await?;
    let (first, second) = (&order[0].to_base64()[..8], &order[1].to_base64()[..8]);
    assert!(
        err.to_string().ends_with(&format!(
            "or bind this checkout to one with `fwd scope attach-dir {first}` or `fwd scope attach-dir {second}`"
        )),
        "{err}"
    );
    ops::start(&ctx, c.implementer, a, Some(other)).await?;
    assert_eq!(ops::worked_in(&ctx, a).await?, order);
    let statuses: Vec<RelationStatus> =
        ops::relations_from(&ctx, a, Some(RelationKind::WorkedIn)).await?.iter().map(|r| r.status().unwrap()).collect();
    assert_eq!(statuses, vec![RelationStatus::Accepted; 2]);
    assert!(ops::relations_from(&ctx, a, None).await?.iter().any(|r| r.id() == second_link));

    // A scope taken off (`fwd unlink`) is one the task is worked in no more; with its last taken
    // off it is worked in none.
    assert_eq!(sorted(order), both);
    ops::unlink(&ctx, c.daniel, RelationKind::WorkedIn, a, c.scope, "the work moved").await?;
    assert_eq!(ops::worked_in(&ctx, a).await?, vec![other]);
    ops::unlink(&ctx, c.daniel, RelationKind::WorkedIn, a, other, "no scope fits").await?;
    assert_eq!(ops::worked_in(&ctx, a).await?, Vec::<EntityId>::new());
    Ok(())
}

/// The scopes on a task's panel change through one write (task oLv1sJ3v, "Edit a task's scopes on
/// the page as removable tags with a [+] scope search"): a scope added comes to be one the task is
/// worked in, a scope taken off is retracted with who took it off, a move takes one off and adds
/// another at once, and the last one taken off leaves the task in no scope; a scope already as
/// asked is left as it is; what is not a scope, and a scope both added and taken off, are refused
/// before anything is written; and none of it tells anyone (docs/DECISIONS.md, D136, "A task may be
/// worked in several scopes"; as attaching a decision to a scope tells no one, D93).
#[tokio::test]
async fn a_tasks_scopes_change_in_one_write_down_to_none_telling_no_one() -> Result<()> {
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let hosting = ops::create_scope(&ctx, c.daniel, named_scope("Cloud Hosting")).await?;
    let docs = ops::create_scope(&ctx, c.daniel, named_scope("Docs")).await?;
    let t = ops::create_task(&ctx, c.daniel, CreateTask { scope: Some(c.scope), ..task("Move me") }).await?;
    let scopes = |ctx: Context| async move { anyhow::Ok(sorted(ops::worked_in(&ctx, t).await?)) };
    // Each write, and what routing plans for every relation it made: nothing.
    let told = |ctx: Context| async move {
        let mut planned = Vec::new();
        for r in ops::relations_from(&ctx, t, Some(RelationKind::WorkedIn)).await? {
            planned.extend(routing::plan_for_relation(&ctx, &r).await?);
        }
        anyhow::Ok(planned.len())
    };

    // Added, and added again: one relation for the scope.
    ops::change_scopes(&ctx, c.implementer, t, &[hosting], &[]).await?;
    ops::change_scopes(&ctx, c.implementer, t, &[hosting], &[]).await?;
    assert_eq!(scopes(ctx.clone()).await?, sorted(vec![c.scope, hosting]));
    assert_eq!(ops::relations_from(&ctx, t, Some(RelationKind::WorkedIn)).await?.len(), 2);
    // Moved: Cloud Hosting off and Docs on, at once.
    ops::change_scopes(&ctx, c.implementer, t, &[docs], &[hosting]).await?;
    assert_eq!(scopes(ctx.clone()).await?, sorted(vec![c.scope, docs]));
    let off: Vec<RelationView> = ops::relations_from(&ctx, t, Some(RelationKind::WorkedIn))
        .await?
        .into_iter()
        .filter(|r| r.to_id().ok() == Some(hosting))
        .collect();
    assert_eq!(off.len(), 1);
    assert_eq!(off[0].status()?, RelationStatus::Retracted);
    assert_eq!(off[0].closed_by()?.map(EntityId::from), Some(c.implementer), "who took it off");
    // Taken off one by one, the last too: in no scope.
    ops::change_scopes(&ctx, c.daniel, t, &[], &[c.scope]).await?;
    ops::change_scopes(&ctx, c.daniel, t, &[], &[docs, hosting]).await?;
    assert_eq!(scopes(ctx.clone()).await?, Vec::<EntityId>::new());
    assert_eq!(told(ctx.clone()).await?, 0, "no one hears of a scope added or taken off");

    // Refused, writing nothing: a task where a scope goes; a scope both added and taken off.
    let other = ops::create_task(&ctx, c.daniel, task("Not a scope")).await?;
    let err = ops::change_scopes(&ctx, c.daniel, t, &[docs, other], &[]).await.unwrap_err();
    assert!(err.to_string().starts_with("refused: `worked_in` joins a task to a scope"), "{err}");
    let err = ops::change_scopes(&ctx, c.daniel, t, &[docs], &[docs]).await.unwrap_err();
    assert!(err.to_string().contains("both added to the task and taken off it"), "{err}");
    assert_eq!(scopes(ctx.clone()).await?, Vec::<EntityId>::new());
    Ok(())
}

/// The scope a name names among scopes in hand, as the page's box for a task's scopes reads it,
/// by the rule the command line reads one by (`ops::resolve_scope`): its title whatever its case
/// and the spaces around it, its whole id, or the start of its id; refused when it names none, or
/// several.
#[test]
fn a_scope_is_named_by_its_title_or_its_id_among_scopes_in_hand() {
    let (hosting, docs, twin) = (EntityId::random(), EntityId::random(), EntityId::random());
    let scopes = vec![(hosting, "Cloud Hosting".to_string()), (docs, "Docs".to_string()), (twin, "Docs".to_string())];
    assert_eq!(ops::scope_by_name(" cloud HOSTING ", &scopes).unwrap(), hosting);
    assert_eq!(ops::scope_by_name(&hosting.to_base64(), &scopes).unwrap(), hosting);
    assert_eq!(ops::scope_by_name(&docs.to_base64()[..10], &scopes).unwrap(), docs);
    assert!(ops::scope_by_name("Docs", &scopes).unwrap_err().to_string().starts_with("2 scopes have the title `Docs`"));
    assert_eq!(
        ops::scope_by_name("Hostng", &scopes).unwrap_err().to_string(),
        "no scope has the title `Hostng` or an id starting with it"
    );
    let gone = EntityId::random();
    assert!(ops::scope_by_name(&gone.to_base64(), &scopes)
        .unwrap_err()
        .to_string()
        .starts_with("no scope of this project"));
}

/// A start never moves a task out of the scope someone chose for it (docs/DECISIONS.md, D87,
/// "Starting a task worked in another scope is refused, with a prompt to switch to that scope
/// first", 2026-09-28): an agent working in one scope is refused a task worked in another, told
/// which scope that is and how to switch to it, and nothing is written; from that scope the task
/// starts and stays there; a task worked in no scope starts in the agent's; and `next` leaves a
/// task worked in another scope for whoever works there.
#[tokio::test]
async fn a_task_worked_in_another_scope_is_started_only_from_that_scope() -> Result<()> {
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let initial = ops::create_scope(&ctx, c.implementer, named_scope("Initial implementation")).await?;
    let hosting = ops::create_scope(&ctx, c.daniel, named_scope("Cloud Hosting")).await?;
    let deploy =
        ops::create_task(&ctx, c.daniel, CreateTask { priority: Some(1), ..task("Deploy the daemon") }).await?;
    ops::link(&ctx, c.daniel, RelationKind::WorkedIn, deploy, hosting, None, false, serde_json::json!({})).await?;
    let naming = ops::create_task(&ctx, c.daniel, CreateTask { priority: Some(2), ..task("Name the crates") }).await?;
    let next = |ctx: Context, scope: EntityId| async move {
        anyhow::Ok(ops::next_task(&ctx, c.implementer, Some(scope)).await?.map(|t| t.id()))
    };
    assert_eq!(next(ctx.clone(), initial).await?, Some(naming), "the task worked in Cloud Hosting is passed over");
    assert_eq!(next(ctx.clone(), hosting).await?, Some(deploy));

    // From Initial implementation: refused, naming Cloud Hosting, which Daniel made and the agent
    // may work in all the same, a scope having no owner (D108), so the agent switches with --scope
    // or binds its checkout to it; the task is not touched.
    let err = ops::start(&ctx, c.implementer, deploy, Some(initial)).await.unwrap_err();
    let short = &hosting.to_base64()[..8];
    assert_eq!(
        err.to_string(),
        format!(
            "task is in the scope \"Cloud Hosting\" ({short}), not in the one you work in; to start it, switch to that scope first: run the command again with `--scope {short}`, or bind this checkout to the scope with `fwd scope attach-dir {short}`"
        )
    );
    let view: TaskView = ctx.get(deploy).await?;
    assert_eq!((view.status()?, ops::assignee_of(&view)?), (TaskStatus::Open, None));

    // From Cloud Hosting: started, and still worked there, by the one relation it had.
    ops::start(&ctx, c.implementer, deploy, Some(hosting)).await?;
    let view: TaskView = ctx.get(deploy).await?;
    assert_eq!((view.status()?, ops::assignee_of(&view)?), (TaskStatus::InProgress, Some(c.implementer)));
    let edges: Vec<(EntityId, RelationStatus)> = ops::relations_from(&ctx, deploy, Some(RelationKind::WorkedIn))
        .await?
        .iter()
        .map(|r| (r.to_id().unwrap(), r.status().unwrap()))
        .collect();
    assert_eq!(edges, vec![(hosting, RelationStatus::Accepted)]);

    // A task worked in no scope starts in the agent's.
    ops::start(&ctx, c.implementer, naming, Some(initial)).await?;
    assert_eq!(ops::worked_in(&ctx, naming).await?, vec![initial]);
    // Nor does a start record the scope in the task's context, which nothing reads any more.
    let context = serde_json::Value::from(ctx.get::<TaskView>(naming).await?.context()?);
    assert!(context.get("scope_id").is_none(), "{context}");

    // A task worked in several scopes is offered by `next`, and started, from each of them, and
    // passed over from any other.
    let both = ops::create_task(&ctx, c.daniel, CreateTask { priority: Some(0), ..task("Ship the relay") }).await?;
    for scope in [hosting, initial] {
        ops::link(&ctx, c.daniel, RelationKind::WorkedIn, both, scope, None, false, serde_json::json!({})).await?;
    }
    let docs = ops::create_scope(&ctx, c.daniel, named_scope("Docs")).await?;
    assert_eq!(next(ctx.clone(), hosting).await?, Some(both));
    assert_eq!(next(ctx.clone(), initial).await?, Some(both));
    assert_eq!(next(ctx.clone(), docs).await?, None, "every ready task is worked in other scopes");
    ops::start(&ctx, c.implementer, both, Some(initial)).await?;
    assert_eq!(sorted(ops::worked_in(&ctx, both).await?), sorted(vec![hosting, initial]), "nothing moved");
    Ok(())
}

/// A task made in a scope is worked in it from the start, by one `worked_in` relation its maker
/// made, so that it is found there and started only there (task cToJvD22); one made in no scope is
/// worked in none until it is started. Making it records nothing of the scope in its context
/// (`scope_id`, until 2026-10-03), and a task made in a scope before 2026-09-30, when making it
/// wrote no `worked_in`, is in no scope until `fwd admin migrate` gives it the scope its context
/// records (`ops::migrate_recorded_scopes`), since nothing else reads that record any more
/// (docs/DECISIONS.md, D136, "A task may be worked in several scopes").
#[tokio::test]
async fn a_task_made_in_a_scope_is_worked_in_it_from_the_start() -> Result<()> {
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let made_in =
        ops::create_task(&ctx, c.daniel, CreateTask { scope: Some(c.scope), ..task("Cap the retries") }).await?;
    let edges: Vec<(EntityId, RelationStatus, EntityId)> =
        ops::relations_from(&ctx, made_in, Some(RelationKind::WorkedIn))
            .await?
            .iter()
            .map(|r| (r.to_id().unwrap(), r.status().unwrap(), r.created_by().unwrap().into()))
            .collect();
    assert_eq!(edges, vec![(c.scope, RelationStatus::Accepted, c.daniel)]);
    let context = serde_json::Value::from(ctx.get::<TaskView>(made_in).await?.context()?);
    assert!(context.get("scope_id").is_none(), "{context}");
    let hosting = ops::create_scope(&ctx, c.daniel, named_scope("Cloud Hosting")).await?;
    let err = ops::start(&ctx, c.implementer, made_in, Some(hosting)).await.unwrap_err();
    assert!(err.to_string().contains("task is in the scope \"Inbox delivery\""), "{err}");
    let nowhere = ops::create_task(&ctx, c.daniel, task("Name the crates")).await?;
    assert_eq!(ops::worked_in(&ctx, nowhere).await?, Vec::<EntityId>::new());

    // Made in the scope before 2026-09-30: its context alone records the scope, which places it in
    // no scope now, until the migration gives it that scope. One whose `worked_in` was taken off
    // stays in none, as does one whose record names no scope the graph holds.
    let recorded = |scope: EntityId| serde_json::json!({"scope_id": scope.to_base64()});
    let made_before = |title: &'static str, record: serde_json::Value| {
        let ctx = ctx.clone();
        async move {
            let t = ops::create_task(&ctx, c.daniel, task(title)).await?;
            let trx = ctx.begin();
            ctx.get::<TaskView>(t).await?.edit(&trx)?.context()?.set(&ankurah::property::Json::new(record))?;
            trx.commit().await?;
            anyhow::Ok(t)
        }
    };
    let older = made_before("Write the primer", recorded(c.scope)).await?;
    assert_eq!(ops::worked_in(&ctx, older).await?, Vec::<EntityId>::new());
    let taken_off = made_before("Write the guide", recorded(c.scope)).await?;
    ops::link(&ctx, c.daniel, RelationKind::WorkedIn, taken_off, hosting, None, false, serde_json::json!({})).await?;
    ops::unlink(&ctx, c.daniel, RelationKind::WorkedIn, taken_off, hosting, "not hosting work").await?;
    let unheld = made_before("Write the site", recorded(EntityId::random())).await?;
    assert_eq!(ops::migrate_recorded_scopes(&ctx, c.daniel).await?, vec![(older, c.scope)]);
    assert_eq!(ops::worked_in(&ctx, older).await?, vec![c.scope]);
    let placed = ops::relations_from(&ctx, older, Some(RelationKind::WorkedIn)).await?;
    assert_eq!(EntityId::from(placed[0].created_by()?), c.daniel, "made by whoever ran the migration");
    for left in [taken_off, unheld] {
        assert_eq!(ops::worked_in(&ctx, left).await?, Vec::<EntityId>::new());
    }
    assert!(ops::migrate_recorded_scopes(&ctx, c.daniel).await?.is_empty(), "a second run finds nothing to do");
    Ok(())
}

/// `fwd admin migrate` on scopes written before D108 ("A scope has no owner: scopes are not
/// assigned to principals"): each scope that holds an owner or a heartbeat has them cleared and
/// nothing else about it changes; a scope made since, which holds neither, is left alone; a
/// second run changes nothing.
#[tokio::test]
async fn the_migration_clears_the_owner_and_heartbeat_scopes_had_before_d108() -> Result<()> {
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let hosting = ops::create_scope(&ctx, c.daniel, named_scope("Cloud Hosting")).await?;
    // What a build from before scopes lost their owner stored on its scopes, written through the
    // fields it declared.
    for (scope, owner) in [(c.scope, c.implementer), (hosting, c.daniel)] {
        let before: before_d108::ScopeView = ctx.get(scope).await?;
        let trx = ctx.begin();
        let m = before.edit(&trx)?;
        m.owner()?.set(&Some(ankurah::property::Ref::new(owner)))?;
        m.last_heartbeat()?.set(&Some(1_790_000_000_000))?;
        m.heartbeat_ttl_s()?.set(&Some(600))?;
        trx.commit().await?;
    }
    let open_core = ops::create_scope(&ctx, c.daniel, named_scope("Open Core")).await?;
    let shown = |ctx: Context, id: EntityId| async move { ops::json_of_scope(&ctx.get::<ScopeView>(id).await?) };
    let before = shown(ctx.clone(), c.scope).await?;

    let done = ops::migrate_scopes(&ctx).await?;
    let mut cleared = done.cleared.clone();
    cleared.sort();
    let mut expected = vec![c.scope, hosting];
    expected.sort();
    assert_eq!(cleared, expected, "the scope made since holds neither, and is left alone");
    for id in [c.scope, hosting, open_core] {
        let after: before_d108::ScopeView = ctx.get(id).await?;
        let owner = after.owner()?.map(EntityId::from);
        assert_eq!((owner, after.last_heartbeat()?, after.heartbeat_ttl_s()?), (None, None, None));
    }
    assert_eq!(shown(ctx.clone(), c.scope).await?, before, "nothing else about a scope changes");
    let scope: ScopeView = ctx.get(c.scope).await?;
    assert_eq!(EntityId::from(scope.created_by()?), c.implementer, "who made it is still recorded");
    assert_eq!(ops::migrate_scopes(&ctx).await?, ops::ScopeMigration::default(), "a second run changes nothing");
    Ok(())
}

/// `fwd admin migrate` on a graph from the days of assignment as a relation: each task with an
/// accepted `assigned_to` and no assignee gets whoever made that scope (its owner, until scopes
/// lost their owner; of several, the newest), marked as carried over so no one is told; every
/// `assigned_to` becomes `worked_in`, each kept, a task being worked in any number of scopes
/// (docs/DECISIONS.md, D136; until 2026-10-03 a task kept the newest accepted one alone); a second
/// run changes nothing.
#[tokio::test]
async fn the_migration_carries_assignment_edges_over_to_the_field_once() -> Result<()> {
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let bob =
        ops::ensure_principal(&ctx, "impl-bob", "Bob", PrincipalKind::Agent, Some(AgentRole::Implementer)).await?;
    let bobs = ops::create_scope(&ctx, bob, named_scope("Bob's")).await?;
    let legacy = |ctx: Context, task: EntityId, scope: EntityId, status: RelationStatus, created: i64| async move {
        let trx = ctx.begin();
        let id = trx
            .create(&Relation {
                kind: RelationKind::AssignedTo,
                from_id: task,
                to_id: scope,
                status,
                created_by: ankurah::property::Ref::new(c.daniel),
                created,
                via: ankurah::property::Json::new(serde_json::json!({})),
                note: None,
                position: None,
                closed_by: None,
                closed: None,
                closed_reason: None,
            })
            .await?
            .id();
        trx.commit().await?;
        anyhow::Ok(id)
    };
    // Claimed the old way; one whose claim was retracted; one already assigned; one claimed twice
    // (the newer claim names the assignee, and both scopes are kept); one worked in a scope
    // already, which gains the scope it was claimed in and no assignee.
    let claimed = ops::create_task(&ctx, c.daniel, task("Claimed")).await?;
    let e1 = legacy(ctx.clone(), claimed, c.scope, RelationStatus::Accepted, 10).await?;
    let released = ops::create_task(&ctx, c.daniel, task("Released")).await?;
    let e2 = legacy(ctx.clone(), released, c.scope, RelationStatus::Retracted, 10).await?;
    let assigned = ops::create_task(&ctx, c.daniel, task("Assigned already")).await?;
    ops::assign(&ctx, c.daniel, assigned, Some(c.daniel)).await?;
    let e3 = legacy(ctx.clone(), assigned, c.scope, RelationStatus::Accepted, 10).await?;
    let twice = ops::create_task(&ctx, c.daniel, task("Claimed twice")).await?;
    let older = legacy(ctx.clone(), twice, c.scope, RelationStatus::Accepted, 10).await?;
    let newer = legacy(ctx.clone(), twice, bobs, RelationStatus::Accepted, 20).await?;
    let moved = ops::create_task(&ctx, c.daniel, task("Worked in a scope already")).await?;
    ops::link(&ctx, c.daniel, RelationKind::WorkedIn, moved, bobs, None, false, serde_json::json!({})).await?;
    let e5 = legacy(ctx.clone(), moved, c.scope, RelationStatus::Accepted, 10).await?;

    let done = ops::migrate_assignments(&ctx).await?;
    let mut assigned_now = done.assigned.clone();
    assigned_now.sort();
    let mut expected = vec![(claimed, c.implementer, c.scope), (twice, bob, bobs)];
    expected.sort();
    assert_eq!(assigned_now, expected);
    let mut renamed = done.renamed.clone();
    renamed.sort();
    let mut all = vec![e1, e2, e3, older, newer, e5];
    all.sort();
    assert_eq!(renamed, all);
    for kept in [e1, e3, older, newer, e5] {
        assert_eq!(ctx.get::<RelationView>(kept).await?.status()?, RelationStatus::Accepted, "none is retracted");
    }

    let who = |ctx: Context, t: EntityId| async move { ops::assignee_of(&ctx.get::<TaskView>(t).await?) };
    assert_eq!(who(ctx.clone(), claimed).await?, Some(c.implementer));
    assert_eq!(who(ctx.clone(), released).await?, None, "a retracted claim assigns no one");
    assert_eq!(who(ctx.clone(), assigned).await?, Some(c.daniel), "an assignee is never replaced");
    assert_eq!(who(ctx.clone(), twice).await?, Some(bob));
    assert_eq!(who(ctx.clone(), moved).await?, None);
    let carried = serde_json::Value::from(ctx.get::<TaskView>(claimed).await?.context()?);
    assert_eq!(carried[ops::ASSIGNED_BY_KEY], ops::MIGRATED);
    assert!(ctx.fetch::<RelationView>("kind = 'assigned_to'").await?.is_empty());
    // Each in the order the task came to be worked in its scopes: the old claims first.
    assert_eq!(ops::worked_in(&ctx, twice).await?, vec![c.scope, bobs]);
    assert_eq!(ops::worked_in(&ctx, moved).await?, vec![c.scope, bobs]);
    assert_eq!(ctx.get::<RelationView>(e2).await?.status()?, RelationStatus::Retracted);

    assert!(ops::migrate_assignments(&ctx).await?.is_empty(), "a second run changes nothing");
    Ok(())
}

fn ruling(title: &str) -> ops::CreateDecision {
    ops::CreateDecision {
        title: title.into(),
        decision: format!("{title}, in the decider's words"),
        ..Default::default()
    }
}

/// Decisions: numbered in order, moved only by the verbs, each move leaving its reason as a comment
/// on the decision in the same write, and named on a command line by `D<number>` or an id's start.
#[tokio::test]
async fn decisions_are_numbered_moved_by_verbs_and_named_by_number() -> Result<()> {
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let d1 = ops::create_decision(&ctx, c.daniel, ruling("Retries back off exponentially")).await?;
    let d2 =
        ops::create_decision(&ctx, c.implementer, ops::CreateDecision { propose: true, ..ruling("Retry forever") })
            .await?;
    let d3 = ops::create_decision(
        &ctx,
        c.triage,
        ops::CreateDecision {
            decided_by: Some(c.daniel),
            area: Some(" process ".into()),
            ..ruling("Agents record Daniel's rulings")
        },
    )
    .await?;
    let numbers: Vec<i64> = ops::decisions(&ctx).await?.iter().map(|d| d.number().unwrap()).collect();
    assert_eq!(numbers, vec![1, 2, 3]);

    // A person's own ruling is accepted and decided by them; a relayed one names who ruled.
    let v1: DecisionView = ctx.get(d1).await?;
    assert_eq!(
        (v1.status()?, v1.decided_by()?.map(ankurah::EntityId::from)),
        (DecisionStatus::Accepted, Some(c.daniel))
    );
    assert!(v1.decided()?.is_some());
    let v3: DecisionView = ctx.get(d3).await?;
    assert_eq!(EntityId::from(v3.author()?), c.triage);
    assert_eq!(v3.decided_by()?.map(EntityId::from), Some(c.daniel));
    assert_eq!(v3.area()?.as_deref(), Some("process"), "the area is kept trimmed");
    let v2: DecisionView = ctx.get(d2).await?;
    assert_eq!((v2.status()?, v2.decided_by()?.map(EntityId::from)), (DecisionStatus::Proposed, None));

    // D<number> and a unique prefix name a decision; the wrong kind and a short prefix are refused.
    assert_eq!(ops::resolve_id(&ctx, "D2", &[ops::Kind::Decision]).await?, d2);
    assert_eq!(ops::resolve_id(&ctx, "d3", ops::ANY).await?, d3);
    assert_eq!(ops::resolve_id(&ctx, &d1.to_base64()[..8], &[ops::Kind::Decision]).await?, d1);
    assert!(ops::resolve_id(&ctx, "D2", &[ops::Kind::Task])
        .await
        .unwrap_err()
        .to_string()
        .contains("names a decision"));
    assert!(ops::resolve_id(&ctx, "D9", ops::ANY).await.unwrap_err().to_string().contains("no decision D9"));
    assert!(ops::resolve_id(&ctx, &d1.to_base64()[..5], ops::ANY).await.is_err(), "five characters are too few");
    assert!(ops::resolve_id(&ctx, &d1.to_base64()[..8], &[ops::Kind::Task]).await.is_err(), "not among tasks");

    // Moves: only from the statuses the rules allow, each with a reason kept as a comment.
    let err = ops::move_decision(&ctx, c.daniel, d1, DecisionMove::Accept, "yes").await.unwrap_err();
    assert!(err.to_string().contains("only proposed decisions can be accepted"), "{err}");
    let err = ops::move_decision(&ctx, c.daniel, d2, DecisionMove::Reject, "  ").await.unwrap_err();
    assert!(err.to_string().contains("note saying why"), "{err}");
    assert_eq!(
        ops::move_decision(&ctx, c.daniel, d2, DecisionMove::Reject, "a cap is safer").await?,
        DecisionStatus::Rejected
    );
    let v2: DecisionView = ctx.get(d2).await?;
    assert_eq!(v2.decided_by()?.map(EntityId::from), Some(c.daniel), "whoever rejects a proposal rules on it");
    assert_eq!(v2.closed_reason()?.as_deref(), Some("a cap is safer"));
    let comments = ops::messages_about(&ctx, "decision", d2).await?;
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].content()?, "Rejected: a cap is safer");
    assert_eq!(EntityId::from(comments[0].author()?), c.daniel);

    // Supersession: the successor must be accepted and another decision; the old one is marked,
    // linked and commented in one write.
    let d4 =
        ops::create_decision(&ctx, c.daniel, ops::CreateDecision { propose: true, ..ruling("Full jitter") }).await?;
    let err = ops::supersede_decision(&ctx, c.daniel, d1, d4, "jitter").await.unwrap_err();
    assert!(err.to_string().contains("accept it first"), "{err}");
    assert!(ops::supersede_decision(&ctx, c.daniel, d1, d1, "itself").await.is_err());
    ops::move_decision(&ctx, c.daniel, d4, DecisionMove::Accept, "measured it").await?;
    ops::supersede_decision(&ctx, c.daniel, d1, d4, "jitter avoids stampedes").await?;
    let v1: DecisionView = ctx.get(d1).await?;
    assert_eq!((v1.status()?, v1.closed_by()?.map(EntityId::from)), (DecisionStatus::Superseded, Some(c.daniel)));
    let lineage = ops::relations_to(&ctx, d1, Some(RelationKind::Supersedes)).await?;
    assert_eq!(lineage.iter().map(|r| r.from_id().unwrap()).collect::<Vec<_>>(), vec![d4]);
    assert_eq!(
        ops::messages_about(&ctx, "decision", d1).await?[0].content()?,
        "Superseded by D4: jitter avoids stampedes"
    );
    assert!(
        ops::move_decision(&ctx, c.daniel, d1, DecisionMove::Repeal, "again").await.is_err(),
        "superseded stays so"
    );

    // `link` refuses a supersession of a decision, which only the verb makes whole.
    let err = ops::link(&ctx, c.daniel, RelationKind::Supersedes, d3, d4, None, false, serde_json::json!({}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("fwd decision supersede"), "{err}");
    Ok(())
}

/// A proposal accepted or rejected on a person's word (`fwd decision accept --decided-by`) names
/// that person as its decider, in the record and in the exported log, while whoever recorded the
/// move wrote its note; a handle that names no person is refused, and a repeal names no one else.
#[tokio::test]
async fn a_ruling_recorded_on_a_persons_word_names_them_as_its_decider() -> Result<()> {
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let proposal = |title: &str| ops::CreateDecision { propose: true, ..ruling(title) };
    let name = ops::create_decision(&ctx, c.triage, proposal("Name")).await?;
    let license = ops::create_decision(&ctx, c.triage, proposal("License")).await?;

    // The handle names the person, with or without its at-sign; an agent's names no person.
    assert_eq!(ops::person_by_handle(&ctx, "@dnorman").await?, c.daniel);
    assert_eq!(ops::person_by_handle(&ctx, "dnorman").await?, c.daniel);
    let err = ops::person_by_handle(&ctx, "@triage-bot").await.unwrap_err();
    assert!(err.to_string().contains("`@triage-bot` is an agent, not a person"), "{err}");
    let err = ops::person_by_handle(&ctx, "@nobody").await.unwrap_err();
    assert!(err.to_string().contains("no one in this project has the handle `@nobody`"), "{err}");

    // Accepted by the triage agent on Daniel's word: Daniel ruled, and the triage agent wrote the note.
    ops::move_decision_for(&ctx, c.triage, c.daniel, name, DecisionMove::Accept, "Daniel said yes").await?;
    let v: DecisionView = ctx.get(name).await?;
    assert_eq!((v.status()?, v.decided_by()?.map(EntityId::from)), (DecisionStatus::Accepted, Some(c.daniel)));
    let notes = ops::messages_about(&ctx, "decision", name).await?;
    assert_eq!(notes.len(), 1);
    assert_eq!(
        (EntityId::from(notes[0].author()?), notes[0].content()?),
        (c.triage, "Accepted: Daniel said yes".into())
    );

    // Declined likewise: the decider is the one who closed it, and the note is the triage agent's.
    ops::move_decision_for(&ctx, c.triage, c.daniel, license, DecisionMove::Reject, "not yet").await?;
    let v: DecisionView = ctx.get(license).await?;
    assert_eq!(
        (v.status()?, v.decided_by()?.map(EntityId::from), v.closed_by()?.map(EntityId::from)),
        (DecisionStatus::Rejected, Some(c.daniel), Some(c.daniel))
    );
    assert_eq!(EntityId::from(ops::messages_about(&ctx, "decision", license).await?[0].author()?), c.triage);

    // The exported log names the person as the one who ruled, and the triage agent as the filer.
    let log = fwd_core::decision_log::render(&fwd_core::decision_log::entries(&ctx).await?);
    assert!(log.contains("decided by dnorman, recorded by triage-bot"), "{log}");
    assert!(log.contains("rejected by dnorman"), "{log}");

    // A repeal names no decider but whoever repeals.
    let err = ops::move_decision_for(&ctx, c.triage, c.daniel, name, DecisionMove::Repeal, "moot").await.unwrap_err();
    assert!(err.to_string().contains("only accepting or rejecting a proposal records someone else's ruling"), "{err}");
    assert_eq!(ctx.get::<DecisionView>(name).await?.status()?, DecisionStatus::Accepted, "and is not made");
    Ok(())
}

/// Makes the move `mv` on the ruling `d` with `note`, by `by`; a supersession names `successor` as
/// the ruling that replaces it.
async fn make_move(
    ctx: &Context,
    by: EntityId,
    mv: DecisionMove,
    d: EntityId,
    successor: EntityId,
    note: &str,
) -> Result<()> {
    match mv {
        DecisionMove::Supersede => ops::supersede_decision(ctx, by, d, successor, note).await,
        _ => ops::move_decision(ctx, by, d, mv, note).await.map(|_| ()),
    }
}

/// A move's note is message text, as a comment's is (Daniel, 2026-09-28, of a supersession's note
/// that named his message with an at-sign: "your `@` didn't work"): the comment the move writes
/// stores each reference as `[[<full id>]]`, shadowed by a `mention` relation in the order the
/// note names them, and a person it names hears of it; a reference that names nothing refuses the
/// move before any of it is made. The decision's `closed_reason`, where the move sets one, keeps
/// the note as typed.
async fn a_moves_note_names_what_a_comment_names(mv: DecisionMove) -> Result<()> {
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    // The triage agent files the ruling, a proposal for Accept and Reject; Daniel moves it. D2 is in
    // force, to replace D1 in a supersession.
    let proposal = matches!(mv, DecisionMove::Accept | DecisionMove::Reject);
    let d = ops::create_decision(
        &ctx,
        c.triage,
        ops::CreateDecision { propose: proposal, ..ruling("Retries back off exponentially") },
    )
    .await?;
    let successor = ops::create_decision(&ctx, c.triage, ruling("Retries back off with full jitter")).await?;
    let asked =
        ops::post_message(&ctx, c.daniel, "Should retries back off?", Priority::Next, MessageContext::default())
            .await?;
    let waiting = ops::create_task(&ctx, c.daniel, task("Retry on disconnect")).await?;

    // A name that fits nothing refuses the move, and nothing of it is made.
    let messages = ctx.fetch::<MessageView>("created > 0").await?.len();
    let err = make_move(&ctx, c.daniel, mv, d, successor, "as @impl-alicee said").await.unwrap_err().to_string();
    assert!(
        err.contains("`@impl-alicee` (no one has the handle `impl-alicee`, and no id starts with it) names nothing"),
        "{mv:?}: {err}"
    );
    let v: DecisionView = ctx.get(d).await?;
    assert_eq!((v.status()?, v.closed_reason()?), (mv.moves_from(), None), "{mv:?}: the decision is as it was");
    assert_eq!(ctx.fetch::<MessageView>("created > 0").await?.len(), messages, "{mv:?}: no comment is written");
    assert!(ops::relations_to(&ctx, d, Some(RelationKind::Supersedes)).await?.is_empty(), "{mv:?}");

    // The note names a message and a task by the start of their ids, and a person by handle.
    let note = format!(
        " as @{} asked, [[{}]] waits on this; @impl-alice, have a look ",
        &asked.to_base64()[..8],
        &waiting.to_base64()[..8]
    );
    make_move(&ctx, c.daniel, mv, d, successor, &note).await?;
    let v: DecisionView = ctx.get(d).await?;
    assert_eq!(v.status()?, mv.moves_to(), "{mv:?}");
    let comments = ops::messages_about(&ctx, "decision", d).await?;
    assert_eq!(comments.len(), 1, "{mv:?}");
    let move_named = match mv {
        DecisionMove::Supersede => "Superseded by D2".to_string(),
        _ => mv.participle()[..1].to_uppercase() + &mv.participle()[1..],
    };
    assert_eq!(
        comments[0].content()?,
        format!(
            "{move_named}: as [[{}]] asked, [[{}]] waits on this; [[{}]], have a look",
            asked.to_base64(),
            waiting.to_base64(),
            c.implementer.to_base64()
        ),
        "{mv:?}"
    );
    let mut edges = ops::relations_from(&ctx, comments[0].id(), Some(RelationKind::Mention)).await?;
    edges.sort_by_key(|r| serde_json::Value::from(r.via().unwrap())["ordinal"].as_i64());
    let shadowed: Vec<_> = edges
        .iter()
        .map(|r| (r.to_id().unwrap(), r.status().unwrap(), serde_json::Value::from(r.via().unwrap())))
        .collect();
    assert_eq!(
        shadowed,
        vec![
            (asked, RelationStatus::Accepted, serde_json::json!({"inline": true, "ordinal": 1})),
            (waiting, RelationStatus::Accepted, serde_json::json!({"inline": true, "ordinal": 2})),
            (c.implementer, RelationStatus::Accepted, serde_json::json!({"inline": true, "ordinal": 3})),
        ],
        "{mv:?}"
    );
    let heard = recipients(&ctx, comments[0].id()).await?;
    assert!(heard.contains(&(c.implementer, NotificationType::Mentioned)), "{mv:?}: {heard:?}");
    let typed = (!matches!(mv, DecisionMove::Accept)).then(|| note.trim().to_string());
    assert_eq!(v.closed_reason()?, typed, "{mv:?}");
    Ok(())
}

#[tokio::test]
async fn accepting_reads_its_note_as_a_comments_text() -> Result<()> {
    a_moves_note_names_what_a_comment_names(DecisionMove::Accept).await
}

#[tokio::test]
async fn rejecting_reads_its_note_as_a_comments_text() -> Result<()> {
    a_moves_note_names_what_a_comment_names(DecisionMove::Reject).await
}

#[tokio::test]
async fn superseding_reads_its_note_as_a_comments_text() -> Result<()> {
    a_moves_note_names_what_a_comment_names(DecisionMove::Supersede).await
}

#[tokio::test]
async fn repealing_reads_its_note_as_a_comments_text() -> Result<()> {
    a_moves_note_names_what_a_comment_names(DecisionMove::Repeal).await
}

/// `cites` joins a task, a message or a decision to a decision, or a decision to the task or
/// message it was decided in; citing twice is one relation.
#[tokio::test]
async fn cites_needs_a_decision_at_one_end_and_is_made_once() -> Result<()> {
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let t = ops::create_task(&ctx, c.implementer, task("Cap the retry delay")).await?;
    let q = ops::post_message(
        &ctx,
        c.implementer,
        "how long may a retry wait?",
        Priority::Now,
        MessageContext { to: Some(c.daniel.to_base64()), ..Default::default() },
    )
    .await?;
    let d = ops::create_decision(&ctx, c.daniel, ops::CreateDecision { from: vec![q], ..ruling("Cap at 30s") }).await?;
    let decided_in = ops::relations_from(&ctx, d, Some(RelationKind::Cites)).await?;
    assert_eq!(decided_in.iter().map(|r| r.to_id().unwrap()).collect::<Vec<_>>(), vec![q], "decided in the question");

    let first = ops::cite(&ctx, c.implementer, t, d, None).await?;
    assert_eq!(ops::cite(&ctx, c.implementer, t, d, Some("again")).await?, first, "citing again is the same relation");
    assert!(ops::cite(&ctx, c.implementer, d, t, None).await.is_err(), "`fwd cite` names the decision second");
    let err =
        ops::link(&ctx, c.daniel, RelationKind::Cites, t, q, None, false, serde_json::json!({})).await.unwrap_err();
    assert!(err.to_string().ends_with(", not a task to a message"), "{err}");
    assert!(ops::link(&ctx, c.daniel, RelationKind::Cites, d, d, None, false, serde_json::json!({})).await.is_err());
    let scope_cites =
        ops::link(&ctx, c.daniel, RelationKind::Cites, c.scope, d, None, false, serde_json::json!({})).await;
    assert!(scope_cites.is_err(), "a scope cites nothing");
    Ok(())
}

/// `fwd cite` after `fwd link T cites D --propose` accepts the proposed citation, with the note
/// it gives (without one, the proposal's note stays); proposing again answers the citation as it
/// stands.
#[tokio::test]
async fn citing_accepts_a_proposed_citation_and_keeps_its_note() -> Result<()> {
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let d = ops::create_decision(&ctx, c.daniel, ruling("Cap at 30s")).await?;
    let status_and_note = |r: RelationView| (r.status().unwrap(), r.note().unwrap());

    let t = ops::create_task(&ctx, c.implementer, task("Cap the retry delay")).await?;
    let proposed =
        ops::link(&ctx, c.implementer, RelationKind::Cites, t, d, Some("maybe"), true, serde_json::json!({})).await?;
    assert_eq!(ops::cite(&ctx, c.daniel, t, d, Some("rests on the cap")).await?, proposed, "the same relation");
    assert_eq!(
        status_and_note(ctx.get(proposed).await?),
        (RelationStatus::Accepted, Some("rests on the cap".to_string()))
    );
    let again =
        ops::link(&ctx, c.implementer, RelationKind::Cites, t, d, Some("again"), true, serde_json::json!({})).await?;
    assert_eq!(again, proposed, "proposing again answers the accepted citation");
    assert_eq!(status_and_note(ctx.get(proposed).await?).0, RelationStatus::Accepted);

    let u = ops::create_task(&ctx, c.implementer, task("Cap the reconnect delay")).await?;
    let proposed =
        ops::link(&ctx, c.implementer, RelationKind::Cites, u, d, Some("maybe"), true, serde_json::json!({})).await?;
    ops::cite(&ctx, c.daniel, u, d, None).await?;
    assert_eq!(status_and_note(ctx.get(proposed).await?), (RelationStatus::Accepted, Some("maybe".to_string())));
    Ok(())
}

/// A decision named twice as where it was decided cites it once.
#[tokio::test]
async fn a_decision_cites_where_it_was_decided_once_however_often_named() -> Result<()> {
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let q = ops::post_message(
        &ctx,
        c.implementer,
        "how long may a retry wait?",
        Priority::Now,
        MessageContext { to: Some(c.daniel.to_base64()), ..Default::default() },
    )
    .await?;
    let d =
        ops::create_decision(&ctx, c.daniel, ops::CreateDecision { from: vec![q, q], ..ruling("Cap at 30s") }).await?;
    let decided_in = ops::relations_from(&ctx, d, Some(RelationKind::Cites)).await?;
    assert_eq!(decided_in.iter().map(|r| r.to_id().unwrap()).collect::<Vec<_>>(), vec![q]);
    Ok(())
}

/// Retracting a supersession would leave the old decision Superseded with no successor, and
/// nothing moves it back, so `unlink` refuses it and says what to do instead.
#[tokio::test]
async fn a_supersession_of_a_decision_is_not_undone() -> Result<()> {
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let old = ops::create_decision(&ctx, c.daniel, ruling("Retries back off exponentially")).await?;
    let new = ops::create_decision(&ctx, c.daniel, ruling("Full jitter")).await?;
    ops::supersede_decision(&ctx, c.daniel, old, new, "jitter avoids stampedes").await?;
    let err = ops::unlink(&ctx, c.daniel, RelationKind::Supersedes, new, old, "oops").await.unwrap_err();
    assert!(err.to_string().contains("a supersession is not undone: D1 stays superseded by D2"), "{err}");
    assert!(err.to_string().contains("fwd decision supersede D2"), "{err}");
    let lineage = ops::relations_to(&ctx, old, Some(RelationKind::Supersedes)).await?;
    assert_eq!(lineage.iter().map(|r| r.status().unwrap()).collect::<Vec<_>>(), vec![RelationStatus::Accepted]);
    let v: DecisionView = ctx.get(old).await?;
    assert_eq!(v.status()?, DecisionStatus::Superseded);
    Ok(())
}

/// `D` and five digits or more names the decision of that number while one has it, and is the
/// start of an id otherwise; shorter, it is only ever a decision's number.
#[tokio::test]
async fn d_and_five_digits_is_a_decision_number_first_and_else_the_start_of_an_id() -> Result<()> {
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    // Decision 12345, written directly: `create_decision` would number it 1.
    let trx = ctx.begin();
    let numbered = trx
        .create(&Decision {
            number: 12345,
            title: "The twelve thousand three hundred and forty-fifth ruling".into(),
            status: DecisionStatus::Accepted,
            area: None,
            scope: None,
            context: String::new(),
            decision: String::new(),
            consequences: String::new(),
            author: ankurah::property::Ref::new(c.daniel),
            decided_by: Some(ankurah::property::Ref::new(c.daniel)),
            created: fwd_core::now_ms(),
            decided: Some(fwd_core::now_ms()),
            closed_by: None,
            closed: None,
            closed_reason: None,
            closed_reason_message: None,
        })
        .await?
        .id();
    trx.commit().await?;

    let refusal = |r: Result<EntityId>| r.unwrap_err().to_string();
    assert_eq!(ops::resolve_id(&ctx, "D12345", &[ops::Kind::Decision]).await?, numbered);
    assert_eq!(ops::resolve_id(&ctx, "d12345", ops::ANY).await?, numbered);
    // While a decision has the number, a command that takes no decision refuses it.
    assert!(refusal(ops::resolve_id(&ctx, "D12345", &[ops::Kind::Task]).await).contains("names a decision"));
    // No decision has 54321: tried as the start of a task's id, which none has here.
    assert_eq!(
        refusal(ops::resolve_id(&ctx, "D54321", &[ops::Kind::Task]).await),
        "no task has an id starting with `D54321`"
    );
    assert!(refusal(ops::resolve_id(&ctx, "D54321", ops::ANY).await).starts_with("no task, message, decision"));
    // Four digits or fewer is too short to start an id: only ever a decision's number.
    assert!(refusal(ops::resolve_id(&ctx, "D5432", &[ops::Kind::Task]).await).contains("names a decision"));
    assert_eq!(refusal(ops::resolve_id(&ctx, "D5432", ops::ANY).await), "there is no decision D5432");
    Ok(())
}

/// A comment on a decision reaches whoever filed it and whoever ruled, never its writer; a
/// proposal reaches the project's triage agent and its people, never its author; a repeal or a supersession
/// reaches the authors of everything citing the decision, never whoever moved it.
#[tokio::test]
async fn decisions_route_to_their_filers_their_deciders_and_what_rests_on_them() -> Result<()> {
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let who = |planned: &[routing::Planned]| {
        let mut out: Vec<_> = planned.iter().map(|p| (p.recipient, p.notification_type)).collect();
        out.sort();
        out
    };
    let sorted = |mut v: Vec<(EntityId, NotificationType)>| {
        v.sort();
        v
    };

    // An agent proposes: the triage agent and Daniel hear; the proposer does not.
    let p =
        ops::create_decision(&ctx, c.implementer, ops::CreateDecision { propose: true, ..ruling("Use tokio") }).await?;
    let pv: DecisionView = ctx.get(p).await?;
    assert_eq!(
        who(&routing::plan_for_decision_created(&ctx, &pv).await?),
        sorted(vec![(c.daniel, NotificationType::DecisionProposed), (c.triage, NotificationType::DecisionProposed)])
    );
    // A ruling recorded as given notifies no one.
    let d = ops::create_decision(
        &ctx,
        c.triage,
        ops::CreateDecision { decided_by: Some(c.daniel), ..ruling("Cap at 30s") },
    )
    .await?;
    let dv: DecisionView = ctx.get(d).await?;
    assert!(routing::plan_for_decision_created(&ctx, &dv).await?.is_empty());

    // Daniel accepts the proposal: its author hears the reason; Daniel, who wrote it, does not.
    ops::move_decision(&ctx, c.daniel, p, DecisionMove::Accept, "fine for the client").await?;
    let reason = ops::messages_about(&ctx, "decision", p).await?.remove(0);
    assert_eq!(recipients(&ctx, reason.id()).await?, vec![(c.implementer, NotificationType::DecisionCommented)]);
    // Alice comments on the triage agent's record of Daniel's ruling: both of them hear.
    let remark = ops::post_message(
        &ctx,
        c.implementer,
        "does the cap include the first try?",
        Priority::Next,
        MessageContext { decision_id: Some(d), ..Default::default() },
    )
    .await?;
    assert_eq!(
        recipients(&ctx, remark).await?,
        sorted(vec![(c.daniel, NotificationType::DecisionCommented), (c.triage, NotificationType::DecisionCommented)])
    );
    // A reply to that comment stays on the decision.
    let answer = ops::post_message(
        &ctx,
        c.daniel,
        "it does",
        Priority::Next,
        MessageContext { reply_to: Some(remark), ..Default::default() },
    )
    .await?;
    let av: MessageView = ctx.get(answer).await?;
    assert_eq!(av.decision()?.map(EntityId::from), Some(d), "a reply inherits the decision it is about");

    // What rests on the ruling: Alice's task, the triage agent's question and Daniel's own decision.
    let t = ops::create_task(&ctx, c.implementer, task("Cap the retry delay")).await?;
    ops::cite(&ctx, c.implementer, t, d, None).await?;
    let q = ops::post_message(
        &ctx,
        c.triage,
        "is 30s long enough on mobile?",
        Priority::Next,
        MessageContext { to: Some(c.triage.to_base64()), ..Default::default() },
    )
    .await?;
    ops::cite(&ctx, c.triage, q, d, None).await?;
    let own = ops::create_decision(&ctx, c.daniel, ruling("Mobile retries follow the desktop cap")).await?;
    ops::cite(&ctx, c.daniel, own, d, None).await?;
    // Daniel repeals it: Alice and the triage agent hear, with the reason; Daniel does not.
    ops::move_decision(&ctx, c.daniel, d, DecisionMove::Repeal, "the transport retries now").await?;
    let dv: DecisionView = ctx.get(d).await?;
    let planned = routing::plan_for_decision_status(&ctx, &dv, DecisionStatus::Accepted, Some(c.daniel)).await?;
    assert_eq!(
        who(&planned),
        sorted(vec![
            (c.implementer, NotificationType::DecisionRepealed),
            (c.triage, NotificationType::DecisionRepealed)
        ])
    );
    let reason = planned[0].originator["message_id"].as_str().unwrap().parse::<EntityId>()?;
    let rv: MessageView = ctx.get(reason).await?;
    assert_eq!(rv.content()?, "Repealed: the transport retries now");
    assert_eq!(dv.closed_reason_message()?.map(EntityId::from), Some(reason), "the decision names its reason");
    assert_eq!(serde_json::Value::from(rv.context()?)["move"], "repealed");
    // The reason is a comment on the decision too, whose filer (the triage agent) would hear it as
    // one; the triage agent rests something on the ruling, so the repeal already told it, with this
    // very comment, and the comment does not tell it again.
    assert!(recipients(&ctx, reason).await?.is_empty(), "one repeal, one notification each");
    // A later comment from Daniel is not the repeal's reason.
    ops::post_message(
        &ctx,
        c.daniel,
        "see the transport's own backoff",
        Priority::Next,
        MessageContext { decision_id: Some(d), ..Default::default() },
    )
    .await?;
    let again = routing::plan_for_decision_status(&ctx, &dv, DecisionStatus::Accepted, Some(c.daniel)).await?;
    assert_eq!(again[0].originator["message_id"], reason.to_base64(), "the reason is the move's own comment");
    assert!(
        routing::plan_for_decision_status(&ctx, &dv, DecisionStatus::Repealed, Some(c.daniel)).await?.is_empty(),
        "no change, no notification"
    );
    Ok(())
}

/// The scope a decision governs, none for the whole project.
async fn governs(ctx: &Context, decision: EntityId) -> Result<Option<EntityId>> {
    ops::governs(&ctx.get::<DecisionView>(decision).await?)
}

/// A decision is attached to the scope whose work it governs as it is made or later, one scope at
/// a time, and detached so that it governs the whole project again (D93); a scope that does not
/// exist is refused and nothing is written, and attaching leaves no comment for anyone to hear.
#[tokio::test]
async fn a_decision_is_attached_to_the_scope_it_governs_and_detached_to_govern_the_whole_project() -> Result<()> {
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let hosting = ops::create_scope(&ctx, c.daniel, named_scope("Cloud Hosting")).await?;

    // Made project-wide unless a scope is given.
    let wide = ops::create_decision(&ctx, c.daniel, ruling("Retries back off exponentially")).await?;
    let billing = ops::CreateDecision { scope: Some(hosting), decided_by: Some(c.daniel), ..ruling("Bill per seat") };
    let billed = ops::create_decision(&ctx, c.triage, billing).await?;
    assert_eq!(governs(&ctx, wide).await?, None);
    assert_eq!(governs(&ctx, billed).await?, Some(hosting));
    let json = ops::json_of_decision(&ctx.get::<DecisionView>(billed).await?)?;
    assert_eq!(
        (json["scope"].as_str(), json["status"].as_str()),
        (Some(hosting.to_base64().as_str()), Some("accepted"))
    );

    // Attached later by anyone, and moved from one scope to another: one scope at a time.
    ops::attach_decision(&ctx, c.implementer, wide, c.scope).await?;
    assert_eq!(governs(&ctx, wide).await?, Some(c.scope));
    ops::attach_decision(&ctx, c.implementer, wide, hosting).await?;
    assert_eq!(governs(&ctx, wide).await?, Some(hosting));
    ops::attach_decision(&ctx, c.implementer, wide, hosting).await?;
    assert_eq!(governs(&ctx, wide).await?, Some(hosting), "attaching it where it is changes nothing");

    // Detached, it governs the whole project again; detaching a project-wide one changes nothing.
    ops::detach_decision(&ctx, c.daniel, wide).await?;
    assert_eq!(governs(&ctx, wide).await?, None);
    ops::detach_decision(&ctx, c.daniel, wide).await?;
    assert_eq!(governs(&ctx, wide).await?, None);
    // The ruling itself is unchanged, and nothing was written on it for anyone to hear.
    let v: DecisionView = ctx.get(wide).await?;
    assert_eq!((v.status()?, v.title()?), (DecisionStatus::Accepted, "Retries back off exponentially".to_string()));
    assert!(ops::messages_about(&ctx, "decision", wide).await?.is_empty(), "attaching leaves no comment");

    // A scope that does not exist, or that is something else, is refused, and nothing is written.
    let nowhere = EntityId::random();
    let err = ops::attach_decision(&ctx, c.daniel, billed, nowhere).await.unwrap_err().to_string();
    assert_eq!(err, format!("no scope of this project has the id {nowhere}, so no ruling can be attached to it"));
    let t = ops::create_task(&ctx, c.daniel, task("Cap the retry delay")).await?;
    assert!(ops::attach_decision(&ctx, c.daniel, billed, t).await.is_err(), "a task is not a scope");
    assert_eq!(governs(&ctx, billed).await?, Some(hosting), "the refusals change nothing");
    let filed = ops::decisions(&ctx).await?.len();
    let astray = ops::CreateDecision { scope: Some(nowhere), ..ruling("Nowhere to govern") };
    let err = ops::create_decision(&ctx, c.daniel, astray).await.unwrap_err().to_string();
    assert!(err.contains("no scope of this project has the id"), "{err}");
    assert_eq!(ops::decisions(&ctx).await?.len(), filed, "no decision is filed");
    Ok(())
}

/// A command line names a scope by its title, whatever its case, as well as by its id or the
/// first characters of it; a title two scopes share is refused with both listed, and a name that
/// fits no scope, a whole id among them, is refused saying so.
#[tokio::test]
async fn a_scope_is_named_by_its_title_or_its_id() -> Result<()> {
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let hosting = ops::create_scope(&ctx, c.daniel, named_scope("Cloud Hosting")).await?;
    assert_eq!(ops::resolve_scope(&ctx, "Cloud Hosting").await?, hosting);
    assert_eq!(ops::resolve_scope(&ctx, " cloud hosting ").await?, hosting);
    assert_eq!(ops::resolve_scope(&ctx, &hosting.to_base64()).await?, hosting);
    assert_eq!(ops::resolve_scope(&ctx, &hosting.to_base64()[..8]).await?, hosting);
    let err = ops::resolve_scope(&ctx, "Cloud Hostng").await.unwrap_err().to_string();
    assert_eq!(err, "no scope has the title `Cloud Hostng` or an id starting with it");
    // A whole id is refused too when it names no scope, or names something else.
    let nowhere = EntityId::random();
    let err = ops::resolve_scope(&ctx, &nowhere.to_base64()).await.unwrap_err().to_string();
    assert_eq!(err, format!("no scope of this project has the id {nowhere}"));
    let t = ops::create_task(&ctx, c.daniel, task("Cap the retry delay")).await?;
    assert!(ops::resolve_scope(&ctx, &t.to_base64()).await.is_err(), "a task is not a scope");
    let twin = ops::create_scope(&ctx, c.implementer, named_scope("cloud hosting")).await?;
    let err = ops::resolve_scope(&ctx, "Cloud Hosting").await.unwrap_err().to_string();
    assert!(err.starts_with("2 scopes have the title `Cloud Hosting` ("), "{err}");
    assert!(err.contains(&hosting.to_base64()[..8]) && err.contains(&twin.to_base64()[..8]), "{err}");
    assert_eq!(ops::resolve_scope(&ctx, &twin.to_base64()[..8]).await?, twin, "its id still names it");
    Ok(())
}

/// The exported log lists the rulings that govern the whole project first, then each scope's under
/// the scope's name, the scopes by name; each keeps its number, and each entry names the scope it
/// governs.
#[tokio::test]
async fn the_exported_log_groups_the_rulings_by_the_scope_they_govern() -> Result<()> {
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let hosting = ops::create_scope(&ctx, c.daniel, named_scope("Cloud Hosting")).await?;
    let billing = ops::CreateDecision { scope: Some(hosting), area: Some("product".into()), ..ruling("Bill per seat") };
    ops::create_decision(&ctx, c.daniel, billing).await?;
    ops::create_decision(&ctx, c.daniel, ruling("Retries back off exponentially")).await?;
    let popped = ops::create_decision(&ctx, c.daniel, ruling("The inbox pops on every tool batch")).await?;
    ops::attach_decision(&ctx, c.implementer, popped, c.scope).await?;

    let log = fwd_core::decision_log::render(&fwd_core::decision_log::entries(&ctx).await?);
    let at = |needle: &str| log.find(needle).unwrap_or_else(|| panic!("{needle:?} is not in {log}"));
    let order = [
        at("\n## Project-wide\n"),
        at("- **D2: Retries back off exponentially** ("),
        at("\n## Cloud Hosting\n"),
        at("- **D1: Bill per seat** (product, governs Cloud Hosting, "),
        at("\n## Inbox delivery\n"),
        at("- **D3: The inbox pops on every tool batch** (governs Inbox delivery, "),
    ];
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{log}");
    assert!(!log[order[1]..order[2]].contains("governs"), "a project-wide ruling names no scope: {log}");
    Ok(())
}

/// The exported log gives why a ruling closed in the note of the comment its closing move wrote,
/// each reference by name as `--format text` prints a message, while `closed_reason` keeps the
/// note as typed for the readers of a decision's JSON (task b6bAAF57, "Show a decision's closing
/// note as its comment, with names drawn, not the raw text"). The comment counts as it stands, an
/// edit by its author included; a ruling closed before moves wrote comments gives its
/// `closed_reason`, a reference in it named where it is a whole id.
#[tokio::test]
async fn a_closing_note_exports_with_the_names_of_what_it_names() -> Result<()> {
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let waiting = ops::create_task(&ctx, c.daniel, task("Retry on disconnect")).await?;
    let closed_on = |v: &DecisionView| -> Result<String> {
        Ok(fwd_core::decision_log::date(v.closed()?.expect("a closed ruling says when")))
    };

    // D1, a proposal accepted with a note, is superseded by D2 with a note naming the task by the
    // start of its id; the acceptance's comment is not why it closed.
    let proposal = ops::CreateDecision { propose: true, ..ruling("Retries back off exponentially") };
    let d1 = ops::create_decision(&ctx, c.triage, proposal).await?;
    ops::move_decision(&ctx, c.daniel, d1, DecisionMove::Accept, "measured it").await?;
    let d2 = ops::create_decision(&ctx, c.daniel, ruling("Retries back off with full jitter")).await?;
    let typed = format!("jitter avoids stampedes, as [[{}]] found", &waiting.to_base64()[..8]);
    ops::supersede_decision(&ctx, c.daniel, d1, d2, &typed).await?;
    let v1: DecisionView = ctx.get(d1).await?;
    assert_eq!(v1.closed_reason()?, Some(typed), "closed_reason keeps the note as typed");

    // D3 is repealed, and the comment the repeal wrote is then edited by its author, who drops the
    // words the move opened it with and names Alice.
    let d3 = ops::create_decision(&ctx, c.daniel, ruling("Retries give up after five tries")).await?;
    ops::move_decision(&ctx, c.daniel, d3, DecisionMove::Repeal, "the transport retries by itself").await?;
    let repeal = ops::messages_about(&ctx, "decision", d3).await?.remove(0);
    ops::edit_message(&ctx, c.daniel, repeal.id(), "The transport retries by itself; @impl-alice measured it").await?;
    let v3: DecisionView = ctx.get(d3).await?;

    // D4 was repealed before moves wrote comments, its `closed_reason` naming the task by its whole
    // id: written directly, since every move writes its comment now.
    let at = fwd_core::now_ms();
    let trx = ctx.begin();
    trx.create(&Decision {
        number: 4,
        title: "Retries wait a second".into(),
        status: DecisionStatus::Repealed,
        area: None,
        scope: None,
        context: String::new(),
        decision: "wait a second between tries".into(),
        consequences: String::new(),
        author: ankurah::property::Ref::new(c.daniel),
        decided_by: Some(ankurah::property::Ref::new(c.daniel)),
        created: at,
        decided: Some(at),
        closed_by: Some(ankurah::property::Ref::new(c.daniel)),
        closed: Some(at),
        closed_reason: Some(format!("[[{}]] replaced the wait", waiting.to_base64())),
        closed_reason_message: None,
    })
    .await?;
    trx.commit().await?;

    let log = fwd_core::decision_log::render(&fwd_core::decision_log::entries(&ctx).await?);
    for line in [
        format!(
            "  - Superseded by D2 on {} (dnorman): jitter avoids stampedes, as [Retry on disconnect] found\n",
            closed_on(&v1)?
        ),
        format!(
            "  - Repealed on {} by dnorman: The transport retries by itself; @impl-alice measured it\n",
            closed_on(&v3)?
        ),
        format!(
            "  - Repealed on {} by dnorman: [Retry on disconnect] replaced the wait\n",
            fwd_core::decision_log::date(at)
        ),
    ] {
        assert!(log.contains(&line), "{line:?} is not in {log}");
    }
    assert!(!log.contains("Accepted: measured it") && !log.contains("[["), "{log}");
    Ok(())
}

/// `fwd decision show --format text` says why a ruling closed as the exported log does
/// (`decision_log::why_closed`), where it printed `closed_reason`, the note as typed (task aHAMvPU1,
/// "Draw names wherever a closing note or a closed reason still prints raw"): a rejection whose
/// note named a task by the start of its id and a person by handle names both, and a ruling in
/// force has nothing to say.
#[tokio::test]
async fn why_a_ruling_closed_names_what_its_note_named() -> Result<()> {
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let waiting = ops::create_task(&ctx, c.daniel, task("Retry on disconnect")).await?;
    let proposal = ops::CreateDecision { propose: true, ..ruling("Retries give up after five tries") };
    let rejected = ops::create_decision(&ctx, c.triage, proposal).await?;
    let typed = format!("[[{}]] retries for ever, as @impl-alice found", &waiting.to_base64()[..8]);
    ops::move_decision(&ctx, c.daniel, rejected, DecisionMove::Reject, &typed).await?;
    let in_force = ops::create_decision(&ctx, c.daniel, ruling("Retries back off exponentially")).await?;

    let v: DecisionView = ctx.get(rejected).await?;
    assert_eq!(v.closed_reason()?, Some(typed), "closed_reason keeps the note as typed");
    assert_eq!(
        fwd_core::decision_log::why_closed(&ctx, &v).await?.as_deref(),
        Some("[Retry on disconnect] retries for ever, as @impl-alice found")
    );
    let v: DecisionView = ctx.get(in_force).await?;
    assert_eq!(fwd_core::decision_log::why_closed(&ctx, &v).await?, None, "a ruling in force has not closed");
    Ok(())
}

/// When two moves of a decision raced, both commit and both comments stand
/// (docs/design/data-model.md, Decision); the later comment's note is why it closed.
#[test]
fn of_two_raced_moves_the_later_comment_says_why_the_decision_closed() {
    let rejected = |created, note: &str| ops::DecisionComment {
        created,
        context: serde_json::json!({ ops::MOVE_KEY: "rejected" }),
        text: format!("Rejected: {note}"),
    };
    let raced = [rejected(2, "a cap is safer"), rejected(1, "retry forever")];
    assert_eq!(ops::closing_note(DecisionStatus::Rejected, &raced), Some("a cap is safer"));
}

/// An at-sign before a thing's id, or the start of one, names that thing as the brackets would;
/// a handle nobody has, that starts no id, is still refused (Daniel, 2026-09-28, typing
/// `@<task id>` on the page and being refused).
#[tokio::test]
async fn an_at_sign_before_an_id_names_that_thing_too() -> Result<()> {
    let ctx = context().await?;
    let c = cast(&ctx).await?;
    let task = ops::create_task(&ctx, c.daniel, task("Create the scopes")).await?;
    let text = format!("if @{} isn't done yet, ask @{}.", task.to_base64(), &c.implementer.to_base64()[..8]);
    let m = ops::post_message(&ctx, c.daniel, &text, Priority::Next, MessageContext::default()).await?;
    let stored = ctx.get::<MessageView>(m).await?.content()?;
    assert_eq!(stored, format!("if [[{}]] isn't done yet, ask [[{}]].", task.to_base64(), c.implementer.to_base64()));
    let named: Vec<ankurah::EntityId> =
        ops::relations_from(&ctx, m, Some(RelationKind::Mention)).await?.iter().map(|r| r.to_id().unwrap()).collect();
    assert!(named.contains(&task) && named.contains(&c.implementer), "both are mentioned: {named:?}");
    let refused =
        ops::post_message(&ctx, c.daniel, "ask @nobody-here", Priority::Next, MessageContext::default()).await;
    assert!(refused.is_err(), "a handle nobody has, that starts no id, is refused");
    Ok(())
}

/// One entity of each kind a relation's end may be, for the tests of what each relation kind
/// joins (`ops::joins`): two tasks, a message, two decisions, a scope, an anchor, a relation, a
/// principal and a notification, and the person who links them.
struct Ends {
    by: EntityId,
    task: EntityId,
    other_task: EntityId,
    message: EntityId,
    decision: EntityId,
    other_decision: EntityId,
    scope: EntityId,
    anchor: EntityId,
    relation: EntityId,
    principal: EntityId,
    notification: EntityId,
}

async fn ends(ctx: &Context) -> Result<Ends> {
    let c = cast(ctx).await?;
    let (by, json) = (c.daniel, serde_json::json!({}));
    let first = ops::create_task(ctx, by, task("Cap the retry delay")).await?;
    let other_task = ops::create_task(ctx, by, task("Retry on disconnect")).await?;
    let relation = ops::link(ctx, by, RelationKind::RelatesTo, first, other_task, None, false, json.clone()).await?;
    Ok(Ends {
        by,
        task: first,
        other_task,
        message: ops::post_message(ctx, by, "why a cap?", Priority::Later, MessageContext::default()).await?,
        decision: ops::create_decision(ctx, by, ruling("Cap at 30s")).await?,
        other_decision: ops::create_decision(ctx, by, ruling("Cap at 60s")).await?,
        scope: c.scope,
        anchor: anchor(ctx, by, EntityId::random(), "src/retry.rs", Some((10, 20))).await?,
        relation,
        principal: c.implementer,
        notification: ops::create_notification(
            ctx,
            by,
            NotificationType::QuestionAsked,
            Priority::Now,
            json.clone(),
            json,
        )
        .await?,
    })
}

/// `fwd link` makes `kind` from and to each pair of `right`, and refuses it between each pair of
/// `wrong`, saying that it joins `joins` and not what the pair's third item says the two are (task
/// V-fkVMZp: a relation whose ends are of other kinds corrupts what every page reads from it, as a
/// decision taken for a task's scope did).
async fn joins_only(
    ctx: &Context,
    e: &Ends,
    kind: RelationKind,
    joins: &str,
    wrong: &[(EntityId, EntityId, &str)],
    right: &[(EntityId, EntityId)],
) -> Result<()> {
    assert_eq!(ops::joins_words(kind), joins);
    let link = |from, to| ops::link(ctx, e.by, kind, from, to, None, false, serde_json::json!({}));
    for (from, to, not) in wrong {
        let refused = link(*from, *to).await.expect_err("a relation whose ends are of other kinds").to_string();
        assert_eq!(refused, format!("refused: `{kind}` joins {joins}, not {not}"));
    }
    for (from, to) in right {
        let made: RelationView = ctx.get(link(*from, *to).await?).await?;
        assert_eq!((made.kind()?, made.from_id()?, made.to_id()?), (kind, *from, *to));
    }
    Ok(())
}

#[tokio::test]
async fn depends_on_joins_a_task_to_a_task() -> Result<()> {
    let ctx = context().await?;
    let e = ends(&ctx).await?;
    let wrong = [(e.task, e.decision, "a task to a decision"), (e.message, e.task, "a message to a task")];
    joins_only(&ctx, &e, RelationKind::DependsOn, "a task to a task", &wrong, &[(e.task, e.other_task)]).await
}

#[tokio::test]
async fn subtask_of_joins_a_task_to_a_task() -> Result<()> {
    let ctx = context().await?;
    let e = ends(&ctx).await?;
    let wrong = [(e.task, e.scope, "a task to a scope"), (e.decision, e.task, "a decision to a task")];
    joins_only(&ctx, &e, RelationKind::SubtaskOf, "a task to a task", &wrong, &[(e.task, e.other_task)]).await
}

#[tokio::test]
async fn motivated_by_joins_a_task_to_a_task_or_a_message() -> Result<()> {
    let ctx = context().await?;
    let e = ends(&ctx).await?;
    let wrong = [(e.task, e.scope, "a task to a scope"), (e.message, e.task, "a message to a task")];
    let right = [(e.task, e.message), (e.task, e.other_task)];
    joins_only(&ctx, &e, RelationKind::MotivatedBy, "a task to a task or a message", &wrong, &right).await
}

#[tokio::test]
async fn discovered_from_joins_a_task_to_a_task_or_a_message() -> Result<()> {
    let ctx = context().await?;
    let e = ends(&ctx).await?;
    let wrong = [(e.task, e.decision, "a task to a decision"), (e.scope, e.message, "a scope to a message")];
    let right = [(e.task, e.message), (e.task, e.other_task)];
    joins_only(&ctx, &e, RelationKind::DiscoveredFrom, "a task to a task or a message", &wrong, &right).await
}

/// A decision supersedes a decision only through `fwd decision supersede`, which `link` names.
#[tokio::test]
async fn supersedes_joins_a_task_to_a_task_or_a_decision_to_a_decision() -> Result<()> {
    let ctx = context().await?;
    let e = ends(&ctx).await?;
    let wrong = [(e.task, e.decision, "a task to a decision"), (e.decision, e.task, "a decision to a task")];
    let joins = "a task to a task, or a decision to a decision";
    joins_only(&ctx, &e, RelationKind::Supersedes, joins, &wrong, &[(e.task, e.other_task)]).await?;
    let json = serde_json::json!({});
    let err = ops::link(&ctx, e.by, RelationKind::Supersedes, e.other_decision, e.decision, None, false, json).await;
    assert!(err.unwrap_err().to_string().contains("`fwd decision supersede <old> <new>"));
    Ok(())
}

#[tokio::test]
async fn duplicates_joins_a_task_to_a_task() -> Result<()> {
    let ctx = context().await?;
    let e = ends(&ctx).await?;
    let wrong = [(e.task, e.message, "a task to a message"), (e.anchor, e.task, "an anchor to a task")];
    joins_only(&ctx, &e, RelationKind::Duplicates, "a task to a task", &wrong, &[(e.task, e.other_task)]).await
}

#[tokio::test]
async fn anchored_at_joins_a_task_to_an_anchor() -> Result<()> {
    let ctx = context().await?;
    let e = ends(&ctx).await?;
    let wrong = [(e.task, e.scope, "a task to a scope"), (e.message, e.anchor, "a message to an anchor")];
    joins_only(&ctx, &e, RelationKind::AnchoredAt, "a task to an anchor", &wrong, &[(e.task, e.anchor)]).await
}

/// The kind the page-fixes agent found unchecked (2026-09-29, task 7dq1vEd_): a task worked in a
/// decision showed the decision's short id as its scope. An id that names nothing here is refused
/// too, and a notification is named as one.
#[tokio::test]
async fn worked_in_joins_a_task_to_a_scope() -> Result<()> {
    let ctx = context().await?;
    let e = ends(&ctx).await?;
    let wrong = [
        (e.task, e.decision, "a task to a decision"),
        (e.scope, e.scope, "a scope to a scope"),
        (e.task, e.notification, "a task to a notification"),
    ];
    joins_only(&ctx, &e, RelationKind::WorkedIn, "a task to a scope", &wrong, &[(e.task, e.scope)]).await?;
    let (gone, lost) = (EntityId::random(), EntityId::random());
    let short = |id: EntityId| id.to_base64().chars().take(8).collect::<String>();
    let link = |from, to| ops::link(&ctx, e.by, RelationKind::WorkedIn, from, to, None, false, serde_json::json!({}));
    assert_eq!(
        link(e.other_task, gone).await.unwrap_err().to_string(),
        format!("refused: `worked_in` joins a task to a scope, and `{}` names nothing in this project", short(gone))
    );
    assert_eq!(
        link(lost, gone).await.unwrap_err().to_string(),
        format!(
            "refused: `worked_in` joins a task to a scope, and `{}` and `{}` name nothing in this project",
            short(lost),
            short(gone)
        )
    );
    Ok(())
}

/// Anything is a step of a task: a message, a highlight, another task.
#[tokio::test]
async fn step_of_joins_anything_to_a_task() -> Result<()> {
    let ctx = context().await?;
    let e = ends(&ctx).await?;
    let wrong = [(e.message, e.decision, "a message to a decision"), (e.task, e.scope, "a task to a scope")];
    let right = [(e.message, e.task), (e.anchor, e.task), (e.other_task, e.task)];
    joins_only(&ctx, &e, RelationKind::StepOf, "anything to a task", &wrong, &right).await
}

#[tokio::test]
async fn concerns_joins_a_message_to_a_task_a_relation_or_a_scope() -> Result<()> {
    let ctx = context().await?;
    let e = ends(&ctx).await?;
    let wrong = [(e.message, e.decision, "a message to a decision"), (e.task, e.other_task, "a task to a task")];
    let right = [(e.message, e.task), (e.message, e.relation), (e.message, e.scope)];
    let joins = "a message to a task, a relation or a scope";
    joins_only(&ctx, &e, RelationKind::Concerns, joins, &wrong, &right).await
}

#[tokio::test]
async fn cites_joins_what_rests_on_a_decision_to_it_or_a_decision_to_its_context() -> Result<()> {
    let ctx = context().await?;
    let e = ends(&ctx).await?;
    let wrong = [(e.task, e.message, "a task to a message"), (e.scope, e.decision, "a scope to a decision")];
    let right = [(e.task, e.decision), (e.message, e.other_decision), (e.decision, e.message)];
    let joins = "a task, a message or a decision to a decision, or a decision to a task or a message";
    joins_only(&ctx, &e, RelationKind::Cites, joins, &wrong, &right).await
}

/// `relates_to` is free association: it joins anything to anything, as before.
#[tokio::test]
async fn relates_to_joins_anything_to_anything() -> Result<()> {
    let ctx = context().await?;
    let e = ends(&ctx).await?;
    let right = [(e.scope, e.principal), (e.notification, e.anchor), (e.decision, e.relation)];
    joins_only(&ctx, &e, RelationKind::RelatesTo, "anything to anything", &[], &right).await
}

/// `fwd task create` makes the relation of `kind` from the new task to what one of its flags names,
/// `made` putting that on a task's spec, as `link` makes it: to each of `right`, and to none of
/// `wrong` or an id that names nothing here, which it refuses with the sentence `link` gives
/// ("`kind` joins ..., not" what the pair's second item says the two are), making no task (task
/// iDJpHjiB: the flags wrote their relations unchecked, and the CLI passes a whole id of any kind
/// through).
async fn creates_only(
    ctx: &Context,
    e: &Ends,
    kind: RelationKind,
    made: fn(EntityId) -> CreateTask,
    wrong: &[(EntityId, &str)],
    right: &[EntityId],
) -> Result<()> {
    let joins = ops::joins_words(kind);
    let tasks = || async { Ok::<_, anyhow::Error>(ctx.fetch::<TaskView>("created > 0").await?.len()) };
    let before = tasks().await?;
    for (to, not) in wrong {
        let refused = ops::create_task(ctx, e.by, made(*to)).await.expect_err("a target of another kind").to_string();
        assert_eq!(refused, format!("refused: `{kind}` joins {joins}, not {not}"));
    }
    let gone = EntityId::random();
    let refused = ops::create_task(ctx, e.by, made(gone)).await.expect_err("a target that is nothing").to_string();
    let short: String = gone.to_base64().chars().take(8).collect();
    assert_eq!(refused, format!("refused: `{kind}` joins {joins}, and `{short}` names nothing in this project"));
    assert_eq!(tasks().await?, before, "a refused task is not made");
    for to in right {
        let task = ops::create_task(ctx, e.by, made(*to)).await?;
        let made: Vec<EntityId> =
            ops::relations_from(ctx, task, Some(kind)).await?.iter().map(|r| r.to_id()).collect::<Result<_, _>>()?;
        assert_eq!(made, vec![*to]);
    }
    Ok(())
}

#[tokio::test]
async fn a_task_made_after_another_depends_on_a_task_alone() -> Result<()> {
    let ctx = context().await?;
    let e = ends(&ctx).await?;
    let wrong =
        [(e.decision, "a task to a decision"), (e.message, "a task to a message"), (e.scope, "a task to a scope")];
    let after = |to| CreateTask { after: vec![to], ..task("Back off between retries") };
    creates_only(&ctx, &e, RelationKind::DependsOn, after, &wrong, &[e.task]).await
}

#[tokio::test]
async fn a_task_made_under_another_is_a_subtask_of_a_task_alone() -> Result<()> {
    let ctx = context().await?;
    let e = ends(&ctx).await?;
    let wrong = [(e.message, "a task to a message"), (e.anchor, "a task to an anchor")];
    let under = |to| CreateTask { under: Some(to), ..task("Jitter the backoff") };
    creates_only(&ctx, &e, RelationKind::SubtaskOf, under, &wrong, &[e.other_task]).await
}

#[tokio::test]
async fn a_task_made_from_something_was_discovered_from_a_task_or_a_message_alone() -> Result<()> {
    let ctx = context().await?;
    let e = ends(&ctx).await?;
    let wrong = [(e.decision, "a task to a decision"), (e.notification, "a task to a notification")];
    let from = |to| CreateTask { from: Some(to), ..task("Log each retry") };
    creates_only(&ctx, &e, RelationKind::DiscoveredFrom, from, &wrong, &[e.message, e.task]).await
}

#[tokio::test]
async fn a_task_made_motivated_by_something_is_motivated_by_a_task_or_a_message_alone() -> Result<()> {
    let ctx = context().await?;
    let e = ends(&ctx).await?;
    let wrong = [
        (e.decision, "a task to a decision"),
        (e.principal, "a task to a principal"),
        (e.relation, "a task to a relation"),
    ];
    let motivated = |to| CreateTask { motivated_by: Some(to), ..task("Cap the reconnect delay") };
    creates_only(&ctx, &e, RelationKind::MotivatedBy, motivated, &wrong, &[e.message, e.other_task]).await
}
