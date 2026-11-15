//! A Claude Code session's companion view, `#/sessions/<session id>`: one session's page, opened
//! beside its chat in the desktop's browser pane (380 to 720 px wide) or in a browser, so that the
//! person stays aligned with what the session works toward and answers it from there (Daniel,
//! 2026-11-15, D226: "Lets call this the session companion view. Presumably it should be built into
//! the main fwd app, but minus all the general navigation, and highly focused for this one
//! session."). It shows what `fwd session view` gives the pane beside the chat, made by the same
//! reads (`fwd_core::session_view`), under the same title, with none of the app's navigation.

use crate::app::NewerBuild;
use crate::empty;
use fwd_core::session_view::TITLE;
use leptos::prelude::*;

/// The companion view of the Claude Code session `session`.
#[component]
pub fn Companion(session: String) -> impl IntoView {
    let _ = session;
    view! {
        <div class="companion">
            <NewerBuild/>
            <header class="companion-header">
                <Title title=TITLE.to_string()/>
            </header>
            <main class="companion-content">
                <p class="muted">{empty::LOADING}</p>
            </main>
        </div>
    }
}

/// The view's title, its first word in the app's mark where that is the app's name, as the app's
/// header shows it.
#[component]
fn Title(title: String) -> impl IntoView {
    match title.strip_prefix("fwd ") {
        Some(rest) => view! {
            <h1>
                <span class="brand"><span class="mark">"fwd"</span></span>
                <span>{rest.to_string()}</span>
            </h1>
        }
        .into_any(),
        None => view! { <h1>{title}</h1> }.into_any(),
    }
}
