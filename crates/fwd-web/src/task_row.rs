//! A task as a row: its short id, its title linked to its page, its labels (`labels.rs`), its
//! status, and the lozenges of what it is joined to (its scope, the rulings it cites, what it
//! depends on, its subtasks: `lozenges.rs`); Enter on the row opens the task. Beneath it hang the
//! comments that open a thread on it (messages whose `task` is this task and that answer no other
//! message: a reply to a comment carries the comment's task too, and belongs under the comment) and
//! the messages linked to it by a `concerns` relation, each drawn as the row of its own models. As
//! a card's own row it also shows, labelled, the prose the card asks for (a review card its
//! done-when and evidence), between its line and what hangs beneath it. That row is the task's
//! embedded mode; its list item is its short id, title, labels, status and how many of its steps
//! are checked.

use crate::lozenges::{self, Target};
use crate::referents::{Child, Listing, Models};
use crate::route::{self, Route};
use crate::thread::{self, Actions, Link, Model, Query, RowAt, ThreadState};
use crate::{empty, fmt, labels, steps, store};
use ankurah::EntityId;
use fwd_core::model::{NotificationType, RelationKind, RelationStatus, TaskView};
use fwd_core::ops;
use leptos::prelude::*;
use serde_json::Value;
use std::collections::BTreeMap;

/// A task's prose, which a card can ask its task row to show: what a reviewer needs without
/// opening the task (its done-when and the evidence it was submitted with), and what whoever rules
/// on a proposal needs (why it should exist and what it is).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Field {
    Why,
    What,
    DoneWhen,
    Evidence,
}

impl Field {
    fn label(self) -> &'static str {
        match self {
            Field::Why => "Why",
            Field::What => "What",
            Field::DoneWhen => "Done when",
            Field::Evidence => "Evidence",
        }
    }

    fn of(self, t: &TaskView) -> String {
        match self {
            Field::Why => ops::text(t.why()).unwrap_or_default(),
            Field::What => ops::text(t.what()).unwrap_or_default(),
            Field::DoneWhen => ops::text(t.done_when()).unwrap_or_default(),
            Field::Evidence => {
                let context: Value = t.context().map(Into::into).unwrap_or(Value::Null);
                context["evidence"].as_str().unwrap_or("").to_string()
            }
        }
    }

    /// The fields a card about a task shows for notifications of these types, in reading order:
    /// a review its done-when and evidence, a proposal its why and what.
    pub fn for_notifications(types: impl IntoIterator<Item = NotificationType>) -> Vec<Field> {
        let mut fields: Vec<Field> = types
            .into_iter()
            .flat_map(|t| match t {
                NotificationType::ReviewRequested => vec![Field::DoneWhen, Field::Evidence],
                NotificationType::TaskProposed => vec![Field::Why, Field::What],
                _ => vec![],
            })
            .collect();
        fields.sort();
        fields.dedup();
        fields
    }
}

/// The links a task follows to the messages beneath it: its comments, and the messages about it.
pub const FOLLOWS: &[Link] = &[Link::Comments, Link::Concerns];

/// The live query behind one of a task's links: its comments need one; the messages linked to it
/// by relations come from the page's standing queries.
pub fn open(parent: EntityId, link: Link) -> Option<Query> {
    (link == Link::Comments).then(|| thread::messages_where("task = ?", parent))
}

/// The messages beneath the task `parent` through `link`.
pub fn listing(parent: EntityId, link: Link, query: Option<&Query>) -> Listing<EntityId, Model, Link> {
    match link {
        Link::Comments => thread::from_messages(query, |m| {
            let on_task = m.task().ok().flatten().map(EntityId::from) == Some(parent);
            let answers_another = m.reply_to().ok().flatten().is_some();
            (on_task && !answers_another).then(|| Child {
                id: m.id(),
                models: Models::one(Model::Message),
                link,
                label: String::new(),
                created: m.created().unwrap_or(0),
            })
        }),
        Link::Concerns => concerning(parent, link),
        other => Listing::Failed(format!("a task does not follow {other:?}")),
    }
}

