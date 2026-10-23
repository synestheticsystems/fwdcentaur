//! fwd-core as an Ankurah node inside the VS Code extension host.
//!
//! The extension host needs live access to the project's graph (comment threads, gutter marks,
//! the inbox) and must write through the same pre-checks as the CLI. VS Code runs extensions on
//! Node.js 24, which has a global `WebSocket` and the built-in `node:sqlite` module but no
//! IndexedDB, so this crate builds fwd-core to WASM for Node and keeps the node's data in
//! `ankurah-storage-sqlite-node`. The package gives JavaScript five things: `Host`, which joins
//! a node to the project's daemon; the classes the Model derive generates for every model
//! (`Task.query(ctx, "status = 'open'")` and the rest), for live queries; fwd-core's operations on
//! messages, notifications, anchors and task status; the rule that picks a highlight's first
//! thread, so that the extension's labels follow the rule the operations follow; and the place a
//! highlight's name begins with (`anchor_place`) and the words for the version of a file a message
//! names (`file_version`), so that the extension names a highlight and a file as the page and the
//! CLI do. Operations that need git (registering a repository, creating and
//! resolving anchors) are not here: the extension runs the bundled `fwd` binary for those.
//!
//! `Host.connect` opens an in-memory SQLite database (the daemon is always local, so the node
//! keeps nothing between runs), creates an ephemeral node over it, connects the node to the
//! project's WebSocket on the daemon, `ws://127.0.0.1:<port>/p/<project-uuid>` with the port and
//! the project from `fwd daemon status` (the connector appends `/ws`), and waits until the node
//! has joined the project's system and finished its own initialization, or rejects once the
//! caller's deadline passes. `Host.connectionState` says whether the node's socket to the daemon
//! is connected, so the extension hears the daemon go away and can join a new node once it is
//! back. `Host.close` drops the node's context, its connection and the node itself; once the
//! caller has also freed the contexts and live queries it took, a Node.js process can exit.
//!
//! The operations return fwd-core's JSON projections (`json_of_notification` and the rest in
//! crates/fwd-core/src/ops.rs) as plain JavaScript objects, and everything here fails with a
//! string. Entity ids are the base64 strings the CLI prints.

use ankurah::{policy::DEFAULT_CONTEXT, Context, EntityId, Node, PermissiveAgent};
use ankurah_storage_sqlite_node::SqliteNodeStorageEngine;
use ankurah_websocket_client_wasm::WebsocketClient;
use fwd_core::dormancy;
use fwd_core::naming;
use fwd_core::ops::{self, MessageContext};
use fwd_core::rules::{self, Verb};
use serde_json::Value;
use std::cell::Cell;
use std::future::Future;
use std::rc::Rc;
use std::sync::{Arc, Once};
use std::time::Duration;
use wasm_bindgen::prelude::*;

/// The models, and with them the classes the Model derive generates for JavaScript: `Task`
/// (the statics `get`, `fetch`, `query`), `TaskView`, `TaskLiveQuery`, `TaskResultSet`,
/// `TaskChangeSet` and `TaskRef`, and the same for every other model.
pub use fwd_core::model::*;

type HostNode = Node<SqliteNodeStorageEngine, PermissiveAgent>;

/// How often `connect` looks whether the node has joined.
const POLL_STEP_MS: u32 = 50;

/// Runs when Node loads the module. wasm-bindgen also exports it to JavaScript as `init`, so a
/// second call does nothing: installing a second tracing subscriber would panic.
#[wasm_bindgen(start)]
fn init() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        console_error_panic_hook::set_once();
        // Ankurah's warnings and errors (a refused handshake, say) reach the extension host's
        // console. Performance marks stay off: Node keeps every mark until it is cleared.
        tracing_wasm::set_as_global_default_with_config(
            tracing_wasm::WASMLayerConfigBuilder::new()
                .set_max_level(tracing::Level::WARN)
                .set_report_logs_in_timings(false)
                .set_console_config(tracing_wasm::ConsoleConfig::ReportWithoutConsoleColor)
                .build(),
        );
    });
}

