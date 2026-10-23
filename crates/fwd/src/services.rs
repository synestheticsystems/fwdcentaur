//! Services the daemon runs as live-query reactors over the durable node. For now: routing,
//! which turns new messages, tasks, status changes, new assignees, relations, decisions,
//! decision moves and a change of what a triage agent serves into notifications
//! (docs/design/agents-and-inbox.md). Views are live, so the caches of statuses, assignees and
//! what each triage agent serves store values, and what a change set travels with its event,
//! read when the change arrived. A fresh entity is reported by the change set as both `added` and
//! `appeared`, so only `added` is consumed, and a bounded set of handled ids guards against
//! duplicates. What one author does on one task grows the notification its recipient already
//! holds unread (rule 10), which routing finds in the graph each time rather than in a cache: a
//! step and its comment join the same one whichever arrives first, after a restart as before it,
//! and an item routed twice is held once (`routing::apply`).

use ankurah::changes::ChangeSet;
use ankurah::policy::DEFAULT_CONTEXT;
use ankurah::signals::Subscribe;
use ankurah::{Context, EntityId, LiveQuery, Node, PermissiveAgent};
use ankurah_storage_sqlite::SqliteStorageEngine;
use anyhow::{anyhow, Result};
use fwd_core::model::*;
use fwd_core::{ops, routing};
use serde_json::Value;
use std::collections::HashMap;
use tokio::sync::mpsc;
use tracing::{info, warn};

/// What the subscriptions hand the worker. A task's or a decision's status, and a task's assignee,
/// are read in the subscription's callback, when the change arrives, and travel with the event
/// (`None` when they could not be read): the views are live, so by the time the worker reaches an
/// event its view may already show a later change. Read then, a creation and a move made right
/// after it would show the same status twice, and no one would be told of the move.
enum Event {
    Message(MessageView),
    TaskAdded(TaskView, Option<TaskStatus>, Assigned),
    TaskUpdated(TaskView, Option<TaskStatus>, Assigned),
    Relation(RelationView),
    DecisionAdded(DecisionView, Option<DecisionStatus>),
    DecisionUpdated(DecisionView, Option<DecisionStatus>),
    /// A principal made or changed, with what it served as a triage agent as the change arrived:
    /// none while it holds no triage or is retired (`ops::triage_standing`).
    Principal(PrincipalView, Option<ops::Serving>),
}

/// A task's assignee as a change carried it: the assignee (`None` when it could not be read), and
/// who set it, as the task's context records (`ops::ASSIGNED_BY_KEY`).
#[derive(Debug, Clone, Default)]
struct Assigned {
    assignee: Option<Option<EntityId>>,
    by: Option<String>,
}

impl Assigned {
    fn of(t: &TaskView) -> Assigned {
        let context: Option<Value> = t.context().ok().map(Into::into);
        Assigned {
            assignee: ops::assignee_of(t).ok(),
            by: context.as_ref().and_then(|c| c.get(ops::ASSIGNED_BY_KEY)).and_then(Value::as_str).map(str::to_string),
        }
    }
}

/// Keeps the subscriptions and the worker alive; drop to stop routing.
pub struct Routing {
    _messages: LiveQuery<MessageView>,
    _tasks: LiveQuery<TaskView>,
    _relations: LiveQuery<RelationView>,
    _decisions: LiveQuery<DecisionView>,
    _principals: LiveQuery<PrincipalView>,
    _guards: Vec<ankurah::signals::SubscriptionGuard>,
    _worker: tokio::task::JoinHandle<()>,
}

pub async fn start(node: Node<SqliteStorageEngine, PermissiveAgent>) -> Result<Routing> {
    let ctx = node.context_async(DEFAULT_CONTEXT).await?;
    let (tx, rx) = mpsc::unbounded_channel::<Event>();

    let statuses = Statuses::seed(&ctx).await?;

    let messages = ctx.query::<MessageView>("created > 0")?;
    let tasks = ctx.query::<TaskView>("created > 0")?;
    let relations = ctx.query::<RelationView>("created > 0")?;
    let decisions = ctx.query::<DecisionView>("created > 0")?;
    // A principal has no `created`: every principal is a person or an agent (`ops::principals`).
    let principals = ctx.query::<PrincipalView>("kind = 'human' OR kind = 'agent'")?;
    messages.wait_initialized().await?;
    tasks.wait_initialized().await?;
    relations.wait_initialized().await?;
    decisions.wait_initialized().await?;
    principals.wait_initialized().await?;

    let mut guards = Vec::new();
    {
        let tx = tx.clone();
        guards.push(messages.subscribe(move |cs: ChangeSet<MessageView>| {
            tracing::debug!(
                initial = cs.initial().len(),
                added = cs.added().len(),
                appeared = cs.appeared().len(),
                updated = cs.updated().len(),
                "message changeset"
            );
            for m in cs.added() {
                let _ = tx.send(Event::Message(m));
            }
        }));
    }
    {
        let tx = tx.clone();
        guards.push(tasks.subscribe(move |cs: ChangeSet<TaskView>| {
            tracing::debug!(
                initial = cs.initial().len(),
                added = cs.added().len(),
                appeared = cs.appeared().len(),
                updated = cs.updated().len(),
                "task changeset"
            );
            for t in cs.added() {
                let (status, assigned) = (t.status().ok(), Assigned::of(&t));
                let _ = tx.send(Event::TaskAdded(t, status, assigned));
            }
            for t in cs.updated() {
                let (status, assigned) = (t.status().ok(), Assigned::of(&t));
                let _ = tx.send(Event::TaskUpdated(t, status, assigned));
            }
        }));
    }
    {
        let tx = tx.clone();
        guards.push(relations.subscribe(move |cs: ChangeSet<RelationView>| {
            for r in cs.added() {
                let _ = tx.send(Event::Relation(r));
            }
        }));
    }
    {
        let tx = tx.clone();
        guards.push(decisions.subscribe(move |cs: ChangeSet<DecisionView>| {
            for d in cs.added() {
                let status = d.status().ok();
                let _ = tx.send(Event::DecisionAdded(d, status));
            }
            for d in cs.updated() {
                let status = d.status().ok();
                let _ = tx.send(Event::DecisionUpdated(d, status));
            }
        }));
    }
    {
        let tx = tx.clone();
        guards.push(principals.subscribe(move |cs: ChangeSet<PrincipalView>| {
            for p in cs.added().into_iter().chain(cs.updated()) {
                let standing = ops::triage_standing(&p);
                let _ = tx.send(Event::Principal(p, standing));
            }
        }));
    }

    let worker = tokio::spawn(worker(ctx, rx, statuses));
    info!("routing service started");
    Ok(Routing {
        _messages: messages,
        _tasks: tasks,
        _relations: relations,
        _decisions: decisions,
        _principals: principals,
        _guards: guards,
        _worker: worker,
    })
}

