//! A task's assignee on its panel, as one control, so that the panel names the assignee once
//! (Daniel, 2026-10-03, of the assignee's lozenge and the picker after it, which named the
//! assignee again: "this part of the task UI seems a bit weird: ASSIGNEE ( dnorman ) assign [
//! dnorman v ] surely we can do better"; task iJBOfKqF): the assignee's lozenge with a button after
//! it that unassigns the task, or, while nobody is assigned, "+ assign", which opens a box offering
//! the project's people and agents as a name is typed. The box offers them as the page's pickers
//! list them (`store::principal_groups`: those that belong to the project, by heading and then by
//! name, a name that several share with the scopes each one's work is in: docs/DECISIONS.md, D106
//! and D107), in the browser's list for a text box, as a task's labels box offers labels
//! (`labels.rs`); Enter, or leaving the box, assigns the one chosen or the one whose name or start
//! of an id is typed ([`assign_to`]), and Escape closes it. Both moves write `ops::assign`, as `fwd
//! task assign` and `fwd task unassign` do, which tells the new assignee unless they made the
//! move, and no one of an unassignment (docs/design/agents-and-inbox.md, rule 7). Until 2026-10-03
//! the panel showed the label "assignee", the assignee's lozenge or "unassigned", and a select of
//! every principal headed "assign", with "no one" first, which showed the assignee again.
//!
//! What the control shows and whom the box assigns are worked out as data and tested natively, and
//! so is that a write which leaves the assignee as it is draws nothing again ([`drawn`]).

use crate::referents::{Mode, Models};
use crate::session::{self, ctx};
use crate::thread::Model;
use crate::{fmt, keys, labels, modes, store};
use ankurah::EntityId;
use fwd_core::{naming, ops};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

/// What the control shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shows {
    /// The assignee's lozenge, and the button after it that unassigns the task.
    Assignee(EntityId),
    /// "+ assign", which opens the box.
    Assign,
    /// The box that assigns the task.
    Box,
}

/// What the control shows for a task assigned to `assignee`, the box being `open` or not: the
/// assignee whenever there is one, since a new assignee closes the box.
fn shows(assignee: Option<EntityId>, open: bool) -> Shows {
    match (assignee, open) {
        (Some(assignee), _) => Shows::Assignee(assignee),
        (None, false) => Shows::Assign,
        (None, true) => Shows::Box,
    }
}

/// The principal the text in the box assigns the task to when Enter is pressed or the box is left:
/// the one whose option reads exactly so (`offered`, each principal the box offers with what its
/// option says, which choosing it writes in the box), else the one the text names as a typed name
/// does (`named`, by `fwd_core::naming::resolve`: a name with or without its `@`, a whole id, or
/// the start of one); none while the box is blank, which closes it. A name that names no one is
/// refused as the command line refuses it, and `named` refuses a name several principals share,
/// listing them.
fn assign_to(
    text: &str,
    offered: &[(EntityId, String)],
    named: impl Fn(&str) -> Result<Option<EntityId>, String>,
) -> Result<Option<EntityId>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    if let Some((id, _)) = offered.iter().find(|(_, option)| option == text) {
        return Ok(Some(*id));
    }
    match named(text)? {
        Some(id) => Ok(Some(id)),
        None => Err(format!("no principal of this project is named `{}`", naming::bare(text))),
    }
}

/// What the control shows, as a memo of the task's assignee and of whether the box is open, so that
/// the control is drawn again only when what it shows changes ([`drawn`]).
fn showing(assignee: Memo<Option<EntityId>>, open: RwSignal<bool>) -> Memo<Shows> {
    Memo::new(move |_| shows(assignee.get(), open.get()))
}

/// What the control's elements read and do, besides what it shows.
#[derive(Clone, Copy)]
struct Moves {
    /// Whether the box is open in place of "+ assign".
    open: RwSignal<bool>,
    /// What is typed in the box.
    typed: RwSignal<String>,
    /// Why the last move was refused.
    error: RwSignal<Option<String>>,
    /// The box, which takes the focus as it opens.
    field: NodeRef<leptos::html::Input>,
    /// Assigns the task to the principal given, or to no one.
    assign: Callback<Option<EntityId>>,
    /// Assigns the task to whom the box names, or closes it when it is blank ([`assign_to`]).
    take: Callback<()>,
    /// Closes the box, empty.
    close: Callback<()>,
}

