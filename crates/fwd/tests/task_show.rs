//! What `fwd task show --include` adds after the checklist, so that a task's thread and its links
//! can be read from the command line (task 4ujMsNeQ, "Let fwd task show print the task's comments
//! and replies"): `messages`, each thread under its first message, and `relations`, what the task
//! points at and what points at it, each as JSON and as text. Each test runs the CLI against a
//! scratch daemon of its own.

use anyhow::{anyhow, Result};
use fwd_core::anchoring::git;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Command;

/// A scratch directory holding the daemon's runtime and data directories and a checkout of a new
/// project, which the person `tester` joined. The daemon is stopped when the test ends.
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
        git(&root, &["remote", "add", "origin", "git@example.com:t/show.git"])?;
        let scratch = Scratch { dir, root };
        scratch.run(&["init", "--name", "show", "--handle", "tester"])?;
        Ok(scratch)
    }

    /// What `fwd <args>` prints in the checkout, reaching this scratch daemon only, with none of
    /// the caller's settings that would choose another project, principal or scope.
    fn run(&self, args: &[&str]) -> Result<String> {
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
        let out = command.args(args).output()?;
        if !out.status.success() {
            return Err(anyhow!("fwd {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr)));
        }
        Ok(String::from_utf8(out.stdout)?)
    }

    /// The JSON `fwd --format json <args>` prints.
    fn json(&self, args: &[&str]) -> Result<Value> {
        Ok(serde_json::from_str(&self.run(&[&["--format", "json"][..], args].concat())?)?)
    }

    /// The id of what `fwd <args>`, a command that makes something, made.
    fn made(&self, args: &[&str]) -> Result<String> {
        let printed = self.json(args)?;
        Ok(printed["record"]["id"].as_str().ok_or_else(|| anyhow!("no id under record in {printed}"))?.to_string())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = self.run(&["daemon", "stop"]);
    }
}

/// The first eight characters of an id, as the text gives them.
fn short(id: &str) -> &str {
    &id[..8]
}

/// `--include messages` gives the task's comments and questions and their replies, oldest first:
/// in the JSON as `messages`, each with its author's handle, its status and, for a question asked
/// of one principal, that principal's handle; in the text after the checklist, each thread under
/// its first message, with who wrote it, whom it asks, the start of its id, whether it is active or
/// resolved and how long ago it was written, and its text beneath. Without it, neither.
#[test]
fn include_messages_gives_each_thread_under_its_first_message() -> Result<()> {
    let s = Scratch::new()?;
    let task = s.made(&["task", "create", "Retry on disconnect"])?;
    let comment = s.made(&["comment", &task, "what if we retried?"])?;
    let reply = s.made(&["reply", &comment, "- with a backoff\n- capped"])?;
    s.run(&["--as", "impl-a", "principal", "add", "impl-a", "--role", "implementer"])?;
    let question = s.made(&["--as", "impl-a", "ask", "--to", "@tester", "--about", &task, "is 30s the cap?"])?;
    s.run(&["message", "resolve", &question])?;

    let shown = s.json(&["task", "show", &task, "--include", "messages"])?;
    let messages = shown["messages"].as_array().ok_or_else(|| anyhow!("no messages in {shown}"))?;
    let got: Vec<Value> = messages
        .iter()
        .map(|m| json!([m["id"], m["author_handle"], m["status"], m["thread"], m["to_handle"]]))
        .collect();
    assert_eq!(
        got,
        [
            json!([comment, "tester", "active", null, null]),
            json!([reply, "tester", "active", comment, null]),
            json!([question, "impl-a", "resolved", null, "tester"]),
        ],
        "{shown}"
    );
    assert_eq!(s.json(&["task", "show", &task])?.get("messages"), None, "only when asked for");

    let text = s.run(&["task", "show", &task, "--include", "messages", "--format", "text"])?;
    let expected = format!(
        "messages:
  @tester (id {}, active, just now)
    what if we retried?
    @tester (id {}, active, just now)
      - with a backoff
      - capped
  @impl-a asks @tester (id {}, resolved, just now)
    is 30s the cap?
",
        short(&comment),
        short(&reply),
        short(&question)
    );
    assert!(text.ends_with(&expected), "{text}");
    Ok(())
}

/// `--include relations` gives what the task points at and what points at it: in the JSON as
/// `relations`, `out` and `in`, each relation as `fwd relations` prints it with the thing at its
/// other end named; in the text after the checklist, each as a line saying which way it goes, with
/// the relation's status when it is not accepted.
#[test]
fn include_relations_gives_what_the_task_points_at_and_what_points_at_it() -> Result<()> {
    let s = Scratch::new()?;
    let task = s.made(&["task", "create", "Retry on disconnect"])?;
    let connector = s.made(&["task", "create", "Connector"])?;
    let jitter = s.made(&["task", "create", "Add jitter", "--under", &task])?;
    s.run(&["link", &task, "depends_on", &connector])?;
    s.run(&["link", &task, "relates_to", &jitter, "--propose"])?;

    let shown = s.json(&["task", "show", &task, "--include", "relations"])?;
    let ends = |side: &str| -> Vec<Value> {
        shown["relations"][side]
            .as_array()
            .into_iter()
            .flatten()
            .map(|r| json!([r["kind"], r["status"], r["other"]["id"], r["other"]["kind"], r["other"]["name"]]))
            .collect()
    };
    assert_eq!(
        ends("out"),
        [
            json!(["depends_on", "accepted", connector, "task", "Connector"]),
            json!(["relates_to", "proposed", jitter, "task", "Add jitter"]),
        ],
        "{shown}"
    );
    assert_eq!(ends("in"), [json!(["subtask_of", "accepted", jitter, "task", "Add jitter"])], "{shown}");
    assert_eq!(s.json(&["task", "show", &task])?.get("relations"), None, "only when asked for");

    let text = s.run(&["task", "show", &task, "--include", "relations", "--format", "text"])?;
    let expected = format!(
        "relations:
  this depends_on Connector (task, {})
  this relates_to Add jitter (task, {}) [proposed]
  Add jitter (task, {}) subtask_of this
",
        short(&connector),
        short(&jitter),
        short(&jitter)
    );
    assert!(text.ends_with(&expected), "{text}");

    // Both at once, in either order: the relations first, then the thread.
    let both = s.run(&["task", "show", &task, "--include", "messages,relations", "--format", "text"])?;
    assert!(both.ends_with(&format!("{expected}messages: none\n")), "{both}");
    Ok(())
}
