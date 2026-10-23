//! What waits on the inbox's reader, sorted apart from what only tells them something (D137, "The
//! inbox shows what waits on its reader above the conversations, in sections stacked as an
//! accordion"). A notification either asks its reader to make a move, a request, or tells them
//! something, a conversation, and a request keeps asking until what it is about changes, which
//! reading it does not change. Until 2026-10-03 the page drew every notification as a
//! conversation's card in the order they arrived, so nothing on it answered what waited on the
//! reader: that morning the badge counted 212 unread notifications while eleven things waited on
//! Daniel, and reading an assignment hid it while its task still waited (task JQyHye7l, from the
//! design pass of task RgAEFRFb).
//!
//! So the page sorts each notification by its type and by the live state of what it is about,
//! which the page's standing queries hold. A request whose subject still waits on the reader makes
//! one row per subject in Waiting on you ([`Row`]), which every other request about that subject
//! joins (a task's assignment and its "now ready" share one), and the row stays, read or not,
//! until its move is made or it is dismissed. Everything else is a conversation, drawn as today's
//! card (`notification_card.rs`), and so is a request whose subject has stopped waiting, so that
//! nothing unread disappears without a word. The rows stand in groups by what they ask ([`Kind`]),
//! in a fixed order, and within a group in batches by who sent them and the scope their work is in,
//! which is how an agent files a plan: no clock decides a batch, as none decides the batches of
//! D95. Within a batch a ready task comes before one that waits on another, then the more urgent,
//! then the oldest.
//!
//! Three things keep one thing one row, and the morning's resolved threads out of the way (task
//! w4g0qBJY). A comment, a reply or a mention about a subject that has a row joins the row, beneath
//! it, rather than taking a card of its own: a comment on a request usually bears on its move. A
//! proposed ruling and the task assigned to the reader to make it share the ruling's row when the
//! task cites the ruling, one to one, so that a plan of nine rulings is nine rows, not eighteen.
//! And an unread notification whose threads were all resolved before the reader read it, or a
//! request whose subject stopped waiting before then, folds into one line per task, else per
//! sender, Settled before you looked ([`Fold`]): that morning 169 of Daniel's 172 comments on code
//! pointed into threads resolved before he looked. They stay unread until the reader marks them:
//! nothing is read automatically, since read means put in front of someone
//! (docs/design/agents-and-inbox.md, "Notifications and their status").
//!
//! The sorting is plain data, tested natively; [`StoreHolds`] reads each subject's facts from the
//! page's standing queries, and the Inbox page and the header's badge both sort with it, so the
//! badge counts the rows the page shows.

use crate::cards::id_at;
use crate::session::ctx;
use crate::store;
use ankurah::signals::Get as AnkurahGet;
use ankurah::EntityId;
use fwd_core::model::*;
use fwd_core::ops;
use fwd_core::query::with_id;
use fwd_core::routing;
use fwd_core::rules::{NotReadyReason, Readiness};
use leptos::prelude::*;
use serde_json::Value;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use wasm_bindgen_futures::spawn_local;

/// How many rows a group shows before "N more", which shows the rest.
pub const CAP: usize = 8;

/// What a request asks of its reader, which names the group of Waiting on you its row stands in.
/// The groups stand in this order, so that the keyboard learns it, the order of how long each
/// kind leaves its sender waiting: a question stops its asker's work, a ruling or a review holds
/// up a plan, a proposed task waits for consent, and an assigned task is the reader's own work.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    /// A question asked of the reader, or a message forwarded to them.
    Question,
    /// A ruling proposed, for the reader to accept or reject.
    Ruling,
    /// Work submitted for the reader's review.
    Review,
    /// A task proposed, for the reader to approve or reject.
    Proposal,
    /// A task assigned to the reader, or one of theirs that became ready.
    Assignment,
}

impl Kind {
    /// Every kind, in the order the groups stand.
    pub const ALL: [Kind; 5] = [Kind::Question, Kind::Ruling, Kind::Review, Kind::Proposal, Kind::Assignment];

    /// The request a notification of type `t` makes, `target` being the type of what it points at
    /// ("Task", "Decision", "Message", "Relation"); none for a conversation. A proposed relation
    /// asks for a ruling on the relation, which no row offers, so it stays a conversation.
    pub fn of(t: NotificationType, target: Option<&str>) -> Option<Kind> {
        use NotificationType as T;
        match (t, target) {
            (T::QuestionAsked, _) => Some(Kind::Question),
            (T::DecisionProposed, Some("Decision")) => Some(Kind::Ruling),
            (T::ReviewRequested, Some("Task")) => Some(Kind::Review),
            (T::TaskProposed, Some("Task")) => Some(Kind::Proposal),
            (T::TaskAssigned | T::TaskUnblocked, Some("Task")) => Some(Kind::Assignment),
            _ => None,
        }
    }

    /// The group's heading on the inbox of `reader`, whom it names as "you" on one's own inbox and
    /// by their handle on another principal's.
    pub fn heading(self, reader: &str) -> String {
        match self {
            Kind::Question => format!("Questions for {reader}"),
            Kind::Ruling => "Proposed rulings".to_string(),
            Kind::Review => "Reviews requested".to_string(),
            Kind::Proposal => "Proposed tasks".to_string(),
            Kind::Assignment => format!("Assigned to {reader}"),
        }
    }

    /// The name the group goes by in this browser's memory of which sections are closed
    /// (`accordion.rs`), and in the group's element id.
    pub fn key(self) -> &'static str {
        match self {
            Kind::Question => "questions",
            Kind::Ruling => "rulings",
            Kind::Review => "reviews",
            Kind::Proposal => "proposals",
            Kind::Assignment => "assigned",
        }
    }

    /// The kind [`Kind::key`] names.
    pub fn keyed(key: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.key() == key)
    }
}

/// Where a message sits, as the page holds it: the message it replies to, and the first message
/// of its thread (none for a message that opens one).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Placed {
    pub reply_to: Option<EntityId>,
    pub thread: Option<EntityId>,
}

/// One notification, as the sorting reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub id: EntityId,
    /// Its type, which says in a line of Settled before you looked what it is.
    pub t: Option<NotificationType>,
    /// The request it makes and the entity the request is about (the question, the decision, the
    /// task); none for a conversation.
    pub request: Option<(Kind, EntityId)>,
    pub unread: bool,
    /// When it last changed (`ops::notification_time`).
    pub at: i64,
    /// Who caused it: the author of the message it is about, whoever proposed, submitted,
    /// assigned, or finished what the task waited on.
    pub actor: Option<EntityId>,
    pub priority: Option<Priority>,
    /// What it is about, the most particular first, which a row about the same thing takes it
    /// beneath: the message its message replies to and the first message of that thread (a
    /// question's row takes the replies in its thread), the task it is on or near, the decision.
    pub about: Vec<EntityId>,
    /// The first messages of the threads it points into: its message's, and those of the messages
    /// its batch holds. Empty for one that points into none.
    pub threads: Vec<EntityId>,
    /// Whether it holds a step added, which no thread settles.
    pub steps: bool,
    /// The task it is about, which names its line in Settled before you looked: for a request about
    /// a task, that task.
    pub task: Option<EntityId>,
}

impl Note {
    /// The notification `id` of type `t`, pointing at `target` and caused as `originator` says, as
    /// [`sort`] reads it; `placed` says where a message it names sits, while the page holds it.
    #[allow(clippy::too_many_arguments)]
    pub fn read(
        id: EntityId,
        t: Option<NotificationType>,
        unread: bool,
        target: &Value,
        originator: &Value,
        at: i64,
        priority: Option<Priority>,
        placed: impl Fn(EntityId) -> Option<Placed>,
    ) -> Note {
        let kind = t.and_then(|t| Kind::of(t, target["type"].as_str()));
        let request = kind.and_then(|kind| Some((kind, subject_of(kind, target, originator)?)));
        let message = id_at(&originator["message_id"]);
        let batch = routing::batch_of(originator);
        let pointed = |kind: &str| id_at(&target["id"]).filter(|_| target["type"] == kind);
        let mut messages: Vec<EntityId> = message.into_iter().collect();
        messages.extend(batch.as_ref().map(routing::Batch::messages).unwrap_or_default());
        let threads = distinct(messages.iter().map(|m| placed(*m).and_then(|p| p.thread).unwrap_or(*m)));
        // A task is named by what it is on, then what it is a step of or near, then what it points
        // at; a decision by the comment's decision, then what it points at.
        let tasks = [
            id_at(&originator["task_id"]),
            id_at(&originator["step_task_id"]),
            id_at(&originator["near_task_id"]),
            pointed("Task"),
            batch.as_ref().map(|b| b.task),
        ];
        let decisions = [id_at(&originator["decision_id"]), pointed("Decision")];
        let replied = message.and_then(&placed).map(|p| [p.reply_to, p.thread]).unwrap_or_default();
        let about = distinct(replied.into_iter().chain(tasks).chain(decisions).flatten());
        Note {
            id,
            t,
            request,
            unread,
            at,
            actor: id_at(&originator["actor_id"]),
            priority,
            about,
            threads,
            steps: batch.is_some_and(|b| b.steps() > 0),
            // A request about a task is about that task, whatever its originator names: a task's
            // "now ready" names, as its `task_id`, the task whose close made it ready.
            task: match request {
                Some((Kind::Review | Kind::Proposal | Kind::Assignment, subject)) => Some(subject),
                _ => tasks.into_iter().flatten().next(),
            },
        }
    }

