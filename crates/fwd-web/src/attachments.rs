//! The highlights a message names, shown beneath its text, so that a reply can point at code and
//! whoever reads it sees the lines where the reply is (Daniel, 2026-09-28, in a review thread:
//! "Are you able to attach a highlight to this thread showing me what you mean? This should be
//! possible similar to attaching a step to a task, except it's a reply or part of a reply here").
//! Naming a highlight in the text is the attachment: the posting path stores every reference as
//! `[[<id>]]`, whatever the writer typed, and writes a `mention` edge beside it (`references.rs`),
//! so no other syntax and no other command is needed.
//!
//! Each highlight shows as a step shows beneath its task (`steps.rs`): one line, its list item
//! (the first highlighted line, the path, the basis and the lines), with an expander that opens
//! its embedded mode in place (its lines, its threads, and a "comment" button that opens a box
//! for a new thread on it). A click on the line goes where a click on the highlight's lozenge
//! would: to its page, or in the VS Code webview to the editor. The box opens on demand rather than
//! standing open as a step's does, because on a page of cards the box that stands open is the
//! card's own, which the page gives the keyboard to when it selects the card (`cards.rs`).
//!
//! Which of the things a message names show beneath it is worked out as data first ([`attached`])
//! and tested natively: the highlights among them, each once, in the order the text first names
//! them, less any that already shows above the message (the highlight whose page or thread the
//! message is in); a task, a decision, a message, a scope or a person stays a lozenge in the text,
//! and so does a whole file the text names at a version (`fwd_core::file_mention`).
//! The ids come from the message's stored text (each `[[<id>]]` outside code), which the row
//! already reads to draw the lozenges in it, rather than from its `mention` relations, which the
//! page holds too: what shows beneath the text then always matches the lozenges in it, in the
//! order the writer named them, and a message written before the page showed anything beneath a
//! message's text shows its highlights as well, since its text names them the same way. A
//! highlight the page does not hold yet (its standing anchors query has not brought it) is not
//! known to be one, so it shows only as its lozenge in the text, and beneath the text once it
//! arrives.
//!
//! The keys, on a highlight's line (Tab reaches each): ArrowRight and ArrowLeft open and close it
//! in place, Enter goes where a click goes, ArrowUp and ArrowDown move between the lines, ArrowUp
//! from the first line goes back to the message's row, and ArrowDown past the last goes on as it
//! would from the message's row (`thread.rs`). Within a highlight opened in place its rows keep
//! their own keys; ArrowUp on its own row, or ArrowLeft there once that row is closed, comes back
//! to its line, and ArrowDown past its last row goes on to the next line, when there is one.

use crate::lozenges::{self, Target};
use crate::referents::{Mode, Models};
use crate::thread::{self, Hooks, Model, RowAt, ThreadState};
use crate::{keys, modes, store};
use ankurah::EntityId;
use fwd_core::ops;
use fwd_core::references::stored_named;
use leptos::prelude::*;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU32, Ordering};
use wasm_bindgen::JsCast;

/// The kinds a message shows beneath its text when the text names one: a highlight, so that a
/// reply can point at code. Anything else a message names (a task, a decision, a message, a scope,
/// a person) stays a lozenge in its text.
const SHOWN_BENEATH: &[Model] = &[Model::Anchor];

/// What shows beneath a message whose stored text is `text`: each entity the text names
/// (`[[<id>]]` outside code) that carries a model in [`SHOWN_BENEATH`] (`models` answers the
/// models the page holds it with, none while it holds nothing with that id), once, in the order
/// the text first names it, and none of `above`, which already show above the message. A whole
/// file the text names at a version (`[[<id>@<version>]]`, `fwd_core::file_mention`) shows as its
/// lozenge in the text alone: a whole-file highlight has no lines to open in place, and the
/// lozenge names the file and its version already.
pub fn attached(text: &str, models: impl Fn(EntityId) -> Models<Model>, above: &[EntityId]) -> Vec<EntityId> {
    let mut shown: Vec<EntityId> = Vec::new();
    for (_, named) in stored_named(text) {
        let id = named.id;
        if named.version.is_some() || shown.contains(&id) || above.contains(&id) {
            continue;
        }
        let carried = models(id);
        if SHOWN_BENEATH.iter().any(|model| carried.contains(*model)) {
            shown.push(id);
        }
    }
    shown
}

// ---------------------------------------------------------------------------------------------
// Drawing

