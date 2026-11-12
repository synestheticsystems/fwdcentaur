//! Moving tasks to another project (`fwd task move`), so that work filed in one project is grouped
//! with the project it serves, with every connection it has kept: tasks are "interconnected across
//! projects, and that's actually desirable! important to keep those connections" (Daniel,
//! 2026-11-12; docs/DECISIONS.md, D175). A move cuts, copies and rewrites no connection: it changes
//! which project groups a task and what belongs to it, and every id a record holds goes on naming
//! what it named.
//!
//! The move makes the task, and everything that belongs to it, records of the destination project,
//! and leaves alone whatever of that is so already. What belongs to a task ([`Belongings`]): the
//! task; the relations from it, whatever their kind or state, since a relation is its `from` end's;
//! the messages about it, replies included, and the relations from them, their mentions among them;
//! the steps of its checklist, which are its `step_of` relations; and the former ids of each of
//! these. What a step is stays where it is, since it has a life of its own: a task that is a step
//! moves only when the move names it too, a message only when it is about the task, and a highlight
//! never. What stays: the relations into the task from records that stay, which are found by its
//! id; notifications, which are their recipients' inboxes; highlights; decisions; scopes; and
//! principals. Every reference to a principal (an author, an assignee, a submitter, a question's
//! addressee, a relation's maker) stays as it is, naming the principal of the project the task
//! left, which a reader shows with that project's name.
//!
//! The scope: the task's `worked_in` relations to scopes of the project it leaves are retracted,
//! with the move as the reason, and one to the destination's scope the move names is made unless
//! it holds already. A destination with an active scope takes a task only into one of its scopes;
//! one with none takes it into none.
//!
//! The writes are ordered so that an interrupted move is no problem: first, in one transaction of
//! the destination, everything that belongs to the task but the task, and its scopes; last, in
//! another, the task itself with the comment that records the move. A second run finds made what
//! the first made and makes the rest, so a move that stopped partway finishes when it is run again,
//! and a task wholly in the destination already is reported as there, with nothing written.

use crate::model::*;
use crate::ops::{self, Reason, WrittenBy};
use crate::project_graph::{Followed, InProjectView, ProjectGraph, ProjectRecord, Whose};
use ankurah::property::{Json, Ref};
use ankurah::{EntityId, View};
use anyhow::{bail, Result};
use serde_json::json;

/// Why a move was refused before anything was written, with what to do instead.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Refused {
    #[error("`{}` names no task the node holds; give a task's id, or the start of it (`fwd task list` lists them)", short(*.0))]
    NoTask(EntityId),
    /// The tasks not in the destination already are of several projects, each `(task, project)`.
    #[error(
        "a move runs in one project, as whoever moves the tasks is there, and these tasks are of several: {}; move each project's tasks with a command of their own",
        .0.iter().map(|(task, project)| format!("`{task}` of {project}")).collect::<Vec<_>>().join(", ")
    )]
    SeveralProjects(Vec<(String, String)>),
    #[error(
        "the node holds no project `{named}`; it holds {}: give one of those names, or the project's id",
        .held.join(", ")
    )]
    NoProject { named: String, held: Vec<String> },
    #[error("{} projects are named `{named}`: give one's id ({})", .ids.len(), .ids.join(", "))]
    SeveralNamed { named: String, ids: Vec<String> },
    #[error(
        "the scope `{title}` ({scope}) is a scope of {project}, not of {destination}: give one of {destination}'s scopes (`fwd --project '{destination}' scope list` lists them)"
    )]
    ScopeElsewhere { scope: String, title: String, project: String, destination: String },
    #[error(
        "{destination} has active scopes ({}), so a task moves into one of them: name it with --to-scope",
        .active.iter().map(|(id, title)| format!("`{title}` {id}")).collect::<Vec<_>>().join(", ")
    )]
    ScopeNeeded { destination: String, active: Vec<(String, String)> },
}

/// The first eight characters of an id, as a refusal names a record.
fn short(id: EntityId) -> String {
    id.to_base64().chars().take(8).collect()
}

/// The project tasks move to, and the scope of it they are worked in there, as `fwd task move --to
/// <project> --to-scope <scope>` names them, read before anything is written.
#[derive(Clone)]
pub struct Destination {
    /// The project's graph, whose transactions take what moves in.
    pub graph: ProjectGraph,
    pub project: ProjectRecord,
    /// The scope the tasks are worked in there, with its title; none for a destination with no
    /// active scope when the move names none.
    pub scope: Option<(EntityId, String)>,
}

