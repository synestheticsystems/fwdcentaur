//! Two projects in one daemon, as it keeps every project in one node and one store
//! (docs/DECISIONS.md, D135), filled through the CLI with look-alike records: the same person and
//! agents by the same handles, a scope, tasks and a ruling under the same names, the same question
//! and comment. No command run in one project prints a record of the other, whole or by the start
//! of its id, nor finds one by its id or its number; and what a command writes in one, and what the
//! daemon's routing makes of it, changes nothing in the other (D38).

mod common;

use ankurah::changes::ChangeSet;
use ankurah::signals::Subscribe;
use ankurah::EntityId;
use anyhow::{anyhow, Result};
use common::Scratch;
use fwd_core::anchoring::git;
use fwd_core::daemon::{self, Autostart, Client};
use fwd_core::model::*;
use fwd_core::ops::MessageContext;
use fwd_core::project::{NamedProject, Paths};
use fwd_core::project_graph::ProjectGraph;
use fwd_core::query::with_id;
use fwd_core::{naming, ops};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use tokio::sync::mpsc;

/// One daemon's folders, and a checkout of each of the two projects it hosts.
struct Daemon {
    _scratch: Scratch,
    paths: Paths,
    alpha: PathBuf,
    beta: PathBuf,
}

impl Daemon {
    /// The daemon's folders in a scratch folder, and two repositories, each the checkout of a
    /// project of its own that `dnorman` made.
    fn new() -> Result<Daemon> {
        let scratch = Scratch::new()?;
        let paths = scratch.paths("run", "data");
        let checkout = |name: &str| -> Result<PathBuf> {
            let root = scratch.path().join(name);
            std::fs::create_dir_all(&root)?;
            git(&root, &["init", "-q", "-b", "main"])?;
            git(&root, &["remote", "add", "origin", &format!("git@example.com:t/{name}.git")])?;
            Ok(root)
        };
        let (alpha, beta) = (checkout("alpha")?, checkout("beta")?);
        let daemon = Daemon { paths, alpha, beta, _scratch: scratch };
        daemon.json(&daemon.alpha, &["init", "--name", "alpha", "--handle", "dnorman"])?;
        daemon.json(&daemon.beta, &["init", "--name", "beta", "--handle", "dnorman"])?;
        Ok(daemon)
    }

    /// What `fwd <args>` exits with in `checkout`, reaching this daemon only, with none of the
    /// caller's settings that would choose another project, principal or scope: its standard output
    /// when it succeeds, its standard error when it fails.
    fn output(&self, checkout: &Path, args: &[&str]) -> Result<Result<String, String>> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fwd"));
        command
            .current_dir(checkout)
            .args(["--format", "json"])
            .args(args)
            .env("FWD_RUNTIME_DIR", &self.paths.runtime_dir)
            .env("FWD_DATA_DIR", &self.paths.data_dir);
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
        let out = command.output()?;
        Ok(if out.status.success() {
            Ok(String::from_utf8(out.stdout)?)
        } else {
            Err(String::from_utf8_lossy(&out.stderr).into_owned())
        })
    }

    /// The JSON `fwd <args>` prints in `checkout`, which must succeed.
    fn json(&self, checkout: &Path, args: &[&str]) -> Result<Value> {
        let printed = self.output(checkout, args)?.map_err(|e| anyhow!("fwd {} failed: {e}", args.join(" ")))?;
        Ok(serde_json::from_str(&printed)?)
    }

    /// The id of what `fwd <args>`, a command that makes something, made in `checkout`.
    fn made(&self, checkout: &Path, args: &[&str]) -> Result<String> {
        let printed = self.json(checkout, args)?;
        Ok(printed["record"]["id"].as_str().ok_or_else(|| anyhow!("no id under record in {printed}"))?.to_string())
    }

    /// A client of the project whose checkout is `checkout`, holding its graph.
    async fn client(&self, checkout: &Path) -> Result<Client> {
        let shown = self.json(checkout, &["project", "show"])?;
        let project = NamedProject {
            id: shown["id"].as_str().ok_or_else(|| anyhow!("no id in {shown}"))?.parse()?,
            name: shown["name"].as_str().unwrap_or_default().to_string(),
        };
        daemon::ensure_daemon(&self.paths, &project, Autostart::Never).await
    }
}

/// What one project holds once [`populate`] has run in its checkout.
struct Held {
    retry: String,
    cap: String,
    scope: String,
    question: String,
}

