//! The shell: the top bar (brand, the project and its selector, the scope the Questions, Tasks,
//! Decisions and Graph pages list by, nav with the live count of what waits on the acting
//! principal, the acting principal, the
//! voice messages are read aloud in, the connection state), above it the bar that offers a newer
//! page once the daemon has restarted on a newer build, the page the route selects, and the one
//! preview every lozenge on the page shows (`lozenges.rs`).

use crate::cards::Notifications;
use crate::route::{self, Route};
use crate::scope_filter::{self, HeaderScope, ScopeChoice};
use crate::session::{self, ctx};
use crate::{anchors, decisions, graph, inbox, lozenges, questions, scopes, tasks};
use crate::{fmt, speech, store, waiting};
use ankurah::signals::Get as AnkurahGet;
use ankurah::EntityId;
use fwd_core::model::NotificationView;
use fwd_core::query::with_id;
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

#[component]
pub fn App() -> impl IntoView {
    if let Err(reason) = store::open() {
        return view! { <BootError reason=reason/> }.into_any();
    }
    let route = route::route();
    // The page component changes only when the section does; a selection change inside a
    // section reaches it as a signal, so filters, zoom and drafts survive a click.
    let section = Memo::new({
        let route = route.clone();
        move |_| route.get().section()
    });
    let selected = Memo::new({
        let route = route.clone();
        move |_| match route.get() {
            Route::Tasks { task: s, .. } | Route::Graph { task: s, .. } | Route::Decisions { decision: s, .. } => s,
            _ => None,
        }
    });
    // The scope the Questions, Tasks, Decisions and Graph pages list by, one choice for the four,
    // which each of their routes carries: none until the page has picked the one it opens on.
    let scope = scope_filter::follow(route.clone());
    // Whether the Tasks page's panel holds the form for a new task.
    let new_task = Memo::new({
        let route = route.clone();
        move |_| matches!(route.get(), Route::NewTask { .. })
    });
    // The question the Questions page selects, where a lozenge naming it leads.
    let question = Memo::new({
        let route = route.clone();
        move |_| match route.get() {
            Route::Questions { question, .. } => question,
            _ => None,
        }
    });
    // Whose Inbox or Questions page shows: the handle of the principal the route names, or none
    // for the principal the page acts as.
    let whose = Memo::new({
        let route = route.clone();
        move |_| match route.get() {
            Route::Inbox { whose } | Route::Questions { whose, .. } => whose,
            _ => None,
        }
    });
    // What the Scopes page's panel shows: a new scope, or one to edit.
    let scope_panel = Memo::new({
        let route = route.clone();
        move |_| match route.get() {
            Route::Scopes(panel) => panel,
            _ => None,
        }
    });
    // The highlight whose page shows.
    let anchor = Memo::new(move |_| match route.get() {
        Route::Anchor(anchor) => Some(anchor),
        _ => None,
    });
    // Whoever embeds the page hears each change of its connection (`session::announce`).
    Effect::new(move |_| session::announce(&session::client().connection_state().get().to_string()));
    session::follow_restarts();
    view! {
        <div class="shell">
            <NewerBuild/>
            <Header scope=scope/>
            <main class="main">
                {move || match section.get() {
                    "tasks" => view! { <tasks::TasksPage selected=selected scope=scope new_task=new_task/> }.into_any(),
                    "decisions" => view! { <decisions::DecisionsPage selected=selected scope=scope/> }.into_any(),
                    "graph" => view! { <graph::GraphPage selected=selected scope=scope/> }.into_any(),
                    "scopes" => view! { <scopes::Scopes panel=scope_panel/> }.into_any(),
                    "questions" => view! { <questions::Questions routed=question whose=whose scope=scope/> }.into_any(),
                    "anchors" => view! { <anchors::AnchorPage id=anchor/> }.into_any(),
                    _ => view! { <inbox::Inbox whose=whose scope=scope/> }.into_any(),
                }}
            </main>
            <lozenges::LozengePreview/>
        </div>
    }
    .into_any()
}

/// The top bar. `scope` is the scope the Questions, Tasks, Decisions and Graph pages list by, which
/// the select after the project's shows and the nav's links to those pages carry, so that it
/// follows the person from one to another, through the Inbox and the Scopes page too.
#[component]
fn Header(scope: Memo<Option<ScopeChoice>>) -> impl IntoView {
    view! {
        <header class="topbar">
            <div class="brand">
                <span class="mark">"fwd"</span>
                <ProjectPicker/>
                <HeaderScope chosen=scope/>
            </div>
            <nav class="nav">
                <NavLink to=Route::inbox() label="Inbox" scope=scope/>
                <WaitingBadge/>
                <NavLink to=Route::Questions { question: None, whose: None, scope: None } label="Questions" scope=scope/>
                <NavLink to=Route::tasks(None, None) label="Tasks" scope=scope/>
                <NavLink to=Route::Decisions { decision: None, scope: None } label="Decisions" scope=scope/>
                <NavLink to=Route::Graph { task: None, scope: None } label="Graph" scope=scope/>
                <NavLink to=Route::Scopes(None) label="Scopes" scope=scope/>
            </nav>
            <div class="topbar-right">
                <ActingPicker/>
                <VoicePicker/>
                <Connection/>
            </div>
        </header>
    }
}

