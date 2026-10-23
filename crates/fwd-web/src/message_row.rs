//! A message as a row: who wrote it, whom it asks when it asks someone (a question, or a forward),
//! when, and when it was edited if it was (and by whom, when an agent that succeeded its author
//! edited it), what it says (each reference in its text drawn as the lozenge of what it names:
//! `references.rs`), the lozenges of what it is on and what it is joined to (the task filed from
//! it, a ruling it cites or one decided in its context: `lozenges.rs`; not what its text names,
//! which shows where the text names it), with reply and resolve, and, on a message the acting
//! principal wrote or whose author it succeeded, edit, which opens its text in a box in place of
//! the text (`thread::edit_box`). Beneath it hang its replies and the forwards of it (messages
//! whose `reply_to` is this message), each drawn as the row of its own models. That row is the
//! message's embedded mode; its list item is who wrote it and how it begins, and its full mode the
//! row standing alone with its references.

use crate::lozenges::{self, Target};
use crate::referents::{Child, Listing, Models};
use crate::session::{self, ctx};
use crate::thread::{self, Actions, Editor, Hooks, Link, Model, Query, RowAt, Sending, ThreadState};
use crate::{empty, fmt, references, steps, store};
use ankurah::EntityId;
use fwd_core::model::{MessageStatus, MessageView, Priority};
use fwd_core::ops::{self, MessageContext};
use fwd_core::{routing, rules};
use leptos::prelude::*;
use serde_json::Value;
use wasm_bindgen_futures::spawn_local;

/// The links a message follows to the messages beneath it: its replies and forwards.
pub const FOLLOWS: &[Link] = &[Link::Replies];

/// The live query behind a message's replies.
pub fn open(parent: EntityId, link: Link) -> Option<Query> {
    (link == Link::Replies).then(|| thread::messages_where("reply_to = ?", parent))
}

/// The messages beneath the message `parent` through `link`.
pub fn listing(parent: EntityId, link: Link, query: Option<&Query>) -> Listing<EntityId, Model, Link> {
    match link {
        Link::Replies => thread::from_messages(query, |m| {
            // From the member's own state: a member whose `reply_to` moved on can linger in the
            // query's results.
            // A forward is a reply addressed to someone; its row says to whom.
            (m.reply_to().ok().flatten().map(EntityId::from) == Some(parent)).then(|| Child {
                id: m.id(),
                models: Models::one(Model::Message),
                link,
                label: String::new(),
                created: m.created().unwrap_or(0),
            })
        }),
        other => Listing::Failed(format!("a message does not follow {other:?}")),
    }
}

/// Whom a message asks, when it asks someone: a principal's handle; for a question asked of no one
/// in particular (`asks: project`), "the scope" and the name of the scope it asks, whose triage
/// agent answers it (`asks_scope`), or "the project" for one that asks no scope, which the triage
/// agent that serves the whole project answers (docs/DECISIONS.md, D122). A question from before
/// 2026-09-29 that `fwd admin migrate` has not carried over shows the word it held, `supervisor`.
pub fn addressee(m: &MessageView) -> Option<String> {
    let context: Value = m.context().map(Into::into).unwrap_or(Value::Null);
    if context[ops::ASKS_KEY].as_str() == Some(ops::ASKS_PROJECT) {
        return Some(match context[ops::ASKS_SCOPE_KEY].as_str().and_then(|s| s.parse::<EntityId>().ok()) {
            Some(scope) => format!("the scope {}", store::scope_label(scope)),
            None => "the project".to_string(),
        });
    }
    let to = context["to"].as_str()?;
    Some(to.parse::<EntityId>().map(store::handle_of).unwrap_or_else(|_| to.to_string()))
}

/// Who wrote the message and how it begins, to name it where it shows again.
pub fn name(id: EntityId) -> Option<String> {
    let m = store::message(id)?;
    let author = m.author().ok().map(|a| store::handle_of(EntityId::from(a))).unwrap_or_default();
    let text = references::plain(&ops::text(m.content()).unwrap_or_default());
    Some(format!("{author} “{}”", fmt::truncate(&text, 60)))
}

/// The lozenge mode: its author and its first words.
pub fn lozenge(id: EntityId) -> AnyView {
    lozenges::single(Target::Message(id))
}

