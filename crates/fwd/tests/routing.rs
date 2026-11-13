//! T7's exit criterion, end to end: a comment written by one ephemeral client becomes a
//! notification in another principal's inbox through the daemon's routing service; and an
//! assignment, a change to a task's field rather than a new entity, reaches the new assignee. And
//! the daemon runs that service once for every project it hosts, which tells whom a write in either
//! of two projects names once, each in its own project's inbox.

mod common;

use ankurah::changes::ChangeSet;
use ankurah::signals::Subscribe;
use ankurah::EntityId;
use anyhow::{anyhow, Result};
use fwd_core::daemon::{self, Autostart};
use fwd_core::model::*;
use fwd_core::ops::{self, CreateTask, MessageContext};
use fwd_core::project;
use fwd_core::project_graph::ProjectGraph;
use fwd_core::query::with_id;
use fwd_core::rules::Verb;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

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
    let ctx = &daniel_client.graph;
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
        let inbox = ops::inbox(&alice_client.graph, alice, true).await?;
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

/// The notifications of one recipient as they arrive, from a live query on its project's graph.
struct Arrivals {
    arrived: mpsc::UnboundedReceiver<NotificationView>,
    _query: ankurah::LiveQuery<NotificationView>,
    _guard: ankurah::signals::SubscriptionGuard,
}

impl Arrivals {
    fn of(graph: &ProjectGraph, recipient: EntityId) -> Result<Arrivals> {
        let query = graph.query::<NotificationView>(with_id("recipient = ?", recipient)?)?;
        let (tx, arrived) = mpsc::unbounded_channel();
        let _guard = query.subscribe(move |changes: ChangeSet<NotificationView>| {
            for n in changes.added() {
                let _ = tx.send(n);
            }
        });
        Ok(Arrivals { arrived, _query: query, _guard })
    }

    /// Returns once a notification of type `t` has arrived, about the message `message` when one
    /// is given. Fails when a minute passes first: the minute only detects a notification that
    /// never comes, and what orders the test is its arrival.
    async fn until(&mut self, t: NotificationType, message: Option<EntityId>) -> Result<()> {
        let arrived = async {
            loop {
                let n = self.arrived.recv().await.ok_or_else(|| anyhow!("the query ended"))?;
                let about = serde_json::Value::from(n.originator()?)["message_id"].as_str().map(str::to_string);
                if n.notification_type()? == t && message.is_none_or(|m| about == Some(m.to_base64())) {
                    return Ok(());
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(60), arrived)
            .await
            .map_err(|_| anyhow!("no {t:?} arrived within a minute"))?
    }
}

/// The types of the notifications `who` holds in `graph`'s project.
async fn held(graph: &ProjectGraph, who: EntityId) -> Result<Vec<NotificationType>> {
    Ok(ops::inbox(graph, who, false).await?.iter().map(|n| n.notification_type()).collect::<Result<_, _>>()?)
}

/// One daemon hosting alpha and beta runs one routing service, which routes each write once, by
/// the rules of the project it was made in, and tells each recipient in its own project: a message
/// of alpha's that names beta's implementer tells her once, in beta's inbox, and a task of beta's
/// finished in beta tells the assignee of alpha's task that waits on it once, in alpha's inbox.
#[tokio::test]
async fn one_service_for_two_projects_tells_each_recipient_of_a_write_once_in_its_own_project() -> Result<()> {
    let dir = common::Scratch::new()?;
    let paths = dir.paths("run", "data");
    let init = |name: &str| -> Result<project::NamedProject> {
        let root = dir.path().join(name);
        std::fs::create_dir_all(&root)?;
        Ok(project::init(&paths, &root, project::InitOptions { name: Some(name), ..Default::default() })?.0.project)
    };
    let (alpha_ws, beta_ws) = (init("alpha")?, init("beta")?);
    let spawn = Autostart::Spawn { daemon_bin: PathBuf::from(env!("CARGO_BIN_EXE_fwd")) };
    let alpha = daemon::ensure_daemon(&paths, &alpha_ws, spawn.clone()).await?;
    let beta = daemon::ensure_daemon(&paths, &beta_ws, spawn).await?;
    assert_eq!(alpha.pf.instance_id, beta.pf.instance_id, "one daemon hosts both");
    let (a, b) = (&alpha.graph, &beta.graph);
    let implementer = |graph: &ProjectGraph| {
        let graph = graph.clone();
        async move {
            ops::ensure_principal(&graph, "impl-alice", "Alice", PrincipalKind::Agent, Some(AgentRole::Implementer))
                .await
        }
    };
    let daniel = ops::ensure_principal(a, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let alice = implementer(a).await?;
    let their_daniel = ops::ensure_principal(b, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let their_alice = implementer(b).await?;
    let blocker =
        ops::create_task(b, their_daniel, CreateTask { title: "Ship the connector".into(), ..Default::default() })
            .await?;
    let waits =
        ops::create_task(a, daniel, CreateTask { title: "Use the connector".into(), ..Default::default() }).await?;
    ops::link(a, daniel, RelationKind::DependsOn, waits, blocker, None, false, serde_json::json!({})).await?;
    ops::assign(a, daniel, waits, Some(alice)).await?;
    let (mut hers, mut alices) = (Arrivals::of(b, their_alice)?, Arrivals::of(a, alice)?);

    let text = format!("[[{their_alice}]], the connector is yours");
    let named = ops::post_message(a, daniel, &text, Priority::Next, MessageContext::default()).await?;
    hers.until(NotificationType::Mentioned, Some(named)).await?;
    ops::transition(b, their_daniel, blocker, Verb::Done, None).await?;
    alices.until(NotificationType::TaskUnblocked, None).await?;
    // The service routes the changes of each collection in the order they were written, so once a
    // mention written last has reached her, the one before it has been routed; and the assignment,
    // written before the finish, was routed before the finish was.
    let last =
        ops::post_message(a, daniel, &format!("[[{their_alice}]], and the docs"), Priority::Next, Default::default())
            .await?;
    hers.until(NotificationType::Mentioned, Some(last)).await?;

    assert_eq!(held(b, their_alice).await?, vec![NotificationType::Mentioned; 2], "each mention once, in beta");
    let mut alpha_held = held(a, alice).await?;
    alpha_held.sort();
    assert_eq!(alpha_held, vec![NotificationType::TaskAssigned, NotificationType::TaskUnblocked]);
    for (graph, who) in [(a, their_alice), (b, alice)] {
        assert!(ops::inbox(graph, who, false).await?.is_empty(), "no notification in the other project's inbox");
    }
    drop((alpha, beta));
    assert!(daemon::stop(&paths).await?);
    let log = std::fs::read_to_string(paths.log_path())?;
    assert_eq!(log.matches("hosting project").count(), 2, "{log}");
    assert_eq!(log.matches("routing service started").count(), 1, "one service for both projects:\n{log}");
    Ok(())
}
