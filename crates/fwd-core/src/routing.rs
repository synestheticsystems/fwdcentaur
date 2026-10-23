//! Routing: who needs to know about a thing. Produces notifications from messages, new tasks,
//! task status changes, a task's new assignee, relations, new decisions, decision moves and a
//! change of what a triage agent serves, following the rules in docs/design/agents-and-inbox.md.
//! The daemon runs this as a live-query service; tests call it directly.
//!
//! What reached the project's one triage agent until 2026-09-30 reaches the triage agent of the
//! scope it concerns, else the one that serves the whole project (docs/DECISIONS.md, D122; the
//! scopes: [`scopes_concerned`], `ops::worked_in`; the agent: `ops::front_line`). What concerns a
//! task concerns each scope it is worked in, any number since D136 ("A task may be worked in
//! several scopes", 2026-10-03), so it reaches the triage agent of each, each agent once
//! (`ops::front_lines`), save a question asked of no one, which asks one scope ([`asked_scope`]).
//!
//! Each planned notification records in its `originator` why its recipient was chosen (`why`,
//! one of [`Why`]), so an inbox can say in words why the notification is there (`reason.rs`).
//! Notifications planned before routing recorded it have no `why`.
//!
//! What one author does on one task reaches each recipient as one notification, which grows while
//! it stays unread (rule 10; D95, "A burst of comments and additions from one author on one task
//! reaches a person as one notification"): a plan for such an item carries it as a [`Batch`] of
//! one, and [`apply`] adds it to the batch the recipient already holds rather than make another.
//! A plan for a step added carries with it the threads open on the step's lines that its recipient
//! did not write in, is not asked by and was not told of ([`BatchItem::Thread`]), so that a comment
//! or a question written on lines before they became a step reaches the task's people with the
//! step; a comment routed after such a step tells them nothing more of it.
//!
//! What would reach a principal a successor took over (`fwd principal succeed`; D114) reaches the
//! last of its successors instead, and says whom it was for ([`Successors`]).

use crate::model::*;
use crate::naming;
use crate::ops;
use crate::query::with_id;
use ankurah::{Context, EntityId};
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

/// A code comment reaches the assignees of tasks anchored this close, in lines, in the same file
/// (anywhere in it, when the comment or the task is on the whole file).
pub const NEARBY_LINES: i64 = 50;

/// Whether two anchors in one file, by their lines (none for the whole file, `ops::lines_of`), are
/// near enough for a comment on one to reach the people working on the other
/// (docs/design/agents-and-inbox.md, rule 1, a comment on code): within [`NEARBY_LINES`] lines of
/// each other, or anywhere at all when either is on the whole file, since every line of a file is
/// part of the whole of it. A whole-file anchor stores its lines as 0 and 0, so read as lines it
/// would sit at the top of the file and reach only the tasks anchored there.
fn nearby(a: Option<(i64, i64)>, b: Option<(i64, i64)>) -> bool {
    let (Some((start, end)), Some((other_start, other_end))) = (a, b) else { return true };
    let distance = if other_end <= start {
        start - other_end
    } else if end <= other_start {
        other_start - end
    } else {
        0
    };
    distance <= NEARBY_LINES
}

/// What a message addressed to someone gives, whether it is a question or a forward (a reply with
/// a `to`, or asking the project): the notification to its addressee, or for a question asked of
/// no one in particular to the triage agent of the scope it asks (rule 5), and those to whom it
/// mentions (rule 13), and nothing else. It is for whom it is addressed to, so no rule that reaches
/// the people of what it is about gives it to them: a comment on code, on a task or on a scope
/// (rules 1 to 3), on a step's highlight (rule 14) or on a decision (rule 11), nor, for a forward,
/// its thread's (rule 6). A question asked of no one reaches its triage agent alone, which answers
/// it or forwards it without distracting the task's implementer, while a comment that asks no one
/// still reaches the task's assignee and the triage agent of each of its scopes (docs/DECISIONS.md,
/// D111, "A message reaches only whom it is addressed to; the triager is the front line for
/// questions directed to it, without distracting the implementer", the triager of that day being
/// the triage agent, one per project until D122 made it one per scope; Daniel,
/// 2026-09-29, on finding the supervisor's commentary under a question asked of him about a task,
/// which had reached the supervisor as a comment on the task: "are we delivering messages
/// addressed to me to the supervisor's inbox? Seems inappropriate"; and 2026-09-26, on a forward
/// from the supervisor to claude beneath his comment: "it didn't say to: dnorman though. it said
/// from supervisor to claude. this is why I was confused to see it in my inbox.").
pub const ADDRESSED_GIVES: &[NotificationType] = &[NotificationType::QuestionAsked, NotificationType::Mentioned];

/// Why routing chose a recipient, stored as its snake_case name in `originator.why`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::Display, strum::EnumString)]
#[strum(serialize_all = "snake_case")]
pub enum Why {
    /// The message's `to` names the recipient.
    Addressee,
    /// A forward (a reply with a `to`) names the recipient.
    Forwarded,
    /// The recipient wrote the message a reply answers.
    ParentAuthor,
    /// The recipient started the thread a reply is in.
    ThreadAuthor,
    /// The recipient wrote in the thread a reply is in.
    Participant,
    /// The message names the recipient inline (a `mention` relation from it to them).
    Mentioned,
    /// The recipient is the task's assignee: of a task commented on, a task that became ready, a
    /// task just assigned to them, or a task anchored near commented code (then `near_task_id`
    /// names it; since 2026-09-29, when it took the place of [`Why::ScopeOwner`] there).
    Assignee,
    /// The recipient created, or is the assignee of, a task that the highlight commented on is a
    /// step of, or that a step was added to (`step_task_id` names the task).
    StepParty,
    /// Until 2026-09-29, when scopes lost their owner (docs/DECISIONS.md, D108, "A scope has no
    /// owner: scopes are not assigned to principals"): the recipient owned the scope concerned,
    /// the scope a comment was about, which reaches that scope's triage agent now, or the scope a
    /// task anchored near commented code was worked in (then `near_task_id` names that task), whose
    /// assignee hears it now. Until 2026-09-28 also the scope a task commented on, assigned or
    /// unblocked was assigned to, which its assignee is now. Kept so that the notifications routed
    /// then still say why they came.
    ScopeOwner,
    /// The recipient is the triage agent that answers first for what the rule is about: the one
    /// that serves the scope it concerns, which `triage_scope_id` then names, else the one that
    /// serves the whole project (docs/design/agents-and-inbox.md, "Roles"; docs/DECISIONS.md,
    /// D122): of a question asked of no one in particular, a comment on a task, on code or on a
    /// scope, a proposal, an unassigned task that became ready, a review. Until 2026-09-30 the
    /// project had one triage agent, which every such rule reached. A notification routed before
    /// 2026-09-29 recorded it as `supervisor`, when the rule reached every principal with that role
    /// (a question to the role among them), and reads as this one.
    #[strum(to_string = "triage", serialize = "supervisor")]
    Triage,
    /// The recipient is one of the project's people, who rule on proposed decisions.
    Person,
    /// The recipient wrote the message a new task was filed from.
    SourceAuthor,
    /// The recipient approved the submitted task (it went through Proposed).
    Approver,
    /// The recipient created the submitted task.
    Creator,
    /// The recipient filed the decision commented on.
    DecisionAuthor,
    /// The recipient ruled on the decision commented on.
    Decider,
    /// The recipient made something that cites the decision that was repealed or superseded
    /// (`citing_id` names it).
    CitingAuthor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    pub recipient: EntityId,
    pub notification_type: NotificationType,
    pub priority: Priority,
    pub target: Value,
    pub originator: Value,
}

impl Planned {
    /// A notification for `recipient`, its `originator` the cause's with `why` added.
    fn new(
        recipient: EntityId,
        notification_type: NotificationType,
        priority: Priority,
        target: Value,
        originator: &Value,
        why: Why,
    ) -> Planned {
        let mut originator = originator.clone();
        originator["why"] = json!(why.to_string());
        Planned { recipient, notification_type, priority, target, originator }
    }

    /// The same notification naming one more entity in its `originator`.
    fn naming(mut self, key: &str, id: EntityId) -> Planned {
        self.originator[key] = json!(id.to_base64());
        self
    }
}

/// The reason routing recorded on a notification's `originator`; none for one planned before
/// routing recorded it, or with a value this build does not know.
pub fn why_of(originator: &Value) -> Option<Why> {
    originator.get("why").and_then(Value::as_str).and_then(|s| s.parse().ok())
}

fn target(kind: &str, id: EntityId) -> Value {
    json!({"type": kind, "id": id.to_base64()})
}

