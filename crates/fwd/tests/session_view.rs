//! `fwd session view` as a pane runs it: the person's view of a Claude Code session, one JSON
//! document on one line, and the actions it names, run as the pane runs them. Each test runs the
//! CLI against a scratch daemon of its own, which hosts two projects, alpha and beta, that the
//! person `tester` made.

use anyhow::{anyhow, Result};
use fwd_core::anchoring::git;
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// The Claude Code session whose view is read; its own thread is the agent `main`.
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

    /// The JSON `fwd --format json <args>` prints, which must succeed.
    fn json(&self, dir: &Path, agent: Option<&str>, args: &[&str]) -> Result<Value> {
        let out = self.command(dir, agent, &[&["--format", "json"][..], args].concat()).output()?;
        if !out.status.success() {
            return Err(anyhow!("fwd {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr)));
        }
        Ok(serde_json::from_slice(&out.stdout)?)
    }

    /// The id of what `fwd <args>`, a command that makes something, made.
    fn made(&self, dir: &Path, agent: Option<&str>, args: &[&str]) -> Result<String> {
        let made = self.json(dir, agent, args)?;
        Ok(made["record"]["id"].as_str().ok_or_else(|| anyhow!("no id under record in {made}"))?.to_string())
    }

    /// Returns once the principal the session's agent `agent` is bound to in `dir`'s project holds
    /// `count` unread notifications: `fwd watch` ends as the last of them arrives.
    fn told(&self, dir: &Path, agent: &str, count: usize) -> Result<()> {
        let watch = ["--format", "json", "watch", "--count", &count.to_string(), "--timeout", "60"];
        let out = self.command(dir, Some(agent), &watch).output()?;
        let ended = String::from_utf8(out.stdout)?;
        match ended.lines().last().map(serde_json::from_str::<Value>).transpose()? {
            Some(last) if last["event"] == "done" => Ok(()),
            _ => Err(anyhow!("the watch did not see {count} notifications: {ended}")),
        }
    }

    /// The session's view as the pane reads it: from a directory of no project, with nothing in
    /// its environment naming a principal, one line on standard output.
    fn view(&self) -> Result<Value> {
        let out = self.command(self.dir.path(), None, &["session", "view", SESSION]).output()?;
        if !out.status.success() {
            return Err(anyhow!("fwd session view failed: {}", String::from_utf8_lossy(&out.stderr)));
        }
        let printed = String::from_utf8(out.stdout)?;
        let (line, rest) = printed.split_once('\n').ok_or_else(|| anyhow!("no whole line in {printed:?}"))?;
        assert_eq!(rest, "", "one line: {printed}");
        Ok(serde_json::from_str(line)?)
    }

    /// Runs `args` as the pane runs an action: the program fwd, no shell, in a directory of no
    /// project, with no session in its environment, `input` on its standard input.
    fn press(&self, args: &[String], input: &str) -> Result<Output> {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let mut child = self
            .command(self.dir.path(), None, &args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?.write_all(input.as_bytes())?;
        Ok(child.wait_with_output()?)
    }

    fn project_id(&self, dir: &Path) -> Result<String> {
        Ok(self.json(dir, None, &["project", "show"])?["id"].as_str().unwrap_or_default().to_string())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = self.command(self.dir.path(), None, &["daemon", "stop"]).output();
    }
}

/// The section `section` of the group `group`, if the group has it.
fn list<'a>(group: &'a Value, section: &str) -> Option<&'a Value> {
    group["sections"].as_array()?.iter().find(|s| s["section"] == section)
}

