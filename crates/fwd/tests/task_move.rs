//! `fwd task move` against a daemon hosting two projects, alpha and beta, with its one routing
//! service running: the move tells only whom its comment tells, each once, since the records that
//! change project are updates the service knows, and the task's next status change is routed by
//! beta's rules; the command runs in the tasks' project and answers per task; one move runs at a
//! time, so two begun at once end as two run one after the other; and the MCP tool the CLI grammar
//! gives moves a task as the calling thread.

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
use fwd_core::task_move::MoveLock;
use serde_json::{json, Value};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tokio::sync::mpsc;

/// One daemon's folders, and a checkout of each of the two projects it hosts, which `dnorman`
/// made.
struct Daemon {
    scratch: Scratch,
    paths: Paths,
    alpha: PathBuf,
    beta: PathBuf,
}

impl Daemon {
    fn new() -> Result<Daemon> {
        let scratch = Scratch::new()?;
        let paths = scratch.paths("run", "data");
        let mut daemon = Daemon { paths, alpha: PathBuf::new(), beta: PathBuf::new(), scratch };
        daemon.alpha = daemon.project("alpha")?;
        daemon.beta = daemon.project("beta")?;
        Ok(daemon)
    }

    /// A checkout of a new project `name`, which `dnorman` made.
    fn project(&self, name: &str) -> Result<PathBuf> {
        let root = self.scratch.path().join(name);
        std::fs::create_dir_all(&root)?;
        git(&root, &["init", "-q", "-b", "main"])?;
        git(&root, &["remote", "add", "origin", &format!("git@example.com:t/{name}.git")])?;
        self.json(&root, &["init", "--name", name, "--handle", "dnorman"])?;
        Ok(root)
    }

    /// `fwd --format <format> <args>` in `checkout`, reaching this daemon only, with none of the
    /// caller's settings that would choose another project, principal or scope.
    fn fwd(&self, checkout: &Path, format: &str, args: &[&str]) -> Result<std::process::Output> {
        Ok(self.command(checkout, format, args).output()?)
    }

    /// `fwd --format json <args>` in `checkout`, started and left running: its standard output
    /// goes to a file of its own, `out`, and each line of its standard error is read as it comes,
    /// so that no pipe the test has not read yet holds it up.
    fn start(&self, checkout: &Path, args: &[&str], out: &str) -> Result<Running> {
        let out = self.scratch.path().join(out);
        let mut child = self
            .command(checkout, "json", args)
            .stdout(Stdio::from(std::fs::File::create(&out)?))
            .stderr(Stdio::piped())
            .spawn()?;
        let stderr = child.stderr.take().ok_or_else(|| anyhow!("no standard error"))?;
        let (tx, said) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Ok(Running { child, out, said })
    }

