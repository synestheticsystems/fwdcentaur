//! A task moved to another project (`task_move`): it takes with it everything that belongs to it
//! and nothing else, every connection it has is found from both ends afterwards, the principals it
//! names stay the ones they were, the ids it and its messages had still find them, a move stopped
//! partway finishes when run again, each refusal writes nothing, and each project's lists and live
//! queries see the task leave and arrive.

use ankurah::changes::{ChangeSet, ItemChange};
use ankurah::policy::DEFAULT_CONTEXT;
use ankurah::signals::Subscribe;
use ankurah::{Context, EntityId, LiveQuery, Node, PermissiveAgent, View};
use ankurah_storage_sqlite::SqliteStorageEngine;
use anyhow::Result;
use fwd_core::model::*;
use fwd_core::ops::{self, CreateDecision, CreateScope, CreateTask, MessageContext};
use fwd_core::project_graph::{InProjectView, ProjectGraph, Whose};
use fwd_core::rules::{NotReadyReason, Readiness, Verb};
use fwd_core::task_move::{self, Brought, Destination, Refused};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// alpha, where the tasks are filed, and beta, where they move, over one store: the person and an
/// implementer in each, alpha's scope `Inbox` and beta's active scope `Commons`.
struct Two {
    node: Context,
    alpha: ProjectGraph,
    beta: ProjectGraph,
    daniel: EntityId,
    alice: EntityId,
    their_daniel: EntityId,
    inbox: EntityId,
    commons: EntityId,
}

