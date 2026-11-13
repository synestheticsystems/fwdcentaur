//! Each reader of an id a record holds, where the record it names is in another project of the
//! node: the reader follows it there and names that project beside the record (docs/DECISIONS.md,
//! D176), and one the node does not hold is said to be one that cannot be followed here. Readiness
//! weighs a dependency wherever it leads, a cycle through two projects is refused, and a relation is
//! changed in the project that made it, whichever projects its ends are in.

use ankurah::{policy::DEFAULT_CONTEXT, Context, EntityId, Node, PermissiveAgent};
use ankurah_storage_sqlite::SqliteStorageEngine;
use anyhow::Result;
use fwd_core::model::*;
use fwd_core::ops::{self, CreateScope, CreateTask, MessageContext};
use fwd_core::project_graph::{ProjectGraph, ProjectGraphError, Whose};
use fwd_core::rules::{NotReadyReason, Readiness, Verb};
use serde_json::json;
use std::sync::Arc;

/// alpha and beta, each with the person and an implementer of the same handles, over one store,
/// and the node's context, through which a test adds a third project.
struct Two {
    node: Context,
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
    let beta = ProjectGraph::ensure(ctx.clone(), uuid::Uuid::new_v4(), "beta").await?;
    let people = |graph: ProjectGraph| async move {
        let daniel = ops::ensure_principal(&graph, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
        let alice =
            ops::add_principal(&graph, "impl-alice", Some(AgentRole::Implementer), None, None, None, None).await?;
        anyhow::Ok((daniel, alice))
    };
    let (daniel, alice) = people(alpha.clone()).await?;
    let (their_daniel, their_alice) = people(beta.clone()).await?;
    Ok(Two { node: ctx, alpha, beta, daniel, alice, their_daniel, their_alice })
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
async fn a_relation_made_in_the_other_project_shows_on_this_projects_task_and_is_changed_in_the_project_that_made_it(
) -> Result<()> {
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

    // Beta's relation is changed through beta's graph, where a command that changes it runs, and
    // alpha's transaction refuses it.
    let refused =
        ops::unlink(&t.alpha, t.daniel, RelationKind::DependsOn, theirs, ours, "no").await.expect_err("beta's");
    assert!(of_another_project(&refused, relation), "{refused}");
    assert_eq!(ops::unlink(&t.beta, t.their_daniel, RelationKind::DependsOn, theirs, ours, "done").await?, 1);
    let proposed =
        ops::link(&t.beta, t.their_daniel, RelationKind::RelatesTo, theirs, ours, None, true, json!({})).await?;
    let refused = ops::decide_relation(&t.alpha, t.daniel, proposed, true, None).await.expect_err("beta's");
    assert!(of_another_project(&refused, proposed), "{refused}");
    ops::decide_relation(&t.beta, t.their_daniel, proposed, true, None).await?;
    assert_eq!(t.alpha.relation(proposed).await?.status()?, RelationStatus::Accepted);
    Ok(())
}

/// Whether `refused` is a transaction's refusal to change `record`, a record of another project.
fn of_another_project(refused: &anyhow::Error, record: EntityId) -> bool {
    matches!(refused.downcast_ref(), Some(ProjectGraphError::OfAnotherProject { record: r, .. }) if *r == record)
}

/// Alpha's task made a step of beta's with `link` run in alpha is a relation of alpha's: it is on
/// beta's task's checklist, and it is checked and unchecked through alpha's graph, where a command
/// that checks it runs from either project, by the principal who checks it there; beta's graph
/// changes none of it.
#[tokio::test]
async fn a_step_another_project_made_is_on_the_tasks_checklist_and_is_checked_in_the_project_that_made_it() -> Result<()>
{
    let t = two().await?;
    let ours = task(&t.alpha, t.daniel, "Cap the retry delay").await?;
    let theirs = task(&t.beta, t.their_daniel, "Retry on disconnect").await?;
    let step = ops::link(&t.alpha, t.daniel, RelationKind::StepOf, ours, theirs, None, false, json!({})).await?;
    let checklist = ops::steps(&t.beta, theirs).await?;
    assert_eq!(checklist.iter().map(ankurah::View::id).collect::<Vec<_>>(), vec![step]);

    let refused = ops::check_step(&t.beta, t.their_daniel, step, true).await.expect_err("alpha's relation");
    assert!(of_another_project(&refused, step), "{refused}");
    ops::check_step(&t.alpha, t.alice, step, true).await?;
    let checked = t.beta.relation(step).await?;
    assert_eq!(checked.status()?, RelationStatus::Resolved);
    assert_eq!(checked.closed_by()?.map(EntityId::from), Some(t.alice), "checked by alpha's implementer");
    ops::check_step(&t.alpha, t.daniel, step, false).await?;
    assert_eq!(t.beta.relation(step).await?.status()?, RelationStatus::Accepted);
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

/// Records, through `graph`, that `record` once had the id `former`, as the move that renumbers
/// records writes it.
async fn renumbered(graph: &ProjectGraph, former: &str, record: EntityId) -> Result<()> {
    let trx = graph.begin();
    trx.create(&FormerId { project: graph.project(), former: former.into(), record }).await?;
    trx.commit().await?;
    Ok(())
}

/// An id that starts with `start`, as a former id people cited might.
fn starting(start: &str) -> String {
    format!("{start}{}", &EntityId::random().to_base64()[start.len()..])
}

/// A start of an id that begins records of two projects, neither of them the command's, by a
/// record's id in one and a former id in the other, is refused as a start two records of one
/// project share is, each named with its project; more of the id names one.
#[tokio::test]
async fn a_start_two_other_projects_records_share_by_a_former_id_is_refused_naming_each_project() -> Result<()> {
    let t = two().await?;
    let gamma = ProjectGraph::ensure(t.node.clone(), uuid::Uuid::new_v4(), "gamma").await?;
    let person = ops::ensure_principal(&gamma, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let in_beta = task(&t.beta, t.their_daniel, "Retry on disconnect").await?;
    let in_gamma = task(&gamma, person, "Cap the retry delay").await?;
    let shared = in_beta.to_base64()[..8].to_string();
    let cited = starting(&shared);
    renumbered(&gamma, &cited, in_gamma).await?;

    let refused = ops::locate(&t.alpha, &shared, ops::ANY).await.expect_err("two projects' tasks start so");
    let said = refused.to_string();
    assert!(said.starts_with(&format!("`{shared}` starts the ids of 2 entities")), "{said}");
    assert!(said.contains(&format!("task {} in beta", &in_beta.to_base64()[..12])), "{said}");
    assert!(said.contains(&format!("(formerly {}) in gamma", &cited[..12])), "{said}");
    let found = ops::locate(&t.alpha, &cited[..12], ops::ANY).await?;
    assert_eq!(found.id, in_gamma, "more of the former id names gamma's task");
    assert!(matches!(found.project, Whose::Other(p) if p.name == "gamma"));
    Ok(())
}

/// A start of an id names a record of the command's project when one starts so, and a record of
/// another project only when none does: a start alpha's task and beta's share names alpha's, typed
/// in alpha, and beta's, typed in beta; a start two of alpha's tasks share is refused in alpha
/// naming those two alone, though beta's starts so too.
#[tokio::test]
async fn a_start_of_an_id_names_a_record_of_the_commands_project_before_one_of_another() -> Result<()> {
    let t = two().await?;
    let ours = task(&t.alpha, t.daniel, "Retry on disconnect").await?;
    let theirs = task(&t.beta, t.their_daniel, "Retry on disconnect").await?;
    let shared = ours.to_base64()[..8].to_string();
    renumbered(&t.beta, &starting(&shared), theirs).await?;
    assert_eq!(ops::locate(&t.alpha, &shared, ops::ANY).await?, ops::Located { id: ours, project: Whose::This });
    assert_eq!(ops::locate(&t.beta, &shared, ops::ANY).await?, ops::Located { id: theirs, project: Whose::This });

    let second = task(&t.alpha, t.daniel, "Cap the retry delay").await?;
    let cited = starting(&shared);
    renumbered(&t.alpha, &cited, second).await?;
    let refused =
        ops::locate(&t.alpha, &shared, ops::ANY).await.expect_err("two of alpha's tasks start so").to_string();
    assert!(refused.starts_with(&format!("`{shared}` starts the ids of 2 entities")), "{refused}");
    assert!(refused.contains(&format!("task {}", &ours.to_base64()[..12])), "{refused}");
    assert!(refused.contains(&format!("(formerly {})", &cited[..12])), "{refused}");
    assert!(!refused.contains(" in beta"), "beta's is named only when alpha holds none: {refused}");
    Ok(())
}

/// A scope of beta's is named in alpha by its whole id, its start and a former id, as any id typed
/// is, while a title names a scope of alpha's alone, a title being its project's own; a ruling of
/// alpha's governs beta's scope once it is attached to it.
#[tokio::test]
async fn a_scope_of_another_project_is_named_by_its_id_and_a_title_names_this_projects_scope_alone() -> Result<()> {
    let t = two().await?;
    let scope = |title: &str| CreateScope { title: title.into(), ..Default::default() };
    let theirs = ops::create_scope(&t.beta, t.their_daniel, scope("Replication")).await?;
    let cited = EntityId::random().to_base64();
    renumbered(&t.beta, &cited, theirs).await?;
    for named in [theirs.to_base64(), theirs.to_base64()[..8].to_string(), cited.clone(), cited[..8].to_string()] {
        assert_eq!(ops::resolve_scope(&t.alpha, &named).await?, theirs, "`{named}` names beta's scope");
    }
    let refused = ops::resolve_scope(&t.alpha, "replication").await.expect_err("beta's title").to_string();
    assert_eq!(refused, "no scope has the title `replication` or an id starting with it");
    let ours = ops::create_scope(&t.alpha, t.daniel, scope("Replication")).await?;
    assert_eq!(ops::resolve_scope(&t.alpha, "replication").await?, ours);

    let ruling = ops::CreateDecision { title: "Cap retries".into(), decided_by: Some(t.daniel), ..Default::default() };
    let decision = ops::create_decision(&t.alpha, t.daniel, ruling).await?;
    ops::attach_decision(&t.alpha, t.daniel, decision, theirs).await?;
    assert_eq!(ops::governs(&t.alpha.get::<DecisionView>(decision).await?)?, Some(theirs));
    Ok(())
}

/// A highlight on the whole of a file of `graph`'s project, made by `by`, in a repository of its
/// own.
async fn whole_file(graph: &ProjectGraph, by: EntityId) -> Result<EntityId> {
    let trx = graph.begin();
    let repo = trx
        .create(&Repo {
            project: graph.project(),
            name: "fwdcentaur".into(),
            remote_urls: ankurah::property::Json::new(json!(["git@github.com:synestheticsystems/fwdcentaur.git"])),
            default_branch: "main".into(),
        })
        .await?
        .id();
    let anchor = trx
        .create(&CodeAnchor {
            project: graph.project(),
            repo: ankurah::property::Ref::new(repo),
            path: "src/net/ws.rs".into(),
            anchor_key: Some("fwd-anchor:1:00".into()),
            span_key: None,
            basis_blob: None,
            head_blob: None,
            index_blob: None,
            nearest_blob: None,
            basis_patch: None,
            blob_sha: None,
            commit_sha: None,
            side: DiffSide::Worktree,
            start_line: 0,
            end_line: 0,
            whole_file: Some(true),
            anchored_text: String::new(),
            context_before: String::new(),
            context_after: String::new(),
            pieces: ankurah::property::Json::new(json!([])),
            symbol_path: None,
            created_by: ankurah::property::Ref::new(by),
            created: fwd_core::now_ms(),
        })
        .await?
        .id();
    trx.commit().await?;
    Ok(anchor)
}

/// A whole-file highlight of beta's, named at a version in a text written in alpha by the start of
/// its id, is the highlight the text mentions, as any id written in a text names a record of any
/// project, and a reader names it with beta.
#[tokio::test]
async fn a_highlight_of_another_project_named_at_a_version_in_a_text_is_mentioned_and_named_with_its_project(
) -> Result<()> {
    let t = two().await?;
    let highlight = whole_file(&t.beta, t.their_daniel).await?;
    let text = format!("see [[{}@worktree]]", &highlight.to_base64()[..8]);
    let posted = ops::post_message(&t.alpha, t.daniel, &text, Priority::Next, MessageContext::default()).await?;
    let stored = ops::text(t.alpha.get::<MessageView>(posted).await?.content())?;
    assert_eq!(stored, format!("see [[{}@worktree]]", highlight.to_base64()));
    assert_eq!(ops::render_references(&t.alpha, &stored).await?, "see [src/net/ws.rs on disk in beta]");
    let mentioned = ops::relations_from(&t.alpha, posted, Some(RelationKind::Mention)).await?;
    assert_eq!(mentioned.iter().map(|r| r.to_id()).collect::<Result<Vec<_>, _>>()?, vec![highlight]);
    Ok(())
}
