//! What the CLI says when it lists fewer tasks than there are, so that a reader can tell a cut
//! list from a whole one: `fwd task list` names how many it showed of how many on standard error
//! when `--limit` cut the list, its JSON staying a plain array, and `fwd prime`, which lists the
//! first ten ready tasks, counts every one in its hook's text and says on standard error otherwise
//! that it cut the list. Each test runs the CLI against a scratch daemon of its own.

use anyhow::{anyhow, Result};
use fwd_core::anchoring::git;
use serde_json::Value;
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
        git(&root, &["remote", "add", "origin", "git@example.com:t/list.git"])?;
        let scratch = Scratch { dir, root };
        scratch.saying(&["init", "--name", "list", "--handle", "tester"])?;
        Ok(scratch)
    }

    /// `fwd <args>` in the checkout, reaching this scratch daemon only, with none of the caller's
    /// settings that would choose another project, principal or scope.
    fn command(&self, args: &[&str]) -> Command {
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
        command.args(args);
        command
    }

    /// What `fwd <args>` prints and what it says on standard error.
    fn saying(&self, args: &[&str]) -> Result<(String, String)> {
        let out = self.command(args).output()?;
        if !out.status.success() {
            return Err(anyhow!("fwd {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr)));
        }
        Ok((String::from_utf8(out.stdout)?, String::from_utf8(out.stderr)?))
    }

    /// What `fwd <args>` prints and says, in the order it wrote them, as a terminal shows them:
    /// both streams go to one file.
    fn as_a_terminal_shows(&self, args: &[&str]) -> Result<String> {
        let path = self.dir.path().join("both.txt");
        let both = std::fs::File::create(&path)?;
        let status = self.command(args).stdout(both.try_clone()?).stderr(both).status()?;
        if !status.success() {
            return Err(anyhow!("fwd {} failed", args.join(" ")));
        }
        Ok(std::fs::read_to_string(&path)?)
    }

    /// The JSON `fwd --format json <args>` prints, and what it says on standard error.
    fn json_saying(&self, args: &[&str]) -> Result<(Value, String)> {
        let (printed, said) = self.saying(&[&["--format", "json"][..], args].concat())?;
        Ok((serde_json::from_str(&printed)?, said))
    }

    /// Files a task titled `title`, with `args` after the title.
    fn file(&self, title: &str, args: &[&str]) -> Result<()> {
        self.json_saying(&[&["task", "create", title][..], args].concat())?;
        Ok(())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = self.saying(&["daemon", "stop"]);
    }
}

/// What `fwd task list` says on standard error when `--limit` cut it to `shown` of `of` tasks,
/// naming `filters` to narrow it with.
fn cut_note(shown: usize, of: usize, filters: &str) -> String {
    format!("fwd: {shown} of {of} tasks shown; raise --limit, or narrow with {filters}\n")
}

/// The filters the note names, and those it names after `--ready`, which reads no status.
const FILTERS: &str = "--status, --ready, --label or --mine";
const AFTER_READY: &str = "--label or --mine";

/// A list `--limit` cut says on standard error how many of how many it shows, counting the tasks
/// the filters keep before the cut, and names the filters that narrow it, leaving `--status` out
/// after `--ready`; the JSON stays the array of the tasks shown, and the note comes after it. A
/// list the limit did not cut, however near it came, says nothing.
#[test]
fn a_list_the_limit_cut_says_how_many_it_shows_of_how_many() -> Result<()> {
    let s = Scratch::new()?;
    s.file("Retry on disconnect", &["--label", "bug"])?;
    s.file("Cap the backoff", &["--label", "bug"])?;
    s.file("Write the primer", &[])?;

    let (listed, said) = s.json_saying(&["task", "list", "--limit", "2"])?;
    assert_eq!(listed.as_array().map(Vec::len), Some(2), "{listed}");
    assert_eq!(said, cut_note(2, 3, FILTERS));
    let (_, said) = s.json_saying(&["task", "list", "--label", "bug", "--limit", "1"])?;
    assert_eq!(said, cut_note(1, 2, FILTERS), "the count is of the tasks the filters keep");
    let (_, said) = s.json_saying(&["task", "list", "--ready", "--limit", "1"])?;
    assert_eq!(said, cut_note(1, 3, AFTER_READY));

    let shown = s.as_a_terminal_shows(&["--format", "text", "task", "list", "--limit", "2"])?;
    assert!(shown.ends_with(&cut_note(2, 3, FILTERS)), "the note comes after the list: {shown}");

    for args in
        [&["task", "list", "--limit", "3"][..], &["task", "list"], &["task", "list", "--label", "bug", "--limit", "2"]]
    {
        let (listed, said) = s.json_saying(args)?;
        assert_eq!(said, "", "{args:?} cut nothing: {listed}");
    }
    Ok(())
}

/// `fwd prime --format hook`, which puts its text into an agent's context, counts every ready task,
/// as it counts every unread notification, while it lists the first ten after `Ready:`; without
/// the hook's format it lists ten and says after them on standard error that it cut the list, and
/// where the rest is. Given no hook's JSON, the command acts for the person, for whom every task
/// here is ready.
#[test]
fn prime_counts_every_ready_task_and_lists_ten() -> Result<()> {
    let s = Scratch::new()?;
    for n in 1..=12 {
        s.file(&format!("Task {n}"), &[])?;
    }
    let (primed, said) = s.json_saying(&["prime"])?;
    assert_eq!(primed["ready_tasks"].as_array().map(Vec::len), Some(10), "{primed}");
    assert_eq!(said, "fwd: 10 of 12 ready tasks shown; `fwd task list --ready --limit 12` lists the rest\n");

    let (primed, said) = s.saying(&["prime", "--format", "hook"])?;
    assert_eq!(said, "", "the hook's text counts them instead");
    assert!(
        primed.starts_with("fwdcentaur context for tester: 12 ready task(s), 0 unread notification(s).\n"),
        "{primed}"
    );
    let listed = primed
        .lines()
        .find_map(|line| line.strip_prefix("Ready: "))
        .ok_or_else(|| anyhow!("no Ready line in {primed}"))?;
    assert_eq!(listed.split("; ").count(), 10, "{listed}");
    Ok(())
}
