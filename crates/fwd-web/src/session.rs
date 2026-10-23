//! The browser's ephemeral node, the connection to the daemon, and who is acting through it.
//!
//! Boot order: check that the daemon answers `/health` and note the build it names, list the
//! projects it hosts, pick one (the one the URL names with `?p=<uuid>`, else the one this tab
//! last opened while the daemon still has it open, else the first that opened), open a fresh
//! IndexedDB database for it, connect to that project's socket, wait for the node to join the
//! project's system and finish initializing, remember the project for this tab, install the
//! reactive_graph bridge, then pick the acting principal: `?as=<handle>` in the URL, otherwise
//! the project's person. The daemon is the page's own origin, which serves the page, unless
//! `?daemon=http://127.0.0.1:<port>` names it on a page no daemon serves: the VS Code webview
//! runs the page from the extension's own files, so its origin is not the daemon's. The
//! selection lives in the query string because the page's router owns the fragment (`#/inbox`
//! and the rest). Every failure becomes a sentence that names the cause and the command that
//! fixes it, and the page tells whoever embeds it how it is doing (`announce`). A page keeps
//! running when the daemon restarts, so each time its connection comes back it asks `/health`
//! again, and offers to load the new page when the daemon now runs another build
//! (`follow_restarts`).

use ankurah::signals::Get as AnkurahGet;
use ankurah::{policy::DEFAULT_CONTEXT, Context, EntityId, Node, PermissiveAgent};
use ankurah_signals::{CurrentObserver, ReactiveGraphObserver};
use ankurah_storage_indexeddb_wasm::IndexedDBStorageEngine;
use ankurah_websocket_client_wasm::WebsocketClient;
use fwd_core::model::PrincipalView;
use fwd_core::naming;
use fwd_core::ops;
use leptos::prelude::*;
use send_wrapper::SendWrapper;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::window;

static NODE: OnceLock<Node<IndexedDBStorageEngine, PermissiveAgent>> = OnceLock::new();
static CONTEXT: OnceLock<Context> = OnceLock::new();
static CLIENT: OnceLock<SendWrapper<WebsocketClient>> = OnceLock::new();
static PROJECT: OnceLock<Project> = OnceLock::new();
static PROJECTS: OnceLock<ArcRwSignal<Vec<Project>>> = OnceLock::new();
/// The daemon's address when `?daemon=` named it (checked at boot), for reading `/projects` again.
static DAEMON: OnceLock<Option<String>> = OnceLock::new();
static LOCKED: OnceLock<bool> = OnceLock::new();
static ACTING: OnceLock<ArcRwSignal<Option<PrincipalView>>> = OnceLock::new();
/// The build the daemon named on `/health` when the page booted: for a page the daemon served,
/// the build the page came from. `None` from a daemon built before `/health` named its build;
/// unset when the answer could not be read, and the page then never offers a newer build.
static BOOTED_ON: OnceLock<Option<String>> = OnceLock::new();
static NEWER_BUILD: OnceLock<ArcRwSignal<Option<String>>> = OnceLock::new();

/// How long the node may take to join the daemon's system before the page gives up with a
/// sentence instead of a blank screen.
const JOIN_TIMEOUT_MS: i32 = 8_000;
const POLL_STEP_MS: i32 = 50;

/// The project this tab last opened, so a page opened without `?p=` returns to it. Written only
/// once the project has opened: a project the daemon cannot open is never remembered, so a
/// reload does not keep returning to it.
const CHOSEN: &str = "fwd.web.project";

const NO_PROJECTS: &str = "No projects yet: run `fwd init` in a repository.";

/// A project the daemon hosts, as its project list reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Project {
    pub id: String,
    pub name: String,
    /// Why the daemon could not open it, when it could not.
    pub error: Option<String>,
}

/// The context every read and transaction goes through: the one `boot` obtained once the node
/// had joined the project and finished initializing. Valid once `boot` has returned `Ok`.
pub fn ctx() -> Context {
    CONTEXT.get().expect("the page has not booted").clone()
}

pub fn client() -> WebsocketClient {
    (**CLIENT.get().expect("client not booted")).clone()
}

/// The project this page shows.
pub fn project() -> Project {
    PROJECT.get().cloned().unwrap_or(Project { id: String::new(), name: String::new(), error: None })
}

