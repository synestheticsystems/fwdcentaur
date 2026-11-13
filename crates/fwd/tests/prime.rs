//! `fwd prime` as an agent gets it back when its context starts over: run by the agent itself, by
//! its session's hook, or by a skill as it loads, which names the session since nothing stamps its
//! command. Each test runs the CLI against a scratch daemon of its own, which hosts two projects,
//! alpha and beta, that the person `tester` made.

use ankurah::property::{Json, Ref};
use ankurah::EntityId;
use anyhow::{anyhow, Result};
use fwd_core::anchoring::git;
use fwd_core::daemon::{self, Autostart, Bind, Client, PortFile};
use fwd_core::model::*;
use fwd_core::naming;
use fwd_core::project::{NamedProject, Paths};
use fwd_core::route::{page_path, Route};
use fwd_core::rules::short_id;
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

/// The Claude Code session the agent here runs in, whose own thread is the agent.
const SESSION: &str = "0b5e55ed-1d1e-4c0a-9d2e-5ca1ab1e0001";

/// A scratch folder holding a daemon's runtime and data folders and a checkout of each of its two
/// projects. The daemon is stopped when the test ends.
struct Scratch {
    dir: tempfile::TempDir,
    alpha: PathBuf,
    beta: PathBuf,
}

impl Scratch {
    fn new() -> Result<Scratch> {
        let dir = tempfile::tempdir()?;
        let checkout = |name: &str| -> Result<PathBuf> {
            let root = dir.path().join(name);
            std::fs::create_dir_all(&root)?;
            git(&root, &["init", "-q", "-b", "main"])?;
            git(&root, &["remote", "add", "origin", &format!("git@example.com:t/{name}.git")])?;
            Ok(root)
        };
        let (alpha, beta) = (checkout("alpha")?, checkout("beta")?);
        let s = Scratch { dir, alpha, beta };
        s.json(&s.alpha, None, &["init", "--name", "alpha", "--handle", "tester"])?;
        s.json(&s.beta, None, &["init", "--name", "beta", "--handle", "tester"])?;
        Ok(s)
    }

    /// `fwd <args>` in `dir`, reaching this scratch daemon only, with none of the caller's settings
    /// that would choose another project, principal or agent; stamped, as the fwd plugin stamps a
    /// command, with the session's agent `agent` (`main` for its own thread) when one is given.
    fn command(&self, dir: &Path, agent: Option<&str>, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fwd"));
        command
            .current_dir(dir)
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
        if let Some(agent) = agent {
            command.env("FWD_CLAUDE_SESSION", SESSION).env("FWD_CLAUDE_AGENT", agent);
        }
        command.args(args);
        command
    }