    /// The notification `n`, as the page holds it; none once it is dismissed, which leaves the
    /// inbox (a dismissed notification can linger in a live query's results for a moment). Where
    /// the messages it names sit is looked up by their ids.
    pub fn of(n: &NotificationView) -> Option<Note> {
        let status = n.status().ok();
        if status == Some(NotificationStatus::Dismissed) {
            return None;
        }
        let target: Value = n.target().map(Into::into).unwrap_or(Value::Null);
        let originator: Value = n.originator().map(Into::into).unwrap_or(Value::Null);
        let unread = status == Some(NotificationStatus::Unread);
        let (t, at, priority) = (n.notification_type().ok(), ops::notification_time(n), n.priority().ok());
        let placed = |message: EntityId| {
            let m = store::message(message)?;
            Some(Placed {
                reply_to: m.reply_to().ok().flatten().map(EntityId::from),
                thread: m.thread().ok().flatten().map(EntityId::from),
            })
        };
        Some(Note::read(n.id(), t, unread, &target, &originator, at, priority, placed))
    }
}

/// Each of `ids` once, in the order they first come.
fn distinct(ids: impl IntoIterator<Item = EntityId>) -> Vec<EntityId> {
    let mut seen = Vec::new();
    for id in ids {
        if !seen.contains(&id) {
            seen.push(id);
        }
    }
    seen
}

/// What a request of `kind` is about: a question is the message it points at (its message, on a
/// notification from before it pointed at one), and any other the decision or the task.
fn subject_of(kind: Kind, target: &Value, originator: &Value) -> Option<EntityId> {
    let pointed = id_at(&target["id"]);
    match kind {
        Kind::Question => pointed.filter(|_| target["type"] == "Message").or_else(|| id_at(&originator["message_id"])),
        _ => pointed,
    }
}

/// What a request is about, as the page holds it now: what says whether it still waits on its
/// reader.
#[derive(Clone, Debug, PartialEq)]
pub enum Subject {
    /// A question: whether it is still open, and whether the reader has answered it in its thread
    /// ([`answered_by`]).
    Question { open: bool, answered: bool },
    /// A ruling, in its status.
    Ruling { status: DecisionStatus },
    /// A task: its status, its assignee, and whether nothing it depends on holds it up.
    Task { status: TaskStatus, assignee: Option<EntityId>, ready: bool },
    /// Something the page does not hold: not arrived yet, or no such entity.
    Absent,
}

impl Subject {
    /// Whether a request of `kind` about it still waits on `reader`: a question until it is
    /// resolved or the reader has answered it; a ruling while it is proposed; a review while its
    /// task is in review; a proposed task while it is proposed; an assignment while its task is
    /// open and still the reader's. What the page does not hold waits until the daemon has
    /// answered the standing queries (`loaded`), when there is no such thing to wait for.
    pub fn waits(&self, kind: Kind, reader: EntityId, loaded: bool) -> bool {
        match (kind, self) {
            (_, Subject::Absent) => !loaded,
            (Kind::Question, Subject::Question { open, answered }) => *open && !answered,
            (Kind::Ruling, Subject::Ruling { status }) => *status == DecisionStatus::Proposed,
            (Kind::Review, Subject::Task { status, .. }) => *status == TaskStatus::InReview,
            (Kind::Proposal, Subject::Task { status, .. }) => *status == TaskStatus::Proposed,
            (Kind::Assignment, Subject::Task { status, assignee, .. }) => {
                *status == TaskStatus::Open && *assignee == Some(reader)
            }
            _ => false,
        }
    }

    /// Whether nothing holds it up: a task that waits on another is not; anything else is.
    fn ready(&self) -> bool {
        !matches!(self, Subject::Task { ready: false, .. })
    }
}

/// A subject's state, the scope its work is in, which places its row in a batch, and for a task
/// the proposed rulings it cites, which pairs it with the row of the one it asks the reader to make.
#[derive(Clone, Debug, PartialEq)]
pub struct Facts {
    pub subject: Subject,
    pub scope: Option<EntityId>,
    pub cites: Vec<EntityId>,
}

impl Facts {
    /// Facts about something the page does not hold.
    pub fn absent() -> Facts {
        Facts { subject: Subject::Absent, scope: None, cites: Vec::new() }
    }
}

/// What the sorting reads of what notifications are about: the facts of a request's subject, and
/// whether a thread is resolved.
pub trait Holds {
    /// What the request of `kind` about `subject` is about now.
    fn facts(&self, kind: Kind, subject: EntityId) -> Facts;
    /// Whether the thread whose first message is `first` is resolved.
    fn resolved(&self, first: EntityId) -> bool;
}

/// One row of Waiting on you: the requests about one subject, with the comments about it.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// What it asks: the first kind, in the groups' order, that still waits.
    pub kind: Kind,
    pub subject: EntityId,
    /// Every notification of the row, newest first: its requests, those of the task paired with
    /// its ruling, and the comments about either. A move made from the row marks them read, and
    /// its × dismisses them.
    pub members: Vec<EntityId>,
    /// The comments, replies and mentions among them, newest first, which show beneath the row.
    pub comments: Vec<EntityId>,
    /// How many of those are unread.
    pub comments_unread: usize,
    /// On a ruling's row, the task assigned to the reader that asks them to make it, which shares
    /// the row (it cites the ruling).
    pub task: Option<EntityId>,
    /// Whether any of its notifications is unread, which the row's new mark says.
    pub unread: bool,
    /// Whether its subject still waits on the reader: false only on the row kept, while it stays
    /// selected, after a move made from it ([`sort`]'s `keep`).
    pub waits: bool,
    /// Who sent it: the sender of its oldest request, the one that made the row.
    pub sender: Option<EntityId>,
    /// The scope its subject's work is in.
    pub scope: Option<EntityId>,
    /// Whether nothing holds its subject up ([`Subject::ready`]).
    pub ready: bool,
    /// The most urgent of its requests' priorities.
    pub priority: Option<Priority>,
    /// When its oldest request, and its newest, last changed: its comments move it nowhere.
    pub first: i64,
    pub newest: i64,
}

impl Row {
    /// Its newest request: the notification that says why the row is in the inbox, which a comment
    /// that joined it does not.
    pub fn newest_request(&self) -> Option<EntityId> {
        self.members.iter().copied().find(|m| !self.comments.contains(m))
    }

    /// Where it stands in its batch: a ready task before one that waits, then the more urgent
    /// (one with no priority last), then the oldest, then by subject, so every client lists alike.
    fn order(&self) -> (bool, bool, Option<Priority>, i64, EntityId) {
        (!self.ready, self.priority.is_none(), self.priority, self.first, self.subject)
    }
}

/// Where a row stood while it waited: its group, its batch, and its place in the batch. A row kept
/// after its move keeps them, so that the move neither moves it (a started task is no longer ready
/// by the transition table's measure, and fell to the end of its batch) nor takes it past a
/// group's first eight rows, which unmounted it and moved the selection on (the review of
/// 2026-10-03).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stood {
    pub kind: Kind,
    pub sender: Option<EntityId>,
    pub scope: Option<EntityId>,
    pub ready: bool,
    pub priority: Option<Priority>,
    pub first: i64,
}

impl Row {
    /// Where it stands.
    pub fn stood(&self) -> Stood {
        Stood {
            kind: self.kind,
            sender: self.sender,
            scope: self.scope,
            ready: self.ready,
            priority: self.priority,
            first: self.first,
        }
    }
}

/// The selected row, which [`sort`] keeps after its move: its subject, and where it stood while it
/// waited, when the page saw it wait.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Keep {
    pub subject: EntityId,
    pub stood: Option<Stood>,
}

/// The rows of one group sent by one principal about one scope's work.
#[derive(Clone, Debug, PartialEq)]
pub struct Batch {
    pub sender: Option<EntityId>,
    pub scope: Option<EntityId>,
    pub rows: Vec<Row>,
}

impl Batch {
    /// When its newest request last changed.
    fn newest(&self) -> i64 {
        self.rows.iter().map(|r| r.newest).max().unwrap_or(0)
    }

    /// How many of its rows are new.
    pub fn unread(&self) -> usize {
        self.rows.iter().filter(|r| r.unread).count()
    }
}

/// One group of Waiting on you: the rows that ask one kind of move, in batches, the newest batch
/// first.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub kind: Kind,
    pub batches: Vec<Batch>,
}

impl Group {
    /// Its rows, in the order they stand.
    pub fn rows(&self) -> impl Iterator<Item = &Row> {
        self.batches.iter().flat_map(|b| b.rows.iter())
    }

    pub fn len(&self) -> usize {
        self.batches.iter().map(|b| b.rows.len()).sum()
    }

    /// How many of its rows are new.
    pub fn unread(&self) -> usize {
        self.rows().filter(|r| r.unread).count()
    }

    /// Who sent its rows, each once, in the order their batches stand.
    pub fn senders(&self) -> Vec<EntityId> {
        let mut senders: Vec<EntityId> = Vec::new();
        for sender in self.batches.iter().filter_map(|b| b.sender) {
            if !senders.contains(&sender) {
                senders.push(sender);
            }
        }
        senders
    }

    /// Its batches as the page draws them: every row when `expanded`, else the first [`CAP`] rows
    /// and, past them, a row kept after its move, which never leaves the page under the reader; a
    /// batch none of whose rows is shown left out.
    pub fn shown(&self, expanded: bool) -> Vec<Batch> {
        let mut seen = 0;
        let mut shown = Vec::new();
        for batch in &self.batches {
            let rows: Vec<Row> = batch
                .rows
                .iter()
                .filter(|row| {
                    seen += 1;
                    expanded || seen <= CAP || !row.waits
                })
                .cloned()
                .collect();
            if !rows.is_empty() {
                shown.push(Batch { sender: batch.sender, scope: batch.scope, rows });
            }
        }
        shown
    }

