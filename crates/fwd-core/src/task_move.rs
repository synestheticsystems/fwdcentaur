//! Moving tasks to another project (`fwd task move`), so that work filed in one project is grouped
//! with the project it serves, with every connection it has kept: tasks are "interconnected across
//! projects, and that's actually desirable! important to keep those connections" (Daniel,
//! 2026-11-12; docs/DECISIONS.md, D175). A move cuts, copies and rewrites no connection: it changes
//! which project groups a task and what belongs to it, and every id a record holds goes on naming
//! what it named.
//!
//! The move makes the task, and everything that belongs to it, records of the destination project,
//! and leaves alone whatever of that is so already. What belongs to a task ([`Belongings`]): the
//! task; the relations from it, whatever their kind or state; the messages about it, replies
//! included, and the relations from them, their mentions among them; and the former ids of each of
//! these. A relation is a record of its `from` end's project, always, so a step of the task's
//! checklist moves with the task only when it is written as one of its messages: a task or a
//! highlight that serves as a step keeps its `step_of` relation in its own project, and the
//! checklist still shows it, since a record's relations are read by its id. What stays: the
//! relations into the task from records that stay; notifications, which are their recipients'
//! inboxes; highlights; decisions; scopes; and principals. Every reference to a principal (an
//! author, an assignee, a submitter, a question's addressee, a relation's maker) stays as it is,
//! naming the principal of the project the task left, which a reader shows with that project's
//! name.
//!
//! The scope: the task's `worked_in` relations to scopes of the project it leaves are retracted,
//! with the move as the reason, and one to the destination's scope the move names, if it names one,
//! is made unless it holds already. A task there already keeps the scopes it has.
//!
//! One move at a time runs on a node ([`MoveLock`]), held from before a move is placed until its
//! last write, so that a second move of the same tasks begun at the same moment sees what the first
//! did, as if run after it. The writes are ordered so that an interrupted move is no problem: first,
//! in one transaction of the destination, everything that belongs to the task but the task, and
//! its scopes; last, in another, the task itself with the comment that records the move. A second
//! run finds made what the first made and makes the rest, so a move that stopped partway finishes
//! when it is run again, and a task wholly in the destination already is reported as there, with
//! nothing written.

use crate::model::*;
use crate::ops::{self, Reason, WrittenBy};
use crate::project::Paths;
use crate::project_graph::{Followed, InProjectView, ProjectGraph, ProjectRecord, Whose};
use ankurah::property::{Json, Ref};
use ankurah::{EntityId, View};
use anyhow::{Context, Result};
use fs4::fs_std::FileExt;
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
        "no scope has the id `{named}`: give one of {destination}'s scopes by its title, its id or the start of it (`fwd --project '{destination}' scope list` lists them)"
    )]
    NoScope { named: String, destination: String },
    #[error(
        "the scope `{title}` ({scope}) is a scope of {project}, not of {destination}: give one of {destination}'s scopes (`fwd --project '{destination}' scope list` lists them)"
    )]
    ScopeElsewhere { scope: String, title: String, project: String, destination: String },
    /// A placed move made through the graph of another project than the one it was placed in,
    /// whose principal `by` is not: the CLI and the MCP make it where they placed it.
    #[error("a move runs in the project of the tasks it moves, {project}, as whoever moves them is there")]
    RunsElsewhere { project: String },
}

/// The first eight characters of an id, as a refusal names a record.
fn short(id: EntityId) -> String {
    id.to_base64().chars().take(8).collect()
}

/// The node's one move at a time: a file lock beside the daemon's own lock files
/// ([`Paths::move_lock`]), which a move holds from before it is placed until its last write, so
/// that two moves begun at the same moment, by two people or two agents told to make the same
/// moves, run one after the other, the second seeing what the first did. Without it both read the
/// task before either wrote, and both wrote: two comments, two `worked_in` relations, and what
/// belongs to the task split between two destinations. The CLI and the daemon's MCP endpoint both
/// take it, through the runtime directory a client finds its daemon by, so they agree on it
/// whatever data directory each was given. It is an advisory lock on an open file, released when
/// the holder drops it or its process ends, killed or not. It orders moves alone: a comment or an
/// amend written while a move runs is not held up, and lands where the task was when it was placed
/// (a comment is found on its task by id, and any later move of the task takes it in).
pub struct MoveLock {
    _held: std::fs::File,
}

impl MoveLock {
    /// Takes the node's move lock, waiting for the move that holds it to end, and telling
    /// `waiting` first when one does, so that whoever runs the command sees why it pauses.
    pub async fn take(paths: &Paths, waiting: impl FnOnce()) -> Result<MoveLock> {
        let path = paths.move_lock();
        if let Some(held) = crate::daemon::try_lock(&path).with_context(|| format!("locking {}", path.display()))? {
            return Ok(MoveLock { _held: held });
        }
        waiting();
        let held = tokio::task::spawn_blocking(move || -> Result<std::fs::File> {
            let file = std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).open(&path)?;
            file.lock_exclusive().with_context(|| format!("locking {}", path.display()))?;
            Ok(file)
        })
        .await??;
        Ok(MoveLock { _held: held })
    }
}

