//! The inbox's card: everything the acting principal was notified of about one conversation, one
//! task or one decision, as one card, so one event (a question answered and a task filed from it)
//! reads as one thing rather than as two cards rendering the same thread (Daniel, 2026-09-26).
//! The card is headed by one line per notification, newest first, each saying in words why it is
//! in this inbox and naming what it is about (`fwd_core::reason`). Beneath them the card mounts,
//! once, the row of what its notifications point into (`thread.rs`): the first message of the
//! conversation, with the replies beneath it and the task filed from one of them as a lozenge on
//! that reply's row; else the task or the decision. A card about a task shows, when a review or a
//! proposal is among its notifications, the task's done-when and evidence, or its why and what,
//! beneath the task's row. Mark read and Dismiss act on every notification of the card. The card
//! is selected and worked from the keyboard as `cards.rs` describes. On another principal's inbox,
//! shown to read (`whose.rs`), the lines say why each notification is in their inbox as they would
//! read it, "you" being them, and Mark read and Dismiss are disabled, saying why on hover.
//!
//! A notification that collects what one author did on one task (D95; `fwd_core::routing::Batch`)
//! belongs on the task's card, whose row opens the task, and lists its items beneath its line,
//! newest last: each step as the lozenge of what it is, with the first line of each comment on its
//! highlight beside it, and each other comment by its first line, in the order `fwd inbox` lists
//! them too (`fwd_core::reason::batch_entries`). Reading the card reads the notification, which
//! ends the batch: what the author does next starts another.

use crate::cards::{id_at, Item, Page};
use crate::referents::{Mode, Models};
use crate::route::Route;
use crate::thread::{self, Model};
use crate::{fmt, modes, references, store, task_row, whose};
use ankurah::EntityId;
use fwd_core::model::{NotificationStatus, NotificationType, NotificationView};
use fwd_core::ops;
use fwd_core::reason::{self, BatchEntry, Held, MessageFacts, Named, Names, Part, RelationFacts};
use fwd_core::routing::Batch;
use leptos::prelude::*;
use serde_json::Value;
use std::cmp::Reverse;
use std::collections::HashMap;
use std::hash::Hash;

/// Where a notification belongs on the page: the card it shares with every notification that
/// points into the same place (`key`), and the row that card mounts (the entity and its model),
/// when there is one the page draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Place {
    pub key: EntityId,
    pub root: Option<(EntityId, Model)>,
}

/// The batch a notification holds when its card lists it (`reason::listed`): several items, or a
/// step alone. A single comment's card is its conversation's, as before batches existed.
fn listed(n: &NotificationView) -> Option<Batch> {
    let originator: Value = n.originator().map(Into::into).unwrap_or(Value::Null);
    reason::listed(&originator)
}

/// The entity a notification is about, as its card shows it: the task a batch it lists is about;
/// the decision a proposal, a repeal or a supersession is about (its reason shows among its
/// comments); else the message behind the notification (a comment on a decision among them); else
/// the task or the decision it names.
fn about(n: &NotificationView) -> Option<(EntityId, Model)> {
    if let Some(batch) = listed(n) {
        return Some((batch.task, Model::Task));
    }
    let notification_type = n.notification_type().ok();
    let target: Value = n.target().map(Into::into).unwrap_or(Value::Null);
    let originator: Value = n.originator().map(Into::into).unwrap_or(Value::Null);
    let target_id = id_at(&target["id"]);
    match (target["type"].as_str(), target_id) {
        (Some("Decision"), Some(decision)) if notification_type != Some(NotificationType::DecisionCommented) => {
            Some((decision, Model::Decision))
        }
        _ => id_at(&originator["message_id"]).map(|m| (m, Model::Message)).or(
            match (target["type"].as_str(), target_id) {
                (Some("Task"), Some(task)) => Some((task, Model::Task)),
                (Some("Decision"), Some(decision)) => Some((decision, Model::Decision)),
                _ => None,
            },
        ),
    }
}