    /// How many rows [`Group::shown`] leaves for "N more".
    pub fn hidden(&self, expanded: bool) -> usize {
        self.len() - self.shown(expanded).iter().map(|b| b.rows.len()).sum::<usize>()
    }
}

/// One line of Settled before you looked: the unread notifications about one task, else from one
/// sender, that settled before the reader looked.
#[derive(Clone, Debug, PartialEq)]
pub struct Fold {
    /// The task they are about.
    pub task: Option<EntityId>,
    /// Who sent them, for a line about no task.
    pub sender: Option<EntityId>,
    /// The notifications, newest first.
    pub members: Vec<EntityId>,
    /// What they are, counted ("169 comments and 18 replies": [`what_they_are`]).
    pub words: String,
    /// When the newest of them last changed.
    pub newest: i64,
}

impl Fold {
    /// What the line goes by: its task, else its sender, else its newest notification.
    pub fn key(&self) -> EntityId {
        self.task.or(self.sender).or(self.members.first().copied()).unwrap_or(EntityId::from_bytes([0; 32]))
    }
}

/// An inbox, sorted: the groups of Waiting on you that have rows, in their order, how many tasks
/// show beside the rulings they ask the reader to make, the lines of Settled before you looked,
/// and the notifications that are conversations.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sorted {
    pub groups: Vec<Group>,
    pub paired: usize,
    pub folds: Vec<Fold>,
    pub conversations: Vec<EntityId>,
}

impl Sorted {
    /// Every row, in the order the page shows them.
    pub fn rows(&self) -> impl Iterator<Item = &Row> {
        self.groups.iter().flat_map(Group::rows)
    }

    /// The row about `subject`, when there is one.
    pub fn row(&self, subject: EntityId) -> Option<&Row> {
        self.rows().find(|r| r.subject == subject)
    }

    /// How many rows wait on the reader: the badge's count, which a row kept after its move does
    /// not join.
    pub fn waiting(&self) -> usize {
        self.rows().filter(|r| r.waits).count()
    }

    /// How many rows are new.
    pub fn unread(&self) -> usize {
        self.rows().filter(|r| r.unread).count()
    }

    /// The group of `kind`, when it has rows.
    pub fn group(&self, kind: Kind) -> Option<&Group> {
        self.groups.iter().find(|g| g.kind == kind)
    }

    /// The line of Settled before you looked that goes by `key`, when there is one.
    pub fn fold(&self, key: EntityId) -> Option<&Fold> {
        self.folds.iter().find(|f| f.key() == key)
    }

    /// How many notifications Settled before you looked holds.
    pub fn settled(&self) -> usize {
        self.folds.iter().map(|f| f.members.len()).sum()
    }
}

/// A row being made: its requests, then what joins it.
struct Making {
    kind: Kind,
    subject: EntityId,
    requests: Vec<Note>,
    joined: Vec<Note>,
    comments: Vec<Note>,
    task: Option<EntityId>,
    waits: bool,
    scope: Option<EntityId>,
    ready: bool,
    cites: Vec<EntityId>,
}

impl Making {
    fn row(self, kept: Option<Stood>) -> Row {
        let newest_first = |notes: &mut Vec<&Note>| notes.sort_by_key(|n| (Reverse(n.at), n.id));
        let mut requests: Vec<&Note> = self.requests.iter().collect();
        newest_first(&mut requests);
        let mut every: Vec<&Note> = self.requests.iter().chain(&self.joined).chain(&self.comments).collect();
        newest_first(&mut every);
        let mut comments: Vec<&Note> = self.comments.iter().collect();
        newest_first(&mut comments);
        let (newest, oldest) = (requests.first(), requests.last());
        let row = Row {
            kind: self.kind,
            subject: self.subject,
            members: every.iter().map(|n| n.id).collect(),
            comments: comments.iter().map(|n| n.id).collect(),
            comments_unread: comments.iter().filter(|n| n.unread).count(),
            task: self.task,
            unread: every.iter().any(|n| n.unread),
            waits: self.waits,
            sender: oldest.and_then(|n| n.actor),
            scope: self.scope,
            ready: self.ready,
            priority: requests.iter().filter_map(|n| n.priority).min(),
            first: oldest.map_or(0, |n| n.at),
            newest: newest.map_or(0, |n| n.at),
        };
        // A row kept after its move stays where it stood.
        match kept.filter(|_| !row.waits) {
            Some(stood) => Row {
                kind: stood.kind,
                sender: stood.sender,
                scope: stood.scope,
                ready: stood.ready,
                priority: stood.priority,
                first: stood.first,
                ..row
            },
            None => row,
        }
    }
}

/// Sorts the notifications `notes` of `reader`'s inbox, `holds` saying what each request's subject
/// is now and which threads are resolved. A subject that waits on the reader for one of the kinds
/// its requests make has a row of the first such kind, which all its requests join; a ruling's
/// row takes the task assigned to the reader that cites it, when each is the other's only one;
/// each comment, reply or mention joins the row of the first thing it is about that has one. Of
/// the rest, the unread ones that settled before the reader looked (every thread they point into
/// resolved, or a request whose subject stopped waiting) fold into a line per task, else per
/// sender, and the others are conversations. `keep` is the selected row: kept as a row while it
/// stays selected after its move, as the selected card is kept with "unread only", where it stood
/// while it waited ([`Keep`]), so that a move does not pull the row from under the reader, who sees
/// what became of it; it leaves when the selection moves on. `loaded` says whether the daemon has
/// answered the page's standing queries ([`Subject::waits`]).
pub fn sort(notes: &[Note], holds: &impl Holds, reader: EntityId, loaded: bool, keep: Option<Keep>) -> Sorted {
    let kept = keep.map(|k| k.subject);
    let mut about: BTreeMap<EntityId, Vec<&Note>> = BTreeMap::new();
    let mut said: Vec<&Note> = Vec::new();
    for note in notes {
        match note.request {
            Some((_, subject)) => about.entry(subject).or_default().push(note),
            None => said.push(note),
        }
    }
    // The rows, and the requests whose subjects stopped waiting.
    let mut making: Vec<Making> = Vec::new();
    let mut stopped: Vec<&Note> = Vec::new();
    for (subject, mut requests) in about {
        requests.sort_by_key(|n| (Reverse(n.at), n.id));
        let kinds: BTreeSet<Kind> = requests.iter().filter_map(|n| n.request.map(|(kind, _)| kind)).collect();
        let (Some(first_kind), Some(newest)) = (kinds.first(), requests.first()) else { continue };
        let facts = holds.facts(*first_kind, subject);
        let waiting = kinds.iter().copied().find(|kind| facts.subject.waits(*kind, reader, loaded));
        let kind = match (waiting, newest.request) {
            (Some(kind), _) => kind,
            (None, Some((kind, _))) if kept == Some(subject) => kind,
            _ => {
                stopped.extend(requests);
                continue;
            }
        };
        making.push(Making {
            kind,
            subject,
            requests: requests.into_iter().cloned().collect(),
            joined: Vec::new(),
            comments: Vec::new(),
            task: None,
            waits: waiting.is_some(),
            scope: facts.scope,
            ready: facts.subject.ready(),
            cites: facts.cites,
        });
    }
    let paired = pair(&mut making);
    // Each comment joins the row of the first thing it is about that has one; a task paired with
    // its ruling leads to the ruling's row.
    let mut row_of: HashMap<EntityId, usize> = HashMap::new();
    for (at, m) in making.iter().enumerate() {
        row_of.insert(m.subject, at);
        row_of.extend(m.task.map(|task| (task, at)));
    }
    let mut rest: Vec<(&Note, bool)> = stopped.into_iter().map(|n| (n, true)).collect();
    for note in said {
        match note.about.iter().find_map(|id| row_of.get(id)) {
            Some(at) => making[*at].comments.push(note.clone()),
            None => rest.push((note, false)),
        }
    }
    let (mut settled, mut conversations) = (Vec::new(), Vec::new());
    for (note, stopped) in rest {
        let resolved = !note.threads.is_empty() && !note.steps && note.threads.iter().all(|t| holds.resolved(*t));
        if note.unread && (stopped || resolved) {
            settled.push(note);
        } else {
            conversations.push(note.id);
        }
    }
    let stood = keep.and_then(|k| k.stood);
    let rows: Vec<Row> =
        making.into_iter().map(|m| if kept == Some(m.subject) { m.row(stood) } else { m.row(None) }).collect();
    let groups = Kind::ALL
        .into_iter()
        .filter_map(|kind| {
            let mut batches: BTreeMap<(Option<EntityId>, Option<EntityId>), Vec<Row>> = BTreeMap::new();
            for row in rows.iter().filter(|r| r.kind == kind) {
                batches.entry((row.sender, row.scope)).or_default().push(row.clone());
            }
            let mut batches: Vec<Batch> = batches
                .into_iter()
                .map(|((sender, scope), mut rows)| {
                    rows.sort_by_key(Row::order);
                    Batch { sender, scope, rows }
                })
                .collect();
            batches.sort_by_key(|b| (Reverse(b.newest()), b.sender, b.scope));
            (!batches.is_empty()).then_some(Group { kind, batches })
        })
        .collect();
    Sorted { groups, paired, folds: fold(settled), conversations }
}