impl Destination {
    /// The project `to` names among the node's projects, by its name (without regard to case) or
    /// its UUID, and its scope `scope` names, by its title among that project's scopes or by an id
    /// (`ops::resolve_scope`). Refused for a project the node does not hold, for a name several
    /// projects share, for a scope of another project, and, without `scope`, when the project has
    /// an active scope, naming its active scopes.
    pub async fn named(graph: &ProjectGraph, to: &str, scope: Option<&str>) -> Result<Destination> {
        let wanted = to.trim();
        let mut projects = graph.projects().await?;
        projects.sort_by(|a, b| a.name.cmp(&b.name));
        let uuid = wanted.parse::<uuid::Uuid>().ok();
        let named: Vec<&ProjectRecord> =
            projects.iter().filter(|p| Some(p.uuid) == uuid || p.name.trim().eq_ignore_ascii_case(wanted)).collect();
        let project = match named.as_slice() {
            [one] => (*one).clone(),
            [] => {
                let held = projects.iter().map(|p| format!("`{}`", p.name)).collect();
                return Err(Refused::NoProject { named: wanted.into(), held }.into());
            }
            several => {
                let ids = several.iter().map(|p| p.uuid.to_string()).collect();
                return Err(Refused::SeveralNamed { named: wanted.into(), ids }.into());
            }
        };
        let there = graph.graph_of_project(&project);
        let scope = match scope {
            Some(named) => Some(scope_of(&there, &project, named).await?),
            None => {
                let active: Vec<ScopeView> = there.fetch("status = 'active'").await?;
                if !active.is_empty() {
                    let active = active.iter().map(|s| Ok((short(s.id()), s.title()?))).collect::<Result<_>>()?;
                    return Err(Refused::ScopeNeeded { destination: project.name, active }.into());
                }
                None
            }
        };
        Ok(Destination { graph: there, project, scope })
    }
}

/// The scope `named` names, which must be one of `project`'s, read in its graph `there`, and its
/// title.
async fn scope_of(there: &ProjectGraph, project: &ProjectRecord, named: &str) -> Result<(EntityId, String)> {
    let id = ops::resolve_scope(there, named).await?;
    match there.follow::<ScopeView>(id).await? {
        Followed::Held { record, project: Whose::This, found } => Ok((record, found.title()?)),
        Followed::Held { record, project: Whose::Other(elsewhere), found } => Err(Refused::ScopeElsewhere {
            scope: short(record),
            title: found.title()?,
            project: elsewhere.name,
            destination: project.name.clone(),
        }
        .into()),
        Followed::NotHeld => bail!("no scope has the id {id}"),
    }
}

/// Where a move of `tasks` to `to` runs, named as `graph` names a project: the project of the
/// tasks that are not in the destination, which must all be of one project, since whoever moves
/// them is the caller's principal there; the destination when every task is there already, as
/// when a move that finished is run again. Refused, before anything is written, for an id that
/// names no task and for tasks of several projects, naming each with its project.
pub async fn place(graph: &ProjectGraph, tasks: &[EntityId], to: &Destination) -> Result<Whose> {
    let mut away: Vec<(EntityId, Whose)> = Vec::new();
    for &task in tasks {
        match graph.follow::<TaskView>(task).await? {
            Followed::Held { record, project, found } => {
                if found.project_id()? != to.project.record {
                    away.push((record, project));
                }
            }
            Followed::NotHeld => return Err(Refused::NoTask(task).into()),
        }
    }
    let mut projects: Vec<&Whose> = Vec::new();
    for (_, project) in &away {
        if !projects.contains(&project) {
            projects.push(project);
        }
    }
    match projects.as_slice() {
        [] if to.project.record == graph.record() => Ok(Whose::This),
        [] => Ok(Whose::Other(to.project.clone())),
        [one] => Ok((*one).clone()),
        _ => {
            let here = graph.projects().await?.into_iter().find(|p| p.record == graph.record());
            let name = |project: &Whose| match project {
                Whose::This => here.as_ref().map_or_else(|| "this project".to_string(), |p| p.name.clone()),
                Whose::Other(p) => p.name.clone(),
            };
            Err(Refused::SeveralProjects(away.iter().map(|(task, project)| (short(*task), name(project))).collect())
                .into())
        }
    }
}

/// What a run of the move did with one task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Moved {
    pub task: EntityId,
    /// The project the task was in when the run began: the one it came from, or the destination
    /// for a task that was there already.
    pub from: ProjectRecord,
    /// Whether this run moved the task itself, which it does not to a task there already.
    pub moved: bool,
    /// What belongs to the task that this run took into the destination.
    pub brought: Brought,
    /// The scopes of the project it left that it was taken off.
    pub taken_off: Vec<EntityId>,
    /// The scopes it is worked in once the run is over (`ops::worked_in`).
    pub worked_in: Vec<EntityId>,
    /// The comment that records the move; none for a task that was there already.
    pub comment: Option<EntityId>,
}

