//! The daemon's MCP endpoint (crates/fwd/src/mcp.rs; task jDv1EY7K), called by a real MCP client,
//! rmcp's Streamable HTTP client, against a scratch daemon of each test's own: the lead's and an
//! implementer's work through it, each call attributed to the thread whose metadata it carries, and
//! the same refusals the CLI gives, which the tests compare with the CLI's own where they can.

use anyhow::{anyhow, Result};
use fwd_core::anchoring::git;
use rmcp::model::{CallToolRequestParams, CallToolResult, RequestMetaObject};
use rmcp::service::RunningService;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{json, Map, Value};
use std::path::PathBuf;
use std::process::Command;

/// A scratch directory holding the daemon's runtime and data directories and a checkout of a new
/// project, `mcp`, which the person `tester` joined. The daemon is stopped when the test ends.
struct Scratch {
    dir: tempfile::TempDir,
    root: PathBuf,
}

impl Scratch {
    fn new() -> Result<Scratch> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().join("app");
        std::fs::create_dir_all(&root)?;
        git(&root, &["init", "-q", "-b", "main"])?;
        git(&root, &["remote", "add", "origin", "git@example.com:t/mcp.git"])?;
        let scratch = Scratch { dir, root };
        scratch.run(&["init", "--name", "mcp", "--handle", "tester"])?;
        Ok(scratch)
    }

    /// `fwd <args>` in the checkout against this scratch daemon only, with none of the caller's
    /// settings that would choose another project, principal or scope: its output, or its error.
    fn output(&self, args: &[&str]) -> Result<std::result::Result<String, String>> {
        self.output_with(args, &[])
    }

    /// `fwd <args>` as [`Scratch::output`] runs it, with `env` set too.
    fn output_with(&self, args: &[&str], env: &[(&str, &str)]) -> Result<std::result::Result<String, String>> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fwd"));
        command
            .current_dir(&self.root)
            .env("FWD_RUNTIME_DIR", self.dir.path().join("run"))
            .env("FWD_DATA_DIR", self.dir.path().join("data"))
            .env("FWD_DEV_RUNNER_DIR", self.dir.path().join("dev-runner"));
        for name in [
            "FWD_PROJECT",
            "FWD_AS",
            "FWD_SCOPE",
            "FWD_DAEMON_BIN",
            "FWD_HANDLE",
            "FWD_CLAUDE_SESSION",
            "FWD_CLAUDE_AGENT",
            "CLAUDE_CODE_SESSION_ID",
        ] {
            command.env_remove(name);
        }
        command.envs(env.iter().copied());
        let out = command.args(args).output()?;
        Ok(if out.status.success() {
            Ok(String::from_utf8(out.stdout)?)
        } else {
            Err(String::from_utf8_lossy(&out.stderr).into_owned())
        })
    }

    fn run(&self, args: &[&str]) -> Result<String> {
        self.output(args)?.map_err(|e| anyhow!("fwd {} failed: {e}", args.join(" ")))
    }

    fn json(&self, args: &[&str]) -> Result<Value> {
        Ok(serde_json::from_str(&self.run(&[&["--format", "json"][..], args].concat())?)?)
    }

    /// The URL of this daemon's MCP endpoint.
    fn url(&self) -> Result<String> {
        let status = self.json(&["daemon", "status"])?;
        let port = status["port"].as_u64().ok_or_else(|| anyhow!("no port in {status}"))?;
        Ok(format!("http://127.0.0.1:{port}/mcp"))
    }

    /// A client connected to the endpoint, through `initialize`.
    async fn client(&self) -> Result<RunningService<RoleClient, ()>> {
        Ok(().serve(StreamableHttpClientTransport::from_uri(self.url()?)).await?)
    }

    /// The principals bound to `subject`, as `fwd principal list --all` lists them.
    fn bound_to(&self, subject: &str) -> Result<Vec<Value>> {
        let all = self.json(&["principal", "list", "--all"])?;
        Ok(all.as_array().into_iter().flatten().filter(|p| p["subject"] == json!(subject)).cloned().collect())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = self.run(&["daemon", "stop"]);
    }
}

/// A Codex thread as the endpoint sees it: the metadata Codex attaches to each of its calls.
#[derive(Clone)]
struct Thread {
    thread: String,
    session: String,
    parent: Option<String>,
}

