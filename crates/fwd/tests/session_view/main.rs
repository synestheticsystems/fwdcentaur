//! `fwd session view` as a pane runs it: the person's view of a Claude Code session, one JSON
//! document on one line, the actions it names, run as the pane runs them, and with `--follow` the
//! stream of documents. Each test runs the CLI against a scratch daemon of its own, which hosts two
//! projects, alpha and beta, that the person `tester` made.

mod follow;
mod once;

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
