//! The rules: readiness, cycles, the scope a checkout finds, starting a task, steps and the threads
//! of their highlights, who takes a step off a checklist and what retiring a highlight moves,
//! acceptance, task transitions, decision moves and who edits a message. Pure functions over small
//! snapshots, so the same code runs as a pre-check in the CLI (to explain a refusal before
//! committing) and in the daemon's validation service (to catch races and misbehaving clients after
//! the fact). See docs/design/data-model.md, "Rules".

use crate::model::{DecisionStatus, MessageStatus, RelationKind, RelationStatus, ScopeStatus, TaskStatus};
use ankurah::EntityId;
use std::collections::{hash_map::Entry, HashMap, HashSet};

/// What the rules need to know about a task.
#[derive(Debug, Clone)]
pub struct TaskSnapshot {
    pub id: EntityId,
    pub status: TaskStatus,
    pub hold: Option<String>,
}

/// What the rules need to know about a scope a task is worked in, for a refusal to name it and
/// say how to switch to it: its id, its name and its status (none when the scope could not be
/// read). A scope has no owner (docs/DECISIONS.md, D108, "A scope has no owner: scopes are not
/// assigned to principals"), so nothing here says whose it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeSnapshot {
    pub id: EntityId,
    pub title: String,
    pub status: Option<ScopeStatus>,
}

/// An active scope as a checkout finds it ([`scope_for_checkout`]): its id and where its work
/// happens, from its `context` (`ops::scope_branches` reads the branches).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopePlace {
    pub id: EntityId,
    /// The repository its branches are in, by its upstream's key; none on a scope made before
    /// repositories were named on scopes, 2026-09-25, or on one that names no branch.
    pub repo: Option<String>,
    pub branches: Vec<String>,
    /// The directory a scope made before 2026-09-25 recorded in place of its repository.
    pub root: Option<String>,
}

/// Where a checkout is, as [`scope_for_checkout`] matches it against scopes.
#[derive(Debug, Clone, Copy)]
pub struct CheckoutPlace<'a> {
    /// The branch it has checked out; none when its HEAD is detached.
    pub branch: Option<&'a str>,
    /// Its repository, by its upstream's key; none without an origin remote.
    pub upstream: Option<&'a str>,
    /// The directory the command runs in.
    pub cwd: &'a std::path::Path,
}

/// The scope a checkout finds for a command that names none ([`scope_for_checkout`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeHere {
    /// No active scope is bound to the checkout, and none names its branch.
    None,
    /// The scope the command works in.
    One(EntityId),
    /// Several active scopes match the checkout at the same step, as a checkout of `main` does
    /// where two scopes name `main`: `branch` is the branch they name, or none when they matched by
    /// the directory an old scope recorded. Only a command that needs a scope refuses to run
    /// ([`RuleError::SeveralScopesHere`]); every other runs with none.
    Several { branch: Option<String>, scopes: Vec<EntityId> },
}

/// What the rules need to know about a relation.
#[derive(Debug, Clone)]
pub struct Edge {
    pub id: EntityId,
    pub kind: RelationKind,
    pub from: EntityId,
    pub to: EntityId,
    pub status: RelationStatus,
}

impl Edge {
    fn counts(&self, kind: RelationKind) -> bool {
        self.kind == kind && holds(kind, self.status)
    }
}

/// Whether a relation of `kind` in `status` holds for the rules: accepted, or, for a step, checked
/// as well, since a checked step is still a step of its task.
pub fn holds(kind: RelationKind, status: RelationStatus) -> bool {
    status == RelationStatus::Accepted || (kind == RelationKind::StepOf && status == RelationStatus::Resolved)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotReadyReason {
    /// The task is not `Open`.
    Status(TaskStatus),
    /// A manual hold, with its reason.
    Held(String),
    /// An accepted `depends_on` target that is not `Done`.
    BlockedBy(EntityId),
    /// An accepted `depends_on` target that is `Cancelled` or `Rejected`; the triage agent must
    /// retract the relation or cancel the dependent.
    DependencyClosed(EntityId, TaskStatus),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnknownReason {
    /// An accepted `depends_on` target that could not be loaded or is not authorized.
    DependencyUnavailable(EntityId),
}

/// Three-valued, following Manifold: the CLI never reports "no blockers" for an incomplete
/// domain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Readiness {
    Ready,
    NotReady(Vec<NotReadyReason>),
    Unknown(Vec<UnknownReason>),
}

impl Readiness {
    pub fn is_ready(&self) -> bool {
        matches!(self, Readiness::Ready)
    }
}

/// `deps` holds one entry per accepted `depends_on` edge leaving the task: the target id and its
/// status, or `None` when the target is unavailable. Known failures win over unknowns
/// (Manifold's `all`: unsatisfied if any child is unsatisfied, satisfied if all are, otherwise
/// unknown).
pub fn readiness(task: &TaskSnapshot, deps: &[(EntityId, Option<TaskStatus>)]) -> Readiness {
    let mut not_ready = Vec::new();
    let mut unknown = Vec::new();
    if task.status != TaskStatus::Open {
        not_ready.push(NotReadyReason::Status(task.status));
    }
    if let Some(reason) = &task.hold {
        not_ready.push(NotReadyReason::Held(reason.clone()));
    }
    for (target, status) in deps {
        match status {
            Some(TaskStatus::Done) => {}
            Some(closed @ (TaskStatus::Cancelled | TaskStatus::Rejected)) => {
                not_ready.push(NotReadyReason::DependencyClosed(*target, *closed))
            }
            Some(_) => not_ready.push(NotReadyReason::BlockedBy(*target)),
            None => unknown.push(UnknownReason::DependencyUnavailable(*target)),
        }
    }
    if !not_ready.is_empty() {
        Readiness::NotReady(not_ready)
    } else if !unknown.is_empty() {
        Readiness::Unknown(unknown)
    } else {
        Readiness::Ready
    }
}

/// The relation kinds that must stay acyclic over the edges that hold ([`holds`]): a step may be
/// a task, and a task cannot be a step of itself, however far round.
pub fn must_be_acyclic(kind: RelationKind) -> bool {
    matches!(kind, RelationKind::DependsOn | RelationKind::SubtaskOf | RelationKind::StepOf)
}

/// If adding an accepted `from -> to` edge of `kind` would close a cycle, returns the cycle as a
/// path of task ids starting and ending at `from` (`[from, to, ..., from]`). A self-edge is the
/// cycle `[from, from]`. Kinds that may cycle never refuse.
pub fn would_close_cycle(kind: RelationKind, from: EntityId, to: EntityId, edges: &[Edge]) -> Option<Vec<EntityId>> {
    if !must_be_acyclic(kind) {
        return None;
    }
    if from == to {
        return Some(vec![from, from]);
    }
    // Search for a path to -> ... -> from over accepted edges of this kind.
    let mut adjacency: HashMap<EntityId, Vec<EntityId>> = HashMap::new();
    for e in edges.iter().filter(|e| e.counts(kind)) {
        adjacency.entry(e.from).or_default().push(e.to);
    }
    let mut parent: HashMap<EntityId, EntityId> = HashMap::new();
    let mut stack = vec![to];
    let mut seen: HashSet<EntityId> = HashSet::from([to]);
    while let Some(node) = stack.pop() {
        if node == from {
            let mut path = vec![from];
            let mut cur = from;
            while cur != to {
                cur = parent[&cur];
                path.push(cur);
            }
            path.push(from);
            path.reverse();
            // path now reads to ... from; present it as from, to, ..., from
            let mut cycle = vec![from];
            cycle.extend(path.into_iter().skip(1).rev().skip(1).collect::<Vec<_>>().into_iter().rev());
            cycle.push(from);
            return Some(cycle);
        }
        for next in adjacency.get(&node).into_iter().flatten() {
            if seen.insert(*next) {
                parent.insert(*next, node);
                stack.push(*next);
            }
        }
    }
    None
}

/// Every cycle currently present among accepted edges of `kind`, as strongly connected
/// components with more than one member (or a self-edge). Used by the validation service for
/// cycles that arrive through sync; no edge is ever dropped automatically.
pub fn find_cycles(kind: RelationKind, edges: &[Edge]) -> Vec<Vec<EntityId>> {
    let mut adjacency: HashMap<EntityId, Vec<EntityId>> = HashMap::new();
    let mut nodes: Vec<EntityId> = Vec::new();
    for e in edges.iter().filter(|e| e.counts(kind)) {
        adjacency.entry(e.from).or_default().push(e.to);
        for n in [e.from, e.to] {
            if !nodes.contains(&n) {
                nodes.push(n);
            }
        }
    }
    // Tarjan's SCC, iterative to keep deep graphs off the stack.
    let mut index = 0usize;
    let mut indices: HashMap<EntityId, usize> = HashMap::new();
    let mut lowlink: HashMap<EntityId, usize> = HashMap::new();
    let mut on_stack: HashSet<EntityId> = HashSet::new();
    let mut stack: Vec<EntityId> = Vec::new();
    let mut result: Vec<Vec<EntityId>> = Vec::new();
    let empty: Vec<EntityId> = Vec::new();

    for &root in &nodes {
        let Entry::Vacant(slot) = indices.entry(root) else { continue };
        slot.insert(index);
        let mut work: Vec<(EntityId, usize)> = vec![(root, 0)];
        lowlink.insert(root, index);
        index += 1;
        stack.push(root);
        on_stack.insert(root);
        while let Some((node, next_child)) = work.pop() {
            let children = adjacency.get(&node).unwrap_or(&empty);
            if next_child < children.len() {
                let child = children[next_child];
                work.push((node, next_child + 1));
                if let Entry::Vacant(slot) = indices.entry(child) {
                    slot.insert(index);
                    lowlink.insert(child, index);
                    index += 1;
                    stack.push(child);
                    on_stack.insert(child);
                    work.push((child, 0));
                } else if on_stack.contains(&child) {
                    let low = lowlink[&node].min(indices[&child]);
                    lowlink.insert(node, low);
                }
            } else {
                if let Some(&(parent_node, _)) = work.last() {
                    let low = lowlink[&parent_node].min(lowlink[&node]);
                    lowlink.insert(parent_node, low);
                }
                if lowlink[&node] == indices[&node] {
                    let mut component = Vec::new();
                    while let Some(member) = stack.pop() {
                        on_stack.remove(&member);
                        component.push(member);
                        if member == node {
                            break;
                        }
                    }
                    let self_loop =
                        component.len() == 1 && adjacency.get(&node).map(|c| c.contains(&node)).unwrap_or(false);
                    if component.len() > 1 || self_loop {
                        component.reverse();
                        result.push(component);
                    }
                }
            }
        }
    }
    result
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RuleError {
    #[error("task is {status}; only an open task can be started")]
    NotOpen { status: TaskStatus },
    #[error("task is assigned to {assignee}; only its assignee starts it (reassign it first)")]
    AssignedToAnother { assignee: EntityId },
    /// A start from a scope the task is not worked in: `scopes` holds every scope it is worked in.
    #[error("{}", worked_elsewhere(.scopes))]
    WorkedInAnotherScope { scopes: Vec<ScopeSnapshot> },
    #[error("{}", several_here(.branch.as_deref(), .scopes))]
    SeveralScopesHere { branch: Option<String>, scopes: Vec<(EntityId, String)> },
    #[error("only a step is checked or unchecked; this relation is {kind}")]
    NotAStep { kind: RelationKind },
    #[error("this step is {status}; only a step that stands is checked or unchecked")]
    StepNotStanding { status: RelationStatus },
    #[error("the submitter cannot accept their own submission")]
    SelfAcceptance,
    #[error("{verb:?} is not allowed from {from}")]
    InvalidTransition { from: TaskStatus, verb: Verb },
    #[error("only {} decisions can be {}; this one is {from}", .mv.moves_from(), .mv.participle())]
    InvalidDecisionMove { from: DecisionStatus, mv: DecisionMove },
    #[error("a decision cannot supersede itself")]
    SupersedesItself,
    #[error("the superseding decision is {status}; accept it first, so that a ruling stays in force")]
    SuccessorNotAccepted { status: DecisionStatus },
    #[error("{}", neither_author_nor_successor(.author, .successor.as_ref()))]
    NeitherAuthorNorSuccessor {
        author: EntityId,
        /// The last of the author's successors, the agent at work in the author's place, which may
        /// edit the message; none when no agent succeeded the author.
        successor: Option<EntityId>,
    },
    #[error(
        "only the task's creator ({}) and its assignee ({}) take a step off its checklist",
        short_id(.creator),
        .assignee.as_ref().map_or_else(|| "no one".to_string(), short_id)
    )]
    NotTheTasksPeople { creator: EntityId, assignee: Option<EntityId> },
    #[error(
        "notification {} is {}'s, and a principal reads, dismisses, replies through or forwards only its own notifications",
        short_id(.notification),
        short_id(.recipient)
    )]
    NotTheRecipient { notification: EntityId, recipient: EntityId },
}

