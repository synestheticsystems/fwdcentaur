//! The project a command runs in. A project is a grouping and the filter of every list, not a wall
//! (docs/DECISIONS.md, D176, D181), so a command that acts on a record of another project runs in
//! that project, as if `--project` had named it, and says so, as the page switches to the project
//! of a record it opens: a read prints the record, and a write is made there, by whoever the
//! caller is there. A command that acts on no one record runs in the project `--project` names on
//! its command line; without that, when it works in a scope of another project (`--scope`,
//! FWD_SCOPE, the MCP's `scope`), in that scope's project, since a scope names its project; and
//! otherwise in the project FWD_PROJECT or the directory names, or the MCP call's `project`. Every
//! other id a command takes is a reference, which names a record of any project and moves nothing.
//!
//! The CLI (`run_cli`) and the MCP endpoint (`mcp`) place a command here before they find whom it
//! acts as, so that its caller is found, or made, in the project it runs in, and nothing is read
//! for the caller or written in the project it started in.

use crate::{
    AnchorCommand, Command, DecisionCommand, MessageCommand, RelationCommand, ScopeCommand, StepCommand, TaskCommand,
};
use ankurah::EntityId;
use anyhow::{bail, Result};
use fwd_core::former_ids::FormerIds;
use fwd_core::model::{RelationKind, RelationView};
use fwd_core::ops::{self, Kind};
use fwd_core::project::Paths;
use fwd_core::project_graph::{Followed, ProjectGraph, Whose};
use fwd_core::rules;
use fwd_core::task_move;

const TASK: &[Kind] = &[Kind::Task];
const SCOPE: &[Kind] = &[Kind::Scope];
const DECISION: &[Kind] = &[Kind::Decision];
const MESSAGE: &[Kind] = &[Kind::Message];
const MESSAGE_OR_NOTIFICATION: &[Kind] = &[Kind::Message, Kind::Notification];
const DISCUSSED: &[Kind] = &[Kind::Task, Kind::Anchor, Kind::Scope, Kind::Decision];

/// What places a command in a project: the record it acts on, as typed on its command line.
enum PlacedBy<'a> {
    /// A record named by an id, among the kinds it may be.
    Record { typed: &'a str, kinds: &'static [Kind] },
    /// A step of a task's checklist (`step check`, `uncheck`, `remove`), named by its number in the
    /// checklist or an id among the task's steps: the step's relation, which the project that made
    /// it changes, whichever project the task is in.
    Step { task: &'a str, step: &'a str },
    /// The relations `from -kind-> to` that hold (`unlink`), which the project that made them
    /// changes, whichever projects their ends are in.
    Relations { from: &'a str, kind: RelationKind, to: &'a str },
    /// The tasks `task move` moves to the project `to`, into its scope `to_scope` when it names
    /// one, which run in the project of those not there already (`task_move::place`).
    Moving { tasks: &'a [String], to: &'a str, to_scope: Option<&'a str> },
}