impl Thread {
    /// A thread no other spawned: its session is its own.
    fn top(id: &str) -> Thread {
        Thread { thread: id.into(), session: id.into(), parent: None }
    }

    /// A subagent `id` that `parent` spawned, in `parent`'s session.
    fn spawned(id: &str, parent: &Thread) -> Thread {
        Thread { thread: id.into(), session: parent.session.clone(), parent: Some(parent.thread.clone()) }
    }

    fn subject(&self) -> String {
        format!("codex:{}/{}", self.session, self.thread)
    }

    /// The `_meta` of a call this thread's model makes, as Codex 0.159.2 sends it.
    fn meta(&self) -> RequestMetaObject {
        let mut turn =
            json!({"thread_id": self.thread, "session_id": self.session, "turn_id": "turn-1", "model": "fake"});
        if let Some(parent) = &self.parent {
            turn["parent_thread_id"] = json!(parent);
            turn["subagent_kind"] = json!("thread_spawn");
        }
        meta(json!({"threadId": self.thread, "sessionId": self.session, "x-codex-turn-metadata": turn}))
    }
}

fn meta(v: Value) -> RequestMetaObject {
    let mut meta = RequestMetaObject::new();
    for (k, v) in v.as_object().cloned().unwrap_or_default() {
        meta.insert(k, v);
    }
    meta
}

fn with_meta(mut params: CallToolRequestParams, meta: RequestMetaObject) -> CallToolRequestParams {
    params.meta = Some(meta);
    params
}

