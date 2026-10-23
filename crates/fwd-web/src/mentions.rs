//! The mention search beneath the page's message boxes (the Ask box, a panel's comment box, a
//! row's reply box and the box that edits a message): while the caret is in an `@name` being
//! typed, the principals that belong to the project whose name starts with what is typed, each with
//! the scopes its work is in beside a name that two or more principals of the project share, so
//! that they can be told apart (docs/DECISIONS.md, D107; docs/design/web-ui.md, "Inline
//! references"), and the agents that have written nothing for twelve hours after all the others,
//! under a heading (D123). A click, or Up and Down and then Enter or Tab, writes the mention in
//! place of what was typed, and Escape closes the list. A mention is written `@name`, or `@` and
//! the first eight characters of the id for a name two or more share, which posting would refuse;
//! posting stores either as the chosen principal's id (`fwd_core::naming::mention_of`).
//!
//! The list takes the keys it uses before its box does, and marks them used, so neither the box
//! nor the page acts on them (`thread.rs`, `cards.rs`). The pointer's press on an offer is kept
//! from taking the focus, so the box keeps its caret while a click chooses.

use crate::person;
use crate::store;
use fwd_core::dormancy;
use fwd_core::naming;
use fwd_core::references::{self, Typing};
use leptos::html::Textarea;
use leptos::prelude::*;
use web_sys::KeyboardEvent;

/// How many principals the list offers at most.
const MOST: usize = 8;

/// A box's mention search: the box, where its caret is while it has the focus (in bytes of its
/// text), which offer the keys are on, and the `@` the list was closed at with Escape, which keeps
/// it closed for that mention.
#[derive(Clone, Copy)]
pub struct Mentions {
    field: NodeRef<Textarea>,
    caret: RwSignal<Option<usize>>,
    active: RwSignal<usize>,
    closed: RwSignal<Option<usize>>,
}

/// One principal the list offers: its name, what it is, the scopes beside a name two or more
/// principals share, how the text writes its mention, and whether it is a dormant agent, which the
/// list offers after the others, under a heading (`fwd_core::dormancy`).
#[derive(Clone, Debug, PartialEq)]
struct Offer {
    handle: String,
    what: &'static str,
    scopes: Option<Vec<String>>,
    written: String,
    dormant: bool,
}

impl Mentions {
    pub fn new(field: NodeRef<Textarea>) -> Mentions {
        Mentions { field, caret: RwSignal::new(None), active: RwSignal::new(0), closed: RwSignal::new(None) }
    }

    /// Reads where the caret is after the box's text changed: what is typed chooses the offers
    /// anew, so the keys start again at the first.
    pub fn typed(self) {
        self.active.set(0);
        self.moved();
    }

    /// Reads where the caret is after a key or a click moved it in the box.
    pub fn moved(self) {
        let caret = self.field.get_untracked().and_then(|field| {
            let at = field.selection_start().ok().flatten()?;
            Some(byte_at(&field.value(), at as usize))
        });
        self.caret.set(caret);
    }

    /// Closes the list once the box has lost the focus.
    pub fn left(self) {
        self.caret.set(None);
    }

    /// The mention being typed in `text` at `caret` and the principals offered for it, while the
    /// list is open.
    fn open(text: &str, caret: Option<usize>, closed: Option<usize>) -> Option<(Typing, Vec<Offer>)> {
        let typing = references::mention_at(text, caret?)?;
        if closed == Some(typing.range.start) {
            return None;
        }
        let principals = store::principals();
        let now = fwd_core::now_ms();
        let offers: Vec<Offer> = naming::mentionable(&principals, &typing.typed, MOST, now)
            .into_iter()
            .map(|p| {
                let handle = p.handle().unwrap_or_default();
                Offer {
                    what: person::role(p),
                    scopes: naming::shows_scopes(&principals, &handle).then(|| store::scope_titles_of(p.id())),
                    written: naming::mention_of(&principals, p),
                    dormant: dormancy::is_dormant(p, now),
                    handle,
                }
            })
            .collect();
        (!offers.is_empty()).then_some((typing, offers))
    }

    /// The keys the open list takes before its box: Up and Down move between the offers, Enter and
    /// Tab write the one the keys are on, and Escape closes the list. True when it took the key,
    /// whose default it prevented, so the box and the page leave it; `write` receives the text with
    /// the mention written. A key held with a modifier, or pressed inside an input method's
    /// composition (whose Enter confirms the composed text), is left to the box.
    pub fn key(self, ev: &KeyboardEvent, text: &str, write: impl Fn(String)) -> bool {
        if ev.alt_key() || ev.ctrl_key() || ev.meta_key() || ev.is_composing() || ev.key_code() == 229 {
            return false;
        }
        let open = Self::open(text, self.caret.get_untracked(), self.closed.get_untracked());
        let Some((typing, offers)) = open else {
            return false;
        };
        let count = offers.len();
        let active = self.active.get_untracked().min(count - 1);
        match ev.key().as_str() {
            "ArrowDown" => self.active.set((active + 1) % count),
            "ArrowUp" => self.active.set((active + count - 1) % count),
            "Enter" | "Tab" if !ev.shift_key() => self.choose(text, &typing, &offers[active], write),
            "Escape" => self.closed.set(Some(typing.range.start)),
            _ => return false,
        }
        ev.prevent_default();
        true
    }