/// The record `command` acts on, which places it, as typed on its command line; none for a command
/// that acts on no one record: one that makes a record (`task create`), lists, or reads the
/// caller's own inbox. `fwd link <from> <kind> <to>` acts on `from`, and `to` may be a record of
/// another project.
fn placed_by(command: &Command) -> Option<PlacedBy<'_>> {
    fn record<'a>(typed: &'a str, kinds: &'static [Kind]) -> Option<PlacedBy<'a>> {
        Some(PlacedBy::Record { typed, kinds })
    }
    match command {
        Command::Task { command } => match command {
            TaskCommand::Show { id, .. }
            | TaskCommand::Assign { id, .. }
            | TaskCommand::Unassign { id }
            | TaskCommand::Approve { id, .. }
            | TaskCommand::Reject { id, .. }
            | TaskCommand::Start { id }
            | TaskCommand::Submit { id, .. }
            | TaskCommand::Accept { id, .. }
            | TaskCommand::Return { id, .. }
            | TaskCommand::Done { id, .. }
            | TaskCommand::Cancel { id, .. }
            | TaskCommand::Reopen { id, .. }
            | TaskCommand::Rejustify { id, .. }
            | TaskCommand::Hold { id, .. }
            | TaskCommand::Release { id }
            | TaskCommand::Explain { id } => record(id, TASK),
            TaskCommand::Amend(a) => record(&a.id, TASK),
            TaskCommand::Move { ids, to, to_scope, .. } => {
                Some(PlacedBy::Moving { tasks: ids, to, to_scope: to_scope.as_deref() })
            }
            TaskCommand::Create(_) | TaskCommand::List { .. } | TaskCommand::Next { .. } => None,
        },
        Command::Step { command } => match command {
            StepCommand::Add { task, .. } | StepCommand::List { task } => record(task, TASK),
            StepCommand::Check { task, step }
            | StepCommand::Uncheck { task, step }
            | StepCommand::Remove { task, step, .. } => Some(PlacedBy::Step { task, step }),
        },
        Command::Link { from, .. } => record(from, ops::ANY),
        Command::Unlink { from, kind, to, .. } => Some(PlacedBy::Relations { from, kind: *kind, to }),
        Command::Relations { id, .. } => record(id, ops::ANY),
        Command::Relation { command: RelationCommand::Approve { id } | RelationCommand::Reject { id, .. } } => {
            record(id, &[Kind::Relation])
        }
        Command::Scope { command } => match command {
            ScopeCommand::Branches { id, .. }
            | ScopeCommand::AttachDir { id, .. }
            | ScopeCommand::Show { id }
            | ScopeCommand::Rename { id, .. }
            | ScopeCommand::Intention { id, .. }
            | ScopeCommand::Status { id, .. } => record(id, SCOPE),
            ScopeCommand::Create { .. } | ScopeCommand::List => None,
        },
        Command::Comment { first, text: Some(_), .. } => record(first, DISCUSSED),
        Command::Anchor { command: AnchorCommand::Show { id, .. } | AnchorCommand::Resolve { id, .. } } => {
            record(id, &[Kind::Anchor])
        }
        Command::Ask { about: Some(about), .. } => record(about, &[Kind::Task, Kind::Decision]),
        Command::Reply { id, .. } | Command::Forward { id, .. } => record(id, MESSAGE_OR_NOTIFICATION),
        Command::Message { command } => match command {
            MessageCommand::List { on: Some(on), .. } => record(on, DISCUSSED),
            MessageCommand::Show { id } | MessageCommand::Resolve { id } | MessageCommand::Unresolve { id } => {
                record(id, MESSAGE)
            }
            MessageCommand::Edit { id, .. } => record(id, MESSAGE_OR_NOTIFICATION),
            MessageCommand::List { on: None, .. } => None,
        },
        Command::Decision { command } => match command {
            DecisionCommand::Show { id }
            | DecisionCommand::Attach { id, .. }
            | DecisionCommand::Detach { id }
            | DecisionCommand::Accept { id, .. }
            | DecisionCommand::Reject { id, .. }
            | DecisionCommand::Repeal { id, .. } => record(id, DECISION),
            DecisionCommand::Supersede { old, .. } => record(old, DECISION),
            DecisionCommand::Create(_) | DecisionCommand::List { .. } | DecisionCommand::Export => None,
        },
        Command::Cite { entity, .. } => record(entity, &[Kind::Task, Kind::Message, Kind::Decision]),
        Command::Read { ids } | Command::Dismiss { ids } => {
            ids.first().map(|id| PlacedBy::Record { typed: id, kinds: &[Kind::Notification] })
        }
        _ => None,
    }
}

/// An id typed on a command line as placing the command resolved it: the text, the kinds it was
/// looked for among, and the record it names, which the command then takes without reading every
/// project's ids again (`App::id`).
#[derive(Debug, Clone)]
pub(crate) struct Typed {
    text: String,
    kinds: &'static [Kind],
    record: EntityId,
}

impl Typed {
    fn of(text: &str, kinds: &'static [Kind], record: EntityId) -> Typed {
        Typed { text: text.to_string(), kinds, record }
    }
}

/// Where a command runs, as [`runs_in`] places it.
pub(crate) struct RunsIn {
    /// Another project, when the record the command acts on, or the scope it works in, is that
    /// project's; otherwise the project it started in.
    pub(crate) project: Whose,
    /// What the command's note names as the record of that project: "`<id>`", "step `2` of
    /// `<task>`", "the scope `<id>`".
    pub(crate) because: String,
    /// The ids typed on the command that placing it resolved.
    pub(crate) typed: Vec<Typed>,
    /// For `task move`, the move as placing it read it, holding the node's move lock until the
    /// command ends, which makes it where it was placed (`task_move::Move`).
    pub(crate) moving: Option<task_move::Move>,
}

