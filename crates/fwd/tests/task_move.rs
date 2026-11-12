//! `fwd task move` against a daemon hosting two projects, alpha and beta, with its one routing
//! service running: the move tells only whom its comment tells, each once, since the records that
//! change project are updates the service knows, and the task's next status change is routed by
//! beta's rules; the command runs in the tasks' project and answers per task; and the MCP tool
//! the CLI grammar gives moves a task as the calling thread.

mod common;

use ankurah::changes::ChangeSet;
use ankurah::signals::Subscribe;
use ankurah::EntityId;
use anyhow::{anyhow, Result};
use common::Scratch;
use fwd_core::anchoring::git;
use fwd_core::daemon::{self, Autostart, Client};
use fwd_core::model::*;
use fwd_core::naming;
use fwd_core::ops;
use fwd_core::project::{NamedProject, Paths};
use fwd_core::project_graph::ProjectGraph;
use fwd_core::query::with_id;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use tokio::sync::mpsc;

/// One daemon's folders, and a checkout of each of the two projects it hosts, which `dnorman`
/// made.
struct Daemon {
    _scratch: Scratch,
    paths: Paths,
    alpha: PathBuf,
    beta: PathBuf,
}

impl Daemon {
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
        Ok(command.output()?)
    }

    /// The JSON `fwd <args>` prints in `checkout`, and what it said on standard error beside it;
    /// what it said alone when it fails.
    fn run(&self, checkout: &Path, args: &[&str]) -> Result<std::result::Result<(Value, String), String>> {
        let out = self.fwd(checkout, "json", args)?;
        let said = String::from_utf8_lossy(&out.stderr).into_owned();
        Ok(if out.status.success() { Ok((serde_json::from_slice(&out.stdout)?, said)) } else { Err(said) })
    }

    /// The JSON `fwd <args>` prints in `checkout`, which must succeed.
    fn json(&self, checkout: &Path, args: &[&str]) -> Result<Value> {
        Ok(self.run(checkout, args)?.map_err(|e| anyhow!("fwd {} failed: {e}", args.join(" ")))?.0)
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

/// The principal of `graph`'s project named `name`.
async fn named(graph: &ProjectGraph, name: &str) -> Result<EntityId> {
    naming::principal_named(graph, name, None).await?.map(|p| p.id()).ok_or_else(|| anyhow!("no {name}"))
}

/// The notifications of one recipient as they arrive, from a live query on its project's graph.
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

    /// Returns once a notification of type `t` has arrived, about the message `message` when one
    /// is given. Fails when a minute passes first: the minute only detects a notification that
    /// never comes, and what orders the test is its arrival.
    async fn until(&mut self, t: NotificationType, message: Option<&str>) -> Result<()> {
        let arrived = async {
            loop {
                let n = self.arrived.recv().await.ok_or_else(|| anyhow!("the inbox's query ended"))?;
                let about = Value::from(n.originator()?)["message_id"].as_str().map(str::to_string);
                if n.notification_type()? == t && message.is_none_or(|m| about.as_deref() == Some(m)) {
                    return Ok(());
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(60), arrived)
            .await
            .map_err(|_| anyhow!("no {t:?} arrived within a minute"))?
    }
}

/// Each notification `who` holds in `graph`'s project, by its id, with its type and the message it
/// is about.
async fn held(graph: &ProjectGraph, who: EntityId) -> Result<Vec<(EntityId, NotificationType, Option<String>)>> {
    let mut out = Vec::new();
    for n in ops::inbox(graph, who, false).await? {
        let about = Value::from(n.originator()?)["message_id"].as_str().map(str::to_string);
        out.push((n.id(), n.notification_type()?, about));
    }
    Ok(out)
}

/// What `who` holds now that it did not hold in `before`, by type and message.
async fn since(
    graph: &ProjectGraph,
    who: EntityId,
    before: &[(EntityId, NotificationType, Option<String>)],
) -> Result<Vec<(NotificationType, Option<String>)>> {
    let mut new: Vec<(NotificationType, Option<String>)> = held(graph, who)
        .await?
        .into_iter()
        .filter(|(id, ..)| !before.iter().any(|(old, ..)| old == id))
        .map(|(_, t, about)| (t, about))
        .collect();
    new.sort();
    Ok(new)
}

/// alpha's task, assigned to its implementer, started in alpha's scope `Inbox`, needing review,
/// with a comment that names the implementer; alpha's triage agent serves alpha whole, and beta's
/// serves beta's scope `Commons`. The move to `Commons` tells only whom its comment tells, each
/// once, in their own projects: the implementer, the task's assignee, in alpha, and beta's triage
/// agent of `Commons`, in beta. Nothing the task held before is told again, though its comment's
/// mention and the rest changed project. The task's submission, its next status change, asks
/// beta's triage agent of `Commons` for the review, by beta's rules, once, and alpha's no one.
#[tokio::test]
async fn the_move_tells_only_whom_its_comment_tells_and_the_next_change_is_routed_by_the_destinations_rules(
) -> Result<()> {
    let daemon = Daemon::new()?;
    let (a, b) = (&daemon.alpha, &daemon.beta);
    daemon.json(a, &["--as", "impl-alice", "principal", "add", "impl-alice", "--role", "implementer"])?;
    daemon
        .json(a, &["--as", "triage-alpha", "principal", "add", "triage-alpha", "--role", "triage", "--project-wide"])?;
    let inbox = daemon.made(a, &["scope", "create", "Inbox"])?;
    let commons = daemon.made(b, &["scope", "create", "Commons"])?;
    daemon.json(
        b,
        &["--scope", &commons, "--as", "triage-beta", "principal", "add", "triage-beta", "--role", "triage"],
    )?;
    let task = daemon.made(a, &["--scope", &inbox, "task", "create", "Write the catalog", "--needs-review"])?;
    daemon.json(a, &["task", "assign", &task, "impl-alice"])?;
    daemon.json(a, &["--as", "impl-alice", "--scope", &inbox, "task", "start", &task])?;
    daemon.made(a, &["comment", &task, "@impl-alice, the catalog first"])?;
    let (alpha, beta) = (daemon.client(a).await?, daemon.client(b).await?);
    let (alice, triage_alpha) = (named(&alpha.graph, "impl-alice").await?, named(&alpha.graph, "triage-alpha").await?);
    let triage_beta = named(&beta.graph, "triage-beta").await?;
    let (mut hers, mut theirs) = (Inbox::of(&alpha.graph, alice)?, Inbox::of(&beta.graph, triage_beta)?);
    // Once a question asked last has reached her, everything written before it has been routed.
    let settled = daemon.made(a, &["ask", "--to", "impl-alice", "settled?"])?;
    hers.until(NotificationType::QuestionAsked, Some(&settled)).await?;
    let before = [
        held(&alpha.graph, alice).await?,
        held(&alpha.graph, triage_alpha).await?,
        held(&beta.graph, triage_beta).await?,
    ];

    let moved =
        daemon.json(a, &["task", "move", "--to", "beta", "--to-scope", "Commons", "--note", "beta's work", &task])?;
    let comment = moved["tasks"][0]["comment"].as_str().ok_or_else(|| anyhow!("no comment in {moved}"))?.to_string();
    hers.until(NotificationType::TaskCommented, Some(&comment)).await?;
    theirs.until(NotificationType::TaskCommented, Some(&comment)).await?;
    let after = daemon.made(a, &["ask", "--to", "impl-alice", "after the move?"])?;
    hers.until(NotificationType::QuestionAsked, Some(&after)).await?;

    let commented = (NotificationType::TaskCommented, Some(comment.clone()));
    let mut asked = vec![commented.clone(), (NotificationType::QuestionAsked, Some(after))];
    asked.sort();
    assert_eq!(since(&alpha.graph, alice, &before[0]).await?, asked, "her: the move's comment, once");
    assert_eq!(since(&alpha.graph, triage_alpha, &before[1]).await?, vec![], "alpha's triage agent: nothing");
    assert_eq!(since(&beta.graph, triage_beta, &before[2]).await?, vec![commented.clone()], "beta's: the comment once");

    daemon.json(a, &["task", "submit", &task, "--evidence", "the catalog is written"])?;
    theirs.until(NotificationType::ReviewRequested, None).await?;
    let last = daemon.made(b, &["ask", "--to", "triage-beta", "after the review?"])?;
    theirs.until(NotificationType::QuestionAsked, Some(&last)).await?;
    let mut told =
        vec![commented, (NotificationType::ReviewRequested, None), (NotificationType::QuestionAsked, Some(last))];
    told.sort();
    assert_eq!(
        since(&beta.graph, triage_beta, &before[2]).await?,
        told,
        "the review asked of beta's triage agent once"
    );
    assert_eq!(since(&alpha.graph, triage_alpha, &before[1]).await?, vec![], "and of alpha's never");
    Ok(())
}

/// `fwd task move`, typed in beta's checkout with alpha's tasks, runs in alpha, where the tasks
/// are, and says so; it answers per task what it moved, from which project and into which scope,
/// as JSON and for people; run again, it reports the tasks as there already; no scope for a project
/// that has an active one, and a project the node does not hold, are refused with what to do.
#[tokio::test]
async fn the_command_runs_in_the_tasks_project_answers_per_task_and_reports_tasks_there_already() -> Result<()> {
    let daemon = Daemon::new()?;
    let (a, b) = (&daemon.alpha, &daemon.beta);
    let commons = daemon.made(b, &["scope", "create", "Commons"])?;
    let first = daemon.made(a, &["task", "create", "Write the catalog"])?;
    let second = daemon.made(a, &["task", "create", "Publish the catalog", "--after", &first])?;
    let beta_s = daemon.made(b, &["task", "create", "Beta's own"])?;

    let args = ["task", "move", "--to", "beta", "--to-scope", &commons[..8], &first[..8], &second];
    let (moved, said) = daemon.run(b, &args)?.map_err(|e| anyhow!("{e}"))?;
    assert!(
        said.contains(&format!("NOTE: `{}` is from project `alpha`, so this command ran there", &first[..8])),
        "{said}"
    );
    assert_eq!(
        (&moved["to"]["project"], &moved["to"]["scope"]["title"]),
        (&json!("beta"), &json!("Commons")),
        "{moved}"
    );
    let tasks = moved["tasks"].as_array().ok_or_else(|| anyhow!("no tasks in {moved}"))?;
    assert_eq!(tasks.len(), 2);
    assert_eq!(
        (&tasks[0]["id"], &tasks[0]["from"], &tasks[0]["moved"]),
        (&json!(first), &json!("alpha"), &json!(true))
    );
    assert_eq!(tasks[1]["brought"], json!({"messages": 0, "relations": 1, "steps": 0, "former_ids": 0}), "{moved}");
    assert_eq!(tasks[1]["worked_in"][0]["id"], json!(commons));
    let beta = daemon.client(b).await?;
    assert!(
        beta.graph.get::<TaskView>(first.parse()?).await.is_ok()
            && beta.graph.get::<TaskView>(second.parse()?).await.is_ok()
    );

    let out = daemon.fwd(b, "text", &["task", "move", "--to", "beta", "--to-scope", "Commons", &first, &second])?;
    let printed = String::from_utf8(out.stdout)?;
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(printed.starts_with("To beta, in the scope Commons:"), "{printed}");
    assert!(printed.contains(&format!("{} \"Write the catalog\" is there already", &first[..8])), "{printed}");
    let messages = ops::messages_about(&beta.graph, "task", first.parse()?).await?;
    assert_eq!(messages.len(), 1, "one comment records the move, and the second run wrote none");

    let third = daemon.made(a, &["task", "create", "Stays in alpha"])?;
    let refused = daemon.run(a, &["task", "move", "--to", "beta", &third, &beta_s])?.expect_err("beta has a scope");
    let what = format!(
        "beta has active scopes (`Commons` {}), so a task moves into one of them: name it with --to-scope",
        &commons[..8]
    );
    assert!(refused.contains(&what), "{refused}");
    let refused = daemon.run(a, &["task", "move", "--to", "gamma", &third])?.expect_err("no project gamma");
    assert!(refused.contains("the node holds no project `gamma`; it holds `alpha`, `beta`"), "{refused}");
    let alpha = daemon.client(a).await?;
    assert!(alpha.graph.get::<TaskView>(third.parse()?).await.is_ok(), "the refusals moved nothing");
    Ok(())
}

/// The MCP tool `task_move`, which the CLI grammar gives, moves a task of alpha's to beta's scope
/// as the calling thread's principal in alpha, where the task is.
#[tokio::test(flavor = "multi_thread")]
async fn the_mcp_tool_moves_a_task_as_the_calling_threads_principal_where_the_task_is() -> Result<()> {
    use rmcp::ServiceExt;
    let daemon = Daemon::new()?;
    let (a, b) = (&daemon.alpha, &daemon.beta);
    let commons = daemon.made(b, &["scope", "create", "Commons"])?;
    let task = daemon.made(a, &["task", "create", "Write the catalog"])?;
    let (alpha, beta) = (daemon.client(a).await?, daemon.client(b).await?);
    let url = format!("http://127.0.0.1:{}/mcp", alpha.pf.port);
    let client = ().serve(rmcp::transport::StreamableHttpClientTransport::from_uri(url)).await?;
    let thread = "019a0000-0000-7000-8000-00000000a0e1";
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
    let args = json!({"project": "beta", "ids": [task], "to": "beta", "to_scope": commons});
    let mut params = rmcp::model::CallToolRequestParams::new("task_move")
        .with_arguments(args.as_object().cloned().unwrap_or_default());
    params.meta = Some(meta);
    let answer = client.call_tool(params).await?;
    let given = answer.structured_content.unwrap_or_default();
    assert_ne!(answer.is_error, Some(true), "{given}");
    assert_eq!(
        (&given["result"]["tasks"][0]["moved"], &given["result"]["from_project"]),
        (&json!(true), &json!("alpha")),
        "{given}"
    );
    assert!(beta.graph.get::<TaskView>(task.parse()?).await.is_ok(), "the task is beta's");
    let comment: EntityId =
        given["result"]["tasks"][0]["comment"].as_str().ok_or_else(|| anyhow!("{given}"))?.parse()?;
    let mover: EntityId = beta.graph.get::<MessageView>(comment).await?.author()?.into();
    let bound = alpha.graph.get::<PrincipalView>(mover).await?.subject()?.unwrap_or_default();
    assert!(bound.ends_with(thread), "the comment is by the thread's principal in alpha: {bound}");
    Ok(())
}
