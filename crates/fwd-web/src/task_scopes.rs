//! A task's scopes on its panel, as tags, so that a person moves a task from scope to scope where
//! they read it (Daniel, 2026-10-03: "Ideally scopes would be like tags, with an ( x ) on each
//! lozenge, and a [+] button that expands a search for scopes." and "I want to (myself) move
//! jKPzvWFZ into another scope in the task view, but there seems to be no way to do it"; task
//! oLv1sJ3v). A task may be worked in several scopes (docs/DECISIONS.md, D136, "A task may be
//! worked in several scopes"), and the panel shows each as its lozenge with a button after it, "×",
//! that takes the task out of that scope; after them, "+ scope", which opens a box offering the
//! project's scopes as a name is typed, active ones first, then by name, as the header's scope
//! filter lists them (`scope_filter::ordered_scopes`), less those the task is in, in the browser's
//! list for a text box, as the assignee's box offers principals (`assignee.rs`). Enter, or leaving
//! the box, adds the scope chosen, or the one a typed name names by the rule the command line
//! reads a scope's name by (`fwd_core::ops::scope_by_name`: its title, its id or the start of its
//! id), read among the scopes the box offers first, then among all, and Escape closes it. Each move
//! is one write, `ops::change_scopes`, so that moving a task is taking one scope off and adding
//! another, and taking off the last leaves the task in no scope, which tells no one, as attaching a
//! decision to a scope tells no one (D93).
//!
//! What the control shows and which scope the box adds are worked out as data and tested natively,
//! and so is that a write which leaves the task's scopes as they are draws nothing again
//! ([`drawn`]).

use crate::referents::{Mode, Models};
use crate::scope_filter::ordered_scopes;
use crate::session::{self, ctx};
use crate::thread::Model;
use crate::{empty, fmt, keys, labels, modes, store};
use ankurah::EntityId;
use fwd_core::ops;
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

/// What the control shows.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Shows {
    /// Nothing yet: the daemon has not answered the page's standing queries, before which a task
    /// can arrive before the relations that place it in its scopes, and those before the scopes, so
    /// the control would say "no scope", then a scope's short id, before the scope's name (task
    /// 7dq1vEd_).
    Loading,
    /// The task's scopes, each a lozenge with the button that takes it off ("no scope" for none),
    /// and after them "+ scope", or the box that adds one while it is `open`.
    Scopes { scopes: Vec<EntityId>, open: bool },
}

/// What the control shows for a task worked in `scopes`, the page having its answer (`loaded`) or
/// not, and the box being `open` or not.
fn shows(loaded: bool, scopes: Vec<EntityId>, open: bool) -> Shows {
    if loaded {
        Shows::Scopes { scopes, open }
    } else {
        Shows::Loading
    }
}

/// The field's label, "scope", or "scopes" for a task worked in several.
fn field_label(scopes: usize) -> &'static str {
    if scopes > 1 {
        "scopes"
    } else {
        "scope"
    }
}

/// The scopes a task worked in `has` is not in, of `all`, every scope's id and name in the order the
/// header's filter lists them: those the box offers.
fn left(all: &[(EntityId, String)], has: &[EntityId]) -> Vec<(EntityId, String)> {
    all.iter().filter(|(id, _)| !has.contains(id)).cloned().collect()
}

/// The scopes the box offers a task worked in `has` ([`left`]), each with what its option says:
/// its name, and after a name two of them share the start of its id, so that each option names one
/// scope.
fn offered(all: &[(EntityId, String)], has: &[EntityId]) -> Vec<(EntityId, String)> {
    let left = left(all, has);
    left.iter()
        .map(|(id, name)| match left.iter().filter(|(_, other)| other == name).count() {
            1 => (*id, name.clone()),
            _ => (*id, format!("{name} · {}", fmt::short(*id))),
        })
        .collect()
}

