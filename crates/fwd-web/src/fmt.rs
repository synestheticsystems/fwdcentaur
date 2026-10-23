//! Small renderers shared by the pages.

use crate::store::Group;
use ankurah::EntityId;
use fwd_core::model::TaskStatus;
use leptos::prelude::*;
use wasm_bindgen_futures::{spawn_local, JsFuture};

/// A picker's principals as `<optgroup>`s under their headings, each option the text
/// [`crate::store::principal_groups`] gives it (the bare handle, or beside a handle two or more
/// principals share the scopes each one's work is in) with the principal's id as its value, the
/// reference a choice stores; `selected` is the one shown as chosen.
pub fn principal_options(groups: Vec<Group>, selected: Option<EntityId>) -> Vec<AnyView> {
    groups
        .into_iter()
        .map(|group| {
            let options = group
                .members
                .into_iter()
                .map(|(id, text)| {
                    view! { <option value=id.to_base64() selected=Some(id) == selected>{text}</option> }
                })
                .collect::<Vec<_>>();
            view! { <optgroup label=group.label>{options}</optgroup> }.into_any()
        })
        .collect()
}

/// How long ago, as the CLI says it: "just now", "5m ago".
pub fn ago(ms: i64) -> String {
    fwd_core::reason::ago(ms)
}

/// When something changed, for a mark saying it changed after it was sent (a message's "edited,
/// 16:40"): the time of day in the reader's time zone, with the day before it when that day is
/// not today ("2026-09-27 16:40").
pub fn clock(ms: i64) -> String {
    let offset_ms =
        -(js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms as f64)).get_timezone_offset() as i64) * 60_000;
    clock_at(ms, js_sys::Date::now() as i64, offset_ms)
}

/// [`clock`] read at `now_ms`, in a time zone `offset_ms` east of UTC.
pub fn clock_at(ms: i64, now_ms: i64, offset_ms: i64) -> String {
    let minutes = (ms + offset_ms).div_euclid(60_000).rem_euclid(24 * 60);
    let time = format!("{:02}:{:02}", minutes / 60, minutes % 60);
    let day = fwd_core::decision_log::date_at(ms, offset_ms);
    if day == fwd_core::decision_log::date_at(now_ms, offset_ms) {
        time
    } else {
        format!("{day} {time}")
    }
}

pub fn short(id: EntityId) -> String {
    id.to_base64().chars().take(8).collect()
}

/// An entity's short id, which copies the whole id when clicked and says for a moment whether it
/// did ("copied", or "copy failed"), so a person can name the entity to the CLI or to someone else
/// (`fwd` takes the whole id, or its first six characters or more).
#[component]
pub fn CopyId(id: EntityId) -> impl IntoView {
    // What the last click did, for a moment: copied (true) or not (false).
    let outcome = RwSignal::new(None::<bool>);
    let full = id.to_base64();
    let on_click = {
        let full = full.clone();
        move |ev: leptos::ev::MouseEvent| {
            // The row around it keeps its own click (focus, selection) out of this.
            ev.stop_propagation();
            let full = full.clone();
            spawn_local(async move {
                let copied = copy_text(&full).await;
                let _ = outcome.try_set(Some(copied));
                let clear = move || {
                    let _ = outcome.try_set(None);
                };
                set_timeout(clear, std::time::Duration::from_millis(1200));
            });
        }
    };
    let said = move || {
        outcome.get().map(|copied| match copied {
            true => view! { <span class="copied">"copied"</span> }.into_any(),
            false => view! { <span class="copy-failed">"copy failed"</span> }.into_any(),
        })
    };
    view! {
        <button class="copy-id mono" title=format!("{full}: click to copy") on:click=on_click>
            {short(id)}
            {said}
        </button>
    }
}

/// Puts `text` on the clipboard and answers whether it is there: through the Clipboard API where
/// the page may use it (on loopback), once the browser says the write succeeded; else, or when the
/// browser refuses the write, through the older copy command, whose own answer is the answer. A
/// page served over plain HTTP on the LAN has no Clipboard API at all.
async fn copy_text(text: &str) -> bool {
    let Some(window) = web_sys::window() else { return false };
    let navigator = window.navigator();
    let has_api = js_sys::Reflect::get(&navigator, &"clipboard".into()).is_ok_and(|c| !c.is_undefined());
    if has_api && JsFuture::from(navigator.clipboard().write_text(text)).await.is_ok() {
        return true;
    }
    copy_command(&window, text)
}

/// The older copy command: a hidden text box holding `text` is selected and copied. True when
/// the browser says it copied.
fn copy_command(window: &web_sys::Window, text: &str) -> bool {
    use wasm_bindgen::JsCast;
    let Some(document) = window.document() else { return false };
    let Some(body) = document.body() else { return false };
    let Ok(field) = document.create_element("textarea") else { return false };
    let Ok(field) = field.dyn_into::<web_sys::HtmlTextAreaElement>() else { return false };
    field.set_value(text);
    // Hidden through its style object: the VS Code webview's Content Security Policy refuses a
    // style attribute the page writes (`lozenges::restyle`), which would leave the box unhidden in
    // the page's flow while it is selected.
    let style = web_sys::HtmlElement::style(&field);
    let _ = style.set_property("position", "fixed");
    let _ = style.set_property("opacity", "0");
    let _ = body.append_child(&field);
    field.select();
    let copied =
        document.dyn_into::<web_sys::HtmlDocument>().is_ok_and(|html| matches!(html.exec_command("copy"), Ok(true)));
    field.remove();
    copied
}

pub fn truncate(s: &str, n: usize) -> String {
    let mut out: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        out.push('…');
    }
    out
}

pub fn status_label(s: TaskStatus) -> String {
    s.to_string().replace('_', " ")
}

pub fn priority_label(p: Option<i64>) -> String {
    p.map(|p| format!("P{p}")).unwrap_or_default()
}

/// The order tasks list in: live work first, then the backlog, then what is closed.
pub fn status_rank(s: TaskStatus) -> u8 {
    match s {
        TaskStatus::InProgress => 0,
        TaskStatus::InReview => 1,
        TaskStatus::Open => 2,
        TaskStatus::Proposed => 3,
        TaskStatus::Done => 4,
        TaskStatus::Cancelled => 5,
        TaskStatus::Rejected => 6,
    }
}

pub fn is_closed(s: TaskStatus) -> bool {
    matches!(s, TaskStatus::Done | TaskStatus::Cancelled | TaskStatus::Rejected)
}

pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::clock_at;

    /// 2026-09-28 16:40 UTC.
    const EDITED: i64 = 1_790_613_600_000;
    const HOUR: i64 = 3_600_000;

    #[test]
    fn a_change_today_says_its_time_and_one_on_another_day_its_day_too() {
        assert_eq!(clock_at(EDITED, EDITED + HOUR, 0), "16:40");
        assert_eq!(clock_at(EDITED, EDITED + 9 * HOUR, 0), "2026-09-28 16:40", "read the next day");
        // In the reader's time zone: seven hours west of UTC it was 09:40 that morning.
        assert_eq!(clock_at(EDITED, EDITED + HOUR, -7 * HOUR), "09:40");
        // Nine hours east it was already the next day, which is today there.
        assert_eq!(clock_at(EDITED, EDITED + HOUR, 9 * HOUR), "01:40");
    }
}