/// Where a notification belongs. A message is followed to its thread's first message, so the
/// notifications pointing into one conversation (a question, the replies to it, the task filed
/// from it, which names the message it came from) share a card; a notification about a task or a
/// decision alone, or a batch of what one author did on a task, shares the card of that task or
/// decision; one about nothing the page draws (a proposed relation) has a card of its own.
pub fn place(n: &NotificationView) -> Place {
    match about(n) {
        Some((message, Model::Message)) => {
            let first = store::message(message)
                .and_then(|m| m.thread().ok().flatten().or(m.reply_to().ok().flatten()).map(EntityId::from))
                .unwrap_or(message);
            Place { key: first, root: Some((first, Model::Message)) }
        }
        Some((id, model)) => Place { key: id, root: Some((id, model)) },
        None => {
            let target: Value = n.target().map(Into::into).unwrap_or(Value::Null);
            Place { key: id_at(&target["id"]).unwrap_or(n.id()), root: None }
        }
    }
}

/// One notification, as the page groups them into cards.
#[derive(Clone, Copy, Debug)]
pub struct Entry<K> {
    pub id: K,
    /// The card it belongs on.
    pub key: K,
    /// When it last changed: when it was made, or when the last item joined its batch
    /// (`ops::notification_time`), so a batch that grew lists as new.
    pub at: i64,
}

/// The cards `entries` make, each with its notifications newest first, the cards newest first by
/// their newest notification (then by key, so every client lists them alike).
pub fn group<K: Copy + Ord + Hash>(entries: &[Entry<K>]) -> Vec<(K, Vec<K>)> {
    let mut by_key: HashMap<K, Vec<(i64, K)>> = HashMap::new();
    for e in entries {
        by_key.entry(e.key).or_default().push((e.at, e.id));
    }
    let mut cards: Vec<(i64, K, Vec<K>)> = by_key
        .into_iter()
        .map(|(key, mut members)| {
            members.sort_by_key(|(at, id)| (Reverse(*at), *id));
            (members[0].0, key, members.into_iter().map(|(_, id)| id).collect())
        })
        .collect();
    cards.sort_by_key(|(newest, key, _)| (Reverse(*newest), *key));
    cards.into_iter().map(|(_, key, members)| (key, members)).collect()
}

/// One card of the inbox: its key, the row it mounts, and its notifications, newest first.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub key: EntityId,
    pub root: Option<(EntityId, Model)>,
    pub members: Vec<EntityId>,
}

/// The cards the notifications `keep` accepts make.
pub fn cards(notifications: &[NotificationView], keep: impl Fn(&NotificationView) -> bool) -> Vec<Card> {
    let mut roots = HashMap::new();
    let entries: Vec<Entry<EntityId>> = notifications
        .iter()
        .filter(|n| keep(n))
        .map(|n| {
            let place = place(n);
            roots.insert(place.key, place.root);
            Entry { id: n.id(), key: place.key, at: ops::notification_time(n) }
        })
        .collect();
    group(&entries)
        .into_iter()
        .map(|(key, members)| Card { key, root: roots.get(&key).copied().flatten(), members })
        .collect()
}

/// The names a sentence mentions, from the page's standing queries: read inside a closure, the
/// sentence redraws as the names arrive or change.
pub struct StoreNames;

impl Names for StoreNames {
    fn handle(&self, principal: EntityId) -> Option<String> {
        store::principal(principal).and_then(|p| p.handle().ok())
    }
    fn task_title(&self, id: EntityId) -> Option<String> {
        store::task(id).and_then(|t| t.title().ok())
    }
    fn decision(&self, id: EntityId) -> Option<(i64, String)> {
        store::decision(id).map(|d| (d.number().unwrap_or(0), d.title().unwrap_or_default()))
    }
    fn scope_name(&self, id: EntityId) -> Option<String> {
        store::scope(id).and_then(|s| s.title().ok())
    }
    fn anchor(&self, id: EntityId) -> Option<(String, Option<(i64, i64)>)> {
        let a = store::anchor(id)?;
        Some((a.path().ok()?, fwd_core::ops::lines_of(&a).ok()?))
    }
    fn message(&self, id: EntityId) -> Option<MessageFacts> {
        store::message(id).map(|m| MessageFacts::of(&m))
    }
    fn relation(&self, id: EntityId) -> Option<RelationFacts> {
        let r = store::relation(id)?;
        Some(RelationFacts { kind: r.kind().ok()?, from: r.from_id().ok()?, to: r.to_id().ok()? })
    }
}