/// Beneath the text of the message whose row is at `at`, what its text names that shows there:
/// nothing at all while it names none. The message's row (`message_row.rs`) draws it wherever
/// the row shows: in a thread, on an inbox or Questions card, on a highlight's page, in a step
/// opened in place.
pub fn beneath(state: ThreadState, at: &RowAt) -> AnyView {
    let (message, me, above) = (at.id, state.me, state.shown_above(at));
    // Changes only when the message's text or what its references name does.
    let shown = Memo::new(move |_| {
        let text = store::message(message).map(|m| ops::text(m.content()).unwrap_or_default()).unwrap_or_default();
        attached(&text, modes::models_of, &above)
    });
    view! { <Attachments message=message me=me shown=shown/> }.into_any()
}

/// Tells one list's elements from another's: the same highlight can show beneath several messages
/// on one page.
fn next_list() -> u32 {
    static LIST: AtomicU32 = AtomicU32::new(0);
    LIST.fetch_add(1, Ordering::Relaxed)
}

fn line_element(list: u32, id: EntityId) -> String {
    format!("attached-{list}-{}", id.to_base64())
}

fn element(target: Option<web_sys::EventTarget>) -> Option<web_sys::Element> {
    target.and_then(|t| t.dyn_into::<web_sys::Element>().ok())
}

/// Gives the focus to the element `id`; false when it is not on the page.
fn focus(id: &str) -> bool {
    let element = web_sys::window().and_then(|w| w.document()).and_then(|d| d.get_element_by_id(id));
    match element.and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok()) {
        Some(element) => element.focus().is_ok(),
        None => false,
    }
}

/// The line of the list `list` that `target` is in or beneath, passing over the line of any list
/// beneath a message shown inside one of its highlights.
fn own_item(target: &web_sys::Element, list: &web_sys::Element) -> Option<web_sys::Element> {
    let mut at = target.closest("li.attached").ok().flatten();
    while let Some(item) = at {
        if item.parent_element().is_some_and(|parent| parent.is_same_node(Some(list.as_ref()))) {
            return Some(item);
        }
        at = item.parent_element().and_then(|parent| parent.closest("li.attached").ok().flatten());
    }
    None
}

/// The thing shown beneath the message `message`, opened in place: its embedded mode as a thread
/// of its own for `me`, whose row leaves out the message's lozenge (the message shows just above
/// it) and opens its box on demand.
fn opened(me: EntityId, message: EntityId, id: EntityId, models: Models<Model>) -> AnyView {
    let hooks = Hooks { beneath: Some(message), ..Hooks::default() };
    thread::card_row(ThreadState::new(me), id, id, models, hooks)
}

