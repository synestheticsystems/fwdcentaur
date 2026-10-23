//! A row of the Inbox's Waiting on you: one thing that waits on the inbox's reader, whatever
//! notifications brought it (`waiting.rs`), drawn for deciding and acting without leaving the list
//! (D137; the design pass of task RgAEFRFb). It is the list-item form of its subject (D44: every
//! kind of entity renders in four modes): one line saying what it is, a second saying who sent it
//! and when, and on its right the moves its subject allows, as the task's panel and a decision's
//! own row offer them (D33: task status stays as transition buttons), at most three: Reply and
//! Resolve on a question; Accept and Reject on a proposed ruling, each asking why, since every move
//! on a decision keeps its reason as a comment on it; Accept and Return on a review; Approve and
//! Reject on a proposed task; Start and Mark done on an assigned task. Only the selected row's
//! first move is the page's primary button, so that a column of ten rulings is not a column of ten
//! blue buttons inviting a click before the reading. A move that needs a reason opens a box beneath
//! the row's lines, where Enter applies it and Escape closes it. Each move runs the operation the
//! panel runs, as the acting principal; once it is made the row's notifications are marked read,
//! as a move on a card's own row marks the card's, and the row stays, saying what became of its
//! subject, until the selection moves on. The × at its end dismisses its notifications.
//!
//! The comments, replies and mentions about a row's subject join the row rather than taking cards of
//! their own (task w4g0qBJY): at rest the row grows by one line, the newest one's author and first
//! line, and says how many there are and how many are new. A ruling's row shares itself with the
//! task assigned to the reader that asks them to make it, when the task cites the ruling: the task
//! shows as a lozenge on the row, and Accept or Reject offers to mark it done in the same step, by a
//! box checked in plain sight, where the task needs no review.
//!
//! ArrowRight, or the caret, opens what the row holds beneath it, mounting no thread until then: a
//! question's whole text, the replies to it and the box to answer it in; a ruling's record and its
//! comments, with its paired task's comments beneath; a task's what, why and done-when (and a
//! review's evidence) with its comments. On
//! another principal's inbox, shown to read (`whose.rs`), the × is disabled, and a move is the
//! acting principal's where the rules let them make it: Start on a task assigned to the
//! principal whose inbox it is stays disabled, saying why on hover. Which moves a row offers is
//! worked out as data ([`offers`]) and tested natively, from the same rules the panels read.

use crate::accordion::{self, RowStep, Section};
use crate::cards::{id_at, Item, Page};
use crate::comment_box::{self, CommentBox};
use crate::lozenges::{self, Target};
use crate::mentions::{MentionList, Mentions};
use crate::referents::Models;
use crate::route::{self, Route};
use crate::session::{self, ctx};
use crate::thread::{self, Hooks, Model};
use crate::waiting::{Kind, Row, Sorted};
use crate::{claim, empty, fmt, keys, notification_card, references, store, whose};
use ankurah::EntityId;
use fwd_core::model::*;
use fwd_core::ops::{self, MessageContext};
use fwd_core::rules::{self, DecisionMove, NotReadyReason, Readiness, RuleError, Verb};
use leptos::prelude::*;
use serde_json::Value;
use std::collections::HashSet;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;

/// A move a row offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    /// Opens the box in the row whose text answers the question.
    Reply,
    /// Resolves the question.
    Resolve,
    /// Accepts the proposed ruling, asking why (`DecisionMove::Accept`).
    AcceptRuling,
    /// Rejects the proposed ruling, asking why (`DecisionMove::Reject`).
    RejectRuling,
    /// Accepts the work in review (`Verb::Accept`).
    AcceptReview,
    /// Returns the work in review, asking what should change (`Verb::Return`).
    Return,
    /// Approves the proposed task (`Verb::Approve`).
    Approve,
    /// Rejects the proposed task, asking why (`Verb::Reject`).
    RejectTask,
    /// Starts the task in the scope it is worked in, as Start on its panel does (`claim.rs`).
    Start,
    /// Marks the task done (`Verb::Done`), which only a task needing no review allows.
    Done,
}

impl Move {
    pub fn label(self) -> &'static str {
        match self {
            Move::Reply => "Reply",
            Move::Resolve => "Resolve",
            Move::AcceptRuling | Move::AcceptReview => "Accept",
            Move::RejectRuling | Move::RejectTask => "Reject",
            Move::Return => "Return",
            Move::Approve => "Approve",
            Move::Start => "Start",
            Move::Done => "Mark done",
        }
    }

    /// What the box beneath the row asks for, for a move that needs a reason, `named` naming the
    /// ruling (`D204`); none for a move made at once. A decision keeps every move's reason as a
    /// comment on it (`ops::move_decision`), so the box says so.
    pub fn asks(self, named: &str) -> Option<String> {
        match self {
            Move::AcceptRuling => Some(format!("Why you accept it. The reason is kept as a comment on {named}.")),
            Move::RejectRuling => Some(format!("Why you reject it. The reason is kept as a comment on {named}.")),
            Move::Return => Some("What should change. It is kept as a comment on the task.".to_string()),
            Move::RejectTask => Some("Why you reject it. The reason is kept as a comment on the task.".to_string()),
            _ => None,
        }
    }

    /// What the box's button says, `named` naming the ruling.
    fn applies(self, named: &str) -> String {
        match self {
            Move::AcceptRuling | Move::RejectRuling => format!("{} {named}", self.label()),
            _ => self.label().to_string(),
        }
    }

    /// What Enter in the box does, for the line beneath it.
    fn enter(self) -> &'static str {
        match self {
            Move::AcceptRuling => "Enter accepts",
            Move::RejectRuling | Move::RejectTask => "Enter rejects",
            Move::Return => "Enter returns it",
            _ => "Enter applies",
        }
    }

    /// A move drawn as quiet words beside the main one: resolving a question beside answering it.
    fn quiet(self) -> bool {
        self == Move::Resolve
    }
}

/// What a row's moves are worked out from: its subject's state, as the page holds it.
#[derive(Clone, Debug, PartialEq)]
pub enum MoveFacts {
    Question {
        open: bool,
    },
    Ruling {
        status: DecisionStatus,
    },
    Task {
        status: TaskStatus,
        needs_review: bool,
        assignee: Option<EntityId>,
        /// Who submitted it for review, whom accepting it refuses (`rules::can_accept`).
        submitted_by: Option<EntityId>,
    },
    /// Not held: nothing to move.
    Absent,
}

/// A move a row offers, and why the acting principal cannot make it, when it cannot: its button
/// then stays, disabled, saying why on hover.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    pub mv: Move,
    pub refused: Option<String>,
}