/// The project tasks move to, and the scope of it they are worked in there, as `fwd task move --to
/// <project> [--to-scope <scope>]` names them, read before anything is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination {
    pub project: ProjectRecord,
    /// The scope the tasks are worked in there, with its title; none when the move names none,
    /// and the tasks arrive in no scope, as tasks without a scope are ordinary in any project.
    pub scope: Option<(EntityId, String)>,
}

impl Destination {
    /// The project `to` names among the node's projects, by its name (without regard to case) or
    /// its UUID, and the scope `scope` names, by its title among that project's scopes or by an id
    /// (`ops::resolve_scope`). Refused for a project the node does not hold, for a name several
    /// projects share, and for a scope of another project.
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
        let scope = match scope {
            Some(named) => Some(scope_of(&graph.graph_of_project(&project), &project, named).await?),
            None => None,
        };
        Ok(Destination { project, scope })
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
        Followed::NotHeld => Err(Refused::NoScope { named: named.into(), destination: project.name.clone() }.into()),
    }
}

/// A move placed under the node's move lock, which it holds until it is dropped: the tasks it
/// moves, each with the project it was in when placed, where it goes, and the project it runs in,
/// whose principal moves them. Only [`place`] makes one, so that a move is placed and made under
/// the lock, and the CLI and the MCP place a move once, before they find whom it acts as, and make
/// it where they placed it.
pub struct Move {
    to: Destination,
    tasks: Vec<EntityId>,
    runs_in: ProjectRecord,
    _lock: MoveLock,
}

/// Places a move of `tasks` to `to`, holding `lock`, the node's move lock, so that what it reads
/// stays true until the move is made: it runs in the project of the tasks that are not in the
/// destination, which must all be of one project, since whoever moves them is the caller's
/// principal there; or in the destination when every task is there already, as when a move that
/// finished is run again. A task named twice, or by a former and a current id, is moved once.
/// Refused, before anything is written, for an id that names no task and for tasks of several
/// projects, naming each with its project.
pub async fn place(graph: &ProjectGraph, tasks: &[EntityId], to: Destination, lock: MoveLock) -> Result<Move> {
    // Followed from the destination, every task's project is named, the destination's as `This`.
    let there = graph.graph_of_project(&to.project);
    let mut placed: Vec<EntityId> = Vec::with_capacity(tasks.len());
    let mut away: Vec<(EntityId, ProjectRecord)> = Vec::new();
    for &task in tasks {
        match there.follow::<TaskView>(task).await? {
            Followed::Held { record, .. } if placed.contains(&record) => {}
            Followed::Held { record, project, .. } => {
                placed.push(record);
                if let Whose::Other(from) = project {
                    away.push((record, from));
                }
            }
            Followed::NotHeld => return Err(Refused::NoTask(task).into()),
        }
    }
    let mut projects: Vec<&ProjectRecord> = Vec::new();
    for (_, project) in &away {
        if !projects.contains(&project) {
            projects.push(project);
        }
    }
    let runs_in = match projects.as_slice() {
        [] => to.project.clone(),
        [one] => (*one).clone(),
        _ => {
            let each = away.iter().map(|(task, project)| (short(*task), project.name.clone())).collect();
            return Err(Refused::SeveralProjects(each).into());
        }
    };
    Ok(Move { to, tasks: placed, runs_in, _lock: lock })
}

impl Move {
    /// The project the move runs in, as `graph` names a project.
    pub fn runs_in(&self, graph: &ProjectGraph) -> Whose {
        if self.runs_in.record == graph.record() {
            Whose::This
        } else {
            Whose::Other(self.runs_in.clone())
        }
    }

    /// Where the tasks go.
    pub fn destination(&self) -> &Destination {
        &self.to
    }

    /// Makes each task, and everything that belongs to it, records of the destination, by `by`,
    /// the caller's principal in `graph`'s project, which must be the project the move runs in, and
    /// records each move with a comment by `by` saying where the task came from and went, with
    /// `note`, whose references are read as a post's are before anything is written. A task there
    /// already is reported so, with whatever of it was left behind taken in.
    pub async fn make(&self, graph: &ProjectGraph, by: EntityId, note: Option<&str>) -> Result<Vec<Moved>> {
        if graph.record() != self.runs_in.record {
            return Err(Refused::RunsElsewhere { project: self.runs_in.name.clone() }.into());
        }
        let reason = match note.map(str::trim).filter(|n| !n.is_empty()) {
            Some(note) => Some(ops::read_reason(graph, by, note).await?),
            None => None,
        };
        let there = graph.graph_of_project(&self.to.project);
        let mut done: Vec<Moved> = Vec::with_capacity(self.tasks.len());
        for &task in &self.tasks {
            done.push(move_one(graph, &there, &self.to, by, task, reason.as_ref()).await?);
        }
        Ok(done)
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
    /// Whether the task was there already and keeps its scopes without the scope the move names:
    /// a move puts a task in the destination's scope as it arrives, and never changes the scopes of
    /// a task there already, which is the job of `fwd link` and `fwd unlink`.
    pub kept_scopes: bool,
    /// The comment that records the move; none for a task that was there already.
    pub comment: Option<EntityId>,
}

/// How many of each kind of what belongs to a task a run took into the destination, as it wrote
/// them: the messages about it, the relations from it and from its messages, and the former ids of
/// all of them and of the task.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Brought {
    pub messages: usize,
    pub relations: usize,
    pub former_ids: usize,
}