    /// The command `fwd --format <format> <args>` in `checkout`, as [`Daemon::fwd`] runs it.
    fn command(&self, checkout: &Path, format: &str, args: &[&str]) -> Command {
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
        command
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
/// as JSON and for people; run again, it reports the tasks as there already, in their scope, and
/// run into another of beta's scopes it says that a task there already keeps its scopes, and how to
/// change them. A move that names no scope takes a task into beta, which has an active scope, in
/// none; a project the node does not hold is refused with what to do, and so are the tasks given
/// before the flags.
#[tokio::test]
async fn the_command_runs_in_the_tasks_project_answers_per_task_and_reports_tasks_there_already() -> Result<()> {
    let daemon = Daemon::new()?;
    let (a, b) = (&daemon.alpha, &daemon.beta);
    let commons = daemon.made(b, &["scope", "create", "Commons"])?;
    let first = daemon.made(a, &["task", "create", "Write the catalog"])?;
    let second = daemon.made(a, &["task", "create", "Publish the catalog", "--after", &first])?;

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
    assert_eq!(tasks[1]["brought"], json!({"messages": 0, "relations": 1, "former_ids": 0}), "{moved}");
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
    let there = format!("{} \"Write the catalog\" is there already, in the scope Commons", &first[..8]);
    assert!(printed.contains(&there), "{printed}");
    let messages = ops::messages_about(&beta.graph, "task", first.parse()?).await?;
    assert_eq!(messages.len(), 1, "one comment records the move, and the second run wrote none");

    daemon.made(b, &["scope", "create", "Annex"])?;
    let (kept, said) =
        daemon.run(b, &["task", "move", "--to", "beta", "--to-scope", "Annex", &first])?.map_err(|e| anyhow!("{e}"))?;
    assert_eq!((&kept["tasks"][0]["moved"], &kept["tasks"][0]["kept_scopes"]), (&json!(false), &json!(true)), "{kept}");
    assert_eq!(kept["tasks"][0]["worked_in"][0]["id"], json!(commons), "{kept}");
    let note = format!(
        "fwd: {} \"Write the catalog\" was in beta already, and stays in the scope Commons: --to-scope `Annex` was not applied, since a move puts a task in a scope only as it arrives; `fwd link {} worked_in <scope>` adds a scope",
        &first[..8], &first[..8]
    );
    assert!(said.contains(&note), "{said}");

    let third = daemon.made(a, &["task", "create", "Into beta, in no scope"])?;
    let (unscoped, _) = daemon.run(a, &["task", "move", "--to", "beta", &third])?.map_err(|e| anyhow!("{e}"))?;
    assert_eq!(
        (&unscoped["to"]["scope"], &unscoped["tasks"][0]["worked_in"]),
        (&json!(null), &json!([])),
        "{unscoped}"
    );
    assert!(beta.graph.get::<TaskView>(third.parse()?).await.is_ok(), "the task is beta's, worked in no scope");

    let fourth = daemon.made(a, &["task", "create", "Stays in alpha"])?;
    let refused = daemon.run(a, &["task", "move", "--to", "gamma", &fourth])?.expect_err("no project gamma");
    assert!(refused.contains("the node holds no project `gamma`; it holds `alpha`, `beta`"), "{refused}");
    let refused = daemon.run(a, &["task", "move", &fourth, "--to", "beta"])?.expect_err("the task before the flags");
    assert!(
        refused.contains("`--to` comes before the tasks, which come last: an id may start with `-`, so every word after the first task is read as another task; give `fwd task move --to <project> [--to-scope <scope>] [--note <text>] <id>...`"),
        "{refused}"
    );
    let alpha = daemon.client(a).await?;
    assert!(alpha.graph.get::<TaskView>(fourth.parse()?).await.is_ok(), "the refusals moved nothing");
    Ok(())
}

/// What a running `fwd task move` says on standard error while another move holds the node's move
/// lock (`MOVE_WAITS` in crates/fwd/src/main.rs).
const WAITS: &str = "fwd: another task move is running on this node; this one waits for it to end";

/// A `fwd task move` left running ([`Daemon::start`]): the process, the file its standard output
/// goes to, and the lines of its standard error as they come.
struct Running {
    child: Child,
    out: PathBuf,
    said: std::sync::mpsc::Receiver<String>,
}

impl Running {
    /// Returns once the move has said that it waits for the lock. Fails when its standard error
    /// ends first, as it does when the move ran without waiting.
    fn until_waiting(&self) -> Result<()> {
        let mut read = Vec::new();
        for line in self.said.iter() {
            if line.starts_with(WAITS) {
                return Ok(());
            }
            read.push(line);
        }
        Err(anyhow!("the move ended without waiting for the lock: {}", read.join("\n")))
    }

    /// The JSON the move prints once it has ended, and what else it said; fails with what it said
    /// when it fails.
    fn ended(mut self) -> Result<(Value, String)> {
        let status = self.child.wait()?;
        let said: Vec<String> = self.said.iter().collect();
        if !status.success() {
            return Err(anyhow!("the move failed: {}", said.join("\n")));
        }
        Ok((serde_json::from_slice(&std::fs::read(&self.out)?)?, said.join("\n")))
    }
}

/// The messages about `task` and its relations of `kind`, wherever each is, through `graph`.
async fn about(graph: &ProjectGraph, task: EntityId) -> Result<(Vec<MessageView>, Vec<RelationView>)> {
    let messages = ops::messages_about(graph, "task", task).await?;
    let worked_in = ops::relations_from(graph, task, Some(RelationKind::WorkedIn)).await?;
    Ok((messages, worked_in))
}

/// A move waits while another holds the node's move lock, and writes nothing meanwhile: the test
/// holds the lock, the move says it waits, the task is still alpha's with nothing about it; once
/// the test lets the lock go, the move ends, having moved the task.
#[tokio::test]
async fn a_move_waits_for_the_one_that_holds_the_nodes_move_lock() -> Result<()> {
    let daemon = Daemon::new()?;
    let (a, b) = (&daemon.alpha, &daemon.beta);
    daemon.made(b, &["scope", "create", "Commons"])?;
    let task = daemon.made(a, &["task", "create", "Write the catalog"])?;
    let id: EntityId = task.parse()?;
    let alpha = daemon.client(a).await?;

    let held = MoveLock::take(&daemon.paths, || panic!("nothing else moves")).await?;
    let moving = daemon.start(b, &["task", "move", "--to", "beta", "--to-scope", "Commons", &task], "move.json")?;
    moving.until_waiting()?;
    let still: TaskView = alpha.graph.get(id).await?;
    assert!(alpha.graph.owns(&still), "the task is alpha's while the move waits");
    assert_eq!(about(&alpha.graph, id).await?.0.len(), 0, "and the move wrote no comment");
    drop(held);

    let (moved, _) = moving.ended()?;
    assert_eq!((&moved["tasks"][0]["moved"], &moved["tasks"][0]["from"]), (&json!(true), &json!("alpha")), "{moved}");
    let beta = daemon.client(b).await?;
    assert!(beta.graph.owns(&beta.graph.get::<TaskView>(id).await?));
    Ok(())
}

/// Two moves of one task to beta, begun while the test holds the lock and so waiting together,
/// end as two moves one after the other: one moves it, the other finds it there already, and the
/// task has one comment that records the move and one scope.
#[tokio::test]
async fn two_moves_of_a_task_to_one_project_at_once_move_it_once() -> Result<()> {
    let daemon = Daemon::new()?;
    let (a, b) = (&daemon.alpha, &daemon.beta);
    let commons = daemon.made(b, &["scope", "create", "Commons"])?;
    let task = daemon.made(a, &["task", "create", "Write the catalog"])?;
    let id: EntityId = task.parse()?;

    let held = MoveLock::take(&daemon.paths, || panic!("nothing else moves")).await?;
    let args = ["task", "move", "--to", "beta", "--to-scope", "Commons", &task];
    let (first, second) = (daemon.start(a, &args, "first.json")?, daemon.start(b, &args, "second.json")?);
    first.until_waiting()?;
    second.until_waiting()?;
    drop(held);
    let (one, _) = first.ended()?;
    let (other, _) = second.ended()?;

    let mut moved = [&one["tasks"][0]["moved"], &other["tasks"][0]["moved"]];
    moved.sort_by_key(|m| m.as_bool());
    assert_eq!(moved, [&json!(false), &json!(true)], "one moved it and one found it there: {one} {other}");
    let beta = daemon.client(b).await?;
    let (messages, worked_in) = about(&beta.graph, id).await?;
    assert_eq!(messages.len(), 1, "one comment records the move");
    let accepted: Vec<EntityId> = worked_in
        .iter()
        .filter(|r| r.status().ok() == Some(RelationStatus::Accepted))
        .map(|r| r.to_id())
        .collect::<Result<_, _>>()?;
    assert_eq!(accepted, [commons.parse::<EntityId>()?], "one scope, Commons");
    Ok(())
}

/// Two moves of one task, one to beta's scope and one to gamma's, begun while the test holds the
/// lock, end as two moves one after the other, in whichever order they took the lock: the second
/// runs where the first left the task, as the caller there, and moves it on, so the task ends in
/// one project with everything that belongs to it, two comments, one per move, and one scope, the
/// last destination's, the first's taken off with the move as the reason.
#[tokio::test]
async fn two_moves_of_a_task_to_two_projects_at_once_move_it_twice_in_turn() -> Result<()> {
    let daemon = Daemon::new()?;
    let (a, b) = (&daemon.alpha, &daemon.beta);
    let c = daemon.project("gamma")?;
    let inbox = daemon.made(a, &["scope", "create", "Inbox"])?;
    let commons = daemon.made(b, &["scope", "create", "Commons"])?;
    let annex = daemon.made(&c, &["scope", "create", "Annex"])?;
    let task = daemon.made(a, &["--scope", &inbox, "task", "create", "Write the catalog"])?;
    daemon.made(a, &["comment", &task, "the catalog first"])?;
    let id: EntityId = task.parse()?;

    let held = MoveLock::take(&daemon.paths, || panic!("nothing else moves")).await?;
    let to_beta = daemon.start(a, &["task", "move", "--to", "beta", "--to-scope", "Commons", &task], "beta.json")?;
    let to_gamma = daemon.start(a, &["task", "move", "--to", "gamma", "--to-scope", "Annex", &task], "gamma.json")?;
    to_beta.until_waiting()?;
    to_gamma.until_waiting()?;
    drop(held);
    let (into_beta, said_beta) = to_beta.ended()?;
    let (into_gamma, said_gamma) = to_gamma.ended()?;

    // The project the task ended in names the order: the move to it ran second, from the other.
    let (gamma, beta) = (daemon.client(&c).await?, daemon.client(b).await?);
    let in_gamma = gamma.graph.get::<TaskView>(id).await.is_ok();
    let (last, first, (second_answer, second_said), first_answer, last_scope, first_scope) = if in_gamma {
        (&gamma, "beta", (&into_gamma, &said_gamma), &into_beta, &annex, &commons)
    } else {
        (&beta, "gamma", (&into_beta, &said_beta), &into_gamma, &commons, &annex)
    };
    let last_name = if in_gamma { "gamma" } else { "beta" };
    assert_eq!(first_answer["tasks"][0]["from"], json!("alpha"), "{first_answer}");
    assert_eq!(
        (&second_answer["tasks"][0]["moved"], &second_answer["tasks"][0]["from"]),
        (&json!(true), &json!(first))
    );
    assert!(
        second_said.contains(&format!("NOTE: `{task}` is from project `{first}`, so this command ran there")),
        "{second_said}"
    );

    let (messages, worked_in) = about(&last.graph, id).await?;
    assert!(messages.iter().all(|m| last.graph.owns(m)), "every message is the last destination's");
    let mut said: Vec<String> = messages.iter().map(|m| ops::text(m.content())).collect::<Result<_>>()?;
    said.sort();
    let mut expected = vec![
        format!("Moved from alpha to {first}."),
        format!("Moved from {first} to {last_name}."),
        "the catalog first".to_string(),
    ];
    expected.sort();
    assert_eq!(said, expected, "two comments, one per move");
    let status = |scope: &str| -> Result<Vec<RelationStatus>> {
        let scope: EntityId = scope.parse()?;
        worked_in.iter().filter(|r| r.to_id().ok() == Some(scope)).map(|r| Ok(r.status()?)).collect()
    };
    assert_eq!(status(&inbox)?, [RelationStatus::Retracted]);
    assert_eq!(status(first_scope)?, [RelationStatus::Retracted], "the first destination's scope is taken off");
    assert_eq!(status(last_scope)?, [RelationStatus::Accepted]);
    assert!(worked_in.iter().all(|r| last.graph.owns(r)), "every relation from it is the last destination's");
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