/// The extension host's node, joined to the project's daemon.
#[wasm_bindgen]
pub struct Host {
    // `close` empties all three; the context and the client each hold the node.
    ctx: Option<Context>,
    client: Option<WebsocketClient>,
    node: Option<HostNode>,
}

#[wasm_bindgen]
impl Host {
    /// Joins a new in-memory node to a project the daemon hosts, at `url`:
    /// `ws://127.0.0.1:<port>/p/<project-uuid>` with the port and the project from
    /// `fwd daemon status` (the connector appends `/ws`). Rejects when the node has not joined
    /// within `timeout_ms`.
    pub async fn connect(url: String, timeout_ms: u32) -> Result<Host, String> {
        let storage = SqliteNodeStorageEngine::open_in_memory()
            .await
            .map_err(|e| format!("Node's built-in SQLite could not open an in-memory database: {e:#}"))?;
        let node = Node::new(Arc::new(storage), PermissiveAgent::new());
        let client = WebsocketClient::new(node.clone(), &url)
            .map_err(|e| format!("Could not open a WebSocket to the daemon at {url}: {e:#}"))?;
        match within(timeout_ms, node.wait_ready()).await {
            Some(Ok(())) => {}
            Some(Err(e)) => return Err(format!("The node could not join the project's system at {url}: {e}")),
            None => {
                return Err(format!(
                    "The daemon at {url} did not let this node join within {timeout_ms} ms. Is the daemon running on that port, and does it host that project? `fwd daemon status` prints both."
                ))
            }
        }
        let ctx = node
            .context_async(DEFAULT_CONTEXT)
            .await
            .map_err(|e| format!("The node joined the daemon at {url} but has no context: {e}"))?;
        Ok(Host { ctx: Some(ctx), client: Some(client), node: Some(node) })
    }

    /// The context every read, live query and transaction goes through. Each call returns a new
    /// handle, which keeps the node in memory until it is freed.
    pub fn context(&self) -> Result<Context, String> {
        self.ctx.clone().ok_or_else(closed)
    }

    /// This node's id, for diagnostics.
    #[wasm_bindgen(js_name = nodeId)]
    pub fn node_id(&self) -> Result<String, String> {
        self.node.as_ref().map(|node| node.id.to_base64()).ok_or_else(closed)
    }

    /// Whether this node's socket to the daemon is connected, so the caller can tell when the
    /// daemon goes away: a signal whose `peek.value()` is `Connected`, `Connecting`, `Closed`,
    /// `Error` or `None`, and whose `subscribe(callback)` calls `callback` with each new state
    /// (a `ConnectionStateEnum` the callback frees) until the guard it returns is freed. The
    /// callback runs while Ankurah is changing the state, so it must not call back into the
    /// package. The signal and the guard hold neither the node nor its socket.
    #[wasm_bindgen(js_name = connectionState, unchecked_return_type = "ConnectionStateEnumSignal")]
    pub fn connection_state(&self) -> Result<JsValue, String> {
        Ok(self.client.as_ref().ok_or_else(closed)?.js_connection_state().into())
    }

    /// Closes the WebSocket and drops this host's context and node. Every context, live query and
    /// subscription guard the caller took holds the node too, and the node keeps a timer running
    /// while it lives, so the Node.js process exits only once those are freed (`free()`) as well.
    pub fn close(&mut self) {
        self.ctx = None;
        self.client = None;
        self.node = None;
    }
}

fn closed() -> String {
    "This host is closed.".to_string()
}