/// Pairs each ruling's row with the row of the task assigned to the reader that asks them to make
/// it, the task citing the ruling, where each is the other's only one: a task that cites another
/// proposed ruling too, whether or not that one has a row here, or a ruling two waiting tasks cite,
/// stays apart, so that marking the task done with the ruling never finishes a task that still
/// asks for one (the review of 2026-10-03). The task's row joins the ruling's, its requests with
/// it. Answers how many tasks it paired.
fn pair(making: &mut Vec<Making>) -> usize {
    let waiting = |m: &Making, kind: Kind| m.kind == kind && m.waits;
    let rulings: Vec<EntityId> = making.iter().filter(|m| waiting(m, Kind::Ruling)).map(|m| m.subject).collect();
    let mut tasks_of: HashMap<EntityId, Vec<EntityId>> = HashMap::new();
    for m in making.iter().filter(|m| waiting(m, Kind::Assignment)) {
        for ruling in &m.cites {
            tasks_of.entry(*ruling).or_default().push(m.subject);
        }
    }
    let pairs: Vec<(EntityId, EntityId)> = making
        .iter()
        .filter(|m| waiting(m, Kind::Assignment))
        .filter_map(|m| match m.cites.as_slice() {
            [ruling] if rulings.contains(ruling) && tasks_of.get(ruling).is_some_and(|tasks| tasks.len() == 1) => {
                Some((*ruling, m.subject))
            }
            _ => None,
        })
        .collect();
    for (ruling, task) in &pairs {
        let Some(at) = making.iter().position(|m| m.subject == *task) else { continue };
        let joined = making.remove(at);
        if let Some(row) = making.iter_mut().find(|m| m.subject == *ruling) {
            row.task = Some(*task);
            row.joined.extend(joined.requests);
        }
    }
    pairs.len()
}

/// The lines of Settled before you looked: `settled` by the task each is about, else by who sent
/// it, each line's notifications newest first, the newest line first.
fn fold(settled: Vec<&Note>) -> Vec<Fold> {
    let mut lines: BTreeMap<(Option<EntityId>, Option<EntityId>), Vec<&Note>> = BTreeMap::new();
    for note in settled {
        let key = match note.task {
            Some(task) => (Some(task), None),
            None => (None, note.actor),
        };
        lines.entry(key).or_default().push(note);
    }
    let mut folds: Vec<Fold> = lines
        .into_iter()
        .map(|((task, sender), mut notes)| {
            notes.sort_by_key(|n| (Reverse(n.at), n.id));
            let types: Vec<Option<NotificationType>> = notes.iter().map(|n| n.t).collect();
            Fold {
                task,
                sender,
                members: notes.iter().map(|n| n.id).collect(),
                words: what_they_are(&types),
                newest: notes.first().map_or(0, |n| n.at),
            }
        })
        .collect();
    folds.sort_by_key(|f| (Reverse(f.newest), f.key()));
    folds
}

/// The noun a notification of type `t` is counted as, one and many.
fn noun(t: Option<NotificationType>) -> (&'static str, &'static str) {
    use NotificationType as T;
    match t {
        Some(T::TaskCommented | T::CodeCommented | T::DecisionCommented) => ("comment", "comments"),
        Some(T::Replied) => ("reply", "replies"),
        Some(T::Mentioned) => ("mention", "mentions"),
        Some(T::QuestionAsked) => ("question", "questions"),
        Some(T::DecisionProposed) => ("proposed ruling", "proposed rulings"),
        Some(T::ReviewRequested) => ("review", "reviews"),
        Some(T::TaskProposed) => ("proposed task", "proposed tasks"),
        Some(T::TaskAssigned | T::TaskUnblocked) => ("assignment", "assignments"),
        Some(T::TaskCreated) => ("task filed", "tasks filed"),
        Some(T::Digest) => ("digest", "digests"),
        Some(T::StepAdded) => ("step added", "steps added"),
        Some(T::DecisionRepealed | T::DecisionSuperseded) => ("fallen ruling", "fallen rulings"),
        None => ("notification", "notifications"),
    }
}

/// What notifications of `types` are, counted, the most first: "169 comments and 18 replies".
pub fn what_they_are(types: &[Option<NotificationType>]) -> String {
    let mut counts: Vec<((&str, &str), usize)> = Vec::new();
    for t in types {
        let noun = noun(*t);
        match counts.iter_mut().find(|(n, _)| *n == noun) {
            Some((_, count)) => *count += 1,
            None => counts.push((noun, 1)),
        }
    }
    counts.sort_by_key(|(noun, count)| (Reverse(*count), noun.0));
    let said: Vec<String> = counts
        .into_iter()
        .map(|((one, many), count)| format!("{count} {}", if count == 1 { one } else { many }))
        .collect();
    match said.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
    }
}

/// The messages `reader` has answered in their threads, from each message's author, `reply_to`
/// and `thread`: every message a message of theirs replies to, and the first message of every
/// thread they wrote in. A question asked of them leaves Waiting on you once it is among these
/// (or resolved): a reply sent from its row, from a card, or by `fwd reply` alike. A question
/// opens its own thread, so asking one answers nothing.
pub fn answered_by(
    reader: EntityId,
    messages: impl IntoIterator<Item = (EntityId, Option<EntityId>, Option<EntityId>)>,
) -> HashSet<EntityId> {
    let mut answered = HashSet::new();
    for (author, reply_to, thread) in messages {
        if author == reader {
            answered.extend(reply_to);
            answered.extend(thread);
        }
    }
    answered
}

/// The messages `reader` has answered ([`answered_by`]), from a live query of the messages they
/// wrote, which is all [`answered_by`] reads: a page that worked it out from its standing query of
/// every message copied every message of the project at each change to any (the review of
/// 2026-10-03). With it, whether the daemon has answered that query, until when a question the
/// reader answered can still count as waiting. None when the query is refused, when nothing counts
/// as answered.
pub fn answers(reader: EntityId) -> (Memo<HashSet<EntityId>>, RwSignal<bool>) {
    let answered = RwSignal::new(false);
    let query = with_id("author = ?", reader).ok().and_then(|selection| ctx().query::<MessageView>(selection).ok());
    if let Some(query) = query.clone() {
        spawn_local(async move {
            if let Err(e) = query.wait_durable_answered().await {
                tracing::warn!("the query of a reader's own messages did not load: {e}");
            }
            answered.try_set(true);
        });
    } else {
        answered.set(true);
    }
    let set = Memo::new(move |_| {
        let Some(query) = &query else { return HashSet::new() };
        answered_by(
            reader,
            query.get().iter().filter_map(|m| {
                let author = EntityId::from(m.author().ok()?);
                let reply_to = m.reply_to().ok().flatten().map(EntityId::from);
                let thread = m.thread().ok().flatten().map(EntityId::from);
                Some((author, reply_to, thread))
            }),
        )
    });
    (set, answered)
}

/// The notification types that make requests ([`Kind::of`]): a proposed task's covers a proposed
/// relation too, which stays a conversation.
pub const REQUEST_TYPES: [NotificationType; 6] = [
    NotificationType::QuestionAsked,
    NotificationType::DecisionProposed,
    NotificationType::ReviewRequested,
    NotificationType::TaskProposed,
    NotificationType::TaskAssigned,
    NotificationType::TaskUnblocked,
];

/// The daemon's selection of a principal's notifications that can make a row of Waiting on you:
/// those not dismissed, read or not, of the request types, its one `?` bound to the principal. The
/// header's badge holds these alone, not every notification the inbox holds (the review of
/// 2026-10-03).
pub fn requests_selection() -> String {
    let types: Vec<String> = REQUEST_TYPES.iter().map(|t| format!("notification_type = '{t}'")).collect();
    format!("recipient = ? AND status != 'dismissed' AND ({})", types.join(" OR "))
}

/// What the page's standing queries hold of what notifications are about, each entity looked up
/// by its id, so that the reader runs again when that entity arrives, leaves or changes in what it
/// read, and not when another does. `answered` holds the messages the reader has answered
/// ([`answered_now`]).
pub struct StoreHolds<'a> {
    pub answered: &'a HashSet<EntityId>,
}

impl Holds for StoreHolds<'_> {
    fn facts(&self, kind: Kind, subject: EntityId) -> Facts {
        match kind {
            Kind::Question => match store::message(subject) {
                Some(m) => Facts {
                    subject: Subject::Question {
                        open: m.status().ok() == Some(MessageStatus::Active),
                        answered: self.answered.contains(&subject),
                    },
                    scope: question_scope(&m),
                    cites: Vec::new(),
                },
                None => Facts::absent(),
            },
            Kind::Ruling => match store::decision(subject).and_then(|d| Some((d.status().ok()?, d))) {
                Some((status, d)) => Facts {
                    subject: Subject::Ruling { status },
                    scope: ops::governs(&d).ok().flatten(),
                    cites: Vec::new(),
                },
                None => Facts::absent(),
            },
            Kind::Review | Kind::Proposal | Kind::Assignment => {
                match store::task(subject).and_then(|t| Some((t.status().ok()?, t))) {
                    Some((status, t)) => {
                        let relations = store::relations_of(subject);
                        Facts {
                            subject: Subject::Task {
                                status,
                                assignee: ops::assignee_of(&t).ok().flatten(),
                                ready: nothing_holds_up(&store::readiness(&t)),
                            },
                            // A task worked in several scopes (D136) batches under the first.
                            scope: store::task_scopes(subject, &relations).first().copied(),
                            // The rulings it cites that are still proposed.
                            cites: relations
                                .iter()
                                .filter(|r| {
                                    r.from_id().ok() == Some(subject)
                                        && r.kind().ok() == Some(RelationKind::Cites)
                                        && ops::standing(r)
                                })
                                .filter_map(|r| r.to_id().ok())
                                .filter(|d| {
                                    store::decision(*d).and_then(|d| d.status().ok()) == Some(DecisionStatus::Proposed)
                                })
                                .collect(),
                        }
                    }
                    None => Facts::absent(),
                }
            }
        }
    }

    fn resolved(&self, first: EntityId) -> bool {
        store::message(first).is_some_and(|m| m.status().ok() == Some(MessageStatus::Resolved))
    }
}