/// The titles of the rows of the section `section` of `group`.
fn titles(group: &Value, section: &str) -> Vec<String> {
    list(group, section)
        .and_then(|s| s["records"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|r| r["title"].as_str().map(str::to_string))
        .collect()
}

/// The session's own thread names itself `lead-sam` in alpha and in beta, and a subagent names
/// itself `impl-sam` in alpha. In alpha, lead-sam asks the person something, asks impl-sam
/// something and asks no one in particular something, works a task the person gave it, and is asked
/// something by the person; impl-sam asks the person something; in beta, lead-sam holds a task. The
/// view lists the own thread's groups first, by project, then the subagent's, each with what waits
/// on the person, what it holds and what waits on others, the unread notifications counted on the
/// group, and the subagent's group open since something in it waits on the person.
#[test]
fn the_view_lists_each_principal_of_the_session_with_what_waits_on_the_person_first() -> Result<()> {
    let s = Scratch::new()?;
    s.json(&s.alpha, Some("main"), &["principal", "add", "lead-sam", "--role", "lead"])?;
    s.json(&s.beta, Some("main"), &["principal", "add", "lead-sam", "--role", "lead"])?;
    s.json(&s.alpha, Some("a1"), &["principal", "add", "impl-sam", "--role", "implementer"])?;
    let what = "When the daemon restarts, a command in flight fails.\n\nRetry it once the daemon answers.";
    let task = s.made(&s.alpha, None, &["task", "create", "Retry on disconnect", "--what", what])?;
    s.json(&s.alpha, None, &["task", "assign", &task, "lead-sam"])?;
    s.json(&s.alpha, Some("main"), &["task", "start", &task])?;
    let held_in_beta = s.made(&s.beta, None, &["task", "create", "Cap the backoff"])?;
    s.json(&s.beta, None, &["task", "assign", &held_in_beta, "lead-sam"])?;
    let asked = "Should the primer name the page?\n\nIt costs every session about forty characters.";
    s.made(&s.alpha, Some("main"), &["ask", "--to", "tester", asked])?;
    s.made(&s.alpha, Some("main"), &["ask", "--to", "impl-sam", "Does the retry cover the page too?"])?;
    s.made(&s.alpha, Some("main"), &["ask", "Who reviews the retry?"])?;
    s.made(&s.alpha, Some("a1"), &["ask", "--to", "tester", "May I cap the backoff at two seconds?"])?;
    s.made(&s.alpha, None, &["ask", "--to", "lead-sam", "How long a wait?"])?;
    // The assignment and the person's question.
    s.told(&s.alpha, "main", 2)?;

    let view = s.view()?;
    assert_eq!(view["title"], "fwd: what this session holds");
    assert!(view["page"].as_str().is_some_and(|page| page.starts_with("http://127.0.0.1:")), "{view}");
    assert_eq!(view["lines"], json!([]));
    let groups = view["principals"].as_array().ok_or_else(|| anyhow!("no principals in {view}"))?;
    let named: Vec<(&Value, &Value, &Value)> =
        groups.iter().map(|g| (&g["title"], &g["tags"][1]["text"], &g["folded"])).collect();
    assert_eq!(
        named,
        [
            (&json!("lead-sam"), &json!("alpha"), &json!(false)),
            (&json!("lead-sam"), &json!("beta"), &json!(false)),
            (&json!("impl-sam"), &json!("alpha"), &json!(false)),
        ],
        "{view}"
    );
    let lead = &groups[0];
    assert_eq!(
        lead["tags"],
        json!([{"text": "lead", "tone": null, "at": null}, {"text": "alpha", "tone": null, "at": null}, {"text": "2 unread", "tone": null, "at": null}])
    );
    let sections: Vec<&str> =
        lead["sections"].as_array().into_iter().flatten().filter_map(|s| s["section"].as_str()).collect();
    assert_eq!(sections, ["waits_on_you", "holds", "waits_on_others"]);
    assert_eq!(titles(lead, "waits_on_you"), ["Should the primer name the page? It costs…"]);
    assert_eq!(titles(lead, "holds"), ["How long a wait?", "Retry on disconnect"]);
    assert_eq!(
        titles(lead, "waits_on_others"),
        ["Who reviews the retry?", "Does the retry cover the page too?"],
        "newest first"
    );
    let question = &list(lead, "waits_on_you").expect("waits on you")["records"][0];
    assert_eq!(question["lines"], json!([asked]));
    assert_eq!(question["tags"][0]["text"], "asked");
    assert_eq!(question["tags"][0]["tone"], "attention");
    let id = question["id"].as_str().unwrap_or_default();
    assert_eq!(question["path"], format!("?p={}#/questions/{id}", s.project_id(&s.alpha)?));
    let held = &list(lead, "holds").expect("holds")["records"];
    assert_eq!(
        (&held[0]["tags"][0]["text"], &held[1]["tags"][0]["text"]),
        (&json!("from tester"), &json!("in progress"))
    );
    assert_eq!(held[1]["lines"], json!([what]));
    let others = &list(lead, "waits_on_others").expect("waits on others");
    assert_eq!(others["folded"], true);
    let to: Vec<&Value> = others["records"].as_array().into_iter().flatten().map(|r| &r["tags"][0]["text"]).collect();
    assert_eq!(to, [&json!("to no one in particular"), &json!("to impl-sam")]);
    assert_eq!(titles(&groups[1], "holds"), ["Cap the backoff"]);
    assert_eq!(titles(&groups[2], "waits_on_you"), ["May I cap the backoff at two seconds?"]);
    Ok(())
}

/// Answer replies to the question in the person's words, exactly as the pane writes them on
/// standard input, a leading hyphen and several lines included, run from any directory with no
/// session in its environment, and prints nothing; the question then leaves what waits on the
/// person. A press that fails says why in one line on standard error and exits non-zero, and
/// prints nothing on standard output.
#[test]
fn answer_replies_in_the_persons_words_and_a_failed_press_says_why_in_one_line() -> Result<()> {
    let s = Scratch::new()?;
    s.json(&s.alpha, Some("main"), &["principal", "add", "lead-sam", "--role", "lead"])?;
    let question = s.made(&s.alpha, Some("main"), &["ask", "--to", "tester", "Should the primer name the page?"])?;
    let view = s.view()?;
    let row = &list(&view["principals"][0], "waits_on_you").expect("waits on you")["records"][0];
    let answer = &row["actions"][0];
    assert_eq!(answer["label"], "Answer");
    assert_eq!(answer["input"], json!({"prompt": "Your answer", "multiline": true}));
    let args: Vec<String> = serde_json::from_value(answer["args"].clone())?;
    assert_eq!(
        args,
        [
            "--project",
            &s.project_id(&s.alpha)?,
            "--as",
            "tester",
            "--format",
            "pane",
            "reply",
            &question,
            "--text-from-stdin"
        ]
    );

    let words = "-- no: the pane links there.\n\nLeave the primer as it is.\n";
    let pressed = s.press(&args, words)?;
    assert_eq!(
        (pressed.status.code(), pressed.stdout.as_slice(), pressed.stderr.as_slice()),
        (Some(0), &b""[..], &b""[..]),
        "{}",
        String::from_utf8_lossy(&pressed.stderr)
    );
    let thread = s.json(&s.alpha, None, &["message", "list"])?;
    let replies: Vec<&Value> =
        thread.as_array().into_iter().flatten().filter(|m| m["reply_to"] == json!(question)).collect();
    assert_eq!(replies.len(), 1, "{thread}");
    assert_eq!(replies[0]["content"], words);
    assert_eq!(list(&s.view()?["principals"][0], "waits_on_you"), None, "answered, it waits on no one");

    let mut gone = args.clone();
    gone[7] = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into();
    let failed = s.press(&gone, "too late")?;
    let said = String::from_utf8(failed.stderr)?;
    assert!(!failed.status.success() && failed.stdout.is_empty(), "{said}");
    assert_eq!(said.lines().count(), 1, "{said}");
    assert!(said.ends_with('\n') && !said.starts_with("Error"), "{said}");
    Ok(())
}

/// `fwd reply` takes its text from standard input whole, even when it holds nothing, and takes it
/// from the argument or standard input, never both; `--format pane` is for the commands the view's
/// actions run alone.
#[test]
fn a_reply_takes_its_text_from_standard_input_whole_and_pane_is_for_actions_alone() -> Result<()> {
    let s = Scratch::new()?;
    let question = s.made(&s.alpha, None, &["ask", "--to", "tester", "Is an empty answer kept?"])?;
    let empty = s.press(
        &[
            "--as".into(),
            "tester".into(),
            "--format".into(),
            "json".into(),
            "--project".into(),
            "alpha".into(),
            "reply".into(),
            question.clone(),
            "--text-from-stdin".into(),
        ],
        "",
    )?;
    assert!(empty.status.success(), "{}", String::from_utf8_lossy(&empty.stderr));
    let made: Value = serde_json::from_slice(&empty.stdout)?;
    assert_eq!(made["record"]["content"], "");

    let both = s.command(&s.alpha, None, &["reply", &question, "words", "--text-from-stdin"]).output()?;
    assert!(!both.status.success(), "the argument and standard input are not both taken");
    let neither = s.command(&s.alpha, None, &["reply", &question]).output()?;
    assert!(!neither.status.success(), "one of them is");

    let other = s.command(&s.alpha, None, &["--format", "pane", "task", "list"]).output()?;
    let said = String::from_utf8(other.stderr)?;
    assert!(
        !other.status.success() && said.contains("--format pane is for the commands a session view's actions run"),
        "{said}"
    );
    Ok(())
}

/// A session no principal is bound to says so in its one line, and the view is JSON alone: another
/// `--format` is refused, as is an id that is no Claude Code session's.
#[test]
fn a_session_no_principal_is_bound_to_says_so() -> Result<()> {
    let s = Scratch::new()?;
    let view = s.view()?;
    assert_eq!(view["principals"], json!([]));
    assert_eq!(
        view["lines"],
        json!(["No fwd principal is bound to this session yet. Its agent names itself with `fwd principal add <name> --role <role>`."])
    );
    let text = s.command(s.dir.path(), None, &["--format", "text", "session", "view", SESSION]).output()?;
    assert!(!text.status.success() && String::from_utf8_lossy(&text.stderr).contains("prints JSON alone"));
    let wrong = s.command(s.dir.path(), None, &["session", "view", "not a session"]).output()?;
    assert!(
        !wrong.status.success() && String::from_utf8_lossy(&wrong.stderr).contains("is not a Claude Code session id")
    );
    Ok(())
}