/// Moves `task`, as [`Move::make`] says, by `by`, of `graph`'s project, where the move runs, to
/// `to`, whose graph is `there`.
async fn move_one(
    graph: &ProjectGraph,
    there: &ProjectGraph,
    to: &Destination,
    by: EntityId,
    task: EntityId,
    reason: Option<&Reason>,
) -> Result<Moved> {
    // Read through the destination's graph, a task there already is `This`, and another names its
    // project. The move lock keeps it where it was placed.
    let (view, from) = match there.follow::<TaskView>(task).await? {
        Followed::Held { found, project: Whose::This, .. } => (found, to.project.clone()),
        Followed::Held { found, project: Whose::Other(from), .. } => (found, from),
        Followed::NotHeld => return Err(Refused::NoTask(task).into()),
    };
    let moving = from.record != to.project.record;
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
                if found.project_id()? == from.record {
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

    let why = format!("the task moved from {} to {}", from.name, to.project.name);
    let brought = bring(there, by, task, &held, (&leaving, &why), joining).await?;
    // Last, the task itself, so that a move interrupted before this point is run again from a
    // known place, and one that got this far is the task in the destination.
    let comment = if moving { Some(arrive(there, to, by, &view, &from, reason).await?) } else { None };
    let worked_in = ops::worked_in(there, task).await?;
    let kept_scopes = !moving && to.scope.as_ref().is_some_and(|(scope, _)| !worked_in.contains(scope));
    Ok(Moved { task, from, moved: moving, brought, taken_off, worked_in, kept_scopes, comment })
}

/// Takes into the destination's graph `there`, in one transaction, what of `held`, the
/// belongings of `task`, is not there yet; retracts the relations `leaving` names, the task's
/// scopes of the project it leaves, with `by` as who took it off them and the reason `why`; and
/// makes `task` worked in the scope `joining`, by `by`. Answers how many of each kind it took in,
/// and commits nothing when there is nothing to write.
async fn bring(
    there: &ProjectGraph,
    by: EntityId,
    task: EntityId,
    held: &Belongings,
    (leaving, why): (&[EntityId], &str),
    joining: Option<EntityId>,
) -> Result<Brought> {
    let t = crate::now_ms();
    let trx = there.begin();
    let mut brought = Brought::default();
    for m in held.messages.iter().filter(|m| !there.owns(*m)) {
        trx.take_in(m)?;
        brought.messages += 1;
    }
    for r in &held.relations {
        let retracting = leaving.contains(&r.id());
        let arriving = !there.owns(r);
        if !arriving && !retracting {
            continue;
        }
        let taken = trx.take_in(r)?;
        if arriving {
            brought.relations += 1;
        }
        if retracting {
            taken.status()?.set(&RelationStatus::Retracted)?;
            taken.closed_by()?.set(&Some(Ref::new(by)))?;
            taken.closed()?.set(&Some(t))?;
            taken.closed_reason()?.set(&Some(why.to_string()))?;
        }
    }
    for f in held.former_ids.iter().filter(|f| !there.owns(*f)) {
        trx.take_in(f)?;
        brought.former_ids += 1;
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
    // A transaction dropped uncommitted writes nothing.
    if brought != Brought::default() || !leaving.is_empty() || joining.is_some() {
        trx.commit().await?;
    }
    Ok(brought)
}

/// Takes the task `view` shows into the destination `to`, whose graph is `there`, in one
/// transaction with the comment by `by` that records its move from `from`, with the reason given,
/// if any, and a `mention` relation to each thing the reason names, as a post writes one. Answers
/// the comment.
async fn arrive(
    there: &ProjectGraph,
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
    let trx = there.begin();
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
    /// The relations from the task and from its messages, whatever their kind or state: a step
    /// written as one of its messages among them, and no relation into it, which is its `from`
    /// end's.
    relations: Vec<RelationView>,
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
        let mut relations = graph.relations_from(task, None).await?;
        for m in &messages {
            relations.extend(graph.relations_from(m.id(), None).await?);
        }
        let ids: Vec<EntityId> =
            std::iter::once(task).chain(messages.iter().map(View::id)).chain(relations.iter().map(View::id)).collect();
        let former_ids = graph.former_ids_of(&ids).await?;
        Ok(Belongings { messages, relations, former_ids })
    }
}