/// Why the notification `n` is in `me`'s inbox, as plain words (for reading aloud), without the
/// line's `fwd reply` and the notification's id, which help no listener.
pub fn sentence(me: EntityId, n: &NotificationView) -> String {
    let Ok(t) = n.notification_type() else { return String::new() };
    let target: Value = n.target().map(Into::into).unwrap_or(Value::Null);
    let originator: Value = n.originator().map(Into::into).unwrap_or(Value::Null);
    reason::plain(&reason::sentence(&StoreNames, me, None, t, &target, &originator))
}

/// A sentence drawn for the page: the tasks and decisions it names lead to them, and what a
/// quoted message names in its text reads as words.
fn parts_view(parts: Vec<Part>) -> Vec<AnyView> {
    parts
        .into_iter()
        .map(|part| match part {
            Part::Text(text) => references::plain(&text).into_any(),
            Part::Name { kind: Named::Task, id, text } => {
                view! { <a class="name" href=Route::task(id).hash()>{text}</a> }.into_any()
            }
            Part::Name { kind: Named::Decision, id, text } => {
                view! { <a class="name" href=Route::decision(id).hash()>{text}</a> }.into_any()
            }
            Part::Name { kind: Named::Anchor, text, .. } => view! { <span class="name mono">{text}</span> }.into_any(),
            Part::Name { text, .. } => view! { <span class="name">{text}</span> }.into_any(),
        })
        .collect()
}

/// A batch's items beneath its line, in the order `fwd inbox` lists them too
/// (`reason::batch_entries`): a step as the lozenge of what it is, with each comment on its
/// highlight beside it as the comment's list item; any other comment as its list item, after the
/// lozenge of the highlight it is on.
fn batch_view(batch: Batch) -> AnyView {
    let held: Vec<Held<EntityId>> = batch.items.iter().map(Held::from).collect();
    let on = |message: EntityId| store::message(message).and_then(|m| m.anchor().ok().flatten()).map(EntityId::from);
    let entries = Memo::new(move |_| reason::batch_entries(&held, on));
    let comment =
        |message: EntityId| modes::draw(Mode::Embedded, Mode::ListItem, message, &Models::one(Model::Message));
    let drawn = move || {
        entries
            .get()
            .into_iter()
            .map(|entry| match entry {
                BatchEntry::Step { step, comments } => view! {
                    <li>
                        {move || modes::draw(Mode::Embedded, Mode::Lozenge, step, &modes::models_of(step))}
                        {comments.into_iter().map(comment).collect::<Vec<_>>()}
                    </li>
                }
                .into_any(),
                BatchEntry::Comment { message, on } => view! {
                    <li>
                        {on.map(|highlight| modes::draw(Mode::Embedded, Mode::Lozenge, highlight, &Models::one(Model::Anchor)))}
                        {comment(message)}
                    </li>
                }
                .into_any(),
            })
            .collect::<Vec<_>>()
    };
    view! { <li class="why-batch"><ul class="batch-items">{drawn}</ul></li> }.into_any()
}

/// One notification's line at the head of its card: why it is in `me`'s inbox, and for a message
/// that its recipient answers inside the message's thread how to answer it there, `fwd reply` and
/// the notification's id (`reason::sentence`), when `reader`, the principal the page acts as, is
/// that recipient (`reason::reply_id`), then when and how urgent; and beneath it, for a batch, the
/// items it holds.
fn reason_line(me: EntityId, reader: EntityId, n: &NotificationView) -> AnyView {
    let reply = n.recipient().ok().and_then(|recipient| reason::reply_id(n.id(), recipient.into(), Some(reader)));
    let t = n.notification_type().ok();
    let target: Value = n.target().map(Into::into).unwrap_or(Value::Null);
    let originator: Value = n.originator().map(Into::into).unwrap_or(Value::Null);
    let unread = n.status().ok() == Some(NotificationStatus::Unread);
    let meta = format!(
        "{} · {}",
        fmt::ago(ops::notification_time(n)),
        n.priority().map(|p| p.to_string()).unwrap_or_default()
    );
    let kind = t.map(|t| t.to_string().replace('_', " ")).unwrap_or_default();
    // Worked out again as what it names arrives or changes, and drawn again only when its words do.
    let parts = Memo::new(move |_| t.map(|t| reason::sentence(&StoreNames, me, reply, t, &target, &originator)));
    let words = move || parts.get().map(parts_view);
    view! {
        <li class="why-line" class:unread=unread title=kind>
            <span class="sentence">{words}</span>
            <span class="meta">{meta}</span>
        </li>
        {listed(n).map(batch_view)}
    }
    .into_any()
}