fn id_at(v: &Value, key: &str) -> Option<EntityId> {
    v.get(key).and_then(Value::as_str).and_then(|s| s.parse().ok())
}

/// One thing a [`Batch`] holds: what an author did on a task, or a thread a step came with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchItem {
    /// A comment on the task, or on the highlight of one of its steps, and when it was written.
    Comment { message: EntityId, at: i64 },
    /// A step added to the task: its `step_of` relation, what became the step, and when.
    Step { relation: EntityId, step: EntityId, at: i64 },
    /// So that a comment or a question written on lines before they became a step reaches the
    /// task's creator and assignee with the step, where it reached neither when it was written
    /// (task WTCAd9la): a thread open on the highlight of a step the batch holds as routing reached
    /// the step, by its first message, whoever wrote it, and when that was written. It is listed with
    /// its step and is not counted as a comment of the batch's author, who may not have written
    /// it. Recorded as `{"thread_id": ..., "at": ...}`, which a build from before it reads as
    /// nothing, rather than as a comment of the batch's author.
    Thread { message: EntityId, at: i64 },
}

impl BatchItem {
    /// When the author did it, or for a thread when its first message was written.
    pub fn at(&self) -> i64 {
        match self {
            BatchItem::Comment { at, .. } | BatchItem::Step { at, .. } | BatchItem::Thread { at, .. } => *at,
        }
    }

    /// The entity that makes it this item and no other: the message, or the step's relation. A
    /// comment and a thread that are one message are one item.
    fn key(&self) -> EntityId {
        match self {
            BatchItem::Comment { message, .. } | BatchItem::Thread { message, .. } => *message,
            BatchItem::Step { relation, .. } => *relation,
        }
    }

    fn to_json(self) -> Value {
        match self {
            BatchItem::Comment { message, at } => json!({"message_id": message.to_base64(), "at": at}),
            BatchItem::Step { relation, step, at } => {
                json!({"relation_id": relation.to_base64(), "step_id": step.to_base64(), "at": at})
            }
            BatchItem::Thread { message, at } => json!({"thread_id": message.to_base64(), "at": at}),
        }
    }

    fn from_json(v: &Value) -> Option<BatchItem> {
        let at = v.get("at").and_then(Value::as_i64).unwrap_or(0);
        if let Some(message) = id_at(v, "thread_id") {
            return Some(BatchItem::Thread { message, at });
        }
        match (id_at(v, "message_id"), id_at(v, "relation_id"), id_at(v, "step_id")) {
            (Some(message), None, _) => Some(BatchItem::Comment { message, at }),
            (None, Some(relation), Some(step)) => Some(BatchItem::Step { relation, step, at }),
            _ => None,
        }
    }
}

/// What one author did on one task, collected in one notification for one recipient while it
/// stays unread (docs/design/agents-and-inbox.md, rule 10): the task, and the items, oldest first,
/// so a burst of steps and comments reaches a person once rather than once per item, with the
/// threads its steps came with. Routing records it in the notification's `originator` as `batch`:
/// `{"task_id": "...", "items": [{"thread_id": "...", "at": <ms>}, {"relation_id": "...",
/// "step_id": "...", "at": <ms>}, {"message_id": "...", "at": <ms>}]}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Batch {
    pub task: EntityId,
    pub items: Vec<BatchItem>,
}

impl Batch {
    fn of_one(task: EntityId, item: BatchItem) -> Batch {
        Batch { task, items: vec![item] }
    }

    /// How many steps it holds.
    pub fn steps(&self) -> usize {
        self.items.iter().filter(|i| matches!(i, BatchItem::Step { .. })).count()
    }

    /// How many comments it holds.
    pub fn comments(&self) -> usize {
        self.items.iter().filter(|i| matches!(i, BatchItem::Comment { .. })).count()
    }

    /// The threads its steps came with, by their first messages, oldest first.
    pub fn threads(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.items.iter().filter_map(|i| match i {
            BatchItem::Thread { message, .. } => Some(*message),
            BatchItem::Comment { .. } | BatchItem::Step { .. } => None,
        })
    }

    /// Every message it holds, its comments and its threads, oldest first.
    pub fn messages(&self) -> Vec<EntityId> {
        self.items
            .iter()
            .filter_map(|i| match i {
                BatchItem::Comment { message, .. } | BatchItem::Thread { message, .. } => Some(*message),
                BatchItem::Step { .. } => None,
            })
            .collect()
    }

    /// Its latest comment.
    pub fn latest_comment(&self) -> Option<EntityId> {
        self.items.iter().rev().find_map(|i| match i {
            BatchItem::Comment { message, .. } => Some(*message),
            BatchItem::Step { .. } | BatchItem::Thread { .. } => None,
        })
    }

    /// The message a reply or a forward given the notification's id acts on, which its
    /// `message_id` names, so that acting through the notification, which settles it, never leaves
    /// a message it lists unanswered without saying so. A batch that holds nothing but what its
    /// author did names its latest comment, as a digest's has since batches existed, all of its
    /// comments being its author's own. One that holds a thread a step came with, which anyone may
    /// have written, names the one message it holds when it holds one, and none when it holds
    /// several, its threads and its author's comments counted alike: `fwd reply` and `fwd forward`
    /// refuse such a notification's id, naming each message it lists.
    pub fn message_behind(&self) -> Option<EntityId> {
        if self.threads().next().is_none() {
            return self.latest_comment();
        }
        match self.messages().as_slice() {
            [only] => Some(*only),
            _ => None,
        }
    }

    fn holds(&self, item: &BatchItem) -> bool {
        self.items.iter().any(|i| i.key() == item.key())
    }

    /// Adds `item` in its place by time, after those of the same time; nothing when it holds
    /// the item already.
    fn add(&mut self, item: BatchItem) {
        if !self.holds(&item) {
            let at = self.items.partition_point(|i| i.at() <= item.at());
            self.items.insert(at, item);
        }
    }

    fn to_json(&self) -> Value {
        json!({"task_id": self.task.to_base64(), "items": self.items.iter().map(|i| i.to_json()).collect::<Vec<_>>()})
    }
}

/// The batch a notification's `originator` records; none for a notification that is not one: one
/// made from anything but a step added or a comment on a task or on a step's highlight, or before
/// batches existed. An item this build does not know is left out.
pub fn batch_of(originator: &Value) -> Option<Batch> {
    let batch = originator.get("batch")?;
    let items = batch.get("items")?.as_array()?.iter().filter_map(BatchItem::from_json).collect();
    Some(Batch { task: id_at(batch, "task_id")?, items })
}

/// More specific types win when one recipient would get several notifications for one cause.
fn precedence(t: NotificationType) -> u8 {
    use NotificationType::*;
    match t {
        QuestionAsked => 0,
        Replied => 1,
        Mentioned => 2,
        ReviewRequested => 3,
        DecisionRepealed => 4,
        DecisionSuperseded => 5,
        TaskCreated => 6,
        TaskProposed => 7,
        DecisionProposed => 8,
        TaskAssigned => 9,
        TaskUnblocked => 10,
        CodeCommented => 11,
        DecisionCommented => 12,
        TaskCommented => 13,
        StepAdded => 15,
        Digest => 16,
    }
}

/// Whom what reaches a principal reaches now. A principal a successor took over, when an agent was
/// relaunched to carry on its job (`fwd principal succeed`; docs/DECISIONS.md, D114), is retired,
/// and no agent reads its inbox, so each plan for it goes to the last of its successors, recording
/// whom it was for (`for_id`, which the inbox's sentence names: `reason::sentence`). The same
/// successors edit the messages it wrote (`rules::can_edit_message`, given [`Successors::chain`]).
pub struct Successors(HashMap<EntityId, EntityId>);

impl Successors {
    /// Each principal of `principals` that a successor took over, with that successor.
    pub fn of(principals: &[PrincipalView]) -> Successors {
        Successors(
            principals
                .iter()
                .filter_map(|p| Some((p.id(), EntityId::from(p.succeeded_by().ok().flatten()?))))
                .collect(),
        )
    }

    /// The successors of the project's principals, read from its graph.
    pub async fn load(ctx: &Context) -> Result<Successors> {
        Ok(Successors::of(&ops::principals(ctx).await?))
    }

    /// The principals that took over `principal`'s job in turn: its successor first, then that
    /// one's, to the last; none when no one took it over. A loop, which `fwd principal succeed`
    /// never makes, ends where it would come round again.
    pub fn chain(&self, principal: EntityId) -> Vec<EntityId> {
        let mut chain = Vec::new();
        let mut at = principal;
        while let Some(&next) = self.0.get(&at) {
            if next == principal || chain.contains(&next) {
                break;
            }
            chain.push(next);
            at = next;
        }
        chain
    }

