//! The Ask box at the top of the Inbox and Questions pages, for recording a question that belongs
//! to no task, anchor or scope, so it stays in sight on the Questions page of whomever it asks until
//! someone resolves it. It posts what `fwd ask` posts: a message that says whom it asks and nothing
//! it is about, so the routing service notifies whom it asks. By default it asks no one in
//! particular, of the scope the header's scope filter chooses (`scope_filter.rs`), which the
//! scope's triage agent answers, else the one that serves the whole project (`asks: project`, with
//! the scope as `asks_scope`; docs/DECISIONS.md, D122); with the header on "all scopes" or
//! "no scope", or with "the project" chosen, it asks of no scope, which the agent that serves the
//! whole project answers. While no triage agent answers for what it asks, the question waits on
//! every Questions page, as a line beneath the box then says. Else it asks the principal of the
//! project chosen, by its id in `to` (docs/design/web-ui.md, "The Ask box"; until 2026-09-29 it
//! asked the supervisor role, and until 2026-09-30 the project's one triage agent).

use crate::mentions::{MentionList, Mentions};
use crate::scope_filter::ScopeChoice;
use crate::session::{self, ctx};
use crate::{fmt, keys, store};
use ankurah::EntityId;
use fwd_core::model::Priority;
use fwd_core::ops::{self, MessageContext};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

/// What the box offers for a question asked of no scope, which the triage agent that serves the
/// whole project answers.
pub const THE_PROJECT: &str = "the project, which the triage agent that serves it whole answers";

/// What the box offers for a question asked of the scope `title`, which its triage agent answers,
/// else the one that serves the whole project.
fn the_scope(title: &str) -> String {
    format!("the scope {title}, which its triage agent answers")
}

/// The line beneath the box while it asks no one in particular and no triage agent answers for
/// what it asks: of the scope `title`, or of no scope.
fn waits(title: Option<&str>) -> String {
    match title {
        Some(title) => format!(
            "No triage agent serves the scope {title}, and none serves the whole project: a question asked of it waits on the Questions page until one does."
        ),
        None => "No triage agent serves the whole project: a question asked of no scope waits on the Questions page until one does."
            .to_string(),
    }
}

/// Whom the box asks: no one in particular, of the header's scope or of none, or a principal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Whom {
    /// No one in particular, of the scope the header chooses (of no scope while it chooses none).
    TheScope,
    /// No one in particular, of no scope.
    TheProject,
    /// The principal of this id.
    Principal(EntityId),
}

/// The value the select holds for the scope, and for the project: words no principal's id can be.
const SCOPE: &str = "scope";
const PROJECT: &str = "project";

#[component]
pub fn AskBox(me: EntityId, scope: Memo<Option<ScopeChoice>>) -> impl IntoView {
    let text = RwSignal::new(String::new());
    let whom = RwSignal::new(Whom::TheScope);
    let priority = RwSignal::new(Priority::Later);
    let error = RwSignal::new(None::<String>);
    let field = NodeRef::<leptos::html::Textarea>::new();
    let mentions = Mentions::new(field);
    // The scope a question asked of no one asks: the header's, while the box asks it.
    let asked_scope = move || match whom.get() {
        Whom::TheScope => scope.get().and_then(ScopeChoice::scope),
        Whom::TheProject | Whom::Principal(_) => None,
    };
    let send = move || {
        let question = text.get_untracked();
        if question.trim().is_empty() {
            return;
        }
        let mc = match whom.get_untracked() {
            Whom::Principal(to) => MessageContext { to: Some(to.to_base64()), ..Default::default() },
            Whom::TheScope | Whom::TheProject => {
                MessageContext { asks_project: true, asks_scope: untrack(asked_scope), ..Default::default() }
            }
        };
        let priority = priority.get_untracked();
        text.set(String::new());
        spawn_local(async move {
            match ops::post_message(&ctx(), me, question.trim(), priority, mc).await {
                Ok(_) => {
                    session::wrote(me);
                    error.set(None);
                }
                Err(e) => {
                    error.set(Some(e.to_string()));
                    // Back in the box, unless something new was typed there meanwhile.
                    text.update(|t| {
                        if t.is_empty() {
                            *t = question;
                        }
                    });
                }
            }
        });
    };
    let on_keydown = move |ev: web_sys::KeyboardEvent| {
        if mentions.key(&ev, &text.get_untracked(), |t| text.set(t)) {
            return;
        }
        if keys::is_send(&ev) {
            ev.prevent_default();
            send();
        }
    };
    // The project's principals, grouped again only when the groups change, and not at every write
    // that stamps its writer's principal (`fwd_core::dormancy::wrote`).
    let groups = Memo::new(move |_| {
        store::principal_groups(match whom.get() {
            Whom::Principal(id) => Some(id),
            _ => None,
        })
    });
    // The header's scope first, since the box asks it by default, while the header chooses one;
    // then the project; then the project's principals.
    let recipients = move || {
        let chosen = whom.get();
        let mut options = Vec::new();
        if let Some(scope) = scope.get().and_then(ScopeChoice::scope) {
            let label = the_scope(&store::scope_label(scope));
            options.push(view! { <option value=SCOPE selected=chosen == Whom::TheScope>{label}</option> }.into_any());
        }
        let project = matches!(chosen, Whom::TheProject) || scope.get().and_then(ScopeChoice::scope).is_none();
        let project = project && !matches!(chosen, Whom::Principal(_));
        options.push(view! { <option value=PROJECT selected=project>{THE_PROJECT}</option> }.into_any());
        let principal = match chosen {
            Whom::Principal(id) => Some(id),
            _ => None,
        };
        options.extend(fmt::principal_options(groups.get(), principal));
        options
    };
    let choose = move |value: String| {
        whom.set(match value.as_str() {
            SCOPE => Whom::TheScope,
            PROJECT => Whom::TheProject,
            id => id.parse::<EntityId>().map(Whom::Principal).unwrap_or(Whom::TheProject),
        })
    };
    let waiting = move || {
        if matches!(whom.get(), Whom::Principal(_)) || !store::standing_loaded() {
            return None;
        }
        let asked = asked_scope();
        store::front_line(asked).is_none().then(|| {
            let line = waits(asked.map(store::scope_label).as_deref());
            view! { <p class="ask-waits">{line}</p> }
        })
    };
    view! {
        <section class="ask-box">
            <textarea
                placeholder="Ask a question that belongs to no task. Enter sends; Shift+Enter starts a new line."
                node_ref=field
                prop:value=move || text.get()
                on:input=move |ev| {
                    text.set(event_target_value(&ev));
                    mentions.typed();
                }
                on:keyup=move |_| mentions.moved()
                on:click=move |_| mentions.moved()
                on:blur=move |_| mentions.left()
                on:keydown=on_keydown
            ></textarea>
            <MentionList mentions=mentions text=text.into() write=Callback::new(move |t| text.set(t))/>
            <div class="actions">
                <label class="field">
                    "to "
                    <select on:change=move |ev| choose(event_target_value(&ev))>{recipients}</select>
                </label>
                <label class="field">
                    "priority "
                    <select on:change=move |ev| {
                        priority.set(if event_target_value(&ev) == "now" { Priority::Now } else { Priority::Later })
                    }>
                        <option value="later" selected=true>"later"</option>
                        <option value="now">"now"</option>
                    </select>
                </label>
                <button class="btn primary" on:click=move |_| send()>"Ask"</button>
                {move || error.get().map(|e| view! { <span class="error">{e}</span> })}
            </div>
            {waiting}
        </section>
    }
}