#[component]
pub fn NotificationCard(
    key: EntityId,
    root: Option<(EntityId, Model)>,
    cards: Memo<Vec<Card>>,
    page: Page,
) -> impl IntoView {
    let item = Item::Thread(key);
    // Whose inbox the card is in: its lines say why each notification is there as its owner reads
    // it, "you" being them.
    let owner = page.whose;
    let members = Memo::new(move |_| {
        cards.with(|all| all.iter().find(|c| c.key == key).map(|c| c.members.clone()).unwrap_or_default())
    });
    // The card's notifications, newest first, each looked up by its id: a memo, so that a change
    // to a notification on another card redraws nothing here, while the closures below read the
    // fields of these, so that a change to one of them redraws this card alone (task GEQEKfUd).
    let shown = Memo::new(move |_| {
        members.with(|ids| ids.iter().filter_map(|id| page.notifications.get(*id)).collect::<Vec<_>>())
    });
    let unread = move || shown.with(|ns| ns.iter().any(|n| n.status().ok() == Some(NotificationStatus::Unread)));
    let selected = Memo::new(move |_| page.selected.get() == Some(item));
    let lines = move || shown.with(|ns| ns.iter().map(|n| reason_line(owner, page.thread.me, n)).collect::<Vec<_>>());
    // What a reviewer or an approver needs, beneath the task's row.
    let fields = Memo::new(move |_| {
        shown.with(|ns| task_row::Field::for_notifications(ns.iter().filter_map(|n| n.notification_type().ok())))
    });
    let mut hooks = page.hooks(item, None);
    hooks.task_fields = Some(fields.into());
    let status_all = move |status: NotificationStatus| {
        let ids = page
            .on_card(key)
            .into_iter()
            .filter(|n| match status {
                NotificationStatus::Read => n.status().ok() == Some(NotificationStatus::Unread),
                _ => n.status().ok() != Some(status),
            })
            .map(|n| n.id())
            .collect();
        page.mark(ids, status, false);
    };
    // On another principal's inbox, shown to read, Mark read and Dismiss stay disabled and say why.
    let acts = page.acts();
    let reading =
        move || (!acts).then(|| whose::reading_inbox(&store::handle_of(owner), &store::handle_of(page.thread.me)));

    view! {
        <article class="notif" id=item.element_id() tabindex="-1"
            class:unread=unread class:selected=move || selected.get()>
            <ul class="why-lines">{lines}</ul>
            {root.map(|(entity, model)| thread::card_row(page.thread, key, entity, Models::one(model), hooks))}
            <div class="actions">
                {move || {
                    unread().then(|| {
                        view! {
                            <button class="btn" disabled=!acts title=reading
                                on:click=move |_| status_all(NotificationStatus::Read)>"Mark read"</button>
                        }
                    })
                }}
                <button class="btn subtle" disabled=!acts title=reading
                    on:click=move |_| status_all(NotificationStatus::Dismissed)>"Dismiss"</button>
            </div>
        </article>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(id: u32, key: u32, at: i64) -> Entry<u32> {
        Entry { id, key, at }
    }

    #[test]
    fn one_card_per_place_newest_first() {
        // A question (key 1) answered at 30 and a task filed from it at 40; a review of a task
        // (key 2) at 35; a reply in another thread (key 3) at 10.
        let cards = group(&[e(10, 1, 20), e(11, 1, 30), e(12, 2, 35), e(13, 1, 40), e(14, 3, 10)]);
        assert_eq!(cards, vec![(1, vec![13, 11, 10]), (2, vec![12]), (3, vec![14])]);
    }

    #[test]
    fn cards_made_at_the_same_time_list_alike_everywhere() {
        let cards = group(&[e(21, 5, 7), e(20, 4, 7), e(22, 4, 7)]);
        assert_eq!(cards, vec![(4, vec![20, 22]), (5, vec![21])]);
    }
}