/// The list item mode: who wrote it and the first line of what it says (references named in
/// words), "resolved" once it is.
pub fn list_item(id: EntityId) -> AnyView {
    (move || {
        let Some(m) = store::message(id) else {
            let absent = format!("{}: {}", fmt::short(id), empty::absent(Some(Model::Message)));
            return view! { <span class="list-item muted">{absent}</span> }.into_any();
        };
        let author = m.author().ok().map(|a| store::handle_of(EntityId::from(a))).unwrap_or_default();
        let text = references::plain(&ops::text(m.content()).unwrap_or_default());
        let first = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").to_string();
        let resolved = m.status().ok() == Some(MessageStatus::Resolved);
        view! {
            <span class="list-item message-item">
                <span class="author">{author}</span>
                " "
                <span class="message-first">{fmt::truncate(&first, 100)}</span>
                {resolved.then(|| view! { " " <span class="pill">"resolved"</span> })}
            </span>
        }
        .into_any()
    })
    .into_any()
}

/// The embedded mode: the message's row, its replies beneath it.
pub fn embedded(state: ThreadState, at: RowAt) -> AnyView {
    view! { <MessageRow state=state at=at/> }.into_any()
}

/// The full mode: the message's row standing alone with every reply beneath it, then what it is
/// joined to, among them the messages that mention it.
pub fn full(id: EntityId) -> AnyView {
    view! {
        <div class="detail">
            {move || session::acting().get().map(|p| {
                let hooks = Hooks { strip_above: true, ..Hooks::default() };
                thread::card_row(ThreadState::new(p.id()), id, id, Models::one(Model::Message), hooks)
            })}
            {lozenges::section(id, "References", &[])}
        </div>
    }
    .into_any()
}

/// Whether a message's row offers to edit it: to its author and to an agent that succeeded its
/// author, as the acting principal, since they alone edit a message (`rules::can_edit_message`,
/// given `successors`, the principals that took over the author's job in turn); not while its
/// author cannot be read.
pub fn offers_edit(author: Option<EntityId>, successors: &[EntityId], me: EntityId) -> bool {
    author.is_some_and(|author| rules::can_edit_message(author, successors, me).is_ok())
}

/// The principals that took over `author`'s job in turn, as the page's principals record them
/// (`routing::Successors::chain`), which a successor's offer to edit reads, and which follow a
/// succession as the page's query receives it.
fn successors_of(author: EntityId) -> Vec<EntityId> {
    routing::Successors::of(&store::principals()).chain(author)
}

#[component]
pub fn MessageRow(state: ThreadState, at: RowAt) -> impl IntoView {
    let (id, row) = (at.id, at.appearance);
    // Changes only when the message arrives or goes: each field below is read inside its own
    // closure, which redraws when the message changes.
    let message = Memo::new(move |_| store::message(id));
    let author_id = move || message.get().and_then(|m| m.author().ok()).map(EntityId::from);
    let author = move || author_id().map(store::handle_of);
    let to = move || message.get().and_then(|m| addressee(&m));
    let meta = move || {
        message.get().map(|m| {
            let ago = fmt::ago(m.created().unwrap_or(0));
            // A question's priority says how soon it wants an answer; a reply only repeats it.
            let said = match addressee(&m) {
                Some(_) => format!("{ago} · {}", m.priority().map(|p| p.to_string()).unwrap_or_default()),
                None => ago,
            };
            // Its text changed after it was sent, so a reader who saw it before knows to look again;
            // and when an agent that succeeded its author changed it, the row names that agent,
            // since the words are no longer all the author's.
            let author = m.author().ok().map(EntityId::from);
            let by = m.edited_by().ok().flatten().map(EntityId::from).filter(|by| Some(*by) != author);
            match (m.edited().ok().flatten(), by) {
                (Some(edited), Some(by)) => {
                    format!("{said} · edited by {}, {}", store::handle_of(by), fmt::clock(edited))
                }
                (Some(edited), None) => format!("{said} · edited, {}", fmt::clock(edited)),
                (None, _) => said,
            }
        })
    };
    let status = move || message.get().and_then(|m| m.status().ok());
    let open = move || status() == Some(MessageStatus::Active);
    let content = move || {
        let text = message.get().map(|m| ops::text(m.content()).unwrap_or_default()).unwrap_or_default();
        references::content(&text)
    };
    let (reply, report, me) = (state.edit(&at), state.report(row), state.me);
    let editing = state.editing(row);
    // The box opens on the text as its author would type it again: names, not ids.
    let start_edit = move || {
        let stored = message.get_untracked().map(|m| ops::text(m.content()).unwrap_or_default()).unwrap_or_default();
        state.start_edit(row, references::editable(&stored));
    };
    let text = move || {
        if editing.get() {
            thread::edit_box(state, row, EDIT)
        } else {
            view! { <p class="content">{content}</p> }.into_any()
        }
    };
    let strip = at.strip();
    let line = view! {
        <div class="msg" class:resolved=move || !open()>
            <span class="author">{author}</span>
            {move || to().map(|to| view! { <span class="muted">" to "</span> <span class="addressee">{to}</span> })}
            // So that the age and the status each read apart from what precedes them, on the line
            // and in the row's text, a space goes before each, as between every row's other
            // parts: without it a question's row read "to dnormanjust now".
            " "
            <span class="meta">{meta}</span>
            {move || status().filter(|_| !open()).map(|s| view! { " " <span class="pill">{s.to_string()}</span> })}
            " "
            {strip}
            {text}
            // Beneath the text, each highlight it names, as a step shows beneath its task.
            {crate::attachments::beneath(state, &at)}
            <span class="row-actions">
                <button class="btn subtle tiny" on:click=move |_| reply.run(())>"reply"</button>
                {move || {
                    open().then(|| {
                        let (label, title) = resolve_words(&steps::checked_by_resolving(id));
                        view! {
                            <button class="btn subtle tiny" title=title on:click=move |_| resolve(me, id, report)>{label}</button>
                        }
                    })
                }}
                {move || {
                    let successors = author_id().map(successors_of).unwrap_or_default();
                    (offers_edit(author_id(), &successors, me) && !editing.get()).then(|| {
                        view! { <button class="btn subtle tiny" on:click=move |_| start_edit()>"edit"</button> }
                    })
                }}
            </span>
        </div>
    }
    .into_any();
    thread::frame(state, at, line, Actions { editor: Some(REPLY), activate: None })
}