/// Every project the daemon hosts, as last read.
pub fn projects() -> Vec<Project> {
    project_list().get_untracked()
}

/// Every project the daemon hosts, as a signal the header's selector redraws from: read at boot,
/// and again by `refresh_projects`, so a project registered while the page is open appears.
pub fn project_list() -> ArcRwSignal<Vec<Project>> {
    PROJECTS.get_or_init(|| ArcRwSignal::new(Vec::new())).clone()
}

/// Reads the daemon's project list again, from the address the page booted with, and replaces
/// the list only when it changed. A read that fails keeps the list as it was: the selector is a
/// convenience, and the page's own project is unaffected either way.
pub async fn refresh_projects() {
    let daemon = DAEMON.get().cloned().flatten();
    if let Ok(list) = hosted_projects(daemon.as_deref()).await {
        let projects = project_list();
        if projects.with_untracked(|current| *current != list) {
            projects.set(list);
        }
    }
}

/// True when the page was opened pinned to one project (`?lock=1`: the VS Code webview shows its
/// window's project only), so the header offers no other. The VS Code webview is the only page
/// opened so, which makes this how the page knows it runs there: a task then opens filling the
/// page, with a Back control on its panel, since VS Code gives the page no back button of its own
/// (`back.rs`).
pub fn locked() -> bool {
    LOCKED.get().copied().unwrap_or(false)
}

/// Who is acting: everything the page writes (a comment, a reply, a move, a start, a mark read) is
/// attributed to this principal; the Inbox and the Questions page open on theirs, and "mine" on the
/// Tasks page keeps the tasks assigned to them. Another principal's inbox or questions show without
/// switching it, from those pages' own selector, only to look (`whose.rs`; docs/DECISIONS.md, D104,
/// "The Questions and Inbox pages open on the viewer's own, and show one other principal's at a
/// time only to look"). A signal, so the header's picker can switch it (the CLI's `--as`).
pub fn acting() -> ArcRwSignal<Option<PrincipalView>> {
    ACTING.get_or_init(|| ArcRwSignal::new(None)).clone()
}

/// The acting principal's id, untracked (for event handlers).
pub fn me() -> Option<EntityId> {
    acting().get_untracked().map(|p| p.id())
}

/// Records that the page wrote as `me` just now (`fwd_core::dormancy::wrote`), as the CLI records
/// it after each command that writes work or changes a principal, so that an agent the page acts as
/// is never listed as dormant while it writes from here (docs/DECISIONS.md, D123): each write the
/// page makes but marking notifications read or dismissed calls it once the write has succeeded.
/// The write was made whatever becomes of the stamp, so a stamp that fails is logged rather than
/// shown: it moves the principal in no picker until its next write stamps it.
pub fn wrote(me: EntityId) {
    spawn_local(async move {
        if let Err(e) = fwd_core::dormancy::wrote(&ctx(), me).await {
            tracing::warn!("could not record that {} wrote from this page: {e}", me.to_base64());
        }
    });
}

/// Shows another project: the page loads again with the new `?p=`, which boots a fresh node on
/// that project's socket, at its inbox (and the tab remembers the project once it has opened).
/// `?as=` is left behind: principals belong to one project's graph, so the new page acts as
/// that project's person until the header's picker says otherwise.
pub fn switch_project(id: &str) {
    let Some(w) = window() else { return };
    let Ok(params) = web_sys::UrlSearchParams::new_with_str(&w.location().search().unwrap_or_default()) else {
        return;
    };
    params.set("p", id);
    params.delete("as");
    let path = w.location().pathname().unwrap_or_else(|_| "/".into());
    let query: String = params.to_string().into();
    let _ = w.location().set_href(&format!("{path}?{query}#/inbox"));
}