/// The scope a name typed in the box names, by the rule the command line reads a scope's name by
/// (`fwd_core::ops::scope_by_name`: its title whatever its case, its whole id or the start of it):
/// among the scopes the box offers first ([`left`]), so that a name the task's own scope shares
/// with one it is not in names the other, then among `all` the project's scopes, whose refusal it
/// gives when neither reading names one.
fn scope_named(name: &str, offered: &[(EntityId, String)], all: &[(EntityId, String)]) -> Result<EntityId, String> {
    ops::scope_by_name(name, offered).or_else(|_| ops::scope_by_name(name, all)).map_err(|e| e.to_string())
}

/// The scope the text in the box adds when Enter is pressed or the box is left: the one whose
/// option reads exactly so (`offered`, each scope the box offers with what its option says, which
/// choosing it writes in the box), else the one the text names (`named`, [`scope_named`]); none
/// while the box is blank, which closes it. A name that names no scope, or several, is refused with
/// the sentence the command line gives.
fn scope_to_add(
    text: &str,
    offered: &[(EntityId, String)],
    named: impl Fn(&str) -> Result<EntityId, String>,
) -> Result<Option<EntityId>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    if let Some((id, _)) = offered.iter().find(|(_, option)| option == text) {
        return Ok(Some(*id));
    }
    named(text).map(Some)
}

/// What the control shows, as a memo of the task's scopes, of whether the page has its answer and
/// of whether the box is open, so that the control is drawn again only when what it shows changes
/// ([`drawn`]).
fn showing(loaded: Signal<bool>, scopes: Memo<Vec<EntityId>>, open: RwSignal<bool>) -> Memo<Shows> {
    Memo::new(move |_| shows(loaded.get(), scopes.get(), open.get()))
}

/// What the control's elements read and do, besides what it shows.
#[derive(Clone, Copy)]
struct Moves {
    /// Whether the box is open in place of "+ scope".
    open: RwSignal<bool>,
    /// What is typed in the box.
    typed: RwSignal<String>,
    /// Why the last move was refused.
    error: RwSignal<Option<String>>,
    /// The box, which takes the focus as it opens.
    field: NodeRef<leptos::html::Input>,
    /// Takes the task out of the scope given.
    take_off: Callback<EntityId>,
    /// Adds the scope the box names, or closes it when it is blank ([`scope_to_add`]).
    take: Callback<()>,
    /// Closes the box, empty.
    close: Callback<()>,
}

/// The control as `showing` says, `list` being the element id of the list the box offers. Drawing
/// it reads nothing but `showing`: each lozenge reads its scope's name inside itself, and each
/// button's label inside its attribute, each updating in place. So a write that leaves the task's
/// scopes as they are, such as the stamp every write puts on its writer's principal
/// (`fwd_core::dormancy::wrote`), draws nothing again, and the keyboard's focus stays on a button
/// and an open preview of a lozenge stays open.
fn drawn(showing: Memo<Shows>, moves: Moves, list: String) -> impl Fn() -> AnyView + Send + Sync + 'static {
    let Moves { open, typed, error, field, take_off, take, close } = moves;
    move || {
        let (scopes, is_open) = match showing.get() {
            Shows::Loading => return view! { <span class="muted">{empty::LOADING}</span> }.into_any(),
            Shows::Scopes { scopes, open } => (scopes, open),
        };
        let tags = if scopes.is_empty() {
            view! { <span class="muted">"no scope"</span> }.into_any()
        } else {
            scopes
                .into_iter()
                .map(|scope| {
                    view! {
                        <span class="picked">
                            {modes::draw(Mode::Full, Mode::Lozenge, scope, &Models::one(Model::Scope))}
                            <button class="btn subtle tiny" title="Take the task out of this scope"
                                aria-label=move || format!("take the task out of {}", store::scope_label(scope))
                                on:click=move |_| take_off.run(scope)>"×"</button>
                        </span>
                    }
                })
                .collect_view()
                .into_any()
        };
        let add = if !is_open {
            view! {
                <button class="btn subtle tiny" title="Add the task to a scope" on:click=move |_| open.set(true)>
                    "+ scope"
                </button>
            }
            .into_any()
        } else {
            view! {
                <input type="text" node_ref=field list=list.clone() aria-label="add the task to the scope"
                    placeholder="a scope's name, or the start of its id"
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
            .into_any()
        };
        view! { {tags} {add} }.into_any()
    }
}