    /// What `fwd <args>` prints, which must succeed.
    fn text(&self, dir: &Path, agent: Option<&str>, args: &[&str]) -> Result<String> {
        let out = self.command(dir, agent, args).output()?;
        if !out.status.success() {
            return Err(anyhow!("fwd {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr)));
        }
        Ok(String::from_utf8(out.stdout)?)
    }

    /// The JSON `fwd --format json <args>` prints, which must succeed.
    fn json(&self, dir: &Path, agent: Option<&str>, args: &[&str]) -> Result<Value> {
        Ok(serde_json::from_str(&self.text(dir, agent, &[&["--format", "json"][..], args].concat())?)?)
    }

    /// The id of what `fwd <args>`, a command that makes something, made.
    fn made(&self, dir: &Path, agent: Option<&str>, args: &[&str]) -> Result<String> {
        let made = self.json(dir, agent, args)?;
        Ok(made["record"]["id"].as_str().ok_or_else(|| anyhow!("no id under record in {made}"))?.to_string())
    }

    /// A client of the project whose checkout is `dir`, holding its graph, for what the CLI cannot
    /// make: records that share a creation time.
    async fn client(&self, dir: &Path) -> Result<Client> {
        let shown = self.json(dir, None, &["project", "show"])?;
        let project = NamedProject {
            id: shown["id"].as_str().ok_or_else(|| anyhow!("no id in {shown}"))?.parse()?,
            name: shown["name"].as_str().unwrap_or_default().to_string(),
        };
        let paths = Paths { runtime_dir: self.dir.path().join("run"), data_dir: self.dir.path().join("data") };
        daemon::ensure_daemon(&paths, &project, Autostart::Never).await
    }

    /// Returns once the principal the session's agent `agent` is bound to in `dir`'s project holds
    /// `count` unread notifications: `fwd watch` ends as the last of them arrives.
    fn told(&self, dir: &Path, agent: &str, count: usize) -> Result<()> {
        self.text(dir, Some(agent), &["--format", "json", "watch", "--count", &count.to_string(), "--timeout", "60"])
            .map(drop)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = self.command(self.dir.path(), None, &["daemon", "stop"]).output();
    }
}

/// What alpha holds for the session's own thread, `impl-sam`, once [`one_of_each`] has run.
struct Held {
    asked_of_it: String,
    it_asked: String,
    started: String,
    to_start: String,
    ready: String,
}

/// The session's own thread names itself `impl-sam` in alpha and holds one of each: a task the
/// person assigned to it and it started, and another assigned to it that it has not started, which
/// is ready; a question it asked the person, a question the person asked it, and the unread
/// notifications of the assignments and the question; a task no one holds is ready, its title on
/// two lines. The person's question is written last, so that once it has reached the agent routing
/// has done with all that came before it.
fn one_of_each(s: &Scratch) -> Result<Held> {
    s.json(&s.alpha, Some("main"), &["principal", "add", "impl-sam", "--role", "implementer"])?;
    let started = s.made(&s.alpha, None, &["task", "create", "Retry on disconnect"])?;
    s.json(&s.alpha, None, &["task", "assign", &started, "impl-sam"])?;
    s.json(&s.alpha, Some("main"), &["task", "start", &started])?;
    let to_start = s.made(&s.alpha, None, &["task", "create", "Log each retry"])?;
    s.json(&s.alpha, None, &["task", "assign", &to_start, "impl-sam"])?;
    let ready = s.made(&s.alpha, None, &["task", "create", "Cap the\nbackoff"])?;
    let it_asked = s.made(&s.alpha, Some("main"), &["ask", "--to", "tester", "should the primer name the page?"])?;
    let asked_of_it = s.made(&s.alpha, None, &["ask", "--to", "impl-sam", "does the retry cover the page too?"])?;
    s.told(&s.alpha, "main", 3)?;
    Ok(Held { asked_of_it, it_asked, started, to_start, ready })
}

/// The records of the section `section` of the principal `p` of a JSON answer.
fn records<'a>(p: &'a Value, section: &str) -> Vec<&'a Value> {
    p["sections"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| s["section"] == section)
        .flat_map(|s| s["records"].as_array().into_iter().flatten())
        .collect()
}

/// The ids of the records of `section` of the principal `p` of a JSON answer.
fn ids(p: &Value, section: &str) -> Vec<String> {
    records(p, section).iter().filter_map(|r| r["id"].as_str().map(str::to_string)).collect()
}

