//! Each reader of an id a record holds, where the record it names is in another project of the
//! node: the reader follows it there and names that project beside the record (docs/DECISIONS.md,
//! D176), and one the node does not hold is said to be one that cannot be followed here. Readiness
//! weighs a dependency wherever it leads, a cycle through two projects is refused, and only the
//! project that made a relation changes it.

use ankurah::{policy::DEFAULT_CONTEXT, EntityId, Node, PermissiveAgent};
use ankurah_storage_sqlite::SqliteStorageEngine;
use anyhow::Result;
use fwd_core::model::*;
use fwd_core::ops::{self, CreateScope, CreateTask, MessageContext};
use fwd_core::project_graph::ProjectGraph;
use fwd_core::rules::{NotReadyReason, Readiness, Verb};
use serde_json::json;
use std::sync::Arc;

/// alpha and beta, each with the person and an implementer of the same handles, over one store.
struct Two {
    alpha: ProjectGraph,
    beta: ProjectGraph,
    daniel: EntityId,
    alice: EntityId,
    /// Beta's principals of the same handles.
    their_daniel: EntityId,
    their_alice: EntityId,
}

async fn two() -> Result<Two> {
    let storage = SqliteStorageEngine::open_in_memory().await?;
    let node = Node::new_durable(Arc::new(storage), PermissiveAgent::new());
    node.system.create().await?;
    let ctx = node.context_async(DEFAULT_CONTEXT).await?;
    let alpha = ProjectGraph::ensure(ctx.clone(), uuid::Uuid::new_v4(), "alpha").await?;
    let beta = ProjectGraph::ensure(ctx, uuid::Uuid::new_v4(), "beta").await?;
    let people = |graph: ProjectGraph| async move {
        let daniel = ops::ensure_principal(&graph, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
        let alice =
            ops::add_principal(&graph, "impl-alice", Some(AgentRole::Implementer), None, None, None, None).await?;
        anyhow::Ok((daniel, alice))
    };
    let (daniel, alice) = people(alpha.clone()).await?;
    let (their_daniel, their_alice) = people(beta.clone()).await?;
    Ok(Two { alpha, beta, daniel, alice, their_daniel, their_alice })
}

async fn task(graph: &ProjectGraph, by: EntityId, title: &str) -> Result<EntityId> {
    ops::create_task(graph, by, CreateTask { title: title.into(), ..Default::default() }).await
}

async fn depends_on(graph: &ProjectGraph, by: EntityId, from: EntityId, to: EntityId) -> Result<EntityId> {
    ops::link(graph, by, RelationKind::DependsOn, from, to, None, false, json!({})).await
}

#[tokio::test]
async fn a_dependency_on_another_projects_task_gates_readiness_in_each_state_and_names_its_project() -> Result<()> {
    let t = two().await?;
    let blocker = task(&t.beta, t.their_daniel, "Ship the connector").await?;
    let waiting = task(&t.alpha, t.daniel, "Use the connector").await?;
    depends_on(&t.alpha, t.daniel, waiting, blocker).await?;
    let readiness = || async { ops::readiness(&t.alpha, &t.alpha.get::<TaskView>(waiting).await?).await };

    // Unfinished: it blocks, and the reason names the task and beta.
    let blocked = Readiness::NotReady(vec![NotReadyReason::BlockedByTaskIn { task: blocker, project: "beta".into() }]);
    assert_eq!(readiness().await?, blocked);
    assert!(ops::ready_tasks(&t.alpha, None).await?.iter().all(|r| r.id() != waiting));

    // Finished there: it counts as finished, and the task is ready.
    ops::transition(&t.beta, t.their_daniel, blocker, Verb::Done, None).await?;
    assert_eq!(readiness().await?, Readiness::Ready);
    assert!(ops::ready_tasks(&t.alpha, None).await?.iter().any(|r| r.id() == waiting));

    // Closed without being done: a dependency issue, named with beta.
    let cancelled = task(&t.beta, t.their_daniel, "Ship the other connector").await?;
    ops::transition(&t.beta, t.their_daniel, cancelled, Verb::Cancel, Some("not needed")).await?;
    let second = task(&t.alpha, t.daniel, "Use the other connector").await?;
    depends_on(&t.alpha, t.daniel, second, cancelled).await?;
    assert_eq!(
        ops::readiness(&t.alpha, &t.alpha.get::<TaskView>(second).await?).await?,
        Readiness::NotReady(vec![NotReadyReason::DependencyClosedIn {
            task: cancelled,
            project: "beta".into(),
            status: TaskStatus::Cancelled
        }])
    );

    // Not held: a relation whose target the node holds no record of blocks, saying it cannot be
    // followed here. `link` refuses such an end, so the relation is written as an older build or a
    // store merge left one.
    let gone = EntityId::random();
    let third = task(&t.alpha, t.daniel, "Wait on what is gone").await?;
    let trx = t.alpha.begin();
    trx.create(&Relation {
        project: t.alpha.project(),
        kind: RelationKind::DependsOn,
        from_id: third,
        to_id: gone,
        status: RelationStatus::Accepted,
        created_by: ankurah::property::Ref::new(t.daniel),
        created: fwd_core::now_ms(),
        via: ankurah::property::Json::new(json!({})),
        note: None,
        position: None,
        closed_by: None,
        closed: None,
        closed_reason: None,
    })
    .await?;
    trx.commit().await?;
    assert_eq!(
        ops::readiness(&t.alpha, &t.alpha.get::<TaskView>(third).await?).await?,
        Readiness::NotReady(vec![NotReadyReason::DependencyNotHeld(gone)])
    );
    Ok(())
}

#[tokio::test]
async fn a_cycle_through_two_projects_is_refused_as_one_within_a_project_is() -> Result<()> {
    let t = two().await?;
    let ours = task(&t.alpha, t.daniel, "Ours").await?;
    let theirs = task(&t.beta, t.their_daniel, "Theirs").await?;
    depends_on(&t.alpha, t.daniel, ours, theirs).await?;
    let refused = depends_on(&t.beta, t.their_daniel, theirs, ours).await.expect_err("a cycle through alpha");
    assert!(refused.to_string().contains("would close a cycle"), "{refused}");
    let made = ops::create_task(
        &t.beta,
        t.their_daniel,
        CreateTask { title: "Under".into(), after: vec![ours], ..Default::default() },
    );
    assert!(made.await.is_ok(), "depending on alpha's task closes no cycle by itself");
    Ok(())
}

#[tokio::test]
async fn a_far_author_assignee_and_scope_are_named_with_their_project() -> Result<()> {
    let t = two().await?;
    // A message in alpha written by beta's principal, and a task of alpha's assigned to beta's
    // implementer, as a task moved from beta keeps them.
    let mc = MessageContext::default();
    let said = ops::post_message(&t.alpha, t.their_daniel, "from beta", Priority::Next, mc).await?;
    let author: EntityId = t.alpha.get::<MessageView>(said).await?.author()?.into();
    assert_eq!(ops::handle_of(&t.alpha, author).await?, "dnorman in beta");
    assert_eq!(ops::handle_of(&t.alpha, t.daniel).await?, "dnorman", "alpha's own principal by its handle alone");
    assert_eq!(
        ops::handle_of(&t.alpha, EntityId::random()).await?.split_once(' ').map(|(_, rest)| rest),
        Some("(cannot be followed here)")
    );

    let held = task(&t.alpha, t.daniel, "Held in beta").await?;
    ops::assign(&t.alpha, t.daniel, held, Some(t.alice)).await?;
    let trx = t.alpha.begin();
    trx.edit(&t.alpha.get::<TaskView>(held).await?)?
        .assignee()?
        .set(&Some(ankurah::property::Ref::new(t.their_alice)))?;
    trx.commit().await?;
    let started = ops::start(&t.alpha, t.daniel, held, None).await.expect_err("assigned to beta's implementer");
    assert!(started.to_string().contains("@impl-alice in beta"), "{started}");

    // A task of alpha's worked in beta's scope: a start from alpha's own scope is refused, naming
    // beta's scope with beta.
    let theirs =
        ops::create_scope(&t.beta, t.their_daniel, CreateScope { title: "Replication".into(), ..Default::default() })
            .await?;
    let ours =
        ops::create_scope(&t.alpha, t.daniel, CreateScope { title: "Release".into(), ..Default::default() }).await?;
    let worked = task(&t.alpha, t.daniel, "Worked in beta's scope").await?;
    ops::link(&t.alpha, t.daniel, RelationKind::WorkedIn, worked, theirs, None, false, json!({})).await?;
    assert_eq!(ops::worked_in(&t.alpha, worked).await?, vec![theirs]);
    let refused = ops::start(&t.alpha, t.alice, worked, Some(ours)).await.expect_err("worked in beta's scope");
    assert!(refused.to_string().contains("Replication in beta"), "{refused}");
    Ok(())
}

#[tokio::test]
async fn a_relation_made_in_the_other_project_shows_on_this_projects_task_and_only_its_project_changes_it() -> Result<()>
{
    let t = two().await?;
    let ours = task(&t.alpha, t.daniel, "Cap the retry delay").await?;
    let theirs = task(&t.beta, t.their_daniel, "Retry on disconnect").await?;
    let relation = depends_on(&t.beta, t.their_daniel, theirs, ours).await?;

    let into_ours = ops::relations_to(&t.alpha, ours, None).await?;
    assert_eq!(into_ours.iter().map(ankurah::View::id).collect::<Vec<_>>(), vec![relation]);
    let other = ops::reference_json(&t.alpha, theirs).await?;
    assert_eq!(
        (other["name"].as_str(), other["project"].as_str()),
        (Some("Retry on disconnect in beta"), Some("beta"))
    );
    assert_eq!(ops::describe(&t.beta, ours).await?.map(|(_, name)| name), Some("Cap the retry delay in alpha".into()));
    let gone = ops::reference_json(&t.alpha, EntityId::random()).await?;
    assert_eq!((gone["kind"].as_str(), gone["followed"].as_bool()), (Some("unknown"), Some(false)));

    let refused =
        ops::unlink(&t.alpha, t.daniel, RelationKind::DependsOn, theirs, ours, "no").await.expect_err("beta's");
    assert!(refused.to_string().contains("project `beta`"), "{refused}");
    let refused = ops::decide_relation(&t.alpha, t.daniel, relation, true, None).await.expect_err("beta's");
    assert!(refused.to_string().contains("project `beta`"), "{refused}");
    assert_eq!(ops::unlink(&t.beta, t.their_daniel, RelationKind::DependsOn, theirs, ours, "done").await?, 1);
    Ok(())
}

#[tokio::test]
async fn a_decision_cites_and_is_made_in_the_context_of_another_projects_records() -> Result<()> {
    let t = two().await?;
    let theirs = task(&t.beta, t.their_daniel, "Retry on disconnect").await?;
    let ruling = ops::CreateDecision {
        title: "Cap retries".into(),
        decided_by: Some(t.daniel),
        from: vec![theirs],
        ..Default::default()
    };
    ops::create_decision(&t.alpha, t.daniel, ruling).await?;
    let log = fwd_core::decision_log::entries(&t.alpha).await?;
    let entry = log.iter().find(|e| e.title == "Cap retries").expect("the ruling");
    assert_eq!(entry.decided_in, vec!["task “Retry on disconnect” in beta".to_string()]);
    let their_ruling =
        ops::CreateDecision { title: "Theirs".into(), decided_by: Some(t.their_daniel), ..Default::default() };
    let far = ops::create_decision(&t.beta, t.their_daniel, their_ruling).await?;
    let ours = task(&t.alpha, t.daniel, "Cap the retry delay").await?;
    ops::cite(&t.alpha, t.daniel, ours, far, None).await?;
    assert_eq!(ops::describe(&t.alpha, far).await?.map(|(_, name)| name), Some("D1 (Theirs) in beta".into()));
    Ok(())
}