pub async fn boot() -> Result<(), String> {
    let w = window().ok_or("This page has no window to run in.")?;
    let session = w.session_storage().ok().flatten();
    let remembered = session.as_ref().and_then(|s| s.get_item(CHOSEN).ok().flatten()).filter(|v| !v.is_empty());
    let locked = query_param("lock").is_some_and(|v| v == "1");
    let protocol = w.location().protocol().unwrap_or_default();
    let daemon = match query_param("daemon") {
        Some(given) => daemon_param(&protocol, &given)?,
        None => None,
    };
    let _ = DAEMON.set(daemon.clone());
    let daemon = daemon.as_deref();

    let health = match fetch(daemon, "/health").await {
        Some(response) if response.ok() => response,
        _ => return Err(not_answering(daemon)),
    };
    let build = named_build(health).await;
    let projects = hosted_projects(daemon).await?;
    // Kept before anything else can fail, so the error card can offer the other projects.
    let booted_twice = || "the page booted twice".to_string();
    project_list().set(projects.clone());
    LOCKED.set(locked).map_err(|_| booted_twice())?;
    match build {
        Ok(build) => BOOTED_ON.set(build).map_err(|_| booted_twice())?,
        Err(why) => tracing::warn!("{why}; the page cannot tell when the daemon restarts on another build"),
    }
    let project = choose(&projects, query_param("p").as_deref(), remembered.as_deref(), locked)?;
    if let Some(error) = &project.error {
        return Err(format!(
            "The daemon could not open the project {}: {error}. `fwd daemon logs` shows why.",
            project.name
        ));
    }

    let storage = IndexedDBStorageEngine::open(&fresh_database(&project.id).await)
        .await
        .map_err(|e| format!("This browser refused IndexedDB storage for the page: {e}"))?;
    let node = Node::new(Arc::new(storage), PermissiveAgent::new());
    let client = WebsocketClient::new(node.clone(), &ws_url(daemon, &project.id))
        .map_err(|e| format!("The page could not open its connection to the daemon: {e}"))?;
    let context = join(&node).await?;
    if let Some(session) = &session {
        let _ = session.set_item(CHOSEN, &project.id);
    }
    NODE.set(node).map_err(|_| booted_twice())?;
    CONTEXT.set(context).map_err(|_| booted_twice())?;
    CLIENT.set(SendWrapper::new(client)).map_err(|_| booted_twice())?;
    PROJECT.set(project).map_err(|_| booted_twice())?;
    // From here on, every Ankurah signal read inside a Leptos closure registers a dependency.
    CurrentObserver::set(ReactiveGraphObserver::new());
    let principal = choose_principal().await?;
    acting().set(Some(principal));
    Ok(())
}

/// The project the URL names (with its error, when the daemon could not open it), else the one
/// this tab last opened when the daemon has it open, else the first one that opened. A page
/// pinned to a project the daemon does not host says so rather than showing another.
fn choose(
    projects: &[Project],
    named: Option<&str>,
    remembered: Option<&str>,
    locked: bool,
) -> Result<Project, String> {
    let hosted = |id: &str| projects.iter().find(|p| p.id == id).cloned();
    if let Some(id) = named {
        if let Some(project) = hosted(id) {
            return Ok(project);
        }
        if locked {
            return Err(format!(
                "The daemon does not host the project {id}. Run `fwd ui` in one of its repositories to open it."
            ));
        }
    }
    if let Some(project) = remembered.and_then(hosted).filter(|p| p.error.is_none()) {
        return Ok(project);
    }
    projects.iter().find(|p| p.error.is_none()).or(projects.first()).cloned().ok_or_else(|| NO_PROJECTS.to_string())
}

/// The daemon's address from `?daemon=`, on a page whose own origin (`protocol`) is neither
/// `http:` nor `https:`: the VS Code webview's page, whose origin is `vscode-webview:`. A page
/// served over HTTP was served by the daemon (or by a dev server proxying to it) and reaches it at
/// its own origin, so there the parameter is ignored: a crafted link must not point the daemon's
/// own page at another port on this machine, which would then receive whatever the person writes.
fn daemon_param(protocol: &str, given: &str) -> Result<Option<String>, String> {
    if matches!(protocol, "http:" | "https:") {
        tracing::warn!("ignored ?daemon={given}: a page served over {protocol} reaches the daemon at its own origin");
        return Ok(None);
    }
    daemon_address(given).map(Some)
}