/// The control as `showing` says, `list` being the element id of the list the box offers. Drawing
/// it reads nothing but `showing`: the lozenge reads the assignee's name inside itself, and the
/// unassign button's label inside its attribute, each updating in place. So a write that leaves the
/// task's assignee as it is, such as the stamp every write puts on its writer's principal
/// (`fwd_core::dormancy::wrote`), draws nothing again, and the keyboard's focus stays on the
/// button and an open preview of the lozenge stays open.
fn drawn(showing: Memo<Shows>, moves: Moves, list: String) -> impl Fn() -> AnyView + Send + Sync + 'static {
    let Moves { open, typed, error, field, assign, take, close } = moves;
    move || match showing.get() {
        Shows::Assignee(person) => view! {
            <span class="picked">
                {modes::draw(Mode::Full, Mode::Lozenge, person, &Models::one(Model::Principal))}
                <button class="btn subtle tiny" title="Unassign the task"
                    aria-label=move || format!("unassign {}", store::handle_of(person))
                    on:click=move |_| assign.run(None)>"×"</button>
            </span>
        }
        .into_any(),
        Shows::Assign => view! {
            <button class="btn subtle tiny" title="Assign the task to someone" on:click=move |_| open.set(true)>
                "+ assign"
            </button>
        }
        .into_any(),
        Shows::Box => view! {
            <input type="text" node_ref=field list=list.clone() aria-label="assign the task to"
                placeholder="a name, or the start of an id"
                prop:value=move || typed.get()
                on:input=move |ev| {
                    typed.set(event_target_value(&ev));
                    error.set(None);
                }
                on:keydown=move |ev| {
                    if keys::is_send(&ev) {
                        ev.prevent_default();
                        take.run(());
                    } else if ev.key() == "Escape" {
                        // Used here, so the panel leaves the key alone (`panel.rs`).
                        ev.prevent_default();
                        close.run(());
                    }
                }
                on:change=move |_| take.run(())
                on:blur=move |_| {
                    if typed.get_untracked().trim().is_empty() {
                        close.run(());
                    }
                }/>
        }
        .into_any(),
    }
}

