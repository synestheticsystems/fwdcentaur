//! A highlight's own page, `#/anchors/<id>`, its full mode. In the browser a highlight has a page
//! of its own that shows only the highlighted text, not the code around it (Daniel, 2026-09-28:
//! "maybe the browser experience could be more basic, like ... having it have its own page that
//! includes only the highlighted text, but not the surrounding code"): where it is, `path:lines`,
//! then the basis, as every name of a highlight begins (fwd-core's `anchor_name`); the highlighted
//! lines; what it is joined to under "References" (the tasks it is a step of, the messages that
//! name it, the tasks concerning it); then its threads as the usual rows (`thread.rs`), beneath
//! the highlight's own row, whose box opens a new thread. In the VS Code webview the same click
//! that leads here reveals the lines in the editor instead (`lozenges::follow`).

use crate::empty::{Empty, Nothing};
use crate::fmt::CopyId;
use crate::referents::Models;
use crate::thread::{self, Hooks, Model, ThreadState};
use crate::{anchor_row, fmt, lozenges, session, store};
use ankurah::EntityId;
use leptos::prelude::*;

/// The page the route `#/anchors/<id>` shows.
#[component]
pub fn AnchorPage(id: Memo<Option<EntityId>>) -> impl IntoView {
    view! {
        <section class="page anchor-page">
            {move || match id.get() {
                Some(id) => crate::modes::full(id, &Models::one(Model::Anchor)),
                None => view! { <p class="empty">"No highlight named."</p> }.into_any(),
            }}
        </section>
    }
}

/// The full mode: everything the highlight's page shows.
pub fn full(id: EntityId) -> AnyView {
    view! { <AnchorDetail id=id/> }.into_any()
}

#[component]
fn AnchorDetail(id: EntityId) -> impl IntoView {
    // Everything above its references and threads, redrawn when the anchor arrives.
    let summary = move || {
        let Some(a) = store::anchor(id) else {
            return view! { <Empty nothing=Nothing::Highlight/> }.into_any();
        };
        let shown = anchor_row::Shown::of(&a);
        let made_by = a.created_by().ok().map(|p| store::handle_of(EntityId::from(p))).unwrap_or_default();
        let made = a.created().unwrap_or(0);
        view! {
            <header class="detail-head">
                <h3 class="mono">{shown.name.place.clone()}</h3>
                <div class="pills">
                    <CopyId id=id/>
                    <span class="pill basis" title=anchor_row::VERSION_TITLE>{shown.name.version.clone()}</span>
                </div>
                <div class="muted">"highlighted by " {made_by} " " {fmt::ago(made)}</div>
            </header>
            {anchor_row::text_view(&shown)}
        }
        .into_any()
    };
    view! {
        <div class="detail">
            {summary}
            // Outside the summary, so a group opened here stays open.
            {lozenges::section(id, "References", &[])}
            <section class="prose">
                <h4>"Threads"</h4>
                // A fresh thread per acting principal: comments and resolutions are theirs.
                {move || session::acting().get().map(|p| {
                    // The page shows the lines and the references above, so the row leaves them
                    // out; its box, always there, opens a new thread on the highlight.
                    let hooks = Hooks { strip_above: true, reply_box: Some(Signal::derive(|| true)), ..Hooks::default() };
                    thread::card_row(ThreadState::new(p.id()), id, id, Models::one(Model::Anchor), hooks)
                })}
            </section>
        </div>
    }
}