/// One read covers the agent in every project: bound to `impl-sam` in alpha and to `lead-sam` in
/// beta, the agent's `fwd prime` run in alpha lists both, alpha first, each with its project's id
/// and name and its own project's records, and the ready tasks of alpha alone, where it works.
#[test]
fn one_read_covers_each_principal_bound_to_the_agent_in_every_project() -> Result<()> {
    let s = Scratch::new()?;
    s.json(&s.alpha, Some("main"), &["principal", "add", "impl-sam", "--role", "implementer"])?;
    s.json(&s.beta, Some("main"), &["principal", "add", "lead-sam", "--role", "lead"])?;
    let ready_in_alpha = s.made(&s.alpha, None, &["task", "create", "Cap the backoff"])?;
    s.made(&s.beta, None, &["task", "create", "Write the primer"])?;
    let held_in_beta = s.made(&s.beta, None, &["task", "create", "Name the page"])?;
    s.json(&s.beta, None, &["task", "assign", &held_in_beta, "lead-sam"])?;

    let primed = s.json(&s.alpha, Some("main"), &["prime"])?;
    let principals = primed["principals"].as_array().ok_or_else(|| anyhow!("no principals in {primed}"))?;
    let named: Vec<(&str, &str, &str)> = principals
        .iter()
        .map(|p| {
            (
                p["project"]["name"].as_str().unwrap_or(""),
                p["name"].as_str().unwrap_or(""),
                p["role"].as_str().unwrap_or(""),
            )
        })
        .collect();
    assert_eq!(named, [("alpha", "impl-sam", "implementer"), ("beta", "lead-sam", "lead")], "{primed}");
    for (p, checkout) in principals.iter().zip([&s.alpha, &s.beta]) {
        assert_eq!(p["project"]["id"], s.json(checkout, None, &["project", "show"])?["id"], "{p}");
    }
    assert_eq!(ids(&principals[0], "ready"), [ready_in_alpha], "{primed}");
    assert_eq!(ids(&principals[1], "assigned"), [held_in_beta], "{primed}");
    let beta_sections: Vec<&str> =
        principals[1]["sections"].as_array().into_iter().flatten().filter_map(|s| s["section"].as_str()).collect();
    assert_eq!(
        beta_sections,
        ["asked_of_you", "you_asked", "assigned", "unread"],
        "no ready tasks where it does not work"
    );
    Ok(())
}

/// A skill names the session on the command, since nothing stamps a command it runs as it loads:
/// `--session` reads what the session's own thread holds, whoever runs it, a subagent's stamp
/// included, and writes nothing, and only `fwd prime` takes it. A session no principal is bound to
/// gets one line saying how its agent comes to hold work, exit 0, and in JSON a state that says so
/// with the same commands.
#[test]
fn a_session_named_on_the_command_reads_as_its_own_thread_whoever_runs_it_and_writes_nothing() -> Result<()> {
    let s = Scratch::new()?;
    one_of_each(&s)?;
    s.json(&s.alpha, Some("a1"), &["principal", "add", "impl-other", "--role", "implementer"])?;
    let stamped = s.json(&s.alpha, Some("main"), &["prime"])?;
    let before =
        (s.json(&s.alpha, None, &["principal", "list", "--all"])?, s.json(&s.alpha, Some("main"), &["inbox"])?);
    assert_eq!(s.json(&s.alpha, None, &["prime", "--session", SESSION])?, stamped);
    assert_eq!(s.json(&s.alpha, Some("a1"), &["prime", "--session", SESSION])?, stamped, "it wins over a stamp");
    let after = (s.json(&s.alpha, None, &["principal", "list", "--all"])?, s.json(&s.alpha, Some("main"), &["inbox"])?);
    assert_eq!(after, before, "nothing was written: no principal stamped, no notification read");

    for refused in [&["task", "create", "Leak", "--session", SESSION][..], &["inbox", "--session", SESSION]] {
        let out = s.command(&s.alpha, None, refused).output()?;
        let said = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success() && said.contains("unexpected argument '--session'"), "{refused:?}: {said}");
    }

    let outside = s.dir.path();
    let nobody = "0b5e55ed-1d1e-4c0a-9d2e-5ca1ab1e0002";
    let line = s.text(outside, None, &["prime", "--session", nobody, "--format", "text"])?;
    assert_eq!(
        line,
        format!("fwd: nothing to show: no principal is bound to the Claude Code agent claude-code:{nobody}/main in any project; name yourself with `fwd principal add <name> --role <role>`, or `fwd principal add <name> --succeeds <old name>` when this session carries on another's work\n")
    );
    let state = s.json(outside, None, &["prime", "--session", nobody])?;
    assert_eq!(state["state"], "unbound");
    assert_eq!(
        state["fix"],
        "fwd principal add <name> --role <role>, or fwd principal add <name> --succeeds <old name> when this session carries on another's work"
    );
    Ok(())
}