/// The daemon's address from `?daemon=`: `http://`, a name of this machine (`127.0.0.1`,
/// `localhost` or `[::1]`) and a port, nothing more. Honoured only on a page no daemon serves
/// (`daemon_param`): the VS Code webview, which passes it, always reaches a daemon on its own
/// machine; an address elsewhere would let a crafted link send another server whatever the person
/// writes.
fn daemon_address(given: &str) -> Result<String, String> {
    let refuse = || {
        format!("The page's `daemon` parameter must name a daemon on this machine, such as http://127.0.0.1:<port>, not `{given}`.")
    };
    let rest = given.strip_prefix("http://").ok_or_else(refuse)?;
    let authority = rest.strip_suffix('/').unwrap_or(rest);
    let (host, port) = authority.rsplit_once(':').ok_or_else(refuse)?;
    let this_machine = matches!(host, "127.0.0.1" | "localhost" | "[::1]");
    let port_given = port.parse::<u16>().is_ok_and(|p| p != 0);
    if !this_machine || !port_given {
        return Err(refuse());
    }
    Ok(format!("http://{authority}"))
}

/// Why the page cannot show anything when the daemon does not answer, and what to run. At the
/// page's own address a fresh page from `fwd ui` finds the daemon wherever it listens now; at an
/// address `?daemon=` named (the VS Code webview), the page cannot tell a daemon that is down from
/// one that refuses the page's origin (the browser hides a refused answer), so it names both.
fn not_answering(daemon: Option<&str>) -> String {
    match daemon {
        None => "The daemon at this address is not answering. Run `fwd ui` (or `fwd ui --lan` for a phone) to open a fresh page.".into(),
        Some(address) => format!(
            "The daemon at {address} is not answering, or does not let this page in. `fwd daemon start` starts it (run `fwd daemon stop` first when a daemon from an older build is running); then open this page again."
        ),
    }
}

/// `GET path` from the daemon: at `daemon` when `?daemon=` named it, else on the page's own
/// origin. `None` when the request did not complete: nothing answered, or, from another origin,
/// the daemon's answer did not let this page read it.
async fn fetch(daemon: Option<&str>, path: &str) -> Option<web_sys::Response> {
    let url = format!("{}{path}", daemon.unwrap_or_default());
    let response = JsFuture::from(window()?.fetch_with_str(&url)).await.ok()?;
    response.dyn_into().ok()
}

/// The build a `/health` answer names: `None` from a daemon built before `/health` named its
/// build, and why not when the answer cannot be read.
async fn named_build(health: web_sys::Response) -> Result<Option<String>, String> {
    let text = match health.text() {
        Ok(promise) => JsFuture::from(promise).await.ok().and_then(|t| t.as_string()),
        Err(_) => None,
    }
    .ok_or("the daemon's health check could not be read")?;
    let health: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("the daemon's health check is not JSON: {e}"))?;
    Ok(health["build"].as_str().map(str::to_string))
}

async fn hosted_projects(daemon: Option<&str>) -> Result<Vec<Project>, String> {
    let response = fetch(daemon, "/projects").await.ok_or_else(|| not_answering(daemon))?;
    if response.status() != 200 {
        return Err(format!("The daemon refused the page's request for its projects (HTTP {}).", response.status()));
    }
    let text = match response.text() {
        Ok(promise) => JsFuture::from(promise).await.ok().and_then(|t| t.as_string()),
        Err(_) => None,
    }
    .ok_or("The daemon's project list could not be read.")?;
    let list: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("The daemon's project list is not JSON: {e}"))?;
    Ok(list
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|p| Project {
                    id: p["id"].as_str().unwrap_or_default().to_string(),
                    name: p["name"].as_str().unwrap_or_default().to_string(),
                    error: p["error"].as_str().map(str::to_string),
                })
                .collect()
        })
        .unwrap_or_default())
}

/// A new, empty IndexedDB database for this load, and the ones nobody uses any more deleted.
///
/// The page is an ephemeral node with a local daemon, so there is nothing worth persisting, and
/// a cache that outlives the daemon has shown stale live-query members after a restart. Every
/// load therefore names its own database (the project's id and a random suffix; two tabs never
/// share one) and holds a Web Lock named after it for as long as the page lives. At boot the
/// page lists the project's databases and deletes those whose lock it can take, which is
/// exactly the set no open tab is using. This tab's previous name (a reload) is remembered in
/// `sessionStorage`, never shared.
async fn fresh_database(project: &str) -> String {
    let prefix = format!("fwd-{project}-");
    let name = format!("{prefix}{}", random_suffix());
    if let Some(w) = window() {
        if let Some(session) = w.session_storage().ok().flatten() {
            let _ = session.set_item("fwd.web.db", &name);
        }
        hold_lock(&w, &name);
        sweep_unused(&w, &prefix, &name).await;
    }
    name
}

