//! The scopes: the areas of work, which of them are active, and on which branches their work
//! happens. One line per scope (its name, its short id, its status and branches, its intention on
//! hover), active scopes first, then by name, and nothing beneath: a scope leads to
//! the Tasks page filtered by it (Daniel, 2026-09-26: "I don't think the scopes listed in the
//! scopes tab should recurse their tasks at all. when you click a scope, it should take you to the
//! tasks tab with the filter set to that scope."). Beside the list, as the Tasks and Decisions
//! pages have theirs, a panel makes a scope or changes one (`scope_panel.rs`; `DECISIONS.md`, "A
//! scope is created and edited in a side panel opened from an Edit control; its name still leads
//! to the filtered Tasks page"): the "New scope" button above the list opens it empty, and each
//! line's Edit control opens it on that scope. A line named the scope's owner and how long since
//! its heartbeat until 2026-09-29, when scopes lost both (docs/DECISIONS.md, D108, "A scope has no
//! owner: scopes are not assigned to principals"): anyone works in any scope, and who does a task
//! is its assignee.

use crate::empty::{Empty, Nothing};
use crate::fmt::CopyId;
use crate::referents::Models;
use crate::route::{self, Route, ScopePanel};
use crate::scope_panel::{self, Draft};
use crate::thread::Model;
use crate::{modes, store};
use ankurah::EntityId;
use fwd_core::model::ScopeStatus;
use fwd_core::ops;
use leptos::prelude::*;
use serde_json::Value;

/// What the list shows of one scope.
struct Line {
    id: EntityId,
    name: String,
    status: Option<ScopeStatus>,
    branches: String,
    intention: String,
}

/// The Scopes page: the list, and beside it (beneath it on a narrow page) the panel `panel` names,
/// from the route.
#[component]
pub fn Scopes(panel: Memo<Option<ScopePanel>>) -> impl IntoView {
    // What was typed for a new scope, kept while the panel shows another scope and back.
    let new_scope = Draft::untouched();
    let lines = move || {
        let mut scopes: Vec<(bool, String, Line)> = store::scopes()
            .iter()
            .map(|s| {
                let status = s.status().ok();
                let name = s.title().unwrap_or_default();
                let line = Line {
                    id: s.id(),
                    status,
                    branches: ops::scope_branches(&s.context().map(Value::from).unwrap_or(Value::Null)).join(", "),
                    intention: ops::text(s.intention()).unwrap_or_default(),
                    name: name.clone(),
                };
                (status != Some(ScopeStatus::Active), name.to_lowercase(), line)
            })
            .collect();
        scopes.sort_by(|a, b| (a.0, &a.1, a.2.id).cmp(&(b.0, &b.1, b.2.id)));
        scopes.into_iter().map(|(_, _, line)| line).collect::<Vec<_>>()
    };
    view! {
        <div class="split" class:with-detail=move || panel.get().is_some()>
            <section class="page list-pane scopes">
                <header class="page-head">
                    <h2>"Scopes"</h2>
                    <button class="btn primary" on:click=move |_| route::go(Route::Scopes(Some(ScopePanel::New)))>
                        "New scope"
                    </button>
                    <span class="muted">"a scope's name leads to its tasks; Edit changes it"</span>
                </header>
                {move || {
                    let lines = lines();
                    if lines.is_empty() {
                        return view! { <Empty nothing=Nothing::Scopes/> }.into_any();
                    }
                    view! {
                        <table class="list scopes">
                            <thead>
                                <tr>
                                    <th>"Scope"</th><th>"Id"</th><th>"Status"</th><th>"Branches"</th><th></th>
                                </tr>
                            </thead>
                            <tbody>{lines.into_iter().map(|line| scope_line(line, panel)).collect::<Vec<_>>()}</tbody>
                        </table>
                    }
                    .into_any()
                }}
            </section>
            {move || {
                panel.get().map(|open| {
                    let shown = match open {
                        ScopePanel::New => scope_panel::new_scope(new_scope),
                        ScopePanel::Edit(id) => modes::full(id, &Models::one(Model::Scope)),
                    };
                    view! { <aside class="detail-pane scope-detail">{shown}</aside> }
                })
            }}
        </div>
    }
}

/// One scope's line; a click on it, or on its name, shows the tasks worked in it, and its Edit
/// control opens its panel.
fn scope_line(line: Line, panel: Memo<Option<ScopePanel>>) -> impl IntoView {
    let id = line.id;
    let tasks = Route::scope_tasks(id);
    let hash = tasks.hash();
    let edit = move |ev: leptos::ev::MouseEvent| {
        // The line's own click leads to the tasks.
        ev.stop_propagation();
        route::go(Route::Scopes(Some(ScopePanel::Edit(id))));
    };
    view! {
        <tr class="row" title=line.intention class:selected=move || panel.get() == Some(ScopePanel::Edit(id))
            on:click=move |_| route::go(tasks.clone())>
            // Each cell names its field, which a narrow page shows beside it once the rows stack.
            <td class="title"><a href=hash>{line.name}</a></td>
            <td data-label="id"><CopyId id=id/></td>
            <td data-label="status">
                {line.status.map(|s| view! { <span class=format!("pill scope-{s}")>{s.to_string()}</span> })}
            </td>
            <td data-label="branches" class="mono">{line.branches}</td>
            <td class="row-edit">
                <button class="btn subtle tiny" title="Change its name, intention, repository, branches or status"
                    on:click=edit>"Edit"</button>
            </td>
        </tr>
    }
}