/// The moves a row of `kind` offers `me`, the principal the page acts as, while its subject is as
/// `facts` say, `handle` naming a principal: the moves the transition table and the decision rules
/// allow from the subject's status, as the task's panel and a decision's own row offer them, and
/// none once the subject has stopped waiting (a row kept after its move). Accepting work one
/// submitted, and starting a task assigned to someone else, stay offered but refused, saying why.
pub fn offers(kind: Kind, facts: &MoveFacts, me: EntityId, handle: impl Fn(EntityId) -> String) -> Vec<Offer> {
    let offer = |mv: Move| Offer { mv, refused: None };
    let allowed =
        |status: TaskStatus, verb: Verb, needs_review: bool| rules::transition(status, verb, needs_review).is_ok();
    match (kind, facts) {
        (Kind::Question, MoveFacts::Question { open: true }) => vec![offer(Move::Reply), offer(Move::Resolve)],
        (Kind::Ruling, MoveFacts::Ruling { status }) => {
            [(Move::AcceptRuling, DecisionMove::Accept), (Move::RejectRuling, DecisionMove::Reject)]
                .into_iter()
                .filter(|(_, mv)| mv.moves_from() == *status)
                .map(|(row_move, _)| offer(row_move))
                .collect()
        }
        (Kind::Review, MoveFacts::Task { status, needs_review, submitted_by, .. }) => {
            let mut offered = Vec::new();
            if allowed(*status, Verb::Accept, *needs_review) {
                let own = submitted_by.is_some_and(|submitter| rules::can_accept(submitter, me).is_err());
                let refused = own.then(|| "You submitted it: someone else accepts it".to_string());
                offered.push(Offer { mv: Move::AcceptReview, refused });
            }
            if allowed(*status, Verb::Return, *needs_review) {
                offered.push(offer(Move::Return));
            }
            offered
        }
        (Kind::Proposal, MoveFacts::Task { status, needs_review, .. }) => {
            [(Move::Approve, Verb::Approve), (Move::RejectTask, Verb::Reject)]
                .into_iter()
                .filter(|(_, verb)| allowed(*status, *verb, *needs_review))
                .map(|(row_move, _)| offer(row_move))
                .collect()
        }
        (Kind::Assignment, MoveFacts::Task { status, needs_review, assignee, .. }) => {
            let mut offered = Vec::new();
            if allowed(*status, Verb::Start, *needs_review) {
                let refused = match rules::can_start(*status, *assignee, me) {
                    Ok(()) => None,
                    Err(RuleError::AssignedToAnother { assignee }) => {
                        Some(format!("Assigned to {}: only its assignee starts it", handle(assignee)))
                    }
                    Err(e) => Some(e.to_string()),
                };
                offered.push(Offer { mv: Move::Start, refused });
            }
            if allowed(*status, Verb::Done, *needs_review) {
                offered.push(offer(Move::Done));
            }
            offered
        }
        _ => Vec::new(),
    }
}

/// A task paired with a ruling's row, as the box that offers to mark it done reads it: its id, its
/// status and whether it needs review.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Paired {
    pub task: EntityId,
    pub status: TaskStatus,
    pub needs_review: bool,
}

impl Paired {
    /// Whether the move that rules on its ruling may also mark it done: only where the task can be
    /// marked done without review (the transition table's `done`), so the box never offers what
    /// the move would be refused.
    pub fn may_be_done(self) -> bool {
        rules::transition(self.status, Verb::Done, self.needs_review).is_ok()
    }
}

/// The task the move `mv` on a ruling's row also marks done: the task paired with the ruling, when
/// the move rules on it (Accept or Reject alike: ruling either way does what the task asks, the
/// lead's ruling of 2026-10-03), the box is `checked`, and the task may be marked done.
pub fn also_done(mv: Move, paired: Option<Paired>, checked: bool) -> Option<EntityId> {
    let rules_on_it = matches!(mv, Move::AcceptRuling | Move::RejectRuling);
    paired.filter(|p| rules_on_it && checked && p.may_be_done()).map(|p| p.task)
}

/// A key for the rows a row of Waiting on you opens beneath it about `entity`, which no card's key
/// is, so that a card about the same entity keeps its own rows (`thread::card_row` keys a card's
/// rows by the card): `entity`'s id with every bit turned over.
fn opened_key(entity: EntityId) -> EntityId {
    EntityId::from_bytes(entity.to_bytes().map(|b| !b))
}

/// The notifications of `row` a move made from it marks read: its requests, and the comments it
/// showed the reader, every one while it is `open` (its thread shows them), else the newest, whose
/// first line it shows at rest. A comment it did not show stays unread, and once the row leaves,
/// it is a card of its own, so that nothing unread leaves without a word.
pub fn shown_to_reader(row: &Row, open: bool) -> Vec<EntityId> {
    let shown: Vec<EntityId> = if open { row.comments.clone() } else { row.comments.iter().take(1).copied().collect() };
    row.members.iter().copied().filter(|m| !row.comments.contains(m) || shown.contains(m)).collect()
}

/// What the comment count on a row says: how many comments, replies and mentions joined it and how
/// many of them are new ("1 comment, new", "3 comments, 1 new"); none while none did.
pub fn comment_words(comments: usize, new: usize) -> Option<String> {
    let counted = match comments {
        0 => return None,
        1 => "1 comment".to_string(),
        n => format!("{n} comments"),
    };
    Some(match (comments, new) {
        (_, 0) => counted,
        (1, _) => format!("{counted}, new"),
        (_, new) => format!("{counted}, {new} new"),
    })
}

/// What the row's moves are worked out from, read from the page's standing queries by id.
fn move_facts(kind: Kind, subject: EntityId) -> MoveFacts {
    match kind {
        Kind::Question => store::message(subject).map_or(MoveFacts::Absent, |m| MoveFacts::Question {
            open: m.status().ok() == Some(MessageStatus::Active),
        }),
        Kind::Ruling => store::decision(subject)
            .and_then(|d| d.status().ok())
            .map_or(MoveFacts::Absent, |status| MoveFacts::Ruling { status }),
        Kind::Review | Kind::Proposal | Kind::Assignment => store::task(subject)
            .and_then(|t| {
                let context: Value = t.context().map(Into::into).unwrap_or(Value::Null);
                Some(MoveFacts::Task {
                    status: t.status().ok()?,
                    needs_review: t.needs_review().unwrap_or(false),
                    assignee: ops::assignee_of(&t).ok().flatten(),
                    submitted_by: id_at(&context["submitted_by"]),
                })
            })
            .unwrap_or(MoveFacts::Absent),
    }
}