/// What the agent holds comes first, most important first, one line per record by its short id:
/// the question asked of it, the one it asked, the task it started and the one it is to start,
/// its unread notifications, then the ready task it could start, whose title on two lines is
/// printed on one; the task assigned to it is not listed again as ready.
#[test]
fn what_the_agent_holds_is_listed_most_important_first_one_line_each() -> Result<()> {
    let s = Scratch::new()?;
    let held = one_of_each(&s)?;
    let unread = s.json(&s.alpha, Some("main"), &["inbox", "--unread"])?;
    let notified: Vec<&str> = unread.as_array().into_iter().flatten().filter_map(|n| n["id"].as_str()).collect();
    assert_eq!(notified.len(), 3, "{unread}");
    let short = |id: &str| -> Result<String> { Ok(short_id(&id.parse::<EntityId>()?)) };

    let primed = s.text(&s.alpha, Some("main"), &["prime", "--format", "hook"])?;
    let lines: Vec<&str> = primed.lines().collect();
    let held_and_asked = [
        "fwdcentaur context for impl-sam (implementer) in alpha.".to_string(),
        "Questions asked of you (1):".into(),
        format!("{} from @tester: does the retry cover the page too?", short(&held.asked_of_it)?),
        "Questions you asked (1):".into(),
        format!("{} to @tester: should the primer name the page?", short(&held.it_asked)?),
        "Assigned to you (2):".into(),
        format!("{} Retry on disconnect [in_progress]", short(&held.started)?),
        format!("{} Log each retry [open]", short(&held.to_start)?),
        "Unread notifications (3):".into(),
    ];
    assert_eq!(lines[..9], held_and_asked, "{primed}");
    // The notifications' lines are the inbox's sentences, in the inbox's order.
    for (line, id) in lines[9..12].iter().zip(&notified) {
        assert!(line.starts_with(&format!("{} ", short(id)?)), "{primed}");
    }
    assert_eq!(
        lines[12..],
        [
            "Ready to start (1):".to_string(),
            format!("{} Cap the backoff", short(&held.ready)?),
            "Use `fwd task next --claim` to take work, `fwd inbox pop` to read notifications.".into(),
        ],
        "{primed}"
    );
    Ok(())
}

