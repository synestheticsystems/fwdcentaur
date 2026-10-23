//! The box beneath a panel's thread (a task's on the Tasks page, a decision's on the Decisions
//! page) that comments on the thing the panel shows, so a comment needs no row chosen first.
//! Enter sends, Shift+Enter starts a new line, as in every box on the page that sends something;
//! an `@` opens the mention search (`mentions.rs`).

use crate::keys;
use crate::mentions::{MentionList, Mentions};
use crate::session::{self, ctx};
use ankurah::EntityId;
use fwd_core::model::Priority;
use fwd_core::ops::{self, MessageContext};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

/// What a comment box comments on.
#[derive(Clone, Copy)]
pub enum Subject {
    Task(EntityId),
    Decision(EntityId),
}

impl Subject {
    fn context(self) -> MessageContext {
        match self {
            Subject::Task(id) => MessageContext { task_id: Some(id), ..Default::default() },
            Subject::Decision(id) => MessageContext { decision_id: Some(id), ..Default::default() },
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            Subject::Task(_) => "comment on this task: Enter sends, Shift+Enter starts a new line",
            Subject::Decision(_) => "comment on this decision: Enter sends, Shift+Enter starts a new line",
        }
    }
}

#[component]
pub fn CommentBox(about: Subject) -> impl IntoView {
    let text = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let field = NodeRef::<leptos::html::Textarea>::new();
    let mentions = Mentions::new(field);
    let send = move || {
        let comment = text.get_untracked().trim().to_string();
        let Some(me) = session::me().filter(|_| !comment.is_empty()) else { return };
        text.set(String::new());
        spawn_local(async move {
            match ops::post_message(&ctx(), me, &comment, Priority::Next, about.context()).await {
                Ok(_) => {
                    session::wrote(me);
                    error.set(None);
                }
                Err(e) => {
                    error.set(Some(e.to_string()));
                    // Back in the box, unless something new was written there meanwhile.
                    text.update(|t| {
                        if t.is_empty() {
                            *t = comment;
                        }
                    });
                }
            }
        });
    };
    view! {
        <div class="reply-box">
            <textarea placeholder=about.placeholder() node_ref=field
                prop:value=move || text.get()
                on:input=move |ev| {
                    text.set(event_target_value(&ev));
                    mentions.typed();
                }
                on:keyup=move |_| mentions.moved()
                on:click=move |_| mentions.moved()
                on:blur=move |_| mentions.left()
                on:keydown=move |ev| {
                    if mentions.key(&ev, &text.get_untracked(), |t| text.set(t)) {
                        return;
                    }
                    if keys::is_send(&ev) {
                        ev.prevent_default();
                        send();
                    }
                }></textarea>
            <MentionList mentions=mentions text=text.into() write=Callback::new(move |t| text.set(t))/>
            <div class="actions">
                <button class="btn primary" on:click=move |_| send()>"Comment"</button>
                {move || error.get().map(|e| view! { <span class="error">{e}</span> })}
            </div>
        </div>
    }
}