    /// The last successor of `principal`, following each successor's own in turn ([`Self::chain`]),
    /// or `principal` when no one took it over.
    pub fn last(&self, principal: EntityId) -> EntityId {
        self.chain(principal).last().copied().unwrap_or(principal)
    }

    /// `planned`, each plan for a principal a successor took over given to its last successor,
    /// with whom it was for in its `originator` (`for_id`), unless that says so already.
    pub fn redirect(&self, planned: Vec<Planned>) -> Vec<Planned> {
        planned
            .into_iter()
            .map(|mut p| {
                let to = self.last(p.recipient);
                if to != p.recipient {
                    if p.originator.get("for_id").is_none() {
                        p.originator["for_id"] = json!(p.recipient.to_base64());
                    }
                    p.recipient = to;
                }
                p
            })
            .collect()
    }
}

/// `planned` as it is delivered: each plan for a principal a successor took over given to the
/// successor ([`Successors`]), then one per recipient, `exclude`, whoever caused it, getting none
/// ([`dedupe`]), so that a successor never hears of what it did itself.
async fn deliver(ctx: &Context, planned: Vec<Planned>, exclude: Option<EntityId>) -> Result<Vec<Planned>> {
    if planned.is_empty() {
        return Ok(planned);
    }
    Ok(dedupe(Successors::load(ctx).await?.redirect(planned), exclude))
}

/// One notification per recipient per cause; `exclude`, whoever caused it, gets none. Of two
/// plans of one type for one recipient, the first planned wins, so planners list the most specific
/// reason first.
pub fn dedupe(planned: Vec<Planned>, exclude: Option<EntityId>) -> Vec<Planned> {
    let mut best: HashMap<EntityId, Planned> = HashMap::new();
    for p in planned {
        if Some(p.recipient) == exclude {
            continue;
        }
        match best.get(&p.recipient) {
            Some(existing) if precedence(existing.notification_type) <= precedence(p.notification_type) => {}
            _ => {
                best.insert(p.recipient, p);
            }
        }
    }
    let mut out: Vec<Planned> = best.into_values().collect();
    out.sort_by_key(|p| p.recipient);
    out
}

/// A task's assignee, if it is a task and has one: the one doing it, whom a comment on the task
/// reaches (rule 2) and, since scopes lost their owner, a comment on code near it (rule 1).
async fn assignee(ctx: &Context, task: EntityId) -> Result<Option<EntityId>> {
    match ctx.get::<TaskView>(task).await {
        Ok(t) => ops::assignee_of(&t),
        Err(_) => Ok(None),
    }
}

/// The people and agents a message names inline: the ends of its `mention` relations that are
/// principals, which `ops::post_message` wrote with it. A relation an edit wrote is left to
/// [`plan_for_edited_mention`], which tells its end once whichever of the two runs first.
async fn mentioned(ctx: &Context, message: EntityId) -> Result<Vec<EntityId>> {
    let mut out = Vec::new();
    for rel in ops::relations_from(ctx, message, Some(RelationKind::Mention)).await? {
        if rel.status()? != RelationStatus::Accepted || written_by_edit(&rel) {
            continue;
        }
        let to: EntityId = rel.to_id()?;
        if !out.contains(&to) && ctx.get::<PrincipalView>(to).await.is_ok() {
            out.push(to);
        }
    }
    Ok(out)
}

/// What a notification caused by a message records of it: the message, its author as the actor,
/// and what it is about.
fn message_originator(msg: &MessageView, mc: &ops::MessageContext) -> Result<Value> {
    let author: EntityId = msg.author()?.into();
    let mut originator = json!({"message_id": msg.id().to_base64(), "actor_id": author.to_base64()});
    if let Some(t) = mc.task_id {
        originator["task_id"] = json!(t.to_base64());
    }
    if let Some(a) = mc.anchor_id {
        originator["anchor_id"] = json!(a.to_base64());
    }
    if let Some(sc) = mc.scope_id {
        originator["scope_id"] = json!(sc.to_base64());
    }
    if let Some(d) = mc.decision_id {
        originator["decision_id"] = json!(d.to_base64());
    }
    Ok(originator)
}

/// The tasks a highlight is a step of, by the `step_of` relations from it that hold.
async fn tasks_stepped(ctx: &Context, anchor: EntityId) -> Result<Vec<EntityId>> {
    let mut tasks = Vec::new();
    for rel in ops::relations_from(ctx, anchor, Some(RelationKind::StepOf)).await? {
        if crate::rules::holds(RelationKind::StepOf, rel.status()?) {
            tasks.push(rel.to_id()?);
        }
    }
    Ok(tasks)
}

/// The tasks anchored near a highlight, in the same file within [`NEARBY_LINES`] of it, or anywhere
/// in it when either is on the whole file (rule 1), each once, in the order of their anchors.
async fn tasks_near(ctx: &Context, anchor: EntityId) -> Result<Vec<EntityId>> {
    let Ok(view) = ctx.get::<CodeAnchorView>(anchor).await else { return Ok(vec![]) };
    let (path, lines) = (view.path()?, ops::lines_of(&view)?);
    let mut tasks = Vec::new();
    for other in ops::anchors_in_path(ctx, &path).await? {
        if !nearby(lines, ops::lines_of(&other)?) {
            continue;
        }
        for rel in ops::relations_to(ctx, other.id(), Some(RelationKind::AnchoredAt)).await? {
            let task: EntityId = rel.from_id()?;
            if rel.status()? == RelationStatus::Accepted && !tasks.contains(&task) {
                tasks.push(task);
            }
        }
    }
    Ok(tasks)
}

/// Adds `scope` to `scopes` when it is one and not there yet.
fn push_scope(scopes: &mut Vec<EntityId>, scope: Option<EntityId>) {
    if let Some(scope) = scope.filter(|s| !scopes.contains(s)) {
        scopes.push(scope);
    }
}

/// The scopes a message concerns, whose triage agents answer first for it (docs/DECISIONS.md,
/// D122: what is asked of no one reaches "the triage agent of the scope it concerns"; `mc` holding
/// what the message is about, as `ops::MessageContext` reads it): the scope it is about; the scopes
/// the task it is about is worked in (`ops::worked_in`); for a highlight, those of the tasks it is a
/// step of and of the tasks anchored near it; the scope the decision it is about governs; each
/// once, in that order. Where none of these finds a scope, the scope its author wrote it in
/// (`written_in`), so that what concerns no scope's work reaches the triage agent of the scope its
/// author works in; none beyond that, when the triage agent that serves the whole project answers
/// for it.
pub async fn scopes_concerned(ctx: &Context, mc: &ops::MessageContext) -> Result<Vec<EntityId>> {
    let mut scopes = Vec::new();
    push_scope(&mut scopes, mc.scope_id);
    if let Some(task) = mc.task_id {
        for scope in ops::worked_in(ctx, task).await? {
            push_scope(&mut scopes, Some(scope));
        }
    }
    if let Some(anchor) = mc.anchor_id {
        for task in tasks_stepped(ctx, anchor).await?.into_iter().chain(tasks_near(ctx, anchor).await?) {
            for scope in ops::worked_in(ctx, task).await? {
                push_scope(&mut scopes, Some(scope));
            }
        }
    }
    if let Some(decision) = mc.decision_id {
        if let Ok(d) = ctx.get::<DecisionView>(decision).await {
            push_scope(&mut scopes, ops::governs(&d)?);
        }
    }
    if scopes.is_empty() {
        push_scope(&mut scopes, mc.written_in);
    }
    Ok(scopes)
}

/// The scope a question asked of no one in particular asks: the one it recorded as it was posted
/// (`ops::ASKS_SCOPE_KEY`), else, for a question from before 2026-09-30, which recorded none, the
/// one [`scope_to_ask`] picks; none when it asks the project.
pub async fn asked_scope(ctx: &Context, mc: &ops::MessageContext) -> Result<Option<EntityId>> {
    match mc.asks_scope {
        Some(scope) => Ok(Some(scope)),
        None => scope_to_ask(ctx, mc).await,
    }
}

/// The one scope a question asked of no one in particular asks when its asker names none, so that
/// one triage agent answers it (docs/DECISIONS.md, D111, "A message reaches only whom it is
/// addressed to"), which `ops::post_message` records as the question is posted: of the scopes what
/// it is about concerns ([`scopes_concerned`]), the one its asker wrote it in (`written_in`) when
/// that is one of them, else the first, which for a task worked in several scopes is the scope it
/// came to be worked in first (the lead, 2026-10-03, on D136, "A task may be worked in several
/// scopes"); none when what it is about concerns no scope and its asker works in none.
pub async fn scope_to_ask(ctx: &Context, mc: &ops::MessageContext) -> Result<Option<EntityId>> {
    let concerned = scopes_concerned(ctx, mc).await?;
    Ok(mc.written_in.filter(|own| concerned.contains(own)).or_else(|| concerned.first().copied()))
}