async fn two() -> Result<Two> {
    let storage = SqliteStorageEngine::open_in_memory().await?;
    let node = Node::new_durable(Arc::new(storage), PermissiveAgent::new());
    node.system.create().await?;
    let ctx = node.context_async(DEFAULT_CONTEXT).await?;
    let alpha = ProjectGraph::ensure(ctx.clone(), uuid::Uuid::new_v4(), "alpha").await?;
    let beta = ProjectGraph::ensure(ctx.clone(), uuid::Uuid::new_v4(), "beta").await?;
    let daniel = ops::ensure_principal(&alpha, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let alice = ops::add_principal(&alpha, "impl-alice", Some(AgentRole::Implementer), None, None, None, None).await?;
    let their_daniel = ops::ensure_principal(&beta, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let scope = |title: &str| CreateScope { title: title.into(), ..Default::default() };
    let inbox = ops::create_scope(&alpha, daniel, scope("Inbox")).await?;
    let commons = ops::create_scope(&beta, their_daniel, scope("Commons")).await?;
    Ok(Two { node: ctx, alpha, beta, daniel, alice, their_daniel, inbox, commons })
}

async fn task(graph: &ProjectGraph, by: EntityId, title: &str) -> Result<EntityId> {
    ops::create_task(graph, by, CreateTask { title: title.into(), ..Default::default() }).await
}

/// Beta, into its scope `Commons`, as `fwd task move --to beta --to-scope Commons` names it.
async fn to_beta(t: &Two) -> Result<Destination> {
    Destination::named(&t.alpha, "beta", Some("Commons")).await
}

/// A former id of `record`, kept in `graph`'s project, as the renumbering of a store writes one.
async fn renumbered(graph: &ProjectGraph, record: EntityId) -> Result<String> {
    let former = EntityId::random().to_base64();
    let trx = graph.begin();
    trx.create(&FormerId { project: graph.project(), former: former.clone(), record }).await?;
    trx.commit().await?;
    Ok(former)
}

/// The project `record` is in, by name.
async fn project_of<V: InProjectView>(t: &Two, record: EntityId) -> Result<&'static str> {
    let view: V = t.node.get(record).await?;
    Ok(match view.project_id()? {
        p if p == t.alpha.record() => "alpha",
        p if p == t.beta.record() => "beta",
        _ => "another",
    })
}

/// Moves `record` into `graph`'s project with an ordinary write through the node, as a move
/// interrupted after it would have left it.
async fn put<V: InProjectView>(t: &Two, graph: &ProjectGraph, record: EntityId) -> Result<()> {
    let trx = t.node.begin();
    let view: V = t.node.get(record).await?;
    V::name_project(&*trx.edit::<V::Model>(view.entity())?, &graph.project())?;
    trx.commit().await?;
    Ok(())
}

/// What alpha holds of a task filed there with something of every kind that belongs to it, and
/// beside it what stays: a comment that names alice and her reply; the scope `Inbox`; a
/// dependency on a task that stays and a dependent that stays; a citation of alpha's decision;
/// two steps, one a task and one a message about nothing else; a former id of the task and of the
/// comment; and a notification about the task in alice's inbox.
struct Filed {
    task: EntityId,
    comment: EntityId,
    reply: EntityId,
    mention: EntityId,
    prerequisite: EntityId,
    depends_on: EntityId,
    dependent: EntityId,
    depended_on_by: EntityId,
    decision: EntityId,
    cites: EntityId,
    step_task: EntityId,
    step_note: EntityId,
    steps: [EntityId; 2],
    in_inbox: EntityId,
    former_task: String,
    former_comment: String,
    notification: EntityId,
}

async fn filed(t: &Two) -> Result<Filed> {
    let (alpha, daniel, alice) = (&t.alpha, t.daniel, t.alice);
    let prerequisite = task(alpha, daniel, "Pick the store").await?;
    let spec = CreateTask {
        title: "Write the catalog".into(),
        what: "the ontology's catalog".into(),
        priority: Some(1),
        labels: vec!["feature".into()],
        needs_review: true,
        after: vec![prerequisite],
        scope: Some(t.inbox),
        ..Default::default()
    };
    let moving = ops::create_task(alpha, daniel, spec).await?;
    ops::assign(alpha, daniel, moving, Some(alice)).await?;
    let on_it = MessageContext { task_id: Some(moving), ..Default::default() };
    let comment = ops::post_message(alpha, daniel, "@impl-alice, the catalog first", Priority::Next, on_it).await?;
    let answer = MessageContext { reply_to: Some(comment), ..Default::default() };
    let reply = ops::post_message(alpha, alice, "on it", Priority::Next, answer).await?;
    let mention = ops::relations_from(alpha, comment, Some(RelationKind::Mention)).await?[0].id();
    let depends_on = ops::relations_from(alpha, moving, Some(RelationKind::DependsOn)).await?[0].id();
    let spec = CreateTask { title: "Publish the catalog".into(), after: vec![moving], ..Default::default() };
    let dependent = ops::create_task(alpha, daniel, spec).await?;
    let depended_on_by = ops::relations_from(alpha, dependent, Some(RelationKind::DependsOn)).await?[0].id();
    let ruling = CreateDecision { title: "One catalog".into(), decided_by: Some(daniel), ..Default::default() };
    let decision = ops::create_decision(alpha, daniel, ruling).await?;
    let cites = ops::cite(alpha, daniel, moving, decision, None).await?;
    let step_task = task(alpha, daniel, "Name the kinds").await?;
    let step_note =
        ops::post_message(alpha, daniel, "read the old notes", Priority::Later, MessageContext::default()).await?;
    let steps = [
        ops::add_step(alpha, daniel, moving, step_task).await?,
        ops::add_step(alpha, daniel, moving, step_note).await?,
    ];
    let in_inbox = ops::relations_from(alpha, moving, Some(RelationKind::WorkedIn)).await?[0].id();
    let former_task = renumbered(alpha, moving).await?;
    let former_comment = renumbered(alpha, comment).await?;
    let target = json!({"type": "Task", "id": moving.to_base64()});
    let notification =
        ops::create_notification(alpha, alice, NotificationType::TaskAssigned, Priority::Next, target, json!({}))
            .await?;
    Ok(Filed {
        task: moving,
        comment,
        reply,
        mention,
        prerequisite,
        depends_on,
        dependent,
        depended_on_by,
        decision,
        cites,
        step_task,
        step_note,
        steps,
        in_inbox,
        former_task,
        former_comment,
        notification,
    })
}

/// A task moved to beta's scope takes with it, unchanged, itself, its messages, the relations
/// from it and from its messages, its steps and its former ids; what stays (a dependency's target
/// and a dependent, the citation's decision, what each step is, the scope it left, its
/// notification, the principals) stays in alpha. Its scope of alpha is retracted with the move as
/// the reason, it is worked in `Commons`, and one comment by the mover says where it came from.
#[tokio::test]
async fn a_moved_task_takes_what_belongs_to_it_and_leaves_what_does_not() -> Result<()> {
    let t = two().await?;
    let f = filed(&t).await?;
    let before = ops::json_of_task(&t.alpha.get::<TaskView>(f.task).await?, None)?;

    let moved = task_move::move_tasks(&t.alpha, t.daniel, &[f.task], &to_beta(&t).await?, Some("it is beta's work"))
        .await?
        .remove(0);

    assert!(moved.moved && moved.from.name == "alpha", "{moved:?}");
    let brought = Brought { messages: 2, relations: 4, steps: 2, former_ids: 2 };
    assert_eq!(moved.brought, brought, "the comment and its reply; worked_in, depends_on, cites and the mention");
    assert_eq!((moved.taken_off.as_slice(), moved.worked_in.as_slice()), (&[t.inbox][..], &[t.commons][..]));
    assert_eq!(ops::json_of_task(&t.beta.get::<TaskView>(f.task).await?, None)?, before, "every field unchanged");
    for message in [f.comment, f.reply] {
        assert_eq!(project_of::<MessageView>(&t, message).await?, "beta");
    }
    for relation in [f.mention, f.depends_on, f.cites, f.in_inbox, f.steps[0], f.steps[1]] {
        assert_eq!(project_of::<RelationView>(&t, relation).await?, "beta", "relation {relation}");
    }
    for former in t.beta.former_ids_of(&[f.task, f.comment]).await? {
        assert!(t.beta.owns(&former), "the former id {} went with its record", former.former()?);
    }
    for task in [f.prerequisite, f.dependent, f.step_task] {
        assert_eq!(project_of::<TaskView>(&t, task).await?, "alpha", "task {task} stays");
    }
    assert_eq!(project_of::<MessageView>(&t, f.step_note).await?, "alpha", "what a step is stays");
    assert_eq!(project_of::<RelationView>(&t, f.depended_on_by).await?, "alpha", "a relation into it stays");
    assert_eq!(project_of::<DecisionView>(&t, f.decision).await?, "alpha");
    assert_eq!(project_of::<ScopeView>(&t, t.inbox).await?, "alpha");
    assert_eq!(project_of::<NotificationView>(&t, f.notification).await?, "alpha");
    assert_eq!(project_of::<PrincipalView>(&t, t.alice).await?, "alpha");

    let left = t.beta.relation(f.in_inbox).await?;
    assert_eq!(left.status()?, RelationStatus::Retracted);
    assert_eq!(left.closed_reason()?.as_deref(), Some("the task moved from alpha to beta"));
    let comment: MessageView = t.beta.get(moved.comment.expect("the move's comment")).await?;
    assert_eq!(ops::text(comment.content())?, "Moved from alpha to beta: it is beta's work");
    assert_eq!(comment.task()?.map(EntityId::from), Some(f.task));
    assert_eq!(EntityId::from(comment.author()?), t.daniel, "by the mover, alpha's principal");
    let comments = ops::messages_about(&t.beta, "task", f.task).await?;
    assert_eq!(comments.len(), 3, "the move wrote one comment");
    Ok(())
}

/// Every connection of a moved task is found from both of its ends: a dependency on a task that
/// stays, which gates its readiness and is named with alpha; a dependent that stays, which waits on
/// it in beta; and a decision of alpha's it cites, named with alpha.
#[tokio::test]
async fn each_connection_of_a_moved_task_is_found_from_both_ends() -> Result<()> {
    let t = two().await?;
    let prerequisite = task(&t.alpha, t.daniel, "Pick the store").await?;
    let spec = CreateTask { title: "Write the catalog".into(), after: vec![prerequisite], ..Default::default() };
    let moving = ops::create_task(&t.alpha, t.daniel, spec).await?;
    let spec = CreateTask { title: "Publish the catalog".into(), after: vec![moving], ..Default::default() };
    let dependent = ops::create_task(&t.alpha, t.daniel, spec).await?;
    let ruling = CreateDecision { title: "One catalog".into(), decided_by: Some(t.daniel), ..Default::default() };
    let decision = ops::create_decision(&t.alpha, t.daniel, ruling).await?;
    let cites = ops::cite(&t.alpha, t.daniel, moving, decision, None).await?;

    task_move::move_tasks(&t.alpha, t.daniel, &[moving], &to_beta(&t).await?, None).await?;

    let ends = |relations: Vec<RelationView>| {
        relations.iter().map(|r| (r.from_id().unwrap(), r.to_id().unwrap())).collect::<Vec<_>>()
    };
    assert_eq!(
        ends(ops::relations_from(&t.beta, moving, Some(RelationKind::DependsOn)).await?),
        [(moving, prerequisite)]
    );
    assert_eq!(
        ends(ops::relations_to(&t.alpha, prerequisite, Some(RelationKind::DependsOn)).await?),
        [(moving, prerequisite)]
    );
    assert_eq!(ends(ops::relations_to(&t.beta, moving, Some(RelationKind::DependsOn)).await?), [(dependent, moving)]);
    assert_eq!(
        ends(ops::relations_from(&t.alpha, dependent, Some(RelationKind::DependsOn)).await?),
        [(dependent, moving)]
    );
    let blocked = |task, project: &str| {
        Readiness::NotReady(vec![NotReadyReason::BlockedByTaskIn { task, project: project.into() }])
    };
    let readiness =
        |graph: ProjectGraph, task| async move { ops::readiness(&graph, &graph.get::<TaskView>(task).await?).await };
    assert_eq!(readiness(t.beta.clone(), moving).await?, blocked(prerequisite, "alpha"));
    assert_eq!(readiness(t.alpha.clone(), dependent).await?, blocked(moving, "beta"));
    ops::transition(&t.alpha, t.daniel, prerequisite, Verb::Done, None).await?;
    assert_eq!(readiness(t.beta.clone(), moving).await?, Readiness::Ready, "readiness follows the dependency");
    assert!(ops::ready_tasks(&t.beta, None).await?.iter().any(|r| r.id() == moving));

    assert_eq!(ops::relations_to(&t.alpha, decision, Some(RelationKind::Cites)).await?[0].id(), cites);
    assert_eq!(ops::relations_from(&t.beta, moving, Some(RelationKind::Cites)).await?[0].id(), cites);
    let named = ops::describe(&t.beta, decision).await?.map(|(_, name)| name);
    assert_eq!(named.as_deref(), Some("D1 (One catalog) in alpha"));
    let named = ops::describe(&t.alpha, moving).await?.map(|(_, name)| name);
    assert_eq!(named.as_deref(), Some("Write the catalog in beta"));
    Ok(())
}

/// The principals a moved task names stay the ones they were, alpha's, each named with alpha in
/// beta: its assignee, the author of its comment, the maker of its relations and the mover, who
/// wrote the comment that records the move.
#[tokio::test]
async fn the_principals_a_moved_task_names_stay_alphas_and_are_named_with_alpha() -> Result<()> {
    let t = two().await?;
    let f = filed(&t).await?;
    let moved = task_move::move_tasks(&t.alpha, t.daniel, &[f.task], &to_beta(&t).await?, None).await?.remove(0);

    let moving: TaskView = t.beta.get(f.task).await?;
    assert_eq!(ops::assignee_of(&moving)?, Some(t.alice));
    assert_eq!(ops::handle_of(&t.beta, t.alice).await?, "impl-alice in alpha");
    let comment: MessageView = t.beta.get(f.comment).await?;
    assert_eq!(EntityId::from(comment.author()?), t.daniel);
    assert_eq!(EntityId::from(t.beta.relation(f.depends_on).await?.created_by()?), t.daniel);
    let record: MessageView = t.beta.get(moved.comment.expect("the move's comment")).await?;
    assert_eq!(EntityId::from(record.author()?), t.daniel);
    assert_eq!(ops::handle_of(&t.beta, t.daniel).await?, "dnorman in alpha", "not beta's dnorman");
    assert_ne!(t.daniel, t.their_daniel);
    Ok(())
}

/// The ids a moved task and its comment had before they were renumbered find them in beta, typed
/// in beta as in alpha, whole or by their start.
#[tokio::test]
async fn the_former_ids_of_a_moved_task_and_its_messages_find_them_in_the_destination() -> Result<()> {
    let t = two().await?;
    let f = filed(&t).await?;
    task_move::move_tasks(&t.alpha, t.daniel, &[f.task], &to_beta(&t).await?, None).await?;

    for (former, record) in [(&f.former_task, f.task), (&f.former_comment, f.comment)] {
        let here = ops::locate(&t.beta, &former[..8], ops::ANY).await?;
        assert_eq!((here.id, here.project), (record, Whose::This), "{former}");
        let from_alpha = ops::locate(&t.alpha, former, ops::ANY).await?;
        assert_eq!(from_alpha.id, record);
        assert!(matches!(from_alpha.project, Whose::Other(p) if p.name == "beta"), "{former}");
    }
    Ok(())
}

/// A move that stopped after some of what belongs to the task reached beta, the task not yet,
/// finishes when run again: the rest is taken in, the task moves, and one comment records it.
#[tokio::test]
async fn a_move_stopped_before_the_task_moved_finishes_when_run_again() -> Result<()> {
    let t = two().await?;
    let f = filed(&t).await?;
    put::<MessageView>(&t, &t.beta, f.comment).await?;
    put::<RelationView>(&t, &t.beta, f.depends_on).await?;
    put::<RelationView>(&t, &t.beta, f.steps[0]).await?;

    let moved = task_move::move_tasks(&t.alpha, t.daniel, &[f.task], &to_beta(&t).await?, None).await?.remove(0);

    assert!(moved.moved);
    assert_eq!(moved.brought, Brought { messages: 1, relations: 3, steps: 1, former_ids: 2 }, "what was left");
    assert_eq!(project_of::<TaskView>(&t, f.task).await?, "beta");
    for message in [f.comment, f.reply] {
        assert_eq!(project_of::<MessageView>(&t, message).await?, "beta");
    }
    for relation in [f.mention, f.depends_on, f.cites, f.in_inbox, f.steps[0], f.steps[1]] {
        assert_eq!(project_of::<RelationView>(&t, relation).await?, "beta", "relation {relation}");
    }
    assert_eq!(ops::messages_about(&t.beta, "task", f.task).await?.len(), 3, "one comment records the move");
    Ok(())
}

/// A task in beta already with a record of it left in alpha, as a comment a client that had not
/// heard of the move wrote there, is reported as there already when the move runs again, from beta
/// where the task is: the record is taken in, and no second comment is written. Run once more, it
/// takes nothing.
#[tokio::test]
async fn a_task_there_already_with_a_record_left_behind_takes_it_in_and_is_reported_as_there() -> Result<()> {
    let t = two().await?;
    let f = filed(&t).await?;
    let to = to_beta(&t).await?;
    task_move::move_tasks(&t.alpha, t.daniel, &[f.task], &to, None).await?;
    put::<MessageView>(&t, &t.alpha, f.reply).await?;
    assert_eq!(task_move::place(&t.alpha, &[f.task], &to).await?, Whose::Other(to.project.clone()));

    let again = task_move::move_tasks(&t.beta, t.their_daniel, &[f.task], &to, None).await?.remove(0);
    assert!(!again.moved && again.comment.is_none(), "{again:?}");
    assert_eq!(again.from.name, "beta");
    assert_eq!(again.brought, Brought { messages: 1, ..Default::default() });
    assert_eq!(project_of::<MessageView>(&t, f.reply).await?, "beta");
    assert_eq!(ops::messages_about(&t.beta, "task", f.task).await?.len(), 3, "no second comment");

    let once_more = task_move::move_tasks(&t.beta, t.their_daniel, &[f.task], &to, None).await?.remove(0);
    assert_eq!((once_more.moved, once_more.brought), (false, Brought::default()));
    Ok(())
}

/// Every record of every model a project holds, by id, for checking that a refusal wrote nothing.
async fn everything(graph: &ProjectGraph) -> Result<Vec<EntityId>> {
    let mut all: Vec<EntityId> = Vec::new();
    all.extend(graph.all::<TaskView>().await?.iter().map(View::id));
    all.extend(graph.all::<RelationView>().await?.iter().map(View::id));
    all.extend(graph.all::<MessageView>().await?.iter().map(View::id));
    all.extend(graph.all::<FormerIdView>().await?.iter().map(View::id));
    all.extend(graph.all::<ScopeView>().await?.iter().map(View::id));
    all.sort();
    Ok(all)
}

/// Each refusal is typed and says what to do, and writes nothing: an id that names no task, tasks
/// of two projects, a project the node does not hold, a scope of another project than the
/// destination, and no scope for a destination that has an active one. A destination with no scope
/// takes a task into none.
#[tokio::test]
async fn each_refusal_is_typed_and_writes_nothing_and_a_destination_with_no_scope_takes_a_task_into_none() -> Result<()>
{
    let t = two().await?;
    let gamma = ProjectGraph::ensure(t.node.clone(), uuid::Uuid::new_v4(), "gamma").await?;
    let theirs =
        task(&gamma, ops::ensure_principal(&gamma, "dnorman", "Daniel", PrincipalKind::Human, None).await?, "Gamma's")
            .await?;
    let ours = task(&t.alpha, t.daniel, "Alpha's").await?;
    let before = (everything(&t.alpha).await?, everything(&t.beta).await?, everything(&gamma).await?);
    let refused = |e: anyhow::Error| e.downcast::<Refused>().expect("a typed refusal");
    let to = to_beta(&t).await?;

    let nothing = EntityId::random();
    let e = refused(task_move::move_tasks(&t.alpha, t.daniel, &[ours, nothing], &to, None).await.unwrap_err());
    assert_eq!(e, Refused::NoTask(nothing));
    let e = refused(task_move::move_tasks(&t.alpha, t.daniel, &[ours, theirs], &to, None).await.unwrap_err());
    let named = vec![
        (ours.to_base64()[..8].to_string(), "alpha".to_string()),
        (theirs.to_base64()[..8].to_string(), "gamma".to_string()),
    ];
    assert_eq!(e, Refused::SeveralProjects(named));
    assert!(e.to_string().contains("move each project's tasks with a command of their own"), "{e}");
    let e = refused(Destination::named(&t.alpha, "nowhere", None).await.err().expect("no such project"));
    assert!(matches!(&e, Refused::NoProject { named, held } if named == "nowhere" && held.len() == 3), "{e}");
    let elsewhere = t.inbox.to_base64();
    let e = refused(Destination::named(&t.alpha, "beta", Some(&elsewhere)).await.err().expect("alpha's scope"));
    assert!(
        matches!(&e, Refused::ScopeElsewhere { project, destination, .. } if project == "alpha" && destination == "beta"),
        "{e}"
    );
    let e = refused(Destination::named(&t.alpha, "beta", None).await.err().expect("beta has a scope"));
    assert_eq!(
        e,
        Refused::ScopeNeeded {
            destination: "beta".into(),
            active: vec![(t.commons.to_base64()[..8].to_string(), "Commons".into())]
        }
    );
    assert!(e.to_string().contains("--to-scope"), "{e}");
    assert_eq!((everything(&t.alpha).await?, everything(&t.beta).await?, everything(&gamma).await?), before);

    let to_gamma = Destination::named(&t.alpha, "GAMMA", None).await?;
    let moved = task_move::move_tasks(&t.alpha, t.daniel, &[ours], &to_gamma, None).await?.remove(0);
    assert!(moved.moved && moved.worked_in.is_empty(), "{moved:?}");
    assert_eq!(project_of::<TaskView>(&t, ours).await?, "another");
    Ok(())
}

/// What a live query names, as its change sets say: each record added to it, and each removed.
struct Heard {
    arrived: mpsc::UnboundedReceiver<(EntityId, bool)>,
    _query: Box<dyn std::any::Any + Send>,
    _guard: ankurah::signals::SubscriptionGuard,
}

impl Heard {
    fn on<V: View + Clone + Send + Sync + 'static>(query: LiveQuery<V>) -> Heard {
        let (tx, arrived) = mpsc::unbounded_channel();
        let _guard = query.subscribe(move |changes: ChangeSet<V>| {
            for change in &changes.changes {
                match change {
                    ItemChange::Add { item, .. } => {
                        let _ = tx.send((item.id(), true));
                    }
                    ItemChange::Remove { item, .. } => {
                        let _ = tx.send((item.id(), false));
                    }
                    ItemChange::Initial { .. } | ItemChange::Update { .. } => {}
                }
            }
        });
        Heard { arrived, _query: Box::new(query), _guard }
    }

    /// Returns once the query has named `id` as added (`added`) or removed; fails when a minute
    /// passes first, which only detects a change that never comes.
    async fn until(&mut self, id: EntityId, added: bool) {
        let heard = async { while self.arrived.recv().await != Some((id, added)) {} };
        tokio::time::timeout(Duration::from_secs(60), heard).await.unwrap_or_else(|_| {
            panic!("the live query never named {id} as {}", if added { "added" } else { "removed" })
        });
    }
}