/// Waits at most `timeout_ms` for `work`, which runs as its own task while this polls for its
/// answer (as `wait_joined` in crates/fwd-web/src/session.rs does); `None` when the time runs out
/// first. The node's readiness wait has no deadline of its own and resolves only over the socket,
/// so without this a socket that never opens would park `connect` forever. After a timeout the
/// task stays parked on the state of a node nobody holds any more; it keeps no timer or socket
/// open, so the process can still exit.
async fn within<T: 'static>(timeout_ms: u32, work: impl Future<Output = T> + 'static) -> Option<T> {
    // Empty until `work` finishes.
    let answer: Rc<Cell<Option<T>>> = Rc::new(Cell::new(None));
    wasm_bindgen_futures::spawn_local({
        let answer = answer.clone();
        async move { answer.set(Some(work.await)) }
    });
    for _ in 0..timeout_ms.div_ceil(POLL_STEP_MS) {
        if let Some(value) = answer.take() {
            return Some(value);
        }
        gloo_timers::future::sleep(Duration::from_millis(POLL_STEP_MS.into())).await;
    }
    answer.take()
}

// ---------------------------------------------------------------------------------------------
// Operations (crates/fwd-core/src/ops.rs), each taking the context from `Host.context()` first

/// `recipient_id`'s notifications that are not dismissed (only the unread ones with
/// `unread_only`): unread first, then by priority, then oldest first.
#[wasm_bindgen]
pub async fn inbox(ctx: &Context, recipient_id: String, unread_only: bool) -> Result<JsValue, String> {
    let found = ops::inbox(ctx, entity_id(&recipient_id)?, unread_only).await.map_err(describe)?;
    json_list(&found, ops::json_of_notification)
}

/// Sets `status` (`unread`, `read` or `dismissed`) on every notification in `ids`, in one
/// transaction.
#[wasm_bindgen]
pub async fn set_notification_status(ctx: &Context, ids: Vec<String>, status: String) -> Result<(), String> {
    let status: NotificationStatus = status
        .parse()
        .map_err(|_| format!("`{status}` is not a notification status: use unread, read or dismissed"))?;
    let ids = ids.iter().map(|id| entity_id(id)).collect::<Result<Vec<_>, _>>()?;
    ops::set_notification_status(ctx, &ids, status).await.map_err(describe)
}

/// Posts a message by `author_id` with `priority` (`now`, `next` or `later`) and returns its id.
/// `context` says what the message is about: `{task_id, anchor_id, scope_id, to, reply_to}`,
/// each optional, where `to` is the id of the principal it asks. A reply inherits its thread and
/// its subjects from the message it answers.
#[wasm_bindgen]
pub async fn post_message(
    ctx: &Context,
    author_id: String,
    content: String,
    priority: String,
    context: JsValue,
) -> Result<String, String> {
    let priority: Priority =
        priority.parse().map_err(|_| format!("`{priority}` is not a priority: use now, next or later"))?;
    let about = message_context(&context)?;
    let author = entity_id(&author_id)?;
    let id = ops::post_message(ctx, author, &content, priority, about).await.map_err(describe)?;
    wrote(ctx, author).await;
    Ok(id.to_base64())
}

/// Marks a message resolved by `by_id`; the first message of a step's highlight's first thread
/// checks the step with it (`ops::resolve_message`).
#[wasm_bindgen]
pub async fn resolve_message(ctx: &Context, by_id: String, message_id: String) -> Result<(), String> {
    let by = entity_id(&by_id)?;
    ops::resolve_message(ctx, by, entity_id(&message_id)?).await.map_err(describe)?;
    wrote(ctx, by).await;
    Ok(())
}

/// Records that `by` wrote just now (`dormancy::wrote`), after each write here but
/// `set_notification_status`, which marks notifications, has succeeded, as the CLI and the page
/// record theirs, so that an agent the window acts as is never listed as dormant while it writes
/// from the editor (docs/DECISIONS.md, D123). The write was made whatever becomes of the stamp, so a
/// stamp that fails goes to the extension host's console rather than failing the write: it moves
/// the principal in no picker until its next write stamps it.
async fn wrote(ctx: &Context, by: EntityId) {
    if let Err(e) = dormancy::wrote(ctx, by).await {
        tracing::warn!("could not record that {} wrote from the editor: {e:#}", by.to_base64());
    }
}