/// Places `command`, which started in `graph`'s project, where it runs: in the project of the
/// record it acts on ([`placed_by`]), found in whichever project it is (`ops::locate`). Without
/// one, it runs where it started when that is the project a `--project` typed on its command line
/// names (`project_typed`), which no scope overrides, the scope being then a reference like any
/// other; else in the project of `scope`, the scope it works in as typed (`--scope` or `FWD_SCOPE`
/// for the CLI, `scope` for the MCP), named by its id or the start of it; otherwise where it
/// started, the project FWD_PROJECT or the directory names, or an MCP call's `project`. A task
/// move is placed under the node's move lock, which the daemon's runtime folder in `paths` holds,
/// telling `waiting` first when another move holds it (`task_move::MoveLock`).
pub(crate) async fn runs_in(
    graph: &ProjectGraph,
    command: &Command,
    project_typed: bool,
    scope: Option<&str>,
    paths: &Paths,
    waiting: impl FnOnce(),
) -> Result<RunsIn> {
    match placed_by(command) {
        Some(PlacedBy::Record { typed, kinds }) => {
            let found = ops::locate(graph, typed, kinds).await?;
            Ok(RunsIn {
                project: found.project,
                because: format!("`{typed}`"),
                typed: vec![Typed::of(typed, kinds, found.id)],
                moving: None,
            })
        }
        Some(PlacedBy::Step { task, step }) => {
            let found = ops::locate(graph, task, TASK).await?;
            let (_, edge) = step_named(graph, found.id, step).await?;
            Ok(RunsIn {
                project: relation_project(graph, edge).await?,
                because: format!("step `{step}` of `{task}`"),
                typed: vec![Typed::of(task, TASK, found.id)],
                moving: None,
            })
        }
        Some(PlacedBy::Relations { from, kind, to }) => {
            let (start, end) = (ops::locate(graph, from, ops::ANY).await?, ops::locate(graph, to, ops::ANY).await?);
            let typed = vec![Typed::of(from, ops::ANY, start.id), Typed::of(to, ops::ANY, end.id)];
            let mut projects: Vec<Whose> = Vec::new();
            for r in ops::relations_from(graph, start.id, Some(kind)).await? {
                if r.to_id()? == end.id && rules::holds(kind, r.status()?) {
                    let project = relation_project(graph, r.id()).await?;
                    if !projects.contains(&project) {
                        projects.push(project);
                    }
                }
            }
            let project = match projects.as_slice() {
                // None holds: the command runs where `from` is, and retracts nothing.
                [] => start.project,
                [one] => one.clone(),
                several => bail!(
                    "the {kind} relations from `{from}` to `{to}` that hold were made in {} projects ({}), and a command runs in one project, so `fwd unlink` cannot retract them together",
                    several.len(),
                    several.iter().map(project_name).collect::<Vec<_>>().join(", ")
                ),
            };
            Ok(RunsIn { project, because: format!("`{from} {kind} {to}`"), typed, moving: None })
        }
        Some(PlacedBy::Moving { tasks, to, to_scope }) => {
            // Everything placing reads is read under the lock, so that a move that waited for
            // another sees what that one did, and runs where the tasks are now.
            let lock = task_move::MoveLock::take(paths, waiting).await?;
            let mut located = Vec::with_capacity(tasks.len());
            for task in tasks {
                located.push((task, ops::locate(graph, task, TASK).await?));
            }
            let records: Vec<EntityId> = located.iter().map(|(_, found)| found.id).collect();
            let destination = task_move::Destination::named(graph, to, to_scope).await?;
            let placed = task_move::place(graph, &records, destination, lock).await?;
            let project = placed.runs_in(graph);
            // The note names a task of the project the move runs in, the first typed.
            let first =
                located.iter().find(|(_, found)| found.project == project).map_or(&tasks[0], |(typed, _)| *typed);
            let typed = located.iter().map(|(task, found)| Typed::of(task, TASK, found.id)).collect();
            Ok(RunsIn { project, because: format!("`{first}`"), typed, moving: Some(placed) })
        }
        None => match scope.filter(|_| !project_typed) {
            Some(scope) => {
                let found = ops::locate(graph, scope, SCOPE).await?;
                Ok(RunsIn {
                    project: found.project,
                    because: format!("the scope `{scope}`"),
                    typed: vec![Typed::of(scope, SCOPE, found.id)],
                    moving: None,
                })
            }
            None => Ok(RunsIn { project: Whose::This, because: String::new(), typed: Vec::new(), moving: None }),
        },
    }
}