/// A moved task leaves alpha's list of tasks and joins beta's, and a live query of each project's
/// tasks, opened before the move, sees it leave and arrive.
#[tokio::test]
async fn a_moved_task_leaves_the_sources_list_and_live_query_and_joins_the_destinations() -> Result<()> {
    let t = two().await?;
    let moving = task(&t.alpha, t.daniel, "Write the catalog").await?;
    let (mine, theirs) = (t.alpha.query_all::<TaskView>()?, t.beta.query_all::<TaskView>()?);
    mine.wait_initialized().await?;
    theirs.wait_initialized().await?;
    let (mut leaving, mut arriving) = (Heard::on(mine), Heard::on(theirs));

    task_move::move_tasks(&t.alpha, t.daniel, &[moving], &to_beta(&t).await?, None).await?;

    leaving.until(moving, false).await;
    arriving.until(moving, true).await;
    assert!(t.alpha.all::<TaskView>().await?.iter().all(|v| v.id() != moving));
    assert!(t.beta.all::<TaskView>().await?.iter().any(|v| v.id() == moving));
    assert!(t.alpha.get::<TaskView>(moving).await.is_err() && t.beta.get::<TaskView>(moving).await.is_ok());
    Ok(())
}

/// A task worked in the destination's scope already, by a relation alpha made, keeps that one: the
/// move makes no second.
#[tokio::test]
async fn a_task_worked_in_the_destinations_scope_already_is_not_given_it_twice() -> Result<()> {
    let t = two().await?;
    let moving = task(&t.alpha, t.daniel, "Write the catalog").await?;
    ops::link(&t.alpha, t.daniel, RelationKind::WorkedIn, moving, t.commons, None, false, json!({})).await?;
    let moved = task_move::move_tasks(&t.alpha, t.daniel, &[moving], &to_beta(&t).await?, None).await?.remove(0);
    assert_eq!(moved.worked_in, vec![t.commons]);
    let accepted = ops::relations_from(&t.beta, moving, Some(RelationKind::WorkedIn))
        .await?
        .into_iter()
        .filter(|r| r.status().ok() == Some(RelationStatus::Accepted))
        .count();
    assert_eq!(accepted, 1);
    Ok(())
}
