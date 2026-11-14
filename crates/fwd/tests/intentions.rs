//! `fwd intention` and `fwd reading` through the CLI, against a scratch daemon of the test's own
//! that hosts two projects, alpha and beta, made by the person `tester`: an agent records what the
//! person wants and works under it, the person confirms the agent's reading of the revision they
//! read and no other, and a text reaches a command whole on standard input.

use anyhow::{anyhow, Result};
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// The Claude Code session the agent here runs in, whose own thread is the agent.
const SESSION: &str = "0b5e55ed-1d1e-4c0a-9d2e-5ca1ab1e0002";

/// A scratch folder holding a daemon's runtime and data folders and a folder of each project.
struct Scratch {
    dir: tempfile::TempDir,
    alpha: PathBuf,
    beta: PathBuf,
}

impl Scratch {
    fn new() -> Result<Scratch> {
        let dir = tempfile::tempdir()?;
        let (alpha, beta) = (dir.path().join("alpha"), dir.path().join("beta"));
        std::fs::create_dir_all(&alpha)?;
        std::fs::create_dir_all(&beta)?;
        let s = Scratch { dir, alpha, beta };
        s.json(&s.alpha, None, &["init", "--name", "alpha", "--handle", "tester"])?;
        s.json(&s.beta, None, &["init", "--name", "beta", "--handle", "tester"])?;
        Ok(s)
    }

