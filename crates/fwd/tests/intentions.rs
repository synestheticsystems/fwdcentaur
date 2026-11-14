//! `fwd intention` through the CLI, against a scratch daemon of the test's own that hosts two
//! projects, alpha and beta, made by the person `tester`: an agent records what the person wants,
//! in their words read whole from standard input; an intention of another project is shown with
//! its project; and an agent assigns itself one and unassigns it.

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

    /// `fwd <args>` in `dir`, reaching this scratch daemon only, as the person, or, stamped as the
    /// fwd plugin stamps a command, as the session's agent `agent`, with `stdin`.
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
        let mut child =
            command.args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
        child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?.write_all(stdin.as_bytes())?;
        Ok(child.wait_with_output()?)
    }

    /// What `fwd <args>` answers, which must succeed, given `stdin`.
    fn json_in(&self, dir: &Path, agent: Option<&str>, args: &[&str], stdin: &str) -> Result<Value> {
        let out = self.run(dir, agent, &[&["--format", "json"], args].concat(), stdin)?;
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

#[test]
fn an_agent_records_what_the_person_said_and_an_intention_of_another_project_shows_its_project() -> Result<()> {
    let s = Scratch::new()?;
    s.json(&s.alpha, Some("main"), &["principal", "add", "impl-sam", "--role", "implementer"])?;
    let words = "- the list keeps its place\n- and its selection\n";
    let create = ["intention", "create", "Readers keep their place", "--words", "-", "--said-by", "tester"];
    let it = s.made(&s.alpha, Some("main"), &create, words)?;
    let shown = s.json(&s.alpha, None, &["intention", "show", &it])?;
    assert_eq!((shown["words"].as_str(), shown["speaker_handle"].as_str()), (Some(words), Some("tester")));
    assert_eq!(shown["created_by_handle"], "impl-sam");
    let text = String::from_utf8(s.run(&s.alpha, None, &["--format", "text", "intention", "show", &it], "")?.stdout)?;
    let day = fwd_core::decision_log::date(shown["created"].as_i64().ok_or_else(|| anyhow!("no created"))?);
    let short: String = it.chars().take(8).collect();
    let said = "    - the list keeps its place\n    - and its selection";
    let expected =
        format!("{short} Readers keep their place [open]\n  @tester, {day}, written down by @impl-sam:\n{said}\n");
    assert_eq!(text, expected, "built from the intention's views, whose words and who wrote them down");

    let theirs = s.made(&s.beta, None, &["intention", "create", "Builds stay under a minute", "--words", ""], "")?;
    let shown = s.json(&s.alpha, None, &["intention", "show", &theirs])?;
    assert_eq!((&shown["from_project"], &shown["title"]), (&"beta".into(), &"Builds stay under a minute".into()));
    Ok(())
}

/// `fwd intention list` lists the intentions of the project it runs in alone, the node holding
/// every project's: alpha's in alpha, beta's in beta.
#[test]
fn the_list_holds_the_intentions_of_the_project_it_runs_in_alone() -> Result<()> {
    let s = Scratch::new()?;
    let create = |dir: &Path, title: &str| s.made(dir, None, &["intention", "create", title, "--words", ""], "");
    let ours = create(&s.alpha, "Readers keep their place")?;
    let theirs = create(&s.beta, "Builds stay under a minute")?;
    let listed = |dir: &Path| -> Result<Vec<Value>> {
        let list = s.json(dir, None, &["intention", "list"])?;
        Ok(list.as_array().into_iter().flatten().map(|r| r["id"].clone()).collect())
    };
    assert_eq!(listed(&s.alpha)?, [Value::from(ours)]);
    assert_eq!(listed(&s.beta)?, [Value::from(theirs)]);
    Ok(())
}

/// `list --active` reads every principal bound to the agent, as `fwd prime` does: run in alpha, it
/// lists an intention assigned to the agent's principal of beta alone, and one both its principals
/// hold once.
#[test]
fn the_active_list_covers_every_principal_bound_to_the_agent() -> Result<()> {
    let s = Scratch::new()?;
    for dir in [&s.alpha, &s.beta] {
        s.json(dir, Some("main"), &["principal", "add", "impl-sam", "--role", "implementer"])?;
    }
    let create = |dir: &Path, title: &str| s.made(dir, None, &["intention", "create", title, "--words", ""], "");
    let both = create(&s.alpha, "Readers keep their place")?;
    let betas = create(&s.beta, "Builds stay under a minute")?;
    for dir in [&s.alpha, &s.beta] {
        s.json(dir, Some("main"), &["intention", "assign", &both])?;
    }
    s.json(&s.beta, Some("main"), &["intention", "assign", &betas])?;
    let active = s.json(&s.alpha, Some("main"), &["intention", "list", "--active"])?;
    let ids: Vec<&Value> = active.as_array().into_iter().flatten().map(|r| &r["id"]).collect();
    assert_eq!(ids, [&Value::from(both), &Value::from(betas)], "{active}");
    Ok(())
}

/// impl-sam, an agent of alpha, assigns itself an intention of beta: it is among its active
/// intentions, named with beta, until it is fulfilled, and is unassigned all the same afterwards;
/// assigning it to someone else is refused.
#[test]
fn an_agent_assigns_itself_an_intention_of_another_project_and_unassigns_it_once_it_ended() -> Result<()> {
    let s = Scratch::new()?;
    s.json(&s.alpha, Some("main"), &["principal", "add", "impl-sam", "--role", "implementer"])?;
    let theirs = s.made(&s.beta, None, &["intention", "create", "Builds stay under a minute", "--words", ""], "")?;
    let assigned = s.json(&s.alpha, Some("main"), &["intention", "assign", &theirs])?;
    assert_eq!((&assigned["kind"], &assigned["from"]), (&"assigned_to".into(), &theirs.clone().into()));
    let active = s.json(&s.alpha, Some("main"), &["intention", "list", "--active"])?;
    assert_eq!((&active[0]["id"], &active[0]["project"]), (&theirs.clone().into(), &"beta".into()), "{active}");

    let to_tester = s.run(&s.alpha, Some("main"), &["intention", "assign", &theirs, "--to", "tester"], "")?;
    assert!(!to_tester.status.success() && String::from_utf8_lossy(&to_tester.stderr).contains("leave out --to"));

    s.json(&s.beta, None, &["intention", "fulfil", &theirs])?;
    assert_eq!(s.json(&s.alpha, Some("main"), &["intention", "list", "--active"])?, serde_json::json!([]));
    let unassigned = s.json(&s.alpha, Some("main"), &["intention", "unassign", &theirs])?;
    assert_eq!(unassigned["retracted"], 1);
    Ok(())
}
