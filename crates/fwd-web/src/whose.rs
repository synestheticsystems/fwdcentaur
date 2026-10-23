//! Whose Inbox or Questions page shows. The page opens on the principal it acts as (the header's
//! `as`), and its own selector shows one other principal's instead, to read without acting as them:
//! Daniel reads his own questions and notifications, and another principal's only to check on an
//! agent or to see how it works, one principal at a time and never everyone's at once (task
//! jPQy9aQC, "Open the Questions and Inbox pages on your own, with a selector to look at another
//! principal's"; Daniel, 2026-09-28: "Most of the time I'm going to want to see my own inbox", and
//! "I don't really ever wanna see all inboxes for all principles at the same time").
//!
//! The route carries the choice (`#/inbox?for=<handle>`, `#/questions?for=<handle>`, or the
//! principal's id for a handle that two or more principals of the project share), so a reload
//! reopens on it and a link can carry it. A route naming no one shows the principal the page acts
//! as, so the nav's links lead to one's own and the page follows the header's `as` when it
//! changes. Looking changes nothing of theirs: the page still acts as its own principal, and on
//! another's cards nothing marks their notifications read or dismisses them, and no card has a
//! reply box (`cards.rs`, `Page::acts`).

use crate::empty::LOADING;
use crate::fmt;
use crate::session::{self, ctx};
use crate::store::{self, Group};
use ankurah::EntityId;
use fwd_core::naming;
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

/// Whose page the route names, as far as the page knows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    /// Not known yet: the page has yet to learn whom it acts as, or to find the principal the
    /// route names.
    Waiting,
    /// The route names no one the page can show: no principal has the name, two or more share it
    /// (the sentence lists them), or it could not be looked up; the sentence says which.
    Refused(String),
    /// The principal whose page it is.
    Principal(EntityId),
}

/// Whose page it is for the route's `named` name: the principal the page acts as when the route
/// names none, else the principal that name or id names, read as every typed name is
/// (`fwd_core::naming`), so a name two or more principals share is refused, listing them. A name
/// the page's standing query of principals does not hold is looked up at the daemon too, which
/// answers whether any principal has it, so a route naming no one who exists says so instead of
/// waiting for ever.
pub fn found(named: Memo<Option<String>>) -> Memo<Found> {
    // The last name the daemon found no principal for, or found shared, and why.
    let missing = RwSignal::new(None::<(String, String)>);
    Effect::new(move |_| {
        let Some(handle) = named.get() else { return };
        spawn_local(async move {
            let why = match naming::principal_named(&ctx(), &handle, None).await {
                Ok(Some(_)) => return,
                Ok(None) => format!("No principal has the handle `{handle}`."),
                Err(e) if e.downcast_ref::<naming::Refused>().is_some() => format!("{e}."),
                Err(e) => format!("The principal `{handle}` could not be looked up: {e}"),
            };
            let _ = missing.try_set(Some((handle, why)));
        });
    });
    Memo::new(move |_| {
        let Some(handle) = named.get() else {
            return session::acting().get().map_or(Found::Waiting, |p| Found::Principal(p.id()));
        };
        match store::principal_named(&handle) {
            Ok(Some(p)) => return Found::Principal(p.id()),
            Err(why) => return Found::Refused(format!("{why}.")),
            Ok(None) => {}
        }
        missing.with(|missing| match missing {
            Some((gone, why)) if *gone == handle => Found::Refused(why.clone()),
            _ => Found::Waiting,
        })
    })
}

/// Whether the cards on a page showing `whose`'s inbox or questions act, on a page acting as `me`:
/// on the page's own principal's cards they reply, mark read and dismiss; on another's, shown to
/// read, they do none of it, since the page would do it as `me` to someone else's inbox.
pub fn acts(me: EntityId, whose: EntityId) -> bool {
    me == whose
}

/// The name the route names once the selector has chosen `chosen`, which `name` names (its handle
/// while no other principal of the project has it, else its id: [`route_name`]), on a page acting
/// as `me`: none for `me`, so the route is the bare one, which follows the header's `as`; else
/// `name`.
pub fn named(chosen: EntityId, name: String, me: EntityId) -> Option<String> {
    (chosen != me).then_some(name)
}

/// What a route names principal `id` by: its handle while no other principal of the project has
/// it, else its id, which names it however many share the handle.
pub fn route_name(id: EntityId) -> String {
    store::unique_handle(id).unwrap_or_else(|| id.to_base64())
}