/// The assignee control on `task`'s panel, beside the words "assignee", and after it why a move
/// was refused.
#[component]
pub fn TaskAssignee(task: EntityId) -> impl IntoView {
    let error = RwSignal::new(None::<String>);
    let assignee = Memo::new(move |_| store::task(task).and_then(|t| ops::assignee_of(&t).ok().flatten()));
    // Whether the box is open in place of "+ assign", and what is typed in it.
    let open = RwSignal::new(false);
    let typed = RwSignal::new(String::new());
    let close = Callback::new(move |()| {
        open.set(false);
        typed.set(String::new());
        error.set(None);
    });
    // A new assignee, set here or anywhere else, closes the box.
    Effect::new(move |_| {
        assignee.track();
        open.set(false);
        typed.set(String::new());
    });
    let assign = Callback::new(move |chosen: Option<EntityId>| {
        let Some(me) = session::me() else { return };
        spawn_local(async move {
            match ops::assign(&ctx(), me, task, chosen).await {
                Ok(()) => {
                    session::wrote(me);
                    error.try_set(None)
                }
                Err(e) => error.try_set(Some(e.to_string())),
            };
        });
    });
    // The principals the box offers, each with what its option says, in the pickers' order: drawn
    // again only when they change, and not at every write that stamps its writer's principal
    // (`fwd_core::dormancy::wrote`).
    let offered =
        Memo::new(move |_| store::principal_groups(None).into_iter().flat_map(|g| g.members).collect::<Vec<_>>());
    let take = Callback::new(move |()| {
        let named = |name: &str| store::principal_named(name).map(|p| p.map(|p| p.id()));
        match assign_to(&typed.get_untracked(), &offered.get_untracked(), named) {
            Ok(None) => close.run(()),
            Ok(Some(chosen)) => {
                close.run(());
                assign.run(Some(chosen));
            }
            Err(why) => error.set(Some(why)),
        }
    });
    // The box takes the focus as it opens.
    let field = NodeRef::<leptos::html::Input>::new();
    Effect::new(move |_| {
        if let Some(field) = field.get() {
            let _ = field.focus();
        }
    });
    let list = format!("assignees-for-{}", fmt::short(task));
    let moves = Moves { open, typed, error, field, assign, take, close };
    let shown = drawn(showing(assignee, open), moves, list.clone());
    view! {
        <span class="field-label">"assignee"</span>
        {shown}
        {labels::datalist(list, move || offered.get().into_iter().map(|(_, option)| option).collect())}
        {move || error.get().map(|e| view! { <span class="error">{e}</span> })}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> EntityId {
        EntityId::from_bytes([n; 32])
    }

    #[test]
    fn the_assignee_shows_once_as_a_lozenge_that_unassigns_else_assign_opens_the_box() {
        let person = id(1);
        // Assigned: the lozenge and the button that unassigns, whatever the box was.
        assert_eq!(shows(Some(person), false), Shows::Assignee(person));
        assert_eq!(shows(Some(person), true), Shows::Assignee(person));
        // Nobody assigned: "+ assign", which opens the box.
        assert_eq!(shows(None, false), Shows::Assign);
        assert_eq!(shows(None, true), Shows::Box);
    }

    #[test]
    fn a_write_that_leaves_the_assignee_as_it_is_draws_the_control_not_again() {
        let owner = Owner::new();
        owner.set();
        // The task's assignee as the standing query of tasks answers it, which it does again at
        // every write to a task, and whether the box is open.
        let answered = RwSignal::new(Some(id(1)));
        let assignee = Memo::new(move |_| answered.get());
        let open = RwSignal::new(false);
        let moves = Moves {
            open,
            typed: RwSignal::new(String::new()),
            error: RwSignal::new(None),
            field: NodeRef::new(),
            assign: Callback::new(|_| {}),
            take: Callback::new(|()| {}),
            close: Callback::new(|()| {}),
        };
        let draw = drawn(showing(assignee, open), moves, "assignees".into());
        // How many times the control has been drawn, counted as the page draws it: again whenever
        // something that drawing it read has changed. No standing query is open here, so a drawing
        // that read one, which the page would draw again at every write, such as the stamp each
        // write puts on its writer's principal, fails this test.
        let draws = Memo::new(move |before: Option<&usize>| {
            let _ = draw();
            before.map_or(1, |n| n + 1)
        });
        assert_eq!(draws.get(), 1);
        // A write that leaves the assignee as it is, and the box closed again, as the effect that
        // closes it on a new assignee leaves it: nothing is drawn again, so the keyboard's focus
        // stays on the button that unassigns and an open preview of the lozenge stays open.
        answered.set(Some(id(1)));
        open.set(false);
        assert_eq!(draws.get(), 1);
        // A new assignee is drawn, and so are unassigning and opening the box.
        answered.set(Some(id(2)));
        assert_eq!(draws.get(), 2);
        answered.set(None);
        assert_eq!(draws.get(), 3);
        open.set(true);
        assert_eq!(draws.get(), 4);
    }

    /// dnorman, and two implementers sharing a name, whose options carry their scopes.
    fn offered() -> Vec<(EntityId, String)> {
        vec![
            (id(1), "dnorman".into()),
            (id(2), "implementer [Replication]".into()),
            (id(3), "implementer [Anchoring]".into()),
        ]
    }

    /// The rule a typed name follows, as the page holds these principals: a name with or without
    /// its `@`, or the start of an id; a name two share is refused, listing them.
    fn named(name: &str) -> Result<Option<EntityId>, String> {
        match naming::bare(name) {
            "dnorman" => Ok(Some(id(1))),
            "implementer" => Err("`implementer` names 2 principals".into()),
            start if start.len() >= 6 && id(3).to_base64().starts_with(start) => Ok(Some(id(3))),
            _ => Ok(None),
        }
    }

    #[test]
    fn the_box_assigns_the_principal_chosen_from_its_list_or_named_by_what_is_typed() {
        // Chosen from the list: the option's text, a shared name's scopes and all.
        assert_eq!(assign_to("implementer [Anchoring]", &offered(), named), Ok(Some(id(3))));
        assert_eq!(assign_to("dnorman", &offered(), named), Ok(Some(id(1))));
        // Typed: a name, its at-sign optional, or the start of an id, the spaces around it left out.
        assert_eq!(assign_to(" @dnorman ", &offered(), named), Ok(Some(id(1))));
        let start: String = id(3).to_base64().chars().take(8).collect();
        assert_eq!(assign_to(&start, &offered(), named), Ok(Some(id(3))));
    }

    #[test]
    fn a_blank_box_closes_and_a_name_that_names_no_one_or_several_is_refused() {
        assert_eq!(assign_to("  ", &offered(), named), Ok(None));
        assert_eq!(
            assign_to("@dnormn", &offered(), named),
            Err("no principal of this project is named `dnormn`".into())
        );
        assert_eq!(assign_to("implementer", &offered(), named), Err("`implementer` names 2 principals".into()));
    }
}