/// A message says what its author said, so its author edits it, and so does an agent that
/// succeeded its author: an agent relaunched to carry on the author's job answers for what the
/// author wrote, and corrects an answer its predecessor gave where the answer stands rather than
/// beside it (docs/DECISIONS.md, D114, a relaunched agent's user succeeds its predecessor's; task
/// sVCUXcEg). `successors` are the principals that took over the author's job in turn, the
/// author's successor first and the last one last (`routing::Successors::chain`), so an agent
/// that succeeded the author through a chain of successions edits it too. Anyone else answers the
/// message with a reply, which keeps who said what.
pub fn can_edit_message(author: EntityId, successors: &[EntityId], editor: EntityId) -> Result<(), RuleError> {
    if editor == author || successors.contains(&editor) {
        Ok(())
    } else {
        Err(RuleError::NeitherAuthorNorSuccessor { author, successor: successors.last().copied() })
    }
}

/// The refusal of an edit by a principal that neither wrote the message nor succeeded its author:
/// the author by the start of its id and, when an agent succeeded it, the last of its successors,
/// which carries on the author's job now.
fn neither_author_nor_successor(author: &EntityId, successor: Option<&EntityId>) -> String {
    let written_by = match successor {
        Some(successor) => format!("{}, whom {} succeeded", short_id(author), short_id(successor)),
        None => short_id(author),
    };
    format!(
        "only its author edits a message, or an agent that succeeded its author, and this one was written by {written_by}; reply to it instead"
    )
}

/// A task's checklist belongs to the task's creator, who set the work out, and its assignee, who
/// does it: they are the ones who hear of its steps (docs/design/agents-and-inbox.md, rules 10
/// and 14). So they alone take a step off it (task 9dvGW0EI, "Take a step off a checklist and
/// retire its highlight in one command: fwd step remove"), and anyone else asks one of them.
pub fn can_remove_step(creator: EntityId, assignee: Option<EntityId>, remover: EntityId) -> Result<(), RuleError> {
    if remover == creator || assignee == Some(remover) {
        Ok(())
    } else {
        Err(RuleError::NotTheTasksPeople { creator, assignee })
    }
}

/// A notification is one inbox item for one recipient (docs/design/data-model.md, Notification),
/// so its recipient alone reads it, dismisses it, or settles it by replying through it or
/// forwarding it, as the page marks no card of another principal's read (task AfhuiHYS): a
/// principal that mishandled an id would otherwise take a notification out of another's unread
/// inbox unseen. A successor is no exception: the notifications its predecessor left unread moved
/// to it, and those that stayed are the predecessor's. Anyone else answers the message behind a
/// notification by the message's own id, which settles nothing.
pub fn can_settle(notification: EntityId, recipient: EntityId, by: EntityId) -> Result<(), RuleError> {
    if by == recipient {
        Ok(())
    } else {
        Err(RuleError::NotTheRecipient { notification, recipient })
    }
}

/// Starting a task makes whoever starts it its assignee, so it is refused unless the task is open
/// and is unassigned or already theirs: two people never hold one task, and a task assigned to
/// someone is reassigned before anyone else starts it.
pub fn can_start(status: TaskStatus, assignee: Option<EntityId>, starter: EntityId) -> Result<(), RuleError> {
    if status != TaskStatus::Open {
        return Err(RuleError::NotOpen { status });
    }
    match assignee {
        Some(assignee) if assignee != starter => Err(RuleError::AssignedToAnother { assignee }),
        _ => Ok(()),
    }
}

/// A start never moves a task out of the scopes someone chose for it (docs/DECISIONS.md, D87,
/// "Starting a task worked in another scope is refused, with a prompt to switch to that scope
/// first", 2026-09-28: "It should probably reject the starting of the task if the scope is
/// different than the agent's current scope. It should prompt them to switch scopes before
/// starting the task."). `scope` is the scope the starter works in, and `worked_in` holds the
/// scopes the task is worked in (the targets of its accepted `worked_in` relations, any number of
/// them since D136, "A task may be worked in several scopes", 2026-10-03). A start from any of the
/// task's scopes goes through, and one from a scope that is none of them is refused, naming each
/// of them and how to switch to one, whoever made it. A task worked in no scope starts in the
/// starter's, and a starter who works in no scope (a person need not have one) starts a task
/// wherever it is worked.
pub fn can_start_in(scope: Option<EntityId>, worked_in: &[ScopeSnapshot]) -> Result<(), RuleError> {
    let Some(here) = scope else { return Ok(()) };
    if worked_in.is_empty() || worked_in.iter().any(|s| s.id == here) {
        return Ok(());
    }
    Err(RuleError::WorkedInAnotherScope { scopes: worked_in.to_vec() })
}