/// Whether nothing a task depends on, and no hold, keeps it from being started: its readiness,
/// leaving out its own status, so that a move on the task (a start, a mark done) does not make it
/// "waiting" and move its row. What cannot be known holds it up.
pub fn nothing_holds_up(readiness: &Readiness) -> bool {
    match readiness {
        Readiness::Ready => true,
        Readiness::NotReady(reasons) => reasons.iter().all(|r| matches!(r, NotReadyReason::Status(_))),
        Readiness::Unknown(_) => false,
    }
}

/// The scope the task `task` is worked in, the first of them for a task worked in several (D136),
/// from the relations at it.
pub fn task_scope(task: EntityId) -> Option<EntityId> {
    store::scopes_of_task(task).first().copied()
}

/// The scope a question's work is in: the scope it asks, for one asked of no one in particular;
/// else the scope it names, the scope its task is worked in, or the scope its decision governs,
/// as the Questions page places it (`questions.rs`).
pub fn question_scope(m: &MessageView) -> Option<EntityId> {
    let context: Value = m.context().map(Into::into).unwrap_or(Value::Null);
    if context[ops::ASKS_KEY].as_str() == Some(ops::ASKS_PROJECT) {
        if let Some(asked) = id_at(&context[ops::ASKS_SCOPE_KEY]) {
            return Some(asked);
        }
    }
    m.scope()
        .ok()
        .flatten()
        .map(EntityId::from)
        .or_else(|| m.task().ok().flatten().and_then(|t| task_scope(EntityId::from(t))))
        .or_else(|| {
            let decision = store::decision(EntityId::from(m.decision().ok().flatten()?))?;
            ops::governs(&decision).ok().flatten()
        })
}