/// Whom a message asks, as rule 5 gives it its question: what [`asked_of`] answers.
enum Asked {
    /// The one principal its `to` names, by id, or by a handle no other principal of the project
    /// has (a handle several share names none of them: docs/design/agents-and-inbox.md, "A
    /// principal's name").
    Principal(EntityId),
    /// For a question asked of no one in particular, the triage agent that answers first for the
    /// scope it asks ([`asked_scope`], `ops::front_line`).
    FrontLine(ops::FrontLine),
}

impl Asked {
    /// The principal the question reaches.
    fn recipient(&self) -> EntityId {
        match self {
            Asked::Principal(principal) => *principal,
            Asked::FrontLine(f) => f.agent,
        }
    }
}

/// Whom the message `mc` describes asks (rule 5), so that the rule and a step's list of the threads
/// on its lines, which leaves a question out for the party it asks (`plan_for_relation`), read it
/// alike: of `principals`, the project's, the one its `to` names, or for a question asked of no one
/// in particular the triage agent that answers first for the scope it asks; none for a message that
/// asks no one, or no one this project has, or while no triage agent answers for its scope.
async fn asked_of(ctx: &Context, principals: &[PrincipalView], mc: &ops::MessageContext) -> Result<Option<Asked>> {
    if let Some(to) = mc.to.as_deref() {
        if let Ok(id) = to.parse::<EntityId>() {
            return Ok(Some(Asked::Principal(id)));
        }
        return Ok(match naming::by_name(principals, to, None).as_slice() {
            [principal] => Some(Asked::Principal(principal.id())),
            _ => None,
        });
    }
    if mc.asks_project {
        return Ok(ops::front_line(principals, asked_scope(ctx, mc).await?).map(Asked::FrontLine));
    }
    Ok(None)
}

/// `p`, a plan for the triage agent `f`, naming the scope `f` serves when it answers as that
/// scope's own (`triage_scope_id`), so that its inbox's sentence says which scope it triages.
fn for_front_line(p: Planned, f: ops::FrontLine) -> Planned {
    match f.scope {
        Some(scope) => p.naming("triage_scope_id", scope),
        None => p,
    }
}

/// Rules 1 to 6, and the rule for a comment on a highlight that is a step, for a new message.
pub async fn plan_for_message(ctx: &Context, msg: &MessageView) -> Result<Vec<Planned>> {
    let author: EntityId = msg.author()?.into();
    let priority = msg.priority()?;
    let mc = ops::MessageContext::from_view(msg)?;
    let msg_target = target("Message", msg.id());
    let originator = message_originator(msg, &mc)?;
    let mut planned = Vec::new();
    let note = |recipient: EntityId, t: NotificationType, tgt: Value, why: Why| {
        Planned::new(recipient, t, priority, tgt, &originator, why)
    };
    let principals = ops::principals(ctx).await?;
    // Whether it asks someone: one principal, or no one in particular. A forward is a reply that
    // does.
    let addressed = mc.to.is_some() || mc.asks_project;

    // Addressed to someone ([`asked_of`]): one principal, by id, or by handle on a message that
    // holds one, when no other principal of the project has that handle; or no one in particular,
    // which the triage agent of the scope it asks hears, else the one that serves the whole
    // project, and while neither exists the agent that comes to answer first for it, as it does
    // ([`plan_for_waiting_questions`]).
    let asked = if mc.reply_to.is_some() { Why::Forwarded } else { Why::Addressee };
    match asked_of(ctx, &principals, &mc).await? {
        Some(Asked::Principal(id)) => {
            planned.push(note(id, NotificationType::QuestionAsked, msg_target.clone(), asked))
        }
        // A question written just before an agent came to answer first for it, or while the daemon
        // told that agent of the questions that waited, may have reached it that way before this
        // plan runs; it hears of the question once.
        Some(Asked::FrontLine(f)) if !asked_already(ctx, f.agent, msg.id()).await? => {
            let question = note(f.agent, NotificationType::QuestionAsked, msg_target.clone(), Why::Triage);
            planned.push(for_front_line(question, f));
        }
        Some(Asked::FrontLine(_)) | None => {}
    }
    // A reply reaches the thread's participants: whoever wrote the message it answers, whoever
    // started the thread, and whoever wrote in it, in that order, so the most specific reason is
    // the one kept. A reply addressed to someone (a forward) is not for them: see below.
    if let (Some(reply_to), false) = (mc.reply_to, addressed) {
        let root_id = msg.thread()?.map(EntityId::from).unwrap_or(reply_to);
        let mut participants = Vec::new();
        for (id, why) in [(reply_to, Why::ParentAuthor), (root_id, Why::ThreadAuthor)] {
            if let Ok(m) = ctx.get::<MessageView>(id).await {
                participants.push((EntityId::from(m.author()?), why));
            }
        }
        let thread: Vec<MessageView> = ops::messages_about(ctx, "thread", root_id).await?;
        for m in thread {
            participants.push((m.author()?.into(), Why::Participant));
        }
        for (p, why) in participants {
            planned.push(note(p, NotificationType::Replied, msg_target.clone(), why));
        }
    }
    // The people it names inline.
    for principal in mentioned(ctx, msg.id()).await? {
        planned.push(note(principal, NotificationType::Mentioned, msg_target.clone(), Why::Mentioned));
    }
    // The triage agents that answer first for a comment on a task, on code or on a scope (rules 1
    // to 3): the one of each scope it concerns, else the one that serves the whole project.
    let front = match (mc.task_id, mc.anchor_id, mc.scope_id) {
        (None, None, None) => Vec::new(),
        _ => ops::front_lines(&principals, &scopes_concerned(ctx, &mc).await?),
    };
    let triage_notes = |t: NotificationType, tgt: Value| {
        front.iter().map(move |f| for_front_line(note(f.agent, t, tgt.clone(), Why::Triage), *f)).collect::<Vec<_>>()
    };
    // A comment on a task: its assignee and its triage agents.
    if let Some(task) = mc.task_id {
        if let Some(assignee) = assignee(ctx, task).await? {
            planned.push(note(assignee, NotificationType::TaskCommented, target("Task", task), Why::Assignee));
        }
        planned.extend(triage_notes(NotificationType::TaskCommented, target("Task", task)));
    }
    // A comment on a highlight that is a step of tasks (a message on code, with no task and
    // answering nothing): each such task's creator and assignee, whose checklist it is on, for the
    // steps the highlight is as routing reaches the comment. One of them told of the comment
    // already is left out below.
    let mut step_of = Vec::new();
    let mut step_parties = Vec::new();
    if let (Some(anchor), None, None) = (mc.anchor_id, mc.task_id, mc.reply_to) {
        for task in tasks_stepped(ctx, anchor).await? {
            let Ok(t) = ctx.get::<TaskView>(task).await else { continue };
            step_of.push(task);
            let parties = [Some(EntityId::from(t.created_by()?)), ops::assignee_of(&t)?];
            for party in parties.into_iter().flatten() {
                step_parties.push(party);
                let step = note(party, NotificationType::CodeCommented, target("CodeAnchor", anchor), Why::StepParty);
                planned.push(step.naming("step_task_id", task));
            }
        }
    }
    // A comment on code: its triage agents, and the assignees of the tasks anchored in the same file
    // within `NEARBY_LINES` of the comment, or anywhere in it when the comment or the task is on
    // the whole file (docs/design/agents-and-inbox.md, rule 1). A task no one is assigned reaches no
    // one here. Until 2026-09-29 the rule reached the owners of the scopes those tasks were worked
    // in (D108).
    if let Some(anchor) = mc.anchor_id {
        planned.extend(triage_notes(NotificationType::CodeCommented, target("CodeAnchor", anchor)));
        for task in tasks_near(ctx, anchor).await? {
            if let Some(assignee) = assignee(ctx, task).await? {
                let near = note(assignee, NotificationType::CodeCommented, target("CodeAnchor", anchor), Why::Assignee);
                planned.push(near.naming("near_task_id", task));
            }
        }
    }
    // A comment on a scope: the scope's triage agent, else the one that serves the whole project
    // (rule 3), since a scope has no owner to hear it (D108); until 2026-09-29 it reached the
    // scope's owner.
    if let Some(scope) = mc.scope_id {
        planned.extend(triage_notes(NotificationType::TaskCommented, target("Scope", scope)));
    }
    // A comment on a decision (the reason for a move among them): whoever filed it and whoever
    // ruled on it.
    if let Some(decision) = mc.decision_id {
        if let Ok(d) = ctx.get::<DecisionView>(decision).await {
            let about = target("Decision", decision);
            planned.push(note(
                d.author()?.into(),
                NotificationType::DecisionCommented,
                about.clone(),
                Why::DecisionAuthor,
            ));
            if let Some(decider) = d.decided_by()? {
                planned.push(note(decider.into(), NotificationType::DecisionCommented, about, Why::Decider));
            }
        }
    }
    // What would reach a principal a successor took over reaches the successor (D114), before the
    // rules below read who the recipients are.
    let successors = Successors::load(ctx).await?;
    let mut planned = successors.redirect(planned);
    // A message always notifies the party to which it was directed, even when that party wrote it,
    // so a question asked of oneself stays in one's own inbox until it is answered or dismissed, and
    // a triage agent that asks the project hears its own question. Every other rule leaves the
    // author out. (`QuestionAsked` comes only from whom a message asks: its `to`, or the project.)
    planned.retain(|p| p.recipient != author || p.notification_type == NotificationType::QuestionAsked);
    // A message addressed to someone reaches only whom it is addressed to and whom it mentions
    // (`ADDRESSED_GIVES`, D111); the people of what it is about see it when they open its thread.
    if addressed {
        planned.retain(|p| ADDRESSED_GIVES.contains(&p.notification_type));
    }
    // A party of a step the highlight is, told of the comment already, is told nothing more of it:
    // the step added that lists it as a thread the step came with did, when routing reached the
    // step first with the comment already written (`plan_for_relation`). So each party hears of the
    // comment once, through whichever of the two events routing reached first. A question still
    // reaches whom it asks, for whom that list leaves it out.
    let mut checked = HashSet::new();
    let mut told = HashSet::new();
    for party in step_parties {
        let reached = successors.last(party);
        if checked.insert(reached) && told_messages(ctx, reached).await?.contains(&msg.id()) {
            told.insert(reached);
        }
    }
    planned.retain(|p| !told.contains(&p.recipient) || p.notification_type == NotificationType::QuestionAsked);
    // The reason for a repeal or a supersession reaches whoever authors something citing the
    // decision through the move's own notification (rule 12), which carries this comment; told
    // here too, they would hear of one move twice.
    if let Some(decision) = mc.decision_id.filter(|_| gives_reason_for_fall(msg)) {
        let told: Vec<EntityId> =
            citing_authors(ctx, decision).await?.into_iter().map(|author| successors.last(author)).collect();
        planned.retain(|p| !told.contains(&p.recipient));
    }
    let mut planned = dedupe(planned, None);
    // A comment that opens a thread and asks no one, on a task or on the highlight of one of its
    // steps, is one item of what its author does on that task, which each recipient hears of in
    // one notification (rule 10): as a comment on the task (rule 2), on the step's highlight
    // (rule 14), or on the code (rule 1). A reply, a question and a mention keep a notification of
    // their own.
    if mc.reply_to.is_none() && !addressed {
        let item = BatchItem::Comment { message: msg.id(), at: msg.created()? };
        let about = mc.task_id.or(step_of.first().copied());
        for p in &mut planned {
            let task = match p.notification_type {
                NotificationType::TaskCommented if p.target["type"] == "Task" => mc.task_id,
                NotificationType::CodeCommented => id_at(&p.originator, "step_task_id").or(about),
                _ => None,
            };
            if let Some(task) = task {
                p.originator["batch"] = Batch::of_one(task, item).to_json();
            }
        }
    }
    Ok(planned)
}

