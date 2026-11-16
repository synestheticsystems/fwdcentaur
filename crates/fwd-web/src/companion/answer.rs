//! Answering a question that waits on the person, from its opened record: the document's Answer
//! action, which asks for the person's words with its own prompt, made as a reply in the question's
//! thread through the app's own write path, as the Inbox's reply box makes one (`ops::post_message`,
//! at the priority of what it answers, as the person the page acts as), never by running the
//! command the action names for the pane. Enter sends and Shift+Enter starts a new line, as in every
//! box on the page that sends something. The box empties only once the daemon has the reply
//! (`session::while_writing`), and the question then leaves Waiting On You as the view reads it
//! again; until then the box keeps its words, and a reply that fails, or that a drop of the
//! connection left unconfirmed, says so beside them. The page, not the box, keeps a reply from the
//! moment it is sent (`replies.rs`), so that one the daemon has not confirmed outlives the
//! question's row.

use super::record::PageLink;
use super::replies::{Replies, Reply};
use super::Connected;
use crate::keys;
use crate::session::{self, graph};
use ankurah::EntityId;
use anyhow::{bail, Context as _};
use fwd_core::model::MessageView;
use fwd_core::ops::{self, MessageContext};
use fwd_core::project_graph::Followed;
use fwd_core::session_view::Input;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;

/// The box that answers the question `question`, titled `title`, with the document's action, its
/// `label` and what it asks for, `input`, open from the start when `writing`, with the link to the
/// question's page, `link`, beside its button.
#[component]
pub fn Answer(
    question: EntityId,
    title: Signal<String>,
    label: String,
    input: Input,
    writing: bool,
    link: Signal<Option<String>>,
) -> impl IntoView {
    let replies = expect_context::<RwSignal<Replies>>();
    // A reply sent from the question's row as drawn before keeps its words in the box drawn now.
    let sent = replies.with_untracked(|r| r.get(question).map(|reply| reply.words.clone()));
    let writing = RwSignal::new(writing || sent.is_some());
    let words = RwSignal::new(sent.unwrap_or_default());
    let sending = Memo::new(move |_| replies.with(|r| r.get(question).is_some_and(Reply::sending)));
    let why = Memo::new(move |_| replies.with(|r| r.get(question)?.ended.as_ref().map(|why| format!("{why:#}"))));
    // Nothing is sent while the connection to the daemon is lost; the words stay in the box.
    let Connected(connected) = expect_context::<Connected>();
    let field_id = format!("answer-{}", question.to_base64());
    let submit = move || {
        let text = words.get_untracked().trim().to_string();
        if text.is_empty() || !connected.get_untracked() {
            return;
        }
        send(replies, question, title.get_untracked(), link.get_untracked(), text.clone(), move || {
            // Unless something new was written meanwhile.
            words.try_update(|written| {
                if written.trim() == text {
                    written.clear();
                }
            });
        });
    };
    let start = {
        let field_id = field_id.clone();
        move |_| {
            writing.set(true);
            let field_id = field_id.clone();
            request_animation_frame(move || focus(&field_id));
        }
    };
    let on_key = move |ev: web_sys::KeyboardEvent| {
        if keys::is_send(&ev) {
            ev.prevent_default();
            submit();
        }
    };
    view! {
        <div class="answer">
            {move || {
                if !writing.get() {
                    return view! {
                        <div class="record-footer">
                            <button type="button" class="btn" on:click=start.clone()>{label.clone()}</button>
                            <PageLink link=link/>
                        </div>
                    }
                    .into_any();
                }
                let field = if input.multiline {
                    view! {
                        <textarea id=field_id.clone() rows=3 prop:value=move || words.get()
                            on:input=move |ev| words.set(event_target_value(&ev)) on:keydown=on_key></textarea>
                    }
                    .into_any()
                } else {
                    view! {
                        <input id=field_id.clone() type="text" prop:value=move || words.get()
                            on:input=move |ev| words.set(event_target_value(&ev))/>
                    }
                    .into_any()
                };
                view! {
                    <form class="answer-form" on:submit=move |ev| {
                        ev.prevent_default();
                        submit();
                    }>
                        <label for=field_id.clone()>{input.prompt.clone()}</label>
                        {field}
                        <div class="record-footer">
                            <button type="submit" class="btn primary" disabled=move || sending.get() || !connected.get()>
                                {label.clone()}
                            </button>
                            <PageLink link=link/>
                        </div>
                        {move || why.get().map(|why| view! { <p class="error" role="status">{why}</p> })}
                    </form>
                }
                .into_any()
            }}
        </div>
    }
}

/// Sends `words` as the person's reply to the question `question`, titled `title`, whose page is at
/// `link`, which `replies` keeps until the daemon confirms it or the person dismisses it, and runs
/// `made` once the daemon has it. Nothing is sent while a reply to the question is being sent, or
/// from a page that acts as no one.
pub(super) fn send(
    replies: RwSignal<Replies>,
    question: EntityId,
    title: String,
    link: Option<String>,
    words: String,
    made: impl FnOnce() + 'static,
) {
    let Some(me) = session::me() else { return };
    if !replies.try_update(|r| r.start(question, title, link, words.clone())).unwrap_or(false) {
        return;
    }
    spawn_local(async move {
        let outcome = reply(me, question, &words).await;
        let confirmed = outcome.is_ok();
        replies.try_update(|r| r.end(question, outcome));
        if confirmed {
            session::wrote(me);
            made();
        }
    });
}

/// Posts `text` as `me`'s reply in the thread of `question`, at the priority of what it answers,
/// as the Inbox's reply box posts one, and succeeds only once the daemon has it. A question the
/// page cannot read, or whose priority it cannot, is refused before anything is sent, since a reply
/// at a priority made up for it would misstate it.
async fn reply(me: EntityId, question: EntityId, text: &str) -> anyhow::Result<()> {
    const UNREAD: &str = "the question this answers could not be read, so nothing was sent";
    let priority = match graph().follow::<MessageView>(question).await.context(UNREAD)? {
        Followed::Held { found, .. } => found.priority().context(UNREAD)?,
        Followed::NotHeld => bail!("this page finds no such question, so nothing was sent"),
    };
    let mc = MessageContext { reply_to: Some(question), ..Default::default() };
    session::while_writing(ops::post_message(&graph(), me, text, priority, mc)).await?;
    Ok(())
}

/// Puts the keyboard in the box whose id is `id`.
fn focus(id: &str) {
    let field = web_sys::window().and_then(|w| w.document()).and_then(|d| d.get_element_by_id(id));
    if let Some(field) = field.and_then(|f| f.dyn_into::<web_sys::HtmlElement>().ok()) {
        let _ = field.focus();
    }
}
