//! Routing across the projects of one node (`fwd_core::routing`): a project is a grouping, not a
//! wall (docs/DECISIONS.md, D176), so each rule reads its principals where its subject is, the
//! triage agent of a scope in the scope's project, wherever the record that concerns the scope is;
//! a recipient that a reference leads to in another project is told; and each notification is made
//! in its recipient's project, whose inbox lists it, whichever project's record caused it.

use ankurah::{policy::DEFAULT_CONTEXT, EntityId, Node, PermissiveAgent};
use ankurah_storage_sqlite::SqliteStorageEngine;
use anyhow::Result;
use fwd_core::model::*;
use fwd_core::ops::{self, CreateDecision, CreateScope, CreateTask, MessageContext};
use fwd_core::project_graph::ProjectGraph;
use fwd_core::reason;
use fwd_core::routing::{self, Planned};
use fwd_core::rules::{DecisionMove, Verb};
use serde_json::json;
use std::sync::Arc;

/// One project's people over the shared store: the person, an implementer and a triage agent that
/// serves the project whole, by the same handles in both projects.
struct People {
    graph: ProjectGraph,
    daniel: EntityId,
    alice: EntityId,
    triage: EntityId,
}

async fn people(graph: ProjectGraph) -> Result<People> {
    let daniel = ops::ensure_principal(&graph, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    ops::join(&graph, daniel).await?;
    let alice = ops::add_principal(&graph, "impl-alice", Some(AgentRole::Implementer), None, None, None, None).await?;
    let triage = ops::add_principal(
        &graph,
        "triage-bot",
        Some(AgentRole::Triage),
        Some(ops::Serves::ProjectWide),
        None,
        None,
        None,
    )
    .await?;
    Ok(People { graph, daniel, alice, triage })
}

/// alpha and beta over one store in memory.
async fn two() -> Result<(People, People)> {
    let storage = SqliteStorageEngine::open_in_memory().await?;
    let node = Node::new_durable(Arc::new(storage), PermissiveAgent::new());
    node.system.create().await?;
    let ctx = node.context_async(DEFAULT_CONTEXT).await?;
    let alpha = ProjectGraph::ensure(ctx.clone(), uuid::Uuid::new_v4(), "alpha").await?;
    let beta = ProjectGraph::ensure(ctx, uuid::Uuid::new_v4(), "beta").await?;
    Ok((people(alpha).await?, people(beta).await?))
}

async fn task(graph: &ProjectGraph, by: EntityId, title: &str) -> Result<EntityId> {
    ops::create_task(graph, by, CreateTask { title: title.into(), ..Default::default() }).await
}

/// Each plan's recipient and type, sorted.
fn told(planned: &[Planned]) -> Vec<(EntityId, NotificationType)> {
    let mut told: Vec<_> = planned.iter().map(|p| (p.recipient, p.notification_type)).collect();
    told.sort();
    told
}

/// The types of the notifications `who` holds in `graph`'s project's inbox.
async fn inbox(graph: &ProjectGraph, who: EntityId) -> Result<Vec<NotificationType>> {
    Ok(ops::inbox(graph, who, false).await?.iter().map(|n| n.notification_type()).collect::<Result<_, _>>()?)
}

/// A comment on a task of alpha's worked in a scope of beta's reaches the triage agent that serves
/// that scope, read among beta's principals, in beta's inbox, and not alpha's agent that serves
/// alpha whole; a comment on a task of alpha's worked in no scope reaches alpha's.
#[tokio::test]
async fn a_comment_on_a_task_worked_in_another_projects_scope_reaches_that_scopes_triage_agent_there() -> Result<()> {
    let (a, b) = two().await?;
    let scope =
        ops::create_scope(&b.graph, b.daniel, CreateScope { title: "Replication".into(), ..Default::default() })
            .await?;
    let of_scope = ops::add_principal(
        &b.graph,
        "triage-repl",
        Some(AgentRole::Triage),
        Some(ops::Serves::Scope(scope)),
        None,
        None,
        None,
    )
    .await?;
    let worked = task(&a.graph, a.daniel, "Retry on disconnect").await?;
    ops::link(&a.graph, a.daniel, RelationKind::WorkedIn, worked, scope, None, false, json!({})).await?;
    let unscoped = task(&a.graph, a.daniel, "Cap the retry delay").await?;

    let comment = |task: EntityId| {
        let graph = a.graph.clone();
        async move {
            let on = MessageContext { task_id: Some(task), ..Default::default() };
            let id = ops::post_message(&graph, a.daniel, "the backoff doubles", Priority::Next, on).await?;
            routing::plan_for_message(&graph, &graph.get(id).await?).await
        }
    };
    let planned = comment(worked).await?;
    assert_eq!(told(&planned), vec![(of_scope, NotificationType::TaskCommented)]);
    assert_eq!(planned[0].originator["triage_scope_id"], scope.to_base64(), "it triages the scope as its own");
    routing::apply(&a.graph, &planned).await?;
    assert_eq!(inbox(&b.graph, of_scope).await?, vec![NotificationType::TaskCommented], "in beta's inbox");
    assert!(a.graph.all::<NotificationView>().await?.is_empty(), "nothing in alpha's");

    assert_eq!(told(&comment(unscoped).await?), vec![(a.triage, NotificationType::TaskCommented)]);
    Ok(())
}

/// A task of beta's that another project's task depends on, finished in beta, tells the assignee
/// of the dependent in alpha, in alpha's inbox; a dependent nobody holds tells the triage agent of
/// its own project, never beta's.
#[tokio::test]
async fn a_finished_task_tells_whoever_holds_a_dependent_in_another_project_in_its_inbox() -> Result<()> {
    let (a, b) = two().await?;
    let blocker = task(&b.graph, b.daniel, "Ship the connector").await?;
    let held = task(&a.graph, a.daniel, "Use the connector").await?;
    let unheld = task(&a.graph, a.daniel, "Document the connector").await?;
    for dependent in [held, unheld] {
        ops::link(&a.graph, a.daniel, RelationKind::DependsOn, dependent, blocker, None, false, json!({})).await?;
    }
    ops::assign(&a.graph, a.daniel, held, Some(a.alice)).await?;
    ops::transition(&b.graph, b.daniel, blocker, Verb::Done, None).await?;
    let finished: TaskView = b.graph.get(blocker).await?;
    let planned = routing::plan_for_task_status(&b.graph, &finished, TaskStatus::Open, Some(b.daniel)).await?;
    let mut expected = vec![(a.alice, NotificationType::TaskUnblocked), (a.triage, NotificationType::TaskUnblocked)];
    expected.sort();
    assert_eq!(told(&planned), expected);
    routing::apply(&b.graph, &planned).await?;
    assert_eq!(inbox(&a.graph, a.alice).await?, vec![NotificationType::TaskUnblocked]);
    assert_eq!(inbox(&a.graph, a.triage).await?, vec![NotificationType::TaskUnblocked]);
    assert!(b.graph.all::<NotificationView>().await?.is_empty(), "nothing in beta's");
    Ok(())
}

/// A decision of beta's that a task of alpha's cites, repealed in beta, tells the task's author in
/// alpha's inbox, and the sentence that says why names the decision with beta's name, since `D1`
/// in alpha is another decision.
#[tokio::test]
async fn a_repealed_decision_tells_the_author_of_another_projects_citation_naming_its_project() -> Result<()> {
    let (a, b) = two().await?;
    let ruling =
        CreateDecision { title: "Cap retries at 30s".into(), decided_by: Some(b.daniel), ..Default::default() };
    let decision = ops::create_decision(&b.graph, b.daniel, ruling).await?;
    let ours = CreateDecision { title: "Log every retry".into(), decided_by: Some(a.daniel), ..Default::default() };
    ops::create_decision(&a.graph, a.daniel, ours).await?;
    let citing = task(&a.graph, a.alice, "Cap the retry delay").await?;
    ops::cite(&a.graph, a.alice, citing, decision, None).await?;
    ops::move_decision(&b.graph, b.daniel, decision, DecisionMove::Repeal, "the transport retries now").await?;
    let repealed: DecisionView = b.graph.get(decision).await?;
    let planned =
        routing::plan_for_decision_status(&b.graph, &repealed, DecisionStatus::Accepted, Some(b.daniel)).await?;
    assert_eq!(told(&planned), vec![(a.alice, NotificationType::DecisionRepealed)]);
    routing::apply(&b.graph, &planned).await?;
    let held = ops::inbox(&a.graph, a.alice, false).await?;
    assert_eq!(held.len(), 1, "in alpha's inbox");
    let said = reason::plain(&reason::gather(&a.graph, &held[0], Some(a.alice)).await?);
    assert!(said.contains("D1 (Cap retries at 30s) in beta"), "{said}");
    Ok(())
}

/// A message of alpha's that names a principal of beta's, by its id, tells that principal in
/// beta's inbox; the sentence beta's principal reads names alpha's decision that the text cites with
/// alpha's name, never as a bare `D1`, which in beta is another decision.
#[tokio::test]
async fn a_message_that_names_another_projects_principal_tells_it_in_its_inbox() -> Result<()> {
    let (a, b) = two().await?;
    for (graph, by) in [(&a.graph, a.daniel), (&b.graph, b.daniel)] {
        let ruling = CreateDecision { title: "Cap retries at 30s".into(), decided_by: Some(by), ..Default::default() };
        ops::create_decision(graph, by, ruling).await?;
    }
    let ours = ops::decisions(&a.graph).await?[0].id();
    let text = format!("does [[{ours}]] hold, [[{}]]?", b.alice);
    let posted = ops::post_message(&a.graph, a.daniel, &text, Priority::Next, MessageContext::default()).await?;
    let planned = routing::plan_for_message(&a.graph, &a.graph.get(posted).await?).await?;
    assert_eq!(told(&planned), vec![(b.alice, NotificationType::Mentioned)]);
    routing::apply(&a.graph, &planned).await?;
    let held = ops::inbox(&b.graph, b.alice, false).await?;
    assert_eq!(held.len(), 1, "in beta's inbox");
    assert!(a.graph.all::<NotificationView>().await?.is_empty(), "nothing in alpha's");
    let said = reason::plain(&reason::gather(&b.graph, &held[0], Some(b.alice)).await?);
    assert!(said.contains("does D1 in alpha hold"), "{said}");
    Ok(())
}

/// A step that alpha adds to a task of beta's tells the task's creator and its assignee, beta's
/// principals, whose checklist it is on, in beta's inboxes.
#[tokio::test]
async fn a_step_added_to_another_projects_task_tells_its_creator_and_assignee_there() -> Result<()> {
    let (a, b) = two().await?;
    let theirs = task(&b.graph, b.daniel, "Review: the retry loop").await?;
    ops::assign(&b.graph, b.daniel, theirs, Some(b.alice)).await?;
    let step = ops::add_step(&a.graph, a.alice, theirs, EntityId::random()).await?;
    let planned = routing::plan_for_relation(&a.graph, &a.graph.relation(step).await?).await?;
    let mut expected = vec![(b.daniel, NotificationType::StepAdded), (b.alice, NotificationType::StepAdded)];
    expected.sort();
    assert_eq!(told(&planned), expected);
    routing::apply(&a.graph, &planned).await?;
    for who in [b.daniel, b.alice] {
        assert_eq!(inbox(&b.graph, who).await?, vec![NotificationType::StepAdded], "in beta's inbox");
    }
    assert!(a.graph.all::<NotificationView>().await?.is_empty(), "nothing in alpha's");
    Ok(())
}
