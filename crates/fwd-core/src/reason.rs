//! Why a notification is in someone's inbox, said in one sentence: "triage-bot replied to your
//! comment on Connector", "claude submitted Ankurah models for your review; you created it". A
//! person reading an inbox needs to see at once whether a notification is theirs to act on and
//! what it is about, and a bare type and id ("replied", "message 6Xxz72ol") said neither, as
//! Daniel's inbox showed on 2026-09-26 ("other cases where there's insufficient context"). The
//! sentence is built from the notification's type, the reason routing recorded (`originator.why`,
//! `routing::Why`), the actor's handle and the names of the things it is about. A notification
//! from before routing recorded its reason gets the best sentence its type allows. A batch of what
//! one author did on one task says how many of what, whatever its type: "impl-x added 3 steps and
//! commented twice on your task Review: the retry loop" (`routing::Batch`), and of the threads
//! open on a step's lines as routing reached it, "dnorman added a step with a comment to your task
//! Review: the retry loop"; beneath the sentence the inbox card and `fwd inbox` list what it holds
//! in one order ([`batch_entries`]).
//!
//! Things are named, never shown as bare ids: a task by its title, a decision as `D14 (title)`, a
//! message by its task (or decision, scope, code) when it has one, else by the first words of its
//! thread's first message. The names come from a [`Names`] lookup, so the CLI builds the sentence
//! from what it fetched ([`gather`]) and the page from its standing queries, and both say the same.

use crate::model::*;
use crate::routing::{self, Why};
use ankurah::{Context, EntityId};
use anyhow::Result;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};

/// A message named by the first this many words of its thread's first message.
pub const NAME_WORDS: usize = 8;

/// One piece of a sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    Text(String),
    /// A thing the sentence names, which a page links to.
    Name {
        kind: Named,
        id: EntityId,
        text: String,
    },
}

/// What kind of thing a [`Part::Name`] names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Named {
    Task,
    Decision,
    Scope,
    Anchor,
}

/// A sentence as plain text, names as their text.
pub fn plain(parts: &[Part]) -> String {
    parts
        .iter()
        .map(|p| match p {
            Part::Text(t) => t.as_str(),
            Part::Name { text, .. } => text.as_str(),
        })
        .collect()
}

/// What a sentence needs to know about a message.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MessageFacts {
    pub author: Option<EntityId>,
    pub content: String,
    /// The id of the principal it asks; a message from before 2026-09-29 may hold `supervisor`,
    /// the role it asked, which a sentence shows as the word.
    pub to: Option<String>,
    /// A question asked of no one in particular (`ops::ASKS_KEY`).
    pub asks_project: bool,
    /// The scope such a question asks, whose triage agent answers it (`ops::ASKS_SCOPE_KEY`); none
    /// for one asked of the project, and for one from before 2026-09-30.
    pub asks_scope: Option<EntityId>,
    pub reply_to: Option<EntityId>,
    /// The thread's first message, for a reply.
    pub thread: Option<EntityId>,
    pub task: Option<EntityId>,
    pub decision: Option<EntityId>,
    pub scope: Option<EntityId>,
    pub anchor: Option<EntityId>,
}

impl MessageFacts {
    pub fn of(m: &MessageView) -> MessageFacts {
        let context: Value = m.context().map(Into::into).unwrap_or(Value::Null);
        MessageFacts {
            author: m.author().ok().map(EntityId::from),
            content: crate::ops::text(m.content()).unwrap_or_default(),
            to: context.get("to").and_then(Value::as_str).map(str::to_string),
            asks_project: context.get(crate::ops::ASKS_KEY).and_then(Value::as_str) == Some(crate::ops::ASKS_PROJECT),
            asks_scope: id_at(&context, crate::ops::ASKS_SCOPE_KEY),
            reply_to: m.reply_to().ok().flatten().map(EntityId::from),
            thread: m.thread().ok().flatten().map(EntityId::from),
            task: m.task().ok().flatten().map(EntityId::from),
            decision: m.decision().ok().flatten().map(EntityId::from),
            scope: m.scope().ok().flatten().map(EntityId::from),
            anchor: m.anchor().ok().flatten().map(EntityId::from),
        }
    }

    /// The thread's first message: this one's `thread`, else the message it answers, else itself.
    fn root(&self, own: EntityId) -> EntityId {
        self.thread.or(self.reply_to).unwrap_or(own)
    }
}

/// What a sentence needs to know about a relation (a proposed one is named in its notification).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelationFacts {
    pub kind: RelationKind,
    pub from: EntityId,
    pub to: EntityId,
}

/// Where a sentence finds the names of what it mentions. Each answers `None` for what it does not
/// hold (not loaded yet, or of another kind).
pub trait Names {
    fn handle(&self, principal: EntityId) -> Option<String>;
    fn task_title(&self, id: EntityId) -> Option<String>;
    /// A decision's number and title.
    fn decision(&self, id: EntityId) -> Option<(i64, String)>;
    fn scope_name(&self, id: EntityId) -> Option<String>;
    /// An anchor's path, and its lines as stored (0-based, the end exclusive), or none for an
    /// anchor on the whole file (`ops::lines_of`).
    fn anchor(&self, id: EntityId) -> Option<(String, Option<(i64, i64)>)>;
    fn message(&self, id: EntityId) -> Option<MessageFacts>;
    fn relation(&self, id: EntityId) -> Option<RelationFacts>;
}

/// How long ago the time `ms` (milliseconds since the epoch) was: "just now", "5m ago", "3h ago",
/// "2d ago".
pub fn ago(ms: i64) -> String {
    let s = (crate::now_ms() - ms).max(0) / 1000;
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86_399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86_400),
    }
}

/// The first `n` words of `text`, with an ellipsis when there were more.
pub fn first_words(text: &str, n: usize) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut out = words.iter().take(n).copied().collect::<Vec<_>>().join(" ");
    if words.len() > n {
        out.push('…');
    }
    out
}

fn id_at(v: &Value, key: &str) -> Option<EntityId> {
    v.get(key).and_then(Value::as_str).and_then(|s| s.parse().ok())
}