/// Of the messages on one highlight, the first message of its first thread (`rules::first_thread`),
/// or `undefined` while no one has started a thread on it. Resolving that message checks the steps
/// the highlight is, and checking one of them resolves it (docs/DECISIONS.md, D99, "Checking a step
/// resolves its highlight's first thread, and resolving that thread checks the step; each control
/// says it does both"), so the extension labels that thread's actions by this rule, the one the
/// operations follow. Each message is `{id, status, created, thread, reply_to}` as fwd-core's JSON
/// of a message spells them, `thread` and `reply_to` absent or null on a message that answers none.
#[wasm_bindgen]
pub fn first_thread(messages: JsValue) -> Result<Option<String>, String> {
    let json = js_sys::JSON::stringify(&messages).ok().and_then(|s| s.as_string());
    let Some(Value::Array(list)) = json.and_then(|j| serde_json::from_str(&j).ok()) else {
        return Err("`messages` must be an array of {id, status, created, thread, reply_to}".into());
    };
    let mut read = Vec::with_capacity(list.len());
    for m in &list {
        let id = m["id"].as_str().ok_or("each message needs its `id`")?;
        let status: MessageStatus = m["status"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| format!("message {id}: `status` must be active, resolved or deleted"))?;
        let created = m["created"].as_i64().ok_or_else(|| format!("message {id}: `created` must be a number"))?;
        let answers = !m["thread"].is_null() || !m["reply_to"].is_null();
        read.push(rules::ThreadMessage { id: entity_id(id)?, answers, status, created });
    }
    Ok(rules::first_thread(&read).map(|id| id.to_base64()))
}

/// Messages whose `field` (`task`, `anchor`, `scope`, `thread` or `reply_to`) points at `id`,
/// oldest first.
#[wasm_bindgen]
pub async fn messages_about(ctx: &Context, field: String, id: String) -> Result<JsValue, String> {
    let found = ops::messages_about(ctx, &field, entity_id(&id)?).await.map_err(describe)?;
    json_list(&found, ops::json_of_message)
}

/// The anchors in the repository-relative `path`, by start line.
#[wasm_bindgen]
pub async fn anchors_in_path(ctx: &Context, path: String) -> Result<JsValue, String> {
    let found = ops::anchors_in_path(ctx, &path).await.map_err(describe)?;
    json_list(&found, ops::json_of_anchor)
}

/// Where the highlight `anchor` is, as a reference to it in a message's text names it:
/// `path:lines`, or the path alone for a whole file (fwd-core's `anchor_name`), so that the
/// extension names a highlight as the page and the CLI do.
#[wasm_bindgen]
pub fn anchor_place(anchor: &CodeAnchorView) -> String {
    fwd_core::anchor_name::AnchorName::of(anchor).place
}

/// The version of a file a message names (fwd-core's `file_mention`), in the words a file's name
/// shows it with after its path: the commit's first seven characters, `index`, or `on disk`
/// (fwd-core's `anchor_name::file_version`), from `version` as a stored file mention writes it
/// after its highlight's id (`worktree`, `index` or a commit's id); `undefined` for anything
/// else. So that the extension names a file a message names as the page and the CLI do.
#[wasm_bindgen]
pub fn file_version(version: String) -> Option<String> {
    fwd_core::file_mention::Version::parse(&version).map(|v| fwd_core::anchor_name::file_version(&v))
}

/// Applies `verb`, spelled as the CLI's `fwd task <verb>`, to a task and returns the task's new
/// status. `note` is the evidence for `submit`, the reason for `reject`, `return`, `cancel`,
/// `reopen` and `rejustify`, and the acceptance note for `accept`. Starting a task is `claim`.
#[wasm_bindgen]
pub async fn transition(
    ctx: &Context,
    by_id: String,
    task_id: String,
    verb: String,
    note: Option<String>,
) -> Result<String, String> {
    let verb = task_verb(&verb)?;
    let by = entity_id(&by_id)?;
    let status = ops::transition(ctx, by, entity_id(&task_id)?, verb, note.as_deref()).await.map_err(describe)?;
    wrote(ctx, by).await;
    Ok(status.to_string())
}