fn random_suffix() -> String {
    let uuid = window()
        .and_then(|w| js_sys::Reflect::get(&w, &"crypto".into()).ok())
        .and_then(|crypto| call(&crypto, "randomUUID", &[]))
        .and_then(|v| v.as_string());
    uuid.map(|u| u.replace('-', ""))
        .unwrap_or_else(|| format!("{:x}{:x}", js_sys::Date::now() as u64, (js_sys::Math::random() * 1e12) as u64))
}

/// `object.method(args...)`, or `None` when the browser has no such method. The Web Locks and
/// `indexedDB.databases()` APIs are reached this way rather than through web-sys bindings whose
/// names have moved between versions.
fn call(object: &JsValue, method: &str, args: &[&JsValue]) -> Option<JsValue> {
    let f: js_sys::Function = js_sys::Reflect::get(object, &method.into()).ok()?.dyn_into().ok()?;
    let result = match args {
        [] => f.call0(object),
        [a] => f.call1(object, a),
        [a, b] => f.call2(object, a, b),
        [a, b, c] => f.call3(object, a, b, c),
        _ => return None,
    };
    result.ok()
}

fn lock_manager(w: &web_sys::Window) -> Option<JsValue> {
    let navigator = js_sys::Reflect::get(w, &"navigator".into()).ok()?;
    js_sys::Reflect::get(&navigator, &"locks".into()).ok().filter(|v| !v.is_undefined() && !v.is_null())
}

/// `navigator.locks.request(name, () => new Promise(() => {}))`: held until the page unloads.
fn hold_lock(w: &web_sys::Window, name: &str) {
    let Some(locks) = lock_manager(w) else { return };
    let never =
        Closure::<dyn FnMut(JsValue) -> js_sys::Promise>::new(|_lock: JsValue| js_sys::Promise::new(&mut |_, _| {}));
    let _ = call(&locks, "request", &[&name.into(), never.as_ref()]);
    never.forget();
}

/// Deletes every database of this project's prefix whose lock is free: the databases of tabs
/// that are gone. `ifAvailable` makes the request answer at once with `null` when another tab
/// holds the lock, so a live tab is never touched.
async fn sweep_unused(w: &web_sys::Window, prefix: &str, keep: &str) {
    let Some(locks) = lock_manager(w) else { return };
    let Ok(idb) = js_sys::Reflect::get(w, &"indexedDB".into()) else { return };
    let Some(listing) = call(&idb, "databases", &[]) else { return };
    let Ok(listing) = JsFuture::from(js_sys::Promise::from(listing)).await else { return };
    let names: Vec<String> = js_sys::Array::from(&listing)
        .iter()
        .filter_map(|info| js_sys::Reflect::get(&info, &"name".into()).ok().and_then(|n| n.as_string()))
        .filter(|n| n.starts_with(prefix) && n != keep)
        .collect();
    for name in names {
        let options = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&options, &"ifAvailable".into(), &JsValue::TRUE);
        let victim = name.clone();
        let on_lock = Closure::once_into_js(move |lock: JsValue| -> js_sys::Promise {
            if lock.is_null() || lock.is_undefined() {
                return js_sys::Promise::resolve(&JsValue::NULL);
            }
            wasm_bindgen_futures::future_to_promise(async move {
                if let Err(e) = IndexedDBStorageEngine::cleanup(&victim).await {
                    tracing::warn!("could not delete the unused page database {victim}: {e}");
                }
                Ok(JsValue::NULL)
            })
        });
        if let Some(request) = call(&locks, "request", &[&name.into(), &options.into(), &on_lock]) {
            let _ = JsFuture::from(js_sys::Promise::from(request)).await;
        }
    }
}

/// The project's socket, `/p/<project>/ws` (the connector appends the last segment): at the
/// daemon `?daemon=` named, else on the page's own origin, since the daemon serves both the page
/// and the sockets. A trunk dev server proxies `/p/` to the daemon, so the page's own origin
/// holds there too.
fn ws_url(daemon: Option<&str>, project: &str) -> String {
    if let Some(address) = daemon {
        // `daemon_address` admits `http://` addresses only.
        return format!("ws://{}/p/{project}", address.trim_start_matches("http://"));
    }
    let location = window().expect("no window").location();
    let host = location.host().unwrap_or_else(|_| "127.0.0.1".into());
    let protocol = location.protocol().unwrap_or_else(|_| "http:".into());
    let scheme = if protocol == "https:" { "wss" } else { "ws" };
    format!("{scheme}://{host}/p/{project}")
}