/// The text is the same byte for byte while what it shows is unchanged, so that a caller keeping
/// it where any change costs, a system prompt, pays once: two runs print the same, ready tasks and
/// notifications that share their time listed by id; and the sections that change only with what
/// the agent holds (`--include questions,assigned`) leave out the others, with the line on how to
/// go on, and print the same after a task is filed that changes the whole text.
#[tokio::test]
async fn the_text_is_the_same_byte_for_byte_while_what_it_shows_is_unchanged() -> Result<()> {
    let s = Scratch::new()?;
    one_of_each(&s)?;
    let client = s.client(&s.alpha).await?;
    let graph = &client.graph;
    let tester = naming::principal_named(graph, "tester", None).await?.ok_or_else(|| anyhow!("no tester"))?.id();
    let agent = naming::principal_named(graph, "impl-sam", None).await?.ok_or_else(|| anyhow!("no impl-sam"))?.id();
    let at = 1_790_000_000_000;
    let trx = graph.begin();
    let (mut tied_tasks, mut tied_notes) = (Vec::new(), Vec::new());
    for n in 0..6 {
        let task = Task {
            project: graph.project(),
            title: format!("Tied task {n}"),
            what: String::new(),
            why: String::new(),
            done_when: String::new(),
            status: TaskStatus::Open,
            needs_review: false,
            hold: None,
            not_before: None,
            assignee: None,
            priority: None,
            labels: Json::new(json!([])),
            context: Json::new(json!({})),
            created_by: Ref::new(tester),
            created: at,
            updated: at,
            closed: None,
            closed_reason: None,
        };
        tied_tasks.push(trx.create(&task).await?.id());
        let note = Notification {
            project: graph.project(),
            recipient: Ref::new(agent),
            notification_type: NotificationType::TaskProposed,
            status: NotificationStatus::Unread,
            priority: Priority::Later,
            created: at,
            updated: None,
            target: Json::new(json!({"type": "Task", "id": tied_tasks[n].to_base64()})),
            originator: Json::new(json!({})),
        };
        tied_notes.push(trx.create(&note).await?.id());
    }
    trx.commit().await?;
    drop(client);

    let whole = || s.text(&s.alpha, Some("main"), &["prime", "--format", "text"]);
    let first = whole()?;
    assert_eq!(whole()?, first);
    let listed: Vec<String> = first.lines().filter_map(|line| line.split(' ').next()).map(str::to_string).collect();
    for tied in [&mut tied_tasks, &mut tied_notes] {
        tied.sort();
        let shorts: Vec<String> = tied.iter().map(short_id).collect();
        let order: Vec<&String> = listed.iter().filter(|word| shorts.contains(word)).collect();
        assert_eq!(order, shorts.iter().collect::<Vec<_>>(), "listed by id: {first}");
    }

    let held = || s.text(&s.alpha, Some("main"), &["prime", "--format", "text", "--include", "questions,assigned"]);
    let held_before = held()?;
    for left_out in ["Unread notifications", "Ready to start", "Use `fwd"] {
        assert!(!held_before.contains(left_out), "{held_before}");
    }
    s.made(&s.alpha, None, &["task", "create", "Write the primer"])?;
    assert_ne!(whole()?, first, "a task filed changes the ready tasks");
    assert_eq!(held()?, held_before);
    Ok(())
}

/// The JSON holds what the text lists: each section with its records in the text's order, each
/// record with its whole id, its kind, its line's text and its path on the page, the address the
/// panel adds it to given once.
#[test]
fn the_json_holds_the_records_the_text_lists() -> Result<()> {
    let s = Scratch::new()?;
    one_of_each(&s)?;
    let text = s.text(&s.alpha, Some("main"), &["prime", "--format", "text"])?;
    let json = s.json(&s.alpha, Some("main"), &["prime"])?;
    let principal = &json["principals"][0];
    let alpha: uuid::Uuid = principal["project"]["id"].as_str().unwrap_or_default().parse()?;
    let mut as_lines = Vec::new();
    for section in principal["sections"].as_array().into_iter().flatten() {
        for r in section["records"].as_array().into_iter().flatten() {
            let id: EntityId = r["id"].as_str().unwrap_or_default().parse()?;
            as_lines.push(format!("{} {}", short_id(&id), r["title"].as_str().unwrap_or("")));
            let route = match r["kind"].as_str() {
                Some("task") => Some(page_path(alpha, &Route::task(id))),
                Some("ask") => Some(page_path(alpha, &Route::question(id))),
                _ => None,
            };
            assert_eq!(r["path"], json!(route), "{r}");
        }
    }
    let text_lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.ends_with(':') && !line.starts_with("fwdcentaur ") && !line.starts_with("Use "))
        .collect();
    assert_eq!(as_lines, text_lines, "{json}\n{text}");
    let sections: Vec<(&str, u64, u64)> = principal["sections"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| (s["section"].as_str().unwrap_or(""), s["shown"].as_u64().unwrap_or(9), s["of"].as_u64().unwrap_or(9)))
        .collect();
    assert_eq!(
        sections,
        [("asked_of_you", 1, 1), ("you_asked", 1, 1), ("assigned", 2, 2), ("unread", 3, 3), ("ready", 1, 1)]
    );
    assert!(json["page"].as_str().is_some_and(|page| page.starts_with("http://127.0.0.1:")), "{json}");
    Ok(())
}

