//! The panel a page shows with its list, sized by the person reading it, so that a long thread is
//! not read in a narrow column however wide the window: the Tasks page's task panel, the Decisions
//! page's decision panel, and the Graph page's task panel (task CyqBDRUz, "Give the Graph page's
//! task panel and the decision panel the same Full screen control"). A button in its corner lets
//! it fill the page, the list hidden, until the same button or Escape gives the list its place
//! back, and dragging its left edge sets how wide it is. Wider than 900 px the list and the panel
//! sit side by side in the page's split (`.split` in app.css), and only there does the edge show;
//! narrower, in the VS Code panel and on a phone, the panel sits beneath the list and shares the
//! view's height with it, and the button is how the panel gets the whole view. On the Graph page
//! the drawing stands where the other pages have their list, and what is said here of the list
//! holds for the drawing there. In the VS Code webview a task's panel on the Tasks page opens
//! filling the page by itself, with a Back control beside the button (`back.rs`).
//!
//! The width a person drags to holds for the visit and is remembered in this browser
//! (localStorage), each page's under a key of its own, when the browser lets the page keep it.
//! The stylesheet keeps the panel at least 320 px wide and the list at least 240 px, whatever the
//! width says, so a width chosen in a wide window fits a narrower one, and comes back when the
//! window widens again.

use crate::{keys, lozenges};
use leptos::prelude::*;
use send_wrapper::SendWrapper;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

/// The narrowest the panel gets and the narrowest it leaves the list, in CSS pixels: the bounds
/// the stylesheet's `.split.resizable` rule keeps the panel within as the window changes.
const MIN_PANEL: f64 = 320.0;
const MIN_LIST: f64 = 240.0;

/// How wide the panel beside a list is, and whether it fills the page: `Copy` handles only, so
/// the page, the panel's edge and its button share one.
#[derive(Clone, Copy)]
pub struct PanelSize {
    /// The panel's width in CSS pixels as last dragged; none until someone drags it, when the
    /// stylesheet's 560 px holds.
    width: RwSignal<Option<f64>>,
    /// Whether the panel fills the page, the list hidden.
    pub full: RwSignal<bool>,
    /// Whether the edge is being dragged: the page then selects no text.
    pub dragging: RwSignal<bool>,
    /// The localStorage key the width is remembered under.
    key: &'static str,
}

impl PanelSize {
    /// The panel's size on a page, with the width this browser remembers under `key`. Called once
    /// in the page's component, never in a part of it that is drawn again, so that the page has
    /// one listener for Escape. The listener gives the list its place back while the page is
    /// there and goes with the page, so the page shown next is the only one listening.
    pub fn new(key: &'static str) -> Self {
        let size = PanelSize {
            width: RwSignal::new(remembered(key)),
            full: RwSignal::new(false),
            dragging: RwSignal::new(false),
            key,
        };
        size.listen_for_escape();
        size
    }

    /// The dragged width, for the split's `--panel-width`, from which the stylesheet sizes the
    /// panel. The page sets it on the split's style object, never in its `style` attribute,
    /// which the VS Code webview's Content Security Policy refuses (`lozenges::restyle`): written
    /// there, the dragged width never applied in the webview.
    pub fn width_property(self) -> Option<String> {
        self.width.get().map(|w| format!("{w:.0}px"))
    }

    /// Escape gives the list its place back while the panel fills the page, unless something else
    /// on the page takes the key first: a reply box, a text field, a lozenge's open preview
    /// (`Escape`). The listener is on the document, which a key reaches after the page's own
    /// elements and before the window, where the lozenge preview listens, so the preview is still
    /// open when this one looks. It goes with the page.
    fn listen_for_escape(self) {
        let Some(document) = web_sys::window().and_then(|w| w.document()) else { return };
        let on_key = Closure::<dyn Fn(web_sys::KeyboardEvent)>::new(move |ev: web_sys::KeyboardEvent| {
            if ev.key() != "Escape" {
                return;
            }
            let active = web_sys::window().and_then(|w| w.document()).and_then(|d| d.active_element());
            let escape = Escape {
                used: ev.default_prevented(),
                modified: !keys::is_plain(&ev),
                full: self.full.get_untracked(),
                typing: active.is_some_and(|a| keys::is_text_field(&a)),
                previewing: lozenges::previewing(),
            };
            if escape.restores() {
                ev.prevent_default();
                self.full.set(false);
            }
        })
        .into_js_value();
        if document.add_event_listener_with_callback("keydown", on_key.unchecked_ref()).is_err() {
            return;
        }
        let (document, on_key) = (SendWrapper::new(document), SendWrapper::new(on_key));
        on_cleanup(move || {
            let _ = document.remove_event_listener_with_callback("keydown", on_key.unchecked_ref());
        });
    }
}