/// Makes the move `mv` on `subject` as `me`, `why` being the reason its box asked for: the
/// operation the task's panel, a decision's own row and a message's resolve run.
async fn make(mv: Move, me: EntityId, subject: EntityId, why: Option<&str>) -> anyhow::Result<()> {
    let ctx = ctx();
    let reason = why.unwrap_or("");
    match mv {
        Move::Resolve => ops::resolve_message(&ctx, me, subject).await.map(drop),
        Move::AcceptRuling => ops::move_decision(&ctx, me, subject, DecisionMove::Accept, reason).await.map(drop),
        Move::RejectRuling => ops::move_decision(&ctx, me, subject, DecisionMove::Reject, reason).await.map(drop),
        Move::AcceptReview => ops::transition(&ctx, me, subject, Verb::Accept, None).await.map(drop),
        Move::Return => ops::transition(&ctx, me, subject, Verb::Return, why).await.map(drop),
        Move::Approve => ops::transition(&ctx, me, subject, Verb::Approve, None).await.map(drop),
        Move::RejectTask => ops::transition(&ctx, me, subject, Verb::Reject, why).await.map(drop),
        Move::Done => ops::transition(&ctx, me, subject, Verb::Done, None).await.map(drop),
        // A reply is sent from the row's box, and Start from its own control (`claim::row_control`).
        Move::Reply | Move::Start => Ok(()),
    }
}

/// The first line of `text` that says something, cut at `n` characters.
fn first_line(text: &str, n: usize) -> String {
    fmt::truncate(text.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or(""), n)
}

/// Gives the keyboard back to the row whose element id is `id`, once it is drawn, when nothing
/// else has it, as when the box or the button that had it went with the move it made.
fn refocus_row(id: String) {
    request_animation_frame(move || {
        let Some(document) = web_sys::window().and_then(|w| w.document()) else { return };
        let Some(row) = document.get_element_by_id(&id) else { return };
        let free = document.active_element().is_none_or(|a| a.tag_name() == "BODY" || row.contains(Some(a.as_ref())));
        if let Some(row) = row.dyn_into::<web_sys::HtmlElement>().ok().filter(|_| free) {
            let _ = row.focus();
        }
    });
}

/// Gives the keyboard to the element whose id is `id`, once it is drawn.
fn focus_element(id: String) {
    request_animation_frame(move || {
        let element = web_sys::window().and_then(|w| w.document()).and_then(|d| d.get_element_by_id(&id));
        if let Some(element) = element.and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok()) {
            let _ = element.focus();
        }
    });
}

/// The row's first line: what its subject is. A question's asker and its first line; a ruling's
/// number and title; a task's title and its status, or "ready" for an assigned task nothing holds
/// up. A title leads to its subject's page.
fn title(kind: Kind, subject: EntityId) -> AnyView {
    match kind {
        Kind::Question => (move || {
            let Some(m) = store::message(subject) else {
                return view! { <span class="muted">{empty::absent(Some(Model::Message))}</span> }.into_any();
            };
            let asker = m.author().ok().map(|a| store::handle_of(EntityId::from(a))).unwrap_or_default();
            let text = references::plain(&ops::text(m.content()).unwrap_or_default());
            view! { <span class="who">{asker}</span> " " {first_line(&text, 200)} }.into_any()
        })
        .into_any(),
        Kind::Ruling => (move || {
            let Some(d) = store::decision(subject) else {
                return view! { <span class="muted">{empty::absent(Some(Model::Decision))}</span> }.into_any();
            };
            let status = d.status().ok().filter(|s| *s != DecisionStatus::Proposed);
            view! {
                <span class="decision-number">{format!("D{}", d.number().unwrap_or(0))}</span>
                " "
                <a href=Route::decision(subject).hash()>{d.title().unwrap_or_default()}</a>
                {status.map(|s| view! { " " <span class=format!("pill decision-{s}")>{s.to_string()}</span> })}
            }
            .into_any()
        })
        .into_any(),
        Kind::Review | Kind::Proposal | Kind::Assignment => (move || {
            let Some(t) = store::task(subject) else {
                return view! { <span class="muted">{empty::absent(Some(Model::Task))}</span> }.into_any();
            };
            let status = t.status().ok();
            let ready = kind == Kind::Assignment && status == Some(TaskStatus::Open) && store::readiness(&t).is_ready();
            let pill = match (ready, status) {
                (true, _) => Some(view! { <span class="pill ready-pill">"ready"</span> }.into_any()),
                (false, Some(s)) => {
                    Some(view! { <span class=format!("pill status-{s}")>{fmt::status_label(s)}</span> }.into_any())
                }
                (false, None) => None,
            };
            view! { <a href=Route::task(subject).hash()>{t.title().unwrap_or_default()}</a> " " {pill} }.into_any()
        })
        .into_any(),
    }
}

/// The row's second line: who sent it and when, and what its kind of request needs at a glance.
/// A question: what it is about, how soon it wants its answer. A ruling: who proposed it, its area,
/// the scope it governs. A review: who submitted it and the first line of the evidence. A proposed
/// task: who proposed it and the first line of why. An assigned task: who assigned it, and whether
/// it is ready or what it waits on. `between` goes before when it came: what joined the row.
fn meta(kind: Kind, subject: EntityId, sender: Option<EntityId>, at: Memo<i64>, between: AnyView) -> AnyView {
    let ago = move || fmt::ago(at.get());
    let said: AnyView = match kind {
        Kind::Question => (move || {
            let m = store::message(subject)?;
            let task = m.task().ok().flatten().map(|t| Target::Task(EntityId::from(t)));
            let decision = m.decision().ok().flatten().map(|d| Target::Decision(EntityId::from(d)));
            let anchor = m.anchor().ok().flatten().map(|a| Target::Anchor(EntityId::from(a)));
            let scope = m.scope().ok().flatten().map(|s| Target::Scope(EntityId::from(s)));
            let about = task
                .or(decision)
                .or(anchor)
                .or(scope)
                .map(|target| view! { "about " {lozenges::single(target)} " · " });
            let priority = m.priority().ok().map(|p| format!("{p} · "));
            Some(view! { {about} {priority} })
        })
        .into_any(),
        Kind::Ruling => (move || {
            let d = store::decision(subject)?;
            let proposer = d.author().ok().map(|a| store::handle_of(EntityId::from(a))).unwrap_or_default();
            let area = d.area().ok().flatten().map(|a| format!("{a} · "));
            let governs = match ops::governs(&d).ok().flatten() {
                Some(scope) => lozenges::single(Target::Scope(scope)),
                None => view! { <span class="governs">"project-wide"</span> }.into_any(),
            };
            Some(view! { {proposer} " · " {area} {governs} " · " })
        })
        .into_any(),
        Kind::Review => (move || {
            let t = store::task(subject)?;
            let context: Value = t.context().map(Into::into).unwrap_or(Value::Null);
            let by = id_at(&context["submitted_by"]).or(sender).map(store::handle_of).unwrap_or_default();
            let evidence = first_line(context["evidence"].as_str().unwrap_or(""), 120);
            let evidence =
                (!evidence.is_empty()).then(|| view! { <span class="request-quote">{evidence}</span> " · " });
            Some(view! { "submitted by " {by} " · " {evidence} })
        })
        .into_any(),
        Kind::Proposal => (move || {
            let t = store::task(subject)?;
            let by = t.created_by().ok().map(|p| store::handle_of(EntityId::from(p))).unwrap_or_default();
            let why = first_line(&ops::text(t.why()).unwrap_or_default(), 120);
            let why = (!why.is_empty()).then(|| view! { <span class="request-quote">{why}</span> " · " });
            Some(view! { "proposed by " {by} " · " {why} })
        })
        .into_any(),
        Kind::Assignment => (move || {
            let t = store::task(subject)?;
            let context: Value = t.context().map(Into::into).unwrap_or(Value::Null);
            let by = id_at(&context[ops::ASSIGNED_BY_KEY])
                .or(sender)
                .map(|p| view! { "assigned by " {store::handle_of(p)} " · " });
            let ready = readiness(&t).map(|said| view! { {said} " · " });
            Some(view! { {by} {ready} })
        })
        .into_any(),
    };
    view! { {said} {between} <span class="request-age">{ago}</span> }.into_any()
}

