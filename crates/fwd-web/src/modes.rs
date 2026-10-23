//! Every kind of entity in four modes (Daniel, 2026-09-28, "Every kind of entity renders in four
//! modes"): a lozenge, a list item, embedded and full (`referents::Mode`). Each kind has one
//! component, a [`Kind`], that answers all four, and the page chooses it by the models an entity
//! carries (`referents::Registry`), so an entity that also carries a model the page does not know
//! still draws as what it knows, and a new kind is one `Kind` and one line in [`kinds`].
//!
//! The four are what the page already drew, mapped rather than rewritten: the lozenges on a row's
//! line are the lozenge mode (`lozenges.rs`); one line in a list, as a task's steps show each step
//! and as the Tasks, Decisions and Scopes pages lay theirs out in columns, is the list item; a
//! thread's row with the conversation beneath it is the embedded mode, the one place the page
//! recurses (`thread.rs`, under the outline ruling of 2026-09-26: messages beneath as rows,
//! relations as lozenges), and an embedded thing never embeds another in full; the task and
//! decision panels and a highlight's page are the full mode.

use crate::referents::{Mode, Models, Registry};
use crate::thread::{self, Hooks, Model, RowAt, ThreadState};
use crate::{anchor_row, anchors, decision_row, decisions, fmt, lozenges, message_row, person, scope_row, session};
use crate::{empty, store, task_row, tasks};
use ankurah::EntityId;
use leptos::prelude::*;
use std::sync::OnceLock;

/// One kind's component: what draws an entity of that kind in each of the four modes.
pub struct Kind {
    /// What a person calls the kind ("task", "highlight"), for what describes it in words (a
    /// step's check box to a screen reader).
    pub name: &'static str,
    /// Its lozenge standing alone, as a reference in a message's text or the assignee beside a
    /// task's scope show it: its preview on hover, and a click going where it leads. The
    /// lozenges on a row's line are the same, grouped by what joins them to the row.
    pub lozenge: fn(EntityId) -> AnyView,
    /// Its one line in a list.
    pub list_item: fn(EntityId) -> AnyView,
    /// Its row in a thread, the conversation about it beneath (`thread::frame`).
    pub embedded: fn(ThreadState, RowAt) -> AnyView,
    /// Everything its own page or panel shows.
    pub full: fn(EntityId) -> AnyView,
}

static TASK: Kind = Kind {
    name: "task",
    lozenge: task_row::lozenge,
    list_item: task_row::list_item,
    embedded: task_row::embedded,
    full: tasks::full,
};

static DECISION: Kind = Kind {
    name: "decision",
    lozenge: decision_row::lozenge,
    list_item: decision_row::list_item,
    embedded: decision_row::embedded,
    full: decisions::full,
};

static MESSAGE: Kind = Kind {
    name: "message",
    lozenge: message_row::lozenge,
    list_item: message_row::list_item,
    embedded: message_row::embedded,
    full: message_row::full,
};

static SCOPE: Kind = Kind {
    name: "scope",
    lozenge: scope_row::lozenge,
    list_item: scope_row::list_item,
    embedded: scope_row::embedded,
    full: crate::scope_panel::full,
};

static HIGHLIGHT: Kind = Kind {
    name: "highlight",
    lozenge: anchor_row::lozenge,
    list_item: anchor_row::list_item,
    embedded: anchor_row::embedded,
    full: anchors::full,
};

static PERSON: Kind = Kind {
    name: "person",
    lozenge: person::lozenge,
    list_item: person::list_item,
    embedded: person::embedded,
    full: person::full,
};

/// Which kind draws which models.
fn kinds() -> &'static Registry<Model, &'static Kind> {
    static KINDS: OnceLock<Registry<Model, &'static Kind>> = OnceLock::new();
    KINDS.get_or_init(|| {
        Registry::new()
            .add(Models::one(Model::Task), &TASK)
            .add(Models::one(Model::Decision), &DECISION)
            .add(Models::one(Model::Message), &MESSAGE)
            .add(Models::one(Model::Scope), &SCOPE)
            .add(Models::one(Model::Anchor), &HIGHLIGHT)
            .add(Models::one(Model::Principal), &PERSON)
    })
}

/// The kind that draws an entity carrying `models`: of those whose models it carries every one
/// of, the most specific. None for an entity carrying no model the page draws.
pub fn kind(models: &Models<Model>) -> Option<&'static Kind> {
    kinds().choose(models).copied()
}

/// Whether one of the page's standing queries holds an entity.
type Holds = fn(EntityId) -> bool;