/// Waits, at most `JOIN_TIMEOUT_MS`, for the node to join the project's system over the socket
/// and then to finish its own initialization, and returns the context every read and write goes
/// through. A joined node is not initialized yet (its synchronous `context` refuses until it
/// is), and `wait_system_ready` has no timeout of its own and only resolves over the socket, so
/// both waits run as one task while this polls, since the wasm executor offers no select.
async fn join(node: &Node<IndexedDBStorageEngine, PermissiveAgent>) -> Result<Context, String> {
    // None while waiting, then the context or the reason there is none.
    let outcome: Rc<RefCell<Option<Result<Context, String>>>> = Rc::new(RefCell::new(None));
    spawn_local({
        let outcome = outcome.clone();
        let node = node.clone();
        async move {
            let result = match node.system.wait_system_ready().await {
                Ok(()) => node.context_async(DEFAULT_CONTEXT).await.map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            *outcome.borrow_mut() = Some(result);
        }
    });
    for _ in 0..(JOIN_TIMEOUT_MS / POLL_STEP_MS) {
        let done = outcome.borrow_mut().take();
        if let Some(result) = done {
            return result.map_err(|e| {
                format!("The page's node could not join the project on the daemon: {e}. `fwd daemon logs` shows why.")
            });
        }
        sleep_ms(POLL_STEP_MS).await;
    }
    Err(format!(
        "The daemon did not accept the connection within {} seconds; `fwd daemon logs` shows why.",
        JOIN_TIMEOUT_MS / 1000
    ))
}

pub async fn sleep_ms(ms: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        if let Some(w) = window() {
            let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
        }
    });
    let _ = JsFuture::from(promise).await;
}

/// Tells whoever embeds the page how it is doing, so a host can put things right: a `fwd:state`
/// event on the window whose `detail` is `failed` when the page could not boot or stopped on an
/// error, and otherwise its connection's state (`Connected`, `Connecting`, `Error`, `Closed`) each
/// time that changes. The VS Code webview's bridge listens: when the connection drops it has the
/// extension register the page's origin with the daemon again (a restarted daemon has forgotten
/// it), and a command that opens a page that failed, or lost the daemon, loads it afresh. In a
/// browser tab nothing listens.
pub fn announce(state: &str) {
    let Some(w) = window() else { return };
    let init = web_sys::CustomEventInit::new();
    init.set_detail(&JsValue::from_str(state));
    if let Ok(event) = web_sys::CustomEvent::new_with_event_init_dict("fwd:state", &init) {
        let _ = w.dispatch_event(&event);
    }
}

/// Asks whoever embeds the page to show the highlight `anchor` in an editor, and answers whether
/// it took the request. In the VS Code webview a click on a highlight reveals its lines in the
/// editor (docs/DECISIONS.md, "A highlight's page in the browser shows only the highlighted
/// text", 2026-09-28), but the page does not call the VS Code API itself: it sends a `fwd:reveal`
/// event on its window, whose `detail` is the anchor's id, and the webview's bridge takes it by
/// cancelling the event and handing the extension a `reveal` message. A file a message names at a
/// version (`fwd_core::file_mention`) is asked for at that `version`, written after the id and an
/// at-sign as the message's text stores it, so that the editor opens the file at that commit, the
/// index's copy, or the file on disk. In a browser tab nothing listens, and the page shows the
/// highlight's own page instead.
pub fn reveal(anchor: EntityId, version: Option<&fwd_core::file_mention::Version>) -> bool {
    let Some(w) = window() else { return false };
    let init = web_sys::CustomEventInit::new();
    let detail = match version {
        Some(version) => format!("{}@{version}", anchor.to_base64()),
        None => anchor.to_base64(),
    };
    init.set_detail(&JsValue::from_str(&detail));
    init.set_cancelable(true);
    let Ok(event) = web_sys::CustomEvent::new_with_event_init_dict("fwd:reveal", &init) else { return false };
    // `dispatch_event` answers false once a listener cancelled the event.
    w.dispatch_event(&event).is_ok_and(|went_on| !went_on)
}

