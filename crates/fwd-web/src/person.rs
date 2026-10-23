//! A person or an agent (a principal) in the four modes. Its lozenge is its handle, with its role
//! beside an agent's ("lead", "triage", "implementer", "reviewer", or "agent" for one with no
//! role), and its
//! preview gives the display name and the role; it leads nowhere yet, since there is no page for a
//! person, and the preview says so. Its list item is one line (handle, display name, role), its
//! row the same line with nothing beneath (no conversation hangs beneath a person), and its full
//! mode that line with the tasks assigned to it. A task's assignee, a `[[<id>]]` naming a
//! principal and an `@handle` in older text draw as its lozenge.

use crate::lozenges::{self, Target};
use crate::referents::{Mode, Models};
use crate::thread::{self, Actions, Link, Model, RowAt, ThreadState};
use crate::{empty, fmt, modes, store};
use ankurah::EntityId;
use fwd_core::model::{AgentRole, PrincipalKind, PrincipalView};
use fwd_core::ops;
use leptos::prelude::*;

/// The links a principal follows to the messages beneath it: none.
pub const FOLLOWS: &[Link] = &[];

/// What a principal is: "person", or an agent's role, or "agent" for one with none. An agent's
/// role is a default, not a rule (docs/design/data-model.md, "Rights"), so this only describes.
pub fn role(p: &PrincipalView) -> &'static str {
    match (p.kind().ok(), ops::role_of(p)) {
        (Some(PrincipalKind::Human), _) => "person",
        (_, Some(AgentRole::Lead)) => "lead",
        (_, Some(AgentRole::Triage)) => "triage",
        (_, Some(AgentRole::Implementer)) => "implementer",
        (_, Some(AgentRole::Reviewer)) => "reviewer",
        (_, None) => "agent",
    }
}

/// The handle, to name a principal where it shows again.
pub fn name(id: EntityId) -> Option<String> {
    store::principal(id).and_then(|p| p.handle().ok())
}

/// The lozenge mode: the handle, the role beside an agent's.
pub fn lozenge(id: EntityId) -> AnyView {
    lozenges::single(Target::Principal(id))
}

/// The list item mode: "impl-alice Alice's agent implementer".
pub fn list_item(id: EntityId) -> AnyView {
    (move || {
        let Some(p) = store::principal(id) else {
            let absent = format!("{}: {}", fmt::short(id), empty::absent(Some(Model::Principal)));
            return view! { <span class="list-item muted">{absent}</span> }.into_any();
        };
        view! {
            <span class="list-item person-item">
                <span class="author">{p.handle().unwrap_or_default()}</span>
                " "
                <span class="muted">{p.display_name().unwrap_or_default()}</span>
                " "
                <span class="pill">{role(&p)}</span>
            </span>
        }
        .into_any()
    })
    .into_any()
}

/// The embedded mode: the list item's line as a row, with nothing beneath it.
pub fn embedded(state: ThreadState, at: RowAt) -> AnyView {
    let line = list_item(at.id);
    thread::frame(state, at, line, Actions::default())
}

/// The full mode: the principal's line, and the tasks assigned to it, each as its list item.
pub fn full(id: EntityId) -> AnyView {
    let assigned = move || {
        let mut tasks: Vec<(i64, EntityId)> = store::tasks()
            .iter()
            .filter(|t| ops::assignee_of(t).ok().flatten() == Some(id))
            .filter(|t| t.status().is_ok_and(|s| !fmt::is_closed(s)))
            .map(|t| (t.updated().unwrap_or(0), t.id()))
            .collect();
        tasks.sort_by_key(|(updated, task)| (std::cmp::Reverse(*updated), *task));
        tasks
            .into_iter()
            .map(|(_, task)| {
                view! { <li>{modes::draw(Mode::Full, Mode::ListItem, task, &Models::one(Model::Task))}</li> }
            })
            .collect::<Vec<_>>()
    };
    view! {
        <div class="detail">
            <header class="detail-head">{list_item(id)}</header>
            <section class="prose">
                <h4>"Assigned to them, open"</h4>
                <ul class="plain-list">{assigned}</ul>
            </section>
        </div>
    }
    .into_any()
}
