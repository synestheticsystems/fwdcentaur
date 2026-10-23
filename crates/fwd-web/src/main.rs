//! fwd-web: the web UI the daemon serves at `http://127.0.0.1:<port>/` (`fwd ui` opens it, and
//! `fwd ui --lan` gives a phone its LAN address). It is an ephemeral Ankurah node in the
//! browser, exactly like the CLI is one in a process: IndexedDB storage, the WASM WebSocket
//! connector to one project's socket on the daemon, live queries for every read and the shared
//! `fwd_core::ops` for every write. Leptos renders; Ankurah signals reach Leptos through
//! `ReactiveGraphObserver`. See docs/design/web-ui.md.

mod accordion;
mod anchor_row;
mod anchors;
mod app;
mod ask;
mod assignee;
mod attachments;
mod back;
mod cards;
mod claim;
mod comment_box;
mod decision_row;
mod decisions;
mod empty;
mod fmt;
mod graph;
mod inbox;
mod keys;
mod labels;
mod layout;
mod lozenges;
mod mentions;
mod message_row;
mod modes;
mod navigation;
mod new_task;
mod notification_card;
mod page_state;
mod panel;
mod person;
mod query_cache;
mod questions;
mod references;
mod referents;
mod request_row;
mod route;
mod scope_filter;
mod scope_panel;
mod scope_row;
mod scopes;
mod session;
mod speech;
mod steps;
mod store;
mod task_row;
mod task_scopes;
mod tasks;
mod thread;
mod waiting;
mod whose;

use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

fn main() {
    // A panic, during boot or later, leaves a sentence on the page instead of a blank or frozen
    // one; the browser console keeps the details.
    std::panic::set_hook(Box::new(|info| {
        console_error_panic_hook::hook(info);
        show_failure(&format!(
            "The page stopped on an internal error ({info}). Reload it; the browser console has the details."
        ));
    }));
    tracing_wasm::set_as_global_default_with_config(
        tracing_wasm::WASMLayerConfigBuilder::new().set_max_level(tracing::Level::INFO).build(),
    );
    spawn_local(async {
        match session::boot().await {
            Ok(()) => leptos::mount::mount_to_body(app::App),
            Err(reason) => {
                leptos::mount::mount_to_body(move || view! { <app::BootError reason=reason/> });
                session::announce("failed");
            }
        }
    });
}

/// Puts the error card at the top of the page with plain DOM calls: after a panic the Leptos
/// runtime may be the thing that broke.
fn show_failure(text: &str) {
    session::announce("failed");
    let Some(document) = web_sys::window().and_then(|w| w.document()) else { return };
    let (Some(body), Ok(card), Ok(title), Ok(line)) =
        (document.body(), document.create_element("div"), document.create_element("h1"), document.create_element("p"))
    else {
        return;
    };
    card.set_class_name("boot-error");
    title.set_text_content(Some("fwd"));
    line.set_text_content(Some(text));
    let _ = card.append_child(&title);
    let _ = card.append_child(&line);
    let _ = body.prepend_with_node_1(&card);
}