/// The scope a command works in when it names none (`--scope`), found from its checkout, so that
/// an agent in its own worktree need not name it each time: the scope the checkout is bound to
/// (`bound`, from `fwd scope attach-dir`) while that scope is active; else the one active scope
/// that names the checkout's branch in the checkout's repository (a scope made before
/// repositories were named on scopes counts when the directory it recorded contains the one the
/// command runs in); else the one active scope whose recorded directory contains it. `active`
/// holds every active scope of the project, whoever made it: anyone works in any scope
/// (docs/DECISIONS.md, D108, "A scope has no owner: scopes are not assigned to principals"; until
/// 2026-09-29 only the caller's own scopes counted, and a binding only for its scope's owner). So
/// a binding wins over a branch, and several scopes at one step, as two scopes naming `main` are
/// for a checkout of `main`, are [`ScopeHere::Several`], which a command that needs a scope
/// refuses and every other runs without one.
pub fn scope_for_checkout(bound: Option<EntityId>, active: &[ScopePlace], checkout: CheckoutPlace) -> ScopeHere {
    if let Some(bound) = bound.filter(|b| active.iter().any(|s| s.id == *b)) {
        return ScopeHere::One(bound);
    }
    let recorded_here = |s: &ScopePlace| s.root.as_deref().is_some_and(|root| checkout.cwd.starts_with(root));
    let one_of = |scopes: Vec<EntityId>, branch: Option<&str>| match scopes.as_slice() {
        [] => None,
        [one] => Some(ScopeHere::One(*one)),
        _ => Some(ScopeHere::Several { branch: branch.map(str::to_string), scopes }),
    };
    if let Some(branch) = checkout.branch {
        let by_branch: Vec<EntityId> = active
            .iter()
            .filter(|s| {
                let same_repo = match (s.repo.as_deref(), checkout.upstream) {
                    (Some(repo), Some(upstream)) => repo == upstream,
                    (Some(_), None) => false,
                    (None, _) => recorded_here(s),
                };
                same_repo && s.branches.iter().any(|b| b == branch)
            })
            .map(|s| s.id)
            .collect();
        if let Some(found) = one_of(by_branch, Some(branch)) {
            return found;
        }
    }
    let by_dir: Vec<EntityId> = active.iter().filter(|s| recorded_here(s)).map(|s| s.id).collect();
    one_of(by_dir, None).unwrap_or(ScopeHere::None)
}

/// The refusal of a command that needs a scope in a checkout several scopes match: each scope by
/// its name and the start of its id, and the two ways to say which one the command works in.
fn several_here(branch: Option<&str>, scopes: &[(EntityId, String)]) -> String {
    let named: Vec<String> = scopes.iter().map(|(id, title)| format!("\"{title}\" ({})", short_id(id))).collect();
    let listed = listed(&named, "and");
    let which = match branch {
        Some(branch) => format!("several active scopes name this checkout's branch {branch}"),
        None => "several active scopes hold this checkout".to_string(),
    };
    format!(
        "{which}: {listed}; say which you work in: run the command again with `--scope <id>`, or bind this checkout to one with `fwd scope attach-dir <id>`"
    )
}

/// The refusal of a start from a scope the task is not worked in ([`can_start_in`]): each scope the
/// task is worked in, by its name and the start of its id, and how to switch to one of them.
/// `--scope` works for any scope; a checkout's binding (`fwd scope attach-dir`) is honoured only
/// while its scope is active, whoever made it, so the refusal offers it only for the active ones.
fn worked_elsewhere(scopes: &[ScopeSnapshot]) -> String {
    let named: Vec<String> = scopes.iter().map(|s| format!("\"{}\" ({})", s.title, short_id(&s.id))).collect();
    let flags: Vec<String> = scopes.iter().map(|s| format!("`--scope {}`", short_id(&s.id))).collect();
    let bindings: Vec<String> = scopes
        .iter()
        .filter(|s| s.status == Some(ScopeStatus::Active))
        .map(|s| format!("`fwd scope attach-dir {}`", short_id(&s.id)))
        .collect();
    let (is_in, switch_to, bind_to) = match scopes {
        [_] => ("the scope", "that scope", "the scope"),
        _ => ("the scopes", "one of them", "one"),
    };
    let again = format!("run the command again with {}", listed(&flags, "or"));
    let how = if bindings.is_empty() {
        again
    } else {
        format!("{again}, or bind this checkout to {bind_to} with {}", listed(&bindings, "or"))
    };
    format!(
        "task is in {is_in} {}, not in the one you work in; to start it, switch to {switch_to} first: {how}",
        listed(&named, "and")
    )
}

/// `items` as a sentence lists them: "a", "a and b", "a, b and c", with `and` (or another word)
/// before the last.
fn listed(items: &[String], and: &str) -> String {
    match items.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} {and} {last}", rest.join(", ")),
        None => String::new(),
    }
}

/// The first eight characters of an id: what the page shows, and enough for any id argument of
/// the CLI to take (docs/DECISIONS.md, D35, "Short ids: the page shows the first characters of an
/// id, and the CLI takes them").
fn short_id(id: &EntityId) -> String {
    id.to_base64().chars().take(8).collect()
}

/// Checking a step moves its `step_of` edge to Resolved, and unchecking moves it back to Accepted
/// (docs/DECISIONS.md, "The checked state is the step edge's status", 2026-09-28). Returns the
/// status the edge moves to, or none when it is already there. No other kind of relation is
/// checked, so none but a step is ever Resolved; a proposed, retracted or rejected step is not a
/// step of the task's checklist, and is neither.
pub fn check_step(
    kind: RelationKind,
    status: RelationStatus,
    check: bool,
) -> Result<Option<RelationStatus>, RuleError> {
    if kind != RelationKind::StepOf {
        return Err(RuleError::NotAStep { kind });
    }
    match (status, check) {
        (RelationStatus::Accepted, true) => Ok(Some(RelationStatus::Resolved)),
        (RelationStatus::Resolved, false) => Ok(Some(RelationStatus::Accepted)),
        (RelationStatus::Resolved, true) | (RelationStatus::Accepted, false) => Ok(None),
        (status, _) => Err(RuleError::StepNotStanding { status }),
    }
}

/// A message on a highlight, as [`move_step_and_thread`] reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadMessage {
    pub id: EntityId,
    /// Whether it answers another message (a reply or a forward: its `thread` or its `reply_to` is
    /// set), and so starts no thread of its own.
    pub answers: bool,
    pub status: MessageStatus,
    pub created: i64,
}

/// A relation as [`move_step_and_thread`] reads it: one of a highlight's `step_of` relations, or
/// the relation someone asked to check, which the rule refuses when it is no standing step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepEdge {
    pub id: EntityId,
    pub kind: RelationKind,
    pub status: RelationStatus,
}

/// The first thread on a highlight, the one whose first message moves with the highlight's steps
/// ([`move_step_and_thread`]): of the messages on the highlight, the one started first that answers
/// no other message and is not deleted, the lower id first between two started at the same moment.
/// None while no one has started a thread on it. Later threads, and every reply, are not it.
pub fn first_thread(on_highlight: &[ThreadMessage]) -> Option<EntityId> {
    on_highlight
        .iter()
        .filter(|m| !m.answers && m.status != MessageStatus::Deleted)
        .min_by_key(|m| (m.created, m.id))
        .map(|m| m.id)
}

/// What a person moved, which [`move_step_and_thread`] moves the rest with: a message they
/// resolved or set back to active, or a step they checked or unchecked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moved<'a> {
    Message(&'a ThreadMessage),
    Step(&'a StepEdge),
}

/// The statuses a move writes, each only where it changes: at most one message's, and the steps'.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    pub message: Option<(EntityId, MessageStatus)>,
    pub steps: Vec<(EntityId, RelationStatus)>,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.message.is_none() && self.steps.is_empty()
    }
}

