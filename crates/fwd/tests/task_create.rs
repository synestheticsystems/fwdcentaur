//! Where `fwd task create` makes a task worked, so that a task filed in a scope is found and
//! started there without a `fwd link` afterwards (task cToJvD22, "Make fwd task create write
//! worked_in for its --scope, and for the scope the checkout finds"): in the scope `--scope` names,
//! else in the scope the checkout finds, the one it is bound to (`fwd scope attach-dir`) or the
//! one active scope that names its branch, else in none. Each test runs the CLI against a scratch
//! daemon of its own.

use anyhow::{anyhow, Result};
use fwd_core::anchoring::git;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Command;

/// A scratch directory holding the daemon's runtime and data directories and a checkout of a new
/// project on the branch `main`, which the person `tester` joined. The daemon is stopped when the
/// test ends.
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
        git(&root, &["remote", "add", "origin", "git@example.com:t/create.git"])?;
        let scratch = Scratch { dir, root };
        scratch.run(&["init", "--name", "create", "--handle", "tester"])?;
        Ok(scratch)
    }

    /// What `fwd <args>` prints as JSON in the checkout.
    fn run(&self, args: &[&str]) -> Result<Value> {
        Ok(serde_json::from_slice(&self.printed("json", args)?)?)
    }

    /// What `fwd --format <format> <args>` prints in the checkout, reaching this scratch daemon
    /// only, with none of the caller's settings that would choose another project, principal or
    /// scope.
    fn printed(&self, format: &str, args: &[&str]) -> Result<Vec<u8>> {
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
        let out = command.args(["--format", format]).args(args).output()?;
        if !out.status.success() {
            return Err(anyhow!("fwd {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr)));
        }
        Ok(out.stdout)
    }

    /// A new scope, made in the checkout, naming `branches` (the checkout's branch, `main`, when
    /// none is given): its id.
    fn scope(&self, title: &str, branches: &[&str]) -> Result<String> {
        let mut args = vec!["scope", "create", title, "--intention", "the work of the test"];
        for branch in branches {
            args.extend(["--branch", branch]);
        }
        let made = self.run(&args)?;
        Ok(made["record"]["id"].as_str().ok_or_else(|| anyhow!("no id under record in {made}"))?.to_string())
    }

    /// The scope the task `fwd task create <args>` makes is worked in, as it prints it and as
    /// `fwd task show` reads it from the graph afterwards; `null` for none.
    fn worked_in(&self, args: &[&str]) -> Result<Value> {
        let made = self.run(&[&["task", "create"][..], args].concat())?;
        let id = made["record"]["id"].as_str().ok_or_else(|| anyhow!("no id under record in {made}"))?;
        let shown = self.run(&["task", "show", id])?;
        assert_eq!(made["record"]["worked_in"], shown["worked_in"], "the task create prints is the task show reads");
        Ok(shown["worked_in"].clone())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = self.run(&["daemon", "stop"]);
    }
}

/// `--scope` names the scope, which wins over the one the checkout finds: a task filed with it is
/// worked in the scope it names.
#[test]
fn a_task_filed_with_scope_is_worked_in_the_scope_it_names() -> Result<()> {
    let s = Scratch::new()?;
    s.scope("Replication", &[])?;
    let storage = s.scope("Storage", &["storage"])?;
    assert_eq!(s.worked_in(&["Store the graph", "--scope", &storage])?, json!(storage));
    Ok(())
}

/// A checkout bound to a scope finds that scope, whatever its branch: a task filed there is worked
/// in it, the binding winning over the scope that names the branch.
#[test]
fn a_task_filed_in_a_checkout_bound_to_a_scope_is_worked_in_it() -> Result<()> {
    let s = Scratch::new()?;
    s.scope("Replication", &[])?;
    let storage = s.scope("Storage", &["storage"])?;
    s.run(&["scope", "attach-dir", &storage])?;
    assert_eq!(s.worked_in(&["Store the graph"])?, json!(storage));
    Ok(())
}

/// A checkout whose branch one active scope names finds that scope: a task filed there is worked
/// in it.
#[test]
fn a_task_filed_where_one_active_scope_names_the_branch_is_worked_in_it() -> Result<()> {
    let s = Scratch::new()?;
    let replication = s.scope("Replication", &[])?;
    assert_eq!(s.worked_in(&["Replicate the relay"])?, json!(replication));
    Ok(())
}

/// For people, `fwd task create` prints the task as `fwd task show` prints it, so that whoever filed
/// it at a terminal sees the scope it is worked in; it printed the task's one line, which names no
/// scope.
#[test]
fn a_task_filed_at_a_terminal_prints_as_task_show_prints_it() -> Result<()> {
    let s = Scratch::new()?;
    s.scope("Replication", &[])?;
    let made = String::from_utf8(s.printed("text", &["task", "create", "Replicate the relay"])?)?;
    let id = made.split_whitespace().next().ok_or_else(|| anyhow!("task create printed nothing"))?;
    let shown = String::from_utf8(s.printed("text", &["task", "show", id])?)?;
    assert_eq!(made, shown);
    assert!(made.contains("scope: Replication"), "{made}");
    Ok(())
}

/// A checkout that finds no scope, none naming its branch or several, files a task worked in
/// none, as before: a start or a link puts it in a scope later.
#[test]
fn a_task_filed_where_no_scope_or_several_are_found_is_worked_in_none() -> Result<()> {
    let s = Scratch::new()?;
    assert_eq!(s.worked_in(&["Write the primer"])?, Value::Null);
    s.scope("Replication", &[])?;
    s.scope("Docs", &[])?;
    assert_eq!(s.worked_in(&["Write the guide"])?, Value::Null);
    Ok(())
}

/// A task linked to a second scope is worked in both (docs/DECISIONS.md, D136, "A task may be
/// worked in several scopes"): `fwd task show` lists them under `scopes`, each with its id and
/// name, keeps the first in `worked_in` for readers that read one, and for people says "scopes:"
/// and both names; taken off one, it is worked in the other alone again, "scope:".
#[test]
fn a_task_linked_to_a_second_scope_shows_both() -> Result<()> {
    let s = Scratch::new()?;
    let replication = s.scope("Replication", &[])?;
    let storage = s.scope("Storage", &["storage"])?;
    let made = s.run(&["task", "create", "Replicate the store"])?;
    let id = made["record"]["id"].as_str().ok_or_else(|| anyhow!("no id under record in {made}"))?.to_string();
    s.run(&["link", &id, "worked_in", &storage])?;
    let shown = s.run(&["task", "show", &id])?;
    let mut scopes: Vec<Value> = shown["scopes"].as_array().cloned().unwrap_or_default();
    assert_eq!(shown["worked_in"], scopes[0]["id"], "the first of them, for one release");
    scopes.sort_by_key(|s| s["title"].as_str().map(str::to_string));
    assert_eq!(
        scopes,
        vec![json!({"id": replication, "title": "Replication"}), json!({"id": storage, "title": "Storage"})]
    );
    let text = String::from_utf8(s.printed("text", &["task", "show", &id])?)?;
    assert!(text.contains("scopes: Replication, Storage") || text.contains("scopes: Storage, Replication"), "{text}");
    s.run(&["unlink", &id, "worked_in", &replication, "--reason", "storage work"])?;
    let text = String::from_utf8(s.printed("text", &["task", "show", &id])?)?;
    assert!(text.contains("scope: Storage"), "{text}");
    Ok(())
}