/// Work moved to another project is still the agent's: a task assigned to it and the questions
/// about it, one asked of it and one it asked, moved from alpha to beta, are listed under its
/// principal of alpha, each naming beta on its line and in the JSON, which gives each a path there.
#[test]
fn work_moved_to_another_project_is_still_held_and_names_where_it_is() -> Result<()> {
    let s = Scratch::new()?;
    s.json(&s.alpha, Some("main"), &["principal", "add", "impl-sam", "--role", "implementer"])?;
    let task = s.made(&s.alpha, None, &["task", "create", "Retry on disconnect"])?;
    s.json(&s.alpha, None, &["task", "assign", &task, "impl-sam"])?;
    let it_asked = s.made(&s.alpha, Some("main"), &["ask", "--to", "tester", "--about", &task, "which backoff?"])?;
    let asked_of_it = s.made(&s.alpha, None, &["ask", "--to", "impl-sam", "--about", &task, "how long a wait?"])?;
    s.json(&s.alpha, None, &["task", "move", "--to", "beta", &task])?;

    let json = s.json(&s.alpha, Some("main"), &["prime"])?;
    let principal = &json["principals"][0];
    assert_eq!(principal["project"]["name"], "alpha", "{json}");
    let beta: uuid::Uuid = s.json(&s.beta, None, &["project", "show"])?["id"].as_str().unwrap_or_default().parse()?;
    for (section, id, route) in [
        ("assigned", &task, Route::task(task.parse()?)),
        ("asked_of_you", &asked_of_it, Route::question(asked_of_it.parse()?)),
        ("you_asked", &it_asked, Route::question(it_asked.parse()?)),
    ] {
        let listed = records(principal, section);
        assert_eq!(listed.len(), 1, "{section}: {json}");
        assert_eq!(listed[0]["id"], *id);
        assert_eq!(listed[0]["project"]["name"], "beta", "{section}: {json}");
        assert_eq!(listed[0]["path"], page_path(beta, &route));
    }
    let text = s.text(&s.alpha, Some("main"), &["prime", "--format", "text"])?;
    let line = format!("{} Retry on disconnect [open] in beta", short_id(&task.parse()?));
    assert!(text.lines().any(|l| l == line), "{text}");
    Ok(())
}

/// The hook of a session's own thread reads for the principal FWD_AS names only while no principal
/// of the project is bound to the thread: once the one bound to it has retired, it prints nothing,
/// as every other hook of a retired principal does, and not the context of the one FWD_AS names.
#[test]
fn a_retired_own_thread_gets_no_one_elses_context_from_fwd_as() -> Result<()> {
    let s = Scratch::new()?;
    s.json(&s.alpha, Some("main"), &["principal", "add", "lead-sam", "--role", "lead"])?;
    s.json(&s.alpha, Some("main"), &["principal", "retire", "lead-sam"])?;
    let mut hook = s.command(&s.alpha, None, &["prime", "--format", "hook"]);
    let mut child = hook.env("FWD_AS", "tester").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn()?;
    let input = json!({"session_id": SESSION, "hook_event_name": "SessionStart"}).to_string();
    child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?.write_all(input.as_bytes())?;
    let out = child.wait_with_output()?;
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8(out.stdout)?, "");
    Ok(())
}

/// `child`'s output, once it exits within `limit`; an error, the child killed, when it does not,
/// so that a deadline that fails to hold fails the test rather than hanging it.
fn exited_within(mut child: Child, limit: Duration) -> Result<(Output, Duration)> {
    let began = Instant::now();
    let pid = child.id();
    let (done, exited) = std::sync::mpsc::channel();
    let stdout = child.stdout.take();
    std::thread::spawn(move || {
        let mut printed = Vec::new();
        if let Some(mut out) = stdout {
            let _ = std::io::Read::read_to_end(&mut out, &mut printed);
        }
        let _ = done.send((child.wait_with_output(), printed));
    });
    match exited.recv_timeout(limit) {
        Ok((out, printed)) => Ok((Output { stdout: printed, ..out? }, began.elapsed())),
        Err(_) => {
            let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
            Err(anyhow!("fwd prime had not exited {}s after it began", limit.as_secs()))
        }
    }
}