/// Whether an assigned task is ready, or what holds it up: the tasks it waits on, as lozenges, and
/// its hold; none for a task its own status alone keeps from being ready (a row kept after its
/// start), whose title's pill says that status.
fn readiness(t: &TaskView) -> Option<AnyView> {
    match store::readiness(t) {
        Readiness::Ready => Some("ready".into_any()),
        Readiness::NotReady(reasons) => {
            let blockers: Vec<EntityId> = reasons
                .iter()
                .filter_map(|r| match r {
                    NotReadyReason::BlockedBy(id) | NotReadyReason::DependencyClosed(id, _) => Some(*id),
                    _ => None,
                })
                .collect();
            let held = reasons.iter().any(|r| matches!(r, NotReadyReason::Held(_)));
            if blockers.is_empty() && !held {
                return None;
            }
            let waits = (!blockers.is_empty()).then(|| {
                let lozenges: Vec<AnyView> =
                    blockers.into_iter().map(|id| lozenges::single(Target::Task(id))).collect();
                view! { "waits on " {lozenges} }
            });
            Some(view! { {held.then_some("on hold ")} {waits} }.into_any())
        }
        Readiness::Unknown(_) => Some("what it waits on is unavailable".into_any()),
    }
}

/// The record a ruling's row opens onto: why it came up, the ruling, its consequences.
fn ruling_record(subject: EntityId) -> AnyView {
    (move || {
        let d = store::decision(subject)?;
        let parts = [
            ("Why it came up", ops::text(d.context()).unwrap_or_default()),
            ("The ruling", ops::text(d.decision()).unwrap_or_default()),
            ("Consequences", ops::text(d.consequences()).unwrap_or_default()),
        ];
        Some(prose(parts.into_iter()))
    })
    .into_any()
}

/// The prose a task's row opens onto: what it is, why, when it is done, and for a review the
/// evidence it was submitted with.
fn task_record(kind: Kind, subject: EntityId) -> AnyView {
    (move || {
        let t = store::task(subject)?;
        let context: Value = t.context().map(Into::into).unwrap_or(Value::Null);
        let mut parts = vec![
            ("What", ops::text(t.what()).unwrap_or_default()),
            ("Why", ops::text(t.why()).unwrap_or_default()),
            ("Done when", ops::text(t.done_when()).unwrap_or_default()),
        ];
        if kind == Kind::Review {
            parts.push(("Evidence", context["evidence"].as_str().unwrap_or("").to_string()));
        }
        Some(prose(parts.into_iter()))
    })
    .into_any()
}