/// A highlight's steps and the first thread on it move together, so that a task's checklist and
/// the conversation on its lines never disagree about whether the lines are settled
/// (docs/DECISIONS.md, D99, "Checking a step resolves its highlight's first thread, and resolving
/// that thread checks the step; each control says it does both", which superseded D91). `done` is
/// true for resolving and checking, false for setting back to active and unchecking.
///
/// - Resolving the first message of the highlight's first thread ([`first_thread`]) checks every
///   step the highlight is, on every task's checklist it is on; setting it back to active unchecks
///   them.
/// - Checking a step resolves that first message, and unchecking it sets the message back to
///   active. The highlight's steps on other tasks' checklists move with it. D99 speaks of one
///   step and its thread; this is the rule's reading of "never disagree" for a highlight that is a
///   step of several tasks, whose one first thread cannot be resolved for one checklist and open
///   for another. So a highlight's steps and its first thread are one state, whichever is moved.
/// - A step whose highlight has no thread yet moves alone, as does any other message: a later
///   thread's first message, a reply, a message on no highlight.
///
/// `on_highlight` holds the messages on the highlight, the moved message among them, and `steps`
/// the highlight's `step_of` relations; both are empty for a message or a step on no highlight. A
/// relation there that is no standing step (proposed, retracted, rejected) stays as it is; a moved
/// step that is none is refused, as [`check_step`] refuses it, and so is a moved relation of
/// another kind. A member already where the move takes it is left as it is, so its record of who
/// moved it and when stays; the caller writes `by` and the time on each member that moves (a
/// message's `resolved_by` and `resolved`, a step's `closed_by` and `closed`), or clears them.
pub fn move_step_and_thread(
    moved: Moved,
    done: bool,
    on_highlight: &[ThreadMessage],
    steps: &[StepEdge],
) -> Result<Changes, RuleError> {
    let first = first_thread(on_highlight).and_then(|id| on_highlight.iter().find(|m| m.id == id));
    let mut changes = Changes::default();
    let (message, together) = match moved {
        Moved::Message(m) => (Some(m), first.is_some_and(|f| f.id == m.id)),
        Moved::Step(step) => {
            if let Some(status) = check_step(step.kind, step.status, done)? {
                changes.steps.push((step.id, status));
            }
            (first, first.is_some())
        }
    };
    let status = if done { MessageStatus::Resolved } else { MessageStatus::Active };
    if let Some(m) = message.filter(|m| m.status != status) {
        changes.message = Some((m.id, status));
    }
    if together {
        let moved_step = match moved {
            Moved::Step(step) => Some(step.id),
            Moved::Message(_) => None,
        };
        for step in steps.iter().filter(|s| Some(s.id) != moved_step) {
            if let Ok(Some(status)) = check_step(step.kind, step.status, done) {
                changes.steps.push((step.id, status));
            }
        }
    }
    Ok(changes)
}

/// What retiring a highlight moves ([`retire_highlight`]): the first message of each thread it
/// resolves, oldest first, and each of the highlight's steps it checks with the first thread.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Retired {
    pub messages: Vec<EntityId>,
    pub steps: Vec<(EntityId, RelationStatus)>,
}

impl Retired {
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty() && self.steps.is_empty()
    }
}

/// Retiring a highlight resolves every thread on it, so that the editor, which draws a highlight
/// while a thread on it is open, stops drawing lines no one is talking about any more; the threads
/// stay on the highlight, and its page lists them, resolved (task 9dvGW0EI, "Take a step off a
/// checklist and retire its highlight in one command: fwd step remove"). A highlight is retired as
/// its step `leaving` comes off a checklist, and the rest moves as resolving the first message of
/// each thread in turn moves it ([`move_step_and_thread`]): resolving the first thread checks every
/// step the highlight still is, on other tasks' checklists, since a highlight's steps and its first
/// thread are one state (docs/DECISIONS.md, D99, "Checking a step resolves its highlight's first
/// thread, and resolving that thread checks the step; each control says it does both"), and a
/// first thread resolved already checks them too, as resolving it again does. `on_highlight` holds
/// the messages on the highlight and `steps` its `step_of` relations, as [`move_step_and_thread`]
/// reads them; the step leaving stays as it is, and so do a reply and a deleted message, which
/// start no thread.
pub fn retire_highlight(
    on_highlight: &[ThreadMessage],
    steps: &[StepEdge],
    leaving: EntityId,
) -> Result<Retired, RuleError> {
    let mut messages = on_highlight.to_vec();
    let mut steps: Vec<StepEdge> = steps.iter().filter(|s| s.id != leaving).cloned().collect();
    let mut roots: Vec<ThreadMessage> =
        messages.iter().filter(|m| !m.answers && m.status != MessageStatus::Deleted).cloned().collect();
    roots.sort_by_key(|m| (m.created, m.id));
    let mut retired = Retired::default();
    for root in &roots {
        let moved = move_step_and_thread(Moved::Message(root), true, &messages, &steps)?;
        if let Some((id, status)) = moved.message {
            messages.iter_mut().filter(|m| m.id == id).for_each(|m| m.status = status);
            retired.messages.push(id);
        }
        for (id, status) in moved.steps {
            steps.iter_mut().filter(|s| s.id == id).for_each(|s| s.status = status);
            retired.steps.push((id, status));
        }
    }
    Ok(retired)
}

/// With `needs_review`, `accept` must come from a principal other than the one who submitted.
pub fn can_accept(submitter: EntityId, accepter: EntityId) -> Result<(), RuleError> {
    if submitter == accepter {
        Err(RuleError::SelfAcceptance)
    } else {
        Ok(())
    }
}

/// The verbs the CLI exposes; the status enum is never set directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Approve,
    Reject,
    Start,
    Submit,
    Accept,
    Return,
    Done,
    Cancel,
    Reopen,
    Rejustify,
}

/// The transition table from docs/design/data-model.md.
pub fn transition(from: TaskStatus, verb: Verb, needs_review: bool) -> Result<TaskStatus, RuleError> {
    use TaskStatus::*;
    let to = match (from, verb) {
        (Proposed, Verb::Approve) => Open,
        (Proposed, Verb::Reject) => Rejected,
        (Open, Verb::Start) => InProgress,
        (InProgress, Verb::Submit) => {
            if needs_review {
                InReview
            } else {
                Done
            }
        }
        (InReview, Verb::Accept) => Done,
        (InReview, Verb::Return) => InProgress,
        (Open | InProgress, Verb::Done) if !needs_review => Done,
        (Open | InProgress | InReview, Verb::Cancel) => Cancelled,
        (Done | Cancelled | Rejected, Verb::Reopen) => Open,
        (Open | InProgress, Verb::Rejustify) => Proposed,
        _ => return Err(RuleError::InvalidTransition { from, verb }),
    };
    Ok(to)
}

/// How a decision's status moves; its status is never set directly. Every move records its
/// reason as a comment on the decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionMove {
    Accept,
    Reject,
    Supersede,
    Repeal,
}

impl DecisionMove {
    /// The status a decision must have for this move.
    pub fn moves_from(self) -> DecisionStatus {
        match self {
            DecisionMove::Accept | DecisionMove::Reject => DecisionStatus::Proposed,
            DecisionMove::Supersede | DecisionMove::Repeal => DecisionStatus::Accepted,
        }
    }

    /// The status the move leaves it in.
    pub fn moves_to(self) -> DecisionStatus {
        match self {
            DecisionMove::Accept => DecisionStatus::Accepted,
            DecisionMove::Reject => DecisionStatus::Rejected,
            DecisionMove::Supersede => DecisionStatus::Superseded,
            DecisionMove::Repeal => DecisionStatus::Repealed,
        }
    }

    /// "accepted", "rejected", "superseded", "repealed": how the move reads in a sentence.
    pub fn participle(self) -> String {
        self.moves_to().to_string()
    }

    /// The move that closed a decision in `status`, whose comment says why it closed: rejecting,
    /// superseding or repealing it. None for a proposal and a ruling in force, which are open.
    pub fn closing(status: DecisionStatus) -> Option<DecisionMove> {
        [DecisionMove::Reject, DecisionMove::Supersede, DecisionMove::Repeal]
            .into_iter()
            .find(|mv| mv.moves_to() == status)
    }
}

/// A proposal is accepted or rejected; a ruling in force is superseded or repealed. Nothing else
/// moves: a rejected, superseded or repealed decision stays so, and a new decision takes its place.
pub fn decision_move(from: DecisionStatus, mv: DecisionMove) -> Result<DecisionStatus, RuleError> {
    if from == mv.moves_from() {
        Ok(mv.moves_to())
    } else {
        Err(RuleError::InvalidDecisionMove { from, mv })
    }
}

/// A supersession swaps one ruling in force for another, so the successor must be a different
/// decision and must itself be accepted: a proposal cannot replace a ruling before it is one.
pub fn can_supersede(old: EntityId, new: EntityId, new_status: DecisionStatus) -> Result<(), RuleError> {
    if old == new {
        return Err(RuleError::SupersedesItself);
    }
    if new_status != DecisionStatus::Accepted {
        return Err(RuleError::SuccessorNotAccepted { status: new_status });
    }
    Ok(())
}

/// The number a new decision takes: one more than the highest any decision of the project has,
/// so 1 for its first. A number is never reused, since decisions are never deleted.
pub fn next_decision_number(existing: impl IntoIterator<Item = i64>) -> i64 {
    existing.into_iter().max().unwrap_or(0).max(0) + 1
}

