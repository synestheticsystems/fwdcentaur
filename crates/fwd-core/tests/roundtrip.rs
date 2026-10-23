//! T2's exit criterion: create two tasks, a depends_on relation, a scope, a message on an anchor
//! and a notification on a SQLite durable node; reopen the store; read everything back with
//! typed reverse queries and a JSON-path query.

use ankurah::{
    policy::DEFAULT_CONTEXT,
    property::{Json, Ref},
    EntityId, Node, PermissiveAgent,
};
use ankurah_storage_sqlite::SqliteStorageEngine;
use anyhow::Result;
use fwd_core::model::*;
use fwd_core::now_ms;
use fwd_core::query::with_id;
use std::sync::Arc;

struct Ids {
    daniel: EntityId,
    scope: EntityId,
    t_retry: EntityId,
    t_ws: EntityId,
    relation: EntityId,
    anchor: EntityId,
    message: EntityId,
    notification: EntityId,
}

async fn populate(node: &Node<SqliteStorageEngine, PermissiveAgent>) -> Result<Ids> {
    let ctx = node.context_async(DEFAULT_CONTEXT).await?;
    let trx = ctx.begin();
    let now = now_ms();

    let daniel = trx
        .create(&Principal {
            kind: PrincipalKind::Human,
            handle: "dnorman".into(),
            display_name: "Daniel Norman".into(),
            auth_subject: Some("github:1234".into()),
            agent_role: None,
            active: true,
            member: Some(true),
            subject: None,
            succeeded_by: None,
            serves: None,
            last_active: None,
        })
        .await?
        .id();

    let repo = trx
        .create(&Repo {
            name: "fwdcentaur".into(),
            remote_urls: Json::new(serde_json::json!(["git@github.com:synestheticsystems/fwdcentaur.git"])),
            default_branch: "main".into(),
        })
        .await?
        .id();

    let scope = trx
        .create(&Scope {
            status: ScopeStatus::Active,
            title: "Inbox delivery for Claude sessions".into(),
            intention: "Get a comment on code to an implementer mid-task.".into(),
            context: Json::new(serde_json::json!({"repo_id": repo.to_base64(), "branch": "feat/inbox"})),
            started: Some(now),
            ended: None,
            created_by: Ref::new(daniel),
            created: now,
        })
        .await?
        .id();

    let task = |title: &str, status: TaskStatus| Task {
        title: title.into(),
        what: format!("{title}: spec"),
        why: "Laptops behind NAT drop; agents lose inbox delivery".into(),
        done_when: "Reconnect within 5s; test covers 3 drops".into(),
        status,
        needs_review: true,
        hold: None,
        assignee: None,
        priority: Some(1),
        labels: Json::new(serde_json::json!(["daemon"])),
        context: Json::new(serde_json::json!({"scope_id": scope.to_base64()})),
        created_by: Ref::new(daniel),
        created: now,
        updated: now,
        closed: None,
        closed_reason: None,
    };
    let t_ws = trx.create(&task("WebSocket connector", TaskStatus::Open)).await?.id();
    let t_retry = trx.create(&task("Retry on websocket disconnect", TaskStatus::Proposed)).await?.id();

    let relation = trx
        .create(&Relation {
            kind: RelationKind::DependsOn,
            from_id: t_retry,
            to_id: t_ws,
            status: RelationStatus::Accepted,
            created_by: Ref::new(daniel),
            created: now,
            via: Json::new(serde_json::json!({"actor_id": daniel.to_base64()})),
            note: None,
            position: None,
            closed_by: None,
            closed: None,
            closed_reason: None,
        })
        .await?
        .id();

    let anchor = trx
        .create(&CodeAnchor {
            repo: Ref::new(repo),
            path: "src/net/ws.rs".into(),
            anchor_key: Some("fwd-anchor:1:00".into()),
            span_key: Some("fwd-span:1:00".into()),
            basis_blob: Some("9f3c0000".into()),
            head_blob: Some("9f3c0000".into()),
            index_blob: Some("9f3c0000".into()),
            nearest_blob: Some("9f3c0000".into()),
            basis_patch: Some(String::new()),
            blob_sha: None,
            commit_sha: Some("abcd0000".into()),
            side: DiffSide::Modified,
            start_line: 120,
            end_line: 134,
            whole_file: Some(false),
            anchored_text: "loop {\n    reconnect().await?;\n}".into(),
            context_before: "// reconnect".into(),
            context_after: "Ok(())".into(),
            pieces: Json::new(
                serde_json::json!([{"basis_lines": [120, 134], "head_lines": [120, 134], "index_lines": [120, 134]}]),
            ),
            symbol_path: None,
            created_by: Ref::new(daniel),
            created: now,
        })
        .await?
        .id();

    let message = trx
        .create(&Message {
            author: Ref::new(daniel),
            created: now,
            content: "Back off, don't hammer the server on reconnect.".into(),
            priority: Priority::Next,
            status: MessageStatus::Active,
            task: Some(Ref::new(t_retry)),
            anchor: Some(Ref::new(anchor)),
            scope: None,
            decision: None,
            reply_to: None,
            thread: None,
            context: Json::new(serde_json::json!({})),
            resolved_by: None,
            resolved: None,
            edited: None,
            edited_by: None,
        })
        .await?
        .id();

    let notification = trx
        .create(&Notification {
            recipient: Ref::new(daniel),
            notification_type: NotificationType::CodeCommented,
            status: NotificationStatus::Unread,
            priority: Priority::Next,
            created: now,
            updated: None,
            target: Json::new(serde_json::json!({"type": "CodeAnchor", "id": anchor.to_base64()})),
            originator: Json::new(
                serde_json::json!({"message_id": message.to_base64(), "actor_id": daniel.to_base64()}),
            ),
        })
        .await?
        .id();

    trx.commit().await?;
    Ok(Ids { daniel, scope, t_retry, t_ws, relation, anchor, message, notification })
}