/// How many of each kind of what belongs to a task a run took into the destination: the messages
/// about it, the relations from it and from its messages (its steps apart), the steps of its
/// checklist, and the former ids of all of them and of the task.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Brought {
    pub messages: usize,
    pub relations: usize,
    pub steps: usize,
    pub former_ids: usize,
}

/// Makes each of `tasks`, and everything that belongs to it, records of the destination `to`, by
/// `by`, the caller's principal in `graph`'s project, where the move runs ([`place`]), and records
/// each move with a comment by `by` saying where the task came from and went, with `note`, whose
/// references are read as a post's are before anything is written. A task there already is
/// reported so, with whatever of it was left behind taken in.
pub async fn move_tasks(
    graph: &ProjectGraph,
    by: EntityId,
    tasks: &[EntityId],
    to: &Destination,
    note: Option<&str>,
) -> Result<Vec<Moved>> {
    if let Whose::Other(there) = place(graph, tasks, to).await? {
        bail!("a move runs in the project of the tasks it moves, {}, as whoever moves them is there", there.name);
    }
    let reason = match note.map(str::trim).filter(|n| !n.is_empty()) {
        Some(note) => Some(ops::read_reason(graph, by, note).await?),
        None => None,
    };
    let projects = graph.projects().await?;
    let mut done: Vec<Moved> = Vec::with_capacity(tasks.len());
    for &task in tasks {
        if !done.iter().any(|d| d.task == task) {
            done.push(move_one(graph, by, task, to, reason.as_ref(), &projects).await?);
        }
    }
    Ok(done)
}

/// Moves `task`, as [`move_tasks`] says, `projects` being the node's.
async fn move_one(
    graph: &ProjectGraph,
    by: EntityId,
    task: EntityId,
    to: &Destination,
    reason: Option<&Reason>,
    projects: &[ProjectRecord],
) -> Result<Moved> {
    let Followed::Held { found: view, .. } = graph.follow::<TaskView>(task).await? else {
        return Err(Refused::NoTask(task).into());
    };
    let left = view.project_id()?;
    let Some(from) = projects.iter().find(|p| p.record == left).cloned() else {
        bail!("the node holds no project {left}, which task {} names", short(task));
    };
    let moving = left != to.project.record;
    let held = Belongings::of(graph, task).await?;

    // The scopes of the project it leaves that it is worked in, which it is taken off; and the
    // destination's scope, unless it is worked in that one already. A task there already keeps
    // the scopes it has.
    let mut leaving: Vec<EntityId> = Vec::new();
    let mut taken_off: Vec<EntityId> = Vec::new();
    let mut joining = None;
    if moving {
        for r in held.relations.iter().filter(|r| worked_in(r, task)) {
            let scope = r.to_id()?;
            if let Followed::Held { found, .. } = graph.follow::<ScopeView>(scope).await? {
                if found.project_id()? == left {
                    leaving.push(r.id());
                    taken_off.push(scope);
                }
            }
        }
        joining = to
            .scope
            .as_ref()
            .map(|(scope, _)| *scope)
            .filter(|scope| !held.relations.iter().any(|r| worked_in(r, task) && r.to_id().ok() == Some(*scope)));
    }

    let brought = Brought {
        messages: held.messages.iter().filter(|m| !to.graph.owns(*m)).count(),
        relations: held.relations.iter().filter(|r| !to.graph.owns(*r)).count(),
        steps: held.steps.iter().filter(|r| !to.graph.owns(*r)).count(),
        former_ids: held.former_ids.iter().filter(|f| !to.graph.owns(*f)).count(),
    };
    if brought != Brought::default() || !leaving.is_empty() || joining.is_some() {
        let why = format!("the task moved from {} to {}", from.name, to.project.name);
        bring(to, by, task, &held, (&leaving, &why), joining).await?;
    }
    // Last, the task itself, so that a move interrupted before this point is run again from a
    // known place, and one that got this far is the task in the destination.
    let comment = if moving { Some(arrive(to, by, &view, &from, reason).await?) } else { None };
    let worked_in = ops::worked_in(&to.graph, task).await?;
    Ok(Moved { task, from, moved: moving, brought, taken_off, worked_in, comment })
}