/// The messages joined to the task `parent` by a `concerns` relation that is neither retracted
/// nor rejected, from the page's standing queries: each once, labelled "about it" (marked while
/// every such relation is only proposed), at its own creation time, or at the relation's until
/// the page has the message.
fn concerning(parent: EntityId, link: Link) -> Listing<EntityId, Model, Link> {
    let relations = store::relations_of(parent);
    if !store::standing_loaded() {
        return Listing::Loading;
    }
    // By message: whether every relation placing it is only proposed, and when the first was made.
    let mut found: BTreeMap<EntityId, (bool, i64)> = BTreeMap::new();
    for r in &relations {
        let (Ok(to), Ok(from), Ok(kind), Ok(status)) = (r.to_id(), r.from_id(), r.kind(), r.status()) else {
            continue;
        };
        if to != parent || kind != RelationKind::Concerns {
            continue;
        }
        if matches!(status, RelationStatus::Retracted | RelationStatus::Rejected) {
            continue;
        }
        let entry = found.entry(from).or_insert((true, i64::MAX));
        entry.0 &= status == RelationStatus::Proposed;
        entry.1 = entry.1.min(r.created().unwrap_or(0));
    }
    let children = found
        .into_iter()
        .map(|(id, (proposed, made))| Child {
            id,
            models: Models::one(Model::Message),
            link,
            label: if proposed { "about it (proposed)".into() } else { "about it".into() },
            created: store::message(id).and_then(|m| m.created().ok()).unwrap_or(made),
        })
        .collect();
    Listing::Ready(children)
}

/// The task's short id and title, to name it where it shows again.
pub fn name(id: EntityId) -> Option<String> {
    let task = store::task(id)?;
    Some(format!("{} {}", fmt::short(id), task.title().unwrap_or_default()))
}

/// The lozenge mode: its title, struck through once it is closed.
pub fn lozenge(id: EntityId) -> AnyView {
    lozenges::single(Target::Task(id))
}

/// The list item mode: its short id, its title (leading to its panel), its labels, its status, and
/// how many of its steps are checked when it has any.
pub fn list_item(id: EntityId) -> AnyView {
    (move || {
        let Some(t) = store::task(id) else {
            let absent = format!("{}: {}", fmt::short(id), empty::absent(Some(Model::Task)));
            return view! { <span class="list-item muted">{absent}</span> }.into_any();
        };
        let status = t.status().ok();
        let progress = steps::progress_of(id);
        view! {
            <span class="list-item task-item">
                <span class="mono">{fmt::short(id)}</span>
                " "
                <a href=Route::task(id).hash()>{t.title().unwrap_or_default()}</a>
                {labels::pills(labels::of(&t))}
                " "
                {status.map(|s| view! { <span class=format!("pill status-{s}")>{fmt::status_label(s)}</span> })}
                {progress.map(|p| view! { " " <span class="step-progress" title="steps checked">{p}</span> })}
            </span>
        }
        .into_any()
    })
    .into_any()
}

/// The embedded mode: the task's row, its comments beneath it.
pub fn embedded(state: ThreadState, at: RowAt) -> AnyView {
    view! { <TaskRow state=state at=at/> }.into_any()
}

#[component]
pub fn TaskRow(state: ThreadState, at: RowAt) -> impl IntoView {
    let id = at.id;
    // Changes only when the task arrives or goes; the fields below redraw on their own.
    let task = Memo::new(move |_| store::task(id));
    let title = move || task.get().and_then(|t| t.title().ok()).unwrap_or_else(|| empty::absent(Some(Model::Task)));
    let status = move || task.get().and_then(|t| t.status().ok());
    // The prose the card asks for, labelled; a field with nothing in it says nothing.
    let asked = at.card_task_fields();
    let fields = move || {
        let (asked, t) = (asked?.get(), task.get()?);
        let shown: Vec<_> = asked
            .into_iter()
            .map(|field| (field.label(), field.of(&t)))
            .filter(|(_, text)| !text.trim().is_empty())
            .map(|(label, text)| view! { <section class="prose"><h4>{label}</h4><p>{text}</p></section> })
            .collect();
        (!shown.is_empty()).then(|| view! { <div class="task-fields">{shown}</div> })
    };
    let strip = at.strip();
    let line = view! {
        <div class="task-ref">
            <span class="mono">{fmt::short(id)}</span>
            " "
            <a href=Route::task(id).hash()>{title}</a>
            {move || task.get().map(|t| labels::pills(labels::of(&t)))}
            " "
            {move || status().map(|s| view! { <span class=format!("pill status-{s}")>{fmt::status_label(s)}</span> })}
            " "
            {strip}
            {fields}
        </div>
    }
    .into_any();
    thread::frame(state, at, line, Actions { editor: None, activate: Some(open_task) })
}

fn open_task(id: EntityId) {
    route::go(Route::task(id));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_review_shows_done_when_and_evidence_and_a_proposal_why_and_what() {
        use NotificationType::*;
        assert_eq!(Field::for_notifications([ReviewRequested]), vec![Field::DoneWhen, Field::Evidence]);
        assert_eq!(Field::for_notifications([TaskProposed, TaskAssigned]), vec![Field::Why, Field::What]);
        assert_eq!(
            Field::for_notifications([ReviewRequested, TaskProposed, ReviewRequested]),
            vec![Field::Why, Field::What, Field::DoneWhen, Field::Evidence],
            "each field once, in reading order"
        );
        assert!(Field::for_notifications([TaskAssigned, TaskUnblocked]).is_empty());
    }
}