#[tokio::test]
async fn models_round_trip_through_sqlite() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("workspace.sqlite");

    let ids = {
        let storage = SqliteStorageEngine::open(&path).await?;
        let node = Node::new_durable(Arc::new(storage), PermissiveAgent::new());
        node.system.create().await?;
        populate(&node).await?
    };

    // Reopen the same file as a fresh durable node: nothing is cached in memory any more.
    let storage = SqliteStorageEngine::open(&path).await?;
    let node = Node::new_durable(Arc::new(storage), PermissiveAgent::new());
    node.system.wait_loaded().await?;
    assert!(node.system.root().is_some(), "the system created before reopening must be found on disk");
    let ctx = node.context_async(DEFAULT_CONTEXT).await?;

    // Direct reads.
    let t_retry: TaskView = ctx.get(ids.t_retry).await?;
    assert_eq!(t_retry.title()?, "Retry on websocket disconnect");
    assert_eq!(t_retry.status()?, TaskStatus::Proposed);
    assert_eq!(t_retry.why()?, "Laptops behind NAT drop; agents lose inbox delivery");
    let scope: ScopeView = ctx.get(ids.scope).await?;
    assert_eq!(scope.status()?, ScopeStatus::Active);
    assert_eq!(EntityId::from(scope.created_by()?), ids.daniel);

    // Status predicates use the stored snake_case names.
    let proposed: Vec<TaskView> = ctx.fetch("status = 'proposed'").await?;
    assert_eq!(proposed.len(), 1);
    assert_eq!(proposed[0].id(), ids.t_retry);

    // Typed reverse queries over Ref fields: what depends on t_ws, and who is notified.
    let blocking: Vec<RelationView> = ctx.fetch(with_id("to_id = ?", ids.t_ws)?).await?;
    assert_eq!(blocking.len(), 1);
    assert_eq!(blocking[0].id(), ids.relation);
    assert_eq!(blocking[0].kind()?, RelationKind::DependsOn);
    assert_eq!(blocking[0].from_id()?, ids.t_retry);

    let inbox: Vec<NotificationView> = ctx.fetch(with_id("recipient = ?", ids.daniel)?).await?;
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].id(), ids.notification);
    assert_eq!(inbox[0].notification_type()?, NotificationType::CodeCommented);
    assert_eq!(inbox[0].status()?, NotificationStatus::Unread);
    let originator: serde_json::Value = inbox[0].originator()?.into();
    assert_eq!(originator["message_id"], ids.message.to_base64());

    // Typed reverse query from the anchor to the messages on it.
    let on_anchor: Vec<MessageView> = ctx.fetch(with_id("anchor = ?", ids.anchor)?).await?;
    assert_eq!(on_anchor.len(), 1);
    assert_eq!(on_anchor[0].id(), ids.message);
    assert_eq!(on_anchor[0].content()?, "Back off, don't hammer the server on reconnect.");
    assert_eq!(on_anchor[0].edited()?, None, "a message never edited has no time of an edit");
    assert!(on_anchor[0].edited_by()?.is_none(), "nor anyone who edited it");

    let anchor: CodeAnchorView = ctx.get(ids.anchor).await?;
    assert_eq!(anchor.basis_blob()?.as_deref(), Some("9f3c0000"));
    assert_eq!(anchor.whole_file()?, Some(false));
    assert_eq!((anchor.start_line()?, anchor.end_line()?), (120, 134));

    Ok(())
}