/// The project that made the relation `edge`, which the node holds.
async fn relation_project(graph: &ProjectGraph, edge: EntityId) -> Result<Whose> {
    match graph.follow::<RelationView>(edge).await? {
        Followed::Held { project, .. } => Ok(project),
        Followed::NotHeld => bail!("the node holds no relation {edge}"),
    }
}

/// A project as a refusal names it: by its name, or for the one the command started in, so.
fn project_name(project: &Whose) -> String {
    match project {
        Whose::This => "this one".into(),
        Whose::Other(p) => format!("`{}`", p.name),
    }
}

/// A step named on the command line among `task`'s checklist ([`step_at`]), its number and its
/// relation, an id it or what it is had before it was renumbered counting as its id: the former
/// ids that start with the name are read, of every project, as one range of their text.
pub(crate) async fn step_named(graph: &ProjectGraph, task: EntityId, step: &str) -> Result<(usize, EntityId)> {
    let former = if ops::may_start_an_id(step) {
        FormerIds::from_views(&graph.former_ids_starting(step).await?)
    } else {
        FormerIds::default()
    };
    step_at(&ops::steps(graph, task).await?, step, &former)
}

/// A step named on the command line among a task's `steps`: its number in the checklist (from 1),
/// or the id of its relation or of what it is (a highlight, a message, a task), whole or its first
/// six characters or more, an id either had before it was renumbered (`former`) counting as its
/// id, looked for among this checklist's steps alone. Returns its number and its relation. A name
/// that fits no step of the checklist is refused, saying so, and so is a start of an id that fits
/// several.
fn step_at(steps: &[RelationView], s: &str, former: &FormerIds) -> Result<(usize, EntityId)> {
    if let Ok(n) = s.parse::<usize>() {
        if (1..=steps.len()).contains(&n) {
            return Ok((n, steps[n - 1].id()));
        }
        if s.len() < ops::MIN_PREFIX {
            bail!("the task has {} step(s), so there is no step {n}", steps.len());
        }
    }
    let whole = s.parse::<EntityId>().ok();
    if whole.is_none() && s.len() < ops::MIN_PREFIX {
        bail!(
            "`{s}` names no step: give its number in the checklist, or the id of its relation or of what it is, whole or its first {} characters or more",
            ops::MIN_PREFIX
        );
    }
    let renumbered: Vec<EntityId> = match whole {
        Some(_) => former.named(s).into_iter().collect(),
        None => former.starting(s).map(|(_, record)| record).collect(),
    };
    let fits =
        |id: EntityId| renumbered.contains(&id) || whole.map_or_else(|| id.to_base64().starts_with(s), |w| w == id);
    let found: Vec<usize> =
        steps.iter().enumerate().filter(|(_, r)| fits(r.id()) || r.from_id().is_ok_and(fits)).map(|(i, _)| i).collect();
    match found.as_slice() {
        [i] => Ok((i + 1, steps[*i].id())),
        [] => bail!(
            "`{s}` names no step on this task's checklist; `fwd step list` lists its steps with their numbers and ids"
        ),
        several => bail!(
            "`{s}` starts the ids of steps {} of this task's checklist; give more of the id",
            several.iter().map(|i| (i + 1).to_string()).collect::<Vec<_>>().join(" and ")
        ),
    }
}

/// The record `text`, typed on the command among `kinds`, names, when placing the command resolved
/// it.
pub(crate) fn typed_record(typed: &[Typed], text: &str, kinds: &[Kind]) -> Option<EntityId> {
    typed.iter().find(|t| t.text == text && t.kinds == kinds).map(|t| t.record)
}

/// The scope a command works in, typed as `scope` (`--scope`, `FWD_SCOPE`, the MCP's `scope`): as
/// placing the command found it, else read in `graph` as any id typed is, in whichever project it is
/// (`ops::resolve_id`).
pub(crate) async fn scope_named(graph: &ProjectGraph, typed: &[Typed], scope: &str) -> Result<EntityId> {
    match typed_record(typed, scope, SCOPE) {
        Some(record) => Ok(record),
        None => ops::resolve_id(graph, scope, SCOPE).await,
    }
}