/// The sentence saying why the notification of type `t`, about `target` and caused as
/// `originator` says, is in `recipient`'s inbox. Given the notification's id, a sentence about a
/// message that the recipient answers inside the message's thread ends with how to answer it there
/// ([`answered_in_its_thread`]): "; reply with fwd reply 8z_sQwsR", the id's first eight
/// characters; without the id it says only why, as the page reads the sentence aloud and as a
/// reader other than the recipient reads it ([`reply_id`]).
pub fn sentence(
    names: &dyn Names,
    recipient: EntityId,
    notification: Option<EntityId>,
    t: NotificationType,
    target: &Value,
    originator: &Value,
) -> Vec<Part> {
    let mut s = Builder { names, me: recipient, parts: Vec::new() };
    let why = routing::why_of(originator);
    let actor = id_at(originator, "actor_id");
    let about = id_at(target, "id");
    let message_id = id_at(originator, "message_id");
    let message = message_id.and_then(|m| names.message(m).map(|f| (m, f)));
    // Several items collected say how many of what; one says what its type says.
    if let Some(batch) = routing::batch_of(originator).filter(|b| b.items.len() > 1) {
        s.batch(actor, why, originator, &batch);
        s.succeeded(originator);
        s.reply_with(notification, message_id, t);
        return s.parts;
    }
    use NotificationType::*;
    match t {
        QuestionAsked => s.asked(actor, why, about.and_then(|m| names.message(m).map(|f| (m, f)))),
        Replied => s.replied(actor, why, about.and_then(|m| names.message(m).map(|f| (m, f)))),
        Mentioned => {
            s.actor(actor);
            match about.and_then(|m| names.message(m).map(|f| (m, f))) {
                Some((id, m)) => {
                    s.text(&format!(" mentioned you in a {}", noun(&m)));
                    s.place(id, &m, " on ", true);
                }
                None => s.text(" mentioned you"),
            }
        }
        TaskCommented => {
            s.actor(actor);
            // A question about a task reached the task's people as a comment on it until
            // 2026-09-29, when routing began to give a question to its addressee alone; such a
            // notification says whom the question asks.
            let asks = message.as_ref().and_then(|(_, m)| match (&m.to, m.asks_project) {
                (Some(to), _) => Some(s.addressee(to)),
                (None, true) => Some("the project".to_string()),
                (None, false) => None,
            });
            let verb = match (&asks, message.as_ref().is_some_and(|(_, m)| m.reply_to.is_some())) {
                (Some(whom), _) => format!(" asked {whom} about "),
                (None, true) => " replied on ".to_string(),
                (None, false) => " commented on ".to_string(),
            };
            let verb = verb.as_str();
            match (target.get("type").and_then(Value::as_str), about) {
                // A comment on a scope reaches the project's triage agent; until 2026-09-29 it
                // reached the scope's owner, whose scope it was (D108).
                (Some("Scope"), Some(scope)) => {
                    s.text(verb);
                    s.text(if why == Some(Why::ScopeOwner) { "your scope " } else { "the scope " });
                    s.scope(scope);
                }
                (_, Some(task)) => {
                    s.text(verb);
                    s.task(task);
                    match why {
                        Some(Why::Assignee) => s.text(", which is assigned to you"),
                        Some(Why::ScopeOwner) => s.text(", which your scope holds"),
                        _ => {}
                    }
                }
                _ => s.text(" commented"),
            }
        }
        CodeCommented => {
            s.actor(actor);
            s.text(" commented on ");
            match about {
                Some(anchor) => s.anchor(anchor),
                None => s.text("code"),
            }
            match why {
                // A highlight on the recipient's checklist: say whose step it is.
                Some(Why::StepParty) => match id_at(originator, "step_task_id") {
                    Some(task) => {
                        s.text(", a step of your task ");
                        s.task(task);
                    }
                    None => s.text(", a step of a task of yours"),
                },
                // Near a task of the recipient's: one assigned to them, or until 2026-09-29 one
                // worked in a scope they owned (D108).
                Some(Why::Assignee) => match id_at(originator, "near_task_id") {
                    Some(task) => {
                        s.text(", near ");
                        s.task(task);
                        s.text(", which is assigned to you");
                    }
                    None => s.text(", near a task assigned to you"),
                },
                Some(Why::ScopeOwner) => match id_at(originator, "near_task_id") {
                    Some(task) => {
                        s.text(", near ");
                        s.task(task);
                        s.text(", which your scope holds");
                    }
                    None => s.text(", near a task your scope holds"),
                },
                _ => {}
            }
        }
        TaskProposed => match (target.get("type").and_then(Value::as_str), about) {
            (Some("Relation"), Some(rel)) => {
                s.actor(actor);
                match names.relation(rel) {
                    Some(r) => {
                        s.text(" proposed that ");
                        s.anything(r.from);
                        s.text(&format!(" {} ", r.kind.to_string().replace('_', " ")));
                        s.anything(r.to);
                    }
                    None => s.text(" proposed a relation"),
                }
            }
            (_, Some(task)) => {
                s.actor(actor);
                s.text(" proposed ");
                s.task(task);
            }
            _ => {
                s.actor(actor);
                s.text(" proposed a task");
            }
        },
        TaskCreated => {
            s.actor(actor);
            let from = message.as_ref().map(|(_, m)| noun(m)).unwrap_or("message");
            s.text(&format!(" filed a task from your {from}: "));
            s.task_or(about, "a task");
        }
        TaskUnblocked => {
            s.task_or(about, "a task");
            match why {
                Some(Why::Assignee) => s.text(", which is assigned to you, is ready"),
                Some(Why::ScopeOwner) => s.text(", which your scope holds, is ready"),
                Some(Why::Triage) => s.text(" is ready and unassigned"),
                _ => s.text(" is ready"),
            }
            if let Some(blocker) = id_at(originator, "task_id") {
                s.text(": ");
                s.task(blocker);
                s.text(match originator.get("to").and_then(Value::as_str) {
                    Some("cancelled") => " was cancelled",
                    Some("rejected") => " was rejected",
                    _ => " is done",
                });
            }
        }
        ReviewRequested => {
            s.actor(actor);
            s.text(" submitted ");
            s.task_or(about, "a task");
            match (why, id_at(originator, "triage_scope_id")) {
                // The triage agent of one of the task's scopes, which routing names, or the one that
                // serves the whole project (docs/DECISIONS.md, D122); until 2026-09-30 the
                // project's one triage agent, whose notifications name no scope. The sentence
                // says why the review reached it, not that the review is its own: a triage agent
                // judges no evidence, and brings the review to its lead unless a person's words
                // or a ruling settle it (D129).
                (Some(Why::Triage), Some(scope)) => {
                    s.text(" for review; you triage the scope ");
                    s.scope(scope);
                }
                (Some(Why::Triage), None) => s.text(" for review; you triage the project"),
                (Some(Why::Creator), _) => s.text(" for your review; you created it"),
                (Some(Why::Approver), _) => s.text(" for your review; you approved it"),
                _ => s.text(" for your review"),
            }
        }
        TaskAssigned => {
            s.task_or(about, "a task");
            if why == Some(Why::Assignee) {
                s.text(" was assigned to you");
            } else {
                // Until 2026-09-28 a task was assigned to a scope, named in the notification.
                s.text(" was assigned to your scope");
                let scope = id_at(originator, "scope_id")
                    .or_else(|| id_at(originator, "relation_id").and_then(|r| names.relation(r)).map(|r| r.to));
                if let Some(scope) = scope {
                    s.text(" ");
                    s.scope(scope);
                }
            }
            if actor.is_some() {
                s.text(" by ");
                s.actor(actor);
            }
        }
        Digest => s.text("several changes, collected"),
        StepAdded => {
            s.actor(actor);
            s.text(" added a step to ");
            match about {
                Some(task) => s.task_of(why, originator, task),
                None => s.text("a task"),
            }
        }
        DecisionProposed => {
            s.actor(actor);
            s.text(" proposed ");
            s.decision_or(about);
        }
        DecisionCommented => {
            s.actor(actor);
            let reply = message.as_ref().is_some_and(|(_, m)| m.reply_to.is_some());
            s.text(if reply { " replied on " } else { " commented on " });
            s.decision_or(about);
            match why {
                Some(Why::DecisionAuthor) => s.text(", which you filed"),
                Some(Why::Decider) => s.text(", which you ruled on"),
                _ => {}
            }
        }
        DecisionRepealed | DecisionSuperseded => {
            s.decision_or(about);
            s.text(if t == DecisionRepealed { " was repealed" } else { " was superseded" });
            if actor.is_some() {
                s.text(" by ");
                s.actor(actor);
            }
            match id_at(originator, "citing_id") {
                Some(citing) => {
                    s.text("; your ");
                    s.citing(citing);
                    s.text(" cites it");
                }
                None => s.text("; something of yours cites it"),
            }
        }
    }
    s.succeeded(originator);
    s.reply_with(notification, message_id, t);
    s.parts
}

/// Whether a notification of type `t` with a message behind it is about a message that its
/// recipient answers inside the message's thread, so that its sentence ends with how: `fwd reply`
/// given the notification's id posts the answer inside that thread and settles the notification,
/// where `fwd comment` starts a thread of its own. An implementer in Koinon XMS that was mentioned
/// found nothing naming `fwd reply` in the notification or in `fwd comment --help`, and answered
/// with a comment on the task (task 2guE1WsN). So a question asked of the recipient, a reply, a
/// mention, a comment on their task, on a scope they triage (which routing types as a comment on a
/// task), on code or on a decision, and a step added to their task with the one thread already on
/// its lines, which routing names as its message only when it lists one
/// (`routing::Batch::message_behind`); a task filed from their message is no such message, and a
/// digest holds several comments, of which a reply given its id answers only the latest.
fn answered_in_its_thread(t: NotificationType) -> bool {
    use NotificationType::*;
    matches!(t, QuestionAsked | Replied | Mentioned | TaskCommented | CodeCommented | DecisionCommented | StepAdded)
}

/// The id a notification's sentence ends with ([`sentence`]): the notification's, when `reader`,
/// the principal the sentence is shown to, is its `recipient`, and none for anyone else or for a
/// command that acts as no one. `fwd reply` given a notification settles it, which is its
/// recipient's to do, so a reader of another principal's inbox (`fwd inbox --for`, the page's cards
/// on another's inbox) is not told to reply through a notification that is not theirs; they answer
/// the message by its own id.
pub fn reply_id(notification: EntityId, recipient: EntityId, reader: Option<EntityId>) -> Option<EntityId> {
    (reader == Some(recipient)).then_some(notification)
}

/// A message's kind of words, for "your question", "a reply": what asks someone, or the project,
/// is a question, what answers a message is a reply, anything else a comment.
fn noun(m: &MessageFacts) -> &'static str {
    if m.to.is_some() || m.asks_project {
        "question"
    } else if m.reply_to.is_some() {
        "reply"
    } else {
        "comment"
    }
}

struct Builder<'a> {
    names: &'a dyn Names,
    me: EntityId,
    parts: Vec<Part>,
}