/// Takes into the destination `to`, in one transaction, what of `held`, the belongings of `task`,
/// is not there yet; retracts the relations `leaving` names, the task's scopes of the project it
/// leaves, with `by` as who took it off them and the reason `why`; and makes `task` worked in the
/// scope `joining`, by `by`.
async fn bring(
    to: &Destination,
    by: EntityId,
    task: EntityId,
    held: &Belongings,
    (leaving, why): (&[EntityId], &str),
    joining: Option<EntityId>,
) -> Result<()> {
    let t = crate::now_ms();
    let trx = to.graph.begin();
    for m in held.messages.iter().filter(|m| !to.graph.owns(*m)) {
        trx.take_in(m)?;
    }
    for r in held.relations.iter().chain(&held.steps) {
        let retracting = leaving.contains(&r.id());
        if to.graph.owns(r) && !retracting {
            continue;
        }
        let taken = trx.take_in(r)?;
        if retracting {
            taken.status()?.set(&RelationStatus::Retracted)?;
            taken.closed_by()?.set(&Some(Ref::new(by)))?;
            taken.closed()?.set(&Some(t))?;
            taken.closed_reason()?.set(&Some(why.to_string()))?;
        }
    }
    for f in held.former_ids.iter().filter(|f| !to.graph.owns(*f)) {
        trx.take_in(f)?;
    }
    if let Some(scope) = joining {
        trx.create(&Relation {
            project: trx.project(),
            kind: RelationKind::WorkedIn,
            from_id: task,
            to_id: scope,
            status: RelationStatus::Accepted,
            created_by: Ref::new(by),
            created: t,
            via: Json::new(json!({"actor_id": by.to_base64()})),
            note: None,
            position: None,
            closed_by: None,
            closed: None,
            closed_reason: None,
        })
        .await?;
    }
    Ok(trx.commit().await?)
}

/// Takes the task `view` shows into the destination `to`, in one transaction with the comment by
/// `by` that records its move from `from`, with the reason given, if any, and a `mention` relation
/// to each thing the reason names, as a post writes one. Answers the comment.
async fn arrive(
    to: &Destination,
    by: EntityId,
    view: &TaskView,
    from: &ProjectRecord,
    reason: Option<&Reason>,
) -> Result<EntityId> {
    let opening = format!("Moved from {} to {}", from.name, to.project.name);
    let (content, named) = match reason {
        Some(reason) => (format!("{opening}: {}", reason.text), reason.named.as_slice()),
        None => (format!("{opening}."), &[][..]),
    };
    let t = crate::now_ms();
    let trx = to.graph.begin();
    trx.take_in(view)?;
    let comment = trx
        .create(&Message {
            project: trx.project(),
            author: Ref::new(by),
            created: t,
            content,
            priority: Priority::Next,
            status: MessageStatus::Active,
            task: Some(Ref::new(view.id())),
            anchor: None,
            scope: None,
            decision: None,
            reply_to: None,
            thread: None,
            context: Json::new(json!({})),
            resolved_by: None,
            resolved: None,
            edited: None,
            edited_by: None,
        })
        .await?
        .id();
    ops::write_mentions(&trx, by, comment, named, WrittenBy::Post, t).await?;
    trx.commit().await?;
    Ok(comment)
}

/// Whether `r` is an accepted `worked_in` from `task`: a scope the task is worked in.
fn worked_in(r: &RelationView, task: EntityId) -> bool {
    r.kind().ok() == Some(RelationKind::WorkedIn)
        && r.from_id().ok() == Some(task)
        && r.status().ok() == Some(RelationStatus::Accepted)
}

/// What belongs to a task and changes project with it, wherever each is now: read by the task's
/// id and its messages' ids, with no project term.
struct Belongings {
    /// The messages about the task, and the replies in their threads.
    messages: Vec<MessageView>,
    /// The relations from the task and from its messages, whatever their kind or state, its steps
    /// apart.
    relations: Vec<RelationView>,
    /// The steps of its checklist: the `step_of` relations to it, whatever their state.
    steps: Vec<RelationView>,
    /// The former ids of the task and of each of the above.
    former_ids: Vec<FormerIdView>,
}

impl Belongings {
    async fn of(graph: &ProjectGraph, task: EntityId) -> Result<Belongings> {
        let mut messages = graph.messages_about("task", task).await?;
        // A reply takes its thread's subject as it is posted; one written before replies did
        // names only its thread.
        let roots: Vec<EntityId> =
            messages.iter().filter(|m| m.thread().ok().flatten().is_none()).map(View::id).collect();
        for root in roots {
            for reply in graph.messages_about("thread", root).await? {
                if !messages.iter().any(|m| m.id() == reply.id()) {
                    messages.push(reply);
                }
            }
        }
        let steps = graph.relations_to(task, Some(RelationKind::StepOf)).await?;
        let mut relations = graph.relations_from(task, None).await?;
        for m in &messages {
            relations.extend(graph.relations_from(m.id(), None).await?);
        }
        relations.retain(|r| !steps.iter().any(|s| s.id() == r.id()));
        let ids: Vec<EntityId> = std::iter::once(task)
            .chain(messages.iter().map(View::id))
            .chain(relations.iter().map(View::id))
            .chain(steps.iter().map(View::id))
            .collect();
        let former_ids = graph.former_ids_of(&ids).await?;
        Ok(Belongings { messages, relations, steps, former_ids })
    }
}