/// Rule 5 for the questions that waited: each active question asked of no one in particular that
/// no triage agent has heard of ([`heard_by_triage`]) reaches the triage agent that answers first
/// for the scope it asks now ([`asked_scope`], `ops::front_line`), so that a question asked while no
/// agent answered for its scope reaches that scope's front line the moment it has one, rather than
/// wait unheard until someone looks at the Questions page (Daniel, 2026-09-30, of a question he had
/// asked of a project with no triage agent: "lets make sure that does not get lost in the
/// shuffle"; task 5RoOJZl9; per scope since D122). The routing service plans this whenever what a
/// triage agent serves changes: an agent that takes triage, a scope or the endorsement to serve the
/// whole project (`fwd principal add --role triage`, `role`, `scope add`, `scope project-wide`,
/// coming back with them, or succeeding the agent that had them) hears of the questions of what it
/// takes, a project-wide one of those asked of no scope and of scopes no agent serves; and one that
/// gives them up (retired, another role, `scope remove`) leaves those it had not heard of, and
/// those it left unread, to whichever agent answers first for them then. This plans `QuestionAsked`,
/// `why` triage, one notification each, oldest first, as `plan_for_message` plans one for the
/// triage agent of the moment; a question whose scope no agent answers for keeps waiting. Its plans
/// are not passed through [`dedupe`], which keeps one per recipient, since each is a question of
/// its own. Until 2026-09-30 it planned for the one agent that came to hold the project's triage
/// (`plan_for_triage_taken`).
pub async fn plan_for_waiting_questions(ctx: &Context) -> Result<Vec<Planned>> {
    let principals = ops::principals(ctx).await?;
    let heard = heard_by_triage(ctx, &principals).await?;
    let active = format!("status = '{}'", MessageStatus::Active);
    let mut waiting = Vec::new();
    for msg in ctx.fetch::<MessageView>(active.as_str()).await? {
        if heard.contains(&msg.id()) {
            continue;
        }
        // A message whose fields cannot be read is left to the Questions page, rather than keep
        // the questions after it from their agents.
        let Ok(mc) = ops::MessageContext::from_view(&msg) else { continue };
        if !mc.asks_project || mc.to.is_some() {
            continue;
        }
        if let Some(f) = ops::front_line(&principals, asked_scope(ctx, &mc).await?) {
            waiting.push((msg.created().unwrap_or(0), msg, mc, f));
        }
    }
    waiting.sort_by_key(|(created, msg, ..)| (*created, msg.id()));
    let mut planned = Vec::new();
    for (_, msg, mc, f) in waiting {
        let originator = message_originator(&msg, &mc)?;
        let question = target("Message", msg.id());
        let priority = msg.priority()?;
        let told = Planned::new(f.agent, NotificationType::QuestionAsked, priority, question, &originator, Why::Triage);
        planned.push(for_front_line(told, f));
    }
    Ok(planned)
}

/// The questions a triage agent has heard of, which no triage agent is told of again
/// ([`plan_for_waiting_questions`]): each message a `QuestionAsked` with `why` triage names, that
/// its recipient read or dismissed, or holds unread while it is active (of `principals`, the
/// project's). The agent that succeeded a triage agent holds what that one left unread
/// (`ops::succeed_principal` moves it), so a question told to the triage agent it succeeded counts;
/// a triage agent retired with no successor reads nothing more, so a question it left unread counts
/// as heard by no one, and reaches the next. A `QuestionAsked` with no `why` was routed before
/// routing recorded it (2026-09-26), when a question to the supervisor role, asked of the project
/// since `fwd admin migrate`, reached the supervisors, and counts as one with `why` triage, as the
/// inbox's sentence reads it (`reason.rs`).
async fn heard_by_triage(ctx: &Context, principals: &[PrincipalView]) -> Result<HashSet<EntityId>> {
    let active: HashSet<EntityId> = principals.iter().filter(|p| p.active().unwrap_or(false)).map(|p| p.id()).collect();
    let questions = format!("notification_type = '{}'", NotificationType::QuestionAsked);
    let asked: Vec<NotificationView> = ctx.fetch(questions.as_str()).await?;
    Ok(asked
        .iter()
        .filter_map(|n| {
            let originator: Value = n.originator().ok()?.into();
            let for_triage = originator.get("why").is_none() || why_of(&originator) == Some(Why::Triage);
            let read = n.status().ok()? != NotificationStatus::Unread;
            let reads_still = active.contains(&EntityId::from(n.recipient().ok()?));
            (for_triage && (read || reads_still)).then(|| id_at(&originator, "message_id")).flatten()
        })
        .collect())
}

/// Whether `recipient` holds a `QuestionAsked` for `message` already, whatever its status.
async fn asked_already(ctx: &Context, recipient: EntityId, message: EntityId) -> Result<bool> {
    let predicate = format!("recipient = ? AND notification_type = '{}'", NotificationType::QuestionAsked);
    let theirs: Vec<NotificationView> = ctx.fetch(with_id(&predicate, recipient)?).await?;
    Ok(theirs.iter().any(|n| {
        let originator: Value = n.originator().map(Into::into).unwrap_or(Value::Null);
        id_at(&originator, "message_id") == Some(message)
    }))
}