/// The bar above the header that says the daemon restarted on a newer build than the page came
/// from, so that a page left open across the restart does not go on showing the old build's page
/// unnoticed: it shows once the daemon has come back on a build other than the one the page
/// booted against (`session::follow_restarts`). It offers to load the new page and never loads
/// it by itself, since a reload loses whatever is typed and not sent yet. In the VS Code webview
/// the page comes with the extension, so the bar offers to reload the window instead
/// (`session::reload`). Its tooltip names both builds.
#[component]
fn NewerBuild() -> impl IntoView {
    let newer = session::newer_build();
    move || {
        newer.get().map(|build| {
            let (says, button) = if session::locked() {
                (
                    "The daemon restarted on a newer build of fwd since this view opened. This view's page comes \
                     with the extension: reload the window to load the extension's newer page.",
                    "Reload window",
                )
            } else {
                (
                    "The daemon restarted on a newer build of fwd since this page loaded, and serves a newer page.",
                    "Reload page",
                )
            };
            let builds = format!("This page came from {}; the daemon now runs {build}.", session::booted_on());
            view! {
                <div class="newer-build" role="status" title=builds>
                    <span class="newer-build-text">{says}</span>
                    <button
                        class="btn primary tiny"
                        title="Anything typed and not sent yet is lost"
                        on:click=move |_| session::reload()
                    >
                        {button}
                    </button>
                </div>
            }
        })
    }
}

/// A link in the nav to the page `to` leads to, naming no task and no one else's page: with the
/// page's scope choice, `scope`, when the page lists by scope and the page has one.
#[component]
fn NavLink(to: Route, label: &'static str, scope: Memo<Option<ScopeChoice>>) -> impl IntoView {
    let route = route::route();
    let section = to.section();
    let href = move || scope.get().map_or_else(|| to.hash(), |choice| to.with_scope(choice).hash());
    view! {
        <a href=href class="navlink" class:active=move || route.get().section() == section>{label}</a>
    }
}

/// How many things wait on the acting principal: the rows of Waiting on you on their inbox, not
/// their unread notifications (D137; on 2026-10-03 the badge counted 212 unread notifications while
/// eleven things waited on Daniel). Its query is opened per principal, so switching the picker
/// mounts a fresh badge with a fresh query.
#[component]
fn WaitingBadge() -> impl IntoView {
    move || session::acting().get().map(|p| view! { <WaitingFor me=p.id()/> })
}

/// [`WaitingBadge`] for `me`: one query of their requests, read or not (`waiting::requests_selection`),
/// sorted once at each change as the Inbox sorts them (`waiting.rs`), so the badge counts the rows
/// the page shows, and what they answered from the messages they wrote (`waiting::answers`). It
/// shows once the daemon has answered the standing queries, which say what each request is about,
/// and the query of their messages: before, a request about something not arrived yet, or a
/// question already answered, would count as waiting.
#[component]
fn WaitingFor(me: EntityId) -> impl IntoView {
    let query = with_id(&waiting::requests_selection(), me)
        .ok()
        .and_then(|selection| ctx().query::<NotificationView>(selection).ok());
    let notifications = query.map_or_else(Notifications::none, Notifications::of);
    let (answered, answers_loaded) = waiting::answers(me);
    let waiting = Memo::new(move |_| {
        if !(store::standing_loaded() && answers_loaded.get()) {
            return 0;
        }
        answered.with(|answered| waiting::sorted_now(&notifications.all(), me, answered, None).waiting())
    });
    move || {
        let n = waiting.get();
        (n > 0).then(|| view! { <span class="badge" title=format!("{n} waiting on you: the rows of Waiting on you in your inbox")>{n}</span> })
    }
}

/// The card a page shows instead of its project: why, and (unless the page is pinned to one
/// project) a link to every other project the daemon has open, since the header and its
/// selector never appear.
#[component]
pub fn BootError(reason: String) -> impl IntoView {
    let current = session::project().id;
    let others: Vec<session::Project> = if session::locked() {
        Vec::new()
    } else {
        session::projects().into_iter().filter(|p| p.error.is_none() && p.id != current).collect()
    };
    view! {
        <div class="boot-error">
            <h1>"fwd"</h1>
            <p>{reason}</p>
            {(!others.is_empty())
                .then(|| {
                    view! {
                        <p>
                            "Other projects: "
                            {others
                                .into_iter()
                                .map(|p| view! { <a href=format!("?p={}", p.id)>{p.name}</a> " " })
                                .collect::<Vec<_>>()}
                        </p>
                    }
                })}
        </div>
    }
}