/// The same records in any project, made through the CLI: an implementer and a triage agent beside
/// the person; a scope; two tasks, one waiting on the other; D1; a question to the implementer,
/// and her comment on a task.
fn populate(daemon: &Daemon, checkout: &Path) -> Result<Held> {
    daemon.json(checkout, &["--as", "impl-alice", "principal", "add", "impl-alice", "--role", "implementer"])?;
    daemon.json(
        checkout,
        &["--as", "triage-bot", "principal", "add", "triage-bot", "--role", "triage", "--project-wide"],
    )?;
    let scope = daemon.made(checkout, &["scope", "create", "Replication"])?;
    let cap = daemon.made(checkout, &["task", "create", "Cap the retry delay"])?;
    let retry = daemon.made(checkout, &["task", "create", "Retry on disconnect", "--after", &cap])?;
    daemon.json(checkout, &["decision", "create", "Cap retries at 30s", "--decision", "30 seconds at most"])?;
    let question = daemon.made(checkout, &["ask", "--to", "impl-alice", "who takes the TLS part?"])?;
    daemon.json(checkout, &["--as", "impl-alice", "comment", &retry, "look at the backoff"])?;
    Ok(Held { retry, cap, scope, question })
}

/// Every record the project `graph` holds, of every model, as its JSON says it, by its id.
async fn records(graph: &ProjectGraph) -> Result<BTreeMap<EntityId, Value>> {
    let mut all = BTreeMap::new();
    for p in graph.all::<PrincipalView>().await? {
        all.insert(p.id(), ops::json_of_principal(&p)?);
    }
    for r in graph.all::<RepoView>().await? {
        all.insert(r.id(), serde_json::json!({"name": r.name()?, "remote_urls": Value::from(r.remote_urls()?)}));
    }
    for t in graph.all::<TaskView>().await? {
        all.insert(t.id(), ops::json_of_task(&t, None)?);
    }
    for r in graph.all::<RelationView>().await? {
        all.insert(r.id(), ops::json_of_relation(&r)?);
    }
    for a in graph.all::<CodeAnchorView>().await? {
        all.insert(a.id(), ops::json_of_anchor(&a)?);
    }
    for m in graph.all::<MessageView>().await? {
        all.insert(m.id(), ops::json_of_message(&m)?);
    }
    for n in graph.all::<NotificationView>().await? {
        all.insert(n.id(), ops::json_of_notification(&n)?);
    }
    for s in graph.all::<ScopeView>().await? {
        all.insert(s.id(), ops::json_of_scope(&s)?);
    }
    for d in graph.all::<DecisionView>().await? {
        all.insert(d.id(), ops::json_of_decision(&d)?);
    }
    for f in graph.all::<FormerIdView>().await? {
        all.insert(f.id(), serde_json::json!({"former": f.former()?, "record": f.record()?.to_base64()}));
    }
    Ok(all)
}

/// Each id in `ids`, whole and by its first eight characters, as the CLI prints one or the other.
fn spellings(ids: impl IntoIterator<Item = EntityId>) -> BTreeSet<String> {
    ids.into_iter().flat_map(|id| [id.to_base64(), id.to_base64()[..8].to_string()]).collect()
}

/// Fails, naming the command, when `printed` holds any of `others`.
fn prints_none_of(others: &BTreeSet<String>, command: &[&str], printed: &str) {
    let leaked: Vec<&String> = others.iter().filter(|id| printed.contains(id.as_str())).collect();
    assert!(leaked.is_empty(), "fwd {} printed the other project's ids {leaked:?}:\n{printed}", command.join(" "));
}

/// How long a test waits for an arrival before it fails as one that never came.
const NEVER: Duration = Duration::from_secs(60);

/// The notifications of `recipient` as they arrive, from a live query on its project's graph.
struct Inbox {
    arrived: mpsc::UnboundedReceiver<NotificationView>,
    _query: ankurah::LiveQuery<NotificationView>,
    _guard: ankurah::signals::SubscriptionGuard,
}

impl Inbox {
    fn of(graph: &ProjectGraph, recipient: EntityId) -> Result<Inbox> {
        let query = graph.query::<NotificationView>(with_id("recipient = ?", recipient)?)?;
        let (tx, arrived) = mpsc::unbounded_channel();
        let _guard = query.subscribe(move |changes: ChangeSet<NotificationView>| {
            for n in changes.added() {
                let _ = tx.send(n);
            }
        });
        Ok(Inbox { arrived, _query: query, _guard })
    }