impl Builder<'_> {
    fn text(&mut self, t: &str) {
        match self.parts.last_mut() {
            Some(Part::Text(last)) => last.push_str(t),
            _ => self.parts.push(Part::Text(t.to_string())),
        }
    }

    fn name(&mut self, kind: Named, id: EntityId, text: String) {
        self.parts.push(Part::Name { kind, id, text });
    }

    /// Whom the notification was for, when routing gave it to the reader as that principal's
    /// successor (`routing::Successors`; docs/DECISIONS.md, D114), so that the sentence's "you" is
    /// read as the job the reader carries on: " (for @triage-1, whom you succeeded)".
    fn succeeded(&mut self, originator: &Value) {
        if let Some(old) = id_at(originator, "for_id").filter(|old| *old != self.me) {
            let who = self.names.handle(old).map(|h| format!("@{h}")).unwrap_or_else(|| "someone".into());
            self.text(&format!(" (for {who}, whom you succeeded)"));
        }
    }

    /// How to answer the message behind a notification of type `t` inside its thread, given the
    /// notification's id ([`answered_in_its_thread`]): "; reply with fwd reply 8z_sQwsR".
    fn reply_with(&mut self, notification: Option<EntityId>, message: Option<EntityId>, t: NotificationType) {
        if let Some(notification) = notification.filter(|_| message.is_some() && answered_in_its_thread(t)) {
            let short: String = notification.to_base64().chars().take(8).collect();
            self.text(&format!("; reply with fwd reply {short}"));
        }
    }

    /// Who acted: their handle, "you", or "someone" when the notification does not say.
    fn actor(&mut self, actor: Option<EntityId>) {
        let who = match actor {
            Some(a) if a == self.me => "you".to_string(),
            Some(a) => self.names.handle(a).unwrap_or_else(|| "someone".into()),
            None => "someone".into(),
        };
        self.text(&who);
    }

    /// Whom a message's `to` names, seen by the reader: "you", or a handle (a `to` that is no id,
    /// a handle or the old role word, as written).
    fn addressee(&self, to: &str) -> String {
        match to.parse::<EntityId>() {
            Ok(p) if p == self.me => "you".into(),
            Ok(p) => self.names.handle(p).unwrap_or_else(|| "someone".into()),
            Err(_) => to.trim_start_matches('@').to_string(),
        }
    }

    /// Whose a message is, seen by the reader: "your", the actor's "their", or "dnorman's".
    fn possessive(&mut self, author: Option<EntityId>, actor: Option<EntityId>) {
        let whose = match author {
            Some(a) if a == self.me => "your".to_string(),
            Some(a) if Some(a) == actor => "their".to_string(),
            Some(a) => format!("{}'s", self.names.handle(a).unwrap_or_else(|| "someone".into())),
            None => "a".to_string(),
        };
        self.text(&whose);
    }

    fn task(&mut self, id: EntityId) {
        let text = self.names.task_title(id).unwrap_or_else(|| "a task".into());
        self.name(Named::Task, id, text);
    }

    fn task_or(&mut self, id: Option<EntityId>, missing: &str) {
        match id {
            Some(id) => self.task(id),
            None => self.text(missing),
        }
    }

    /// A task that steps were added to or comments written on, as the recipient reads it: "your
    /// task" when it is theirs (they are its assignee, or a party to its checklist); for the
    /// assignee of a task anchored near the code commented on (until 2026-09-29, the owner of the
    /// scope that task was worked in: D108), with the task of theirs it is near; else by its title.
    fn task_of(&mut self, why: Option<Why>, originator: &Value, task: EntityId) {
        match (why, id_at(originator, "near_task_id")) {
            (Some(Why::Assignee), Some(near)) => {
                self.task(task);
                self.text(", near ");
                self.task(near);
                self.text(", which is assigned to you");
            }
            (Some(Why::Assignee | Why::StepParty), _) => {
                self.text("your task ");
                self.task(task);
            }
            (Some(Why::ScopeOwner), near) => {
                self.task(task);
                match near {
                    Some(near) => {
                        self.text(", near ");
                        self.task(near);
                        self.text(", which your scope holds");
                    }
                    None => self.text(", near a task your scope holds"),
                }
            }
            _ => self.task(task),
        }
    }

    /// What one author did on one task, collected in one notification (rule 10 of
    /// docs/design/agents-and-inbox.md), as how many of what: "impl-x added 3 steps and commented
    /// twice on your task Review: the retry loop", "impl-x added 2 steps to your task …",
    /// "impl-x commented 3 times on …", and the threads its steps came with ([`Self::threads`]):
    /// "dnorman added a step with a comment to your task …".
    fn batch(&mut self, actor: Option<EntityId>, why: Option<Why>, originator: &Value, batch: &routing::Batch) {
        self.actor(actor);
        let (steps, comments) = (batch.steps(), batch.comments());
        match steps {
            0 => {}
            1 => self.text(" added a step"),
            n => self.text(&format!(" added {n} steps")),
        }
        if steps > 0 {
            self.threads(batch);
        }
        if comments > 0 {
            self.text(if steps > 0 { " and commented" } else { " commented" });
            match comments {
                1 => {}
                2 => self.text(" twice"),
                n => self.text(&format!(" {n} times")),
            }
        }
        self.text(if comments > 0 { " on " } else { " to " });
        self.task_of(why, originator, batch.task);
    }

    /// The threads a batch's steps came with, the comments and questions open on their lines as
    /// routing reached them (`routing::BatchItem::Thread`), whoever wrote them, as how many of each:
    /// " with a comment", " with 2 comments and a question"; nothing when there are none. A message
    /// whose words are not known yet counts as a comment until they are.
    fn threads(&mut self, batch: &routing::Batch) {
        let (mut comments, mut questions) = (0, 0);
        for message in batch.threads() {
            match self.names.message(message) {
                Some(m) if noun(&m) == "question" => questions += 1,
                _ => comments += 1,
            }
        }
        let counted = |n: usize, noun: &str| match n {
            1 => format!("a {noun}"),
            n => format!("{n} {noun}s"),
        };
        let said: Vec<String> = [(comments, "comment"), (questions, "question")]
            .into_iter()
            .filter(|(n, _)| *n > 0)
            .map(|(n, noun)| counted(n, noun))
            .collect();
        if !said.is_empty() {
            self.text(&format!(" with {}", said.join(" and ")));
        }
    }

    fn decision(&mut self, id: EntityId) {
        let text = match self.names.decision(id) {
            Some((number, title)) => format!("D{number} ({title})"),
            None => "a decision".into(),
        };
        self.name(Named::Decision, id, text);
    }

    fn decision_or(&mut self, id: Option<EntityId>) {
        match id {
            Some(id) => self.decision(id),
            None => self.text("a decision"),
        }
    }

    fn scope(&mut self, id: EntityId) {
        let text = self.names.scope_name(id).unwrap_or_else(|| "a scope".into());
        self.name(Named::Scope, id, text);
    }

    /// Code by its path and lines, 1-based as an editor shows them (`ws.rs:5-7`, `ws.rs:5`), and a
    /// whole file by its path alone, since it names no line: where the highlight is, as every
    /// name of one begins (`anchor_name`).
    fn anchor(&mut self, id: EntityId) {
        let text = match self.names.anchor(id) {
            Some((path, stored)) => crate::anchor_name::place(&path, crate::anchor_name::lines(stored)),
            None => "code".into(),
        };
        self.name(Named::Anchor, id, text);
    }

    /// An entity of any kind, for a relation's ends: a task, a decision, a scope, or a message by
    /// its first words.
    fn anything(&mut self, id: EntityId) {
        if self.names.task_title(id).is_some() {
            self.task(id);
        } else if self.names.decision(id).is_some() {
            self.decision(id);
        } else if self.names.scope_name(id).is_some() {
            self.scope(id);
        } else if let Some(m) = self.names.message(id) {
            let words = self.words_of(id, &m);
            self.text(&words);
        } else {
            self.text("something");
        }
    }

    /// What cites a decision, as its author reads it: "task Connector", "comment “…”", "decision
    /// D5 (…)".
    fn citing(&mut self, id: EntityId) {
        if self.names.task_title(id).is_some() {
            self.text("task ");
            self.task(id);
        } else if self.names.decision(id).is_some() {
            self.text("decision ");
            self.decision(id);
        } else if let Some(m) = self.names.message(id) {
            self.text(noun(&m));
            self.place(id, &m, " on ", true);
        } else {
            self.text("work");
        }
    }

    /// The first words of the thread a message is in, quoted.
    fn words_of(&self, id: EntityId, m: &MessageFacts) -> String {
        let root = m.root(id);
        let content = if root == id { Some(m.content.clone()) } else { self.names.message(root).map(|r| r.content) };
        let words = first_words(&self.readable(content.as_deref().unwrap_or(&m.content)), NAME_WORDS);
        if words.is_empty() {
            "a message".into()
        } else {
            format!("“{words}”")
        }
    }

    /// A message's text with its inline references (`[[<id>]]`) named, never shown as ids: a person
    /// as `@handle`, a task by its title, a decision by its number, a whole file the text names at
    /// a version by its path and that version (`docs/cli.md on disk`; `file_mention.rs`), and
    /// anything else as `…`.
    fn readable(&self, text: &str) -> String {
        crate::references::render_named(text, |named| {
            let id = named.id;
            let file = named.version.as_ref().and_then(|version| {
                let (path, _) = self.names.anchor(id)?;
                Some(crate::anchor_name::AnchorName::file_at(&path, version).text())
            });
            file.or_else(|| self.names.handle(id).map(|h| format!("@{h}")))
                .or_else(|| self.names.task_title(id))
                .or_else(|| self.names.decision(id).map(|(number, _)| format!("D{number}")))
                .or_else(|| Some("…".into()))
        })
    }

    /// Where a message is: `lead` and what it is about (its task, decision, scope or code) when it
    /// has a subject, else its thread's first words, after " in" when `in_thread` and the message
    /// is not the thread's first.
    fn place(&mut self, id: EntityId, m: &MessageFacts, lead: &str, in_thread: bool) {
        if let Some(task) = m.task {
            self.text(lead);
            self.task(task);
        } else if let Some(decision) = m.decision {
            self.text(lead);
            self.decision(decision);
        } else if let Some(scope) = m.scope {
            self.text(lead);
            self.text("scope ");
            self.scope(scope);
        } else if let Some(anchor) = m.anchor {
            self.text(lead);
            self.anchor(anchor);
        } else {
            let words = self.words_of(id, m);
            self.text(if in_thread && m.root(id) != id { " in " } else { " " });
            self.text(&words);
        }
    }

    /// A question or a forward: to the recipient, or to no one in particular, which reaches the
    /// recipient as the triage agent of the scope it asks, named, or of the project. One routed
    /// before 2026-09-29 to a principal that held the supervisor role, for a question or a forward
    /// to the role, records the same reason (`Why::Triage`), and reads as asked of the project too.
    fn asked(&mut self, actor: Option<EntityId>, why: Option<Why>, message: Option<(EntityId, MessageFacts)>) {
        self.actor(actor);
        let Some((id, m)) = message else {
            self.text(" asked you something");
            return;
        };
        let of_no_one = why == Some(Why::Triage) || (why.is_none() && m.asks_project);
        let forwarded = m.reply_to.filter(|_| matches!(why, Some(Why::Forwarded | Why::Triage) | None));
        match forwarded.and_then(|p| self.names.message(p).map(|f| (p, f))) {
            // A forward names what it forwards: whose it was, and where.
            Some((parent, p)) => {
                if of_no_one {
                    self.text(" forwarded to ");
                    self.asked_of(m.asks_scope);
                    self.text(" ");
                } else {
                    self.text(" forwarded you ");
                }
                self.possessive(p.author, actor);
                self.text(&format!(" {}", noun(&p)));
                self.place(parent, &p, " on ", true);
            }
            None => {
                if of_no_one {
                    self.text(" asked ");
                    self.asked_of(m.asks_scope);
                } else {
                    self.text(" asked you");
                }
                self.place(id, &m, " about ", false);
            }
        }
    }

    /// Whom a question asked of no one in particular asks, in the sentence's words: the scope it
    /// asks, named, whose triage agent answers it, or the project.
    fn asked_of(&mut self, scope: Option<EntityId>) {
        match scope {
            Some(scope) => {
                self.text("the scope ");
                self.scope(scope);
            }
            None => self.text("the project"),
        }
    }

    /// A reply, said from where the recipient stands in its thread.
    fn replied(&mut self, actor: Option<EntityId>, why: Option<Why>, reply: Option<(EntityId, MessageFacts)>) {
        self.actor(actor);
        let Some((id, r)) = reply else {
            self.text(" replied");
            return;
        };
        match (why, r.reply_to.and_then(|p| self.names.message(p).map(|f| (p, f)))) {
            (Some(Why::ParentAuthor), Some((parent, p))) => {
                self.text(&format!(" replied to your {}", noun(&p)));
                self.place(parent, &p, " on ", true);
            }
            (Some(Why::ThreadAuthor), _) => {
                self.text(" replied in your thread");
                self.thread(id, &r, " on ", " ");
            }
            (Some(Why::Participant), _) => {
                self.text(" replied");
                self.thread(id, &r, " on ", " in ");
                self.text(", a thread you are in");
            }
            _ => {
                self.text(" replied");
                self.thread(id, &r, " on ", " in ");
            }
        }
    }

    /// The thread a reply is in: `subject_lead` and its subject when it has one, else `words_lead`
    /// and the first words of its first message.
    fn thread(&mut self, id: EntityId, r: &MessageFacts, subject_lead: &str, words_lead: &str) {
        let root = r.root(id);
        let first = if root == id { Some(r.clone()) } else { self.names.message(root) };
        let has_subject = |m: &MessageFacts| m.task.or(m.decision).or(m.scope).or(m.anchor).is_some();
        match first {
            Some(first) if has_subject(&first) => self.place(root, &first, subject_lead, false),
            _ if has_subject(r) => self.place(id, r, subject_lead, false),
            _ => {
                let words = self.words_of(id, r);
                self.text(words_lead);
                self.text(&words);
            }
        }
    }
}