#[component]
fn Attachments(message: EntityId, me: EntityId, shown: Memo<Vec<EntityId>>) -> impl IntoView {
    let list = next_list();
    let expanded = RwSignal::new(HashSet::<EntityId>::new());
    let set_open = move |id: EntityId, open: bool| {
        expanded.update(|e| {
            if open {
                e.insert(id);
            } else {
                e.remove(&id);
            }
        })
    };
    let on_key = move |ev: web_sys::KeyboardEvent| {
        if ev.default_prevented() || !keys::is_plain(&ev) {
            return;
        }
        let (Some(target), Some(list_element)) = (element(ev.target()), element(ev.current_target())) else { return };
        let Some(item) = own_item(&target, &list_element) else { return };
        let Some(id) = item.get_attribute("data-id").and_then(|id| id.parse::<EntityId>().ok()) else { return };
        let all = shown.get_untracked();
        let Some(at) = all.iter().position(|shown| *shown == id) else { return };
        let before = at.checked_sub(1).and_then(|i| all.get(i)).copied();
        let after = all.get(at + 1).copied();
        let on_line = item.first_element_child().is_some_and(|line| line.contains(Some(target.as_ref())));
        let used = if on_line {
            match ev.key().as_str() {
                "ArrowRight" => {
                    set_open(id, true);
                    true
                }
                "ArrowLeft" => {
                    set_open(id, false);
                    true
                }
                "ArrowUp" => match before {
                    Some(before) => focus(&line_element(list, before)),
                    // Back to the message's own row.
                    None => list_element
                        .closest("[role=treeitem]")
                        .ok()
                        .flatten()
                        .and_then(|row| row.dyn_into::<web_sys::HtmlElement>().ok())
                        .is_some_and(|row| row.focus().is_ok()),
                },
                // Past the last line the key is the rows': they go on as from the message's row.
                "ArrowDown" => after.is_some_and(|after| focus(&line_element(list, after))),
                _ => false,
            }
        } else if keys::is_text_field(&target) {
            false
        } else {
            // Within a highlight opened in place, a key its rows left alone at their edge.
            match ev.key().as_str() {
                "ArrowUp" | "ArrowLeft" => focus(&line_element(list, id)),
                "ArrowDown" => after.is_some_and(|after| focus(&line_element(list, after))),
                _ => false,
            }
        };
        if used {
            ev.prevent_default();
        }
    };
    let item = move |id: EntityId| {
        let models = modes::models_of(id);
        let target = Target::by_models(id, &models);
        let open = Memo::new(move |_| expanded.with(|e| e.contains(&id)));
        let href = move || lozenges::destination(target).map(|to| to.hash());
        let first_line = modes::draw(Mode::Embedded, Mode::ListItem, id, &models);
        view! {
            <li class="attached" data-id=id.to_base64()>
                <div class="attached-line">
                    <button class="caret" tabindex="-1" title="show it here, with the conversation about it"
                        aria-label="show it here" aria-expanded=move || open.get().to_string()
                        on:click=move |_| set_open(id, !open.get_untracked())>
                        {move || if open.get() { "▾" } else { "▸" }}
                    </button>
                    // A highlight's list item holds no link of its own, so the whole line is one.
                    <a class="attached-body" id=line_element(list, id) href=href
                        on:click=move |ev| lozenges::revealed_instead(&ev, target)>
                        {first_line}
                    </a>
                </div>
                {move || {
                    open.get().then(|| {
                        view! { <div class="attached-embedded">{opened(me, message, id, models.clone())}</div> }
                    })
                }}
            </li>
        }
    };
    view! {
        <ul class="attachments" aria-label="the highlights it names" on:keydown=on_key>
            <For each=move || shown.get() key=|id| *id children=item/>
        </ul>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> EntityId {
        EntityId::from_bytes([n; 32])
    }

    fn named(n: u8) -> String {
        format!("[[{}]]", id(n).to_base64())
    }

    /// The page's standing queries in miniature: 1 and 2 are highlights, 3 a task, 4 a decision, 5
    /// a person, 6 a message and 7 a scope; the page holds nothing with any other id.
    fn models(id: EntityId) -> Models<Model> {
        let model = match id.to_bytes()[0] {
            1 | 2 => Model::Anchor,
            3 => Model::Task,
            4 => Model::Decision,
            5 => Model::Principal,
            6 => Model::Message,
            7 => Model::Scope,
            _ => return Models::default(),
        };
        Models::one(model)
    }

    #[test]
    fn only_the_highlights_a_message_names_show_beneath_it() {
        let text = format!(
            "{} waits on {} as {} ruled; {} said so in {}, in {}, about {} and {}",
            named(3),
            named(1),
            named(4),
            named(5),
            named(6),
            named(7),
            named(2),
            named(1)
        );
        assert_eq!(attached(&text, models, &[]), vec![id(1), id(2)], "each highlight once, in the order first named");
        let task_only = format!("tracked in {}, as {} ruled", named(3), named(4));
        assert!(attached(&task_only, models, &[]).is_empty(), "a task and a decision stay lozenges in the text");
        assert!(attached("no references at all", models, &[]).is_empty());
    }

    #[test]
    fn a_highlight_the_page_does_not_hold_yet_is_its_lozenge_in_the_text_only() {
        let text = format!("{} and {}", named(9), named(2));
        assert_eq!(attached(&text, models, &[]), vec![id(2)]);
    }

    #[test]
    fn a_highlight_already_shown_above_the_message_does_not_show_again_beneath_it() {
        // A reply in a thread on highlight 1, on that highlight's page, naming it and highlight 2.
        let text = format!("as {} shows, and {} too", named(1), named(2));
        assert_eq!(attached(&text, models, &[id(1)]), vec![id(2)]);
    }

    #[test]
    fn a_highlight_named_in_code_is_text_as_the_posting_path_reads_it() {
        let text = format!("run `{}` then look at\n```\n{}\n```\nand {}", named(1), named(2), named(2));
        assert_eq!(attached(&text, models, &[]), vec![id(2)], "only the reference outside code");
    }

    #[test]
    fn a_file_a_message_names_at_a_version_is_its_lozenge_in_the_text_only() {
        let file = format!("[[{}@worktree]]", id(1).to_base64());
        let text = format!("the primer is {file}, and see {}", named(2));
        assert_eq!(attached(&text, models, &[]), vec![id(2)], "the file stays in the text");
        let both = format!("{file} is {}", named(1));
        assert_eq!(attached(&both, models, &[]), vec![id(1)], "the highlight named by its id alone shows beneath");
    }
}