/// Starts the task `task_id` for `by_id` in the scope `scope_id`, in one transaction, as `fwd task
/// start` does in the caller's scope (`ops::claim`): the task goes in progress with `by_id` as its
/// assignee, and a task worked in no scope comes to be worked in `scope_id`. The scope is where the
/// task is worked, not an assignment (docs/DECISIONS.md, D45, "Assignment is an optional field on
/// the task, never an edge; the scope edge means where the work happens"), so a task worked only in
/// other scopes, any number since D136 ("A task may be worked in several scopes"), is refused rather
/// than moved into this one (D87, "Starting a task worked in another scope is refused, with a prompt
/// to switch to that scope first"), and one worked in it among others starts as it is.
#[wasm_bindgen]
pub async fn claim(ctx: &Context, by_id: String, task_id: String, scope_id: String) -> Result<(), String> {
    let by = entity_id(&by_id)?;
    ops::claim(ctx, by, entity_id(&task_id)?, entity_id(&scope_id)?).await.map_err(describe)?;
    wrote(ctx, by).await;
    Ok(())
}

/// The principal `name` names, read as the CLI's `--as` reads it (`fwd_core::naming`: its name, or
/// its id), or `null` when none has it; a name two or more principals of the project share is
/// refused with a sentence listing each by its id, which the extension's `fwd.as` then takes.
#[wasm_bindgen]
pub async fn principal_named(ctx: &Context, name: String) -> Result<JsValue, String> {
    match naming::principal_named(ctx, &name, None).await.map_err(describe)? {
        Some(principal) => js(&ops::json_of_principal(&principal).map_err(describe)?),
        None => Ok(JsValue::NULL),
    }
}

/// What the mention search in a comment box offers for `typed`, what follows the `@`: the
/// principals of the project that belong to it whose name starts with it (`naming::mentionable`),
/// the agents that have written nothing for twelve hours after all the others (D123), each as
/// `{id, handle, what, scopes, written, dormant}`: what it is (person, its role, or agent), the
/// titles of the scopes its work is in beside a name two or more principals share (else null), how
/// the text writes its mention (`@name`, or `@` and the start of the id for a shared name), and
/// whether it is dormant (`dormancy::is_dormant`, by this machine's clock).
#[wasm_bindgen]
pub async fn mention_offers(ctx: &Context, typed: String, most: u32) -> Result<JsValue, String> {
    let principals = ops::principals(ctx).await.map_err(describe)?;
    let now = fwd_core::now_ms();
    let offered = naming::mentionable(&principals, &typed, most as usize, now);
    let shared: Vec<EntityId> = offered
        .iter()
        .filter(|p| naming::shows_scopes(&principals, &p.handle().unwrap_or_default()))
        .map(|p| p.id())
        .collect();
    // The work behind the scopes, read only when some name is shared.
    let loaded =
        if shared.is_empty() { None } else { Some(naming::Loaded::of(ctx, &shared).await.map_err(describe)?) };
    let offers: Vec<Value> = offered
        .iter()
        .map(|p| {
            let scopes = loaded.as_ref().filter(|_| shared.contains(&p.id())).map(|loaded| {
                let work = loaded.work();
                naming::scope_titles(&naming::scopes_of(p.id(), &work), work.scopes)
            });
            serde_json::json!({
                "id": p.id().to_base64(),
                "handle": p.handle().unwrap_or_default(),
                "what": naming::what(p),
                "scopes": scopes,
                "written": naming::mention_of(&principals, p),
                "dormant": dormancy::is_dormant(p, now),
            })
        })
        .collect();
    js(&Value::Array(offers))
}