/// Sorts the notifications `notifications` of `reader`'s inbox as the page holds what they are
/// about now ([`sort`] with [`StoreHolds`]), `answered` holding the messages the reader has
/// answered.
pub fn sorted_now(
    notifications: &[NotificationView],
    reader: EntityId,
    answered: &HashSet<EntityId>,
    keep: Option<Keep>,
) -> Sorted {
    let notes: Vec<Note> = notifications.iter().filter_map(Note::of).collect();
    sort(&notes, &StoreHolds { answered }, reader, store::standing_loaded(), keep)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn id(n: u8) -> EntityId {
        EntityId::from_bytes([n; 32])
    }

    /// The reader every test sorts for.
    const READER: u8 = 1;

    /// An unread notification about nothing, made at `at` by a principal no test names.
    fn note(n: u8, at: i64) -> Note {
        Note {
            id: id(n),
            t: None,
            request: None,
            unread: true,
            at,
            actor: Some(id(90)),
            priority: Some(Priority::Next),
            about: Vec::new(),
            threads: Vec::new(),
            steps: false,
            task: None,
        }
    }

    /// The type of notification that makes a request of `kind`.
    fn type_of(kind: Kind) -> NotificationType {
        match kind {
            Kind::Question => NotificationType::QuestionAsked,
            Kind::Ruling => NotificationType::DecisionProposed,
            Kind::Review => NotificationType::ReviewRequested,
            Kind::Proposal => NotificationType::TaskProposed,
            Kind::Assignment => NotificationType::TaskAssigned,
        }
    }

    /// A request of `kind` about subject `subject`, made at `at`, sent by `actor`.
    fn request(n: u8, kind: Kind, subject: u8, at: i64, actor: u8, unread: bool) -> Note {
        Note {
            t: Some(type_of(kind)),
            request: Some((kind, id(subject))),
            unread,
            actor: Some(id(actor)),
            ..note(n, at)
        }
    }

    fn conversation(n: u8, at: i64) -> Note {
        note(n, at)
    }

    /// A comment about what `about` names, the most particular first, in the thread `thread`.
    fn comment(n: u8, at: i64, about: &[u8], thread: u8, unread: bool) -> Note {
        Note {
            t: Some(NotificationType::TaskCommented),
            about: about.iter().map(|a| id(*a)).collect(),
            threads: vec![id(thread)],
            unread,
            ..note(n, at)
        }
    }

    /// What the page holds: each subject, the scope of each, the rulings each task cites, and the
    /// threads resolved.
    #[derive(Default)]
    struct Held {
        subjects: HashMap<EntityId, Subject>,
        scopes: HashMap<EntityId, EntityId>,
        cites: HashMap<EntityId, Vec<EntityId>>,
        resolved: HashSet<EntityId>,
    }

    impl Holds for Held {
        fn facts(&self, _: Kind, subject: EntityId) -> Facts {
            Facts {
                subject: self.subjects.get(&subject).cloned().unwrap_or(Subject::Absent),
                scope: self.scopes.get(&subject).copied(),
                cites: self.cites.get(&subject).cloned().unwrap_or_default(),
            }
        }

        fn resolved(&self, first: EntityId) -> bool {
            self.resolved.contains(&first)
        }
    }

    fn held(subjects: &[(u8, Subject)]) -> Held {
        Held { subjects: subjects.iter().map(|(n, s)| (id(*n), s.clone())).collect(), ..Held::default() }
    }

    /// What each subject is now, with no scope unless `scopes` names one.
    fn sorted(notes: &[Note], subjects: &[(u8, Subject)], scopes: &[(u8, u8)], keep: Option<u8>) -> Sorted {
        let mut holds = held(subjects);
        holds.scopes = scopes.iter().map(|(s, scope)| (id(*s), id(*scope))).collect();
        sort(notes, &holds, id(READER), true, keep.map(|k| Keep { subject: id(k), stood: None }))
    }

    fn open_task(assignee: Option<u8>, ready: bool) -> Subject {
        Subject::Task { status: TaskStatus::Open, assignee: assignee.map(id), ready }
    }

    fn task(status: TaskStatus) -> Subject {
        Subject::Task { status, assignee: Some(id(READER)), ready: true }
    }

    fn proposed() -> Subject {
        Subject::Ruling { status: DecisionStatus::Proposed }
    }

    /// The rows of each group, by subject, in the order they stand.
    fn layout(s: &Sorted) -> Vec<(Kind, Vec<EntityId>)> {
        s.groups.iter().map(|g| (g.kind, g.rows().map(|r| r.subject).collect())).collect()
    }

    fn ids(ns: &[u8]) -> Vec<EntityId> {
        ns.iter().map(|n| id(*n)).collect()
    }

    #[test]
    fn each_request_type_asks_its_kind_and_everything_else_is_a_conversation() {
        use NotificationType as T;
        assert_eq!(Kind::of(T::QuestionAsked, Some("Message")), Some(Kind::Question));
        assert_eq!(Kind::of(T::DecisionProposed, Some("Decision")), Some(Kind::Ruling));
        assert_eq!(Kind::of(T::ReviewRequested, Some("Task")), Some(Kind::Review));
        assert_eq!(Kind::of(T::TaskProposed, Some("Task")), Some(Kind::Proposal));
        assert_eq!(Kind::of(T::TaskAssigned, Some("Task")), Some(Kind::Assignment));
        assert_eq!(Kind::of(T::TaskUnblocked, Some("Task")), Some(Kind::Assignment), "a task of yours now ready");
        assert_eq!(Kind::of(T::TaskProposed, Some("Relation")), None, "a proposed relation stays a card");
        for t in [
            T::TaskCommented,
            T::CodeCommented,
            T::Mentioned,
            T::Replied,
            T::TaskCreated,
            T::Digest,
            T::DecisionCommented,
            T::DecisionRepealed,
            T::DecisionSuperseded,
            T::StepAdded,
        ] {
            assert_eq!(Kind::of(t, Some("Task")), None, "{t:?}");
        }
    }

    #[test]
    fn a_notification_names_what_its_request_is_about_and_who_sent_it() {
        let (question, decision, task, author) = (id(10), id(11), id(12), id(13));
        let read = |t, target: Value, originator: Value| {
            Note::read(id(2), Some(t), true, &target, &originator, 5, Some(Priority::Now), |_| None)
        };
        let asked = read(
            NotificationType::QuestionAsked,
            json!({"type": "Message", "id": question.to_base64()}),
            json!({"message_id": question.to_base64(), "actor_id": author.to_base64()}),
        );
        assert_eq!((asked.request, asked.actor), (Some((Kind::Question, question)), Some(author)));
        let proposed = read(
            NotificationType::DecisionProposed,
            json!({"type": "Decision", "id": decision.to_base64()}),
            json!({"actor_id": author.to_base64()}),
        );
        assert_eq!(proposed.request, Some((Kind::Ruling, decision)));
        let assigned = read(NotificationType::TaskAssigned, json!({"type": "Task", "id": task.to_base64()}), json!({}));
        assert_eq!((assigned.request, assigned.actor), (Some((Kind::Assignment, task)), None));
        let replied = read(
            NotificationType::Replied,
            json!({"type": "Message", "id": question.to_base64()}),
            json!({"message_id": question.to_base64()}),
        );
        assert_eq!(replied.request, None, "a conversation");
    }

    #[test]
    fn a_conversation_says_what_it_is_about_the_most_particular_first_and_which_threads_it_is_in() {
        let (reply, answered, first, task, decision) = (id(20), id(21), id(22), id(23), id(24));
        let placed = |m: EntityId| (m == reply).then_some(Placed { reply_to: Some(answered), thread: Some(first) });
        // A reply on a thread about a task: the message it answers and its thread, then the task.
        let n = Note::read(
            id(2),
            Some(NotificationType::Replied),
            true,
            &json!({"type": "Message", "id": reply.to_base64()}),
            &json!({"message_id": reply.to_base64(), "task_id": task.to_base64()}),
            5,
            None,
            placed,
        );
        assert_eq!(n.about, vec![answered, first, task]);
        assert_eq!((n.threads, n.steps, n.task), (vec![first], false, Some(task)));
        // A digest of a comment and a step on a task: the comment's thread, and a step.
        let (comment, step, relation) = (id(30), id(31), id(32));
        let digest = Note::read(
            id(3),
            Some(NotificationType::Digest),
            true,
            &json!({"type": "Task", "id": task.to_base64()}),
            &json!({"message_id": comment.to_base64(), "batch": {"task_id": task.to_base64(), "items": [
                {"message_id": comment.to_base64(), "at": 1},
                {"relation_id": relation.to_base64(), "step_id": step.to_base64(), "at": 2},
            ]}}),
            5,
            None,
            |_| None,
        );
        assert_eq!((digest.threads, digest.steps, digest.task), (vec![comment], true, Some(task)));
        assert_eq!(digest.about, vec![task]);
        // A comment on a decision.
        let on_decision = Note::read(
            id(4),
            Some(NotificationType::DecisionCommented),
            true,
            &json!({"type": "Decision", "id": decision.to_base64()}),
            &json!({"message_id": comment.to_base64(), "decision_id": decision.to_base64()}),
            5,
            None,
            |_| None,
        );
        assert_eq!((on_decision.about, on_decision.task), (vec![decision], None));
    }

    #[test]
    fn a_request_about_a_task_is_about_that_task_whatever_its_originator_names() {
        // "Read the plan" (20) became ready when "Write the plan" (21) was done: the notification
        // points at the task that became ready, and its originator names the one that closed.
        let (ready, closed) = (id(20), id(21));
        let n = Note::read(
            id(2),
            Some(NotificationType::TaskUnblocked),
            true,
            &json!({"type": "Task", "id": ready.to_base64()}),
            &json!({"task_id": closed.to_base64(), "from": "open", "to": "done"}),
            5,
            None,
            |_| None,
        );
        assert_eq!((n.request, n.task), (Some((Kind::Assignment, ready)), Some(ready)));
        // Started from the command line before the reader looked, it folds under the task itself.
        let s = sort(&[n], &held(&[(20, task(TaskStatus::InProgress))]), id(READER), true, None);
        assert_eq!(s.folds.iter().map(|f| f.task).collect::<Vec<_>>(), vec![Some(ready)]);
    }

    #[test]
    fn a_request_waits_until_its_subject_changes_and_reading_it_changes_nothing() {
        let reader = id(READER);
        let waits = |kind, subject: Subject| subject.waits(kind, reader, true);
        assert!(waits(Kind::Question, Subject::Question { open: true, answered: false }));
        assert!(!waits(Kind::Question, Subject::Question { open: false, answered: false }), "resolved");
        assert!(!waits(Kind::Question, Subject::Question { open: true, answered: true }), "answered in its thread");
        assert!(waits(Kind::Ruling, proposed()));
        for status in [DecisionStatus::Accepted, DecisionStatus::Rejected] {
            assert!(!waits(Kind::Ruling, Subject::Ruling { status }), "{status}");
        }
        assert!(waits(Kind::Review, task(TaskStatus::InReview)));
        assert!(!waits(Kind::Review, task(TaskStatus::Done)), "accepted");
        assert!(!waits(Kind::Review, task(TaskStatus::InProgress)), "returned");
        assert!(waits(Kind::Proposal, task(TaskStatus::Proposed)));
        assert!(!waits(Kind::Proposal, task(TaskStatus::Open)), "approved");
        assert!(waits(Kind::Assignment, open_task(Some(READER), true)));
        assert!(waits(Kind::Assignment, open_task(Some(READER), false)), "waiting on another task still waits on you");
        for status in [TaskStatus::InProgress, TaskStatus::Done, TaskStatus::Cancelled] {
            assert!(!waits(Kind::Assignment, task(status)), "started, finished or cancelled: {status}");
        }
        assert!(!waits(Kind::Assignment, open_task(Some(9), true)), "given to someone else");
        assert!(!waits(Kind::Assignment, open_task(None, true)), "given to no one");
        // What the page does not hold waits until the daemon has answered, then there is nothing.
        assert!(Subject::Absent.waits(Kind::Ruling, reader, false));
        assert!(!Subject::Absent.waits(Kind::Ruling, reader, true));
        // A subject of another model than the kind asks about never waits.
        assert!(!waits(Kind::Ruling, task(TaskStatus::Proposed)));
    }

    #[test]
    fn the_requests_about_one_subject_make_one_row_which_stays_read_or_not() {
        // A task assigned to the reader, then ready: one row of both, the newest first; read.
        let notes = [request(2, Kind::Assignment, 20, 10, 5, false), request(3, Kind::Assignment, 20, 30, 6, false)];
        let s = sorted(&notes, &[(20, open_task(Some(READER), true))], &[], None);
        assert_eq!(layout(&s), vec![(Kind::Assignment, vec![id(20)])]);
        let row = s.row(id(20)).expect("one row");
        assert_eq!(row.members, vec![id(3), id(2)]);
        assert!(!row.unread, "read, it stays, without its new mark");
        assert_eq!(row.sender, Some(id(5)), "who sent the request that made the row");
        assert!(s.conversations.is_empty() && s.folds.is_empty());
        assert_eq!((s.waiting(), s.unread()), (1, 0));
    }

    #[test]
    fn a_request_whose_subject_stopped_waiting_is_not_a_row_and_settled_before_you_looked_when_unread() {
        let notes = [
            request(2, Kind::Ruling, 20, 10, 5, true),
            request(3, Kind::Ruling, 21, 11, 5, true),
            request(4, Kind::Ruling, 22, 12, 5, false),
            conversation(6, 13),
        ];
        let subjects = [
            (20, proposed()),
            (21, Subject::Ruling { status: DecisionStatus::Accepted }),
            (22, Subject::Ruling { status: DecisionStatus::Rejected }),
        ];
        let s = sorted(&notes, &subjects, &[], None);
        assert_eq!(layout(&s), vec![(Kind::Ruling, vec![id(20)])]);
        assert_eq!(s.folds.len(), 1, "accepted by someone else before the reader looked");
        assert_eq!((s.folds[0].sender, s.folds[0].members.clone()), (Some(id(5)), vec![id(3)]));
        let mut conversations = s.conversations.clone();
        conversations.sort();
        assert_eq!(conversations, ids(&[4, 6]), "read, it is a card under unread only's rule; a reply, a card");
    }

    #[test]
    fn a_subject_takes_the_first_kind_that_waits_and_every_request_about_it_joins() {
        // The reader was assigned the task, which is now in review, submitted for their review.
        let notes = [request(2, Kind::Assignment, 20, 10, 5, false), request(3, Kind::Review, 20, 20, 6, true)];
        let s = sorted(&notes, &[(20, task(TaskStatus::InReview))], &[], None);
        assert_eq!(layout(&s), vec![(Kind::Review, vec![id(20)])]);
        assert_eq!(s.row(id(20)).map(|r| r.members.clone()), Some(ids(&[3, 2])));
    }

    #[test]
    fn groups_stand_in_a_fixed_order_whatever_arrived_first() {
        let notes = [
            request(2, Kind::Assignment, 20, 50, 5, true),
            request(3, Kind::Proposal, 21, 40, 5, true),
            request(4, Kind::Review, 22, 30, 5, true),
            request(5, Kind::Ruling, 23, 20, 5, true),
            request(6, Kind::Question, 24, 10, 5, true),
        ];
        let subjects = [
            (20, open_task(Some(READER), true)),
            (21, task(TaskStatus::Proposed)),
            (22, task(TaskStatus::InReview)),
            (23, proposed()),
            (24, Subject::Question { open: true, answered: false }),
        ];
        let kinds: Vec<Kind> = sorted(&notes, &subjects, &[], None).groups.iter().map(|g| g.kind).collect();
        assert_eq!(kinds, Kind::ALL.to_vec());
    }

    #[test]
    fn rows_batch_by_sender_and_scope_and_no_clock_decides_a_batch() {
        // lead-fwd (5) proposes three rulings in the scope archive (40), one of them a day later,
        // and one in the scope identifiers (41); impl-x (6) one in the scope archive.
        let day = 86_400_000;
        let notes = [
            request(2, Kind::Ruling, 20, 100, 5, true),
            request(3, Kind::Ruling, 21, 101, 5, true),
            request(4, Kind::Ruling, 22, 100 + day, 5, true),
            request(7, Kind::Ruling, 23, 50, 5, false),
            request(8, Kind::Ruling, 24, 60, 6, true),
        ];
        let subjects: Vec<(u8, Subject)> = (20..=24).map(|n| (n, proposed())).collect();
        let scopes = [(20, 40), (21, 40), (22, 40), (23, 41), (24, 40)];
        let s = sorted(&notes, &subjects, &scopes, None);
        let group = s.group(Kind::Ruling).expect("the rulings");
        let batches: Vec<(Option<EntityId>, Option<EntityId>, Vec<EntityId>)> =
            group.batches.iter().map(|b| (b.sender, b.scope, b.rows.iter().map(|r| r.subject).collect())).collect();
        assert_eq!(
            batches,
            vec![
                (Some(id(5)), Some(id(40)), ids(&[20, 21, 22])),
                (Some(id(6)), Some(id(40)), ids(&[24])),
                (Some(id(5)), Some(id(41)), ids(&[23])),
            ],
            "the newest batch first; a ruling a day later is in the same batch, oldest first within it"
        );
        assert_eq!(group.senders(), ids(&[5, 6]));
        assert_eq!((group.len(), group.unread()), (5, 4));
        assert_eq!(group.batches[0].unread(), 3);
    }

    #[test]
    fn within_a_batch_ready_tasks_come_first_then_the_more_urgent_then_the_oldest() {
        let mut urgent = request(4, Kind::Assignment, 22, 30, 5, true);
        urgent.priority = Some(Priority::Now);
        let notes = [
            request(2, Kind::Assignment, 20, 10, 5, true),
            request(3, Kind::Assignment, 21, 20, 5, true),
            urgent,
            request(5, Kind::Assignment, 23, 5, 5, true),
        ];
        let subjects = [
            (20, open_task(Some(READER), true)),
            (21, open_task(Some(READER), true)),
            (22, open_task(Some(READER), true)),
            (23, open_task(Some(READER), false)),
        ];
        let s = sorted(&notes, &subjects, &[], None);
        assert_eq!(
            layout(&s),
            vec![(Kind::Assignment, ids(&[22, 20, 21, 23]))],
            "the urgent one, then the oldest, and the one waiting on another task last"
        );
    }

    #[test]
    fn the_selected_row_stays_after_its_move_until_the_selection_moves_on() {
        let notes = [request(2, Kind::Ruling, 20, 10, 5, false)];
        let accepted = [(20, Subject::Ruling { status: DecisionStatus::Accepted })];
        let kept = sorted(&notes, &accepted, &[], Some(20));
        let row = kept.row(id(20)).expect("kept while selected");
        assert!(!row.waits);
        assert_eq!(kept.waiting(), 0, "the badge no longer counts it");
        assert!(kept.conversations.is_empty());
        let moved_on = sorted(&notes, &accepted, &[], None);
        assert!(moved_on.row(id(20)).is_none());
        assert_eq!(moved_on.conversations, ids(&[2]), "read with the move, it is a card unread only hides");
        // A selected subject with no row to keep is nothing to keep.
        assert!(sorted(&[conversation(3, 1)], &[], &[], Some(20)).row(id(20)).is_none());
    }

    #[test]
    fn a_row_kept_after_its_move_stays_where_it_stood_and_shows_past_the_first_eight() {
        // Ten ready tasks assigned to the reader in one batch, the oldest first.
        let notes: Vec<Note> =
            (0..10).map(|n| request(100 + n, Kind::Assignment, 20 + n, i64::from(n), 5, false)).collect();
        let open: Vec<(u8, Subject)> = (0..10).map(|n| (20 + n, open_task(Some(READER), true))).collect();
        let before = sort(&notes, &held(&open), id(READER), true, None);
        let order = |s: &Sorted| s.rows().map(|r| r.subject).collect::<Vec<_>>();
        let shown = |s: &Sorted| {
            s.group(Kind::Assignment)
                .map(|g| g.shown(false))
                .unwrap_or_default()
                .iter()
                .flat_map(|b| b.rows.iter().map(|r| r.subject))
                .collect::<Vec<_>>()
        };
        assert_eq!(shown(&before), ids(&[20, 21, 22, 23, 24, 25, 26, 27]), "the first eight");
        // The eighth is started: in progress, and no longer ready by the transition table, which
        // put it last in its batch, past the eight, until it kept where it stood.
        let started = |holds: &mut Held, n: u8| {
            holds.subjects.insert(
                id(n),
                Subject::Task { status: TaskStatus::InProgress, assignee: Some(id(READER)), ready: false },
            );
        };
        let mut after = held(&open);
        started(&mut after, 27);
        let stood = before.row(id(27)).map(Row::stood);
        let kept = sort(&notes, &after, id(READER), true, Some(Keep { subject: id(27), stood }));
        assert_eq!(order(&kept), order(&before), "the started row stays where it stood");
        assert!(shown(&kept).contains(&id(27)));
        assert!(!kept.row(id(27)).is_some_and(|r| r.waits));
        // A kept row past the first eight shows too: the tenth, started after "N more" showed it.
        let mut after = held(&open);
        started(&mut after, 29);
        let stood = before.row(id(29)).map(Row::stood);
        let kept = sort(&notes, &after, id(READER), true, Some(Keep { subject: id(29), stood }));
        assert_eq!(shown(&kept), ids(&[20, 21, 22, 23, 24, 25, 26, 27, 29]));
        assert_eq!(kept.group(Kind::Assignment).map(|g| g.hidden(false)), Some(1), "the ninth waits behind N more");
    }

    #[test]
    fn a_task_is_held_up_by_what_it_waits_on_and_its_hold_not_by_its_own_status() {
        use fwd_core::rules::{NotReadyReason as R, Readiness};
        assert!(nothing_holds_up(&Readiness::Ready));
        assert!(nothing_holds_up(&Readiness::NotReady(vec![R::Status(TaskStatus::InProgress)])), "started");
        assert!(!nothing_holds_up(&Readiness::NotReady(vec![R::BlockedBy(id(3))])));
        assert!(!nothing_holds_up(&Readiness::NotReady(vec![
            R::Status(TaskStatus::InProgress),
            R::Held("wait".into())
        ])));
        assert!(!nothing_holds_up(&Readiness::NotReady(vec![R::DependencyClosed(id(3), TaskStatus::Cancelled)])));
        assert!(!nothing_holds_up(&Readiness::Unknown(Vec::new())));
    }

    #[test]
    fn requests_about_what_the_page_does_not_hold_wait_until_the_daemon_has_answered() {
        let notes = [request(2, Kind::Assignment, 20, 10, 5, true)];
        let sort_with = |loaded| sort(&notes, &Held::default(), id(READER), loaded, None);
        assert_eq!(layout(&sort_with(false)), vec![(Kind::Assignment, ids(&[20]))], "not arrived yet");
        let answered = sort_with(true);
        assert!(answered.groups.is_empty(), "no such task");
        assert_eq!(answered.folds.iter().map(|f| f.members.clone()).collect::<Vec<_>>(), vec![ids(&[2])]);
    }

    #[test]
    fn another_principals_inbox_sorts_for_them() {
        // The task is assigned to the principal whose inbox shows, not to whoever reads it.
        let notes = [request(2, Kind::Assignment, 20, 10, 5, true)];
        let theirs = [(20, open_task(Some(7), true))];
        let s = sort(&notes, &held(&theirs), id(7), true, None);
        assert_eq!(layout(&s), vec![(Kind::Assignment, ids(&[20]))]);
        assert!(sorted(&notes, &theirs, &[], None).groups.is_empty(), "not the reader's");
    }

    #[test]
    fn a_group_shows_eight_rows_and_the_rest_on_asking() {
        let notes: Vec<Note> =
            (0..11).map(|n| request(100 + n, Kind::Question, 20 + n, i64::from(n), 5, true)).collect();
        let subjects: Vec<(u8, Subject)> =
            (0..11).map(|n| (20 + n, Subject::Question { open: true, answered: false })).collect();
        // Two batches: the first three from one scope, the rest from another.
        let scopes: Vec<(u8, u8)> = (0..11).map(|n| (20 + n, if n < 3 { 40 } else { 41 })).collect();
        let s = sorted(&notes, &subjects, &scopes, None);
        let group = s.group(Kind::Question).expect("the questions");
        assert_eq!(group.len(), 11);
        let rows = |batches: &[Batch]| batches.iter().map(|b| b.rows.len()).collect::<Vec<_>>();
        assert_eq!(rows(&group.batches), vec![8, 3], "the newer batch first");
        assert_eq!(rows(&group.shown(false)), vec![8], "a batch none of whose rows shows is left out");
        assert_eq!(group.hidden(false), 3);
        assert_eq!(rows(&group.shown(true)), vec![8, 3]);
        assert_eq!(group.hidden(true), 0);
    }

    #[test]
    fn a_question_is_answered_by_the_readers_reply_in_its_thread_and_by_no_one_elses() {
        let (reader, other) = (id(READER), id(9));
        let (question, forward, reply) = (id(20), id(21), id(22));
        let answered = answered_by(
            reader,
            [
                // The reader's question opens its own thread: it answers nothing.
                (reader, None, None),
                // Someone else's reply to the question.
                (other, Some(question), Some(question)),
                // The reader's reply to a reply deep in another thread.
                (reader, Some(reply), Some(id(30))),
                // Someone else's forward in that thread.
                (other, Some(id(31)), Some(id(30))),
            ],
        );
        assert!(!answered.contains(&question), "someone else's reply does not answer it for the reader");
        assert!(answered.contains(&reply) && answered.contains(&id(30)), "the reader wrote in that thread");
        assert!(!answered.contains(&forward));
        let answered = answered_by(reader, [(reader, Some(question), Some(question))]);
        assert!(answered.contains(&question));
    }

    /// A notification as the daemon's selection reads it: whose it is, its status and its type.
    struct Stored {
        recipient: EntityId,
        status: NotificationStatus,
        t: NotificationType,
    }

    impl ankurah::core::selection::filter::ValueLookup<ankurah::ankql::ast::Parsed> for Stored {
        fn value_at(&self, path: &ankurah::ankql::ast::PathExpr) -> Option<ankurah::Value> {
            match path.property() {
                "recipient" => Some(ankurah::Value::EntityId(self.recipient)),
                "status" => Some(ankurah::Value::String(self.status.to_string())),
                "notification_type" => Some(ankurah::Value::String(self.t.to_string())),
                _ => None,
            }
        }
    }

    #[test]
    fn the_badge_holds_the_readers_requests_read_or_not_and_nothing_else() {
        use NotificationType as T;
        let (me, other) = (id(READER), id(9));
        let selection = with_id(&requests_selection(), me).expect("the selection parses");
        let selects = |recipient, status, t| {
            ankurah::core::selection::filter::evaluate_predicate(&Stored { recipient, status, t }, &selection.predicate)
                .expect("it evaluates")
        };
        for t in REQUEST_TYPES {
            assert!(selects(me, NotificationStatus::Unread, t), "{t:?}");
            assert!(selects(me, NotificationStatus::Read, t), "{t:?} read: it may still wait");
            assert!(!selects(me, NotificationStatus::Dismissed, t), "{t:?} dismissed");
            assert!(!selects(other, NotificationStatus::Unread, t), "{t:?} another's");
        }
        for t in [T::Replied, T::TaskCommented, T::CodeCommented, T::Mentioned, T::Digest, T::DecisionCommented] {
            assert!(!selects(me, NotificationStatus::Unread, t), "{t:?} is no request");
        }
        // Each of them makes a request about what it points at, and every request is one of them.
        let pointing = [
            (T::QuestionAsked, "Message"),
            (T::DecisionProposed, "Decision"),
            (T::ReviewRequested, "Task"),
            (T::TaskProposed, "Task"),
            (T::TaskAssigned, "Task"),
            (T::TaskUnblocked, "Task"),
        ];
        assert_eq!(pointing.map(|(t, _)| t), REQUEST_TYPES);
        for (t, target) in pointing {
            assert!(Kind::of(t, Some(target)).is_some(), "{t:?}");
        }
    }

    #[test]
    fn group_keys_name_each_kind_once() {
        for kind in Kind::ALL {
            assert_eq!(Kind::keyed(kind.key()), Some(kind));
        }
        assert_eq!(Kind::keyed("conversations"), None);
        assert_eq!(Kind::Question.heading("you"), "Questions for you");
        assert_eq!(Kind::Assignment.heading("lead-fwd"), "Assigned to lead-fwd");
    }

    #[test]
    fn a_comment_about_a_subject_with_a_row_shows_beneath_the_row_instead_of_a_card() {
        // A task assigned to the reader, read; a comment on it, new; a comment on another task.
        let notes = [
            request(2, Kind::Assignment, 20, 10, 5, false),
            comment(3, 20, &[20], 50, true),
            comment(4, 30, &[21], 51, true),
        ];
        let s = sorted(&notes, &[(20, open_task(Some(READER), true))], &[], None);
        let row = s.row(id(20)).expect("the task's row");
        assert_eq!((row.comments.clone(), row.comments_unread), (ids(&[3]), 1));
        assert_eq!(row.members, ids(&[3, 2]), "a move from the row marks it read with the row");
        assert_eq!(row.newest_request(), Some(id(2)), "why the row is here is its request, not the comment");
        assert!(row.unread, "new while its comment is");
        assert_eq!((row.first, row.newest), (10, 10), "a comment moves the row nowhere");
        assert_eq!(s.conversations, ids(&[4]), "about no row, a card");
    }

    #[test]
    fn a_reply_joins_the_question_it_answers_before_the_task_the_thread_is_on() {
        // A question about task 20, asked of the reader (row 30), and the task, assigned to them.
        let notes = [
            request(2, Kind::Question, 30, 10, 5, true),
            request(3, Kind::Assignment, 20, 11, 6, true),
            comment(4, 20, &[30, 30, 20], 30, true),
        ];
        let subjects = [(30, Subject::Question { open: true, answered: false }), (20, open_task(Some(READER), true))];
        let s = sorted(&notes, &subjects, &[], None);
        assert_eq!(s.row(id(30)).map(|r| r.comments.clone()), Some(ids(&[4])), "the question's row");
        assert_eq!(s.row(id(20)).map(|r| r.comments.len()), Some(0));
    }

    #[test]
    fn a_ruling_and_the_task_asking_the_reader_to_make_it_share_the_rulings_row() {
        // Ruling 40 proposed; task 20, assigned to the reader, cites it and gathered a comment;
        // task 21 cites nothing.
        let notes = [
            request(2, Kind::Ruling, 40, 10, 5, true),
            request(3, Kind::Assignment, 20, 11, 5, true),
            request(4, Kind::Assignment, 21, 12, 5, true),
            comment(6, 20, &[20], 50, true),
        ];
        let mut holds =
            held(&[(40, proposed()), (20, open_task(Some(READER), true)), (21, open_task(Some(READER), true))]);
        holds.cites.insert(id(20), ids(&[40]));
        let s = sort(&notes, &holds, id(READER), true, None);
        assert_eq!(layout(&s), vec![(Kind::Ruling, ids(&[40])), (Kind::Assignment, ids(&[21]))]);
        let row = s.row(id(40)).expect("the ruling's row");
        assert_eq!(row.task, Some(id(20)));
        assert_eq!(row.members, ids(&[6, 3, 2]), "the task's request and its comment with the ruling's");
        assert_eq!(row.comments, ids(&[6]));
        assert_eq!(s.paired, 1);
        assert_eq!(s.waiting(), 2, "one row for the ruling and its task");
    }

    #[test]
    fn a_task_citing_two_rulings_or_a_ruling_two_tasks_cite_stays_apart() {
        let notes = [
            request(2, Kind::Ruling, 40, 10, 5, true),
            request(3, Kind::Ruling, 41, 10, 5, true),
            request(4, Kind::Assignment, 20, 11, 5, true),
            request(5, Kind::Ruling, 42, 10, 5, true),
            request(6, Kind::Assignment, 21, 12, 5, true),
            request(7, Kind::Assignment, 22, 12, 5, true),
        ];
        let open = || open_task(Some(READER), true);
        let mut holds =
            held(&[(40, proposed()), (41, proposed()), (42, proposed()), (20, open()), (21, open()), (22, open())]);
        // Task 20 asks for two rulings; rulings 42 is asked for by two tasks.
        holds.cites.insert(id(20), ids(&[40, 41]));
        holds.cites.insert(id(21), ids(&[42]));
        holds.cites.insert(id(22), ids(&[42]));
        let s = sort(&notes, &holds, id(READER), true, None);
        assert_eq!(s.paired, 0);
        assert!(s.rows().all(|r| r.task.is_none()));
        assert_eq!(s.group(Kind::Assignment).map(Group::len), Some(3));
        // A task citing a second proposed ruling, one the reader has no row for (proposed to
        // someone else, or dismissed), stays apart too: marking it done would close a task that
        // still asks for that ruling.
        let notes = [request(2, Kind::Ruling, 40, 10, 5, true), request(4, Kind::Assignment, 20, 11, 5, true)];
        let mut holds = held(&[(40, proposed()), (20, open())]);
        holds.cites.insert(id(20), ids(&[40, 43]));
        let s = sort(&notes, &holds, id(READER), true, None);
        assert_eq!((s.paired, layout(&s).len()), (0, 2));
        // A ruling that no longer waits pairs with nothing: its task keeps its own row.
        let notes = [request(2, Kind::Ruling, 40, 10, 5, false), request(4, Kind::Assignment, 20, 11, 5, true)];
        let mut holds = held(&[(40, Subject::Ruling { status: DecisionStatus::Accepted }), (20, open())]);
        holds.cites.insert(id(20), ids(&[40]));
        let s = sort(&notes, &holds, id(READER), true, None);
        assert_eq!(layout(&s), vec![(Kind::Assignment, ids(&[20]))]);
    }

    #[test]
    fn unread_notifications_whose_threads_were_resolved_before_the_reader_looked_fold_per_task() {
        let mut digest = comment(6, 40, &[21], 52, true);
        digest.steps = true;
        let mut near = comment(3, 20, &[21], 50, true);
        near.task = Some(id(21));
        let mut also_near = comment(4, 25, &[21], 51, true);
        also_near.task = Some(id(21));
        also_near.t = Some(NotificationType::Replied);
        let mut read = comment(5, 30, &[21], 50, false);
        read.task = Some(id(21));
        let notes = [
            near,
            also_near,
            read,
            digest,
            // A question asked of the reader that its asker resolved before they answered.
            request(7, Kind::Question, 60, 35, 8, true),
            // A comment in a thread still open.
            comment(9, 45, &[22], 53, true),
        ];
        let mut holds = held(&[(60, Subject::Question { open: false, answered: false })]);
        holds.resolved = ids(&[50, 51, 52]).into_iter().collect();
        let s = sort(&notes, &holds, id(READER), true, None);
        /// A line as the test reads it: its task, its sender, its notifications and its words.
        type Line = (Option<EntityId>, Option<EntityId>, Vec<EntityId>, String);
        let lines: Vec<Line> = s.folds.iter().map(|f| (f.task, f.sender, f.members.clone(), f.words.clone())).collect();
        assert_eq!(
            lines,
            vec![
                (None, Some(id(8)), ids(&[7]), "1 question".to_string()),
                (Some(id(21)), None, ids(&[4, 3]), "1 comment and 1 reply".to_string()),
            ],
            "the newest line first; per task, else per sender"
        );
        let mut conversations = s.conversations.clone();
        conversations.sort();
        assert_eq!(conversations, ids(&[5, 6, 9]), "read, holding a step, or in a thread still open: cards");
        assert_eq!(s.settled(), 3);
        assert_eq!(s.fold(id(21)).map(|f| f.members.len()), Some(2));
    }

    #[test]
    fn a_line_of_settled_notifications_says_what_they_are_the_most_first() {
        use NotificationType as T;
        let mut types = vec![Some(T::CodeCommented); 169];
        types.extend(vec![Some(T::Replied); 18]);
        assert_eq!(what_they_are(&types), "169 comments and 18 replies");
        assert_eq!(what_they_are(&[Some(T::QuestionAsked)]), "1 question");
        assert_eq!(
            what_they_are(&[Some(T::Mentioned), Some(T::Replied), Some(T::Replied), Some(T::TaskCommented)]),
            "2 replies, 1 comment and 1 mention"
        );
        assert_eq!(what_they_are(&[]), "");
    }
}