/// The scopes control on `task`'s panel, beside the words "scope" (or "scopes"), and after it why
/// a move was refused.
#[component]
pub fn TaskScopes(task: EntityId) -> impl IntoView {
    let error = RwSignal::new(None::<String>);
    let scopes = Memo::new(move |_| store::scopes_of_task(task));
    let loaded = Signal::derive(store::standing_loaded);
    // Whether the box is open in place of "+ scope", and what is typed in it.
    let open = RwSignal::new(false);
    let typed = RwSignal::new(String::new());
    let close = Callback::new(move |()| {
        open.set(false);
        typed.set(String::new());
        error.set(None);
    });
    // One write per move (`ops::change_scopes`), as whoever the page acts as.
    let change = move |add: Vec<EntityId>, take_off: Vec<EntityId>| {
        let Some(me) = session::me() else { return };
        spawn_local(async move {
            match ops::change_scopes(&ctx(), me, task, &add, &take_off).await {
                Ok(()) => {
                    session::wrote(me);
                    error.try_set(None)
                }
                Err(e) => error.try_set(Some(e.to_string())),
            };
        });
    };
    let take_off = Callback::new(move |scope: EntityId| change(Vec::new(), vec![scope]));
    // The scopes the box offers, each with what its option says: drawn again only when they
    // change, and not at every write that stamps its writer's principal.
    let offers = Memo::new(move |_| offered(&ordered_scopes(), &scopes.get()));
    let take = Callback::new(move |()| {
        let all = ordered_scopes();
        let not_in = left(&all, &scopes.get_untracked());
        let named = |name: &str| scope_named(name, &not_in, &all);
        match scope_to_add(&typed.get_untracked(), &offers.get_untracked(), named) {
            Ok(None) => close.run(()),
            Ok(Some(scope)) => {
                close.run(());
                change(vec![scope], Vec::new());
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
    let list = format!("scopes-for-{}", fmt::short(task));
    let moves = Moves { open, typed, error, field, take_off, take, close };
    let shown = drawn(showing(loaded, scopes, open), moves, list.clone());
    view! {
        <span class="field-label">{move || field_label(scopes.with(Vec::len))}</span>
        {shown}
        {labels::datalist(list, move || offers.get().into_iter().map(|(_, option)| option).collect())}
        {move || error.get().map(|e| view! { <span class="error">{e}</span> })}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> EntityId {
        EntityId::from_bytes([n; 32])
    }

    /// The project's scopes as the header's filter lists them: two active, one done, two of them
    /// sharing a name.
    fn all() -> Vec<(EntityId, String)> {
        vec![
            (id(1), "Cloud Hosting".into()),
            (id(2), "Docs".into()),
            (id(3), "Docs".into()),
            (id(4), "Open Core".into()),
        ]
    }

    #[test]
    fn the_scopes_show_once_the_page_has_its_answer_each_with_a_way_off_and_a_way_to_add_one() {
        assert_eq!(shows(false, vec![id(1)], false), Shows::Loading, "nothing to say before the answer");
        assert_eq!(shows(true, vec![id(1), id(2)], false), Shows::Scopes { scopes: vec![id(1), id(2)], open: false });
        assert_eq!(shows(true, vec![], true), Shows::Scopes { scopes: vec![], open: true }, "no scope, the box open");
        assert_eq!((field_label(0), field_label(1), field_label(2)), ("scope", "scope", "scopes"));
    }

    #[test]
    fn the_box_offers_the_scopes_the_task_is_not_in_each_option_naming_one() {
        // In Cloud Hosting: the rest, in the filter's order, the two named alike told apart.
        let docs = |n: u8| format!("Docs · {}", fmt::short(id(n)));
        assert_eq!(offered(&all(), &[id(1)]), vec![(id(2), docs(2)), (id(3), docs(3)), (id(4), "Open Core".into())]);
        // In one of the two Docs: the other reads by its name alone.
        assert_eq!(offered(&all(), &[id(1), id(2)]), vec![(id(3), "Docs".into()), (id(4), "Open Core".into())]);
        assert!(offered(&all(), &[id(1), id(2), id(3), id(4)]).is_empty(), "in every scope");
    }

    /// The rule a name typed in the box follows for a task worked in `has`, as the page holds these
    /// scopes ([`scope_named`]).
    fn named(has: Vec<EntityId>) -> impl Fn(&str) -> Result<EntityId, String> {
        move |name: &str| scope_named(name, &left(&all(), &has), &all())
    }

    #[test]
    fn the_box_adds_the_scope_chosen_from_its_list_or_named_by_what_is_typed() {
        let offers = offered(&all(), &[]);
        // Chosen from the list: the option's text, a shared name's id and all.
        let second_docs = format!("Docs · {}", fmt::short(id(3)));
        assert_eq!(scope_to_add(&second_docs, &offers, named(vec![])), Ok(Some(id(3))));
        assert_eq!(scope_to_add("Open Core", &offers, named(vec![])), Ok(Some(id(4))));
        // Typed: a title whatever its case, or the start of an id, the spaces around it left out.
        assert_eq!(scope_to_add(" cloud hosting ", &offers, named(vec![])), Ok(Some(id(1))));
        assert_eq!(scope_to_add(&id(4).to_base64()[..8], &offers, named(vec![])), Ok(Some(id(4))));
    }

    /// A name two scopes share names the one the box offers, when the task is in the other: typed
    /// in any case, as well as chosen from the list.
    #[test]
    fn a_shared_name_names_the_scope_the_task_is_not_in() {
        let offers = offered(&all(), &[id(2)]);
        for typed in ["Docs", "docs", " DOCS "] {
            assert_eq!(scope_to_add(typed, &offers, named(vec![id(2)])), Ok(Some(id(3))), "{typed}");
        }
    }

    #[test]
    fn a_blank_box_closes_and_a_name_that_names_no_scope_or_several_is_refused() {
        let offers = offered(&all(), &[]);
        assert_eq!(scope_to_add("  ", &offers, named(vec![])), Ok(None));
        assert_eq!(
            scope_to_add("Hostng", &offers, named(vec![])),
            Err("no scope has the title `Hostng` or an id starting with it".into())
        );
        // Both Docs offered: the name names two.
        assert!(scope_to_add("docs", &offers, named(vec![]))
            .unwrap_err()
            .starts_with("2 scopes have the title `docs`"));
    }

    /// Adding a scope, taking one off and taking off the last draw the control again, each once;
    /// a write that leaves the task's scopes as they are, and the box as it was, draws nothing.
    #[test]
    fn adding_and_taking_off_draw_the_control_again_and_a_write_that_changes_neither_does_not() {
        let owner = Owner::new();
        owner.set();
        // The task's scopes as the page reads them, which it does again at every write to one of
        // its relations, and whether the box is open.
        let answered = RwSignal::new(vec![id(1)]);
        let scopes = Memo::new(move |_| answered.get());
        let open = RwSignal::new(false);
        let moves = Moves {
            open,
            typed: RwSignal::new(String::new()),
            error: RwSignal::new(None),
            field: NodeRef::new(),
            take_off: Callback::new(|_| {}),
            take: Callback::new(|()| {}),
            close: Callback::new(|()| {}),
        };
        let draw = drawn(showing(Signal::stored(true), scopes, open), moves, "scopes".into());
        let draws = Memo::new(move |before: Option<&usize>| {
            let _ = draw();
            before.map_or(1, |n| n + 1)
        });
        assert_eq!(draws.get(), 1);
        // The same scopes again: nothing is drawn again.
        answered.set(vec![id(1)]);
        assert_eq!(draws.get(), 1);
        // The box opened; a scope added, closing the box, drawn once for both; a move (one off,
        // another on); and the last one taken off.
        open.set(true);
        assert_eq!(draws.get(), 2);
        answered.set(vec![id(1), id(4)]);
        open.set(false);
        assert_eq!(draws.get(), 3);
        answered.set(vec![id(4), id(2)]);
        assert_eq!(draws.get(), 4);
        answered.set(vec![]);
        assert_eq!(draws.get(), 5);
    }
}