/// Which project the page shows, among every project the daemon hosts. Choosing another reloads
/// the page on that project's node. The selector is always there (Daniel, 2026-09-26: "I want a
/// project selector in the web UI"), even with one project; only a page pinned to one project
/// (the VS Code webview, `?lock=1`) shows the name alone. The daemon's list is read again when
/// the selector takes the focus or is opened, and every 30 seconds while the page is in view, so
/// a project registered after the page opened appears without a reload; the options change in
/// place and the page's own project stays the one selected. One the daemon could not open shows
/// disabled, with its reason on hover.
#[component]
fn ProjectPicker() -> impl IntoView {
    let current = session::project();
    if session::locked() {
        return view! { <span class="ws" title="project">{current.name}</span> }.into_any();
    }
    let refresh = move || spawn_local(session::refresh_projects());
    let in_view = || web_sys::window().and_then(|w| w.document()).is_some_and(|d| !d.hidden());
    if let Ok(every) = set_interval_with_handle(
        move || {
            if in_view() {
                refresh();
            }
        },
        std::time::Duration::from_secs(30),
    ) {
        on_cleanup(move || every.clear());
    }
    let projects = session::project_list();
    let options = move || {
        projects
            .get()
            .into_iter()
            .map(|p| {
                let selected = p.id == current.id;
                let label = if p.error.is_some() { format!("{} (could not open)", p.name) } else { p.name.clone() };
                view! {
                    <option value=p.id.clone() selected=selected disabled=p.error.is_some() title=p.error.clone()>
                        {label}
                    </option>
                }
            })
            .collect::<Vec<_>>()
    };
    let on_change = move |ev: leptos::ev::Event| session::switch_project(&event_target_value(&ev));
    view! {
        <select class="project" title="project" on:change=on_change on:focus=move |_| refresh() on:mousedown=move |_| refresh()>
            {options}
        </select>
    }
    .into_any()
}

/// Who the page acts as, as the CLI's `--as` says whom a command acts as: the person by default, or
/// any principal of the project chosen here (those that belong to it: docs/DECISIONS.md, D106), to whom everything the page writes from then on is attributed
/// and on whose own Inbox and Questions page the nav's links open (`session::acting`). Looking at
/// another principal's inbox or questions needs no switch here: those pages' own selector shows one
/// other principal's without acting as them (`whose.rs`; task jPQy9aQC, "Open the Questions and
/// Inbox pages on your own, with a selector to look at another principal's"). Handles are free
/// text, so the options are grouped under what each principal is (people, then agents by the role
/// stored on them, the dormant agents last under a heading of their own), show the bare handle, with
/// the scopes each one's work is in beside a handle two or more principals share
/// (`store::principal_groups`), and carry the principal's id as their value.
#[component]
fn ActingPicker() -> impl IntoView {
    let acting = session::acting();
    let on_change = {
        let acting = acting.clone();
        move |ev: leptos::ev::Event| {
            let Ok(id) = event_target_value(&ev).parse::<EntityId>() else { return };
            if let Some(p) = store::principals().into_iter().find(|p| p.id() == id) {
                acting.set(Some(p));
            }
        }
    };
    // The options are drawn again only when the groups change, and not at every write that stamps
    // its writer's principal (`fwd_core::dormancy::wrote`).
    let groups = Memo::new(move |_| store::principal_groups(session::acting().get().map(|p| p.id())));
    view! {
        <label class="acting" title="Act as this principal (the CLI's --as)">
            <span class="acting-label">"as"</span>
            <select on:change=on_change>
                {move || {
                    let current = acting.get().map(|p| p.id());
                    fmt::principal_options(groups.get(), current)
                }}
            </select>
        </label>
    }
}

/// The voice the inbox reads a message aloud in (Shift twice), a setting of this browser. Hidden
/// while the browser offers no English voice.
#[component]
fn VoicePicker() -> impl IntoView {
    let voices = speech::voices();
    let chosen = speech::chosen();
    move || {
        let voices = voices.get();
        if voices.is_empty() {
            return None;
        }
        let speaking = speech::effective(&voices, chosen.get().as_deref());
        let options = voices
            .into_iter()
            .map(|name| {
                let selected = speaking.as_deref() == Some(name.as_str());
                view! { <option value=name.clone() selected=selected>{name.clone()}</option> }
            })
            .collect::<Vec<_>>();
        Some(view! {
            <label class="acting" title="The voice the inbox reads a message aloud in: press Shift twice">
                <span class="acting-label">"voice"</span>
                <select on:change=move |ev| speech::choose(event_target_value(&ev))>
                    <option value="" selected=speaking.is_none()>"browser default"</option>
                    {options}
                </select>
            </label>
        })
    }
}

#[component]
fn Connection() -> impl IntoView {
    move || {
        let state = session::client().connection_state().get().to_string();
        let class = format!("conn conn-{}", state.to_lowercase());
        view! { <span class=class title=state.clone()>{state.clone()}</span> }
    }
}