/// Whether a message is the reason a decision was repealed or superseded: the comment the move
/// writes, which names the move in `context.move`.
fn gives_reason_for_fall(msg: &MessageView) -> bool {
    let context: Value = msg.context().map(Into::into).unwrap_or(Value::Null);
    let fallen = [DecisionStatus::Repealed.to_string(), DecisionStatus::Superseded.to_string()];
    context.get(ops::MOVE_KEY).and_then(Value::as_str).is_some_and(|m| fallen.iter().any(|f| f == m))
}

/// The authors of everything that stands on `decision` by a standing `cites` relation: whom its
/// repeal or supersession tells.
async fn citing_authors(ctx: &Context, decision: EntityId) -> Result<Vec<EntityId>> {
    let mut authors = Vec::new();
    for rel in ops::relations_to(ctx, decision, Some(RelationKind::Cites)).await? {
        if !ops::standing(&rel) {
            continue;
        }
        if let Some(author) = ops::author_of(ctx, rel.from_id()?).await? {
            authors.push(author);
        }
    }
    Ok(authors)
}

/// A new task: TaskProposed, when it is Proposed, to the triage agent of each scope it is worked in
/// (`ops::worked_in`), each scope's own, else the one that serves the whole project, each agent once
/// (docs/DECISIONS.md, D136: "A proposal reaches the triage agent of each of its scopes"), and for a
/// task worked in none to the one that serves the whole project; TaskCreated to the authors of the
/// messages it was discovered from or motivated by.
pub async fn plan_for_task_created(ctx: &Context, task: &TaskView) -> Result<Vec<Planned>> {
    let creator: EntityId = task.created_by()?.into();
    let priority = Priority::Next;
    let originator = json!({"actor_id": creator.to_base64(), "task_id": task.id().to_base64()});
    let tgt = target("Task", task.id());
    let mut planned = Vec::new();
    if task.status()? == TaskStatus::Proposed {
        for f in ops::triage_for_scopes(ctx, &ops::worked_in(ctx, task.id()).await?).await? {
            let proposed =
                Planned::new(f.agent, NotificationType::TaskProposed, priority, tgt.clone(), &originator, Why::Triage);
            planned.push(for_front_line(proposed, f));
        }
    }
    for rel in ops::relations_from(ctx, task.id(), None).await? {
        if !matches!(rel.kind()?, RelationKind::DiscoveredFrom | RelationKind::MotivatedBy) {
            continue;
        }
        let source: EntityId = rel.to_id()?;
        if let Ok(m) = ctx.get::<MessageView>(source).await {
            let created = Planned::new(
                m.author()?.into(),
                NotificationType::TaskCreated,
                priority,
                tgt.clone(),
                &originator,
                Why::SourceAuthor,
            );
            planned.push(created.naming("message_id", source));
        }
    }
    deliver(ctx, planned, Some(creator)).await
}

/// A status change: ReviewRequested on submit to the triage agent of each scope the task is worked
/// in (`ops::worked_in`), each scope's own, else the one that serves the whole project, each agent
/// once, which accepts or returns it only where a person's words or a recorded decision settle it
/// and brings the rest to its lead (docs/DECISIONS.md, D129); to whoever approved the task, or else
/// its creator, only when no triage agent can take it; never to the submitter. TaskUnblocked to the
/// assignees of dependents that just became ready (or, for one nobody holds, to the triage agent of
/// each scope it is worked in) on close. Each scope's agent hears of a task worked in several, since
/// none of its scopes is primary (D136, "A task may be worked in several scopes"; the lead,
/// 2026-10-03, for reviews and tasks that became ready).
pub async fn plan_for_task_status(
    ctx: &Context,
    task: &TaskView,
    previous: TaskStatus,
    actor: Option<EntityId>,
) -> Result<Vec<Planned>> {
    let status = task.status()?;
    if status == previous {
        return Ok(vec![]);
    }
    let tgt = target("Task", task.id());
    let mut originator =
        json!({"task_id": task.id().to_base64(), "from": previous.to_string(), "to": status.to_string()});
    if let Some(a) = actor {
        originator["actor_id"] = json!(a.to_base64());
    }
    let mut planned = Vec::new();
    if status == TaskStatus::InReview {
        // Review goes to a triage agent first, which settles only what a person's words or a ruling
        // settle and brings the rest to its lead (D129), so that it stays out of a person's inbox:
        // Daniel found nine review requests in his own inbox on 2026-09-26 ("some cases where its
        // too much noise"). The triage agent of each of the task's scopes takes it, else the one
        // that serves the whole project; none takes its own submission, so one a scope's agent
        // submitted goes to the other scopes' agents, or with none to the agent that serves the
        // whole project, and where none can take it, as in a project with no triage agent, to the
        // approver or the creator.
        let principals = ops::principals(ctx).await?;
        let scopes = ops::worked_in(ctx, task.id()).await?;
        let not_submitter = |f: &ops::FrontLine| Some(f.agent) != actor;
        let mut reviewers: Vec<ops::FrontLine> =
            ops::front_lines(&principals, &scopes).into_iter().filter(not_submitter).collect();
        if reviewers.is_empty() {
            reviewers.extend(ops::front_line(&principals, None).filter(not_submitter));
        }
        let review = |reviewer: EntityId, why: Why| {
            Planned::new(reviewer, NotificationType::ReviewRequested, Priority::Next, tgt.clone(), &originator, why)
        };
        for f in &reviewers {
            planned.push(for_front_line(review(f.agent, Why::Triage), *f));
        }
        if reviewers.is_empty() {
            let context: Value = task.context()?.into();
            let approver = context.get("approved_by").and_then(Value::as_str).and_then(|s| s.parse::<EntityId>().ok());
            planned.push(match approver {
                Some(approver) => review(approver, Why::Approver),
                None => review(task.created_by()?.into(), Why::Creator),
            });
        }
    }
    if matches!(status, TaskStatus::Done | TaskStatus::Cancelled | TaskStatus::Rejected) {
        for rel in ops::relations_to(ctx, task.id(), Some(RelationKind::DependsOn)).await? {
            if rel.status()? != RelationStatus::Accepted {
                continue;
            }
            let dependent: EntityId = rel.from_id()?;
            let Ok(dep_view) = ctx.get::<TaskView>(dependent).await else { continue };
            if !ops::readiness(ctx, &dep_view).await?.is_ready() {
                continue;
            }
            let dep_target = target("Task", dependent);
            match ops::assignee_of(&dep_view)? {
                Some(assignee) => planned.push(Planned::new(
                    assignee,
                    NotificationType::TaskUnblocked,
                    Priority::Next,
                    dep_target,
                    &originator,
                    Why::Assignee,
                )),
                None => {
                    for f in ops::triage_for_scopes(ctx, &ops::worked_in(ctx, dependent).await?).await? {
                        let unblocked = Planned::new(
                            f.agent,
                            NotificationType::TaskUnblocked,
                            Priority::Later,
                            dep_target.clone(),
                            &originator,
                            Why::Triage,
                        );
                        planned.push(for_front_line(unblocked, f));
                    }
                }
            }
        }
    }
    deliver(ctx, planned, actor).await
}

/// A task's assignee changed from `previous` to the one it has now: TaskAssigned to the new
/// assignee (rule 7), never to `actor`, who made the change. An assignee cleared tells no one. It
/// reads nothing but the task, and who succeeded the assignee when a successor took it over.
pub async fn plan_for_assignee(
    ctx: &Context,
    task: &TaskView,
    previous: Option<EntityId>,
    actor: Option<EntityId>,
) -> Result<Vec<Planned>> {
    let Some(assignee) = ops::assignee_of(task)? else { return Ok(vec![]) };
    if Some(assignee) == previous {
        return Ok(vec![]);
    }
    let mut originator = json!({"task_id": task.id().to_base64()});
    if let Some(a) = actor {
        originator["actor_id"] = json!(a.to_base64());
    }
    if let Some(p) = previous {
        originator["previous_id"] = json!(p.to_base64());
    }
    let assigned = Planned::new(
        assignee,
        NotificationType::TaskAssigned,
        Priority::Next,
        target("Task", task.id()),
        &originator,
        Why::Assignee,
    );
    deliver(ctx, vec![assigned], actor).await
}

/// A thread open on a highlight, as the plan for a step made of the highlight reads it: its first
/// message, everyone who wrote in it, whom it asks (rule 5), when it was written and how urgent.
struct OpenThread {
    message: EntityId,
    writers: Vec<EntityId>,
    asks: Option<EntityId>,
    at: i64,
    priority: Priority,
}