/// Escape pressed on the page, with what decides whether the panel takes it: the key gives the
/// list its place back only while the panel fills the page and nothing else on the page takes it.
#[derive(Clone, Copy, Default)]
struct Escape {
    /// A handler on the page used the key already: a reply box's Escape leaves the box for its row.
    used: bool,
    /// A modifier key was held with it.
    modified: bool,
    /// The panel fills the page, the list hidden.
    full: bool,
    /// A text field has the keyboard, and Escape there is the field's.
    typing: bool,
    /// A lozenge's preview is open, and Escape closes it first.
    previewing: bool,
}

impl Escape {
    /// Whether this press gives the list its place back.
    fn restores(self) -> bool {
        self.full && !(self.used || self.modified || self.typing || self.previewing)
    }
}

/// The panel's left edge, lying over its border: dragging it with a mouse, a finger or a pen sets
/// the panel's width, and the width is remembered once the drag ends. The page puts it in the
/// split, beside the panel, and it shows only while the panel is beside the list. It takes no
/// keyboard focus: the button is the keyboard's way to a wider panel.
#[component]
pub fn PanelEdge(size: PanelSize) -> impl IntoView {
    let edge = NodeRef::<leptos::html::Div>::new();
    // Where the drag began: the pointer's x and the panel's width then, in CSS pixels.
    let start = StoredValue::new(None::<(f64, f64)>);
    let end = move || {
        if start.get_value().is_none() {
            return;
        }
        start.set_value(None);
        size.dragging.set(false);
        if let Some(width) = size.width.get_untracked() {
            remember(size.key, width);
        }
    };
    // A drag cut short by the edge going (the panel closed, or another task opened, mid-drag)
    // leaves the page selecting text again.
    on_cleanup(move || size.dragging.set(false));
    let press = move |ev: web_sys::PointerEvent| {
        // The main button of a mouse; a finger or a pen touching counts as that button too.
        if ev.button() != 0 {
            return;
        }
        let Some(edge) = edge.get_untracked() else { return };
        let Some((_, panel)) = split_and_panel(&edge) else { return };
        // No text selection starts, and a finger does not scroll the page instead.
        ev.prevent_default();
        // The rest of the drag comes to the edge wherever the pointer goes.
        let _ = edge.set_pointer_capture(ev.pointer_id());
        start.set_value(Some((f64::from(ev.client_x()), panel.get_bounding_client_rect().width())));
        size.dragging.set(true);
    };
    let drag = move |ev: web_sys::PointerEvent| {
        let Some((x, width)) = start.get_value() else { return };
        // A pointer moving with no button held was released where the edge did not hear it.
        if ev.buttons() == 0 {
            end();
            return;
        }
        let Some((split, _)) = edge.get_untracked().and_then(|e| split_and_panel(&e)) else { return };
        let wanted = width + x - f64::from(ev.client_x());
        size.width.set(Some(bounded(wanted, split.get_bounding_client_rect().width()).round()));
    };
    view! {
        <div class="panel-edge" node_ref=edge role="separator" aria-orientation="vertical"
            title="Drag to widen or narrow the panel"
            on:pointerdown=press on:pointermove=drag
            on:pointerup=move |_| end() on:pointercancel=move |_| end()></div>
    }
}