fn object(v: Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

/// Calls `tool` with `args` and the project `mcp`, carrying `meta`.
async fn call(
    client: &RunningService<RoleClient, ()>,
    meta: Option<RequestMetaObject>,
    tool: &str,
    args: Value,
) -> Result<CallToolResult> {
    let mut args = object(args);
    args.entry("project").or_insert(json!("mcp"));
    let mut params = CallToolRequestParams::new(tool.to_string()).with_arguments(args);
    if let Some(meta) = meta {
        params.meta = Some(meta);
    }
    Ok(client.call_tool(params).await?)
}

/// What a call answered, failing the test with its refusal.
async fn ok(client: &RunningService<RoleClient, ()>, t: &Thread, tool: &str, args: Value) -> Result<Value> {
    let r = call(client, Some(t.meta()), tool, args.clone()).await?;
    let s = r.structured_content.clone().unwrap_or_default();
    if r.is_error == Some(true) {
        return Err(anyhow!("{tool} {args} was refused: {}", s["error"]));
    }
    Ok(s["result"].clone())
}

/// Why a call was refused, failing the test when it was not.
async fn refused(
    client: &RunningService<RoleClient, ()>,
    meta: Option<RequestMetaObject>,
    tool: &str,
    args: Value,
) -> Result<String> {
    let r = call(client, meta, tool, args.clone()).await?;
    let s = r.structured_content.clone().unwrap_or_default();
    if r.is_error != Some(true) {
        return Err(anyhow!("{tool} {args} was not refused: {s}"));
    }
    Ok(s["error"].as_str().unwrap_or_default().to_string())
}

/// What `read` gives once `done` holds of it, read again every 100 ms for up to ten seconds.
async fn until<F, Fut>(mut read: F, done: impl Fn(&Value) -> bool) -> Result<Value>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<Value>>,
{
    let mut last = Value::Null;
    for _ in 0..100 {
        last = read().await?;
        if done(&last) {
            return Ok(last);
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    Err(anyhow!("never: {last}"))
}

/// The id of what a call that makes something made.
fn made(v: &Value) -> String {
    v["record"]["id"].as_str().unwrap_or_default().to_string()
}

/// A client connects with `initialize`, lists the tools, which are the CLI's commands with none
/// taking an actor, and discovers the project before anything is made.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_initializes_lists_the_tools_and_finds_the_project_without_being_made_anyone() -> Result<()> {
    let s = Scratch::new()?;
    let client = s.client().await?;
    let info = client.peer_info().ok_or_else(|| anyhow!("no server info"))?;
    assert_eq!(info.server_info.as_ref().map(|i| i.name.as_str()), Some("fwd"));
    // No instructions: Codex charges a plugin server's instructions to each of its tools against
    // the budget every installed plugin shares, which hid most of fwd's tools (crates/fwd/src/mcp.rs).
    assert_eq!(info.instructions, None);
    let tools = client.list_all_tools().await?;
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    for name in [
        "project_list",
        "whoami",
        "task_create",
        "task_submit",
        "task_accept",
        "inbox",
        "inbox_pop",
        "reply",
        "principal_add",
    ] {
        assert!(names.contains(&name), "{name} in {names:?}");
    }
    // The catalog as served stays compact: each tool is its command's short help, abridged, with the
    // full help one `help` call away (crates/fwd/src/mcp.rs, the size guard in its tests).
    let served: usize = tools.iter().map(|t| serde_json::to_string(t).map(|j| j.len()).unwrap_or(0)).sum();
    assert!(served <= 36_000, "the served catalog is {served} bytes");
    for absent in ["watch", "daemon_stop", "init", "principal_bind", "admin_bind", "anchor_create", "scope_attach_dir"]
    {
        assert!(!names.contains(&absent), "{absent} is not offered");
    }
    for tool in &tools {
        let properties = tool.input_schema["properties"].as_object().cloned().unwrap_or_default();
        assert!(!properties.contains_key("as"), "{} takes no actor", tool.name);
    }
    let t = Thread::top("019a0000-0000-7000-8000-00000000a001");
    let projects = ok(&client, &t, "project_list", json!({})).await;
    // `project_list` takes no `project`; the helper adds one, which it refuses.
    assert!(projects.is_err());
    let r = client
        .call_tool(with_meta(CallToolRequestParams::new("project_list").with_arguments(Map::new()), t.meta()))
        .await?;
    let listed = &r.structured_content.unwrap_or_default()["result"];
    assert_eq!(listed.as_array().map(|l| l.len()), Some(1), "{listed}");
    assert_eq!(listed[0]["name"], "mcp");
    // The CLI's full help for a tool's command, which the catalog abridges.
    let r = client
        .call_tool(with_meta(
            CallToolRequestParams::new("help").with_arguments(object(json!({"command": "task_start"}))),
            t.meta(),
        ))
        .await?;
    let help = r.structured_content.unwrap_or_default()["result"]["help"].as_str().unwrap_or_default().to_string();
    assert!(help.contains("Start the task") && help.contains("--scope"), "{help}");
    let whoami = ok(&client, &t, "whoami", json!({})).await?;
    assert_eq!(whoami["subject"], json!(t.subject()));
    assert_eq!(whoami["principal"], Value::Null);
    assert_eq!(whoami["first_write_makes"]["role"], "implementer", "a top-level thread is not taken for a lead");
    assert!(s.bound_to(&t.subject())?.is_empty(), "discovery made no one");
    Ok(())
}

/// A call without the thread's metadata, or with metadata that disagrees with itself, is refused
/// and runs nothing: no principal is made, nothing is written. An unknown tool is a protocol error.
#[tokio::test(flavor = "multi_thread")]
async fn a_call_fwd_cannot_attribute_is_refused_and_runs_nothing() -> Result<()> {
    let s = Scratch::new()?;
    let client = s.client().await?;
    let before = s.json(&["task", "list"])?;
    let why = refused(&client, None, "task_create", json!({"title": "anonymous"})).await?;
    assert!(why.contains("cannot tell which thread makes this call"), "{why}");
    let forged = meta(json!({
        "threadId": "t-a", "sessionId": "t-a",
        "x-codex-turn-metadata": {"thread_id": "t-b", "session_id": "t-a"},
    }));
    let why = refused(&client, Some(forged), "task_create", json!({"title": "forged"})).await?;
    assert!(why.contains("are not the thread_id and session_id"), "{why}");
    let half = meta(json!({"threadId": "t-a", "sessionId": "t-a"}));
    let why = refused(&client, Some(half), "task_create", json!({"title": "half"})).await?;
    assert!(why.contains("x-codex-turn-metadata"), "{why}");
    assert_eq!(s.json(&["task", "list"])?, before, "nothing was written");
    let all = s.json(&["principal", "list", "--all"])?;
    assert!(
        all.as_array().unwrap().iter().all(|p| p["handle"].as_str().is_some_and(|h| !h.starts_with("codex-"))),
        "{all}"
    );
    // An actor typed as an argument is refused as an argument the tool does not take.
    let t = Thread::top("t-arg");
    let why = refused(&client, Some(t.meta()), "task_create", json!({"title": "x", "as": "tester"})).await?;
    assert!(why.contains("takes no argument `as`"), "{why}");
    let unknown = client
        .call_tool(with_meta(CallToolRequestParams::new("no_such_tool").with_arguments(Map::new()), t.meta()))
        .await;
    assert!(unknown.is_err(), "an unknown tool is a protocol error");
    Ok(())
}

/// A lead thread files work and reviews it; a subagent it spawned takes the work, asks, and
/// submits. Every call is attributed to the thread that made it, each thread made one principal
/// on its first write, and the CLI's transitions, dependencies, review rule and inbox semantics
/// hold through the endpoint.
#[tokio::test(flavor = "multi_thread")]
async fn a_lead_and_its_implementer_work_through_the_endpoint_as_themselves() -> Result<()> {
    let s = Scratch::new()?;
    let client = s.client().await?;
    let lead = Thread::top("019a1111-0000-7000-8000-0000000000aa");
    let worker = Thread::spawned("019a1111-0000-7000-8000-0000000000bb", &lead);

    // The lead names itself with its role before it writes, as an agent of the CLI does; no
    // principal is made for it first.
    let named = ok(&client, &lead, "principal_add", json!({"handle": "lead-a", "role": "lead"})).await?;
    assert_eq!(named["record"]["subject"], json!(lead.subject()));
    // The lead files two tasks, the second after the first.
    let first =
        ok(&client, &lead, "task_create", json!({"title": "Connector", "needs_review": true, "label": ["feature"]}))
            .await?;
    let first_id = made(&first);
    let second = ok(&client, &lead, "task_create", json!({"title": "Use the connector", "after": [first_id]})).await?;
    let second_id = made(&second);
    let leads = s.bound_to(&lead.subject())?;
    assert_eq!(leads.len(), 1, "{leads:?}");
    let lead_principal = leads[0].clone();
    assert_eq!(lead_principal["agent_role"], "lead");
    assert_eq!(lead_principal["handle"], "lead-a");
    assert_eq!(first["record"]["created_by"], lead_principal["id"], "attributed to the lead's thread");
    assert_eq!(second["record"]["readiness"]["state"], "not_ready");

    // The implementer's next task is the first; its first read of its own tasks made it, as an
    // implementer, named after the end of its thread id.
    let next = ok(&client, &worker, "task_next", json!({"claim": true})).await?;
    assert_eq!(next["id"], json!(first_id));
    assert_eq!(next["claimed"], json!(true));
    let workers = s.bound_to(&worker.subject())?;
    assert_eq!((workers.len(), workers[0]["agent_role"].as_str()), (1, Some("implementer")));
    assert_eq!(workers[0]["handle"], "codex-000000bb");
    let worker_principal = workers[0].clone();
    assert_eq!(next["assignee"], worker_principal["id"]);

    // A transition the task's status does not allow is refused in the CLI's words.
    let why = refused(&client, Some(worker.meta()), "task_accept", json!({"id": second_id})).await?;
    let cli = s.output(&["--as", "tester", "task", "accept", &second_id])?.expect_err("the CLI refuses it too");
    assert!(why.contains("is not allowed from open") && cli.contains(&why), "MCP: {why}\nCLI: {cli}");

    // The implementer asks the lead, by name, about the task; the lead sees it in its inbox, which
    // a list leaves unread and a pop marks read, and replies through the notification, settling it.
    let lead_handle = lead_principal["handle"].as_str().unwrap();
    let question =
        ok(&client, &worker, "ask", json!({"to": lead_handle, "about": first_id, "text": "Retry on 503?"})).await?;
    assert_eq!(question["record"]["author"], worker_principal["id"]);
    // The daemon's routing service tells the lead a moment after the question is written.
    let listed = until(
        || ok(&client, &lead, "inbox", json!({"unread": true})),
        |l| l.as_array().is_some_and(|l| l.iter().any(|n| n["type"] == "question_asked")),
    )
    .await?;
    let asked: Vec<&Value> = listed.as_array().unwrap().iter().filter(|n| n["type"] == "question_asked").collect();
    assert_eq!(asked.len(), 1, "{listed}");
    let notification = asked[0]["id"].as_str().unwrap().to_string();
    assert_eq!(ok(&client, &lead, "inbox", json!({"unread": true})).await?, listed, "a list marks nothing read");
    let replied = ok(&client, &lead, "reply", json!({"id": notification, "text": "Yes, three times."})).await?;
    assert_eq!(replied["settled"], json!(notification));
    assert_eq!(replied["record"]["author"], lead_principal["id"]);
    // The reply reaches the implementer a moment later; each pop marks what it printed read.
    let mut popped: Vec<Value> = Vec::new();
    for _ in 0..100 {
        popped.extend(ok(&client, &worker, "inbox_pop", json!({})).await?.as_array().cloned().unwrap_or_default());
        if popped.iter().any(|n| n["type"] == "replied") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(popped.iter().any(|n| n["type"] == "replied"), "{popped:?}");
    assert_eq!(ok(&client, &worker, "inbox_pop", json!({})).await?, json!([]), "a pop marks what it printed read");

    // The implementer submits; it cannot accept its own submission; the lead accepts.
    let submitted = ok(&client, &worker, "task_submit", json!({"id": first_id, "evidence": "tests pass"})).await?;
    assert_eq!(submitted["status"], "in_review");
    let why = refused(&client, Some(worker.meta()), "task_accept", json!({"id": first_id})).await?;
    assert!(why.contains("the submitter cannot accept their own submission"), "{why}");
    let accepted = ok(&client, &lead, "task_accept", json!({"id": first_id, "note": "good"})).await?;
    assert_eq!(accepted["status"], "done");
    let shown = ok(&client, &lead, "task_show", json!({"id": second_id})).await?;
    assert_eq!(shown["readiness"]["state"], "ready", "the dependency is done: {shown}");

    // The CLI reads what the endpoint wrote, attributed as it was.
    let cli = s.json(&["task", "show", &first_id, "--include", "messages"])?;
    assert_eq!(cli["status"], "done");
    let authors: Vec<&str> =
        cli["messages"].as_array().unwrap().iter().filter_map(|m| m["author_handle"].as_str()).collect();
    assert!(
        authors.contains(&lead_handle) && authors.contains(&worker_principal["handle"].as_str().unwrap()),
        "{authors:?}"
    );
    Ok(())
}

/// Two threads are two principals; one thread's first calls arriving together make one; a thread
/// resumed with a new connection is the same principal; and a thread that names itself first is
/// made as it says.
#[tokio::test(flavor = "multi_thread")]
async fn each_thread_is_one_principal_however_its_calls_arrive() -> Result<()> {
    let s = Scratch::new()?;
    let a = Thread::top("019a2222-0000-7000-8000-00000000000a");
    let b = Thread::top("019a2222-0000-7000-8000-00000000000b");
    let client = std::sync::Arc::new(s.client().await?);
    let mut calls = Vec::new();
    for n in 0..8 {
        let (client, a) = (client.clone(), a.clone());
        calls.push(tokio::spawn(async move {
            ok(&client, &a, "task_create", json!({"title": format!("together {n}")})).await
        }));
    }
    let mut creators = std::collections::HashSet::new();
    for c in calls {
        creators.insert(c.await??["record"]["created_by"].as_str().unwrap().to_string());
    }
    assert_eq!(creators.len(), 1, "one principal for one thread: {creators:?}");
    assert_eq!(s.bound_to(&a.subject())?.len(), 1);

    let by_b = ok(&client, &b, "task_create", json!({"title": "by b"})).await?;
    assert!(!creators.contains(by_b["record"]["created_by"].as_str().unwrap()), "another thread is another principal");

    // Resumed: a new connection, the same thread.
    let again = s.client().await?;
    let resumed = ok(&again, &a, "task_create", json!({"title": "after resume"})).await?;
    assert!(creators.contains(resumed["record"]["created_by"].as_str().unwrap()));

    // A thread that names itself before it writes is made with its name and role, bound to it.
    let c = Thread::top("019a2222-0000-7000-8000-00000000000c");
    let named = ok(&again, &c, "principal_add", json!({"handle": "reviewer-c", "role": "reviewer"})).await?;
    assert_eq!((named["handle"].as_str(), named["agent_role"].as_str()), (Some("reviewer-c"), Some("reviewer")));
    assert_eq!(named["subject"], json!(c.subject()));
    let wrote = ok(&again, &c, "task_create", json!({"title": "by c"})).await?;
    assert_eq!(wrote["record"]["created_by"], named["id"]);
    // A thread bound already is refused a second name, as an agent of the CLI is.
    let why =
        refused(&again, Some(c.meta()), "principal_add", json!({"handle": "other-c", "role": "implementer"})).await?;
    assert!(why.contains("a subject binds one principal"), "{why}");
    // A thread's principal retired writes nothing until it comes back, as the CLI's does.
    ok(&again, &c, "principal_retire", json!({"handle": "reviewer-c"})).await?;
    let why = refused(&again, Some(c.meta()), "task_create", json!({"title": "retired"})).await?;
    assert!(why.contains("is retired"), "{why}");
    ok(&again, &c, "principal_add", json!({"handle": "reviewer-c"})).await?;
    ok(&again, &c, "task_create", json!({"title": "back"})).await?;
    Ok(())
}

/// A lead from before automatic attachment, which worked by `--as` (lead-codex), is carried over to
/// its thread by the person, in a terminal (`fwd admin bind`), never by the thread choosing it; the
/// bind takes only such a lead, and never replaces a principal the thread has already.
#[tokio::test(flavor = "multi_thread")]
async fn the_person_binds_a_lead_from_before_to_its_thread_and_nothing_else() -> Result<()> {
    let s = Scratch::new()?;
    s.run(&["--as", "lead-old", "principal", "add", "lead-old", "--role", "lead"])?;
    let client = s.client().await?;
    let t = Thread::top("019a3333-0000-7000-8000-00000000000a");
    let whoami = ok(&client, &t, "whoami", json!({"project": "mcp"})).await?;
    assert_eq!(whoami["principal"], Value::Null);
    assert!(whoami["bind_instead"].as_str().unwrap().contains(&format!("--subject {}", t.subject())), "{whoami}");
    // A thread never takes over another principal itself: its name is another's.
    let why = refused(&client, Some(t.meta()), "principal_add", json!({"handle": "lead-old", "role": "lead"})).await?;
    assert!(why.contains("is taken"), "{why}");

    s.run(&["admin", "bind", "lead-old", "--subject", &t.subject()])?;
    let wrote = ok(&client, &t, "task_create", json!({"title": "as the old lead"})).await?;
    let old = s
        .json(&["principal", "list"])?
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["handle"] == "lead-old")
        .cloned()
        .unwrap();
    assert_eq!(wrote["record"]["created_by"], old["id"]);
    assert_eq!(s.bound_to(&t.subject())?.len(), 1);

    // A thread that wrote first has a principal of its own, which the bind never replaces.
    s.run(&["--as", "lead-two", "principal", "add", "lead-two", "--role", "lead"])?;
    let u = Thread::top("019a3333-0000-7000-8000-00000000000b");
    let first = ok(&client, &u, "task_create", json!({"title": "first by u"})).await?;
    let refused_bind = s.output(&["admin", "bind", "lead-two", "--subject", &u.subject()])?.expect_err("bound already");
    assert!(refused_bind.contains("nothing was bound"), "{refused_bind}");
    let after = ok(&client, &u, "task_create", json!({"title": "still u's own"})).await?;
    assert_eq!(after["record"]["created_by"], first["record"]["created_by"]);
    // The way out when the thread was made a principal first, even by a call that was refused:
    // the person retires that one, and the bind then takes the thread, unbinding it.
    s.run(&["--as", "lead-three", "principal", "add", "lead-three", "--role", "lead"])?;
    let w = Thread::top("019a3333-0000-7000-8000-00000000000d");
    let why = refused(&client, Some(w.meta()), "comment", json!({"first": "no task named"})).await?;
    assert!(!why.is_empty());
    let stub = s.bound_to(&w.subject())?;
    assert_eq!(stub.len(), 1, "the refused write made the thread a principal");
    let first_try = s.output(&["admin", "bind", "lead-three", "--subject", &w.subject()])?.expect_err("bound already");
    assert!(first_try.contains("retire @"), "{first_try}");
    s.run(&["principal", "retire", stub[0]["id"].as_str().unwrap()])?;
    s.run(&["admin", "bind", "lead-three", "--subject", &w.subject()])?;
    let as_lead = ok(&client, &w, "task_create", json!({"title": "as lead-three"})).await?;
    let three = s
        .json(&["principal", "list"])?
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["handle"] == "lead-three")
        .cloned()
        .unwrap();
    assert_eq!(as_lead["record"]["created_by"], three["id"]);
    let bound = s.bound_to(&w.subject())?;
    assert_eq!(bound.len(), 1, "the retired stub was unbound: {bound:?}");
    // Only a lead is bound so; nor is a Claude Code subject.
    s.run(&["--as", "impl-x", "principal", "add", "impl-x", "--role", "implementer"])?;
    let v = Thread::top("019a3333-0000-7000-8000-00000000000c");
    let not_lead = s.output(&["admin", "bind", "impl-x", "--subject", &v.subject()])?.expect_err("not a lead");
    assert!(not_lead.contains("is not a lead"), "{not_lead}");
    let claude = s.output(&["admin", "bind", "lead-two", "--subject", "claude-code:s/main"])?.expect_err("not Codex");
    assert!(claude.contains("is not a Codex thread's subject"), "{claude}");
    // An agent cannot run the person's bind: a Claude Code agent that named itself is refused.
    let agent = [("FWD_CLAUDE_SESSION", "s-1"), ("FWD_CLAUDE_AGENT", "main")];
    s.output_with(&["principal", "add", "claude-lead", "--role", "lead"], &agent)?.map_err(|e| anyhow!(e))?;
    let refused_agent =
        s.output_with(&["admin", "bind", "lead-two", "--subject", &v.subject()], &agent)?.expect_err("refused");
    assert!(refused_agent.contains("is the person's"), "{refused_agent}");
    Ok(())
}

/// A thread with no principal names itself, or names itself as another's successor, through
/// `principal_add`, with nothing made for it first; `principal_succeed` from such a thread is
/// refused as the CLI refuses it, making nothing; and several first `principal_add`s of one thread
/// make one principal.
#[tokio::test(flavor = "multi_thread")]
async fn a_thread_names_itself_or_succeeds_another_with_no_principal_made_first() -> Result<()> {
    let s = Scratch::new()?;
    s.run(&["--as", "lead-fwd", "principal", "add", "lead-fwd", "--role", "lead"])?;
    let client = std::sync::Arc::new(s.client().await?);
    let r = Thread::top("019a4444-0000-7000-8000-00000000000a");
    let why = refused(&client, Some(r.meta()), "principal_succeed", json!({"handle": "lead-fwd"})).await?;
    assert!(why.contains("has not named itself"), "{why}");
    assert!(s.bound_to(&r.subject())?.is_empty(), "no stub was made");
    let successor = ok(&client, &r, "principal_add", json!({"handle": "lead-codex", "succeeds": "lead-fwd"})).await?;
    assert_eq!(
        (successor["record"]["agent_role"].as_str(), successor["succeeded"]["active"].as_bool()),
        (Some("lead"), Some(false))
    );
    assert_eq!(s.bound_to(&r.subject())?.len(), 1);

    let n = Thread::top("019a4444-0000-7000-8000-00000000000b");
    let mut calls = Vec::new();
    for _ in 0..4 {
        let (client, n) = (client.clone(), n.clone());
        calls.push(tokio::spawn(async move {
            call(&client, Some(n.meta()), "principal_add", json!({"handle": "reviewer-n", "role": "reviewer"})).await
        }));
    }
    for c in calls {
        c.await??;
    }
    assert_eq!(s.bound_to(&n.subject())?.len(), 1, "one principal for one thread's first names");
    Ok(())
}

/// A thread bound under one session that calls under another is refused before any principal is
/// made, rather than becoming a second principal with its inbox and work split.
#[tokio::test(flavor = "multi_thread")]
async fn a_thread_seen_under_another_session_is_refused_not_split() -> Result<()> {
    let s = Scratch::new()?;
    let client = s.client().await?;
    let root = Thread::top("019a5555-0000-7000-8000-00000000000a");
    let child = Thread::spawned("019a5555-0000-7000-8000-00000000000b", &root);
    ok(&client, &child, "task_create", json!({"title": "by the child"})).await?;
    let moved = Thread { session: "019a5555-0000-7000-8000-0000000000ff".into(), ..child.clone() };
    let why = refused(&client, Some(moved.meta()), "task_create", json!({"title": "split"})).await?;
    assert!(why.contains("under another session"), "{why}");
    assert!(s.bound_to(&moved.subject())?.is_empty());
    Ok(())
}

/// The daemon's own environment, inherited from whichever command started it, means nothing to a
/// call: an `FWD_SCOPE`, `FWD_AS` or `CLAUDE_CODE_SESSION_ID` there neither puts a call in a scope,
/// nor makes it act as anyone, nor refuses it.
#[tokio::test(flavor = "multi_thread")]
async fn the_daemons_environment_means_nothing_to_a_call() -> Result<()> {
    let s = Scratch::new()?;
    let scope =
        s.json(&["scope", "create", "Inherited", "--intention", "x"])?["record"]["id"].as_str().unwrap().to_string();
    s.run(&["daemon", "stop"])?;
    let inherited = [
        ("FWD_SCOPE", scope.as_str()),
        ("FWD_AS", "tester"),
        ("FWD_PROJECT", "elsewhere"),
        ("CLAUDE_CODE_SESSION_ID", "s-inherited"),
    ];
    s.output_with(&["daemon", "start"], &inherited)?.map_err(|e| anyhow!(e))?;
    let client = s.client().await?;
    let t = Thread::top("019a6666-0000-7000-8000-00000000000a");
    let made = ok(&client, &t, "task_create", json!({"title": "no scope"})).await?;
    assert_eq!(made["record"]["scopes"], json!([]), "{made}");
    let tester =
        s.json(&["principal", "list"])?.as_array().unwrap().iter().find(|p| p["handle"] == "tester").cloned().unwrap();
    assert_ne!(made["record"]["created_by"], tester["id"]);
    let whoami = ok(&client, &t, "whoami", json!({})).await?;
    assert_eq!(whoami["subject"], json!(t.subject()));
    let in_scope = ok(&client, &t, "task_create", json!({"title": "in scope", "scope": scope})).await?;
    // `scope` is read as the CLI reads --scope: an id or its first characters, not a title.
    let by_title = refused(&client, Some(t.meta()), "task_create", json!({"title": "x", "scope": "Inherited"})).await?;
    let cli = s.output(&["--scope", "Inherited", "task", "create", "x"])?.expect_err("the CLI refuses a title too");
    assert!(cli.contains(&by_title), "MCP: {by_title}\nCLI: {cli}");
    assert_eq!(in_scope["record"]["scopes"][0]["id"], json!(scope));
    Ok(())
}

/// The endpoint answers only a process on this machine that is not a web page: a request carrying
/// an `Origin`, as a browser's does, is refused before it reaches MCP.
#[tokio::test(flavor = "multi_thread")]
async fn a_web_page_cannot_reach_the_endpoint() -> Result<()> {
    use std::io::{Read, Write};
    let s = Scratch::new()?;
    let url = s.url()?;
    let authority = url.trim_start_matches("http://").trim_end_matches("/mcp").to_string();
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}).to_string();
    let send = |origin: Option<&str>| -> Result<String> {
        let mut stream = std::net::TcpStream::connect(&authority)?;
        let origin = origin.map(|o| format!("Origin: {o}\r\n")).unwrap_or_default();
        write!(
            stream,
            "POST /mcp HTTP/1.1\r\nHost: {authority}\r\n{origin}Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\nMCP-Protocol-Version: 2025-06-18\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )?;
        let mut answer = String::new();
        stream.read_to_string(&mut answer)?;
        Ok(answer)
    };
    let from_page = send(Some(&format!("http://{authority}")))?;
    assert!(from_page.starts_with("HTTP/1.1 403"), "{from_page}");
    let from_process = send(None)?;
    assert!(from_process.starts_with("HTTP/1.1 200"), "{from_process}");
    Ok(())
}
