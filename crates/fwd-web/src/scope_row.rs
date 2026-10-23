//! A scope as a row: its name, its short id (a click copies the whole id), its status and its
//! branches, with its intention on hover, and a count of the tasks worked in it as a lozenge
//! (`lozenges.rs`). Nothing hangs beneath it: the tasks worked in a scope are the Tasks page
//! filtered by it (Daniel, 2026-09-26: "I don't think the scopes listed in the scopes tab should
//! recurse their tasks at all"), where a task's scope lozenge leads. Its list item is its name and
//! status on one line; the Tasks page shows that line with its references above the tasks it is
//! filtered to ([`heading`]). Its full mode is its panel on the Scopes page (`scope_panel.rs`).
//! Until 2026-09-29 the row and the list item named the scope's owner too, and the row how long
//! since its heartbeat: a scope has neither now (docs/DECISIONS.md, D108, "A scope has no owner:
//! scopes are not assigned to principals").

use crate::fmt::CopyId;
use crate::lozenges::{self, Target};
use crate::route::{Route, ScopePanel};
use crate::thread::{self, Actions, Link, Model, RowAt, ThreadState};
use crate::{empty, fmt, store};
use ankurah::EntityId;
use fwd_core::ops;
use leptos::prelude::*;
use serde_json::Value;

/// The links a scope follows to the messages beneath it: none.
pub const FOLLOWS: &[Link] = &[];

/// The scope's name, to name it where it shows again.
pub fn name(id: EntityId) -> Option<String> {
    Some(format!("scope {}", store::scope(id)?.title().unwrap_or_default()))
}

/// The lozenge mode: the scope's name in its green outline.
pub fn lozenge(id: EntityId) -> AnyView {
    lozenges::single(Target::Scope(id))
}

/// The list item mode: its name (leading to the tasks worked in it) and its status.
pub fn list_item(id: EntityId) -> AnyView {
    (move || {
        let Some(s) = store::scope(id) else {
            let absent = format!("{}: {}", fmt::short(id), empty::absent(Some(Model::Scope)));
            return view! { <span class="list-item muted">{absent}</span> }.into_any();
        };
        view! {
            <span class="list-item scope-item" title=ops::text(s.intention()).unwrap_or_default()>
                <a class="scope-name" href=Route::scope_tasks(id).hash()>{s.title().unwrap_or_default()}</a>
                " "
                {s.status().ok().map(|st| view! { <span class=format!("pill scope-{st}")>{st.to_string()}</span> })}
            </span>
        }
        .into_any()
    })
    .into_any()
}

/// The embedded mode: the scope's row.
pub fn embedded(state: ThreadState, at: RowAt) -> AnyView {
    view! { <ScopeRow state=state at=at/> }.into_any()
}

/// What heads the Tasks page filtered to the scope: its line, its intention, what it is joined to
/// (the tasks worked in it, counted, and the messages naming it), and a link to its panel.
pub fn heading(id: EntityId) -> AnyView {
    let intention = move || store::scope(id).map(|s| ops::text(s.intention()).unwrap_or_default()).unwrap_or_default();
    view! {
        <div class="scope-full">
            {list_item(id)}
            " "
            <CopyId id=id/>
            " "
            <a class="scope-edit" href=Route::Scopes(Some(ScopePanel::Edit(id))).hash()
                title="its name, intention, repository, branches and status, on the Scopes page">"edit"</a>
            {lozenges::strip(id, None)}
            {move || {
                let text = intention();
                (!text.trim().is_empty()).then(|| view! { <p class="muted">{text}</p> })
            }}
        </div>
    }
    .into_any()
}

#[component]
pub fn ScopeRow(state: ThreadState, at: RowAt) -> impl IntoView {
    let id = at.id;
    // Changes only when the scope arrives or goes; the fields below redraw on their own.
    let scope = Memo::new(move |_| store::scope(id));
    let title = move || scope.get().and_then(|s| s.title().ok()).unwrap_or_else(|| empty::absent(Some(Model::Scope)));
    let status = move || scope.get().and_then(|s| s.status().ok());
    let branches = move || {
        scope.get().map(|s| ops::scope_branches(&s.context().map(Value::from).unwrap_or(Value::Null)).join(", "))
    };
    let intention = move || scope.get().map(|s| ops::text(s.intention()).unwrap_or_default()).unwrap_or_default();
    let strip = at.strip();
    let line = view! {
        <div class="scope-ref" title=intention>
            <span class="scope-name">{title}</span>
            " "
            <CopyId id=id/>
            " "
            {move || status().map(|s| view! { <span class=format!("pill scope-{s}")>{s.to_string()}</span> })}
            " "
            <span class="mono">{branches}</span>
            " "
            {strip}
        </div>
    }
    .into_any();
    thread::frame(state, at, line, Actions::default())
}