/// Labelled prose, each piece that says something.
fn prose(parts: impl Iterator<Item = (&'static str, String)>) -> AnyView {
    parts
        .filter(|(_, text)| !text.trim().is_empty())
        .map(|(label, text)| view! { <section class="prose"><h4>{label}</h4><p>{references::content(&text)}</p></section> })
        .collect::<Vec<_>>()
        .into_any()
}

/// One row of Waiting on you, about `subject`, which asks a move of `kind`, as `sorted` holds it on
/// `page`; `open_rows` holds the subjects whose rows are open.
#[component]
pub fn RequestRow(
    subject: EntityId,
    kind: Kind,
    sorted: Memo<Sorted>,
    page: Page,
    open_rows: RwSignal<HashSet<EntityId>>,
) -> impl IntoView {
    let item = Item::Request(subject);
    let row = Memo::new(move |_| sorted.with(|s| s.row(subject).cloned()));
    let unread = Memo::new(move |_| row.with(|r| r.as_ref().is_some_and(|r| r.unread)));
    let waits = Memo::new(move |_| row.with(|r| r.as_ref().is_some_and(|r| r.waits)));
    let at = Memo::new(move |_| row.with(|r| r.as_ref().map_or(0, |r| r.newest)));
    let sender = row.with_untracked(|r| r.as_ref().and_then(|r| r.sender));
    let selected = Memo::new(move |_| page.selected.get() == Some(item));
    let opened = Memo::new(move |_| open_rows.with(|rows| rows.contains(&subject)));
    let (me, acts) = (page.thread.me, page.acts());
    let reading =
        move || (!acts).then(|| whose::reading_inbox(&store::handle_of(page.whose), &store::handle_of(page.thread.me)));
    // How the row names the task that shares a ruling's row: "your task" on one's own inbox.
    let whose_task =
        move || if acts { "your task".to_string() } else { format!("{}'s task", store::handle_of(page.whose)) };
    // The ids of the row's element, of its reason box and of its reply box.
    let row_id = StoredValue::new(item.element_id());
    let reason_id = StoredValue::new(format!("{}-reason", item.element_id()));
    let reply_id = StoredValue::new(format!("{}-reply", item.element_id()));

    // The move waiting for its reason, the reason, why the last move failed, the reply being
    // written, and the scope Start's picker chose, kept while the row redraws.
    let pending = RwSignal::new(None::<Move>);
    let note = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let replying = RwSignal::new(false);
    let reply = RwSignal::new(String::new());
    let chosen_scope = RwSignal::new(None::<EntityId>);

    // What a move made from the row reads: its requests, and the comments it showed (all of them
    // open, else the newest), read at the click, when the row is on the page; none once it has gone.
    let members = move || {
        let open = opened.try_get_untracked().unwrap_or(false);
        row.try_with_untracked(|r| r.as_ref().map(|r| shown_to_reader(r, open))).flatten().unwrap_or_default()
    };
    // Marks read those of `ids` still unread: what a move made from the row deals with, as a move
    // on a card's own row marks the card's; on another principal's inbox nothing is marked.
    let mark_read = move |ids: Vec<EntityId>| {
        if !acts {
            return;
        }
        let unread: Vec<EntityId> = untrack(|| {
            ids.into_iter()
                .filter(|id| {
                    page.notifications.get(*id).is_some_and(|n| n.status().ok() == Some(NotificationStatus::Unread))
                })
                .collect()
        });
        page.mark(unread, NotificationStatus::Read, false);
    };
    // The keyboard coming into the row, on a move, a box or a lozenge, selects it, so that a row a
    // move is made from stays, showing what became of its subject, until the selection moves on
    // (`waiting::sort`'s `keep`).
    let hold = move || {
        if page.selected.get_untracked() != Some(item) {
            page.selected.set(Some(item));
        }
    };
    let back_to_row = move || focus_element(row_id.get_value());
    // The task paired with the ruling, as it is now, and whether the box marking it done with the
    // ruling is checked: it starts checked.
    let paired = Memo::new(move |_| {
        let task = row.with(|r| r.as_ref().and_then(|r| r.task))?;
        let t = store::task(task)?;
        Some(Paired { task, status: t.status().ok()?, needs_review: t.needs_review().unwrap_or(false) })
    });
    let done_too = RwSignal::new(true);
    let made = move |mv: Move, why: Option<String>| {
        let ids = members();
        let then_done = also_done(mv, paired.get_untracked(), done_too.get_untracked());
        hold();
        spawn_local(async move {
            let done = match make(mv, me, subject, why.as_deref()).await {
                Ok(()) => match then_done {
                    Some(task) => ops::transition(&ctx(), me, task, Verb::Done, None).await.map(drop),
                    None => Ok(()),
                },
                failed => failed,
            };
            match done {
                Ok(()) => {
                    session::wrote(me);
                    pending.try_set(None);
                    note.try_set(String::new());
                    error.try_set(None);
                    mark_read(ids);
                    row_id.try_with_value(|id| refocus_row(id.clone()));
                }
                Err(e) => {
                    error.try_set(Some(e.to_string()));
                }
            }
        });
    };
    let open_reply = move || {
        hold();
        replying.set(true);
        focus_element(reply_id.get_value());
    };
    let choose = move |mv: Move| {
        error.set(None);
        match mv {
            Move::Reply => open_reply(),
            mv if mv.asks("").is_some() => {
                hold();
                pending.set(Some(mv));
                note.set(String::new());
                focus_element(reason_id.get_value());
            }
            mv => made(mv, None),
        }
    };
    let apply = move || {
        let Some(mv) = pending.get_untracked() else { return };
        let why = note.get_untracked();
        if why.trim().is_empty() {
            error.set(Some(format!("{} needs a reason: say why in the box", mv.label())));
            return;
        }
        made(mv, Some(why));
    };
    let cancel = move || {
        pending.set(None);
        note.set(String::new());
        back_to_row();
    };
    let send_reply = move || {
        let text = reply.get_untracked().trim().to_string();
        if text.is_empty() {
            return;
        }
        let ids = members();
        hold();
        spawn_local(async move {
            // At the priority of what it answers, as `fwd reply` and a reply box send it.
            let priority = store::message(subject).and_then(|m| m.priority().ok()).unwrap_or(Priority::Next);
            let mc = MessageContext { reply_to: Some(subject), ..Default::default() };
            match ops::post_message(&ctx(), me, &text, priority, mc).await {
                Ok(_) => {
                    session::wrote(me);
                    reply.try_update(|written| {
                        if written.trim() == text {
                            written.clear();
                        }
                    });
                    replying.try_set(false);
                    error.try_set(None);
                    mark_read(ids);
                    row_id.try_with_value(|id| refocus_row(id.clone()));
                }
                Err(e) => {
                    error.try_set(Some(e.to_string()));
                }
            }
        });
    };

    // Opening a row mounts what it holds; ArrowRight on an open row goes into its thread, ArrowLeft
    // closes it, then climbs to its group's heading, and Enter opens its subject's page (a
    // question's row, its reply box).
    let set_open = move |open: bool| {
        open_rows.update(|rows| {
            if open {
                rows.insert(subject);
            } else {
                rows.remove(&subject);
            }
        })
    };
    let activate = move || match kind {
        Kind::Question => open_reply(),
        Kind::Ruling => route::go(Route::decision(subject)),
        Kind::Review | Kind::Proposal | Kind::Assignment => route::go(Route::task(subject)),
    };
    let on_key = move |ev: web_sys::KeyboardEvent| {
        if ev.default_prevented() || !keys::is_plain(&ev) {
            return;
        }
        let Some(target) = ev.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) else { return };
        if row_id.with_value(|id| target.id() == *id) {
            let Some(step) = accordion::on_row(&ev.key(), opened.get_untracked()) else { return };
            ev.prevent_default();
            match step {
                RowStep::Open => set_open(true),
                RowStep::Close => set_open(false),
                RowStep::IntoThread => {
                    let first = target.query_selector(".request-beneath [role=treeitem]").ok().flatten();
                    if let Some(first) = first.and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok()) {
                        let _ = first.focus();
                    }
                }
                RowStep::ToGroup => page.select(Item::Section(Section::Group(kind))),
                RowStep::Activate => activate(),
            }
        } else if ev.key() == "ArrowLeft" && target.closest("[role=tree]").ok().flatten().is_some() {
            // The thread beneath the row left the key alone: its top rows have no row above them
            // in the thread, so the key goes back up to the row.
            ev.prevent_default();
            page.select(item);
        }
    };
    // A click on the row, not on one of its controls or in what it holds open, selects it.
    let on_click = move |ev: web_sys::MouseEvent| {
        let on_control = ev.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()).is_some_and(|t| {
            t.closest("a, button, input, textarea, select, label, [role=tree], .request-beneath")
                .ok()
                .flatten()
                .is_some()
        });
        if !on_control {
            hold();
        }
    };

    // Why the row is in the inbox, as its newest request's card line says it, on hover.
    let hover = move || {
        let newest = row.with(|r| r.as_ref().and_then(|r| r.newest_request()))?;
        let n = page.notifications.get(newest)?;
        Some(notification_card::sentence(page.whose, &n))
    };
    // Start's own control makes the start (`claim::row_control`) and, in its continuation, marks
    // read the notifications the row held at the click: the start draws the row's moves again (the
    // task leaves Open), and nothing the row owns may carry what follows it. The click also selects
    // the row, as every other move does, so that the row stays after the start until the selection
    // moves on: Safari and Firefox on macOS do not focus a clicked button, so there the row's focus
    // never selects it.
    let started = move || {
        let ids = members();
        hold();
        move || {
            mark_read(ids);
            row_id.try_with_value(|id| refocus_row(id.clone()));
        }
    };
    let moves = move || {
        let offered = offers(kind, &move_facts(kind, subject), me, store::handle_of);
        let first = offered.first().map(|o| o.mv);
        offered
            .into_iter()
            .map(|offer| {
                let primary = Signal::derive(move || selected.get() && first == Some(offer.mv));
                match (offer.mv, offer.refused) {
                    // Start reads the scopes it may start the task in, which redraws it alone.
                    (Move::Start, None) => (move || {
                        claim::row_control(subject, claim::offer(subject), chosen_scope, error, primary, started)
                    })
                    .into_any(),
                    (mv, refused) => {
                        let refused_now = refused.is_some();
                        let class = move || match (mv.quiet(), primary.get() && !refused_now) {
                            (true, _) => "btn subtle",
                            (false, true) => "btn primary",
                            (false, false) => "btn",
                        };
                        view! {
                            <button class=class disabled=refused_now title=refused on:click=move |_| choose(mv)>
                                {mv.label()}
                            </button>
                        }
                        .into_any()
                    }
                }
            })
            .collect::<Vec<_>>()
    };
    // Dismissing the row dismisses every notification of it, as Dismiss on a card does.
    let dismiss = move |_| {
        let every = row.with_untracked(|r| r.as_ref().map(|r| r.members.clone()).unwrap_or_default());
        page.mark(every, NotificationStatus::Dismissed, false);
    };
    let dismiss_says = move || reading().unwrap_or_else(|| "Dismiss: its notifications leave the inbox".to_string());

    // The box a move's reason is written in, beneath the row's lines: drawn again only when the
    // move it waits for changes, so the box keeps the keyboard while the subject changes.
    let named = Memo::new(move |_| {
        (kind == Kind::Ruling)
            .then(|| store::decision(subject).and_then(|d| d.number().ok()).map(|n| format!("D{n}")))
            .flatten()
            .unwrap_or_default()
    });
    let reason_box = move || {
        let mv = pending.get()?;
        let named = named.get_untracked();
        Some(view! {
            <div class="reason-box">
                <label class="reason-label" for=reason_id.get_value()>{mv.asks(&named)}</label>
                <div class="reason-field">
                    <input id=reason_id.get_value() type="text" prop:value=move || note.get()
                        on:input=move |ev| note.set(event_target_value(&ev))
                        on:keydown=move |ev| {
                            if keys::is_send(&ev) {
                                ev.prevent_default();
                                apply();
                            } else if ev.key() == "Escape" {
                                ev.prevent_default();
                                cancel();
                            }
                        }/>
                    <button class="btn primary" on:click=move |_| apply()>{mv.applies(&named)}</button>
                    <button class="btn subtle" on:click=move |_| cancel()>"Cancel"</button>
                </div>
                {move || {
                    // In plain sight, checked by default: the task the move also marks done.
                    let task = paired.get().filter(|p| also_done(mv, Some(*p), true).is_some())?.task;
                    let title = store::task(task).and_then(|t| t.title().ok()).unwrap_or_default();
                    Some(view! {
                        <label class="check">
                            <input type="checkbox" prop:checked=move || done_too.get()
                                on:change=move |ev| done_too.set(event_target_checked(&ev))/>
                            {format!(" Also mark {} “{title}” done", whose_task())}
                        </label>
                    })
                }}
                <span class="keys">{mv.enter()} " · Escape closes the box"</span>
            </div>
        })
    };

    // The box a question is answered in: opened by Reply, by Enter on the row, or with the row;
    // drawn again only when it opens or closes, so it keeps the keyboard while the question changes.
    let asker = Memo::new(move |_| {
        (kind == Kind::Question)
            .then(|| store::message(subject).and_then(|m| m.author().ok()).map(|a| store::handle_of(EntityId::from(a))))
            .flatten()
            .unwrap_or_default()
    });
    let shows_reply = Memo::new(move |_| kind == Kind::Question && (replying.get() || opened.get()));
    let reply_box = move || {
        shows_reply.get().then(|| {
            let field = NodeRef::<leptos::html::Textarea>::new();
            let mentions = Mentions::new(field);
            let on_keydown = move |ev: web_sys::KeyboardEvent| {
                if mentions.key(&ev, &reply.get_untracked(), |t| reply.set(t)) {
                    return;
                }
                if keys::is_send(&ev) {
                    ev.prevent_default();
                    send_reply();
                    return;
                }
                let Some(text_box) = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlTextAreaElement>().ok()) else {
                    return;
                };
                let (first, last) = keys::caret_lines(&text_box);
                match ev.key().as_str() {
                    "Escape" => {
                        ev.prevent_default();
                        if reply.get_untracked().trim().is_empty() {
                            replying.set(false);
                        }
                        back_to_row();
                    }
                    "ArrowUp" if first && keys::is_plain(&ev) => {
                        ev.prevent_default();
                        back_to_row();
                    }
                    // Down on the last line selects the stop below, when there is one.
                    "ArrowDown" if last && keys::is_plain(&ev) && page.step_from(Some(item), 1) => {
                        ev.prevent_default();
                    }
                    _ => {}
                }
            };
            view! {
                <div class="reply-box request-reply">
                    <textarea id=reply_id.get_value() node_ref=field placeholder=move || format!("Your reply to {}", asker.get())
                        prop:value=move || reply.get()
                        on:input=move |ev| {
                            reply.set(event_target_value(&ev));
                            mentions.typed();
                        }
                        on:keyup=move |_| mentions.moved()
                        on:click=move |_| mentions.moved()
                        on:blur=move |_| mentions.left()
                        on:keydown=on_keydown></textarea>
                    <MentionList mentions=mentions text=reply.into() write=Callback::new(move |t| reply.set(t))/>
                    <div class="actions">
                        <button class="btn primary" on:click=move |_| send_reply()>"Send"</button>
                        <span class="keys">
                            "Enter sends · Shift+Enter starts a new line · ↑ on the first line goes to the row · Esc leaves the box"
                        </span>
                    </div>
                </div>
            }
        })
    };

    // What the open row holds beneath its lines, mounted only while it is open: the rows beneath
    // its subject, worked from the keyboard as a card's are. Up from the first of them comes back
    // to the row, and Down past the last goes on to the stop below; a reply sent from one of their
    // boxes marks the row's notifications read, as on a card.
    let beneath = move || {
        if !opened.get() {
            return None;
        }
        let key = opened_key(subject);
        let hooks = Hooks {
            entered: Some(Callback::new(move |()| hold())),
            step: Some(Callback::new(move |delta: isize| {
                if delta < 0 {
                    page.select(item);
                    true
                } else {
                    page.step_from(Some(item), 1)
                }
            })),
            sent: acts.then(|| Callback::new(move |()| mark_read(members()))),
            ..Hooks::default()
        };
        let held = match kind {
            Kind::Question => {
                let text = move || {
                    store::message(subject).map(|m| references::content(&ops::text(m.content()).unwrap_or_default()))
                };
                view! {
                    <p class="content">{text}</p>
                    {thread::rows_beneath(page.thread, key, subject, Models::one(Model::Message), hooks)}
                }
                .into_any()
            }
            Kind::Ruling => {
                // The task that shares the row, its comments beneath the ruling's: one of them
                // usually bears on the ruling.
                let on_task = row.with_untracked(|r| r.as_ref().and_then(|r| r.task)).map(|task| {
                    view! {
                        <p class="request-on">"On " {whose_task} " " {lozenges::single(Target::Task(task))}</p>
                        {thread::rows_beneath(page.thread, opened_key(task), task, Models::one(Model::Task), hooks)}
                    }
                });
                view! {
                    {ruling_record(subject)}
                    {thread::rows_beneath(page.thread, key, subject, Models::one(Model::Decision), hooks)}
                    {on_task}
                    <CommentBox about=comment_box::Subject::Decision(subject)/>
                }
                .into_any()
            }
            Kind::Review | Kind::Proposal | Kind::Assignment => view! {
                {task_record(kind, subject)}
                {thread::rows_beneath(page.thread, key, subject, Models::one(Model::Task), hooks)}
                <CommentBox about=comment_box::Subject::Task(subject)/>
            }
            .into_any(),
        };
        Some(view! { <div class="request-beneath">{held}</div> })
    };
    let paired_task = move || {
        let task = row.with(|r| r.as_ref().and_then(|r| r.task))?;
        Some(view! { {whose_task} " " {lozenges::single(Target::Task(task))} " · " })
    };
    // How many comments joined the row, and the newest one's author and first line beneath its
    // lines, else what its notification says.
    let comments = Memo::new(move |_| {
        row.with(|r| r.as_ref().map(|r| (r.comments.clone(), r.comments_unread)).unwrap_or_default())
    });
    let comment_count = move || {
        let (said, fresh) = comments.with(|(all, new)| (comment_words(all.len(), *new), *new > 0));
        said.map(|said| view! { <span class="request-comments" class:new=fresh>{said}</span> " · " })
    };
    let newest_comment = move || {
        let newest = comments.with(|(all, _)| all.first().copied())?;
        let n = page.notifications.get(newest)?;
        let originator: Value = n.originator().ok()?.into();
        let line = match id_at(&originator["message_id"]).and_then(store::message) {
            Some(m) => {
                let author = m.author().ok().map(|a| store::handle_of(EntityId::from(a))).unwrap_or_default();
                let text = first_line(&references::plain(&ops::text(m.content()).unwrap_or_default()), 200);
                view! { <span class="who">{author}</span> " " {text} }.into_any()
            }
            None => notification_card::sentence(page.whose, &n).into_any(),
        };
        Some(view! { <div class="request-comment">{line}</div> })
    };
    // A row kept after its move says that it leaves when the selection moves on.
    let kept = move || {
        (!waits.get()).then(|| view! { " · " <span class="muted">"leaves Waiting on you when you move on"</span> })
    };

    view! {
        <div class="request-row" id=row_id.get_value() tabindex="-1" class:unread=move || unread.get()
            class:selected=move || selected.get() class:settled=move || !waits.get()
            aria-expanded=move || opened.get().to_string()
            on:keydown=on_key on:click=on_click on:focusin=move |_| hold()>
            <span class="caret" title="open or close what it holds" on:click=move |_| set_open(!opened.get_untracked())>
                {move || if opened.get() { "▾" } else { "▸" }}
            </span>
            <div class="request-main">
                <div class="request-title" title=hover>{title(kind, subject)}</div>
                <div class="request-meta">
                    {meta(kind, subject, sender, at, view! { {paired_task}{comment_count} }.into_any())}{kept}
                </div>
                {newest_comment}
                {reason_box}
                {beneath}
                {reply_box}
                {move || error.get().map(|e| view! { <p class="error">{e}</p> })}
            </div>
            <div class="request-moves">
                {moves}
                <button class="xbtn" aria-label="Dismiss" title=dismiss_says disabled=!acts on:click=dismiss>"×"</button>
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> EntityId {
        EntityId::from_bytes([n; 32])
    }

    const ME: u8 = 1;

    fn handle(p: EntityId) -> String {
        if p == id(7) {
            "lead-fwd".into()
        } else {
            "someone".into()
        }
    }

    /// What a row of `kind` offers `me` about a subject in `facts`: each move, and whether it is
    /// refused.
    fn offered(kind: Kind, facts: MoveFacts) -> Vec<(Move, Option<String>)> {
        offers(kind, &facts, id(ME), handle).into_iter().map(|o| (o.mv, o.refused)).collect()
    }

    fn task(status: TaskStatus, needs_review: bool, assignee: Option<u8>, submitted_by: Option<u8>) -> MoveFacts {
        MoveFacts::Task { status, needs_review, assignee: assignee.map(id), submitted_by: submitted_by.map(id) }
    }

    #[test]
    fn a_question_offers_reply_and_resolve_until_it_is_resolved() {
        assert_eq!(
            offered(Kind::Question, MoveFacts::Question { open: true }),
            vec![(Move::Reply, None), (Move::Resolve, None)]
        );
        assert!(offered(Kind::Question, MoveFacts::Question { open: false }).is_empty());
    }

    #[test]
    fn a_proposed_ruling_offers_accept_and_reject_and_each_asks_why() {
        let proposed = MoveFacts::Ruling { status: DecisionStatus::Proposed };
        assert_eq!(offered(Kind::Ruling, proposed), vec![(Move::AcceptRuling, None), (Move::RejectRuling, None)]);
        for status in [DecisionStatus::Accepted, DecisionStatus::Rejected, DecisionStatus::Superseded] {
            assert!(offered(Kind::Ruling, MoveFacts::Ruling { status }).is_empty(), "{status}: no row move");
        }
        assert_eq!(
            Move::AcceptRuling.asks("D204").as_deref(),
            Some("Why you accept it. The reason is kept as a comment on D204.")
        );
        assert_eq!(Move::AcceptRuling.applies("D204"), "Accept D204");
        assert!(Move::RejectRuling.asks("D204").is_some());
    }

    #[test]
    fn a_review_offers_accept_and_return_and_refuses_the_submitter_its_own_acceptance() {
        let in_review = |submitter| task(TaskStatus::InReview, true, Some(9), submitter);
        assert_eq!(offered(Kind::Review, in_review(Some(9))), vec![(Move::AcceptReview, None), (Move::Return, None)]);
        let own = offered(Kind::Review, in_review(Some(ME)));
        assert_eq!(own[0], (Move::AcceptReview, Some("You submitted it: someone else accepts it".into())));
        assert_eq!(own[1], (Move::Return, None));
        assert!(Move::AcceptReview.asks("").is_none(), "accepting needs no note");
        assert!(Move::Return.asks("").is_some(), "returning asks what should change");
        assert!(offered(Kind::Review, task(TaskStatus::Done, true, Some(9), Some(9))).is_empty(), "accepted");
    }

    #[test]
    fn a_proposed_task_offers_approve_and_reject_which_asks_why() {
        assert_eq!(
            offered(Kind::Proposal, task(TaskStatus::Proposed, false, None, None)),
            vec![(Move::Approve, None), (Move::RejectTask, None)]
        );
        assert!(Move::Approve.asks("").is_none());
        assert!(Move::RejectTask.asks("").is_some());
        assert!(offered(Kind::Proposal, task(TaskStatus::Open, false, None, None)).is_empty(), "approved");
    }

    #[test]
    fn an_assigned_task_offers_start_and_mark_done_where_the_rules_let_them_through() {
        assert_eq!(
            offered(Kind::Assignment, task(TaskStatus::Open, false, Some(ME), None)),
            vec![(Move::Start, None), (Move::Done, None)]
        );
        // A task needing review is done through its review, never marked done.
        assert_eq!(offered(Kind::Assignment, task(TaskStatus::Open, true, Some(ME), None)), vec![(Move::Start, None)]);
        // Read on lead-fwd's inbox: the task is theirs, which only they start; marking it done is
        // the acting principal's to do.
        assert_eq!(
            offered(Kind::Assignment, task(TaskStatus::Open, false, Some(7), None)),
            vec![(Move::Start, Some("Assigned to lead-fwd: only its assignee starts it".into())), (Move::Done, None)]
        );
        assert!(offered(Kind::Assignment, task(TaskStatus::InProgress, true, Some(ME), None)).is_empty(), "started");
    }

    #[test]
    fn a_row_offers_nothing_for_what_the_page_does_not_hold_or_of_another_kind() {
        for kind in Kind::ALL {
            assert!(offered(kind, MoveFacts::Absent).is_empty(), "{kind:?}");
        }
        assert!(offered(Kind::Ruling, MoveFacts::Question { open: true }).is_empty());
    }

    #[test]
    fn every_offered_move_is_one_its_operation_lets_through() {
        // The transition table and the decision rules, which `make` runs through ops, allow each
        // move a row offers from the status it offers it in.
        let statuses = [
            TaskStatus::Proposed,
            TaskStatus::Open,
            TaskStatus::InProgress,
            TaskStatus::InReview,
            TaskStatus::Done,
            TaskStatus::Cancelled,
            TaskStatus::Rejected,
        ];
        for kind in [Kind::Review, Kind::Proposal, Kind::Assignment] {
            for status in statuses {
                for needs_review in [false, true] {
                    for (mv, refused) in offered(kind, task(status, needs_review, Some(ME), Some(9))) {
                        let verb = match mv {
                            Move::AcceptReview => Verb::Accept,
                            Move::Return => Verb::Return,
                            Move::Approve => Verb::Approve,
                            Move::RejectTask => Verb::Reject,
                            Move::Start => Verb::Start,
                            Move::Done => Verb::Done,
                            other => panic!("{other:?} offered on a task"),
                        };
                        assert!(refused.is_none(), "{mv:?} on a {status} task");
                        assert!(rules::transition(status, verb, needs_review).is_ok(), "{mv:?} from {status}");
                    }
                }
            }
        }
    }

    #[test]
    fn ruling_on_a_paired_ruling_also_marks_its_task_done_where_the_box_is_checked_and_it_needs_no_review() {
        let paired = |status, needs_review| Some(Paired { task: id(20), status, needs_review });
        let open = paired(TaskStatus::Open, false);
        assert_eq!(also_done(Move::AcceptRuling, open, true), Some(id(20)));
        assert_eq!(also_done(Move::RejectRuling, open, true), Some(id(20)), "rejecting is ruling too");
        assert_eq!(also_done(Move::AcceptRuling, open, false), None, "the box unchecked");
        assert_eq!(also_done(Move::AcceptRuling, None, true), None, "no task shares the row");
        assert_eq!(also_done(Move::Reply, open, true), None, "a move that rules on nothing");
        assert_eq!(also_done(Move::AcceptRuling, paired(TaskStatus::InProgress, false), true), Some(id(20)));
        assert_eq!(also_done(Move::AcceptRuling, paired(TaskStatus::Open, true), true), None, "it needs review");
        assert_eq!(also_done(Move::AcceptRuling, paired(TaskStatus::Done, false), true), None, "done already");
    }

    #[test]
    fn a_move_marks_read_the_requests_and_only_the_comments_the_row_showed() {
        let row = Row {
            kind: Kind::Assignment,
            subject: id(20),
            // Three comments, the newest first among the row's notifications, and two requests.
            members: vec![id(5), id(4), id(3), id(2), id(1)],
            comments: vec![id(5), id(4), id(3)],
            comments_unread: 3,
            task: None,
            unread: true,
            waits: true,
            sender: None,
            scope: None,
            ready: true,
            priority: None,
            first: 0,
            newest: 0,
        };
        assert_eq!(shown_to_reader(&row, false), vec![id(5), id(2), id(1)], "at rest the row shows the newest");
        assert_eq!(shown_to_reader(&row, true), row.members, "open, it shows every one");
    }

    #[test]
    fn a_row_says_how_many_comments_joined_it_and_how_many_are_new() {
        assert_eq!(comment_words(0, 0), None);
        assert_eq!(comment_words(1, 1).as_deref(), Some("1 comment, new"));
        assert_eq!(comment_words(1, 0).as_deref(), Some("1 comment"));
        assert_eq!(comment_words(3, 1).as_deref(), Some("3 comments, 1 new"));
        assert_eq!(comment_words(3, 0).as_deref(), Some("3 comments"));
    }

    #[test]
    fn what_an_open_row_holds_is_keyed_apart_from_any_card() {
        let task = id(20);
        assert_ne!(opened_key(task), task, "a card about the task keys its rows by the task");
        assert_eq!(opened_key(task), opened_key(task), "the same key each time the row opens");
        assert_eq!(opened_key(opened_key(task)), task);
    }

    #[test]
    fn the_first_line_is_the_first_that_says_something() {
        assert_eq!(
            first_line("\n  \nShould the graph draw cancelled tasks?\nMore.", 100),
            "Should the graph draw cancelled tasks?"
        );
        assert_eq!(first_line("abcdef", 3), "abc…");
        assert_eq!(first_line("", 3), "");
    }
}