    /// Returns, once it has arrived, the notification of the message `message`, which routing
    /// made after everything written in the project before that message. Fails, naming the
    /// message, when a minute passes first: the minute only detects a notification that never
    /// comes, and what orders the test is its arrival.
    async fn told_of(&mut self, message: &str) -> Result<EntityId> {
        let arrived = async {
            loop {
                let n = self.arrived.recv().await.ok_or_else(|| anyhow!("the inbox's query ended"))?;
                if Value::from(n.originator()?)["message_id"] == message {
                    return Ok(n.id());
                }
            }
        };
        tokio::time::timeout(NEVER, arrived)
            .await
            .map_err(|_| anyhow!("no notification of message {message} arrived within {}s", NEVER.as_secs()))?
    }
}

/// A daemon hosting alpha and beta, each filled by [`populate`], with a client of each.
struct Two {
    daemon: Daemon,
    a: Held,
    b: Held,
    alpha: Client,
    beta: Client,
}

impl Two {
    async fn new() -> Result<Two> {
        let daemon = Daemon::new()?;
        let a = populate(&daemon, &daemon.alpha)?;
        let b = populate(&daemon, &daemon.beta)?;
        let (alpha, beta) = (daemon.client(&daemon.alpha).await?, daemon.client(&daemon.beta).await?);
        assert_eq!(alpha.pf.instance_id, beta.pf.instance_id, "one daemon hosts both");
        Ok(Two { daemon, a, b, alpha, beta })
    }
}

/// Fails, naming the command, when `printed` holds none of `wanted`, so that a command checked to
/// print nothing of the other project's is known to have printed its own.
fn prints_its_own(wanted: &str, command: &[&str], printed: &str) {
    assert!(
        printed.contains(wanted),
        "fwd {} printed nothing of its own project's {wanted}:\n{printed}",
        command.join(" ")
    );
}