/// Given `--timeout`, nothing outlasts the deadline: a daemon that holds its lock and whose port
/// takes requests and never answers, as the person's machine once had, and a hook's standard input
/// that never closes, end the command by the deadline with exit 0 and one line saying why there
/// is nothing to show, as JSON with `--format json`; with no daemon and none to start, at once,
/// naming that; and a reader of its output gone by then costs it no failure.
#[test]
fn with_a_deadline_nothing_outlasts_it_and_a_reader_gone_costs_no_failure() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let paths = Paths { runtime_dir: dir.path().join("run"), data_dir: dir.path().join("data") };
    std::fs::create_dir_all(&paths.runtime_dir)?;
    let fwd = |args: &[&str]| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fwd"));
        command
            .current_dir(dir.path())
            .env("FWD_RUNTIME_DIR", &paths.runtime_dir)
            .env("FWD_DATA_DIR", &paths.data_dir)
            .env("FWD_DEV_RUNNER_DIR", dir.path().join("dev-runner"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .args(args);
        for name in ["FWD_PROJECT", "FWD_AS", "FWD_SCOPE", "FWD_DAEMON_BIN", "FWD_CLAUDE_SESSION", "FWD_CLAUDE_AGENT"] {
            command.env_remove(name);
        }
        command
    };
    // Without the deadline a command waits ten seconds for a daemon to answer, then fails.
    let limit = Duration::from_secs(20);

    let (out, _) = exited_within(
        fwd(&["--no-autostart", "prime", "--session", SESSION, "--timeout", "5", "--format", "text"]).spawn()?,
        limit,
    )?;
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8(out.stdout)?, "fwd: nothing to show: no daemon is running and autostart is off\n");

    let silent = std::net::TcpListener::bind("127.0.0.1:0")?;
    let held = daemon::try_lock(&paths.lifetime_lock())?.ok_or_else(|| anyhow!("the lifetime lock is taken"))?;
    let pf = PortFile {
        port: silent.local_addr()?.port(),
        pid: std::process::id(),
        instance_id: "silent".into(),
        started: 0,
        bind: Bind::Loopback,
    };
    daemon::write_port_file(&paths.port_file(), &pf)?;
    for (format, said) in [
        ("text", "fwd: nothing to show: it did not finish within 2s\n".to_string()),
        ("json", format!("{}\n", json!({"state": "unavailable", "reason": "it did not finish within 2s"}))),
    ] {
        let (out, took) =
            exited_within(fwd(&["prime", "--session", SESSION, "--timeout", "2", "--format", format]).spawn()?, limit)?;
        assert_eq!(out.status.code(), Some(0), "{format}");
        assert_eq!(String::from_utf8(out.stdout)?, said);
        assert!(took < Duration::from_secs(6), "{format}: it ended {took:?} after it began");
    }
    drop(held);

    // A hook's command reads its standard input before anything else, and here it never closes.
    let mut waiting = fwd(&["prime", "--format", "hook", "--timeout", "1"]).stdin(Stdio::piped()).spawn()?;
    let open = waiting.stdin.take();
    let (out, took) = exited_within(waiting, limit)?;
    assert_eq!(
        (out.status.code(), String::from_utf8(out.stdout)?.as_str()),
        (Some(0), "fwd: nothing to show: it did not finish within 1s\n")
    );
    assert!(took < Duration::from_secs(5), "it ended {took:?} after it began");
    // And its reader is gone by the time it says so.
    let mut unread = fwd(&["prime", "--format", "hook", "--timeout", "1"]).stdin(Stdio::piped()).spawn()?;
    let still_open = unread.stdin.take();
    drop(unread.stdout.take());
    let (out, _) = exited_within(unread, limit)?;
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    drop((open, still_open));
    Ok(())
}