/// What the Inbox says while it shows `whose`'s notifications to a page acting as `me`: beneath
/// its heading, and on hover over the controls it leaves disabled.
pub fn reading_inbox(whose: &str, me: &str) -> String {
    format!(
        "Reading {whose}'s inbox as {me}: nothing here marks it read or dismisses it, and a reply or a move made from a card's rows is {me}'s."
    )
}

/// What the Questions page says beneath its heading while it shows the questions asked of `whose`
/// to a page acting as `me`.
pub fn reading_questions(whose: &str, me: &str) -> String {
    format!(
        "Reading the questions asked of {whose} as {me}: the cards have no reply box here, and a reply or a resolve made from a card's rows is {me}'s."
    )
}

/// The page's selector of whose page it shows: every principal that belongs to the project,
/// grouped as the header's `as` picker groups them, with `shown` chosen (listed even when it does
/// not belong); none is chosen while the page has no one to show. `hint` says on hover what the
/// choice does. Choosing takes the page
/// to its route naming the principal chosen, or naming no one for `me`, the principal the page acts
/// as: `choose` receives the handle the route is to name ([`named`]).
#[component]
pub fn WhosePicker(
    me: EntityId,
    shown: Option<EntityId>,
    hint: &'static str,
    choose: Callback<Option<String>>,
) -> impl IntoView {
    // Each heading and its principals; a change to anything else about a principal does not draw
    // the options again.
    let listed = Memo::new(move |_| {
        store::principal_groups(shown).into_iter().map(|g| (g.label, g.members)).collect::<Vec<_>>()
    });
    let options = move || {
        let groups = listed.get().into_iter().map(|(label, members)| Group { label, members }).collect();
        fmt::principal_options(groups, shown)
    };
    let on_change = move |ev: leptos::ev::Event| {
        let Ok(id) = event_target_value(&ev).parse::<EntityId>() else { return };
        if store::principal(id).is_some() {
            choose.run(named(id, route_name(id), me));
        }
    };
    // The select shows `shown`, set once its options are drawn, as the scope filter's is
    // (`scope_filter.rs`): an option's `selected` attribute stops moving a select once a person has
    // chosen in it. A value no option has shows none chosen.
    let select = NodeRef::<leptos::html::Select>::new();
    Effect::new(move |_| {
        listed.track();
        if let Some(select) = select.get() {
            select.set_value(&shown.map(|id| id.to_base64()).unwrap_or_default());
        }
    });
    view! {
        <label class="field whose" title=hint>
            "for "
            <select node_ref=select on:change=on_change>{options}</select>
        </label>
    }
}

/// What a page shows in place of its cards while it has no one to show them for: a line saying it
/// is loading until it knows whose page it is; when the name its route names is no principal's, or
/// is shared by two or more, why, under its heading and the selector, to choose someone.
pub fn no_one_to_show(
    found: Found,
    title: &'static str,
    me: EntityId,
    hint: &'static str,
    choose: Callback<Option<String>>,
) -> AnyView {
    match found {
        Found::Refused(why) => view! {
            <section class="page">
                <header class="page-head">
                    <h2>{title}</h2>
                    <WhosePicker me=me shown=None hint=hint choose=choose/>
                </header>
                <p class="error">{why}</p>
            </section>
        }
        .into_any(),
        _ => view! { <section class="page"><p class="muted">{LOADING}</p></section> }.into_any(),
    }
}

#[cfg(test)]
mod tests {
    use super::{acts, named};
    use ankurah::EntityId;

    #[test]
    fn only_the_acting_principals_own_cards_act() {
        let (me, claude) = (EntityId::from_bytes([1; 32]), EntityId::from_bytes([2; 32]));
        assert!(acts(me, me), "one's own inbox and questions reply, mark read and dismiss");
        assert!(!acts(me, claude), "another's, shown to read, do none of it");
    }

    #[test]
    fn choosing_oneself_names_no_one_in_the_route_and_choosing_another_names_them_by_handle() {
        let (me, claude) = (EntityId::from_bytes([1; 32]), EntityId::from_bytes([2; 32]));
        // One's own page is the bare route, which follows the header's `as` when it changes.
        assert_eq!(named(me, "dnorman".into(), me), None);
        assert_eq!(named(claude, "claude".into(), me), Some("claude".into()));
    }
}