/// How the bar names the build of a daemon that does not name its own.
const UNNAMED: &str = "an unnamed build";

/// The build the page booted against, as the daemon named it, for the bar that offers a newer
/// one (`app.rs`, `NewerBuild`).
pub fn booted_on() -> String {
    BOOTED_ON.get().cloned().flatten().unwrap_or_else(|| UNNAMED.to_string())
}

/// The build the daemon runs now, once the page has seen it come back on a build other than the
/// one the page booted against; none while it runs the page's own. The bar at the top of the page
/// shows while it holds one (`app.rs`, `NewerBuild`).
pub fn newer_build() -> ArcRwSignal<Option<String>> {
    NEWER_BUILD.get_or_init(|| ArcRwSignal::new(None)).clone()
}

/// So that a page left open while the daemon restarts on a new build, which keeps showing what
/// the old build drew, says so: each time the page's connection comes back, which it does by
/// itself when the daemon restarts on the same port (`DECISIONS.md`, D80, "A daemon restart is
/// silent and never ends a watch; only a long outage surfaces"), this asks `/health` which build
/// the daemon runs now (`check_build`). The shell calls it once.
pub fn follow_restarts() {
    Effect::new(move |was: Option<bool>| {
        let connected = client().connection_state().get().to_string() == "Connected";
        if came_back(was, connected) {
            spawn_local(check_build());
        }
        connected
    });
}

/// Whether the connection has come back: connected now, and not when last seen. The first sight
/// of it, connected since the page booted, is no return.
fn came_back(was: Option<bool>, connected: bool) -> bool {
    connected && was == Some(false)
}

/// Asks `/health` which build the daemon runs now, and offers it (`newer_build`) when it is not
/// the build the page booted against, or takes the offer back when the daemon runs the page's
/// build again. Nothing changes when the answer cannot be read, nor ever when the page could not
/// read the build it booted against.
async fn check_build() {
    let Some(booted) = BOOTED_ON.get() else { return };
    let daemon = DAEMON.get().cloned().flatten();
    let Some(health) = fetch(daemon.as_deref(), "/health").await.filter(web_sys::Response::ok) else { return };
    let Ok(now) = named_build(health).await else { return };
    let offered = offer(booted.as_deref(), now.as_deref());
    let newer = newer_build();
    if newer.get_untracked() != offered {
        newer.set(offered);
    }
}

/// What the bar offers once the daemon is back: the build it runs now, when that is not the build
/// the page booted against; nothing when it came back on the page's own.
fn offer(booted: Option<&str>, now: Option<&str>) -> Option<String> {
    (booted != now).then(|| now.unwrap_or(UNNAMED).to_string())
}

/// Loads the page again, for the bar that offers the daemon's newer build. Only the bar's button
/// calls it, since a reload loses what is typed and not sent yet. It first sends a cancellable
/// `fwd:reload` event on its window. In the VS Code webview the page's files are the extension's,
/// and a new build of the extension takes over only when the window reloads, so the webview's
/// bridge takes the event by cancelling it and has the extension ask VS Code to reload the
/// window. In a browser tab nothing takes the event, and the page reloads itself from the daemon,
/// which serves its new build's page.
pub fn reload() {
    let Some(w) = window() else { return };
    let init = web_sys::CustomEventInit::new();
    init.set_cancelable(true);
    let Ok(event) = web_sys::CustomEvent::new_with_event_init_dict("fwd:reload", &init) else { return };
    // `dispatch_event` answers false once a listener cancelled the event.
    if w.dispatch_event(&event).unwrap_or(true) {
        let _ = w.location().reload();
    }
}

/// True when the URL has `?debug=1`: the pages then show how many live queries their rows hold.
pub fn debug() -> bool {
    query_param("debug").is_some_and(|v| v == "1")
}

fn query_param(name: &str) -> Option<String> {
    let search = window()?.location().search().ok()?;
    let params = web_sys::UrlSearchParams::new_with_str(&search).ok()?;
    params.get(name).filter(|s| !s.is_empty())
}

