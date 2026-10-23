//! Which checkout an anchor resolves in, on a machine with several checkouts of its repository:
//! the command's own when it holds the anchor's basis, else another the daemon knows that holds
//! it, else the command's own again; the index's answer beside the working tree's in one call,
//! as the editor asks for it when it opens a highlight; and lines anchored as a revision has them,
//! as the editor anchors them on a revision's side of a diff. Each test runs the CLI against a
//! scratch daemon of its own.

use anyhow::{anyhow, Result};
use fwd_core::anchoring::{blob_exists, git};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;

const ORIGIN: &str = "git@example.com:t/anchors.git";
const LIB: &str = "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\nl10\n";

/// A scratch directory holding the daemon's runtime and data directories and the checkouts. The
/// daemon is stopped when the test ends, whether it passed or not.
struct Scratch {
    dir: tempfile::TempDir,
}

impl Scratch {
    fn new() -> Result<Scratch> {
        Ok(Scratch { dir: tempfile::tempdir()? })
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// `fwd` in `cwd`, reaching this scratch daemon only (which it starts when none runs), with
    /// none of the caller's settings that would choose another project, principal or scope.
    fn command(&self, cwd: &Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fwd"));
        command
            .current_dir(cwd)
            .env("FWD_RUNTIME_DIR", self.path("run"))
            .env("FWD_DATA_DIR", self.path("data"))
            // The daemon inherits it: its sutra card goes to a scratch registry.
            .env("FWD_DEV_RUNNER_DIR", self.path("dev-runner"));
        for name in ["FWD_PROJECT", "FWD_AS", "FWD_SCOPE", "FWD_DAEMON_BIN", "FWD_HANDLE"] {
            command.env_remove(name);
        }
        command
    }

    /// The JSON `fwd --format json <args>` prints in `cwd`.
    fn fwd(&self, cwd: &Path, args: &[&str]) -> Result<Value> {
        let out = self.command(cwd).args(["--format", "json"]).args(args).output()?;
        if !out.status.success() {
            return Err(anyhow!("fwd {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr)));
        }
        Ok(serde_json::from_slice(&out.stdout)?)
    }

    /// What `fwd --format json <args>` made in `cwd`, a command that makes something: the record
    /// it prints under `record`.
    fn made(&self, cwd: &Path, args: &[&str]) -> Result<Value> {
        let printed = self.fwd(cwd, args)?;
        printed.get("record").cloned().ok_or_else(|| anyhow!("nothing under record in {printed}"))
    }

    /// A repository with `src/lib.rs` committed and an origin remote, joined to a new project;
    /// joining starts the scratch daemon and tells it of this checkout.
    fn project(&self) -> Result<PathBuf> {
        let root = self.path("app");
        std::fs::create_dir_all(root.join("src"))?;
        git(&root, &["init", "-q", "-b", "main"])?;
        configure(&root)?;
        git(&root, &["remote", "add", "origin", ORIGIN])?;
        std::fs::write(root.join("src/lib.rs"), LIB)?;
        git(&root, &["add", "src/lib.rs"])?;
        git(&root, &["commit", "-q", "-m", "init"])?;
        self.fwd(&root, &["init", "--name", "anchors", "--handle", "tester", "--display-name", "Tester"])?;
        Ok(root)
    }

    /// Another clone of the same upstream, with an object database of its own. The daemon learns
    /// of it when a command first runs there.
    fn clone(&self, from: &Path, name: &str) -> Result<PathBuf> {
        let root = self.path(name);
        let (from, to) = (from.to_str().expect("a UTF-8 path"), root.to_str().expect("a UTF-8 path"));
        git(self.dir.path(), &["clone", "-q", "--no-local", from, to])?;
        configure(&root)?;
        git(&root, &["remote", "set-url", "origin", ORIGIN])?;
        Ok(root)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = self.command(self.dir.path()).args(["daemon", "stop"]).output();
    }
}

fn configure(root: &Path) -> Result<()> {
    git(root, &["config", "user.email", "t@example.com"])?;
    git(root, &["config", "user.name", "t"])?;
    git(root, &["config", "commit.gpgsign", "false"])?;
    Ok(())
}

/// Puts `lines` before line `at` (0-based) of `src/lib.rs` in the working tree of `root`.
fn insert(root: &Path, at: usize, lines: &[&str]) -> Result<()> {
    let file = root.join("src/lib.rs");
    let mut all: Vec<String> = std::fs::read_to_string(&file)?.lines().map(str::to_string).collect();
    all.splice(at..at, lines.iter().map(|line| line.to_string()));
    std::fs::write(&file, all.join("\n") + "\n")?;
    Ok(())
}

fn id(anchor: &Value) -> Result<&str> {
    anchor["id"].as_str().ok_or_else(|| anyhow!("no id in {anchor}"))
}

/// An anchor made on lines not committed has for its basis content that only the clone it was
/// made in holds. Shown from another clone, it resolves in the clone that holds the basis, whose
/// working tree has the lines; the command's own checkout would find them gone.
#[test]
fn an_anchor_resolves_in_the_checkout_holding_its_basis_when_the_commands_own_does_not() -> Result<()> {
    let s = Scratch::new()?;
    let app = s.project()?;
    let other = s.clone(&app, "other")?;
    insert(&app, 1, &["fn new_a() {}", "fn new_b() {}"])?;
    let anchor = s.made(&app, &["anchor", "create", "--path", "src/lib.rs", "--lines", "2-3"])?;
    let basis = anchor["basis_blob"].as_str().ok_or_else(|| anyhow!("no basis in {anchor}"))?;
    assert!(!blob_exists(&other, basis), "the other clone lacks the basis");

    let resolved = &s.fwd(&other, &["anchor", "show", id(&anchor)?])?["resolved"];
    assert_eq!(resolved["label"], "exact", "{resolved}");
    assert_eq!(resolved["range"], json!({"start_line": 1, "end_line": 3}), "{resolved}");
    assert_eq!(resolved["target"]["blob"], basis, "the working tree the lines were anchored in: {resolved}");
    Ok(())
}

/// When no checkout holds the basis (the clone the anchor was made in is gone), each can rebuild
/// it from the anchor's patch, and the command's own checkout is chosen over another with the
/// target, even one the daemon saw first. The two place the lines differently.
#[test]
fn with_no_checkout_holding_the_basis_the_commands_own_checkout_is_chosen() -> Result<()> {
    let s = Scratch::new()?;
    let app = s.project()?;
    let other = s.clone(&app, "other")?;
    let gone = s.clone(&app, "gone")?;
    insert(&gone, 0, &["fn only_in_gone() {}"])?;
    let anchor = s.made(&gone, &["anchor", "create", "--path", "src/lib.rs", "--lines", "5-6"])?;
    std::fs::remove_dir_all(&gone)?;
    insert(&app, 0, &["// a", "// b", "// c"])?;

    // The anchored lines, l4 and l5, are lines 3 and 4 in the other clone and 6 and 7 in the first.
    let resolved = &s.fwd(&other, &["anchor", "show", id(&anchor)?])?["resolved"];
    assert_eq!(resolved["range"], json!({"start_line": 3, "end_line": 5}), "{resolved}");
    Ok(())
}

/// `--also index` answers where the lines are in the index beside where they are in the working
/// tree, in one call: here lines in a staged change, a line above them added and not staged, so
/// that the two answers differ.
#[test]
fn anchor_show_also_answers_for_the_index_in_the_same_call() -> Result<()> {
    let s = Scratch::new()?;
    let app = s.project()?;
    std::fs::write(app.join("src/lib.rs"), LIB.replace("l3\n", "staged\n"))?;
    git(&app, &["add", "src/lib.rs"])?;
    insert(&app, 0, &["unstaged"])?;
    let anchor = s.made(&app, &["anchor", "create", "--path", "src/lib.rs", "--lines", "4-4"])?;

    let shown = s.fwd(&app, &["anchor", "show", id(&anchor)?, "--also", "index"])?;
    let (resolved, index) = (&shown["resolved"], &shown["also"]["index"]);
    assert_eq!(resolved["range"], json!({"start_line": 3, "end_line": 4}), "{resolved}");
    assert_eq!(index["target"]["kind"], "index", "{index}");
    assert_eq!(index["range"], json!({"start_line": 2, "end_line": 3}), "{index}");
    assert_eq!(shown["also"].as_object().map(|also| also.len()), Some(1), "only what was asked for");
    Ok(())
}

/// A comment made on the index's side of a diff (task Iz0HMyk0, "Let a comment, a question or a
/// step anchor lines on a revision's side of a diff"), in a file with one change staged and
/// another not, anchors the lines as the index has them: the anchor records the diff's side and
/// has the index's blob for its basis, and `fwd anchor show` finds the lines in the index where
/// they were anchored and in the working tree one line down. A question made on the right of the
/// staged changes and a step made on HEAD record their sides the same way.
#[test]
fn a_comment_made_against_the_index_anchors_the_lines_the_index_has() -> Result<()> {
    let s = Scratch::new()?;
    let app = s.project()?;
    std::fs::write(app.join("src/lib.rs"), LIB.replace("l3\n", "staged\n"))?;
    git(&app, &["add", "src/lib.rs"])?;
    insert(&app, 0, &["unstaged"])?;
    let blob = |rev: &str| -> Result<String> {
        Ok(git(&app, &["rev-parse", &format!("{rev}:src/lib.rs")])?.trim().to_string())
    };
    let anchor = |made: &Value| -> Result<String> {
        Ok(made["anchor"].as_str().ok_or_else(|| anyhow!("no anchor in {made}"))?.to_string())
    };

    let comment = s.made(&app, &["comment", "--path", "src/lib.rs", "--lines", "3-3", "--against", "index", "why?"])?;
    let shown = s.fwd(&app, &["anchor", "show", &anchor(&comment)?, "--against", "index"])?;
    let made = (&shown["side"], &shown["anchored_text"], &shown["basis_blob"]);
    assert_eq!(made, (&json!("base"), &json!("staged"), &json!(blob("")?)), "{shown}");
    assert_eq!(shown["resolved"]["label"], "exact", "{shown}");
    assert_eq!(shown["resolved"]["range"], json!({"start_line": 2, "end_line": 3}), "{shown}");
    let in_worktree = &s.fwd(&app, &["anchor", "show", &anchor(&comment)?, "--against", "worktree"])?["resolved"];
    assert_eq!(in_worktree["label"], "shifted", "{in_worktree}");
    assert_eq!(in_worktree["range"], json!({"start_line": 3, "end_line": 4}), "{in_worktree}");

    let on = ["--path", "src/lib.rs", "--lines", "4-4", "--against", "index", "--side", "modified"];
    let question = s.made(&app, &[&["ask", "and?"][..], &on].concat())?;
    let asked = s.fwd(&app, &["anchor", "show", &anchor(&question)?])?;
    assert_eq!((&asked["side"], &asked["anchored_text"]), (&json!("modified"), &json!("l4")), "{asked}");
    let review = s.made(&app, &["task", "create", "Review the staged change"])?;
    let task = review["id"].as_str().ok_or_else(|| anyhow!("no id in {review}"))?;
    let step = s.made(
        &app,
        &["step", "add", task, "--path", "src/lib.rs", "--lines", "3-3", "--against", "HEAD", "was l3 used?"],
    )?;
    let on_head = s.fwd(&app, &["anchor", "show", step["id"].as_str().unwrap_or_default(), "--against", "HEAD"])?;
    let made = (&on_head["side"], &on_head["anchored_text"], &on_head["basis_blob"]);
    assert_eq!(made, (&json!("base"), &json!("l3"), &json!(blob("HEAD")?)), "{on_head}");
    assert_eq!(on_head["resolved"]["label"], "exact", "{on_head}");
    Ok(())
}