/// The models the entity `id` carries, as the page's standing queries hold it; none while they do
/// not (not arrived yet, or of a kind the page has no query for). Each entity carries one model
/// today, so the first collection holding the id answers.
pub fn models_of(id: EntityId) -> Models<Model> {
    let held: [(Model, Holds); 6] = [
        (Model::Task, |id| store::task(id).is_some()),
        (Model::Decision, |id| store::decision(id).is_some()),
        (Model::Message, |id| store::message(id).is_some()),
        (Model::Scope, |id| store::scope(id).is_some()),
        (Model::Anchor, |id| store::anchor(id).is_some()),
        (Model::Principal, |id| store::principal(id).is_some()),
    ];
    held.into_iter().find(|(_, holds)| holds(id)).map(|(model, _)| Models::one(model)).unwrap_or_default()
}

/// Draws `id`, carrying `models`, in the mode `wanted` inside something drawn in the mode
/// `within`: in `wanted`, or in the richest mode `within` allows when `wanted` is richer, so an
/// embedded thing never holds another in full.
pub fn draw(within: Mode, wanted: Mode, id: EntityId, models: &Models<Model>) -> AnyView {
    draw_beneath(within, wanted, id, models, None)
}

/// [`draw`], for something the page shows beneath the entity `beneath` (a step beneath its task),
/// whose lozenge it then leaves out. The embedded mode standing alone is a thread of its own for
/// the acting principal, whose row shows its reply box, as a selected card's does.
pub fn draw_beneath(
    within: Mode,
    wanted: Mode,
    id: EntityId,
    models: &Models<Model>,
    beneath: Option<EntityId>,
) -> AnyView {
    let mode = within.nest(wanted);
    let Some(kind) = kind(models) else { return fallback(mode, id, models) };
    match mode {
        Mode::Lozenge => (kind.lozenge)(id),
        Mode::ListItem => (kind.list_item)(id),
        Mode::Embedded => {
            let models = models.clone();
            (move || {
                session::acting().get().map(|p| {
                    let hooks = Hooks { reply_box: Some(Signal::derive(|| true)), beneath, ..Hooks::default() };
                    thread::card_row(ThreadState::new(p.id()), id, id, models.clone(), hooks)
                })
            })
            .into_any()
        }
        Mode::Full => (kind.full)(id),
    }
}

/// Draws `id` in full, as its own page or panel does.
pub fn full(id: EntityId, models: &Models<Model>) -> AnyView {
    match kind(models) {
        Some(kind) => (kind.full)(id),
        None => fallback(Mode::Full, id, models),
    }
}

/// What the page draws of an entity carrying no model it knows: a lozenge with its short id, a
/// line with its id and its models, a row saying the same (`thread.rs`), or a sentence. One
/// carrying no model at all is one the page does not hold, and says so as `empty.rs` words it.
fn fallback(mode: Mode, id: EntityId, models: &Models<Model>) -> AnyView {
    let named = (!models.is_empty())
        .then(|| models.iter().map(|model| format!("{model:?}").to_lowercase()).collect::<Vec<_>>().join(", "));
    let carried = move || named.clone().unwrap_or_else(|| empty::absent(None));
    match mode {
        Mode::Lozenge => lozenges::single(lozenges::Target::Unknown(id)),
        Mode::ListItem | Mode::Embedded => view! {
            <span class="list-item"><span class="mono">{fmt::short(id)}</span>" "<span class="muted">{carried}</span></span>
        }
        .into_any(),
        Mode::Full => view! { <p class="empty">{move || format!("{}: {}", fmt::short(id), carried())}</p> }.into_any(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_model_the_page_draws_has_a_kind_answering_the_four_modes() {
        let named = |model| kind(&Models::one(model)).map(|k| k.name);
        assert_eq!(named(Model::Task), Some("task"));
        assert_eq!(named(Model::Decision), Some("decision"));
        assert_eq!(named(Model::Message), Some("message"));
        assert_eq!(named(Model::Scope), Some("scope"));
        assert_eq!(named(Model::Anchor), Some("highlight"));
        assert_eq!(named(Model::Principal), Some("person"));
        assert!(kind(&Models::default()).is_none(), "an entity the page does not hold draws as the fallback");
        // An entity carrying a model beside another still draws as the first registered it carries
        // all of; with one model per kind, as what it is.
        assert_eq!(kind(&Models::new([Model::Anchor, Model::Principal])).map(|k| k.name), Some("highlight"));
    }
}