/// The last status routing saw for each task and decision, the last assignee for each task, and
/// what each principal served as a triage agent, so a change is told from a repeat. Views are
/// live, so values are stored.
#[derive(Default)]
struct Statuses {
    tasks: HashMap<EntityId, TaskStatus>,
    decisions: HashMap<EntityId, DecisionStatus>,
    assignees: HashMap<EntityId, Option<EntityId>>,
    triage: HashMap<EntityId, Option<ops::Serving>>,
}

impl Statuses {
    /// What routing remembers at its start, read from the graph, so that the first change of an
    /// existing task, decision or principal has a "previous", and what exists then produces
    /// nothing: the assignments, and what each triage agent serves, whose waiting questions reach
    /// it as it takes a scope or the whole project, never at a start.
    async fn seed(ctx: &Context) -> Result<Statuses> {
        let mut statuses = Statuses::default();
        for t in ctx.fetch::<TaskView>("created > 0").await? {
            if let Ok(s) = t.status() {
                statuses.tasks.insert(t.id(), s);
            }
            if let Ok(a) = ops::assignee_of(&t) {
                statuses.assignees.insert(t.id(), a);
            }
        }
        for d in ctx.fetch::<DecisionView>("created > 0").await? {
            if let Ok(s) = d.status() {
                statuses.decisions.insert(d.id(), s);
            }
        }
        for p in ops::principals(ctx).await? {
            statuses.triage.insert(p.id(), ops::triage_standing(&p));
        }
        Ok(statuses)
    }
}

async fn worker(ctx: Context, mut rx: mpsc::UnboundedReceiver<Event>, mut statuses: Statuses) {
    let mut handled: std::collections::VecDeque<EntityId> = std::collections::VecDeque::new();
    while let Some(event) = rx.recv().await {
        // Creation events are routed once per entity, whatever the change set reports.
        let key = match &event {
            Event::Message(m) => Some(m.id()),
            Event::TaskAdded(t, ..) => Some(t.id()),
            Event::Relation(r) => Some(r.id()),
            Event::DecisionAdded(d, _) => Some(d.id()),
            Event::TaskUpdated(..) | Event::DecisionUpdated(..) | Event::Principal(..) => None,
        };
        if let Some(id) = key {
            if handled.contains(&id) {
                continue;
            }
            handled.push_back(id);
            if handled.len() > 4096 {
                handled.pop_front();
            }
        }
        if let Err(e) = handle(&ctx, event, &mut statuses).await {
            warn!(error = %e, "routing failed for an event");
        }
    }
}

/// The status an event carries, or why there is none.
fn carried<S>(status: Option<S>) -> Result<S> {
    status.ok_or_else(|| anyhow!("its status could not be read when the change arrived"))
}

/// The notification a task's change owes its assignee (rule 7): TaskAssigned when the assignee
/// the change carried differs from the last one routing saw for the task (none for a task it has
/// not seen), and never to whoever made the change. While the view already shows a later
/// assignee the change is left to the event that carries that one, as for statuses. An assignee
/// carried over by `fwd admin migrate` from an `assigned_to` relation is no news: its assignee
/// heard of the assignment when the relation was made.
async fn assignment(
    ctx: &Context,
    statuses: &mut Statuses,
    t: &TaskView,
    carried: Assigned,
) -> Result<Vec<routing::Planned>> {
    let Some(current) = carried.assignee else { return Ok(vec![]) };
    let previous = statuses.assignees.insert(t.id(), current).flatten();
    if previous == current || ops::assignee_of(t).ok() != Some(current) || carried.by.as_deref() == Some(ops::MIGRATED)
    {
        return Ok(vec![]);
    }
    let actor = carried.by.and_then(|s| s.parse::<EntityId>().ok());
    routing::plan_for_assignee(ctx, t, previous, actor).await
}