/// `?as=` wins: a principal's id, as `fwd --as <name> ui` writes it so that it names one principal
/// however many share the name, or a name, read as every typed name is (`fwd_core::naming`), so a
/// name two or more principals of the project share is refused, listing them; otherwise the
/// project's person: the first by handle of the active people who belong to the project, else of
/// the active people.
async fn choose_principal() -> Result<PrincipalView, String> {
    let ctx = ctx();
    if let Some(named) = query_param("as") {
        let none = if named.parse::<EntityId>().is_ok() { "id" } else { "handle" };
        return naming::principal_named(&ctx, &named, None)
            .await
            .map_err(|e| format!("{e}."))?
            .ok_or_else(|| format!("No principal has the {none} `{named}`."));
    }
    // `active` is filtered here, not in the predicate: a boolean literal in a predicate misses
    // entities that arrived over the wire (Ankurah 0.9.3; docs/design/data-model.md).
    let mut humans: Vec<PrincipalView> = ctx
        .fetch::<PrincipalView>("kind = 'human'")
        .await
        .map_err(|e| format!("could not list the project's people: {e}"))?
        .into_iter()
        .filter(|p| p.active().unwrap_or(false))
        .collect();
    humans.sort_by_key(|p| (!ops::belongs(p), p.handle().unwrap_or_default()));
    humans
        .into_iter()
        .next()
        .ok_or_else(|| "This project has no person yet: run `fwd init` in the repository first.".to_string())
}

#[cfg(test)]
mod tests {
    use super::{came_back, daemon_address, daemon_param, offer, UNNAMED};

    #[test]
    fn the_page_asks_for_the_daemons_build_each_time_its_connection_comes_back() {
        // Connected since the boot, lost as the daemon stops, retrying, back, then lost and back
        // again: one question at each return and none at the first sight.
        let mut was = None;
        let asked: Vec<bool> = [true, false, false, true, true, false, true]
            .into_iter()
            .map(|connected| {
                let back = came_back(was, connected);
                was = Some(connected);
                back
            })
            .collect();
        assert_eq!(asked, [false, false, false, true, false, false, true]);
        // A page whose connection dropped before the shell first looked asks once it is back.
        assert!(came_back(Some(false), true));
        assert!(!came_back(None, false));
    }

    #[test]
    fn the_bar_offers_the_daemons_build_only_when_it_is_not_the_one_the_page_booted_against() {
        let (old, new) = ("0.0.1-10.g1111111", "0.0.1-12.g2222222");
        assert_eq!(offer(Some(old), Some(old)), None, "restarted on the page's own build");
        assert_eq!(offer(Some(old), Some(new)), Some(new.to_string()));
        assert_eq!(offer(None, Some(new)), Some(new.to_string()), "booted against a daemon that named no build");
        assert_eq!(offer(Some(old), None), Some(UNNAMED.to_string()), "back on a build that names none");
        assert_eq!(offer(None, None), None);
    }

    #[test]
    fn only_a_page_no_daemon_serves_takes_the_daemons_address_from_its_url() {
        let local = "http://127.0.0.1:4242";
        assert_eq!(daemon_param("vscode-webview:", local), Ok(Some(local.to_string())));
        for served in ["http:", "https:"] {
            assert_eq!(daemon_param(served, local), Ok(None), "a page served over {served} ignores it");
            assert_eq!(daemon_param(served, "http://evil.example:4242"), Ok(None));
        }
        assert!(daemon_param("vscode-webview:", "http://evil.example:4242").is_err());
    }

    #[test]
    fn the_daemon_parameter_names_a_daemon_on_this_machine_and_nothing_else() {
        for given in ["http://127.0.0.1:4242", "http://127.0.0.1:4242/", "http://localhost:4242", "http://[::1]:4242"] {
            assert_eq!(daemon_address(given).as_deref(), Ok(given.trim_end_matches('/')), "{given}");
        }
        for given in [
            "https://127.0.0.1:4242",
            "ws://127.0.0.1:4242",
            "127.0.0.1:4242",
            "http://127.0.0.1",
            "http://127.0.0.1:0",
            "http://127.0.0.1:4242/p",
            "http://127.0.0.1:4242?x=1",
            "http://evil.example:4242",
            "http://127.0.0.1.evil.example:4242",
            "http://me@127.0.0.1:4242",
        ] {
            assert!(daemon_address(given).is_err(), "{given}");
        }
    }
}