#[tokio::test]
async fn no_command_in_one_project_prints_finds_or_changes_a_record_of_the_other() -> Result<()> {
    let Two { daemon, a, b, alpha, beta } = Two::new().await?;
    let theirs = records(&beta.graph).await?;
    let others = spellings(theirs.keys().copied());
    assert_eq!(records(&alpha.graph).await?.len(), theirs.len(), "the two hold look-alike records");

    // Every command that reads, run in alpha, prints alpha's records, and none of beta's.
    let reads: Vec<Vec<&str>> = vec![
        vec!["task", "list"],
        vec!["task", "list", "--ready"],
        vec!["task", "show", &a.retry, "--include", "messages,relations"],
        vec!["task", "explain", &a.retry],
        vec!["--as", "impl-alice", "task", "next"],
        vec!["inbox"],
        vec!["--as", "impl-alice", "inbox"],
        vec!["--as", "triage-bot", "inbox"],
        vec!["decision", "list"],
        vec!["decision", "show", "D1"],
        vec!["decision", "export"],
        vec!["scope", "list"],
        vec!["scope", "show", &a.scope],
        vec!["principal", "list"],
        vec!["relations", &a.retry],
        vec!["graph", "--all"],
        vec!["message", "show", &a.question],
        vec!["step", "list", &a.retry],
        vec!["whoami"],
        vec!["--as", "impl-alice", "prime"],
    ];
    for command in &reads {
        let printed = daemon.output(&daemon.alpha, command)?.map_err(|e| anyhow!("fwd {}: {e}", command.join(" ")))?;
        prints_none_of(&others, command, &printed);
    }
    let listed = daemon.json(&daemon.alpha, &["task", "list"])?.to_string();
    assert!(listed.contains(&a.retry) && listed.contains(&a.cap), "alpha's own tasks are listed: {listed}");

    // A record of beta's, named in alpha by its whole id or the start of it, is not found; D1 is
    // alpha's own.
    for named in [b.retry.as_str(), &b.retry[..8], b.question.as_str(), &b.cap[..8]] {
        let shown = daemon.output(&daemon.alpha, &["task", "show", named])?;
        assert!(shown.is_err(), "fwd task show {named} in alpha found beta's record: {shown:?}");
    }
    let d1 = daemon.json(&daemon.alpha, &["decision", "show", "D1"])?.to_string();
    let ours = records(&alpha.graph).await?;
    assert!(ours.keys().any(|id| d1.contains(&id.to_base64()) && ours[id]["ref"] == "D1"), "{d1}");

    // What alpha's writes, and the daemon's routing of them, make stays in alpha. Routing handles
    // one project's records in the order they were written, so once the question asked last in
    // alpha has reached its implementer, everything before it has been routed; and once a question
    // asked in beta after it has reached beta's, the one node has passed all of alpha's writes on.
    let alice_of = |graph: &ProjectGraph| {
        let graph = graph.clone();
        async move {
            let named = naming::principal_named(&graph, "impl-alice", None).await?;
            named.map(|p| p.id()).ok_or_else(|| anyhow!("no impl-alice"))
        }
    };
    let mut alpha_inbox = Inbox::of(&alpha.graph, alice_of(&alpha.graph).await?)?;
    let mut beta_inbox = Inbox::of(&beta.graph, alice_of(&beta.graph).await?)?;
    daemon.json(&daemon.alpha, &["ask", "--to", "impl-alice", "and the notifications?"])?;
    let made = daemon.made(&daemon.alpha, &["task", "create", "Cap the retry delay", "--after", &a.retry])?;
    daemon.json(&daemon.alpha, &["task", "assign", &made, "@impl-alice"])?;
    daemon.json(&daemon.alpha, &["--as", "impl-alice", "comment", &a.retry, "done with the backoff"])?;
    daemon.json(&daemon.alpha, &["decision", "create", "Retry forever", "--propose"])?;
    let last = daemon.made(&daemon.alpha, &["ask", "--to", "impl-alice", "anything else?"])?;
    alpha_inbox.told_of(&last).await?;
    let alpha_now = records(&alpha.graph).await?;
    assert!(alpha_now.values().any(|d| d["number"] == 2), "alpha's next ruling is D2, whatever beta holds");

    // Asked through the test's own client rather than the CLI, which would also stamp the asker as
    // having written, so that beta's question and its notification are all it adds.
    let beta_alice = alice_of(&beta.graph).await?;
    let person = ops::person_by_handle(&beta.graph, "dnorman").await?;
    let to_alice = MessageContext { to: Some(beta_alice.to_base64()), ..Default::default() };
    let asked = ops::post_message(&beta.graph, person, "is it in yet?", Priority::Now, to_alice).await?;
    let reached = beta_inbox.told_of(&asked.to_base64()).await?;
    let mut now = records(&beta.graph).await?;
    assert!(now.remove(&asked).is_some() && now.remove(&reached).is_some(), "beta's own question reached her");
    assert_eq!(now, theirs, "nothing alpha's commands wrote, or its routing made of them, changed beta");
    Ok(())
}

/// Records, through `client`'s graph, that `record` once had the id `former`, as the move that
/// renumbers records writes it.
async fn renumbered(client: &Client, former: &str, record: EntityId) -> Result<()> {
    let trx = client.graph.begin();
    trx.create(&FormerId { project: client.graph.project(), former: former.into(), record }).await?;
    trx.commit().await?;
    Ok(())
}

