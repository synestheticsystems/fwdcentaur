//! T7's exit criterion, end to end: a comment written by one ephemeral client becomes a
//! notification in another principal's inbox through the daemon's routing service; and an
//! assignment, a change to a task's field rather than a new entity, reaches the new assignee.

mod common;

use anyhow::Result;
use fwd_core::daemon::{self, Autostart};
use fwd_core::model::*;
use fwd_core::ops::{self, CreateTask, MessageContext};
use fwd_core::project;
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[tokio::test]
async fn a_comment_reaches_the_implementer_and_the_triage_agent() -> Result<()> {
    let dir = common::Scratch::new()?;
    let paths = dir.paths("run", "data");
    let root = dir.path().join("repo");
    std::fs::create_dir_all(&root)?;
    let ws =
        project::init(&paths, &root, project::InitOptions { name: Some("routing"), ..Default::default() })?.0.project;
    let spawn = Autostart::Spawn { daemon_bin: PathBuf::from(env!("CARGO_BIN_EXE_fwd")) };

    let daniel_client = daemon::ensure_daemon(&paths, &ws, spawn.clone()).await?;
    let ctx = &daniel_client.ctx;
    let daniel = ops::ensure_principal(ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let alice =
        ops::ensure_principal(ctx, "impl-alice", "Alice", PrincipalKind::Agent, Some(AgentRole::Implementer)).await?;
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
    let task =
        ops::create_task(ctx, daniel, CreateTask { title: "Retry on disconnect".into(), ..Default::default() }).await?;
    // Daniel assigns the task to Alice, so Alice (not the actor) is told.
    ops::assign(ctx, daniel, task, Some(alice)).await?;

    // A second client, as Alice would be in her own session, sees the assignment and the comment.
    let alice_client = daemon::ensure_daemon(&paths, &ws, spawn.clone()).await?;
    let message = ops::post_message(
        ctx,
        daniel,
        "back off, don't hammer",
        Priority::Next,
        MessageContext { task_id: Some(task), ..Default::default() },
    )
    .await?;

    let deadline = Instant::now() + Duration::from_secs(10);
    let inbox = loop {
        let inbox = ops::inbox(&alice_client.ctx, alice, true).await?;
        if inbox.iter().any(|n| n.notification_type().ok() == Some(NotificationType::TaskCommented)) {
            break inbox;
        }
        assert!(Instant::now() < deadline, "the routing service did not deliver within 10s: {inbox:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let commented = inbox.iter().find(|n| n.notification_type().ok() == Some(NotificationType::TaskCommented)).unwrap();
    let originator: serde_json::Value = commented.originator()?.into();
    assert_eq!(originator["message_id"], message.to_base64());
    let assigned: Vec<_> =
        inbox.iter().filter(|n| n.notification_type().ok() == Some(NotificationType::TaskAssigned)).collect();
    assert_eq!(assigned.len(), 1, "the assignment produced one TaskAssigned: {inbox:?}");
    let originator: serde_json::Value = assigned[0].originator()?.into();
    assert_eq!(originator["why"], "assignee");
    assert_eq!(originator["actor_id"], daniel.to_base64());

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let triage_inbox = ops::inbox(ctx, triage, true).await?;
        if triage_inbox.iter().any(|n| n.notification_type().ok() == Some(NotificationType::TaskCommented)) {
            break;
        }
        assert!(Instant::now() < deadline, "the triage agent was not notified within 10s");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(ops::inbox(ctx, daniel, true).await?.is_empty(), "the author is never notified of their own comment");

    drop(alice_client);
    drop(daniel_client);
    assert!(daemon::stop(&paths).await?);
    Ok(())
}