/// What the CLI fetched for one sentence: a [`Names`] over maps, which records each name asked for
/// and not held, so [`gather`] knows what to fetch next.
#[derive(Default)]
pub struct Fetched {
    handles: HashMap<EntityId, String>,
    tasks: HashMap<EntityId, String>,
    decisions: HashMap<EntityId, (i64, String)>,
    scopes: HashMap<EntityId, String>,
    anchors: HashMap<EntityId, (String, Option<(i64, i64)>)>,
    messages: HashMap<EntityId, MessageFacts>,
    relations: HashMap<EntityId, RelationFacts>,
    wanted: RefCell<BTreeSet<(Want, EntityId)>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Want {
    Handle,
    Task,
    Decision,
    Scope,
    Anchor,
    Message,
    Relation,
}

impl Fetched {
    fn held<T: Clone>(&self, map: &HashMap<EntityId, T>, want: Want, id: EntityId) -> Option<T> {
        let found = map.get(&id).cloned();
        if found.is_none() {
            self.wanted.borrow_mut().insert((want, id));
        }
        found
    }
}

impl Names for Fetched {
    fn handle(&self, principal: EntityId) -> Option<String> {
        self.held(&self.handles, Want::Handle, principal)
    }
    fn task_title(&self, id: EntityId) -> Option<String> {
        self.held(&self.tasks, Want::Task, id)
    }
    fn decision(&self, id: EntityId) -> Option<(i64, String)> {
        self.held(&self.decisions, Want::Decision, id)
    }
    fn scope_name(&self, id: EntityId) -> Option<String> {
        self.held(&self.scopes, Want::Scope, id)
    }
    fn anchor(&self, id: EntityId) -> Option<(String, Option<(i64, i64)>)> {
        self.held(&self.anchors, Want::Anchor, id)
    }
    fn message(&self, id: EntityId) -> Option<MessageFacts> {
        self.held(&self.messages, Want::Message, id)
    }
    fn relation(&self, id: EntityId) -> Option<RelationFacts> {
        self.held(&self.relations, Want::Relation, id)
    }
}

/// The sentence for the notification `n` as `reader` reads it, ending with how to answer the
/// message behind it when it has one to answer and `reader` is its recipient ([`reply_id`]), with
/// the names fetched from `ctx`: the sentence is built, what it asked for and did not have is
/// fetched, and it is built again, until it asks for nothing new (a message's thread is a second
/// step, its first message's author a third).
pub async fn gather(ctx: &Context, n: &NotificationView, reader: Option<EntityId>) -> Result<Vec<Part>> {
    let recipient: EntityId = n.recipient()?.into();
    let reply = reply_id(n.id(), recipient, reader);
    let t = n.notification_type()?;
    let target: Value = n.target()?.into();
    let originator: Value = n.originator()?.into();
    let mut fetched = Fetched::default();
    let mut tried: BTreeSet<(Want, EntityId)> = BTreeSet::new();
    for _ in 0..5 {
        let parts = sentence(&fetched, recipient, reply, t, &target, &originator);
        let wanted: Vec<(Want, EntityId)> =
            std::mem::take(&mut *fetched.wanted.borrow_mut()).into_iter().filter(|w| tried.insert(*w)).collect();
        if wanted.is_empty() {
            return Ok(parts);
        }
        for (want, id) in wanted {
            fetch(ctx, &mut fetched, want, id).await;
        }
    }
    Ok(sentence(&fetched, recipient, reply, t, &target, &originator))
}

/// Fetches one name into `f`; a name that cannot be read (another kind of entity, or gone) is
/// left out, and the sentence says "a task" or "someone" in its place.
async fn fetch(ctx: &Context, f: &mut Fetched, want: Want, id: EntityId) {
    match want {
        Want::Handle => {
            if let Some(h) = ctx.get::<PrincipalView>(id).await.ok().and_then(|p| p.handle().ok()) {
                f.handles.insert(id, h);
            }
        }
        Want::Task => {
            if let Some(title) = ctx.get::<TaskView>(id).await.ok().and_then(|t| t.title().ok()) {
                f.tasks.insert(id, title);
            }
        }
        Want::Decision => {
            if let Ok(d) = ctx.get::<DecisionView>(id).await {
                f.decisions.insert(id, (d.number().unwrap_or(0), d.title().unwrap_or_default()));
            }
        }
        Want::Scope => {
            if let Some(title) = ctx.get::<ScopeView>(id).await.ok().and_then(|s| s.title().ok()) {
                f.scopes.insert(id, title);
            }
        }
        Want::Anchor => {
            if let Ok(a) = ctx.get::<CodeAnchorView>(id).await {
                if let (Ok(path), Ok(lines)) = (a.path(), crate::ops::lines_of(&a)) {
                    f.anchors.insert(id, (path, lines));
                }
            }
        }
        Want::Message => {
            if let Ok(m) = ctx.get::<MessageView>(id).await {
                f.messages.insert(id, MessageFacts::of(&m));
            }
        }
        Want::Relation => {
            if let Ok(r) = ctx.get::<RelationView>(id).await {
                if let (Ok(kind), Ok(from), Ok(to)) = (r.kind(), r.from_id(), r.to_id()) {
                    f.relations.insert(id, RelationFacts { kind, from, to });
                }
            }
        }
    }
}

/// The batch a notification lists beneath its sentence, on its inbox card and in `fwd inbox`
/// (`routing::Batch`): one of several items, a step with the threads it came with among them, or a
/// step alone. A batch of one comment lists nothing, since that comment is the message behind the
/// notification, which shows as it did before batches existed.
pub fn listed(originator: &Value) -> Option<routing::Batch> {
    routing::batch_of(originator).filter(|b| b.items.len() > 1 || b.comments() == 0)
}

/// One thing a batch holds, by id: a step (what became the step: a highlight, a task, a message)
/// or a comment (its message: a comment of the batch's author, or a thread a step came with).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Held<I> {
    Step(I),
    Comment(I),
}