/// An id a record of alpha's had before it was renumbered names that record wherever a command takes
/// an id, whole or as a prefix: `--as` for a principal, a scope by name or id, a checklist step by
/// its relation or by what it is, any record's argument, and the scope a checkout is bound to. In
/// beta, the same text names nothing.
#[tokio::test]
async fn a_former_id_names_its_record_wherever_a_command_takes_an_id_in_its_own_project_alone() -> Result<()> {
    let Two { daemon, a, b, alpha, beta: _ } = Two::new().await?;
    let alice = naming::principal_named(&alpha.graph, "impl-alice", None).await?.map(|p| p.id()).expect("impl-alice");
    let scope: EntityId = a.scope.parse()?;
    let step: EntityId = daemon.made(&daemon.alpha, &["step", "add", &a.retry, &a.cap])?.parse()?;
    let cited = || EntityId::random().to_base64();
    let (as_alice, as_scope, as_step, as_cap, as_question) = (cited(), cited(), cited(), cited(), cited());
    renumbered(&alpha, &as_alice, alice).await?;
    renumbered(&alpha, &as_scope, scope).await?;
    renumbered(&alpha, &as_step, step).await?;
    renumbered(&alpha, &as_cap, a.cap.parse()?).await?;
    renumbered(&alpha, &as_question, a.question.parse()?).await?;

    let in_alpha = |args: &[&str]| daemon.json(&daemon.alpha, args).map(|v| v.to_string());
    for named in [as_alice.as_str(), &as_alice[..8]] {
        prints_its_own(&alice.to_base64(), &["--as", named, "whoami"], &in_alpha(&["--as", named, "whoami"])?);
    }
    in_alpha(&["decision", "attach", "D1", &as_scope])?;
    let attached = in_alpha(&["decision", "list", "--attached", &as_scope[..8]])?;
    prints_its_own("\"D1\"", &["decision", "list", "--attached", &as_scope[..8]], &attached);
    in_alpha(&["step", "check", &a.retry, &as_step])?;
    in_alpha(&["step", "uncheck", &a.retry, &as_cap[..8]])?;
    prints_its_own(&a.cap, &["task", "show", &as_cap], &in_alpha(&["task", "show", &as_cap])?);
    prints_its_own(
        &a.question,
        &["message", "show", &as_question[..8]],
        &in_alpha(&["message", "show", &as_question[..8]])?,
    );

    // The scope a checkout was bound to before the renumbering, which checkouts.json keeps as the
    // id it had then: a scope on another branch, which the checkout finds through its binding alone
    // (on its own branch it finds Replication).
    let elsewhere: EntityId =
        daemon.made(&daemon.alpha, &["scope", "create", "Elsewhere", "--branch", "elsewhere"])?.parse()?;
    let as_elsewhere = cited();
    renumbered(&alpha, &as_elsewhere, elsewhere).await?;
    assert_eq!(
        daemon.json(&daemon.alpha, &["whoami"])?["scope"],
        a.scope,
        "unbound, the checkout finds its branch's scope"
    );
    let checkout = fwd_core::project::checkout_at(&daemon.alpha)?.expect("alpha's checkout");
    let mut binding = fwd_core::daemon::CheckoutRecord::from(&checkout);
    binding.scope = Some(as_elsewhere);
    daemon::report_checkout(&alpha, &binding).await?;
    let here = daemon.json(&daemon.alpha, &["whoami"])?;
    assert_eq!(
        here["scope"],
        elsewhere.to_base64(),
        "the checkout's binding names its scope through the former id: {here}"
    );

    for args in [
        vec!["--as", as_alice.as_str(), "whoami"],
        vec!["decision", "attach", "D1", &as_scope],
        vec!["decision", "list", "--attached", &as_scope],
        vec!["step", "check", &b.retry, &as_step],
        vec!["task", "show", &as_cap],
        vec!["message", "show", &as_question[..8]],
    ] {
        let refused = daemon.output(&daemon.beta, &args)?;
        assert!(refused.is_err(), "fwd {} in beta found what alpha's former id names: {refused:?}", args.join(" "));
    }
    Ok(())
}

/// The next line a `fwd watch` prints, as JSON; fails, saying what was awaited, when none comes in a
/// minute.
async fn next_line(
    lines: &mut tokio::io::Lines<tokio::io::BufReader<tokio::process::ChildStdout>>,
    awaited: &str,
) -> Result<Value> {
    let line = tokio::time::timeout(NEVER, lines.next_line())
        .await
        .map_err(|_| anyhow!("the watch printed no line for {awaited} within {}s", NEVER.as_secs()))??
        .ok_or_else(|| anyhow!("the watch ended before {awaited}"))?;
    Ok(serde_json::from_str(&line)?)
}