/// The threads open on `highlight` as routing reaches the step made of it whose relation is
/// `step`, oldest first, which the step's plan lists ([`BatchItem::Thread`]): each first message on
/// it (answering nothing) that names no task, as rule 14 reads a comment on a step's highlight, and
/// is still active, whenever it was written, but for the comment the command that added the step
/// wrote with it (`ops::FIRST_OF_STEP_KEY`), which is the step's own first comment rule 14 reports,
/// not a thread the step found on its lines. The times a message and a step were made with come
/// from their writers' clocks, which decide nothing (docs/design/data-model.md: timestamps are
/// informational): what routing finds is what it lists, and rule 14 tells nothing more of a comment
/// to a party this list told of it. A message whose fields cannot be read is left out, rather than
/// keep the step's people from hearing of the step. A step that is no highlight has none.
async fn open_threads(ctx: &Context, highlight: EntityId, step: EntityId) -> Result<Vec<OpenThread>> {
    let on_it = ops::messages_about(ctx, "anchor", highlight).await?;
    if on_it.is_empty() {
        return Ok(Vec::new());
    }
    let principals = ops::principals(ctx).await?;
    let mut threads = Vec::new();
    for m in &on_it {
        if let Ok(Some(thread)) = open_thread(ctx, &principals, m, step).await {
            threads.push(thread);
        }
    }
    Ok(threads)
}

/// `m` as an [`OpenThread`] when it opens a thread that is still open, names no task and is not
/// the first comment of the step whose relation is `step`, and none otherwise; of `principals`,
/// the project's, whom it asks.
async fn open_thread(
    ctx: &Context,
    principals: &[PrincipalView],
    m: &MessageView,
    step: EntityId,
) -> Result<Option<OpenThread>> {
    let mc = ops::MessageContext::from_view(m)?;
    let opens = mc.reply_to.is_none() && mc.task_id.is_none() && mc.first_of_step != Some(step);
    if !opens || m.status()? != MessageStatus::Active {
        return Ok(None);
    }
    let mut writers = vec![EntityId::from(m.author()?)];
    for reply in ops::messages_about(ctx, "thread", m.id()).await? {
        writers.push(reply.author()?.into());
    }
    let asks = asked_of(ctx, principals, &mc).await?.map(|a| a.recipient());
    Ok(Some(OpenThread { message: m.id(), writers, asks, at: m.created()?, priority: m.priority()? }))
}

/// A new relation: TaskProposed, when proposed, to the triage agent of each scope of the task at
/// one of its ends ([`relation_scopes`]), else the one that serves the whole project; for a
/// `mention` an edit wrote, what [`plan_for_edited_mention`] plans. A step added to a task (a
/// `step_of` relation that holds): StepAdded to the task's creator and assignee, whose checklist it
/// is on, as one item of what its author does on the task (rule 10), listing with the step the
/// threads open on its highlight as routing reaches it ([`open_threads`]) that the recipient did
/// not write in, is not asked by and was not told of, comments and questions alike. Daniel reviews
/// by writing on lines and then adding them to a checklist, and in that order his comment reached
/// neither of the task's people, and no one at all where its scope had no triage agent (task
/// WTCAd9la). A question among them is still asked only of whom it asks (rule 5), which the step's
/// list leaves it out for, since rule 5 tells them of it; so the step's people read a question
/// asked before the step with it, and one asked after the step reaches only whom it asks (D111).
/// The notification is as urgent as the most urgent of the step and the threads it lists, as a
/// digest is as urgent as its items, and names as its message the one message it lists, when it
/// lists one ([`Batch::message_behind`]). No one hears of a relation they made.
pub async fn plan_for_relation(ctx: &Context, rel: &RelationView) -> Result<Vec<Planned>> {
    let (kind, status) = (rel.kind()?, rel.status()?);
    if kind == RelationKind::Mention {
        return plan_for_edited_mention(ctx, rel).await;
    }
    let creator: EntityId = rel.created_by()?.into();
    let originator = json!({"actor_id": creator.to_base64(), "relation_id": rel.id().to_base64()});
    let mut planned = Vec::new();
    if status == RelationStatus::Proposed {
        for f in ops::triage_for_scopes(ctx, &relation_scopes(ctx, rel).await?).await? {
            let proposed = Planned::new(
                f.agent,
                NotificationType::TaskProposed,
                Priority::Next,
                target("Relation", rel.id()),
                &originator,
                Why::Triage,
            );
            planned.push(for_front_line(proposed, f));
        }
    } else if kind == RelationKind::StepOf && crate::rules::holds(kind, status) {
        let task: EntityId = rel.to_id()?;
        if let Ok(t) = ctx.get::<TaskView>(task).await {
            let step = rel.from_id()?;
            let item = BatchItem::Step { relation: rel.id(), step, at: rel.created()? };
            let threads = open_threads(ctx, step, rel.id()).await?;
            let successors = if threads.is_empty() { None } else { Some(Successors::load(ctx).await?) };
            for party in [Some(EntityId::from(t.created_by()?)), ops::assignee_of(&t)?].into_iter().flatten() {
                let mut batch = Batch::of_one(task, item);
                let mut priority = Priority::Next;
                if let Some(successors) = &successors {
                    // Whom the notification reaches: the party, or the last of its successors
                    // (D114). It is listed no thread that it, or a principal it succeeded, wrote
                    // in or is asked by, nor one it was told of already.
                    let reached = successors.last(party);
                    let theirs = |principal: EntityId| successors.last(principal) == reached;
                    let told = told_messages(ctx, reached).await?;
                    for thread in &threads {
                        let known = thread.writers.iter().any(|writer| theirs(*writer))
                            || thread.asks.is_some_and(theirs)
                            || told.contains(&thread.message);
                        if !known {
                            batch.add(BatchItem::Thread { message: thread.message, at: thread.at });
                            priority = priority.min(thread.priority);
                        }
                    }
                }
                let mut added = Planned::new(
                    party,
                    NotificationType::StepAdded,
                    priority,
                    target("Task", task),
                    &originator,
                    Why::StepParty,
                )
                .naming("step_task_id", task);
                if let Some(message) = batch.message_behind() {
                    added = added.naming("message_id", message);
                }
                added.originator["batch"] = batch.to_json();
                planned.push(added);
            }
        }
    }
    deliver(ctx, planned, Some(creator)).await
}

/// The scopes whose triage agents decide a proposed relation: the scopes the task at its `from` end
/// is worked in (`ops::worked_in`), else those of the task at its `to` end; none when neither end is
/// a task, or neither task is worked in a scope, when the triage agent that serves the whole project
/// decides it.
async fn relation_scopes(ctx: &Context, rel: &RelationView) -> Result<Vec<EntityId>> {
    for end in [rel.from_id()?, rel.to_id()?] {
        if ctx.get::<TaskView>(end).await.is_ok() {
            let scopes = ops::worked_in(ctx, end).await?;
            if !scopes.is_empty() {
                return Ok(scopes);
            }
        }
    }
    Ok(Vec::new())
}

/// Rule 13 for an edit: a `mention` relation that an edit of its message wrote (`via` carries
/// `ops::EDITED_KEY`), naming a person or an agent, gives them `Mentioned`, as the relations a
/// post writes do through [`plan_for_message`], which leaves these alone. Only the newly named
/// hear of it, once: not someone the message named before (a standing mention of them from the
/// message that was made earlier, or that comes earlier in the same edit), not whoever made the
/// edit, and not someone already told of the message (a notification it caused), since routing
/// tells a recipient of one cause once. Whoever made the edit, the relation's maker, is the one
/// who mentioned them: the author, or an agent that succeeded the author
/// (`rules::can_edit_message`), which the author's name would misattribute. Nothing else an edit
/// does tells anyone.
async fn plan_for_edited_mention(ctx: &Context, rel: &RelationView) -> Result<Vec<Planned>> {
    if !written_by_edit(rel) || rel.status()? != RelationStatus::Accepted {
        return Ok(vec![]);
    }
    let (message, named) = (rel.from_id()?, rel.to_id()?);
    let Ok(msg) = ctx.get::<MessageView>(message).await else { return Ok(vec![]) };
    if ctx.get::<PrincipalView>(named).await.is_err() {
        return Ok(vec![]);
    }
    let place = |r: &RelationView| (r.created().unwrap_or(0), ops::mention_ordinal(r));
    let named_before = ops::relations_from(ctx, message, Some(RelationKind::Mention)).await?.iter().any(|r| {
        r.id() != rel.id()
            && r.to_id().ok() == Some(named)
            && r.status().ok() == Some(RelationStatus::Accepted)
            && place(r) < place(rel)
    });
    // Told already: whoever it reaches now, the principal named or its successor (D114).
    let reached = Successors::load(ctx).await?.last(named);
    if named_before || told_of(ctx, reached, message).await? {
        return Ok(vec![]);
    }
    let editor: EntityId = rel.created_by()?.into();
    let mut originator = message_originator(&msg, &ops::MessageContext::from_view(&msg)?)?;
    originator["actor_id"] = json!(editor.to_base64());
    let mentioned = Planned::new(
        named,
        NotificationType::Mentioned,
        msg.priority()?,
        target("Message", message),
        &originator,
        Why::Mentioned,
    );
    deliver(ctx, vec![mentioned], Some(editor)).await
}

