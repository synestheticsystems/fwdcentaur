//! `fwd prime` as an agent gets it back when its context starts over: run by the agent itself, by
//! its session's hook, or by a skill as it loads, which names the session since nothing stamps its
//! command. Each test runs the CLI against a scratch daemon of its own, which hosts two projects,
//! alpha and beta, that the person `tester` made.

use anyhow::{anyhow, Result};
use fwd_core::anchoring::git;
use fwd_core::daemon::{self, Bind, PortFile};
use fwd_core::project::Paths;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
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
    ready: String,
}

/// The session's own thread names itself `impl-sam` in alpha and holds one of each: a task the
/// person assigned to it and it started, a question it asked the person, a question the person
/// asked it, and the two unread notifications of the assignment and the question; a task no one
/// holds is ready. The person's question is written last, so that once it has reached the agent
/// routing has done with all that came before it.
fn one_of_each(s: &Scratch) -> Result<Held> {
    s.json(&s.alpha, Some("main"), &["principal", "add", "impl-sam", "--role", "implementer"])?;
    let started = s.made(&s.alpha, None, &["task", "create", "Retry on disconnect"])?;
    s.json(&s.alpha, None, &["task", "assign", &started, "impl-sam"])?;
    s.json(&s.alpha, Some("main"), &["task", "start", &started])?;
    let ready = s.made(&s.alpha, None, &["task", "create", "Cap the backoff"])?;
    let it_asked = s.made(&s.alpha, Some("main"), &["ask", "--to", "tester", "should the primer name the page?"])?;
    let asked_of_it = s.made(&s.alpha, None, &["ask", "--to", "impl-sam", "does the retry cover the page too?"])?;
    s.told(&s.alpha, "main", 2)?;
    Ok(Held { asked_of_it, it_asked, started, ready })
}

/// The lines of `text` that list a record, each starting with the record's id.
fn record_lines(text: &str) -> Vec<&str> {
    text.lines().filter(|line| line.split(' ').next().is_some_and(|word| word.len() == 43)).collect()
}

/// One read covers the agent in every project: bound to `impl-sam` in alpha and to `lead-sam` in
/// beta, the agent's `fwd prime` lists both, the project it runs in first, each with its project's
/// id and name and its own project's records alone.
#[test]
fn one_read_covers_each_principal_bound_to_the_agent_in_every_project() -> Result<()> {
    let s = Scratch::new()?;
    s.json(&s.alpha, Some("main"), &["principal", "add", "impl-sam", "--role", "implementer"])?;
    s.json(&s.beta, Some("main"), &["principal", "add", "lead-sam", "--role", "lead"])?;
    let in_alpha = s.made(&s.alpha, None, &["task", "create", "Cap the backoff"])?;
    let in_beta = s.made(&s.beta, None, &["task", "create", "Write the primer"])?;

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
    let ids = |p: &Value| -> Vec<String> {
        p["records"].as_array().into_iter().flatten().filter_map(|r| r["id"].as_str().map(str::to_string)).collect()
    };
    assert_eq!(ids(&principals[0]), [in_alpha], "{primed}");
    assert_eq!(ids(&principals[1]), [in_beta], "{primed}");
    Ok(())
}

/// A skill names the session on the command, since nothing stamps a command it runs as it loads:
/// from any directory, `--session` reads what the session's own thread holds, as its stamped
/// command does, and only `fwd prime` takes it, so that no other command reads or writes as that
/// agent through it. A session no principal is bound to gets one line saying how its agent comes
/// to hold work, exit 0, and in JSON a state that says so with the same commands.
#[test]
fn a_session_named_on_the_command_reads_as_its_own_thread_and_nothing_else_takes_it() -> Result<()> {
    let s = Scratch::new()?;
    one_of_each(&s)?;
    let outside = s.dir.path();
    let stamped = s.json(&s.alpha, Some("main"), &["prime"])?;
    let named = s.json(outside, None, &["prime", "--session", SESSION])?;
    assert_eq!(named, stamped);

    for refused in [&["task", "create", "Leak", "--session", SESSION][..], &["inbox", "--session", SESSION]] {
        let out = s.command(&s.alpha, None, refused).output()?;
        let said = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success() && said.contains("unexpected argument '--session'"), "{refused:?}: {said}");
    }
    assert_eq!(s.json(&s.alpha, None, &["task", "list"])?.as_array().map(Vec::len), Some(2), "nothing was made");

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

/// What the agent holds comes first, most important first, one line per record by its id: the
/// question asked of it, the one it asked, the task it started, its unread notifications, then the
/// ready task it could start.
#[test]
fn what_the_agent_holds_is_listed_most_important_first_one_line_each() -> Result<()> {
    let s = Scratch::new()?;
    let held = one_of_each(&s)?;
    let unread = s.json(&s.alpha, Some("main"), &["inbox", "--unread"])?;
    let notified: Vec<&str> = unread.as_array().into_iter().flatten().filter_map(|n| n["id"].as_str()).collect();
    assert_eq!(notified.len(), 2, "{unread}");

    let primed = s.text(&s.alpha, Some("main"), &["prime", "--format", "hook"])?;
    let lines: Vec<&str> = primed.lines().collect();
    let held_and_asked = [
        "fwdcentaur context for impl-sam (implementer) in alpha.".to_string(),
        "Questions asked of you (1):".into(),
        format!("{} from @tester: does the retry cover the page too?", held.asked_of_it),
        "Questions you asked (1):".into(),
        format!("{} to @tester: should the primer name the page?", held.it_asked),
        "Assigned to you (1):".into(),
        format!("{} Retry on disconnect [in_progress]", held.started),
        "Unread notifications (2):".into(),
    ];
    assert_eq!(lines[..8], held_and_asked, "{primed}");
    // The notifications' lines are the inbox's sentences, in the inbox's order.
    for (line, id) in lines[8..10].iter().zip(&notified) {
        assert!(line.starts_with(&format!("{id} ")), "{primed}");
    }
    assert_eq!(
        lines[10..],
        [
            "Ready to start (1):".to_string(),
            format!("{} Cap the backoff", held.ready),
            "Use `fwd task next --claim` to take work, `fwd inbox pop` to read notifications.".into(),
        ],
        "{primed}"
    );
    Ok(())
}