/// Whether a ruling attached to the scope `governs` (none for one that governs the whole project)
/// applies to the work of `scope`: the project's rulings apply to every scope's work, and a
/// scope's own to its work alone (docs/DECISIONS.md, D93, "A decision may be attached to the scope
/// it governs; one attached to none governs the whole project, and its area is its type",
/// 2026-09-28: "In any context, the decisions that apply are the project-wide ones and those
/// attached to the scope in view").
pub fn applies_to(governs: Option<EntityId>, scope: EntityId) -> bool {
    governs.is_none_or(|governed| governed == scope)
}

/// `D12` (or `d12`) names decision 12; anything else is not a decision number. An entity id is
/// 43 characters of base64, so `D` and a few digits is never a whole one (it may begin one, which
/// `ops::resolve_id` tries after the number).
pub fn decision_number(s: &str) -> Option<i64> {
    let digits = s.strip_prefix('D').or_else(|| s.strip_prefix('d'))?;
    if digits.is_empty() || digits.len() > 18 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id() -> EntityId {
        EntityId::random()
    }
    fn edge(kind: RelationKind, from: EntityId, to: EntityId, status: RelationStatus) -> Edge {
        Edge { id: id(), kind, from, to, status }
    }
    fn open(id: EntityId) -> TaskSnapshot {
        TaskSnapshot { id, status: TaskStatus::Open, hold: None }
    }

    #[test]
    fn ready_when_open_unheld_and_all_dependencies_done() {
        let t = open(id());
        assert_eq!(readiness(&t, &[]), Readiness::Ready);
        assert_eq!(readiness(&t, &[(id(), Some(TaskStatus::Done))]), Readiness::Ready);
    }

    #[test]
    fn not_ready_reasons_are_listed() {
        let dep = id();
        let t = TaskSnapshot { id: id(), status: TaskStatus::Proposed, hold: Some("waiting on design".into()) };
        let r = readiness(&t, &[(dep, Some(TaskStatus::InProgress))]);
        assert_eq!(
            r,
            Readiness::NotReady(vec![
                NotReadyReason::Status(TaskStatus::Proposed),
                NotReadyReason::Held("waiting on design".into()),
                NotReadyReason::BlockedBy(dep),
            ])
        );
    }

    #[test]
    fn closed_dependency_is_a_distinct_reason() {
        let dep = id();
        let r = readiness(&open(id()), &[(dep, Some(TaskStatus::Cancelled))]);
        assert_eq!(r, Readiness::NotReady(vec![NotReadyReason::DependencyClosed(dep, TaskStatus::Cancelled)]));
    }

    #[test]
    fn incomplete_domain_is_unknown_never_ready() {
        let dep = id();
        assert_eq!(
            readiness(&open(id()), &[(dep, None)]),
            Readiness::Unknown(vec![UnknownReason::DependencyUnavailable(dep)])
        );
        // A known blocker wins over an unknown one.
        let blocker = id();
        let r = readiness(&open(id()), &[(dep, None), (blocker, Some(TaskStatus::Open))]);
        assert_eq!(r, Readiness::NotReady(vec![NotReadyReason::BlockedBy(blocker)]));
    }

    #[test]
    fn refused_cycle_lists_its_members() {
        let (a, b, c) = (id(), id(), id());
        let edges = vec![
            edge(RelationKind::DependsOn, a, b, RelationStatus::Accepted),
            edge(RelationKind::DependsOn, b, c, RelationStatus::Accepted),
        ];
        assert_eq!(would_close_cycle(RelationKind::DependsOn, c, a, &edges), Some(vec![c, a, b, c]));
        assert_eq!(would_close_cycle(RelationKind::DependsOn, a, a, &edges), Some(vec![a, a]));
        assert_eq!(would_close_cycle(RelationKind::DependsOn, a, c, &edges), None);
    }

    #[test]
    fn proposed_and_retracted_edges_and_other_kinds_do_not_count() {
        let (a, b) = (id(), id());
        let edges = vec![
            edge(RelationKind::DependsOn, a, b, RelationStatus::Proposed),
            edge(RelationKind::DependsOn, a, b, RelationStatus::Retracted),
            edge(RelationKind::RelatesTo, a, b, RelationStatus::Accepted),
        ];
        assert_eq!(would_close_cycle(RelationKind::DependsOn, b, a, &edges), None);
        assert_eq!(would_close_cycle(RelationKind::RelatesTo, b, a, &edges), None, "relates_to may cycle");
        assert!(find_cycles(RelationKind::DependsOn, &edges).is_empty());
    }

    #[test]
    fn find_cycles_reports_each_component_once() {
        let (a, b, c, d, e) = (id(), id(), id(), id(), id());
        let edges = vec![
            edge(RelationKind::SubtaskOf, a, b, RelationStatus::Accepted),
            edge(RelationKind::SubtaskOf, b, a, RelationStatus::Accepted),
            edge(RelationKind::SubtaskOf, c, d, RelationStatus::Accepted),
            edge(RelationKind::SubtaskOf, e, e, RelationStatus::Accepted),
        ];
        let mut cycles = find_cycles(RelationKind::SubtaskOf, &edges);
        for c in &mut cycles {
            c.sort();
        }
        cycles.sort();
        let mut expected = vec![vec![a, b], vec![e]];
        for c in &mut expected {
            c.sort();
        }
        expected.sort();
        assert_eq!(cycles, expected);
    }

    #[test]
    fn a_task_is_started_by_its_assignee_or_by_anyone_when_unassigned() {
        let (me, you) = (id(), id());
        assert_eq!(can_start(TaskStatus::Open, None, me), Ok(()));
        assert_eq!(can_start(TaskStatus::Open, Some(me), me), Ok(()));
        assert_eq!(can_start(TaskStatus::Open, Some(you), me), Err(RuleError::AssignedToAnother { assignee: you }));
        assert_eq!(can_start(TaskStatus::Proposed, None, me), Err(RuleError::NotOpen { status: TaskStatus::Proposed }));
    }

    #[test]
    fn a_start_from_another_scope_is_refused_naming_the_scope_to_switch_to() {
        let (mine, elsewhere) = (id(), id());
        let scope = |id: EntityId, status: ScopeStatus| ScopeSnapshot {
            id,
            title: "Cloud Hosting".into(),
            status: Some(status),
        };
        let short: String = elsewhere.to_base64().chars().take(8).collect();

        let paused = [scope(elsewhere, ScopeStatus::Paused)];
        let refused = can_start_in(Some(mine), &paused).unwrap_err();
        assert_eq!(refused, RuleError::WorkedInAnotherScope { scopes: paused.to_vec() });
        assert_eq!(
            refused.to_string(),
            format!(
                "task is in the scope \"Cloud Hosting\" ({short}), not in the one you work in; to start it, switch to that scope first: run the command again with `--scope {short}`"
            )
        );
        // A checkout's binding is honoured for any active scope, whoever made it, so binding is
        // offered for every active scope and for no other.
        let active = [scope(elsewhere, ScopeStatus::Active)];
        assert!(can_start_in(Some(mine), &active)
            .unwrap_err()
            .to_string()
            .ends_with(&format!(", or bind this checkout to the scope with `fwd scope attach-dir {short}`")));

        assert_eq!(can_start_in(Some(elsewhere), &active), Ok(()), "from the task's own scope");
        assert_eq!(can_start_in(Some(mine), &[]), Ok(()), "a task worked in no scope");
        assert_eq!(can_start_in(None, &active), Ok(()), "a starter who works in no scope");
    }

    /// A task may be worked in several scopes (docs/DECISIONS.md, D136): a start from any of them
    /// goes through, and one from none of them is refused, naming each of them, with `--scope` for
    /// each and a binding for each active one.
    #[test]
    fn a_task_in_several_scopes_starts_from_any_of_them_and_the_refusal_names_them_all() {
        let (hosting, core, docs, mine) = (id(), id(), id(), id());
        let short = |id: &EntityId| -> String { id.to_base64().chars().take(8).collect() };
        let scope = |id: EntityId, title: &str, status: ScopeStatus| ScopeSnapshot {
            id,
            title: title.into(),
            status: Some(status),
        };
        let three = [
            scope(hosting, "Cloud Hosting", ScopeStatus::Active),
            scope(core, "Open Core", ScopeStatus::Planned),
            scope(docs, "Docs", ScopeStatus::Active),
        ];
        for from in [hosting, core, docs] {
            assert_eq!(can_start_in(Some(from), &three), Ok(()), "from each of its scopes");
        }
        let (h, c, d) = (short(&hosting), short(&core), short(&docs));
        assert_eq!(
            can_start_in(Some(mine), &three).unwrap_err().to_string(),
            format!(
                "task is in the scopes \"Cloud Hosting\" ({h}), \"Open Core\" ({c}) and \"Docs\" ({d}), not in the one you work in; to start it, switch to one of them first: run the command again with `--scope {h}`, `--scope {c}` or `--scope {d}`, or bind this checkout to one with `fwd scope attach-dir {h}` or `fwd scope attach-dir {d}`"
            )
        );
        let planned = [scope(core, "Open Core", ScopeStatus::Planned), scope(docs, "Docs", ScopeStatus::Paused)];
        assert_eq!(
            can_start_in(Some(mine), &planned).unwrap_err().to_string(),
            format!(
                "task is in the scopes \"Open Core\" ({c}) and \"Docs\" ({d}), not in the one you work in; to start it, switch to one of them first: run the command again with `--scope {c}` or `--scope {d}`"
            ),
            "no binding is offered while none of them is active"
        );
    }

    const UPSTREAM: &str = "github.com/org/app";

    /// An active scope of `UPSTREAM` naming `branches`.
    fn place(id: EntityId, branches: &[&str]) -> ScopePlace {
        ScopePlace {
            id,
            repo: Some(UPSTREAM.into()),
            branches: branches.iter().map(|b| b.to_string()).collect(),
            root: None,
        }
    }

    fn checkout<'a>(branch: Option<&'a str>, cwd: &'a std::path::Path) -> CheckoutPlace<'a> {
        CheckoutPlace { branch, upstream: Some(UPSTREAM), cwd }
    }

    #[test]
    fn a_checkout_finds_the_scope_bound_to_it_else_the_one_naming_its_branch_whoever_made_them() {
        let cwd = std::path::Path::new("/work/app");
        let (retry, docs, other_repo) = (id(), id(), id());
        let active = [
            place(retry, &["feat/retry"]),
            place(docs, &["feat/docs"]),
            ScopePlace { repo: Some("github.com/org/other".into()), ..place(other_repo, &["feat/retry"]) },
        ];
        // Nothing in the scopes says who made them: every active scope is a candidate.
        assert_eq!(scope_for_checkout(None, &active, checkout(Some("feat/retry"), cwd)), ScopeHere::One(retry));
        assert_eq!(scope_for_checkout(None, &active, checkout(Some("main"), cwd)), ScopeHere::None);
        assert_eq!(scope_for_checkout(None, &active, checkout(None, cwd)), ScopeHere::None, "a detached HEAD");
        // A binding wins over the branch, and holds whatever the branch.
        assert_eq!(scope_for_checkout(Some(docs), &active, checkout(Some("feat/retry"), cwd)), ScopeHere::One(docs));
        assert_eq!(scope_for_checkout(Some(docs), &active, checkout(None, cwd)), ScopeHere::One(docs));
        // A binding to a scope that is not active (not among `active`) is passed over.
        assert_eq!(scope_for_checkout(Some(id()), &active, checkout(Some("feat/retry"), cwd)), ScopeHere::One(retry));
        // A checkout without an origin remote matches no scope that names its repository.
        let no_remote = CheckoutPlace { upstream: None, ..checkout(Some("feat/retry"), cwd) };
        assert_eq!(scope_for_checkout(None, &active, no_remote), ScopeHere::None);
    }

    #[test]
    fn several_scopes_naming_the_branch_leave_the_checkout_ambiguous_until_it_is_bound() {
        let cwd = std::path::Path::new("/work/app");
        let (initial, reviews) = (id(), id());
        let active = [place(initial, &["main"]), place(reviews, &["main", "feat/review"])];
        assert_eq!(
            scope_for_checkout(None, &active, checkout(Some("main"), cwd)),
            ScopeHere::Several { branch: Some("main".into()), scopes: vec![initial, reviews] }
        );
        assert_eq!(scope_for_checkout(None, &active, checkout(Some("feat/review"), cwd)), ScopeHere::One(reviews));
        assert_eq!(scope_for_checkout(Some(initial), &active, checkout(Some("main"), cwd)), ScopeHere::One(initial));

        // The refusal of a command that needs a scope names each candidate and both ways to choose.
        let short = |id: EntityId| -> String { id.to_base64().chars().take(8).collect() };
        let refused = RuleError::SeveralScopesHere {
            branch: Some("main".into()),
            scopes: vec![(initial, "Initial implementation".into()), (reviews, "Reviews and highlights".into())],
        };
        assert_eq!(
            refused.to_string(),
            format!(
                "several active scopes name this checkout's branch main: \"Initial implementation\" ({}) and \"Reviews and highlights\" ({}); say which you work in: run the command again with `--scope <id>`, or bind this checkout to one with `fwd scope attach-dir <id>`",
                short(initial),
                short(reviews)
            )
        );
    }

    #[test]
    fn a_scope_made_before_repositories_were_named_is_found_by_its_directory() {
        let (old, older) = (id(), id());
        let recorded = |id: EntityId, root: &str, branches: &[&str]| ScopePlace {
            id,
            repo: None,
            branches: branches.iter().map(|b| b.to_string()).collect(),
            root: Some(root.into()),
        };
        let inside = std::path::Path::new("/work/app/src");
        let active = [recorded(old, "/work/app", &["main"])];
        assert_eq!(scope_for_checkout(None, &active, checkout(Some("main"), inside)), ScopeHere::One(old));
        assert_eq!(
            scope_for_checkout(None, &active, checkout(Some("main"), std::path::Path::new("/elsewhere"))),
            ScopeHere::None
        );
        // Failing a branch, the directory alone; two that hold it are ambiguous.
        assert_eq!(scope_for_checkout(None, &active, checkout(Some("feat/x"), inside)), ScopeHere::One(old));
        let both = [recorded(old, "/work/app", &["main"]), recorded(older, "/work", &[])];
        assert_eq!(
            scope_for_checkout(None, &both, checkout(Some("feat/x"), inside)),
            ScopeHere::Several { branch: None, scopes: vec![old, older] }
        );
    }

    #[test]
    fn only_a_standing_step_is_checked_and_nothing_else_is_ever_resolved() {
        use RelationStatus::*;
        assert_eq!(check_step(RelationKind::StepOf, Accepted, true), Ok(Some(Resolved)));
        assert_eq!(check_step(RelationKind::StepOf, Resolved, false), Ok(Some(Accepted)));
        assert_eq!(check_step(RelationKind::StepOf, Resolved, true), Ok(None), "checking twice changes nothing");
        assert_eq!(check_step(RelationKind::StepOf, Accepted, false), Ok(None));
        for status in [Proposed, Retracted, Rejected] {
            assert_eq!(check_step(RelationKind::StepOf, status, true), Err(RuleError::StepNotStanding { status }));
        }
        for kind in [
            RelationKind::DependsOn,
            RelationKind::SubtaskOf,
            RelationKind::MotivatedBy,
            RelationKind::DiscoveredFrom,
            RelationKind::RelatesTo,
            RelationKind::Supersedes,
            RelationKind::Duplicates,
            RelationKind::AnchoredAt,
            RelationKind::AssignedTo,
            RelationKind::Concerns,
            RelationKind::Cites,
            RelationKind::WorkedIn,
            RelationKind::Mention,
        ] {
            assert_eq!(check_step(kind, Accepted, true), Err(RuleError::NotAStep { kind }));
        }
    }

    fn message(created: i64, answers: bool, status: MessageStatus) -> ThreadMessage {
        ThreadMessage { id: id(), answers, status, created }
    }
    fn step(status: RelationStatus) -> StepEdge {
        StepEdge { id: id(), kind: RelationKind::StepOf, status }
    }

    #[test]
    fn the_first_thread_is_the_first_started_that_answers_nothing_and_is_not_deleted() {
        use MessageStatus as M;
        let (reply, deleted) = (message(1, true, M::Active), message(2, false, M::Deleted));
        let (first, later) = (message(5, false, M::Resolved), message(9, false, M::Active));
        let on = [later, reply.clone(), first.clone(), deleted];
        assert_eq!(first_thread(&on), Some(first.id), "a reply and a deleted message start no thread");
        assert_eq!(first_thread(&[reply, message(3, true, M::Active)]), None, "replies alone: no thread");
        assert_eq!(first_thread(&[]), None);
        // Two started at the same moment: the lower id is the first, whichever is listed first.
        let (a, b) = (message(5, false, M::Active), message(5, false, M::Active));
        let lower = a.id.min(b.id);
        assert_eq!(first_thread(&[a.clone(), b.clone()]), Some(lower));
        assert_eq!(first_thread(&[b, a]), Some(lower));
    }

    #[test]
    fn resolving_a_highlights_first_thread_checks_its_steps_and_setting_it_back_unchecks_them() {
        use MessageStatus as M;
        use RelationStatus as R;
        let (first, later, reply) =
            (message(1, false, M::Active), message(2, false, M::Active), message(3, true, M::Active));
        let on = [first.clone(), later.clone(), reply.clone()];
        // A step of each of two tasks, one of them checked already, and a step taken off a checklist.
        let (open, done, gone) = (step(R::Accepted), step(R::Resolved), step(R::Retracted));
        let steps = [open.clone(), done.clone(), gone];
        let resolved = move_step_and_thread(Moved::Message(&first), true, &on, &steps).unwrap();
        assert_eq!(resolved, Changes { message: Some((first.id, M::Resolved)), steps: vec![(open.id, R::Resolved)] });
        let settled = ThreadMessage { status: M::Resolved, ..first.clone() };
        let back =
            move_step_and_thread(Moved::Message(&settled), false, std::slice::from_ref(&settled), &steps).unwrap();
        assert_eq!(back, Changes { message: Some((first.id, M::Active)), steps: vec![(done.id, R::Accepted)] });
        // A later thread, and a reply, move alone.
        for alone in [&later, &reply] {
            let moved = move_step_and_thread(Moved::Message(alone), true, &on, &steps).unwrap();
            assert_eq!(moved, Changes { message: Some((alone.id, M::Resolved)), steps: vec![] });
        }
        // So does a message on no highlight.
        let loose = message(4, false, M::Active);
        let moved = move_step_and_thread(Moved::Message(&loose), true, &[], &[]).unwrap();
        assert_eq!(moved, Changes { message: Some((loose.id, M::Resolved)), steps: vec![] });
        // Resolving a resolved first thread leaves the message as it is, and checks a step that
        // was unchecked meanwhile.
        let again =
            move_step_and_thread(Moved::Message(&settled), true, std::slice::from_ref(&settled), &steps).unwrap();
        assert_eq!(again, Changes { message: None, steps: vec![(open.id, R::Resolved)] });
    }

    #[test]
    fn checking_a_step_resolves_its_highlights_first_thread_and_its_other_steps_move_with_it() {
        use MessageStatus as M;
        use RelationStatus as R;
        let (first, later) = (message(1, false, M::Active), message(2, false, M::Active));
        let (mine, theirs) = (step(R::Accepted), step(R::Accepted));
        let steps = [mine.clone(), theirs.clone()];
        let checked = move_step_and_thread(Moved::Step(&mine), true, &[later, first.clone()], &steps).unwrap();
        assert_eq!(
            checked,
            Changes {
                message: Some((first.id, M::Resolved)),
                steps: vec![(mine.id, R::Resolved), (theirs.id, R::Resolved)]
            }
        );
        let (mine, theirs) = (StepEdge { status: R::Resolved, ..mine }, StepEdge { status: R::Resolved, ..theirs });
        let resolved = [ThreadMessage { status: M::Resolved, ..first.clone() }];
        let steps = [mine.clone(), theirs.clone()];
        let unchecked = move_step_and_thread(Moved::Step(&mine), false, &resolved, &steps).unwrap();
        assert_eq!(
            unchecked,
            Changes {
                message: Some((first.id, M::Active)),
                steps: vec![(mine.id, R::Accepted), (theirs.id, R::Accepted)]
            }
        );
        // A highlight no one has started a thread on: the step moves alone.
        let alone = move_step_and_thread(Moved::Step(&mine), false, &[], &steps).unwrap();
        assert_eq!(alone, Changes { message: None, steps: vec![(mine.id, R::Accepted)] });
    }

    #[test]
    fn a_moved_step_must_be_a_standing_step() {
        use RelationStatus as R;
        let first = message(1, false, MessageStatus::Active);
        let proposed = step(R::Proposed);
        assert_eq!(
            move_step_and_thread(
                Moved::Step(&proposed),
                true,
                std::slice::from_ref(&first),
                std::slice::from_ref(&proposed)
            ),
            Err(RuleError::StepNotStanding { status: R::Proposed })
        );
        let dependency = StepEdge { kind: RelationKind::DependsOn, ..step(R::Accepted) };
        assert_eq!(
            move_step_and_thread(Moved::Step(&dependency), true, &[first], &[]),
            Err(RuleError::NotAStep { kind: RelationKind::DependsOn })
        );
    }

    #[test]
    fn retiring_a_highlight_resolves_every_thread_and_checks_its_other_steps_with_the_first() {
        use MessageStatus as M;
        use RelationStatus as R;
        let (first, later) = (message(1, false, M::Active), message(2, false, M::Active));
        let (reply, settled, deleted) =
            (message(3, true, M::Active), message(4, false, M::Resolved), message(5, false, M::Deleted));
        let on = [later.clone(), reply, first.clone(), settled, deleted];
        // The step coming off its checklist, the highlight's step on another checklist, one
        // checked already, and one taken off before.
        let (leaving, theirs, done, gone) =
            (step(R::Accepted), step(R::Accepted), step(R::Resolved), step(R::Retracted));
        let steps = [leaving.clone(), theirs.clone(), done.clone(), gone];
        let retired = retire_highlight(&on, &steps, leaving.id).unwrap();
        assert_eq!(
            retired,
            Retired { messages: vec![first.id, later.id], steps: vec![(theirs.id, R::Resolved)] },
            "each open thread, oldest first; a reply, a resolved thread and a deleted one stay as they are, and so does the step leaving"
        );
        // A first thread resolved already checks a step left unchecked, as resolving it again does.
        let quiet = [ThreadMessage { status: M::Resolved, ..first }];
        let again = retire_highlight(&quiet, &steps, leaving.id).unwrap();
        assert_eq!(again, Retired { messages: vec![], steps: vec![(theirs.id, R::Resolved)] });
        let agreed = [leaving.clone(), done];
        assert!(retire_highlight(&quiet, &agreed, leaving.id).unwrap().is_empty(), "nothing open, nothing apart");
        assert!(
            retire_highlight(&[], &steps, leaving.id).unwrap().is_empty(),
            "a highlight no one started a thread on"
        );
    }

    #[test]
    fn only_the_tasks_creator_and_its_assignee_take_a_step_off_its_checklist() {
        let (creator, assignee, other) = (id(), id(), id());
        assert_eq!(can_remove_step(creator, Some(assignee), creator), Ok(()));
        assert_eq!(can_remove_step(creator, Some(assignee), assignee), Ok(()));
        assert_eq!(can_remove_step(creator, None, creator), Ok(()), "a task no one is assigned: its creator");
        let refused = can_remove_step(creator, Some(assignee), other).unwrap_err();
        assert_eq!(refused, RuleError::NotTheTasksPeople { creator, assignee: Some(assignee) });
        assert_eq!(
            refused.to_string(),
            format!(
                "only the task's creator ({}) and its assignee ({}) take a step off its checklist",
                short_id(&creator),
                short_id(&assignee)
            )
        );
        assert_eq!(
            can_remove_step(creator, None, other).unwrap_err().to_string(),
            format!(
                "only the task's creator ({}) and its assignee (no one) take a step off its checklist",
                short_id(&creator)
            )
        );
    }

    #[test]
    fn only_its_recipient_settles_a_notification() {
        let (notification, recipient, other) = (id(), id(), id());
        assert_eq!(can_settle(notification, recipient, recipient), Ok(()));
        let refused = can_settle(notification, recipient, other).unwrap_err();
        assert_eq!(refused, RuleError::NotTheRecipient { notification, recipient });
        assert_eq!(
            refused.to_string(),
            format!(
                "notification {} is {}'s, and a principal reads, dismisses, replies through or forwards only its own notifications",
                short_id(&notification),
                short_id(&recipient)
            )
        );
    }

    #[test]
    fn steps_between_tasks_stay_acyclic_whether_checked_or_not() {
        let (a, b, c) = (id(), id(), id());
        let edges = vec![
            edge(RelationKind::StepOf, a, b, RelationStatus::Resolved),
            edge(RelationKind::StepOf, b, c, RelationStatus::Accepted),
        ];
        assert_eq!(would_close_cycle(RelationKind::StepOf, c, a, &edges), Some(vec![c, a, b, c]));
        assert_eq!(would_close_cycle(RelationKind::StepOf, a, a, &edges), Some(vec![a, a]));
        assert_eq!(would_close_cycle(RelationKind::StepOf, a, c, &edges), None);
        let resolved_elsewhere = vec![edge(RelationKind::DependsOn, a, b, RelationStatus::Resolved)];
        assert_eq!(
            would_close_cycle(RelationKind::DependsOn, b, a, &resolved_elsewhere),
            None,
            "only a step holds when resolved"
        );
    }

    #[test]
    fn its_author_and_the_agents_that_succeeded_its_author_edit_a_message() {
        let (author, second, third, stranger) = (id(), id(), id(), id());
        assert_eq!(can_edit_message(author, &[], author), Ok(()), "its author, whom no one succeeded");
        assert_eq!(can_edit_message(author, &[second], author), Ok(()), "its author, succeeded since");
        assert_eq!(can_edit_message(author, &[second], second), Ok(()), "the author's successor");
        let chain = [second, third];
        assert_eq!(can_edit_message(author, &chain, third), Ok(()), "the successor of the author's successor");
        assert_eq!(can_edit_message(author, &chain, second), Ok(()), "and the one between");

        let refused = can_edit_message(author, &[], stranger).unwrap_err();
        assert_eq!(refused, RuleError::NeitherAuthorNorSuccessor { author, successor: None });
        assert_eq!(
            refused.to_string(),
            format!(
                "only its author edits a message, or an agent that succeeded its author, and this one was written by {}; reply to it instead",
                short_id(&author)
            )
        );
        let refused = can_edit_message(author, &chain, stranger).unwrap_err();
        assert_eq!(
            refused,
            RuleError::NeitherAuthorNorSuccessor { author, successor: Some(third) },
            "the refusal names the last successor, which carries on the author's job now"
        );
        assert_eq!(
            refused.to_string(),
            format!(
                "only its author edits a message, or an agent that succeeded its author, and this one was written by {}, whom {} succeeded; reply to it instead",
                short_id(&author),
                short_id(&third)
            )
        );
    }

    #[test]
    fn acceptance_needs_another_principal() {
        let (me, you) = (id(), id());
        assert_eq!(can_accept(me, me), Err(RuleError::SelfAcceptance));
        assert_eq!(can_accept(me, you), Ok(()));
    }

    #[test]
    fn transitions_follow_the_table() {
        use TaskStatus::*;
        assert_eq!(transition(Proposed, Verb::Approve, true), Ok(Open));
        assert_eq!(transition(Proposed, Verb::Reject, true), Ok(Rejected));
        assert_eq!(transition(Open, Verb::Start, true), Ok(InProgress));
        assert_eq!(transition(InProgress, Verb::Submit, true), Ok(InReview));
        assert_eq!(transition(InProgress, Verb::Submit, false), Ok(Done));
        assert_eq!(transition(InReview, Verb::Accept, true), Ok(Done));
        assert_eq!(transition(InReview, Verb::Return, true), Ok(InProgress));
        assert_eq!(transition(Open, Verb::Done, false), Ok(Done));
        assert_eq!(
            transition(Open, Verb::Done, true),
            Err(RuleError::InvalidTransition { from: Open, verb: Verb::Done })
        );
        assert_eq!(transition(InReview, Verb::Cancel, true), Ok(Cancelled));
        assert_eq!(transition(Rejected, Verb::Reopen, true), Ok(Open));
        assert_eq!(transition(InProgress, Verb::Rejustify, true), Ok(Proposed));
        assert_eq!(
            transition(Done, Verb::Start, true),
            Err(RuleError::InvalidTransition { from: Done, verb: Verb::Start })
        );
        assert_eq!(
            transition(Proposed, Verb::Cancel, true),
            Err(RuleError::InvalidTransition { from: Proposed, verb: Verb::Cancel })
        );
    }

    #[test]
    fn decisions_move_from_proposed_or_from_accepted_and_nowhere_else() {
        use DecisionStatus::*;
        assert_eq!(decision_move(Proposed, DecisionMove::Accept), Ok(Accepted));
        assert_eq!(decision_move(Proposed, DecisionMove::Reject), Ok(Rejected));
        assert_eq!(decision_move(Accepted, DecisionMove::Supersede), Ok(Superseded));
        assert_eq!(decision_move(Accepted, DecisionMove::Repeal), Ok(Repealed));
        for from in [Proposed, Accepted, Rejected, Superseded, Repealed] {
            for mv in [DecisionMove::Accept, DecisionMove::Reject, DecisionMove::Supersede, DecisionMove::Repeal] {
                if from != mv.moves_from() {
                    assert_eq!(decision_move(from, mv), Err(RuleError::InvalidDecisionMove { from, mv }));
                }
            }
        }
        assert_eq!(
            decision_move(Repealed, DecisionMove::Accept).unwrap_err().to_string(),
            "only proposed decisions can be accepted; this one is repealed"
        );
        assert_eq!(
            decision_move(Proposed, DecisionMove::Repeal).unwrap_err().to_string(),
            "only accepted decisions can be repealed; this one is proposed"
        );
        // Three moves close a decision; a proposal and a ruling in force are open.
        let closing = [Proposed, Accepted, Rejected, Superseded, Repealed].map(DecisionMove::closing);
        assert_eq!(
            closing,
            [None, None, Some(DecisionMove::Reject), Some(DecisionMove::Supersede), Some(DecisionMove::Repeal)]
        );
    }

    #[test]
    fn a_ruling_is_superseded_only_by_another_ruling_in_force() {
        let (old, new) = (id(), id());
        assert_eq!(can_supersede(old, new, DecisionStatus::Accepted), Ok(()));
        assert_eq!(can_supersede(old, old, DecisionStatus::Accepted), Err(RuleError::SupersedesItself));
        assert_eq!(
            can_supersede(old, new, DecisionStatus::Proposed),
            Err(RuleError::SuccessorNotAccepted { status: DecisionStatus::Proposed })
        );
    }

    #[test]
    fn decisions_number_from_one_past_the_highest() {
        assert_eq!(next_decision_number([]), 1);
        assert_eq!(next_decision_number([1, 2, 3]), 4);
        assert_eq!(next_decision_number([7, 2]), 8, "a gap is never filled");
        assert_eq!(next_decision_number([3, 3]), 4, "a duplicate counts once");
    }

    #[test]
    fn the_projects_rulings_apply_to_every_scopes_work_and_a_scopes_own_to_its_work_alone() {
        let (here, there) = (id(), id());
        assert!(applies_to(None, here), "a ruling attached to no scope governs the whole project");
        assert!(applies_to(Some(here), here));
        assert!(!applies_to(Some(there), here), "another scope's ruling does not apply here");
    }

    #[test]
    fn a_decision_number_is_d_and_digits() {
        assert_eq!(decision_number("D12"), Some(12));
        assert_eq!(decision_number("d7"), Some(7));
        for not_one in ["D", "12", "D-1", "D1a", "DD1", "-5Yk2Qd7wXcL9mPz0RtVbA", "D1234567890123456789"] {
            assert_eq!(decision_number(not_one), None, "{not_one}");
        }
    }

    /// Random edits with the pre-check applied never produce a cycle, and readiness never says
    /// Ready while any dependency is not Done.
    #[test]
    fn random_dag_edits_never_produce_a_false_ready_or_a_cycle() {
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut rand = move |n: usize| -> usize {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as usize
        };
        let tasks: Vec<EntityId> = (0..12).map(|_| id()).collect();
        let statuses = [TaskStatus::Open, TaskStatus::Done, TaskStatus::InProgress, TaskStatus::Cancelled];
        let mut status_of: HashMap<EntityId, TaskStatus> = tasks.iter().map(|t| (*t, TaskStatus::Open)).collect();
        let mut edges: Vec<Edge> = Vec::new();
        let mut refused = 0;
        for _ in 0..400 {
            match rand(4) {
                0 | 1 => {
                    let from = tasks[rand(tasks.len())];
                    let to = tasks[rand(tasks.len())];
                    if would_close_cycle(RelationKind::DependsOn, from, to, &edges).is_some() {
                        refused += 1;
                    } else {
                        edges.push(edge(RelationKind::DependsOn, from, to, RelationStatus::Accepted));
                    }
                }
                2 => {
                    if !edges.is_empty() {
                        let i = rand(edges.len());
                        edges[i].status = RelationStatus::Retracted;
                    }
                }
                _ => {
                    let t = tasks[rand(tasks.len())];
                    status_of.insert(t, statuses[rand(statuses.len())]);
                }
            }
            assert!(
                find_cycles(RelationKind::DependsOn, &edges).is_empty(),
                "the pre-check must keep the graph acyclic"
            );
            for t in &tasks {
                let deps: Vec<(EntityId, Option<TaskStatus>)> = edges
                    .iter()
                    .filter(|e| e.counts(RelationKind::DependsOn) && e.from == *t)
                    .map(|e| (e.to, Some(status_of[&e.to])))
                    .collect();
                let snapshot = TaskSnapshot { id: *t, status: status_of[t], hold: None };
                let r = readiness(&snapshot, &deps);
                let all_done = deps.iter().all(|(_, s)| *s == Some(TaskStatus::Done));
                assert_eq!(
                    r.is_ready(),
                    status_of[t] == TaskStatus::Open && all_done,
                    "readiness must match the rule for {t}"
                );
            }
        }
        assert!(refused > 0, "the random walk should have attempted at least one cycle");
    }
}