async fn handle(ctx: &Context, event: Event, statuses: &mut Statuses) -> Result<()> {
    tracing::debug!(
        event = match &event {
            Event::Message(_) => "message",
            Event::TaskAdded(..) => "task_added",
            Event::TaskUpdated(..) => "task_updated",
            Event::Relation(_) => "relation",
            Event::DecisionAdded(..) => "decision_added",
            Event::DecisionUpdated(..) => "decision_updated",
            Event::Principal(..) => "principal",
        },
        "routing event"
    );
    // A status change is routed from the previous status the cache holds to the one the event
    // carries. The plans read the view, though, so while the view already shows a later status
    // the change is left to the event that carries that one: routing it here as well would tell
    // the later change twice, the first time as this one.
    let planned = match event {
        Event::Message(m) => routing::plan_for_message(ctx, &m).await?,
        Event::TaskAdded(t, status, assigned) => {
            statuses.tasks.insert(t.id(), carried(status)?);
            let mut planned = routing::plan_for_task_created(ctx, &t).await?;
            planned.extend(assignment(ctx, statuses, &t, assigned).await?);
            planned
        }
        Event::TaskUpdated(t, status, assigned) => {
            let current = carried(status)?;
            let mut planned = match statuses.tasks.insert(t.id(), current) {
                Some(previous) if previous != current && t.status().ok() == Some(current) => {
                    let context: Value = t.context()?.into();
                    let actor_key = if current == TaskStatus::InReview { "submitted_by" } else { "accepted_by" };
                    let actor = context.get(actor_key).and_then(Value::as_str).and_then(|s| s.parse::<EntityId>().ok());
                    routing::plan_for_task_status(ctx, &t, previous, actor).await?
                }
                _ => vec![],
            };
            planned.extend(assignment(ctx, statuses, &t, assigned).await?);
            planned
        }
        Event::Relation(r) => routing::plan_for_relation(ctx, &r).await?,
        Event::DecisionAdded(d, status) => {
            statuses.decisions.insert(d.id(), carried(status)?);
            routing::plan_for_decision_created(ctx, &d).await?
        }
        Event::DecisionUpdated(d, status) => {
            let current = carried(status)?;
            match statuses.decisions.insert(d.id(), current) {
                // Whoever repealed or superseded it is recorded on it, and is left out.
                Some(previous) if previous != current && d.status().ok() == Some(current) => {
                    let actor = d.closed_by()?.map(EntityId::from);
                    routing::plan_for_decision_status(ctx, &d, previous, actor).await?
                }
                _ => vec![],
            }
        }
        // A change of what a triage agent serves, by an agent naming itself with the role,
        // selecting it, taking a scope or the whole project, coming back with them, succeeding the
        // agent that had them, or giving them up, moves who answers first for some questions: each
        // question asked of no one that no triage agent has heard of reaches the agent that answers
        // first for its scope now (rule 5). A change that leaves what it serves as it was, and the
        // daemon's start, tell no one. The plan reads the graph, so a change undone before the
        // worker reached it tells no one either.
        Event::Principal(p, standing) => {
            let before = statuses.triage.insert(p.id(), standing.clone()).flatten();
            if before != standing {
                routing::plan_for_waiting_questions(ctx).await?
            } else {
                vec![]
            }
        }
    };
    if !planned.is_empty() {
        let ids = routing::apply(ctx, &planned).await?;
        info!(count = ids.len(), "notifications made or grown");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fwd_core::ops::{self, CreateDecision, CreateTask, MessageContext};
    use fwd_core::rules::{DecisionMove, Verb};
    use std::sync::Arc;

    async fn context() -> Result<Context> {
        let storage = SqliteStorageEngine::open_in_memory().await?;
        let node = Node::new_durable(Arc::new(storage), PermissiveAgent::new());
        node.system.create().await?;
        Ok(node.context_async(DEFAULT_CONTEXT).await?)
    }

    /// How many notifications of `kind` `who` has.
    async fn told(ctx: &Context, who: EntityId, kind: NotificationType) -> Result<usize> {
        let inbox = ops::inbox(ctx, who, false).await?;
        Ok(inbox.iter().filter(|n| n.notification_type().ok() == Some(kind)).count())
    }

    /// Feeds the worker's handler the events the subscriptions would have sent, in order.
    async fn route(ctx: &Context, events: Vec<Event>) -> Result<()> {
        let mut statuses = Statuses::default();
        for event in events {
            handle(ctx, event, &mut statuses).await?;
        }
        Ok(())
    }

    /// A decision proposed, accepted and repealed before the worker reached its creation: the
    /// events carry the statuses the changes arrived with, so the repeal reaches the author of
    /// what cites the decision, once.
    #[tokio::test]
    async fn a_decision_moved_before_the_worker_reached_its_creation_is_told_once() -> Result<()> {
        let ctx = context().await?;
        let daniel = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
        let alice =
            ops::ensure_principal(&ctx, "impl-alice", "Alice", PrincipalKind::Agent, Some(AgentRole::Implementer))
                .await?;
        let d = ops::create_decision(
            &ctx,
            daniel,
            CreateDecision { title: "Cap retries at 30s".into(), propose: true, ..Default::default() },
        )
        .await?;
        let t = ops::create_task(&ctx, alice, CreateTask { title: "Cap the retry delay".into(), ..Default::default() })
            .await?;
        ops::cite(&ctx, alice, t, d, None).await?;
        ops::move_decision(&ctx, daniel, d, DecisionMove::Accept, "measured it").await?;
        ops::move_decision(&ctx, daniel, d, DecisionMove::Repeal, "the transport retries now").await?;
        // The view is live: it reads Repealed already, whatever each event carries.
        let view: DecisionView = ctx.get(d).await?;
        route(
            &ctx,
            vec![
                Event::DecisionAdded(view.clone(), Some(DecisionStatus::Proposed)),
                Event::DecisionUpdated(view.clone(), Some(DecisionStatus::Accepted)),
                Event::DecisionUpdated(view, Some(DecisionStatus::Repealed)),
            ],
        )
        .await?;
        assert_eq!(told(&ctx, alice, NotificationType::DecisionRepealed).await?, 1);
        Ok(())
    }

    /// Attaching a decision to a scope, or detaching it, changes which rulings a scope's work lists
    /// and not the ruling, so it tells nobody (D93): not whoever filed it, ruled on it or rests on
    /// it, and not the project's triage agent.
    #[tokio::test]
    async fn attaching_a_decision_to_a_scope_or_detaching_it_tells_nobody() -> Result<()> {
        let ctx = context().await?;
        let daniel = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
        let alice =
            ops::ensure_principal(&ctx, "impl-alice", "Alice", PrincipalKind::Agent, Some(AgentRole::Implementer))
                .await?;
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
        let scope =
            ops::create_scope(&ctx, alice, ops::CreateScope { title: "Retry work".into(), ..Default::default() })
                .await?;
        let ruling =
            CreateDecision { title: "Cap retries at 30s".into(), decided_by: Some(daniel), ..Default::default() };
        let d = ops::create_decision(&ctx, triage, ruling).await?;
        let t = ops::create_task(&ctx, alice, CreateTask { title: "Cap the retry delay".into(), ..Default::default() })
            .await?;
        ops::cite(&ctx, alice, t, d, None).await?;
        let view: DecisionView = ctx.get(d).await?;
        let mut statuses = Statuses::default();
        handle(&ctx, Event::DecisionAdded(view.clone(), Some(DecisionStatus::Accepted)), &mut statuses).await?;
        ops::attach_decision(&ctx, daniel, d, scope).await?;
        handle(&ctx, Event::DecisionUpdated(view.clone(), Some(DecisionStatus::Accepted)), &mut statuses).await?;
        ops::detach_decision(&ctx, alice, d).await?;
        handle(&ctx, Event::DecisionUpdated(view, Some(DecisionStatus::Accepted)), &mut statuses).await?;
        for who in [daniel, alice, triage] {
            assert!(ops::inbox(&ctx, who, false).await?.is_empty(), "no one hears of it");
        }
        Ok(())
    }

    /// A task claimed and submitted before the worker reached its creation: the submission asks
    /// its creator for a review, once.
    #[tokio::test]
    async fn a_task_moved_before_the_worker_reached_its_creation_is_told_once() -> Result<()> {
        let ctx = context().await?;
        let daniel = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
        let alice =
            ops::ensure_principal(&ctx, "impl-alice", "Alice", PrincipalKind::Agent, Some(AgentRole::Implementer))
                .await?;
        let scope =
            ops::create_scope(&ctx, alice, ops::CreateScope { title: "Retry work".into(), ..Default::default() })
                .await?;
        let t = ops::create_task(
            &ctx,
            daniel,
            CreateTask { title: "Retry on disconnect".into(), needs_review: true, ..Default::default() },
        )
        .await?;
        ops::claim(&ctx, alice, t, scope).await?;
        ops::transition(&ctx, alice, t, Verb::Submit, Some("commit 1, tests green")).await?;
        let view: TaskView = ctx.get(t).await?;
        let unassigned = Assigned { assignee: Some(None), by: None };
        let hers = Assigned { assignee: Some(Some(alice)), by: Some(alice.to_base64()) };
        route(
            &ctx,
            vec![
                Event::TaskAdded(view.clone(), Some(TaskStatus::Open), unassigned),
                Event::TaskUpdated(view.clone(), Some(TaskStatus::InProgress), hers.clone()),
                Event::TaskUpdated(view, Some(TaskStatus::InReview), hers),
            ],
        )
        .await?;
        assert_eq!(told(&ctx, daniel, NotificationType::ReviewRequested).await?, 1);
        assert_eq!(
            told(&ctx, alice, NotificationType::TaskAssigned).await?,
            0,
            "starting it herself is no news to her"
        );
        Ok(())
    }

    /// Rule 7: a task's new assignee is told once, whoever else changes the task after; an
    /// assignment that existed when routing started, one its assignee made, an unassignment, and one
    /// the migration carried over tell no one.
    #[tokio::test]
    async fn a_new_assignee_is_told_once_and_an_old_assignment_never() -> Result<()> {
        let ctx = context().await?;
        let daniel = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
        let alice =
            ops::ensure_principal(&ctx, "impl-alice", "Alice", PrincipalKind::Agent, Some(AgentRole::Implementer))
                .await?;
        let bob =
            ops::ensure_principal(&ctx, "impl-bob", "Bob", PrincipalKind::Agent, Some(AgentRole::Implementer)).await?;
        let new_task = |title: &str| {
            let (ctx, title) = (ctx.clone(), title.to_string());
            async move { ops::create_task(&ctx, daniel, CreateTask { title, ..Default::default() }).await }
        };
        // Assigned before routing started: the snapshot seeds the cache, and a later edit of the
        // task that leaves the assignee alone tells no one.
        let old = new_task("Assigned last week").await?;
        ops::assign(&ctx, daniel, old, Some(alice)).await?;
        let mut statuses = Statuses::default();
        for t in ctx.fetch::<TaskView>("created > 0").await? {
            statuses.tasks.insert(t.id(), t.status()?);
            statuses.assignees.insert(t.id(), ops::assignee_of(&t)?);
        }
        ops::amend(&ctx, old, ops::Amend { title: Some("Assigned last week, retitled".into()), ..Default::default() })
            .await?;
        let view: TaskView = ctx.get(old).await?;
        handle(&ctx, Event::TaskUpdated(view.clone(), Some(TaskStatus::Open), Assigned::of(&view)), &mut statuses)
            .await?;
        assert_eq!(told(&ctx, alice, NotificationType::TaskAssigned).await?, 0);

        // Daniel assigns a new task to Alice: she hears once, however many changes follow.
        let fresh = new_task("Retry on disconnect").await?;
        let view: TaskView = ctx.get(fresh).await?;
        handle(&ctx, Event::TaskAdded(view.clone(), Some(TaskStatus::Open), Assigned::of(&view)), &mut statuses)
            .await?;
        ops::assign(&ctx, daniel, fresh, Some(alice)).await?;
        for _ in 0..2 {
            handle(&ctx, Event::TaskUpdated(view.clone(), Some(TaskStatus::Open), Assigned::of(&view)), &mut statuses)
                .await?;
        }
        assert_eq!(told(&ctx, alice, NotificationType::TaskAssigned).await?, 1);
        let reason = ops::inbox(&ctx, alice, true).await?;
        assert_eq!(serde_json::Value::from(reason[0].originator()?)["why"], "assignee");
        // Reassigned to Bob: Bob hears; Daniel, who did it, does not.
        ops::assign(&ctx, daniel, fresh, Some(bob)).await?;
        handle(&ctx, Event::TaskUpdated(view.clone(), Some(TaskStatus::Open), Assigned::of(&view)), &mut statuses)
            .await?;
        assert_eq!(told(&ctx, bob, NotificationType::TaskAssigned).await?, 1);
        assert_eq!(told(&ctx, daniel, NotificationType::TaskAssigned).await?, 0);
        // Unassigned, as the page's assignee control does before it assigns anyone else: no one
        // hears. Assigned to Alice again: she hears once more.
        ops::assign(&ctx, daniel, fresh, None).await?;
        handle(&ctx, Event::TaskUpdated(view.clone(), Some(TaskStatus::Open), Assigned::of(&view)), &mut statuses)
            .await?;
        for (principal, before) in [(alice, 1), (bob, 1), (daniel, 0)] {
            assert_eq!(told(&ctx, principal, NotificationType::TaskAssigned).await?, before);
        }
        ops::assign(&ctx, daniel, fresh, Some(alice)).await?;
        handle(&ctx, Event::TaskUpdated(view.clone(), Some(TaskStatus::Open), Assigned::of(&view)), &mut statuses)
            .await?;
        assert_eq!(told(&ctx, alice, NotificationType::TaskAssigned).await?, 2);

        // Carried over by the migration: no one hears.
        let migrated = new_task("Claimed before the field existed").await?;
        let view: TaskView = ctx.get(migrated).await?;
        handle(&ctx, Event::TaskAdded(view.clone(), Some(TaskStatus::Open), Assigned::of(&view)), &mut statuses)
            .await?;
        let carried = Assigned { assignee: Some(Some(bob)), by: Some(ops::MIGRATED.into()) };
        let trx = ctx.begin();
        view.edit(&trx)?.assignee()?.set(&Some(ankurah::property::Ref::new(bob)))?;
        trx.commit().await?;
        handle(&ctx, Event::TaskUpdated(view, Some(TaskStatus::Open), carried), &mut statuses).await?;
        assert_eq!(told(&ctx, bob, NotificationType::TaskAssigned).await?, 1, "still the one from the reassignment");
        Ok(())
    }

    /// Rule 10: a step and its first comment reach the task's assignee as one notification,
    /// whichever of their events the worker reaches first, and an event delivered again adds
    /// nothing to it.
    #[tokio::test]
    async fn a_step_and_its_comment_are_one_notification_in_either_order() -> Result<()> {
        let ctx = context().await?;
        let daniel = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
        let alice =
            ops::ensure_principal(&ctx, "impl-alice", "Alice", PrincipalKind::Agent, Some(AgentRole::Implementer))
                .await?;
        let review =
            ops::create_task(&ctx, alice, CreateTask { title: "Review: the retry loop".into(), ..Default::default() })
                .await?;
        ops::assign(&ctx, alice, review, Some(daniel)).await?;
        let highlight = EntityId::random();
        let edge = ops::add_step(&ctx, alice, review, highlight).await?;
        let on = MessageContext { anchor_id: Some(highlight), ..Default::default() };
        let comment = ops::post_message(&ctx, alice, "look here", Priority::Next, on).await?;
        let (step, said): (RelationView, MessageView) = (ctx.get(edge).await?, ctx.get(comment).await?);
        route(
            &ctx,
            vec![
                Event::Message(said.clone()),
                Event::Relation(step.clone()),
                Event::Relation(step),
                Event::Message(said),
            ],
        )
        .await?;
        let inbox = ops::inbox(&ctx, daniel, true).await?;
        assert_eq!(inbox.len(), 1, "{inbox:?}");
        let batch = routing::batch_of(&inbox[0].originator()?.into()).expect("it lists what it holds");
        assert_eq!((batch.task, batch.steps(), batch.comments()), (review, 1, 1));
        Ok(())
    }

    /// `who` asks no one in particular, of no scope, as `fwd ask` without `--to` does where it finds
    /// no scope, a moment after whatever came before, so that the questions are ordered by when
    /// they were asked.
    async fn ask(ctx: &Context, who: EntityId, text: &str) -> Result<EntityId> {
        ask_in(ctx, who, text, None).await
    }

    /// `who` asks no one in particular, of `scope`, as `fwd ask --scope` does, a moment after
    /// whatever came before.
    async fn ask_in(ctx: &Context, who: EntityId, text: &str, scope: Option<EntityId>) -> Result<EntityId> {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let asked = MessageContext { asks_project: true, asks_scope: scope, ..Default::default() };
        ops::post_message(ctx, who, text, Priority::Now, asked).await
    }

    /// The triage agent that answers first for `scope`, or for what concerns no scope when it is
    /// none (`ops::front_line`).
    async fn front(ctx: &Context, scope: Option<EntityId>) -> Result<Option<EntityId>> {
        Ok(ops::triage_for(ctx, scope).await?.map(|f| f.agent))
    }

    /// The event a new message sends.
    async fn message(ctx: &Context, id: EntityId) -> Result<Event> {
        Ok(Event::Message(ctx.get(id).await?))
    }

    /// The event a principal's change sends, read as the change arrives.
    async fn principal(ctx: &Context, id: EntityId) -> Result<Event> {
        let view: PrincipalView = ctx.get(id).await?;
        let standing = ops::triage_standing(&view);
        Ok(Event::Principal(view, standing))
    }

    /// The questions `who` has been asked, read or not, each with why, in the order they were
    /// asked.
    async fn asked(ctx: &Context, who: EntityId) -> Result<Vec<(EntityId, String)>> {
        let mut found = Vec::new();
        for n in ops::inbox(ctx, who, false).await? {
            if n.notification_type()? != NotificationType::QuestionAsked {
                continue;
            }
            let originator: Value = n.originator()?.into();
            let question: EntityId = originator["message_id"].as_str().expect("it names its question").parse()?;
            let when = ctx.get::<MessageView>(question).await?.created()?;
            found.push((when, question, originator["why"].as_str().unwrap_or_default().to_string()));
        }
        found.sort();
        Ok(found.into_iter().map(|(_, question, why)| (question, why)).collect())
    }

    /// `who` reads every notification it has.
    async fn read_all(ctx: &Context, who: EntityId) -> Result<()> {
        let unread: Vec<EntityId> = ops::inbox(ctx, who, true).await?.iter().map(|n| n.id()).collect();
        ops::set_notification_status(ctx, &unread, NotificationStatus::Read).await
    }

    /// Triage per scope (docs/DECISIONS.md, D122): two triage agents in one project, one per scope,
    /// each hear only the questions asked of their own scope; a question asked of a scope no agent
    /// serves, or of no scope, while no agent serves the whole project, reaches no one and waits;
    /// the agent that takes that scope hears of its question then, once, and of no other, and the
    /// agent endorsed for the whole project of the one asked of no scope; an agent that gives its
    /// scope up leaves the scope's next questions to the agent that serves the whole project.
    #[tokio::test]
    async fn each_scopes_questions_reach_its_own_triage_agent_or_wait_for_one() -> Result<()> {
        let ctx = context().await?;
        let daniel = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
        let scope = |title: &str| {
            let (ctx, title) = (ctx.clone(), title.to_string());
            async move { ops::create_scope(&ctx, daniel, ops::CreateScope { title, ..Default::default() }).await }
        };
        let (replication, storage, docs) = (scope("Replication").await?, scope("Storage").await?, scope("Docs").await?);
        let mut statuses = Statuses::seed(&ctx).await?;
        let triage = |question: EntityId| (question, "triage".to_string());
        let a = ops::add_principal(
            &ctx,
            "triage-a",
            Some(AgentRole::Triage),
            Some(ops::Serves::Scope(replication)),
            None,
            None,
            None,
        )
        .await?;
        let b = ops::add_principal(
            &ctx,
            "triage-b",
            Some(AgentRole::Triage),
            Some(ops::Serves::Scope(storage)),
            None,
            None,
            None,
        )
        .await?;
        for p in [a, b] {
            handle(&ctx, principal(&ctx, p).await?, &mut statuses).await?;
        }
        let in_replication = ask_in(&ctx, daniel, "does the relay replicate?", Some(replication)).await?;
        let in_storage = ask_in(&ctx, daniel, "is SQLite enough?", Some(storage)).await?;
        let in_docs = ask_in(&ctx, daniel, "who writes the primer?", Some(docs)).await?;
        let of_no_scope = ask(&ctx, daniel, "who takes the release?").await?;
        for q in [in_replication, in_storage, in_docs, of_no_scope] {
            handle(&ctx, message(&ctx, q).await?, &mut statuses).await?;
        }
        assert_eq!(asked(&ctx, a).await?, vec![triage(in_replication)]);
        assert_eq!(asked(&ctx, b).await?, vec![triage(in_storage)]);
        // triage-b takes Docs as well, and hears of the question that waited there, and no other.
        ops::serve(&ctx, "triage-b", ops::Serves::Scope(docs)).await?;
        handle(&ctx, principal(&ctx, b).await?, &mut statuses).await?;
        assert_eq!(asked(&ctx, b).await?, vec![triage(in_storage), triage(in_docs)]);
        // An agent endorsed for the whole project hears of the question asked of no scope.
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
        handle(&ctx, principal(&ctx, whole).await?, &mut statuses).await?;
        assert_eq!(asked(&ctx, whole).await?, vec![triage(of_no_scope)]);
        // triage-a gives Replication up: the scope's next question reaches the agent that serves
        // the whole project, and triage-a keeps the one it was told of.
        ops::stop_serving(&ctx, "triage-a", ops::Serves::Scope(replication)).await?;
        handle(&ctx, principal(&ctx, a).await?, &mut statuses).await?;
        let later = ask_in(&ctx, daniel, "and the notifications?", Some(replication)).await?;
        handle(&ctx, message(&ctx, later).await?, &mut statuses).await?;
        assert_eq!(asked(&ctx, whole).await?, vec![triage(of_no_scope), triage(later)]);
        assert_eq!(asked(&ctx, a).await?, vec![triage(in_replication)]);
        Ok(())
    }

    /// Rule 5: a question asked of no scope while no agent serves the whole project reaches no one,
    /// and waits; the agent that comes to serve the whole project is told of it then, once, with why
    /// triage, whether it names itself with the role, selects it, or succeeds a triage agent retired
    /// before the question was asked (task 5RoOJZl9). A change that leaves what it serves as it was
    /// tells it nothing more. Of the questions a triage agent retired with no successor was told of,
    /// one it read is not told again, and one it left unread, in an inbox no agent reads now, is.
    #[tokio::test]
    async fn a_question_that_waited_for_triage_reaches_the_agent_that_takes_it_once() -> Result<()> {
        let ctx = context().await?;
        let daniel = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
        let alice =
            ops::add_principal(&ctx, "impl-alice", Some(AgentRole::Implementer), None, None, None, None).await?;
        let mut statuses = Statuses::seed(&ctx).await?;
        let triage = |question: EntityId| (question, "triage".to_string());

        // An agent names itself with the role.
        let first = ask(&ctx, daniel, "who takes the TLS part?").await?;
        handle(&ctx, message(&ctx, first).await?, &mut statuses).await?;
        let bot = ops::add_principal(
            &ctx,
            "triage-1",
            Some(AgentRole::Triage),
            Some(ops::Serves::ProjectWide),
            None,
            None,
            None,
        )
        .await?;
        handle(&ctx, principal(&ctx, bot).await?, &mut statuses).await?;
        assert_eq!(asked(&ctx, bot).await?, vec![triage(first)]);
        // Given a display name, it holds triage still, and hears nothing more.
        ops::add_principal(&ctx, "triage-1", None, None, Some("Triage"), Some(bot), None).await?;
        handle(&ctx, principal(&ctx, bot).await?, &mut statuses).await?;
        assert_eq!(asked(&ctx, bot).await?, vec![triage(first)]);

        // It reads the first question, is asked a second, and is retired before it reads that one;
        // a third waits. Alice selects triage, and hears of the second and the third.
        read_all(&ctx, bot).await?;
        let second = ask(&ctx, daniel, "is Windows in scope?").await?;
        handle(&ctx, message(&ctx, second).await?, &mut statuses).await?;
        ops::retire_principal(&ctx, "triage-1").await?;
        handle(&ctx, principal(&ctx, bot).await?, &mut statuses).await?;
        let third = ask(&ctx, daniel, "who reviews the connector?").await?;
        handle(&ctx, message(&ctx, third).await?, &mut statuses).await?;
        ops::set_role(&ctx, "impl-alice", AgentRole::Triage, Some(ops::Serves::ProjectWide)).await?;
        handle(&ctx, principal(&ctx, alice).await?, &mut statuses).await?;
        assert_eq!(asked(&ctx, alice).await?, vec![triage(second), triage(third)]);

        // Alice reads them and is retired; a fourth waits; triage-4 succeeds her, and hears of it.
        read_all(&ctx, alice).await?;
        ops::retire_principal(&ctx, "impl-alice").await?;
        handle(&ctx, principal(&ctx, alice).await?, &mut statuses).await?;
        let fourth = ask(&ctx, daniel, "does the daemon restart on update?").await?;
        handle(&ctx, message(&ctx, fourth).await?, &mut statuses).await?;
        let heir = ops::add_successor(&ctx, "triage-4", None, None, None, None, None, "impl-alice").await?.successor;
        handle(&ctx, principal(&ctx, alice).await?, &mut statuses).await?;
        handle(&ctx, principal(&ctx, heir).await?, &mut statuses).await?;
        assert_eq!(asked(&ctx, heir).await?, vec![triage(fourth)]);
        Ok(())
    }

    /// A successor carries on its predecessor's job, its unread notifications with it
    /// (`ops::succeed_principal`), so a question the triage agent it succeeded was told of never
    /// reaches it a second time: not one that agent left unread, which moved to it, nor one it read.
    #[tokio::test]
    async fn a_successor_is_not_told_again_of_a_question_its_predecessor_was_told_of() -> Result<()> {
        let ctx = context().await?;
        let daniel = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
        let mut statuses = Statuses::seed(&ctx).await?;
        let triage = |question: EntityId| (question, "triage".to_string());
        let waited = ask(&ctx, daniel, "who takes the TLS part?").await?;
        handle(&ctx, message(&ctx, waited).await?, &mut statuses).await?;
        let first = ops::add_principal(
            &ctx,
            "triage-1",
            Some(AgentRole::Triage),
            Some(ops::Serves::ProjectWide),
            None,
            None,
            None,
        )
        .await?;
        handle(&ctx, principal(&ctx, first).await?, &mut statuses).await?;
        let held = ask(&ctx, daniel, "is Windows in scope?").await?;
        handle(&ctx, message(&ctx, held).await?, &mut statuses).await?;
        assert_eq!(asked(&ctx, first).await?, vec![triage(waited), triage(held)]);

        // triage-2 succeeds it, unread: both questions move to triage-2, and neither is told again.
        let second = ops::add_successor(&ctx, "triage-2", None, None, None, None, None, "triage-1").await?.successor;
        for p in [first, second] {
            handle(&ctx, principal(&ctx, p).await?, &mut statuses).await?;
        }
        assert_eq!(asked(&ctx, second).await?, vec![triage(waited), triage(held)]);

        // It reads them, and Bob, an implementer, succeeds it: he hears of neither.
        read_all(&ctx, second).await?;
        let bob = ops::add_principal(&ctx, "impl-bob", Some(AgentRole::Implementer), None, None, None, None).await?;
        handle(&ctx, principal(&ctx, bob).await?, &mut statuses).await?;
        ops::succeed_principal(&ctx, "triage-2", bob).await?;
        for p in [second, bob] {
            handle(&ctx, principal(&ctx, p).await?, &mut statuses).await?;
        }
        assert_eq!(front(&ctx, None).await?, Some(bob), "Bob serves the whole project in its place");
        assert_eq!(asked(&ctx, bob).await?, vec![]);
        Ok(())
    }

    /// The daemon's start tells no one: the agent that holds triage then is not told of the
    /// questions that waited before it took the role, since only a change after the start is news.
    /// An agent that gives triage up is told nothing, and one that takes it again after the start is
    /// told of the questions that wait still; one retired and brought back reads its inbox again,
    /// which holds them, and is not told of them twice.
    #[tokio::test]
    async fn the_triage_agent_at_the_start_is_told_only_when_it_takes_the_role_again() -> Result<()> {
        let ctx = context().await?;
        let daniel = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
        let waited = ask(&ctx, daniel, "who takes the TLS part?").await?;
        let bot = ops::add_principal(
            &ctx,
            "triage-bot",
            Some(AgentRole::Triage),
            Some(ops::Serves::ProjectWide),
            None,
            None,
            None,
        )
        .await?;
        // The daemon starts with the agent holding triage: a change of it after the start tells it
        // nothing.
        let mut statuses = Statuses::seed(&ctx).await?;
        ops::add_principal(&ctx, "triage-bot", None, None, Some("Triage"), Some(bot), None).await?;
        handle(&ctx, principal(&ctx, bot).await?, &mut statuses).await?;
        assert_eq!(asked(&ctx, bot).await?, vec![]);
        // It gives triage up, and hears nothing; it takes it again, and hears of the question.
        ops::set_role(&ctx, "triage-bot", AgentRole::Implementer, None).await?;
        handle(&ctx, principal(&ctx, bot).await?, &mut statuses).await?;
        assert_eq!(asked(&ctx, bot).await?, vec![]);
        ops::set_role(&ctx, "triage-bot", AgentRole::Triage, Some(ops::Serves::ProjectWide)).await?;
        handle(&ctx, principal(&ctx, bot).await?, &mut statuses).await?;
        assert_eq!(asked(&ctx, bot).await?, vec![(waited, "triage".to_string())]);
        // Retired, it leaves the question unread in an inbox no agent reads; brought back with its
        // role, it reads that inbox again, and hears of the question no second time.
        ops::retire_principal(&ctx, "triage-bot").await?;
        handle(&ctx, principal(&ctx, bot).await?, &mut statuses).await?;
        ops::add_principal(&ctx, "triage-bot", None, None, None, Some(bot), None).await?;
        handle(&ctx, principal(&ctx, bot).await?, &mut statuses).await?;
        assert_eq!(front(&ctx, None).await?, Some(bot));
        assert_eq!(asked(&ctx, bot).await?, vec![(waited, "triage".to_string())]);
        Ok(())
    }

    /// A question written as an agent takes triage reaches it once, whichever of the two changes
    /// the worker reaches first: when the agent's comes first, the questions that waited, read from
    /// the graph, hold the question already, and its own plan then leaves the agent out; when the
    /// question's comes first, its plan finds the agent holding triage, and the questions that
    /// waited then leave it out.
    #[tokio::test]
    async fn a_question_written_as_an_agent_takes_triage_reaches_it_once() -> Result<()> {
        let ctx = context().await?;
        let daniel = ops::ensure_principal(&ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
        let mut statuses = Statuses::seed(&ctx).await?;
        let first = ops::add_principal(
            &ctx,
            "triage-1",
            Some(AgentRole::Triage),
            Some(ops::Serves::ProjectWide),
            None,
            None,
            None,
        )
        .await?;
        let early = ask(&ctx, daniel, "who takes the TLS part?").await?;
        handle(&ctx, principal(&ctx, first).await?, &mut statuses).await?;
        handle(&ctx, message(&ctx, early).await?, &mut statuses).await?;
        assert_eq!(asked(&ctx, first).await?, vec![(early, "triage".to_string())]);

        read_all(&ctx, first).await?;
        ops::retire_principal(&ctx, "triage-1").await?;
        handle(&ctx, principal(&ctx, first).await?, &mut statuses).await?;
        let late = ask(&ctx, daniel, "is Windows in scope?").await?;
        let second = ops::add_principal(
            &ctx,
            "triage-2",
            Some(AgentRole::Triage),
            Some(ops::Serves::ProjectWide),
            None,
            None,
            None,
        )
        .await?;
        handle(&ctx, message(&ctx, late).await?, &mut statuses).await?;
        handle(&ctx, principal(&ctx, second).await?, &mut statuses).await?;
        assert_eq!(asked(&ctx, second).await?, vec![(late, "triage".to_string())]);
        Ok(())
    }
}