/// Whether an edit of its message wrote this `mention` relation, rather than the post.
fn written_by_edit(rel: &RelationView) -> bool {
    let via: Value = rel.via().map(Into::into).unwrap_or(Value::Null);
    via.get(ops::EDITED_KEY).and_then(Value::as_bool) == Some(true)
}

/// Whether `recipient` has a notification that `message` caused, whatever its status
/// ([`told_messages`]).
async fn told_of(ctx: &Context, recipient: EntityId, message: EntityId) -> Result<bool> {
    Ok(told_messages(ctx, recipient).await?.contains(&message))
}

/// The messages `recipient` has a notification of, whatever its status: each a notification names
/// as its message, and each a batch holds among its items, a comment or a thread a step came with,
/// since a batch names only one message (rule 10).
async fn told_messages(ctx: &Context, recipient: EntityId) -> Result<HashSet<EntityId>> {
    let theirs: Vec<NotificationView> = ctx.fetch(with_id("recipient = ?", recipient)?).await?;
    let mut told = HashSet::new();
    for n in &theirs {
        let originator: Value = n.originator().map(Into::into).unwrap_or(Value::Null);
        told.extend(id_at(&originator, "message_id"));
        told.extend(batch_of(&originator).map(|b| b.messages()).unwrap_or_default());
    }
    Ok(told)
}

/// A new decision: a proposal reaches the triage agent of the scope it governs (`ops::governs`),
/// else the one that serves the whole project, and the project's people, who may accept or reject
/// it; never its author. A ruling recorded as given notifies no one.
pub async fn plan_for_decision_created(ctx: &Context, decision: &DecisionView) -> Result<Vec<Planned>> {
    let author: EntityId = decision.author()?.into();
    if decision.status()? != DecisionStatus::Proposed {
        return Ok(vec![]);
    }
    let originator = json!({"actor_id": author.to_base64(), "decision_id": decision.id().to_base64()});
    let proposed = |recipient: EntityId, why: Why| {
        Planned::new(
            recipient,
            NotificationType::DecisionProposed,
            Priority::Next,
            target("Decision", decision.id()),
            &originator,
            why,
        )
    };
    let mut planned = Vec::new();
    if let Some(f) = ops::triage_for(ctx, ops::governs(decision)?).await? {
        planned.push(for_front_line(proposed(f.agent, Why::Triage), f));
    }
    for person in ops::people(ctx).await? {
        planned.push(proposed(person, Why::Person));
    }
    deliver(ctx, planned, Some(author)).await
}

/// A decision repealed or superseded: the authors of everything that cites it (tasks, messages
/// and decisions resting on the ruling) are told, since each needs re-justifying; never whoever
/// moved it. This is the inbox side of the retraction audit.
pub async fn plan_for_decision_status(
    ctx: &Context,
    decision: &DecisionView,
    previous: DecisionStatus,
    actor: Option<EntityId>,
) -> Result<Vec<Planned>> {
    let status = decision.status()?;
    let notification_type = match status {
        _ if status == previous => return Ok(vec![]),
        DecisionStatus::Repealed => NotificationType::DecisionRepealed,
        DecisionStatus::Superseded => NotificationType::DecisionSuperseded,
        _ => return Ok(vec![]),
    };
    let mut originator = json!({
        "decision_id": decision.id().to_base64(),
        "from": previous.to_string(),
        "to": status.to_string(),
    });
    if let Some(a) = actor {
        originator["actor_id"] = json!(a.to_base64());
    }
    // The comment that records why, written with the move, so a reader of the notification sees
    // the reason without looking it up (and `plan_for_message` leaves these recipients out of that
    // comment's own notifications).
    if let Some(reason) = decision.closed_reason_message()? {
        originator["message_id"] = json!(EntityId::from(reason).to_base64());
    }
    let mut planned = Vec::new();
    for rel in ops::relations_to(ctx, decision.id(), Some(RelationKind::Cites)).await? {
        if !ops::standing(&rel) {
            continue;
        }
        let citing: EntityId = rel.from_id()?;
        if let Some(author) = ops::author_of(ctx, citing).await? {
            let told = Planned::new(
                author,
                notification_type,
                Priority::Next,
                target("Decision", decision.id()),
                &originator,
                Why::CitingAuthor,
            );
            planned.push(told.naming("citing_id", citing));
        }
    }
    deliver(ctx, planned, actor).await
}

/// How far, in milliseconds, the clock of whoever made an item may run ahead of the daemon's for
/// the notification that holds the item to be found still ([`join`]).
const CLOCK_SLACK_MS: i64 = 10 * 60 * 1000;

/// The keys of a notification's `originator` that describe the one item it was made from, which
/// a batch of several no longer is: [`join`] drops them, and the batch's items say it instead.
const ITEM_KEYS: &[&str] =
    &["message_id", "task_id", "anchor_id", "scope_id", "decision_id", "relation_id", "step_task_id"];

/// Makes the planned notifications. A plan carrying a batch of one item (a step added, with the
/// threads it came with, or a comment on a task or on a step's highlight) joins the batch its
/// recipient already holds unread from the same author about the same task, and makes a
/// notification only when there is none (rule 10); reading or dismissing that batch ends it, so the
/// next item starts another. Answers the notifications made or joined.
pub async fn apply(ctx: &Context, planned: &[Planned]) -> Result<Vec<EntityId>> {
    let mut ids = Vec::new();
    for p in planned {
        let joined = match batch_of(&p.originator) {
            Some(batch) => join(ctx, p, batch).await?,
            None => None,
        };
        let id = match joined {
            Some(id) => id,
            None => {
                ops::create_notification(
                    ctx,
                    p.recipient,
                    p.notification_type,
                    p.priority,
                    p.target.clone(),
                    p.originator.clone(),
                )
                .await?
            }
        };
        ids.push(id);
    }
    Ok(ids)
}

/// Where the item a plan carries (its batch of one, `planned`, with the threads a step came with)
/// goes when it needs no notification of its own: the recipient's notification that holds it
/// already, so an item routed twice is held once; else the batch the recipient holds unread from
/// the same author about the same task, which it joins with its threads, and which becomes a
/// `Digest` of them all. None when there is neither.
async fn join(ctx: &Context, p: &Planned, planned: Batch) -> Result<Option<EntityId>> {
    let Some(&item) = planned.items.iter().find(|i| !matches!(i, BatchItem::Thread { .. })) else {
        return Ok(None);
    };
    let actor = p.originator.get("actor_id");
    // The unread notifications, and any changed since the item was made: one that holds it was
    // made or grew after it, by the daemon's clock, which the author's may run ahead of.
    let since = item.at() - CLOCK_SLACK_MS;
    let predicate = format!("recipient = ? AND (status = 'unread' OR created >= {since} OR updated >= {since})");
    let mut open: Option<(NotificationView, Value, Batch)> = None;
    for n in ctx.fetch::<NotificationView>(with_id(&predicate, p.recipient)?).await? {
        let originator: Value = n.originator()?.into();
        let Some(batch) = batch_of(&originator).filter(|b| b.task == planned.task) else { continue };
        if originator.get("actor_id") != actor {
            continue;
        }
        if batch.holds(&item) {
            return Ok(Some(n.id()));
        }
        let newer = open.as_ref().is_none_or(|(o, ..)| ops::notification_time(o) < ops::notification_time(&n));
        if n.status()? == NotificationStatus::Unread && newer {
            open = Some((n, originator, batch));
        }
    }
    let Some((n, mut originator, mut batch)) = open else { return Ok(None) };
    for item in &planned.items {
        batch.add(*item);
    }
    // The notification now says what the batch holds: its items, the message a reply given its
    // id answers, and the task, which is what it is about. A reason that makes the task the
    // recipient's own wins, so its sentence says "your task".
    let own = |why: Option<Why>| matches!(why, Some(Why::Assignee | Why::StepParty));
    if own(why_of(&p.originator)) && !own(why_of(&originator)) {
        originator["why"] = p.originator["why"].clone();
    }
    if let Some(fields) = originator.as_object_mut() {
        for key in ITEM_KEYS {
            fields.remove(*key);
        }
    }
    if let Some(message) = batch.message_behind() {
        originator["message_id"] = json!(message.to_base64());
    }
    originator["batch"] = batch.to_json();
    // The most urgent of its items' priorities.
    let priority = n.priority()?.min(p.priority);
    ops::update_notification(ctx, &n, NotificationType::Digest, priority, target("Task", batch.task), originator)
        .await?;
    Ok(Some(n.id()))
}
