//! A task's not-before date through the CLI: what `--not-before` on `fwd task create` and `fwd task
//! amend` stores, clears and refuses, how `fwd task show` prints it, and what it keeps a task out
//! of. Each test runs the CLI against a scratch daemon of its own.

use anyhow::{anyhow, Result};
use fwd_core::anchoring::git;
use fwd_core::decision_log::parse_date;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::{Command, Output};

/// POSIX time zones, which need no time zone database on the machine running the test: eight hours
/// west of UTC, ten hours east of it, and UTC.
const WEST: &str = "XXX+8";
const EAST: &str = "XXX-10";
const UTC: &str = "UTC0";

/// POSIX rules for clocks that change, which need no database either: ten hours east of UTC, going
/// forward at 02:00 on the first Sunday of October; and five hours west, going forward at midnight
/// on the second Sunday of March.
const FORWARD_AT_TWO: &str = "AEST-10AEDT,M10.1.0,M4.1.0/3";
const FORWARD_AT_MIDNIGHT: &str = "CST5CDT,M3.2.0/0,M11.1.0/1";

/// A day far enough ahead that it has not come, and one long gone.
const AHEAD: &str = "2099-01-01";
const GONE: &str = "2000-01-01";

/// The instant `hour` o'clock UTC on `day`, as a task stores it.
fn utc(day: &str, hour: i64) -> i64 {
    parse_date(day).expect("a day of the calendar") + (hour - 12) * 3_600_000
}

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
        git(&root, &["remote", "add", "origin", "git@example.com:t/not-before.git"])?;
        let scratch = Scratch { dir, root };
        scratch.run(&["init", "--name", "not-before", "--handle", "tester"])?;
        Ok(scratch)
    }

    /// `fwd --format <format> <args>` in the checkout, in the time zone `zone` (the machine's with
    /// none), reaching this scratch daemon only, with none of the caller's settings that would
    /// choose another project, principal or scope.
    fn output(&self, zone: Option<&str>, format: &str, args: &[&str]) -> Result<Output> {
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
        if let Some(zone) = zone {
            command.env("TZ", zone);
        }
        Ok(command.args(["--format", format]).args(args).output()?)
    }

    /// The standard output of `fwd --format <format> <args>`, in the time zone `zone`.
    fn printed(&self, zone: Option<&str>, format: &str, args: &[&str]) -> Result<String> {
        let out = self.output(zone, format, args)?;
        if !out.status.success() {
            return Err(anyhow!("fwd {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr)));
        }
        Ok(String::from_utf8(out.stdout)?)
    }

    /// The JSON `fwd <args>` prints.
    fn run(&self, args: &[&str]) -> Result<Value> {
        Ok(serde_json::from_str(&self.printed(None, "json", args)?)?)
    }

    /// What refuses `fwd <args>`, which must fail.
    fn refusal(&self, args: &[&str]) -> Result<String> {
        let out = self.output(None, "json", args)?;
        if out.status.success() {
            return Err(anyhow!("fwd {} was let through", args.join(" ")));
        }
        Ok(String::from_utf8(out.stderr)?)
    }

    /// Files a task with `args` after its title, in the time zone `zone`: its id, and what the
    /// command printed of it.
    fn file(&self, zone: Option<&str>, args: &[&str]) -> Result<(String, Value)> {
        let made: Value =
            serde_json::from_str(&self.printed(zone, "json", &[&["task", "create", "Tidy up"][..], args].concat())?)?;
        let id = made["record"]["id"].as_str().ok_or_else(|| anyhow!("no id under record in {made}"))?.to_string();
        Ok((id, made["record"].clone()))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = self.run(&["daemon", "stop"]);
    }
}

/// So that a Mac, a Pi and a hosted page agree on when a task comes due, a day means its start where
/// the command ran, and the task stores that instant: the same day typed in two time zones is two
/// instants, and the one instant reads back as the day it was typed as there, and as the hour it
/// comes elsewhere.
#[test]
fn a_day_is_stored_as_the_instant_it_starts_where_the_command_ran() -> Result<()> {
    let s = Scratch::new()?;
    let (west, printed) = s.file(Some(WEST), &["--not-before", "2030-06-15"])?;
    assert_eq!(printed["not_before"], json!(utc("2030-06-15", 8)), "midnight eight hours west of UTC");
    let (_, printed) = s.file(Some(EAST), &["--not-before", "2030-06-15"])?;
    assert_eq!(printed["not_before"], json!(utc("2030-06-14", 14)), "midnight ten hours east of UTC");

    let shown_in = |zone| s.printed(Some(zone), "text", &["task", "show", &west]);
    let read_there = shown_in(WEST)?;
    assert!(read_there.lines().any(|l| l.ends_with("not before 2030-06-15")), "{read_there}");
    let read_elsewhere = shown_in(UTC)?;
    assert!(read_elsewhere.lines().any(|l| l.ends_with("not before 2030-06-15 08:00")), "{read_elsewhere}");
    Ok(())
}

/// A day the clocks change on still starts at its first instant on the machine's own clock: this runs
/// the real `localtime` across a change, where the tests in `not_before.rs` make the offsets up.
#[test]
fn a_day_the_clocks_change_on_is_stored_as_the_instant_it_starts_there() -> Result<()> {
    let s = Scratch::new()?;
    let (_, printed) = s.file(Some(FORWARD_AT_TWO), &["--not-before", "2026-10-04"])?;
    assert_eq!(printed["not_before"], json!(utc("2026-10-03", 14)), "its midnight comes before the change");
    let (_, printed) = s.file(Some(FORWARD_AT_MIDNIGHT), &["--not-before", "2026-03-08"])?;
    assert_eq!(printed["not_before"], json!(utc("2026-03-08", 5)), "it has no midnight, and starts at 01:00");
    Ok(())
}

/// An RFC 3339 instant is the instant it names, whatever time zone the command runs in.
#[test]
fn an_instant_is_stored_as_it_is_given_in_any_time_zone() -> Result<()> {
    let s = Scratch::new()?;
    for zone in [WEST, EAST] {
        let (_, printed) = s.file(Some(zone), &["--not-before", "2030-06-15T09:30:00-07:00"])?;
        assert_eq!(printed["not_before"], json!(utc("2030-06-15", 16) + 30 * 60_000), "in {zone}");
    }
    Ok(())
}

/// `fwd task amend` gives a task a date, moves it and takes it off, and `fwd task show` says so each
/// time; a task never given one prints `null` and no words for it.
#[test]
fn amend_gives_a_task_a_date_moves_it_and_clears_it() -> Result<()> {
    let s = Scratch::new()?;
    let (id, printed) = s.file(None, &[])?;
    assert_eq!(printed["not_before"], Value::Null);
    let not_before = |args: &[&str]| -> Result<Value> {
        s.run(&[&["task", "amend", &id][..], args].concat())?;
        Ok(s.run(&["task", "show", &id])?["not_before"].clone())
    };
    assert_eq!(not_before(&["--not-before", "2030-06-15T00:00:00Z"])?, json!(utc("2030-06-15", 0)));
    assert_eq!(not_before(&["--not-before", "2031-06-15T00:00:00Z"])?, json!(utc("2031-06-15", 0)));
    assert_eq!(not_before(&["--title", "Tidy up more"])?, json!(utc("2031-06-15", 0)), "another field leaves it be");
    assert_eq!(not_before(&["--clear-not-before"])?, Value::Null);
    let text = s.printed(None, "text", &["task", "show", &id])?;
    assert!(!text.contains("not before"), "{text}");
    Ok(())
}

/// A bad date is refused with the sentence saying what it wants, before anything is written: no task
/// is filed, and the date of one already filed stays.
#[test]
fn a_bad_date_is_refused_saying_what_it_wants_and_changes_nothing() -> Result<()> {
    let s = Scratch::new()?;
    let wants = "a not-before date is YYYY-MM-DD";
    let refusal = s.refusal(&["task", "create", "Tidy up", "--not-before", "soon"])?;
    assert!(refusal.contains(wants), "{refusal}");
    assert_eq!(s.run(&["task", "list"])?, json!([]), "nothing filed");

    let (id, printed) = s.file(None, &["--not-before", AHEAD])?;
    let refusal = s.refusal(&["task", "amend", &id, "--not-before", "2030-02-30"])?;
    assert!(refusal.contains(wants), "{refusal}");
    assert_eq!(s.run(&["task", "show", &id])?["not_before"], printed["not_before"], "the date stays");
    Ok(())
}

/// Until its date a task stays out of `fwd task next` and `fwd task list --ready`, and says why; once
/// the date has passed, or its date is taken off, it is ready as any other.
#[test]
fn a_task_not_yet_due_stays_out_of_what_is_ready_until_its_date() -> Result<()> {
    let s = Scratch::new()?;
    let (id, printed) = s.file(None, &["--not-before", AHEAD])?;
    let date = printed["not_before"].as_i64().ok_or_else(|| anyhow!("no date in {printed}"))?;
    let readiness = |id: &str| -> Result<Value> { Ok(s.run(&["task", "show", id])?["readiness"].clone()) };
    assert_eq!(
        readiness(&id)?,
        json!({"state": "not_ready", "reasons": [format!("NotYetDue({date})")]}),
        "it says why"
    );
    assert_eq!(s.run(&["task", "next"])?["task"], Value::Null);
    assert_eq!(s.run(&["task", "list", "--ready"])?, json!([]));
    assert_eq!(s.run(&["task", "list"])?.as_array().map(Vec::len), Some(1), "it is still listed among the open");

    s.run(&["task", "amend", &id, "--not-before", GONE])?;
    assert_eq!(readiness(&id)?, json!({"state": "ready"}));
    assert_eq!(s.run(&["task", "next"])?["id"], json!(id));

    s.run(&["task", "amend", &id, "--not-before", AHEAD])?;
    s.run(&["task", "amend", &id, "--clear-not-before"])?;
    assert_eq!(readiness(&id)?, json!({"state": "ready"}));
    Ok(())
}

/// Nothing refuses an early start: whoever wants to start a task not yet due does, and from then on
/// it behaves as any started task, its date only printed.
#[test]
fn a_task_not_yet_due_can_be_started_early() -> Result<()> {
    let s = Scratch::new()?;
    let (id, printed) = s.file(None, &["--not-before", AHEAD])?;
    let started = s.run(&["task", "start", &id])?;
    assert_eq!(started["status"], json!("in_progress"));
    assert_eq!(started["not_before"], printed["not_before"], "the date stays on the task");
    assert_eq!(started["readiness"], json!({"state": "not_ready", "reasons": ["Status(InProgress)"]}));
    Ok(())
}