    /// Writes `offer`'s mention in `text` in place of `typing`, with a space after it unless one
    /// follows already, and puts the caret after that space.
    fn choose(self, text: &str, typing: &Typing, offer: &Offer, write: impl Fn(String)) {
        let rest = &text[typing.range.end..];
        let space = if rest.starts_with(char::is_whitespace) { "" } else { " " };
        let written = format!("{}{}{space}{rest}", &text[..typing.range.start], offer.written);
        let caret = typing.range.start + offer.written.len() + 1;
        if let Some(field) = self.field.get_untracked() {
            field.set_value(&written);
            let at = units_at(&written, caret.min(written.len())) as u32;
            let _ = field.set_selection_range(at, at);
            let _ = field.focus();
        }
        self.active.set(0);
        write(written);
        self.moved();
    }
}

/// The byte of `text` that `units` UTF-16 units into it reach, as a text field counts its caret.
fn byte_at(text: &str, units: usize) -> usize {
    let mut counted = 0;
    for (byte, c) in text.char_indices() {
        if counted >= units {
            return byte;
        }
        counted += c.len_utf16();
    }
    text.len()
}

/// How many UTF-16 units the first `byte` bytes of `text` take, as a text field counts its caret.
fn units_at(text: &str, byte: usize) -> usize {
    text[..byte].encode_utf16().count()
}

/// The list beneath a box, while the caret is in a mention being typed and some principal's name
/// starts with it; `text` is the box's text and `write` takes the text with the mention written.
#[component]
pub fn MentionList(mentions: Mentions, text: Signal<String>, write: Callback<String>) -> impl IntoView {
    move || {
        let text = text.get();
        let (typing, offers) = Mentions::open(&text, mentions.caret.get(), mentions.closed.get())?;
        let active = mentions.active.get().min(offers.len() - 1);
        // The dormant agents come last (`naming::mentionable`), under a heading of their own that
        // the keys pass over.
        let first_dormant = offers.iter().position(|offer| offer.dormant);
        let rows = offers
            .into_iter()
            .enumerate()
            .map(|(i, offer)| {
                let heading = (Some(i) == first_dormant).then(|| {
                    view! { <li class="mention-heading" role="presentation">"dormant"</li> }
                });
                let (text, typing, chosen) = (text.clone(), typing.clone(), offer.clone());
                // Three scopes and a count of the rest, as a refusal and a picker name them.
                let scopes = offer.scopes.map(|scopes| {
                    if scopes.is_empty() {
                        return view! { <span class="muted">"no scope"</span> }.into_any();
                    }
                    let more = scopes.len().saturating_sub(naming::SCOPES_NAMED);
                    let named = scopes
                        .into_iter()
                        .take(naming::SCOPES_NAMED)
                        .map(|title| view! { <span class="pill scope-active">{title}</span> })
                        .collect::<Vec<_>>();
                    view! { {named} {(more > 0).then(|| view! { <span class="muted">{format!("+{more}")}</span> })} }
                        .into_any()
                });
                view! {
                    {heading}
                    <li role="option" class="mention-offer" class:active=move || i == active aria-selected=(i == active).to_string()
                        on:mousedown=|ev| ev.prevent_default()
                        on:click=move |_| mentions.choose(&text, &typing, &chosen, |t| write.run(t))>
                        <span class="author">{offer.handle}</span>
                        <span class="muted">{offer.what}</span>
                        {scopes}
                    </li>
                }
            })
            .collect::<Vec<_>>();
        Some(view! { <ul class="mention-list" role="listbox" aria-label="people and agents to mention">{rows}</ul> })
    }
}

#[cfg(test)]
mod tests {
    use super::{byte_at, units_at};

    #[test]
    fn a_caret_counted_in_utf16_units_is_found_in_the_texts_bytes_and_back() {
        let text = "é 😀 @d";
        // é is 1 unit and 2 bytes, the emoji 2 units and 4 bytes.
        assert_eq!(byte_at(text, 0), 0);
        assert_eq!(byte_at(text, 1), 2);
        assert_eq!(byte_at(text, 4), 7, "after the emoji");
        assert_eq!(byte_at(text, 7), text.len());
        assert_eq!(byte_at(text, 99), text.len(), "past the end is the end");
        for byte in [0, 2, 3, 7, 8, 9, text.len()] {
            assert_eq!(byte_at(text, units_at(text, byte)), byte, "byte {byte}");
        }
    }
}