impl From<&routing::BatchItem> for Held<EntityId> {
    fn from(item: &routing::BatchItem) -> Held<EntityId> {
        match *item {
            routing::BatchItem::Step { step, .. } => Held::Step(step),
            routing::BatchItem::Comment { message, .. } | routing::BatchItem::Thread { message, .. } => {
                Held::Comment(message)
            }
        }
    }
}

/// One entry of the list beneath a batch's sentence: a step, with the batch's comments on its
/// highlight; or a comment on its own, with the highlight it was written on when that is no step
/// of the batch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BatchEntry<I> {
    Step { step: I, comments: Vec<I> },
    Comment { message: I, on: Option<I> },
}

/// What a batch lists beneath its sentence, on its inbox card and in `fwd inbox`, so its reader
/// sees each thing its author did, not only how many: what it holds, oldest first, so that the
/// newest is last, each step in its place with the batch's comments on its highlight, so a step
/// and its first comment, or the threads open on its lines as routing reached it, read as one
/// entry, and every other comment in its own place. `on` answers the highlight a comment was
/// written on, when the reader holds the comment and it has one.
pub fn batch_entries<I: Copy + PartialEq>(held: &[Held<I>], on: impl Fn(I) -> Option<I>) -> Vec<BatchEntry<I>> {
    let steps: Vec<I> = held
        .iter()
        .filter_map(|h| match h {
            Held::Step(step) => Some(*step),
            Held::Comment(_) => None,
        })
        .collect();
    let on_step = |message: I| on(message).filter(|highlight| steps.contains(highlight));
    let mut out: Vec<BatchEntry<I>> = held
        .iter()
        .filter_map(|h| match *h {
            Held::Step(step) => Some(BatchEntry::Step { step, comments: Vec::new() }),
            Held::Comment(message) if on_step(message).is_some() => None,
            Held::Comment(message) => Some(BatchEntry::Comment { message, on: on(message) }),
        })
        .collect();
    for h in held {
        let Held::Comment(message) = *h else { continue };
        let Some(highlight) = on_step(message) else { continue };
        let entry = out.iter_mut().find(|e| matches!(e, BatchEntry::Step { step, .. } if *step == highlight));
        if let Some(BatchEntry::Step { comments, .. }) = entry {
            comments.push(message);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A small graph of names, held in maps.
    #[derive(Default)]
    struct Graph {
        f: Fetched,
    }

    fn id() -> EntityId {
        EntityId::random()
    }

    impl Graph {
        fn person(&mut self, handle: &str) -> EntityId {
            let p = id();
            self.f.handles.insert(p, handle.into());
            p
        }
        fn task(&mut self, title: &str) -> EntityId {
            let t = id();
            self.f.tasks.insert(t, title.into());
            t
        }
        fn scope(&mut self, title: &str) -> EntityId {
            let s = id();
            self.f.scopes.insert(s, title.into());
            s
        }
        fn message(&mut self, m: MessageFacts) -> EntityId {
            let i = id();
            self.f.messages.insert(i, m);
            i
        }
        /// Why a notification is in `me`'s inbox, as the page reads it aloud: built without the
        /// notification's id, the sentence never ends with how to answer.
        fn say(&self, me: EntityId, t: NotificationType, target: Value, originator: Value) -> String {
            plain(&sentence(&self.f, me, None, t, &target, &originator))
        }
        /// The sentence of the notification `n`, as `fwd inbox`, the hooks and the page's lines
        /// give it.
        fn say_of(&self, n: EntityId, me: EntityId, t: NotificationType, target: Value, originator: Value) -> String {
            plain(&sentence(&self.f, me, Some(n), t, &target, &originator))
        }
    }

    fn msg(id: EntityId) -> Value {
        json!({"type": "Message", "id": id.to_base64()})
    }

    fn task_target(id: EntityId) -> Value {
        json!({"type": "Task", "id": id.to_base64()})
    }

    fn cause(actor: EntityId, why: &str) -> Value {
        json!({"actor_id": actor.to_base64(), "why": why})
    }

    #[test]
    fn a_reply_says_whose_message_it_answers_and_where() {
        let mut g = Graph::default();
        let (daniel, triage) = (g.person("dnorman"), g.person("triage-bot"));
        let task = g.task("A restart legible to watchers");
        let comment = g.message(MessageFacts { author: Some(daniel), task: Some(task), ..Default::default() });
        let reply = g.message(MessageFacts {
            author: Some(triage),
            reply_to: Some(comment),
            thread: Some(comment),
            task: Some(task),
            ..Default::default()
        });
        let mut o = cause(triage, "parent_author");
        o["message_id"] = json!(reply.to_base64());
        assert_eq!(
            g.say(daniel, NotificationType::Replied, msg(reply), o),
            "triage-bot replied to your comment on A restart legible to watchers"
        );
    }

    /// A notification about a message that its recipient answers inside the message's thread ends
    /// with how, naming the notification by the first eight characters of its id: a question asked
    /// of them, a reply, a mention, and a comment on their task, on a scope they triage, on code or
    /// on a decision (task 2guE1WsN). Read aloud it does not, nor shown to a reader who is not its
    /// recipient, nor does a task filed from their message, a digest, or a notification with no
    /// message behind it, which `fwd reply` refuses.
    #[test]
    fn a_message_to_answer_ends_with_fwd_reply_and_the_notifications_id() {
        use NotificationType::*;
        let mut g = Graph::default();
        let (daniel, claude) = (g.person("dnorman"), g.person("claude"));
        let task = g.task("Retry on disconnect");
        let comment = g.message(MessageFacts { author: Some(claude), task: Some(task), ..Default::default() });
        let n = id();
        let reply = format!("; reply with fwd reply {}", &n.to_base64()[..8]);
        let mut o = cause(claude, "mentioned");
        o["message_id"] = json!(comment.to_base64());
        assert_eq!(
            g.say_of(n, daniel, Mentioned, msg(comment), o.clone()),
            format!("claude mentioned you in a comment on Retry on disconnect{reply}")
        );
        let on = |kind: &str| json!({"type": kind, "id": id().to_base64()});
        for (t, target) in [
            (QuestionAsked, msg(comment)),
            (Replied, msg(comment)),
            (TaskCommented, task_target(task)),
            (TaskCommented, on("Scope")),
            (CodeCommented, on("CodeAnchor")),
            (DecisionCommented, on("Decision")),
        ] {
            let said = g.say_of(n, daniel, t, target, o.clone());
            assert!(said.ends_with(&reply), "{t}: {said}");
        }

        assert_eq!(
            g.say(daniel, Mentioned, msg(comment), o.clone()),
            "claude mentioned you in a comment on Retry on disconnect"
        );
        // The id goes to its recipient alone: another reader is not told to reply through it.
        assert_eq!(reply_id(n, daniel, Some(daniel)), Some(n));
        assert_eq!(reply_id(n, daniel, Some(claude)), None);
        assert_eq!(reply_id(n, daniel, None), None, "a command that acts as no one");
        assert_eq!(
            g.say_of(n, daniel, TaskCreated, task_target(task), o),
            "claude filed a task from your comment: Retry on disconnect"
        );
        let mut digest = batch(claude, "triage", task, 0, 2);
        digest["message_id"] = json!(comment.to_base64());
        assert_eq!(
            g.say_of(n, daniel, Digest, task_target(task), digest),
            "claude commented twice on Retry on disconnect"
        );
        assert_eq!(
            g.say_of(n, daniel, Mentioned, msg(comment), cause(claude, "mentioned")),
            "claude mentioned you in a comment on Retry on disconnect"
        );
    }

    #[test]
    fn a_file_a_message_names_at_a_version_reads_as_its_path_and_that_version() {
        let mut g = Graph::default();
        let (daniel, claude) = (g.person("dnorman"), g.person("claude"));
        let primer = id();
        g.f.anchors.insert(primer, ("docs/using-fwd.md".into(), None));
        let question = g.message(MessageFacts {
            author: Some(daniel),
            to: Some(claude.to_base64()),
            content: format!("is [[{}@worktree]] the primer?", primer.to_base64()),
            ..Default::default()
        });
        let answer = g.message(MessageFacts {
            author: Some(claude),
            reply_to: Some(question),
            thread: Some(question),
            content: "it is".into(),
            ..Default::default()
        });
        assert_eq!(
            g.say(daniel, NotificationType::Replied, msg(answer), cause(claude, "parent_author")),
            "claude replied to your question “is docs/using-fwd.md on disk the primer?”"
        );
    }

    #[test]
    fn a_message_with_no_subject_is_named_by_its_threads_first_words() {
        let mut g = Graph::default();
        let (daniel, claude) = (g.person("dnorman"), g.person("claude"));
        let question = g.message(MessageFacts {
            author: Some(daniel),
            to: Some(claude.to_base64()),
            content: "does a question asked of myself reach my own inbox, or not?".into(),
            ..Default::default()
        });
        let answer = g.message(MessageFacts {
            author: Some(claude),
            reply_to: Some(question),
            thread: Some(question),
            content: "it does".into(),
            ..Default::default()
        });
        assert_eq!(
            g.say(daniel, NotificationType::Replied, msg(answer), cause(claude, "parent_author")),
            "claude replied to your question “does a question asked of myself reach my…”"
        );
        let follow_up = g.message(MessageFacts {
            author: Some(daniel),
            reply_to: Some(answer),
            thread: Some(question),
            content: "thanks".into(),
            ..Default::default()
        });
        assert_eq!(
            g.say(claude, NotificationType::Replied, msg(follow_up), cause(daniel, "parent_author")),
            "dnorman replied to your reply in “does a question asked of myself reach my…”",
            "a reply that is not the thread's first is placed in its thread"
        );
        let bob = g.person("impl-bob");
        assert_eq!(
            g.say(bob, NotificationType::Replied, msg(follow_up), cause(daniel, "participant")),
            "dnorman replied in “does a question asked of myself reach my…”, a thread you are in"
        );
        assert_eq!(
            g.say(daniel, NotificationType::Replied, msg(follow_up), cause(claude, "thread_author")),
            "claude replied in your thread “does a question asked of myself reach my…”"
        );
    }

    #[test]
    fn a_question_names_its_subject_and_a_forward_whose_question_it_carries() {
        let mut g = Graph::default();
        let (daniel, claude, triage) = (g.person("dnorman"), g.person("claude"), g.person("triage-bot"));
        let task = g.task("Ankurah models in fwd-core");
        let question = g.message(MessageFacts {
            author: Some(claude),
            to: Some(triage.to_base64()),
            task: Some(task),
            ..Default::default()
        });
        assert_eq!(
            g.say(triage, NotificationType::QuestionAsked, msg(question), cause(claude, "addressee")),
            "claude asked you about Ankurah models in fwd-core",
            "the triage agent is asked as anyone is"
        );
        let forward = g.message(MessageFacts {
            author: Some(triage),
            to: Some(daniel.to_base64()),
            reply_to: Some(question),
            thread: Some(question),
            task: Some(task),
            ..Default::default()
        });
        assert_eq!(
            g.say(daniel, NotificationType::QuestionAsked, msg(forward), cause(triage, "forwarded")),
            "triage-bot forwarded you claude's question on Ankurah models in fwd-core"
        );
        // Asked of no one in particular, it reaches the triage agent as a question to the project,
        // and a forward to the project likewise.
        let of_project = g.message(MessageFacts {
            author: Some(claude),
            asks_project: true,
            task: Some(task),
            ..Default::default()
        });
        assert_eq!(
            g.say(triage, NotificationType::QuestionAsked, msg(of_project), cause(claude, "triage")),
            "claude asked the project about Ankurah models in fwd-core"
        );
        let handed_on = g.message(MessageFacts {
            author: Some(daniel),
            asks_project: true,
            reply_to: Some(forward),
            thread: Some(question),
            task: Some(task),
            ..Default::default()
        });
        assert_eq!(
            g.say(triage, NotificationType::QuestionAsked, msg(handed_on), cause(daniel, "triage")),
            "dnorman forwarded to the project your question on Ankurah models in fwd-core"
        );
        // One that asks a scope, whose triage agent answers it, names the scope, asked or forwarded.
        let replication = g.scope("Replication");
        let of_scope = g.message(MessageFacts {
            author: Some(claude),
            asks_project: true,
            asks_scope: Some(replication),
            task: Some(task),
            ..Default::default()
        });
        assert_eq!(
            g.say(triage, NotificationType::QuestionAsked, msg(of_scope), cause(claude, "triage")),
            "claude asked the scope Replication about Ankurah models in fwd-core"
        );
        let to_scope = g.message(MessageFacts {
            author: Some(daniel),
            asks_project: true,
            asks_scope: Some(replication),
            reply_to: Some(forward),
            thread: Some(question),
            task: Some(task),
            ..Default::default()
        });
        assert_eq!(
            g.say(triage, NotificationType::QuestionAsked, msg(to_scope), cause(daniel, "triage")),
            "dnorman forwarded to the scope Replication your question on Ankurah models in fwd-core"
        );
        let direct = g.message(MessageFacts {
            author: Some(claude),
            to: Some(daniel.to_base64()),
            task: Some(task),
            ..Default::default()
        });
        assert_eq!(
            g.say(daniel, NotificationType::QuestionAsked, msg(direct), cause(claude, "addressee")),
            "claude asked you about Ankurah models in fwd-core"
        );
    }

    #[test]
    fn a_review_says_why_it_is_yours() {
        let mut g = Graph::default();
        let (daniel, claude, triage) = (g.person("dnorman"), g.person("claude"), g.person("triage-bot"));
        let task = g.task("Ankurah models in fwd-core");
        assert_eq!(
            g.say(daniel, NotificationType::ReviewRequested, task_target(task), cause(claude, "creator")),
            "claude submitted Ankurah models in fwd-core for your review; you created it"
        );
        // A triage agent reads why the review reached it, not that the review is its own (D129).
        assert_eq!(
            g.say(triage, NotificationType::ReviewRequested, task_target(task), cause(claude, "triage")),
            "claude submitted Ankurah models in fwd-core for review; you triage the project"
        );
        // The triage agent of one of the task's scopes, which routing names, reads which scope it
        // triages.
        let replication = g.scope("Replication");
        let mut of_scope = cause(claude, "triage");
        of_scope["triage_scope_id"] = json!(replication.to_base64());
        assert_eq!(
            g.say(triage, NotificationType::ReviewRequested, task_target(task), of_scope),
            "claude submitted Ankurah models in fwd-core for review; you triage the scope Replication"
        );
    }

    /// A notification routed before 2026-09-29 names the supervisor role as why it reached its
    /// recipient, and a question from then may be addressed to the role: each reads as the triage
    /// agent's, a question to the role as asked of the project.
    #[test]
    fn a_notification_routed_to_the_supervisor_role_reads_as_the_triage_agents() {
        let mut g = Graph::default();
        let (claude, triage) = (g.person("claude"), g.person("triage-bot"));
        let task = g.task("Ankurah models in fwd-core");
        assert_eq!(routing::why_of(&json!({"why": "supervisor"})), Some(Why::Triage));
        assert_eq!(Why::Triage.to_string(), "triage", "routing records the new word");
        assert_eq!(
            g.say(triage, NotificationType::ReviewRequested, task_target(task), cause(claude, "supervisor")),
            "claude submitted Ankurah models in fwd-core for review; you triage the project"
        );
        let old = g.message(MessageFacts {
            author: Some(claude),
            to: Some("supervisor".into()),
            task: Some(task),
            ..Default::default()
        });
        assert_eq!(
            g.say(triage, NotificationType::QuestionAsked, msg(old), cause(claude, "supervisor")),
            "claude asked the project about Ankurah models in fwd-core"
        );
    }

    #[test]
    fn a_question_about_a_task_says_whom_it_asks_to_the_tasks_people() {
        let mut g = Graph::default();
        let (daniel, claude, triage) = (g.person("dnorman"), g.person("claude"), g.person("triage-bot"));
        let task = g.task("Retry on disconnect");
        let question = g.message(MessageFacts {
            author: Some(claude),
            to: Some(daniel.to_base64()),
            task: Some(task),
            ..Default::default()
        });
        let mut o = cause(claude, "triage");
        o["message_id"] = json!(question.to_base64());
        assert_eq!(
            g.say(triage, NotificationType::TaskCommented, task_target(task), o),
            "claude asked dnorman about Retry on disconnect"
        );
    }

    #[test]
    fn a_task_filed_from_a_question_names_the_task() {
        let mut g = Graph::default();
        let (daniel, triage) = (g.person("dnorman"), g.person("triage-bot"));
        let question = g.message(MessageFacts {
            author: Some(daniel),
            to: Some(triage.to_base64()),
            content: "should the retry cap be configurable?".into(),
            ..Default::default()
        });
        let task = g.task("Make the retry cap configurable");
        let mut o = cause(triage, "source_author");
        o["message_id"] = json!(question.to_base64());
        assert_eq!(
            g.say(daniel, NotificationType::TaskCreated, task_target(task), o),
            "triage-bot filed a task from your question: Make the retry cap configurable"
        );
    }

    #[test]
    fn a_fallen_ruling_names_what_of_yours_cites_it() {
        let mut g = Graph::default();
        let (daniel, claude) = (g.person("dnorman"), g.person("claude"));
        let task = g.task("Cap the retry delay");
        let d = id();
        g.f.decisions.insert(d, (3, "Retries back off exponentially".into()));
        let mut o = cause(daniel, "citing_author");
        o["citing_id"] = json!(task.to_base64());
        assert_eq!(
            g.say(claude, NotificationType::DecisionRepealed, json!({"type": "Decision", "id": d.to_base64()}), o),
            "D3 (Retries back off exponentially) was repealed by dnorman; your task Cap the retry delay cites it"
        );
    }

    #[test]
    fn an_assignment_names_the_scope_and_a_mention_the_place() {
        let mut g = Graph::default();
        let (daniel, alice) = (g.person("dnorman"), g.person("impl-alice"));
        let task = g.task("Retry on disconnect");
        let scope = id();
        g.f.scopes.insert(scope, "Reconnects that survive NAT drops".into());
        let mut o = cause(daniel, "scope_owner");
        o["scope_id"] = json!(scope.to_base64());
        assert_eq!(
            g.say(alice, NotificationType::TaskAssigned, task_target(task), o),
            "Retry on disconnect was assigned to your scope Reconnects that survive NAT drops by dnorman"
        );
        let comment = g.message(MessageFacts { author: Some(daniel), task: Some(task), ..Default::default() });
        assert_eq!(
            g.say(alice, NotificationType::Mentioned, msg(comment), cause(daniel, "mentioned")),
            "dnorman mentioned you in a comment on Retry on disconnect"
        );
    }

    #[test]
    fn an_assignee_hears_of_their_task_and_a_step_party_of_their_step() {
        let mut g = Graph::default();
        let (daniel, alice) = (g.person("dnorman"), g.person("impl-alice"));
        let task = g.task("Retry on disconnect");
        assert_eq!(
            g.say(alice, NotificationType::TaskAssigned, task_target(task), cause(daniel, "assignee")),
            "Retry on disconnect was assigned to you by dnorman"
        );
        let comment = g.message(MessageFacts { author: Some(daniel), task: Some(task), ..Default::default() });
        let mut o = cause(daniel, "assignee");
        o["message_id"] = json!(comment.to_base64());
        assert_eq!(
            g.say(alice, NotificationType::TaskCommented, task_target(task), o),
            "dnorman commented on Retry on disconnect, which is assigned to you"
        );
        let blocker = g.task("Connector");
        let mut o = json!({"why": "assignee", "to": "done"});
        o["task_id"] = json!(blocker.to_base64());
        assert_eq!(
            g.say(alice, NotificationType::TaskUnblocked, task_target(task), o),
            "Retry on disconnect, which is assigned to you, is ready: Connector is done"
        );
        let review = g.task("Review: ws.rs changes");
        let anchor = id();
        g.f.anchors.insert(anchor, ("ws.rs".into(), Some((4, 7))));
        let mut o = cause(alice, "step_party");
        o["step_task_id"] = json!(review.to_base64());
        assert_eq!(
            g.say(daniel, NotificationType::CodeCommented, json!({"type": "CodeAnchor", "id": anchor.to_base64()}), o),
            "impl-alice commented on ws.rs:5-7, a step of your task Review: ws.rs changes"
        );
    }

    #[test]
    fn code_is_named_by_its_path_and_lines_and_a_whole_file_by_its_path_alone() {
        let mut g = Graph::default();
        let (daniel, alice) = (g.person("dnorman"), g.person("impl-alice"));
        let task = g.task("Split the parser");
        let (lines, line, whole) = (id(), id(), id());
        g.f.anchors.insert(lines, ("src/parser.rs".into(), Some((400, 410))));
        g.f.anchors.insert(line, ("src/parser.rs".into(), Some((400, 401))));
        g.f.anchors.insert(whole, ("src/parser.rs".into(), None));
        let mut o = cause(daniel, "assignee");
        o["near_task_id"] = json!(task.to_base64());
        let said = |anchor: EntityId, o: &Value| {
            let on = json!({"type": "CodeAnchor", "id": anchor.to_base64()});
            g.say(alice, NotificationType::CodeCommented, on, o.clone())
        };
        assert_eq!(
            said(lines, &o),
            "dnorman commented on src/parser.rs:401-410, near Split the parser, which is assigned to you"
        );
        assert_eq!(
            said(line, &o),
            "dnorman commented on src/parser.rs:401, near Split the parser, which is assigned to you"
        );
        assert_eq!(
            said(whole, &o),
            "dnorman commented on src/parser.rs, near Split the parser, which is assigned to you",
            "a whole file names no line"
        );
        // Routed before scopes lost their owner (D108), to the owner of the scope the task was
        // worked in: it still says why it came.
        o["why"] = json!("scope_owner");
        assert_eq!(
            said(lines, &o),
            "dnorman commented on src/parser.rs:401-410, near Split the parser, which your scope holds"
        );
    }

    #[test]
    fn a_comment_on_a_scope_reaches_the_triage_agent_and_one_routed_before_its_owner() {
        let mut g = Graph::default();
        let (daniel, triage) = (g.person("dnorman"), g.person("triage-bot"));
        let scope = id();
        g.f.scopes.insert(scope, "Reconnects that survive NAT drops".into());
        let comment = g.message(MessageFacts { author: Some(daniel), ..Default::default() });
        let on = json!({"type": "Scope", "id": scope.to_base64()});
        let mut o = cause(daniel, "triage");
        o["message_id"] = json!(comment.to_base64());
        assert_eq!(
            g.say(triage, NotificationType::TaskCommented, on.clone(), o.clone()),
            "dnorman commented on the scope Reconnects that survive NAT drops"
        );
        o["why"] = json!("scope_owner");
        assert_eq!(
            g.say(triage, NotificationType::TaskCommented, on, o),
            "dnorman commented on your scope Reconnects that survive NAT drops"
        );
    }

    #[test]
    fn a_message_named_by_its_words_names_its_references_too() {
        let mut g = Graph::default();
        let (daniel, alice) = (g.person("dnorman"), g.person("impl-alice"));
        let task = g.task("Connector");
        let question = g.message(MessageFacts {
            author: Some(daniel),
            to: Some(alice.to_base64()),
            content: format!("should [[{}]] wait for [[{}]]?", alice.to_base64(), task.to_base64()),
            ..Default::default()
        });
        let answer = g.message(MessageFacts {
            author: Some(alice),
            reply_to: Some(question),
            thread: Some(question),
            content: "no".into(),
            ..Default::default()
        });
        assert_eq!(
            g.say(daniel, NotificationType::Replied, msg(answer), cause(alice, "parent_author")),
            "impl-alice replied to your question “should @impl-alice wait for Connector?”"
        );
    }

    #[test]
    fn a_notification_from_before_reasons_were_recorded_says_what_its_type_allows() {
        let mut g = Graph::default();
        let (daniel, claude) = (g.person("dnorman"), g.person("claude"));
        let task = g.task("Connector");
        let comment = g.message(MessageFacts { author: Some(daniel), task: Some(task), ..Default::default() });
        let reply = g.message(MessageFacts {
            author: Some(claude),
            reply_to: Some(comment),
            thread: Some(comment),
            task: Some(task),
            ..Default::default()
        });
        let old = json!({"actor_id": claude.to_base64(), "message_id": reply.to_base64()});
        assert_eq!(g.say(daniel, NotificationType::Replied, msg(reply), old.clone()), "claude replied on Connector");
        assert_eq!(
            g.say(daniel, NotificationType::ReviewRequested, task_target(task), old),
            "claude submitted Connector for your review"
        );
    }

    /// An originator for a batch by `actor` on `task`, told for `why`, of `steps` steps and then
    /// `comments` comments.
    fn batch(actor: EntityId, why: &str, task: EntityId, steps: usize, comments: usize) -> Value {
        let step = |n: usize| json!({"relation_id": id().to_base64(), "step_id": id().to_base64(), "at": n});
        let comment = |n: usize| json!({"message_id": id().to_base64(), "at": 100 + n});
        let items: Vec<Value> = (0..steps).map(step).chain((0..comments).map(comment)).collect();
        let mut o = cause(actor, why);
        o["batch"] = json!({"task_id": task.to_base64(), "items": items});
        o
    }

    #[test]
    fn a_batch_says_how_many_of_what_its_author_did_on_the_task() {
        let mut g = Graph::default();
        let (daniel, x, triage) = (g.person("dnorman"), g.person("impl-x"), g.person("triage-bot"));
        let task = g.task("Review: the retry loop");
        let said = |me: EntityId, why: &str, steps: usize, comments: usize| {
            g.say(me, NotificationType::Digest, task_target(task), batch(x, why, task, steps, comments))
        };
        assert_eq!(
            said(daniel, "step_party", 3, 2),
            "impl-x added 3 steps and commented twice on your task Review: the retry loop"
        );
        assert_eq!(said(daniel, "step_party", 2, 0), "impl-x added 2 steps to your task Review: the retry loop");
        assert_eq!(
            said(daniel, "assignee", 1, 1),
            "impl-x added a step and commented on your task Review: the retry loop"
        );
        assert_eq!(said(triage, "triage", 0, 3), "impl-x commented 3 times on Review: the retry loop");
        let near = g.task("Retry on disconnect");
        let mut o = batch(x, "assignee", task, 0, 2);
        o["near_task_id"] = json!(near.to_base64());
        assert_eq!(
            g.say(triage, NotificationType::Digest, task_target(task), o.clone()),
            "impl-x commented twice on Review: the retry loop, near Retry on disconnect, which is assigned to you"
        );
        o["why"] = json!("scope_owner");
        assert_eq!(
            g.say(triage, NotificationType::Digest, task_target(task), o),
            "impl-x commented twice on Review: the retry loop, near Retry on disconnect, which your scope holds",
            "as a digest routed before scopes lost their owner (D108) says"
        );
        // One item says what its type says, as before batches existed.
        assert_eq!(
            g.say(daniel, NotificationType::StepAdded, task_target(task), batch(x, "step_party", task, 1, 0)),
            "impl-x added a step to your task Review: the retry loop"
        );
        let comment = g.message(MessageFacts { author: Some(x), task: Some(task), ..Default::default() });
        let mut o = batch(x, "assignee", task, 0, 0);
        o["batch"]["items"] = json!([{"message_id": comment.to_base64(), "at": 1}]);
        o["message_id"] = json!(comment.to_base64());
        assert_eq!(
            g.say(daniel, NotificationType::TaskCommented, task_target(task), o),
            "impl-x commented on Review: the retry loop, which is assigned to you"
        );
    }

    #[test]
    fn a_batch_lists_its_items_when_it_holds_several_or_a_step_alone() {
        let (x, task) = (id(), id());
        let counts = |o: &Value| listed(o).map(|b| (b.steps(), b.comments()));
        assert_eq!(counts(&batch(x, "assignee", task, 0, 1)), None, "one comment is the message behind it");
        assert_eq!(counts(&batch(x, "step_party", task, 1, 0)), Some((1, 0)));
        assert_eq!(counts(&batch(x, "step_party", task, 1, 1)), Some((1, 1)));
        assert_eq!(counts(&batch(x, "triage", task, 0, 2)), Some((0, 2)));
        assert_eq!(counts(&cause(x, "assignee")), None, "a notification that is no batch");
    }

    /// A review's batch, oldest first: step 1 (a highlight, 101) and its first comment (201),
    /// step 2 (102) and its comment (202), a comment on the task (203), a second comment on step 1
    /// (204), a step that is a task (103), and a comment on a highlight that is not a step of the
    /// batch (205, on 900).
    #[test]
    fn a_batch_lists_each_step_with_its_comments_and_the_rest_in_place_newest_last() {
        use Held::{Comment, Step};
        let held =
            [Step(101), Comment(201), Step(102), Comment(202), Comment(203), Comment(204), Step(103), Comment(205)];
        let on = |message: u32| match message {
            201 | 204 => Some(101),
            202 => Some(102),
            205 => Some(900),
            _ => None,
        };
        assert_eq!(
            batch_entries(&held, on),
            vec![
                BatchEntry::Step { step: 101, comments: vec![201, 204] },
                BatchEntry::Step { step: 102, comments: vec![202] },
                BatchEntry::Comment { message: 203, on: None },
                BatchEntry::Step { step: 103, comments: vec![] },
                BatchEntry::Comment { message: 205, on: Some(900) },
            ]
        );
        // A comment its reader does not hold yet shows on its own until it arrives.
        assert_eq!(
            batch_entries(&[Step(101), Comment(201)], |_| None),
            vec![BatchEntry::Step { step: 101, comments: vec![] }, BatchEntry::Comment { message: 201, on: None }]
        );
        // A comment written before its highlight became a step still reads with the step.
        assert_eq!(
            batch_entries(&[Comment(201), Step(101)], on),
            vec![BatchEntry::Step { step: 101, comments: vec![201] }]
        );
    }

    /// A step added with the threads already open on its lines (task WTCAd9la) says how many of
    /// each it came with, whoever wrote them, a question by its own word, and lists them beside
    /// the step; with the one thread routing names as the message behind it, its sentence ends with
    /// how to answer it, and with several it does not. In a digest, the threads its steps came with
    /// are said apart from what its author commented.
    #[test]
    fn a_step_added_with_the_threads_on_its_lines_says_how_many_of_each() {
        use NotificationType::{Digest, StepAdded};
        let mut g = Graph::default();
        let (daniel, lead, bob) = (g.person("dnorman"), g.person("lead-storage"), g.person("impl-bob"));
        let task = g.task("Review: the policy context");
        let highlight = id();
        let on_it =
            |author: EntityId| MessageFacts { author: Some(author), anchor: Some(highlight), ..Default::default() };
        let comment = g.message(on_it(daniel));
        let bobs = g.message(on_it(bob));
        let question = g.message(MessageFacts { asks_project: true, ..on_it(daniel) });
        let step = json!({"relation_id": id().to_base64(), "step_id": highlight.to_base64(), "at": 50});
        let thread = |m: EntityId, at: i64| json!({"thread_id": m.to_base64(), "at": at});
        let added = |items: Vec<Value>| {
            let mut o = cause(daniel, "step_party");
            o["batch"] = json!({"task_id": task.to_base64(), "items": items});
            o
        };
        let n = id();

        let mut one = added(vec![thread(comment, 10), step.clone()]);
        one["message_id"] = json!(comment.to_base64());
        assert_eq!(
            g.say_of(n, lead, StepAdded, task_target(task), one),
            format!(
                "dnorman added a step with a comment to your task Review: the policy context; reply with fwd reply {}",
                &n.to_base64()[..8]
            )
        );
        let several = added(vec![thread(comment, 10), thread(bobs, 20), thread(question, 30), step.clone()]);
        assert_eq!(
            g.say_of(n, lead, StepAdded, task_target(task), several.clone()),
            "dnorman added a step with 2 comments and a question to your task Review: the policy context"
        );
        let batch = listed(&several).expect("a step with its threads is listed");
        let held: Vec<Held<EntityId>> = batch.items.iter().map(Held::from).collect();
        let on = |m: EntityId| g.f.messages.get(&m).and_then(|f| f.anchor);
        assert_eq!(
            batch_entries(&held, on),
            vec![BatchEntry::Step { step: highlight, comments: vec![comment, bobs, question] }],
            "the threads beside their step, oldest first"
        );

        let mut digest = added(vec![thread(question, 10), step, json!({"message_id": comment.to_base64(), "at": 60})]);
        digest["message_id"] = json!(comment.to_base64());
        assert_eq!(
            g.say_of(n, lead, Digest, task_target(task), digest),
            "dnorman added a step with a question and commented on your task Review: the policy context"
        );
    }

    #[test]
    fn a_name_not_yet_known_is_asked_for_and_never_shown_as_an_id() {
        let g = Graph::default();
        let (me, task) = (id(), id());
        let said = g.say(me, NotificationType::TaskUnblocked, task_target(task), json!({"why": "triage"}));
        assert_eq!(said, "a task is ready and unassigned");
        assert!(g.f.wanted.borrow().contains(&(Want::Task, task)), "the missing title is asked for");
    }
}
