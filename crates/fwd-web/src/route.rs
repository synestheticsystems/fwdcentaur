//! The page's route, followed as it changes: the routes themselves, and how each is written, are
//! `fwd_core::route`'s, which the CLI's links share. A `hashchange` listener keeps the signal
//! current, and remembers the route the page left (`previous`); `go` sets the hash and lets the
//! listener do the rest, and `replace` does the same without a new step in the browser's history.
//! The daemon serves one page, so the path never changes and a reload lands where it was.

pub use fwd_core::route::{Route, ScopePanel};
use leptos::prelude::*;
use std::sync::{Mutex, OnceLock};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

static ROUTE: OnceLock<ArcRwSignal<Route>> = OnceLock::new();
/// The route the page was on before the one `ROUTE` holds, once the page has moved.
static PREVIOUS: Mutex<Option<Route>> = Mutex::new(None);

/// The current route, reactively. The listener is installed on first use and lives as long as
/// the page.
pub fn route() -> ArcRwSignal<Route> {
    ROUTE
        .get_or_init(|| {
            let signal = ArcRwSignal::new(Route::parse(&current_hash()));
            let target = signal.clone();
            let listener = Closure::<dyn FnMut()>::new(move || {
                let next = Route::parse(&current_hash());
                let left = target.get_untracked();
                if next != left && !next.fills_in(&left) {
                    if let Ok(mut previous) = PREVIOUS.lock() {
                        *previous = Some(left);
                    }
                }
                target.set(next);
            });
            if let Some(w) = web_sys::window() {
                let _ = w.add_event_listener_with_callback("hashchange", listener.as_ref().unchecked_ref());
            }
            listener.forget();
            signal
        })
        .clone()
}

/// The route the page was on before it came to the one it is on; none until the page has moved.
/// The listener keeps it whatever moved the page: `go`, `replace`, a link, the browser's own back
/// button, or VS Code sending the webview's page to a route. The page putting its scope choice in
/// a route that named none (`scope_filter::follow`) does not move it. It is where a task was
/// opened from, which Back on the task's panel returns to in the VS Code webview (`back.rs`).
pub fn previous() -> Option<Route> {
    PREVIOUS.lock().ok()?.clone()
}

pub fn go(route: Route) {
    if let Some(w) = web_sys::window() {
        let _ = w.location().set_hash(&route.hash());
    }
}

/// Takes the page to `route` in place of the route it is on, so the browser's back button skips
/// the one replaced: for a page that fills in what its route left out, as the page does with its
/// scope choice (`scope_filter::follow`).
pub fn replace(route: Route) {
    if let Some(w) = web_sys::window() {
        let _ = w.location().replace(&route.hash());
    }
}

fn current_hash() -> String {
    web_sys::window().and_then(|w| w.location().hash().ok()).unwrap_or_default()
}
