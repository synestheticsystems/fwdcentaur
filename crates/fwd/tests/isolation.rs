//! Two projects in one daemon, as it keeps every project in one node and one store
//! (docs/DECISIONS.md, D135), filled through the CLI with look-alike records: the same person and
//! agents by the same handles, a scope, tasks and a ruling under the same names, the same question
//! and comment. A project is a grouping and the filter of every list, not a wall (D176, D181):
//! every command that lists, and every watch, run in one project prints that project's records
//! alone, and what a command writes in one, and what the daemon's routing makes of it, changes
//! nothing in the other; while a record of the other project, named by its id or the start of it,
//! is found there, the command runs there and says so, and a record held by one project's record
//! is shown with its project's name, through the CLI and through the MCP tools alike.

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
use serde_json::{json, Value};
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

    /// `fwd --format <format> <args>` in `checkout`, reaching this daemon only, with none of the
    /// caller's settings that would choose another project, principal or scope.
    fn fwd(&self, checkout: &Path, format: &str, args: &[&str]) -> Result<std::process::Output> {
        self.fwd_with(checkout, format, args, &[])
    }

    /// `fwd --format <format> <args>` as [`Daemon::fwd`] runs it, with `env` set too.
    fn fwd_with(
        &self,
        checkout: &Path,
        format: &str,
        args: &[&str],
        env: &[(&str, &str)],
    ) -> Result<std::process::Output> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fwd"));
        command
            .current_dir(checkout)
            .args(["--format", format])
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
        Ok(command.envs(env.iter().copied()).output()?)
    }

    /// What `fwd <args>` exits with in `checkout`, as JSON: its standard output when it succeeds,
    /// its standard error when it fails.
    fn output(&self, checkout: &Path, args: &[&str]) -> Result<Result<String, String>> {
        let out = self.fwd(checkout, "json", args)?;
        Ok(if out.status.success() {
            Ok(String::from_utf8(out.stdout)?)
        } else {
            Err(String::from_utf8_lossy(&out.stderr).into_owned())
        })
    }

    /// The JSON `fwd <args>` prints in `checkout`, which must succeed, and what it said on standard
    /// error beside it.
    fn json_and_said(&self, checkout: &Path, args: &[&str]) -> Result<(Value, String)> {
        self.json_and_said_with(checkout, args, &[])
    }

    /// What [`Daemon::json_and_said`] gives, for `fwd <args>` run with `env` set too.
    fn json_and_said_with(&self, checkout: &Path, args: &[&str], env: &[(&str, &str)]) -> Result<(Value, String)> {
        let out = self.fwd_with(checkout, "json", args, env)?;
        let said = String::from_utf8_lossy(&out.stderr).into_owned();
        if !out.status.success() {
            return Err(anyhow!("fwd {} failed: {said}", args.join(" ")));
        }
        Ok((serde_json::from_slice(&out.stdout)?, said))
    }

    /// The text `fwd <args>` prints for people in `checkout`, which must succeed.
    fn text(&self, checkout: &Path, args: &[&str]) -> Result<String> {
        let out = self.fwd(checkout, "text", args)?;
        if !out.status.success() {
            return Err(anyhow!("fwd {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr)));
        }
        Ok(String::from_utf8(out.stdout)?)
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
    /// Opens the query, and returns once it holds what the inbox holds already, so that what
    /// arrives after is heard as it arrives.
    async fn of(graph: &ProjectGraph, recipient: EntityId) -> Result<Inbox> {
        let query = graph.query::<NotificationView>(with_id("recipient = ?", recipient)?)?;
        query.wait_initialized().await?;
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

/// Fails unless `fwd <args>`, run in alpha, ran in beta, the project of the record it acts on: it
/// said so on standard error, and its JSON names beta as `from_project`. Returns the JSON.
fn ran_in_beta(daemon: &Daemon, args: &[&str]) -> Result<Value> {
    let (printed, said) = daemon.json_and_said(&daemon.alpha, args)?;
    assert!(said.contains("is from project `beta`, so this command ran there"), "fwd {}: {said}", args.join(" "));
    assert_eq!(printed["from_project"], "beta", "fwd {}: {printed}", args.join(" "));
    Ok(printed)
}

#[tokio::test]
async fn every_list_in_one_project_prints_its_own_records_a_typed_id_reaches_the_other_and_writes_stay() -> Result<()> {
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

    // A record of beta's, named in alpha by its whole id or the start of it, is found in beta,
    // where the command runs, saying so; D1 is alpha's own, a decision's number being its
    // project's.
    for (named, record) in [(b.retry.as_str(), &b.retry), (&b.retry[..8], &b.retry), (&b.cap[..8], &b.cap)] {
        assert_eq!(ran_in_beta(&daemon, &["task", "show", named])?["id"], *record);
    }
    assert_eq!(ran_in_beta(&daemon, &["message", "show", &b.question])?["id"], b.question);
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
    let mut alpha_inbox = Inbox::of(&alpha.graph, alice_of(&alpha.graph).await?).await?;
    let mut beta_inbox = Inbox::of(&beta.graph, alice_of(&beta.graph).await?).await?;
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

/// A task of alpha's shows what it holds of beta's with beta's name, as JSON and as text: the task
/// it depends on, which gates its readiness until beta finishes it, the scope it is worked in, its
/// assignee and the author of a comment on it, as a task moved from beta keeps them, and the
/// relation beta made that points at it. `fwd link` run in alpha joins alpha's task to beta's.
#[tokio::test]
async fn a_task_shows_what_it_holds_of_the_other_project_with_that_projects_name() -> Result<()> {
    let Two { daemon, a, b, alpha, beta } = Two::new().await?;
    daemon.json(&daemon.alpha, &["link", &a.cap, "depends_on", &b.cap])?;
    daemon.json(&daemon.alpha, &["link", &a.cap, "worked_in", &b.scope])?;
    daemon.json(&daemon.beta, &["link", &b.retry, "relates_to", &a.cap])?;
    let their_alice = naming::principal_named(&beta.graph, "impl-alice", None).await?.map(|p| p.id()).expect("hers");
    let their_person = ops::person_by_handle(&beta.graph, "dnorman").await?;
    let cap: EntityId = a.cap.parse()?;
    let trx = alpha.graph.begin();
    let held: TaskView = alpha.graph.get(cap).await?;
    trx.edit(&held)?.assignee()?.set(&Some(ankurah::property::Ref::new(their_alice)))?;
    trx.commit().await?;
    let on_cap = MessageContext { task_id: Some(cap), ..Default::default() };
    ops::post_message(&alpha.graph, their_person, "written in beta", Priority::Next, on_cap).await?;

    let args = ["task", "show", a.cap.as_str(), "--include", "relations,messages"];
    let shown = daemon.json(&daemon.alpha, &args)?;
    assert_eq!(shown["assignee_handle"], "impl-alice in beta", "{shown}");
    // Worked in alpha's scope, where it was filed, and in beta's, of the same title.
    let scopes: Vec<(Value, Value)> = shown["scopes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| (s["title"].clone(), s.get("project").cloned().unwrap_or(Value::Null)))
        .collect();
    assert_eq!(scopes, vec![(json!("Replication"), Value::Null), (json!("Replication in beta"), json!("beta"))]);
    let other = |side: &str, kind: &str| {
        shown["relations"][side]
            .as_array()
            .into_iter()
            .flatten()
            .find(|r| r["kind"] == kind)
            .map(|r| r["other"].clone())
    };
    let blocker = other("out", "depends_on").expect("the dependency");
    assert_eq!((&blocker["name"], &blocker["project"]), (&json!("Cap the retry delay in beta"), &json!("beta")));
    let pointing = other("in", "relates_to").expect("beta's relation");
    assert_eq!((&pointing["id"], &pointing["project"]), (&json!(b.retry), &json!("beta")));
    let said = shown["messages"].as_array().into_iter().flatten().find(|m| m["content"] == "written in beta");
    assert_eq!(said.map(|m| m["author_handle"].clone()), Some(json!("dnorman in beta")));
    assert_eq!(shown["readiness"]["state"], "not_ready");
    assert!(
        shown["readiness"]["reasons"].to_string().contains("BlockedByTaskIn")
            && shown["readiness"]["reasons"].to_string().contains("beta")
    );
    let text = daemon.text(&daemon.alpha, &args)?;
    for line in [
        "assigned to @impl-alice in beta",
        "scopes: Replication, Replication in beta",
        "Cap the retry delay in beta (task,",
        "@dnorman in beta",
    ] {
        assert!(text.contains(line), "`{line}` in:\n{text}");
    }
    let listed = daemon.json(&daemon.alpha, &["relations", &a.cap])?;
    let theirs: Vec<&Value> = listed
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| [&b.cap, &b.scope, &b.retry].iter().any(|id| r["other"]["id"] == **id))
        .collect();
    assert_eq!(theirs.len(), 3, "{listed}");
    assert!(theirs.iter().all(|r| r["other"]["project"] == "beta"), "{listed}");

    // Finished in beta, the dependency counts as finished in alpha.
    ops::transition(&beta.graph, their_person, b.cap.parse()?, fwd_core::rules::Verb::Done, None).await?;
    assert_eq!(daemon.json(&daemon.alpha, &["task", "show", &a.cap])?["readiness"]["state"], "ready");
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
/// its relation or by what it is, any record's argument, and the scope a checkout is bound to.
/// Typed in beta as a record's argument, it names alpha's record, where the command runs, and as a
/// scope's id it names alpha's scope, which a ruling of beta's may govern; as a principal's name or
/// a step of beta's checklist, which are each project's own, it names nothing in beta.
#[tokio::test]
async fn a_former_id_names_its_record_wherever_a_command_takes_an_id_and_as_a_records_argument_in_any_project(
) -> Result<()> {
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

    for args in [vec!["--as", as_alice.as_str(), "whoami"], vec!["step", "check", &b.retry, &as_step]] {
        let refused = daemon.output(&daemon.beta, &args)?;
        assert!(refused.is_err(), "fwd {} in beta found what alpha's former id names: {refused:?}", args.join(" "));
    }
    let attached = daemon.json(&daemon.beta, &["decision", "attach", "D1", &as_scope])?;
    assert_eq!((&attached["scope"], &attached["scope_title"]), (&json!(a.scope), &json!("Replication in alpha")));
    let listed = daemon.json(&daemon.beta, &["decision", "list", "--attached", &as_scope[..8]])?;
    let refs: Vec<&Value> = listed.as_array().into_iter().flatten().map(|d| &d["ref"]).collect();
    assert_eq!(refs, vec!["D1"], "beta's D1, attached to alpha's scope: {listed}");
    for (args, record) in
        [(vec!["task", "show", &as_cap], &a.cap), (vec!["message", "show", &as_question[..8]], &a.question)]
    {
        let (printed, said) = daemon.json_and_said(&daemon.beta, &args)?;
        assert_eq!((&printed["id"], &printed["from_project"]), (&json!(record), &json!("alpha")), "{said}");
    }
    Ok(())
}

/// A comment typed in alpha on beta's task, named by the start of its id, is written in beta, by
/// the person's principal there, and says so: a person neither project holds a principal for is
/// found, or made, in beta alone, and nothing of alpha's changes, a principal for that person
/// included. An agent beta alone holds acts there through `--as`, typed in alpha.
#[tokio::test]
async fn a_write_through_another_projects_id_is_made_there_by_the_caller_there_and_nothing_where_it_started(
) -> Result<()> {
    let Two { daemon, a: _, b, alpha, beta } = Two::new().await?;
    daemon.json(&daemon.beta, &["--as", "impl-bea", "principal", "add", "impl-bea", "--role", "implementer"])?;
    // The person the next commands act as, whom neither project has met.
    std::fs::write(daemon.paths.data_dir.join("identity.toml"), "handle = \"carol\"\ndisplay_name = \"Carol\"\n")?;
    let before = records(&alpha.graph).await?;

    let named = &b.retry[..8];
    let (made, said) = daemon.json_and_said(&daemon.alpha, &["comment", named, "from alpha's checkout"])?;
    assert!(said.contains(&format!("NOTE: `{named}` is from project `beta`, so this command ran there")), "{said}");
    assert_eq!(made["from_project"], "beta", "{made}");
    let comment: EntityId = made["record"]["id"].as_str().ok_or_else(|| anyhow!("no id in {made}"))?.parse()?;
    let written: MessageView = beta.graph.get(comment).await?;
    let carol = ops::person_by_handle(&beta.graph, "carol").await?;
    assert_eq!(EntityId::from(written.author()?), carol, "written by carol's principal in beta");
    assert_eq!(written.task()?.map(EntityId::from), Some(b.retry.parse()?));

    let shown = ran_in_beta(&daemon, &["--as", "impl-bea", "task", "show", &b.retry])?;
    assert_eq!(shown["id"], b.retry);
    let (_, said) = daemon.json_and_said(&daemon.alpha, &["--as", "impl-bea", "comment", &b.retry, "and from her"])?;
    assert!(said.contains("so this command ran there"), "{said}");

    assert_eq!(records(&alpha.graph).await?, before, "nothing of alpha's changed, and no principal was made there");
    assert!(ops::person_by_handle(&alpha.graph, "carol").await.is_err(), "alpha holds no principal for carol");
    Ok(())
}

/// A command typed in alpha whose record is beta's, refused in beta for its caller, an agent alpha
/// holds and beta does not, says that it ran in beta, keeps the commands the refusal suggests as it
/// words them, and ends saying that any of them is for beta too and how to run it there, since run
/// in alpha's checkout it would run in alpha: a comment, and a move of beta's task.
#[tokio::test]
async fn a_refusal_in_the_project_a_command_ran_in_names_it_and_says_what_it_suggests_is_for_it_too() -> Result<()> {
    let Two { daemon, a: _, b, alpha: _, beta: _ } = Two::new().await?;
    daemon.json(&daemon.alpha, &["--as", "lead-alba", "principal", "add", "lead-alba", "--role", "lead"])?;
    let named = &b.retry[..8];
    for args in [
        vec!["--as", "lead-alba", "comment", named, "from alpha's lead"],
        vec!["--as", "lead-alba", "task", "move", "--to", "alpha", named],
    ] {
        let refused = daemon.output(&daemon.alpha, &args)?.expect_err("beta holds no lead-alba");
        let ran = format!("`{named}` is from project `beta`, so this command ran there: no principal of this project is named `lead-alba`");
        assert!(refused.contains(&ran), "fwd {}: {refused}", args.join(" "));
        assert!(
            refused.contains("`fwd principal add lead-alba --role ") && refused.contains("`fwd init`"),
            "fwd {}: {refused}",
            args.join(" ")
        );
        assert!(
            refused.trim_end().ends_with(
                "Any command suggested above is for project `beta` too: run it there with `--project 'beta'`, or from one of its checkouts"
            ),
            "fwd {}: {refused}",
            args.join(" ")
        );
    }
    Ok(())
}

/// A comment of alpha's that names beta's implementer tells her in beta, where her inbox is, and
/// her reply through that notification is a message of the comment's thread, made in alpha by her,
/// beta's principal, with the notification marked read in beta; her forward through the next one is
/// made in alpha too, asks beta's person, and reaches him in beta.
#[tokio::test]
async fn a_recipient_of_another_project_replies_and_forwards_through_her_notification() -> Result<()> {
    let Two { daemon, a, b: _, alpha, beta } = Two::new().await?;
    let named = |graph: ProjectGraph, name: &'static str| async move {
        naming::principal_named(&graph, name, None).await?.map(|p| p.id()).ok_or_else(|| anyhow!("no {name}"))
    };
    let alice = named(beta.graph.clone(), "impl-alice").await?;
    let daniel = named(beta.graph.clone(), "dnorman").await?;
    let (mut hers, mut his) = (Inbox::of(&beta.graph, alice).await?, Inbox::of(&beta.graph, daniel).await?);
    let mention = format!("@{} look at the backoff", alice.to_base64());

    let comment = daemon.made(&daemon.alpha, &["comment", &a.retry, &mention])?;
    let told = hers.told_of(&comment).await?.to_base64();
    let (replied, _) = daemon.json_and_said(&daemon.beta, &["--as", "impl-alice", "reply", &told, "on it"])?;
    let reply: EntityId = replied["record"]["id"].as_str().ok_or_else(|| anyhow!("{replied}"))?.parse()?;
    assert_eq!(replied["settled"], told, "{replied}");
    let made: MessageView = alpha.graph.get(reply).await?;
    assert_eq!(made.reply_to()?.map(EntityId::from), Some(comment.parse()?), "a message of the comment's thread");
    assert_eq!(EntityId::from(made.author()?), alice, "by her, beta's principal");
    assert_eq!(beta.graph.get::<NotificationView>(told.parse()?).await?.status()?, NotificationStatus::Read);

    let comment = daemon.made(&daemon.alpha, &["comment", &a.retry, &mention])?;
    let told = hers.told_of(&comment).await?.to_base64();
    let args = ["--as", "impl-alice", "forward", &told, "--to", "dnorman", "--note", "yours, I think"];
    let (forwarded, _) = daemon.json_and_said(&daemon.beta, &args)?;
    let forward = forwarded["record"]["id"].as_str().ok_or_else(|| anyhow!("{forwarded}"))?.to_string();
    let made: MessageView = alpha.graph.get(forward.parse()?).await?;
    assert_eq!(made.reply_to()?.map(EntityId::from), Some(comment.parse()?));
    assert_eq!(Value::from(made.context()?)["to"], daniel.to_base64(), "it asks beta's person");
    assert_eq!(beta.graph.get::<NotificationView>(told.parse()?).await?.status()?, NotificationStatus::Read);
    his.told_of(&forward).await?;
    Ok(())
}

/// The messages about a record are its connections, read by its id wherever they were written, as
/// its relations are: `message list --on` alpha's task lists a message beta holds about it beside
/// alpha's own, named with beta, as JSON and for people, while the list of alpha's messages holds
/// none of beta's.
#[tokio::test]
async fn the_messages_about_a_record_are_listed_wherever_they_are_each_named_with_its_project() -> Result<()> {
    let Two { daemon, a, b: _, alpha: _, beta } = Two::new().await?;
    let their_daniel = naming::principal_named(&beta.graph, "dnorman", None).await?.map(|p| p.id()).expect("his");
    let about = MessageContext { task_id: Some(a.retry.parse()?), ..Default::default() };
    let theirs = ops::post_message(&beta.graph, their_daniel, "beta's word on it", Priority::Next, about).await?;

    let listed = daemon.json(&daemon.alpha, &["message", "list", "--on", &a.retry])?;
    let listed = listed.as_array().ok_or_else(|| anyhow!("{listed}"))?;
    let named: Vec<(&str, Option<&str>)> =
        listed.iter().map(|m| (m["content"].as_str().unwrap_or_default(), m["project"].as_str())).collect();
    assert!(named.contains(&("look at the backoff", None)), "alpha's own comment, unnamed: {named:?}");
    assert!(named.contains(&("beta's word on it", Some("beta"))), "beta's, named with beta: {named:?}");
    let text = daemon.text(&daemon.alpha, &["message", "list", "--on", &a.retry])?;
    assert!(text.contains(&format!("{} beta's word on it [active] in beta", theirs.to_base64())), "{text}");
    let alphas = daemon.json(&daemon.alpha, &["message", "list"])?.to_string();
    assert!(!alphas.contains(&theirs.to_base64()), "a list of alpha's messages holds none of beta's: {alphas}");
    Ok(())
}

/// A notification of beta's, named in alpha by its whole id as by the start of it, is read in beta,
/// by its recipient there.
#[tokio::test]
async fn a_notifications_whole_id_typed_in_one_project_marks_it_where_it_is() -> Result<()> {
    let Two { daemon, a: _, b: _, alpha: _, beta } = Two::new().await?;
    let alice = naming::principal_named(&beta.graph, "impl-alice", None).await?.map(|p| p.id()).expect("hers");
    let told = ops::inbox(&beta.graph, alice, false).await?.first().map(|n| n.id()).expect("her question's notice");
    let read = ran_in_beta(&daemon, &["--as", "impl-alice", "read", &told.to_base64()])?;
    assert_eq!(read["read"], 1, "{read}");
    assert_eq!(beta.graph.get::<NotificationView>(told).await?.status()?, NotificationStatus::Read);
    Ok(())
}

/// Alpha's task made a step of beta's task by `link` run in alpha is alpha's relation, which a
/// command checks from either project: from alpha, where it runs; from beta, by the step's number
/// on beta's checklist, in alpha, saying so; and whoever checks it is the caller in alpha.
#[tokio::test]
async fn a_step_another_project_made_is_checked_from_either_project_where_its_relation_is() -> Result<()> {
    let Two { daemon, a, b, alpha, beta } = Two::new().await?;
    let edge: EntityId = daemon.made(&daemon.alpha, &["link", &a.cap, "step_of", &b.retry])?.parse()?;
    let person = ops::person_by_handle(&alpha.graph, "dnorman").await?;

    let (checked, said) = daemon.json_and_said(&daemon.alpha, &["step", "check", &b.retry, &a.cap])?;
    assert!(!said.contains("NOTE"), "the step's relation is alpha's, where the command started: {said}");
    assert_eq!((&checked["edge"], &checked["checked"]), (&json!(edge.to_base64()), &json!(true)), "{checked}");
    assert_eq!(beta.graph.relation(edge).await?.closed_by()?.map(EntityId::from), Some(person));

    let (unchecked, said) = daemon.json_and_said(&daemon.beta, &["step", "uncheck", &b.retry, "1"])?;
    let note = format!("NOTE: step `1` of `{}` is from project `alpha`, so this command ran there", b.retry);
    assert!(said.contains(&note), "{said}");
    assert_eq!((&unchecked["checked"], &unchecked["from_project"]), (&json!(false), &json!("alpha")), "{unchecked}");
    daemon.json(&daemon.beta, &["step", "check", &b.retry, &a.cap[..8]])?;
    let r = beta.graph.relation(edge).await?;
    assert_eq!((r.status()?, r.closed_by()?.map(EntityId::from)), (RelationStatus::Resolved, Some(person)));
    Ok(())
}

/// A scope of beta's, named in alpha by its id or the start of it, is a scope like any other: a
/// ruling of alpha's is attached to it, the command running in alpha, whose ruling it acts on;
/// given as the scope a command that acts on no one record works in, it runs that command in beta,
/// saying so, where the task it makes is beta's; and a command that acts on a record of alpha's
/// runs in alpha whatever scope it works in.
#[tokio::test]
async fn a_scope_of_the_other_project_is_followed_as_a_reference_and_runs_a_command_on_no_record_there() -> Result<()> {
    let Two { daemon, a, b, alpha, beta } = Two::new().await?;
    let (attached, said) = daemon.json_and_said(&daemon.alpha, &["decision", "attach", "D1", &b.scope[..8]])?;
    assert!(!said.contains("NOTE"), "{said}");
    assert_eq!((&attached["scope"], &attached["scope_title"]), (&json!(b.scope), &json!("Replication in beta")));
    assert!(attached.get("from_project").is_none(), "{attached}");

    let scoped = ["--scope", b.scope.as_str()];
    let (me, said) = daemon.json_and_said(&daemon.alpha, &[&scoped[..], &["whoami"]].concat())?;
    let note = format!("NOTE: the scope `{}` is from project `beta`, so this command ran there", b.scope);
    assert!(said.contains(&note), "{said}");
    assert_eq!((&me["project"]["name"], &me["scope"]), (&json!("beta"), &json!(b.scope)), "{me}");
    let before = records(&alpha.graph).await?.len();
    let made = daemon.made(&daemon.alpha, &[&scoped[..], &["task", "create", "Made in beta's scope"]].concat())?;
    assert!(beta.graph.get::<TaskView>(made.parse()?).await.is_ok(), "the task is beta's");
    assert_eq!(records(&alpha.graph).await?.len(), before, "alpha holds nothing more");

    let (shown, said) = daemon.json_and_said(&daemon.alpha, &[&scoped[..], &["task", "show", &a.cap]].concat())?;
    assert!(!said.contains("NOTE") && shown.get("from_project").is_none(), "{said} {shown}");
    Ok(())
}

/// A whole id typed as the scope a command works in names a scope or nothing: a task of either
/// project, the task the command starts among them, and an id the node holds nothing by are each
/// refused with the sentence that refuses any id naming no scope, where the scope places a command
/// that acts on no one record (`ask`), where it is read for a command its record placed (`task
/// start`), and where it is the scope a scope command acts on too (`scope show`), and neither
/// project changes: the task does not start, comes to be worked in nothing, and no question is
/// asked.
#[tokio::test]
async fn a_whole_id_that_names_no_scope_is_refused_as_the_scope_a_command_works_in_and_nothing_is_written() -> Result<()>
{
    let Two { daemon, a, b, alpha, beta } = Two::new().await?;
    let before = (records(&alpha.graph).await?, records(&beta.graph).await?);
    let nowhere = EntityId::random().to_base64();
    for not_a_scope in [a.cap.as_str(), b.cap.as_str(), nowhere.as_str()] {
        let starting = ["--scope", not_a_scope, "task", "start", a.cap.as_str()];
        let asking = ["--scope", not_a_scope, "ask", "who takes the TLS part?"];
        let showing = ["--scope", not_a_scope, "scope", "show", not_a_scope];
        for args in [&starting[..], &asking[..], &showing[..]] {
            let refused = daemon.output(&daemon.alpha, args)?.expect_err("no scope has that id");
            let refusal = format!("no scope has the id {not_a_scope}");
            assert!(refused.contains(&refusal), "fwd {}: {refused}", args.join(" "));
        }
    }
    let after = (records(&alpha.graph).await?, records(&beta.graph).await?);
    assert!(after == before, "nothing was written in either project");
    Ok(())
}

/// A `--project` typed on the command line is never overridden by a scope: given alpha by
/// `--project` and beta's scope by `--scope`, a command that acts on no one record runs in alpha,
/// saying nothing, and the scope is followed as a reference, so the task it makes is alpha's and
/// is worked in beta's scope. FWD_PROJECT, which the command line does not type, gives way to the
/// scope as the directory does.
#[tokio::test]
async fn a_project_typed_on_the_command_line_is_never_overridden_by_a_scope_of_another_project() -> Result<()> {
    let Two { daemon, a: _, b, alpha, beta } = Two::new().await?;
    let scoped = ["--project", "alpha", "--scope", b.scope.as_str()];
    let create = [&scoped[..], &["task", "create", "Made in alpha, worked in beta's scope"]].concat();
    let (made, said) = daemon.json_and_said(&daemon.beta, &create)?;
    assert!(!said.contains("NOTE") && made.get("from_project").is_none(), "{said} {made}");
    let task: EntityId = made["record"]["id"].as_str().ok_or_else(|| anyhow!("no id in {made}"))?.parse()?;
    assert!(alpha.graph.get::<TaskView>(task).await.is_ok(), "the task is alpha's");
    assert_eq!(ops::worked_in(&alpha.graph, task).await?, vec![b.scope.parse()?], "worked in beta's scope");
    let (me, _) = daemon.json_and_said(&daemon.beta, &[&scoped[..], &["whoami"]].concat())?;
    assert_eq!((&me["project"]["name"], &me["scope"]), (&json!("alpha"), &json!(b.scope)), "{me}");

    let by_env = [("FWD_PROJECT", "alpha")];
    let (me, said) = daemon.json_and_said_with(&daemon.beta, &["--scope", b.scope.as_str(), "whoami"], &by_env)?;
    let note = format!("NOTE: the scope `{}` is from project `beta`, so this command ran there", b.scope);
    assert!(said.contains(&note), "{said}");
    assert_eq!(me["project"]["name"], "beta", "FWD_PROJECT gives way to the scope: {me}");
    assert!(beta.graph.get::<TaskView>(task).await.is_err(), "beta holds nothing of it");
    Ok(())
}

/// The start of an id that a task of beta's has and a former id beta keeps for another of its
/// tasks share, typed in alpha, where nothing starts so, is refused with each task named with its
/// project.
#[tokio::test]
async fn a_start_two_of_the_other_projects_records_share_by_a_former_id_is_refused_naming_their_project() -> Result<()>
{
    let Two { daemon, a: _, b, alpha: _, beta } = Two::new().await?;
    let shared = &b.retry[..8];
    let cited = format!("{shared}{}", &EntityId::random().to_base64()[8..]);
    renumbered(&beta, &cited, b.cap.parse()?).await?;
    let refused = daemon.output(&daemon.alpha, &["task", "show", shared])?.expect_err("two of beta's tasks start so");
    assert!(refused.contains(&format!("`{shared}` starts the ids of 2 entities")), "{refused}");
    assert!(refused.contains(&format!("task {} in beta", &b.retry[..12])), "{refused}");
    assert!(refused.contains(&format!("(formerly {}) in beta", &cited[..12])), "{refused}");
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

/// A Codex thread calling the daemon's MCP endpoint, with the ids Codex attaches to each call, each
/// call starting in alpha.
struct Thread {
    client: rmcp::service::RunningService<rmcp::RoleClient, ()>,
    meta: rmcp::model::RequestMetaObject,
}

impl Thread {
    /// The thread `thread`, joined to the endpoint of the daemon listening on `port`.
    async fn on(port: u16, thread: &str) -> Result<Thread> {
        use rmcp::ServiceExt;
        let url = format!("http://127.0.0.1:{port}/mcp");
        let client = ().serve(rmcp::transport::StreamableHttpClientTransport::from_uri(url)).await?;
        let mut meta = rmcp::model::RequestMetaObject::new();
        for (key, value) in json!({
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
        Ok(Thread { client, meta })
    }

    /// What the tool `tool` answers given `args`, in alpha: its structured content, which must not
    /// be a refusal.
    async fn call(&self, tool: &str, args: Value) -> Result<Value> {
        let mut args = args.as_object().cloned().unwrap_or_default();
        args.insert("project".into(), json!("alpha"));
        let mut params = rmcp::model::CallToolRequestParams::new(tool.to_string()).with_arguments(args.clone());
        params.meta = Some(self.meta.clone());
        let answer = self.client.call_tool(params).await?;
        let given = answer.structured_content.unwrap_or_default();
        if answer.is_error == Some(true) {
            return Err(anyhow!("{tool} {} was refused: {given}", Value::Object(args)));
        }
        Ok(given)
    }
}

/// The daemon's MCP endpoint, called as a Codex thread in alpha, gives alpha's records alone from
/// every tool that reads, and given beta's record by its id runs in beta and says so.
#[tokio::test(flavor = "multi_thread")]
async fn the_mcp_read_tools_in_one_project_give_its_own_records_and_reach_the_others_by_id() -> Result<()> {
    let Two { daemon, a, b, alpha, beta } = Two::new().await?;
    daemon.made(&daemon.alpha, &["step", "add", &a.retry, &a.cap])?;
    let thread = Thread::on(alpha.pf.port, "019a0000-0000-7000-8000-0000000a1fa0").await?;
    let others = spellings(records(&beta.graph).await?.keys().copied());
    for (tool, args, own) in [
        ("task_list", json!({}), a.retry.as_str()),
        ("task_show", json!({"id": a.retry}), a.retry.as_str()),
        ("task_explain", json!({"id": a.retry}), a.cap.as_str()),
        ("step_list", json!({"task": a.retry}), a.cap.as_str()),
        ("relations", json!({"id": a.retry}), a.cap.as_str()),
        ("scope_list", json!({}), a.scope.as_str()),
        ("decision_list", json!({}), "D1"),
        ("decision_show", json!({"id": "D1"}), "Cap retries at 30s"),
        ("principal_list", json!({}), "impl-alice"),
        ("message_show", json!({"id": a.question}), a.question.as_str()),
    ] {
        let given = thread.call(tool, args).await?.to_string();
        prints_none_of(&others, &[tool], &given);
        prints_its_own(own, &[tool], &given);
    }
    for (tool, id) in [("task_show", &b.retry), ("message_show", &b.question)] {
        let given = thread.call(tool, json!({"id": id})).await?;
        assert_eq!((&given["result"]["id"], &given["result"]["from_project"]), (&json!(id), &json!("beta")), "{given}");
        let notes = given["notes"].to_string();
        assert!(notes.contains("is from project `beta`, so this command ran there"), "{notes}");
    }
    Ok(())
}

/// A call through the MCP endpoint, started in alpha, that acts on a record of beta's runs in beta
/// as the thread's principal there, made there by the call: a comment on beta's task, named by the
/// start of its id, is written in beta by that principal, and alpha holds nothing more, no
/// principal for the thread among it; a reply given the whole id of the notification of a question
/// beta asks that principal settles it there. A call that acts on no one record, given beta's
/// scope, runs in beta.
#[tokio::test(flavor = "multi_thread")]
async fn an_mcp_call_on_the_others_record_or_in_its_scope_runs_there_as_the_threads_principal_there() -> Result<()> {
    // The daemon is bound, not dropped: its scratch folder stops it when it goes.
    let Two { daemon: _daemon, a: _, b, alpha, beta } = Two::new().await?;
    let before = records(&alpha.graph).await?;
    let thread = Thread::on(alpha.pf.port, "019a0000-0000-7000-8000-0000000b2fb0").await?;

    let named = &b.retry[..8];
    let given = thread.call("comment", json!({"first": named, "text": "from a thread in alpha"})).await?;
    let note = format!("NOTE: `{named}` is from project `beta`, so this command ran there");
    assert!(given["notes"].to_string().contains(&note), "{given}");
    assert_eq!(given["result"]["from_project"], "beta", "{given}");
    let id: EntityId = given["result"]["record"]["id"].as_str().ok_or_else(|| anyhow!("no id in {given}"))?.parse()?;
    let author: EntityId = beta.graph.get::<MessageView>(id).await?.author()?.into();
    let bound = beta.graph.get::<PrincipalView>(author).await?.subject()?.unwrap_or_default();
    assert!(bound.starts_with("codex:") && bound.ends_with("0000000b2fb0"), "the thread's principal in beta: {bound}");
    assert_eq!(records(&alpha.graph).await?, before, "nothing of alpha's changed, and no principal was made there");

    let mut inbox = Inbox::of(&beta.graph, author).await?;
    let person = ops::person_by_handle(&beta.graph, "dnorman").await?;
    let to_thread = MessageContext { to: Some(author.to_base64()), ..Default::default() };
    let asked = ops::post_message(&beta.graph, person, "is it in yet?", Priority::Now, to_thread).await?;
    let told = inbox.told_of(&asked.to_base64()).await?;
    let given = thread.call("reply", json!({"id": told.to_base64(), "text": "not yet"})).await?;
    let note = format!("NOTE: `{}` is from project `beta`, so this command ran there", told.to_base64());
    assert!(given["notes"].to_string().contains(&note), "{given}");
    assert_eq!(
        (&given["result"]["settled"], &given["result"]["from_project"]),
        (&json!(told.to_base64()), &json!("beta"))
    );
    assert_eq!(beta.graph.get::<NotificationView>(told).await?.status()?, NotificationStatus::Read);

    let given = thread.call("task_list", json!({"scope": b.scope})).await?;
    let note = format!("NOTE: the scope `{}` is from project `beta`, so this command ran there", b.scope);
    assert!(given["notes"].to_string().contains(&note), "{given}");
    prints_its_own(&b.retry, &["task_list", "--scope"], &given["result"].to_string());
    Ok(())
}