/// A message's reply box: what is written in it becomes a reply to the message.
const REPLY: Editor = Editor { placeholder: "your reply", send: send_reply };

/// Posts `text` as `me`'s reply to `parent`, at the priority of what it answers, as `fwd reply`
/// does.
fn send_reply(me: EntityId, parent: EntityId, text: String) -> Sending {
    Box::pin(async move {
        let priority = store::message(parent).and_then(|m| m.priority().ok()).unwrap_or(Priority::Next);
        let mc = MessageContext { reply_to: Some(parent), ..Default::default() };
        ops::post_message(&ctx(), me, &text, priority, mc).await.map_err(|e| e.to_string())
    })
}

/// The resolve button's label and title: on the first message of a step's highlight's first thread,
/// resolving checks the step on each task in `tasks` too (docs/DECISIONS.md, D99, "Checking a step
/// resolves its highlight's first thread, and resolving that thread checks the step; each control
/// says it does both"), so the button says so and names the tasks.
fn resolve_words(tasks: &[EntityId]) -> (&'static str, Option<String>) {
    if tasks.is_empty() {
        return ("resolve", None);
    }
    let names: Vec<String> =
        tasks.iter().map(|t| store::task(*t).and_then(|t| t.title().ok()).unwrap_or_else(|| fmt::short(*t))).collect();
    (
        "resolve and check the step",
        Some(format!("resolves this thread and checks its highlight's step of {}", names.join(", "))),
    )
}

/// A message's edit box: what is written in it replaces the message's text, as `fwd message edit`
/// does (`ops::edit_message`), which keeps the message's id, thread and place.
const EDIT: Editor = Editor { placeholder: "what the message should say", send: save_edit };

/// Saves `text` as `me`'s new text of `message`; a refusal (another's message, a name that fits
/// nothing, a blank text) comes back in the words `ops::edit_message` gives it.
fn save_edit(me: EntityId, message: EntityId, text: String) -> Sending {
    Box::pin(
        async move { ops::edit_message(&ctx(), me, message, &text).await.map(|()| message).map_err(|e| e.to_string()) },
    )
}

fn resolve(me: EntityId, message: EntityId, report: Callback<String>) {
    spawn_local(async move {
        match ops::resolve_message(&ctx(), me, message).await {
            Ok(_) => session::wrote(me),
            Err(e) => {
                report.try_run(e.to_string());
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::offers_edit;
    use ankurah::EntityId;

    #[test]
    fn its_author_and_the_agents_that_succeeded_its_author_are_offered_to_edit_a_message() {
        let [me, you, them] = [1, 2, 3].map(|b| EntityId::from_bytes([b; 32]));
        assert!(offers_edit(Some(me), &[], me), "one's own message");
        assert!(!offers_edit(Some(you), &[], me), "someone else's");
        assert!(offers_edit(Some(you), &[me], me), "one whose author one succeeded");
        assert!(offers_edit(Some(you), &[them, me], me), "one whose author one succeeded through another");
        assert!(!offers_edit(Some(you), &[them], me), "one whose author another agent succeeded");
        assert!(!offers_edit(None, &[me], me), "a message whose author cannot be read");
    }
}