    /// `fwd --format json <args>` in `dir`, reaching this scratch daemon only, as the person, or,
    /// stamped as the fwd plugin stamps a command, as the session's agent `agent`, with `stdin`.
    fn run(&self, dir: &Path, agent: Option<&str>, args: &[&str], stdin: &str) -> Result<Output> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fwd"));
        command
            .current_dir(dir)
            .env("FWD_RUNTIME_DIR", self.dir.path().join("run"))
            .env("FWD_DATA_DIR", self.dir.path().join("data"))
            .env("FWD_DEV_RUNNER_DIR", self.dir.path().join("dev-runner"));
        for name in
            ["FWD_PROJECT", "FWD_AS", "FWD_SCOPE", "FWD_CLAUDE_SESSION", "FWD_CLAUDE_AGENT", "CLAUDE_CODE_SESSION_ID"]
        {
            command.env_remove(name);
        }
        if let Some(agent) = agent {
            command.env("FWD_CLAUDE_SESSION", SESSION).env("FWD_CLAUDE_AGENT", agent);
        }
        let mut child = command
            .args(["--format", "json"])
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?.write_all(stdin.as_bytes())?;
        Ok(child.wait_with_output()?)
    }

    /// What `fwd <args>` answers, which must succeed, given `stdin`.
    fn json_in(&self, dir: &Path, agent: Option<&str>, args: &[&str], stdin: &str) -> Result<Value> {
        let out = self.run(dir, agent, args, stdin)?;
        if !out.status.success() {
            return Err(anyhow!("`fwd {}` failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr)));
        }
        Ok(serde_json::from_slice(&out.stdout)?)
    }

    fn json(&self, dir: &Path, agent: Option<&str>, args: &[&str]) -> Result<Value> {
        self.json_in(dir, agent, args, "")
    }

    /// The id of what `fwd <args>` made.
    fn made(&self, dir: &Path, agent: Option<&str>, args: &[&str], stdin: &str) -> Result<String> {
        let made = self.json_in(dir, agent, args, stdin)?;
        Ok(made["record"]["id"].as_str().ok_or_else(|| anyhow!("no id under record in {made}"))?.to_string())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = self.run(self.dir.path(), None, &["daemon", "stop"], "");
    }
}

/// The one intention `impl-sam` works under, as `intention list --mine` gives it.
fn only_one(s: &Scratch) -> Result<Value> {
    let mine = s.json(&s.alpha, Some("main"), &["intention", "list", "--mine"])?;
    match mine["intentions"].as_array().map(Vec::as_slice) {
        Some([one]) => Ok(one.clone()),
        _ => Err(anyhow!("not one intention: {mine}")),
    }
}

#[test]
fn an_agent_works_under_what_the_person_said_and_the_person_confirms_the_reading_they_read() -> Result<()> {
    let s = Scratch::new()?;
    s.json(&s.alpha, Some("main"), &["principal", "add", "impl-sam", "--role", "implementer"])?;
    let words = "- the list keeps its place\n- and its selection\n";
    let it = s.made(
        &s.alpha,
        Some("main"),
        &[
            "intention",
            "create",
            "Readers keep their place",
            "--words",
            "-",
            "--said-by",
            "tester",
            "--status",
            "committed",
        ],
        words,
    )?;
    let shown = s.json(&s.alpha, None, &["intention", "show", &it])?;
    assert_eq!((shown["words"].as_str(), shown["speaker_handle"].as_str()), (Some(words), Some("tester")));
    assert_eq!(shown["written_by_handle"], "impl-sam");

    s.json(&s.alpha, Some("main"), &["link", "me", "works_under", &it])?;
    let reading =
        s.made(&s.alpha, Some("main"), &["reading", "set", &it, "which list?", "--stance", "question"], "")?;
    assert_eq!(only_one(&s)?["standing"], "to_discuss");
    s.json(&s.alpha, Some("main"), &["reading", "set", &it, "the task list", "--stance", "agree"])?;
    let unconfirmed = only_one(&s)?;
    assert_eq!(
        (&unconfirmed["standing"], &unconfirmed["reading"]["id"]),
        (&"unconfirmed".into(), &reading.clone().into())
    );

    let stale = s.run(&s.alpha, None, &["reading", "confirm", &reading, "--as-read", "000000000000"], "")?;
    let said = String::from_utf8_lossy(&stale.stderr);
    assert_eq!(stale.status.code(), Some(5), "a stale revision: {said}");
    assert!(said.contains("has changed since you read it"), "{said}");
    let revision = unconfirmed["reading"]["revision"].as_str().ok_or_else(|| anyhow!("no revision"))?;
    s.json(&s.alpha, None, &["reading", "confirm", &reading, "--as-read", revision])?;
    let confirmed = only_one(&s)?;
    assert_eq!((&confirmed["standing"], &confirmed["confirmed"]["by_handle"]), (&"confirmed".into(), &"tester".into()));

    s.json(&s.alpha, Some("main"), &["unlink", "me", "works_under", &it, "--reason", "done with it"])?;
    let mine = s.json(&s.alpha, Some("main"), &["intention", "list", "--mine"])?;
    assert_eq!(mine["intentions"], serde_json::json!([]));
    Ok(())
}

#[test]
fn an_agent_works_under_an_intention_of_another_project_and_a_text_reaches_a_command_whole_on_stdin() -> Result<()> {
    let s = Scratch::new()?;
    s.json(&s.alpha, Some("main"), &["principal", "add", "impl-sam", "--role", "implementer"])?;
    let theirs = s.made(&s.beta, None, &["intention", "create", "Builds stay under a minute", "--words", ""], "")?;
    s.json(&s.alpha, Some("main"), &["link", "me", "works_under", &theirs])?;
    let held = only_one(&s)?;
    assert_eq!((&held["intention"]["project"], &held["intention"]["words"]), (&"beta".into(), &"".into()));
    assert_eq!(held["standing"], "unread");
    let shown = s.json(&s.alpha, None, &["intention", "show", &theirs])?;
    assert_eq!((&shown["from_project"], &shown["title"]), (&"beta".into(), &"Builds stay under a minute".into()));

    let asked = s.made(&s.alpha, Some("main"), &["ask", "--to", "tester", "which build?"], "")?;
    let text = "- the release build\n- not the debug one";
    let reply = s.json_in(&s.alpha, None, &["reply", &asked, "-"], text)?;
    assert_eq!(reply["record"]["content"], text);
    Ok(())
}