/// The text is the same byte for byte while what it shows is unchanged, so that a caller keeping
/// it where any change costs, a system prompt, pays once: two runs print the same, and the
/// sections that change only with what the agent holds (`--include questions,assigned`) leave out
/// the others and print the same after a task is filed that changes the whole text.
#[test]
fn the_text_is_the_same_byte_for_byte_while_what_it_shows_is_unchanged() -> Result<()> {
    let s = Scratch::new()?;
    one_of_each(&s)?;
    let whole = || s.text(&s.alpha, Some("main"), &["prime", "--format", "text"]);
    let held = || s.text(&s.alpha, Some("main"), &["prime", "--format", "text", "--include", "questions,assigned"]);
    let (first, held_before) = (whole()?, held()?);
    assert_eq!(whole()?, first);
    assert!(!held_before.contains("Unread notifications") && !held_before.contains("Ready to start"), "{held_before}");
    s.made(&s.alpha, None, &["task", "create", "Write the primer"])?;
    assert_ne!(whole()?, first, "a task filed changes the ready tasks");
    assert_eq!(held()?, held_before);
    Ok(())
}

/// The JSON holds the records the text lists, in the text's order, each with its id, its kind and
/// its line's text, and for each section how many it shows of how many.
#[test]
fn the_json_holds_the_records_the_text_lists() -> Result<()> {
    let s = Scratch::new()?;
    one_of_each(&s)?;
    let text = s.text(&s.alpha, Some("main"), &["prime", "--format", "text"])?;
    let json = s.json(&s.alpha, Some("main"), &["prime"])?;
    let principal = &json["principals"][0];
    let records = principal["records"].as_array().ok_or_else(|| anyhow!("no records in {json}"))?;
    let as_lines: Vec<String> = records
        .iter()
        .map(|r| format!("{} {}", r["id"].as_str().unwrap_or(""), r["title"].as_str().unwrap_or("")))
        .collect();
    assert_eq!(as_lines, record_lines(&text), "{json}\n{text}");
    let kinds: Vec<&str> = records.iter().filter_map(|r| r["kind"].as_str()).collect();
    assert_eq!(kinds, ["ask", "ask", "task", "notification", "notification", "task"]);
    let sections: Vec<(&str, u64, u64)> = principal["sections"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| (s["section"].as_str().unwrap_or(""), s["shown"].as_u64().unwrap_or(9), s["of"].as_u64().unwrap_or(9)))
        .collect();
    assert_eq!(
        sections,
        [("asked_of_you", 1, 1), ("you_asked", 1, 1), ("assigned", 1, 1), ("unread", 2, 2), ("ready", 1, 1)]
    );
    assert!(json["page"].as_str().is_some_and(|page| page.starts_with("http://127.0.0.1:")), "{json}");
    Ok(())
}

/// Given `--timeout`, a daemon that cannot be reached costs no more than the deadline: a daemon
/// that holds its lock and whose port takes requests and never answers, as the person's machine
/// once had, ends the command by the deadline with exit 0 and one line saying why there is nothing
/// to show, as JSON with `--format json`; and with no daemon and none to start, at once, naming
/// that.
#[test]
fn with_a_deadline_an_unreachable_daemon_gives_one_line_and_exit_0_in_time() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let paths = Paths { runtime_dir: dir.path().join("run"), data_dir: dir.path().join("data") };
    std::fs::create_dir_all(&paths.runtime_dir)?;
    let fwd = |args: &[&str]| -> Result<(Output, Duration)> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fwd"));
        command
            .current_dir(dir.path())
            .env("FWD_RUNTIME_DIR", &paths.runtime_dir)
            .env("FWD_DATA_DIR", &paths.data_dir)
            .env("FWD_DEV_RUNNER_DIR", dir.path().join("dev-runner"))
            .args(args);
        for name in ["FWD_PROJECT", "FWD_AS", "FWD_SCOPE", "FWD_DAEMON_BIN", "FWD_CLAUDE_SESSION", "FWD_CLAUDE_AGENT"] {
            command.env_remove(name);
        }
        let began = Instant::now();
        let out = command.output()?;
        Ok((out, began.elapsed()))
    };

    let (out, _) = fwd(&["--no-autostart", "prime", "--session", SESSION, "--timeout", "5", "--format", "text"])?;
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
        ("json", format!("{}\n", serde_json::json!({"state": "unavailable", "reason": "it did not finish within 2s"}))),
    ] {
        let (out, took) = fwd(&["prime", "--session", SESSION, "--timeout", "2", "--format", format])?;
        assert_eq!(out.status.code(), Some(0), "{format}");
        assert_eq!(String::from_utf8(out.stdout)?, said);
        // Without the deadline it waits ten seconds for a daemon to answer, then fails.
        assert!(took < Duration::from_secs(6), "{format}: it ended {took:?} after it began");
    }
    drop(held);
    Ok(())
}