/// The person the extension acts as unless told otherwise: the project's first active human by
/// handle, as the web UI chooses.
#[wasm_bindgen]
pub async fn default_principal(ctx: &Context) -> Result<JsValue, String> {
    // `active` is filtered here, not in the predicate: a boolean literal in a predicate missed
    // entities that arrived over the wire on Ankurah 0.9.3 (docs/design/data-model.md).
    let mut people: Vec<PrincipalView> = ctx
        .fetch::<PrincipalView>("kind = 'human'")
        .await
        .map_err(|e| format!("Could not list the project's people: {e}"))?
        .into_iter()
        .filter(|p| p.active().unwrap_or(false))
        .collect();
    people.sort_by_key(|p| p.handle().unwrap_or_default());
    let person =
        people.into_iter().next().ok_or("This project has no person yet: run `fwd init` in the repository first.")?;
    js(&ops::json_of_principal(&person).map_err(describe)?)
}

// ---------------------------------------------------------------------------------------------
// Crossing the boundary

fn entity_id(text: &str) -> Result<EntityId, String> {
    ops::parse_id(text).map_err(describe)
}

/// An error with its causes, on one line.
fn describe(error: anyhow::Error) -> String {
    format!("{error:#}")
}

fn js(value: &Value) -> Result<JsValue, String> {
    js_sys::JSON::parse(&value.to_string()).map_err(|_| "JSON.parse refused the JSON of a result".to_string())
}

fn json_list<T>(items: &[T], json_of: fn(&T) -> anyhow::Result<Value>) -> Result<JsValue, String> {
    let list = items.iter().map(json_of).collect::<anyhow::Result<Vec<Value>>>().map_err(describe)?;
    js(&Value::Array(list))
}

/// The CLI's `fwd task` subcommands that run a status verb. `start` is not one: a task starts with
/// `claim`, which also makes whoever starts it its assignee and gives a task worked in no scope the
/// scope where it is worked (docs/DECISIONS.md, D45, "Assignment is an optional field on the task,
/// never an edge; the scope edge means where the work happens").
fn task_verb(verb: &str) -> Result<Verb, String> {
    Ok(match verb {
        "approve" => Verb::Approve,
        "reject" => Verb::Reject,
        "submit" => Verb::Submit,
        "accept" => Verb::Accept,
        "return" => Verb::Return,
        "done" => Verb::Done,
        "cancel" => Verb::Cancel,
        "reopen" => Verb::Reopen,
        "rejustify" => Verb::Rejustify,
        "start" => return Err("A task starts with `claim`, which also assigns it to a scope.".to_string()),
        other => {
            return Err(format!(
                "`{other}` is not a task verb: use approve, reject, submit, accept, return, done, cancel, reopen or rejustify"
            ))
        }
    })
}

/// What a message is about, from the object `post_message` takes. A missing, `undefined` or
/// `null` field is unset; a field `MessageContext` does not have is refused rather than dropped,
/// so a misspelled one fails loudly.
fn message_context(context: &JsValue) -> Result<MessageContext, String> {
    let mut about = MessageContext::default();
    if context.is_undefined() || context.is_null() {
        return Ok(about);
    }
    let json = js_sys::JSON::stringify(context).ok().and_then(|s| s.as_string());
    let Some(Value::Object(fields)) = json.and_then(|j| serde_json::from_str(&j).ok()) else {
        return Err("`context` must be an object: {task_id, anchor_id, scope_id, to, reply_to}, each optional".into());
    };
    for (key, value) in fields {
        let text = |value: Value| match value {
            Value::Null => Ok(None),
            Value::String(s) => Ok(Some(s)),
            _ => Err(format!("`context.{key}` must be a string")),
        };
        let id = |value: Value| text(value)?.map(|t| entity_id(&t)).transpose();
        match key.as_str() {
            "task_id" => about.task_id = id(value)?,
            "anchor_id" => about.anchor_id = id(value)?,
            "scope_id" => about.scope_id = id(value)?,
            "reply_to" => about.reply_to = id(value)?,
            "to" => about.to = text(value)?,
            _ => {
                return Err(format!(
                    "`context` has no field `{key}`: it takes task_id, anchor_id, scope_id, to and reply_to"
                ))
            }
        }
    }
    Ok(about)
}