/// A `fwd watch` held open in alpha, as an agent runs it, prints alpha's notifications alone: a
/// question asked in beta while it waits never reaches it, and the one asked in alpha after it does.
#[tokio::test]
async fn a_watch_held_open_in_one_project_hears_nothing_written_in_the_other() -> Result<()> {
    use tokio::io::AsyncBufReadExt;
    let Two { daemon, a, b: _, alpha: _, beta } = Two::new().await?;
    let mut watch = tokio::process::Command::from({
        let mut command = Command::new(env!("CARGO_BIN_EXE_fwd"));
        command
            .current_dir(&daemon.alpha)
            .args(["--format", "json", "--as", "impl-alice", "watch", "--count", "2"])
            .env("FWD_RUNTIME_DIR", &daemon.paths.runtime_dir)
            .env("FWD_DATA_DIR", &daemon.paths.data_dir)
            .env_remove("FWD_PROJECT")
            .env_remove("FWD_AS")
            .env_remove("FWD_CLAUDE_SESSION")
            .env_remove("FWD_CLAUDE_AGENT")
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .stdout(std::process::Stdio::piped());
        command
    })
    .kill_on_drop(true)
    .spawn()?;
    let mut lines = tokio::io::BufReader::new(watch.stdout.take().expect("piped")).lines();
    // The question waiting from `populate` comes first, as the watch's snapshot: the watch is open.
    let waiting = next_line(&mut lines, "the question waiting in alpha").await?;
    assert_eq!(waiting["notification"]["originator"]["message_id"], a.question, "{waiting}");

    daemon.made(&daemon.beta, &["ask", "--to", "impl-alice", "is beta's question heard in alpha?"])?;
    let asked = daemon.made(&daemon.alpha, &["ask", "--to", "impl-alice", "is alpha's?"])?;
    let heard = next_line(&mut lines, "the question asked in alpha").await?;
    assert_eq!(
        heard["notification"]["originator"]["message_id"], asked,
        "the next thing the watch hears is alpha's question: {heard}"
    );
    let others = spellings(records(&beta.graph).await?.keys().copied());
    for line in [&waiting, &heard] {
        prints_none_of(&others, &["watch"], &line.to_string());
    }
    let ended = tokio::time::timeout(NEVER, watch.wait()).await.map_err(|_| anyhow!("the watch did not end"))??;
    assert!(ended.success(), "the watch ended with {ended}");
    Ok(())
}

/// The daemon's MCP endpoint, called as a Codex thread in alpha, gives alpha's records alone from
/// every tool that reads, and finds none of beta's by its id.
#[tokio::test(flavor = "multi_thread")]
async fn the_mcp_read_tools_in_one_project_give_none_of_the_others_records() -> Result<()> {
    use rmcp::model::{CallToolRequestParams, RequestMetaObject};
    use rmcp::ServiceExt;
    let Two { daemon, a, b, alpha, beta } = Two::new().await?;
    daemon.made(&daemon.alpha, &["step", "add", &a.retry, &a.cap])?;
    let url = format!("http://127.0.0.1:{}/mcp", alpha.pf.port);
    let client = ().serve(rmcp::transport::StreamableHttpClientTransport::from_uri(url)).await?;
    let thread = "019a0000-0000-7000-8000-0000000a1fa0";
    let mut meta = RequestMetaObject::new();
    for (key, value) in serde_json::json!({
        "threadId": thread,
        "sessionId": thread,
        "x-codex-turn-metadata": {"thread_id": thread, "session_id": thread, "turn_id": "turn-1", "model": "fake"},
    })
    .as_object()
    .cloned()
    .unwrap_or_default()
    {
        meta.insert(key, value);
    }
    let call = |tool: &'static str, args: Value| {
        let (client, meta) = (&client, meta.clone());
        async move {
            let mut args = args.as_object().cloned().unwrap_or_default();
            args.insert("project".into(), serde_json::json!("alpha"));
            let mut params = CallToolRequestParams::new(tool.to_string()).with_arguments(args);
            params.meta = Some(meta);
            anyhow::Ok(client.call_tool(params).await?)
        }
    };
    let others = spellings(records(&beta.graph).await?.keys().copied());
    for (tool, args, own) in [
        ("task_list", serde_json::json!({}), a.retry.as_str()),
        ("task_show", serde_json::json!({"id": a.retry}), a.retry.as_str()),
        ("task_explain", serde_json::json!({"id": a.retry}), a.cap.as_str()),
        ("step_list", serde_json::json!({"task": a.retry}), a.cap.as_str()),
        ("relations", serde_json::json!({"id": a.retry}), a.cap.as_str()),
        ("scope_list", serde_json::json!({}), a.scope.as_str()),
        ("decision_list", serde_json::json!({}), "D1"),
        ("decision_show", serde_json::json!({"id": "D1"}), "Cap retries at 30s"),
        ("principal_list", serde_json::json!({}), "impl-alice"),
        ("message_show", serde_json::json!({"id": a.question}), a.question.as_str()),
    ] {
        let answer = call(tool, args.clone()).await?;
        let given = answer.structured_content.clone().unwrap_or_default().to_string();
        assert_ne!(answer.is_error, Some(true), "{tool} {args} was refused: {given}");
        prints_none_of(&others, &[tool], &given);
        prints_its_own(own, &[tool], &given);
    }
    for (tool, id) in [("task_show", &b.retry), ("message_show", &b.question)] {
        let answer = call(tool, serde_json::json!({"id": id})).await?;
        assert_eq!(answer.is_error, Some(true), "{tool} of beta's {id} through alpha answered");
    }
    Ok(())
}
