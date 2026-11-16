//! Answering a question that waits on the person, from its opened record: the document's Answer
//! action, which asks for the person's words with its own prompt, made as a reply in the question's
//! thread through the app's own write path, as the Inbox's reply box makes one (`ops::post_message`,
//! at the priority of what it answers, as the person the page acts as), never by running the
//! command the action names for the pane. Enter sends and Shift+Enter starts a new line, as in every
//! box on the page that sends something. Once the reply is made the question leaves Waiting On You
//! as the view reads it again; until then the box keeps its words, and a reply that fails says why
//! beside them.

use super::record::PageLink;
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

/// The box that answers the question `question` with the document's action, its `label` and what
/// it asks for, `input`, open from the start when `writing`, with the link to the question's page,
/// `link`, beside its button.
#[component]
pub fn Answer(
    question: EntityId,
    label: String,
    input: Input,
    writing: bool,
    link: Signal<Option<String>>,
) -> impl IntoView {
    let writing = RwSignal::new(writing);
    let words = RwSignal::new(String::new());
    let sending = RwSignal::new(false);
    let error = RwSignal::new(None::<anyhow::Error>);
    // Nothing is sent while the connection to the daemon is lost; the words stay in the box.
    let Connected(connected) = expect_context::<Connected>();
    let field_id = format!("answer-{}", question.to_base64());
    let send = move || {
        let text = words.get_untracked().trim().to_string();
        let ready = !text.is_empty() && !sending.get_untracked() && connected.get_untracked();
        let Some(me) = session::me().filter(|_| ready) else { return };
        sending.set(true);
        error.set(None);
        spawn_local(async move {
            match reply(me, question, &text).await {
                Ok(()) => {
                    session::wrote(me);
                    // Unless something new was written meanwhile.
                    words.try_update(|written| {
                        if written.trim() == text {
                            written.clear();
                        }
                    });
                }
                Err(why) => {
                    error.try_set(Some(why));
                }
            }
            sending.try_set(false);
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
            send();
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
                        send();
                    }>
                        <label for=field_id.clone()>{input.prompt.clone()}</label>
                        {field}
                        <div class="record-footer">
                            <button type="submit" class="btn primary" disabled=move || sending.get() || !connected.get()>
                                {label.clone()}
                            </button>
                            <PageLink link=link/>
                        </div>
                        {move || {
                            error.with(|why| {
                                why.as_ref().map(|why| view! { <p class="error" role="status">{format!("{why:#}")}</p> })
                            })
                        }}
                    </form>
                }
                .into_any()
            }}
        </div>
    }
}

/// Posts `text` as `me`'s reply in the thread of `question`, at the priority of what it answers,
/// as the Inbox's reply box posts one. A question the page cannot read, or whose priority it
/// cannot, is refused before anything is sent, since a reply at a priority made up for it would
/// misstate it.
async fn reply(me: EntityId, question: EntityId, text: &str) -> anyhow::Result<()> {
    const UNREAD: &str = "the question this answers could not be read, so nothing was sent";
    let priority = match graph().follow::<MessageView>(question).await.context(UNREAD)? {
        Followed::Held { found, .. } => found.priority().context(UNREAD)?,
        Followed::NotHeld => bail!("this page finds no such question, so nothing was sent"),
    };
    let mc = MessageContext { reply_to: Some(question), ..Default::default() };
    ops::post_message(&graph(), me, text, priority, mc).await?;
    Ok(())
}

/// Puts the keyboard in the box whose id is `id`.
fn focus(id: &str) {
    let field = web_sys::window().and_then(|w| w.document()).and_then(|d| d.get_element_by_id(id));
    if let Some(field) = field.and_then(|f| f.dyn_into::<web_sys::HtmlElement>().ok()) {
        let _ = field.focus();
    }
}