/// The button in the panel's corner that lets the panel fill the page, and gives the list its
/// place back. It shows at every width: beneath the list, on a narrow page, it is how the panel
/// gets the whole view (in the VS Code webview a task opens so, and the button gives the list
/// back). Its words name what the panel hides, `hides`, so that a reader knows what comes back:
/// "the list" unless the page says otherwise, and "the graph" on the Graph page, whose panel hides
/// the drawing. They do not say where it goes, since that is beside the panel on a wide page and
/// above it on a narrow one.
#[component]
pub fn FullScreenToggle(size: PanelSize, #[prop(default = "the list")] hides: &'static str) -> impl IntoView {
    let full = size.full;
    view! {
        <button class="btn subtle full-screen-toggle"
            aria-label=move || if full.get() { format!("Restore {hides}") } else { "Show the panel full screen".into() }
            title=move || {
                if full.get() {
                    format!("Bring {hides} back (Escape)")
                } else {
                    format!("Let the panel fill the page, {hides} hidden (Escape brings it back)")
                }
            }
            on:click=move |_| full.update(|f| *f = !*f)>
            {move || if full.get() { "Restore" } else { "Full screen" }}
        </button>
    }
}

/// Scrolls a panel into view as it opens on a page too narrow to show it beside its list, where it
/// goes beneath the list (below 900 px, as the stylesheet says) and would otherwise open out of
/// sight: for a panel a button above the list opens, a new scope's or a new task's. `pane` is an
/// element at the top of the panel.
pub fn show_beneath_list(pane: NodeRef<leptos::html::Div>) {
    Effect::new(move |_| {
        let Some(pane) = pane.get() else { return };
        let narrow = web_sys::window()
            .and_then(|w| w.match_media("(max-width: 900px)").ok().flatten())
            .is_some_and(|query| query.matches());
        if narrow {
            request_animation_frame(move || pane.scroll_into_view());
        }
    });
}

/// The split an edge lies in, and the panel it sizes: the split's `.detail-pane`.
fn split_and_panel(edge: &web_sys::Element) -> Option<(web_sys::Element, web_sys::Element)> {
    let split = edge.parent_element()?;
    let panel = split.query_selector(":scope > .detail-pane").ok().flatten()?;
    Some((split, panel))
}

/// The width the panel takes when a drag asks for `wanted` in a split `split` wide: at least
/// `MIN_PANEL`, and leaving the list at least `MIN_LIST`; the panel's minimum wins in a split too
/// narrow for both.
fn bounded(wanted: f64, split: f64) -> f64 {
    wanted.min(split - MIN_LIST).max(MIN_PANEL)
}

fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

/// The width this browser remembers under `key`, when it has one and lets the page read it.
fn remembered(key: &str) -> Option<f64> {
    width_from(&local_storage()?.get_item(key).ok().flatten()?)
}

/// A remembered width: a positive number of pixels; anything else is none, and the stylesheet's
/// width holds.
fn width_from(text: &str) -> Option<f64> {
    text.trim().parse::<f64>().ok().filter(|w| w.is_finite() && *w > 0.0)
}

/// Remembers `width` under `key` when the browser lets the page write there; it holds for the
/// visit either way.
fn remember(key: &str, width: f64) {
    if let Some(storage) = local_storage() {
        let _ = storage.set_item(key, &format!("{width:.0}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dragged_width_leaves_the_panel_and_the_list_room() {
        assert_eq!(bounded(700.0, 1400.0), 700.0);
        assert_eq!(bounded(100.0, 1400.0), MIN_PANEL, "the panel keeps its minimum");
        assert_eq!(bounded(1300.0, 1400.0), 1400.0 - MIN_LIST, "the list keeps its minimum");
        assert_eq!(bounded(500.0, 500.0), MIN_PANEL, "the panel's minimum wins in a split too narrow for both");
    }

    #[test]
    fn escape_gives_the_list_back_only_while_the_panel_fills_the_page_and_nothing_else_takes_it() {
        let full = Escape { full: true, ..Escape::default() };
        assert!(full.restores());
        for (what, escape) in [
            ("the panel shares the page with the list", Escape::default()),
            ("a reply box used it to leave the box", Escape { used: true, ..full }),
            ("a modifier key was held", Escape { modified: true, ..full }),
            ("a text field has the keyboard", Escape { typing: true, ..full }),
            ("a lozenge's preview is open", Escape { previewing: true, ..full }),
        ] {
            assert!(!escape.restores(), "{what}");
        }
    }

    #[test]
    fn a_remembered_width_is_a_positive_number_of_pixels() {
        assert_eq!(width_from("640"), Some(640.0));
        assert_eq!(width_from(" 612.5 "), Some(612.5));
        for text in ["", "wide", "-20", "0", "NaN", "inf"] {
            assert_eq!(width_from(text), None, "{text:?}");
        }
    }
}
