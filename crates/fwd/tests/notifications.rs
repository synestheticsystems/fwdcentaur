//! What `fwd reply` and `fwd forward` do to the notification they are given (task TdwRqMeQ, "Mark a
//! notification read when the agent replies to it, forwards it or dismisses it by its id, as the
//! primer says it does"): given a notification's id, they answer the message behind it or hand it
//! on, mark the notification read in the transaction that posts their message, so that acting on a
//! notification settles it, and name it beside `record` as `settled`; given a message's id, they
//! leave every notification as it is. Each test runs the CLI against a scratch daemon of its own.

use anyhow::{anyhow, Result};
use fwd_core::anchoring::git;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

/// A scratch directory holding the daemon's runtime and data directories and a checkout of a new
/// project, which the person `tester` joined and the agent `impl-a` named itself in. The daemon is
/// stopped when the test ends.
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
        git(&root, &["remote", "add", "origin", "git@example.com:t/settle.git"])?;
        let scratch = Scratch { dir, root };
        scratch.run(&["init", "--name", "settle", "--handle", "tester"])?;
        scratch.run(&["--as", "impl-a", "principal", "add", "impl-a", "--role", "implementer"])?;
        Ok(scratch)
    }

    /// What `fwd <args>` prints in the checkout, or its standard error when it fails.
    fn run(&self, args: &[&str]) -> Result<String> {
        let out = self.output(args)?;
        if !out.status.success() {
            return Err(anyhow!("fwd {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr)));
        }
        Ok(String::from_utf8(out.stdout)?)
    }

    /// `fwd <args>` run in the checkout, reaching this scratch daemon only, with none of the
    /// caller's settings that would choose another project, principal or scope.
    fn output(&self, args: &[&str]) -> Result<Output> {
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
        Ok(command.args(args).output()?)
    }

    /// The JSON `fwd --format json <args>` prints.
    fn json(&self, args: &[&str]) -> Result<Value> {
        Ok(serde_json::from_str(&self.run(&[&["--format", "json"][..], args].concat())?)?)
    }

    /// A question the person asks `impl-a`, by its id, and the id of the notification that tells
    /// `impl-a` of it, once the daemon's routing service has made it.
    fn question_for_the_agent(&self, text: &str) -> Result<(String, String)> {
        let asked = self.json(&["ask", "--to", "@impl-a", text])?;
        let question = asked["record"]["id"].as_str().ok_or_else(|| anyhow!("no id under record in {asked}"))?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let unread = self.json(&["--as", "impl-a", "inbox", "--unread"])?;
            let told = unread.as_array().into_iter().flatten().find(|n| n["message"]["id"] == question);
            if let Some(id) = told.and_then(|n| n["id"].as_str()) {
                return Ok((question.to_string(), id.to_string()));
            }
            if Instant::now() >= deadline {
                return Err(anyhow!("impl-a was not told of the question within 10s: {unread}"));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// The status of `impl-a`'s notification `id` (`unread` or `read`), or none once it is
    /// dismissed, since `fwd inbox` leaves a dismissed notification out.
    fn status(&self, id: &str) -> Result<Option<String>> {
        let inbox = self.json(&["--as", "impl-a", "inbox"])?;
        let found = inbox.as_array().into_iter().flatten().find(|n| n["id"] == id);
        Ok(found.map(|n| n["status"].as_str().unwrap_or_default().to_string()))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = self.run(&["daemon", "stop"]);
    }
}

/// Given a notification's id, or its first eight characters as the hooks print it, `fwd reply`
/// answers the message behind it, in its thread, and marks the notification read, naming it as
/// `settled`, while a reply refused for naming no one leaves it unread. A reply through a
/// notification read already is taken all the same, and a reply through one dismissed leaves it
/// dismissed, naming it as `settled` all the same (the lead's ruling in task TdwRqMeQ's thread,
/// 2026-09-30: a later answer in the thread does not undo the recipient's dismissal).
#[test]
fn a_reply_given_a_notification_id_settles_it_and_says_so() -> Result<()> {
    let s = Scratch::new()?;
    let (question, notification) = s.question_for_the_agent("which way?")?;
    assert_eq!(s.status(&notification)?.as_deref(), Some("unread"));
    let refused = s.run(&["--as", "impl-a", "reply", &notification, "ask @nobody"]);
    assert!(refused.is_err(), "a reply naming no one is refused: {refused:?}");
    assert_eq!(s.status(&notification)?.as_deref(), Some("unread"));

    let replied = s.json(&["--as", "impl-a", "reply", &notification[..8], "this way"])?;
    assert_eq!(replied["record"]["reply_to"], question, "{replied}");
    assert_eq!(replied["settled"], notification, "{replied}");
    assert_eq!(s.status(&notification)?.as_deref(), Some("read"));

    let again = s.json(&["--as", "impl-a", "reply", &notification, "and the other way on Sundays"])?;
    assert_eq!(again["settled"], notification, "{again}");
    assert_eq!(s.status(&notification)?.as_deref(), Some("read"));

    s.json(&["--as", "impl-a", "dismiss", &notification])?;
    let late = s.json(&["--as", "impl-a", "reply", &notification, "and never on holidays"])?;
    assert_eq!(late["settled"], notification, "{late}");
    assert_eq!(s.status(&notification)?, None, "the notification stays dismissed");
    Ok(())
}

/// Given a notification's id, `fwd forward` hands the message behind it on, in its thread, and
/// marks the notification read, naming it as `settled`.
#[test]
fn a_forward_given_a_notification_id_settles_it_and_says_so() -> Result<()> {
    let s = Scratch::new()?;
    let (question, notification) = s.question_for_the_agent("is Windows in scope?")?;

    let forwarded =
        s.json(&["--as", "impl-a", "forward", &notification, "--to", "@tester", "--note", "yours to rule"])?;
    assert_eq!(forwarded["record"]["reply_to"], question, "{forwarded}");
    assert_eq!(forwarded["settled"], notification, "{forwarded}");
    assert_eq!(s.status(&notification)?.as_deref(), Some("read"));
    Ok(())
}

/// Given the message's id instead, `fwd reply` and `fwd forward` answer it and hand it on as they
/// do through the notification, and leave the notification unread, with no `settled`: they settle
/// only a notification they are given, rather than guess which of the caller's notifications of
/// that message, if any, they act on.
#[test]
fn a_reply_or_a_forward_given_a_message_id_leaves_the_notification_as_it_is() -> Result<()> {
    let s = Scratch::new()?;
    let (question, notification) = s.question_for_the_agent("which way?")?;

    let replied = s.json(&["--as", "impl-a", "reply", &question, "this way"])?;
    assert_eq!(replied["record"]["reply_to"], question, "{replied}");
    assert_eq!(replied.get("settled"), None, "{replied}");
    let forwarded = s.json(&["--as", "impl-a", "forward", &question, "--to", "@tester", "--note", "yours to rule"])?;
    assert_eq!(forwarded["record"]["reply_to"], question, "{forwarded}");
    assert_eq!(forwarded.get("settled"), None, "{forwarded}");
    assert_eq!(s.status(&notification)?.as_deref(), Some("unread"));
    Ok(())
}

/// A notification's sentence ends with how to answer the message behind it for its recipient
/// alone (task 2guE1WsN), since `fwd reply` given a notification settles it, which is its
/// recipient's to do: `impl-a`'s inbox ends the question's sentence with `; reply with fwd reply`
/// and the notification's first eight characters, while the person reading `impl-a`'s inbox with
/// `--for` reads why the notification is there and nothing more.
#[test]
fn only_its_recipient_is_told_to_reply_through_a_notification() -> Result<()> {
    let s = Scratch::new()?;
    let (_, theirs) = s.question_for_the_agent("which way?")?;
    let reason = |args: &[&str]| -> Result<String> {
        let inbox = s.json(args)?;
        let found = inbox.as_array().into_iter().flatten().find(|n| n["id"] == theirs.as_str());
        let found = found.ok_or_else(|| anyhow!("{theirs} is not in {inbox}"))?;
        Ok(found["reason"].as_str().unwrap_or_default().to_string())
    };
    let own = reason(&["--as", "impl-a", "inbox"])?;
    let read_by_the_person = reason(&["inbox", "--for", "impl-a"])?;
    assert_eq!(own, format!("{read_by_the_person}; reply with fwd reply {}", &theirs[..8]));
    assert!(!read_by_the_person.contains("fwd reply"), "{read_by_the_person}");
    Ok(())
}

/// Only its recipient reads, dismisses, replies through or forwards a notification, as the page
/// marks no card of another principal's read (task AfhuiHYS): given a notification addressed to
/// another principal, `fwd read`, `fwd dismiss`, `fwd reply` and `fwd forward` refuse it, naming
/// whose it is, and change nothing, a read of several ids among them marking none; `reply` and
/// `forward` name the message's id, through which the message is answered without settling
/// anything. A person reading, dismissing and answering through their own notifications is
/// unaffected.
#[test]
fn only_its_recipient_reads_dismisses_replies_through_or_forwards_a_notification() -> Result<()> {
    let s = Scratch::new()?;
    let (question, theirs) = s.question_for_the_agent("which way?")?;
    // The person's own notification: a question the agent asks them.
    let asked = s.json(&["--as", "impl-a", "ask", "--to", "@tester", "and which day?"])?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let mine = loop {
        let unread = s.json(&["inbox", "--unread"])?;
        let told = unread.as_array().into_iter().flatten().find(|n| n["message"]["id"] == asked["record"]["id"]);
        if let Some(id) = told.and_then(|n| n["id"].as_str()) {
            break id.to_string();
        }
        assert!(Instant::now() < deadline, "the person was not told of the question within 10s: {unread}");
        std::thread::sleep(Duration::from_millis(100));
    };

    // The person is refused the agent's notification, each command naming whose it is.
    let whose = format!(
        "notification {} is @impl-a's, not @tester's, and a principal reads, dismisses, replies through or forwards only its own notifications",
        &theirs[..8]
    );
    for args in [
        vec!["read", mine.as_str(), theirs.as_str()],
        vec!["dismiss", theirs.as_str()],
        vec!["reply", theirs.as_str(), "this way"],
        vec!["forward", theirs.as_str(), "--to", "@tester", "--note", "mine now"],
    ] {
        let refused = s.run(&args).expect_err("refused").to_string();
        assert!(refused.contains(&whose), "{args:?}: {refused}");
        if matches!(args[0], "reply" | "forward") {
            let instead = format!("nothing was posted; to answer the message behind it without settling anything, give its id, {}, in place of the notification's", &question[..8]);
            assert!(refused.contains(&instead), "{args:?}: {refused}");
        }
    }
    assert_eq!(s.status(&theirs)?.as_deref(), Some("unread"), "nothing marked the agent's notification");
    let own = s.json(&["inbox", "--unread"])?;
    assert!(own.as_array().into_iter().flatten().any(|n| n["id"] == mine), "the read of both marked neither: {own}");
    let messages = s.json(&["message", "list"])?;
    let answers = messages.as_array().into_iter().flatten().filter(|m| m["reply_to"] == question).count();
    assert_eq!(answers, 0, "no reply or forward was posted: {messages}");

    // Through the message's id, the person answers it, and the agent's notification stays unread.
    let replied = s.json(&["reply", &question, "this way"])?;
    assert_eq!(replied.get("settled"), None, "{replied}");
    assert_eq!(s.status(&theirs)?.as_deref(), Some("unread"));

    // The person's own notification: read, dismissed and answered through as before.
    s.json(&["read", &mine])?;
    s.json(&["dismiss", &mine])?;
    let answered = s.json(&["reply", &mine, "on Monday"])?;
    assert_eq!(answered["settled"], mine, "{answered}");
    Ok(())
}

/// A comment written on lines before they become a step reaches the task's creator with the step
/// (task WTCAd9la): `impl-b` comments on a line of `lib.rs` and the person then adds it to the
/// checklist of a review `impl-a` made, and `impl-a`'s notification of the step lists the comment
/// beside the line, in the JSON's `listed` with its author and in the text after its author's
/// handle, since the sentence names the person, its sentence ending with how to answer it, which
/// `fwd reply` given the notification's id does, in the comment's thread, settling the
/// notification; `fwd forward` given the id of another such notification hands its one comment on.
/// A step added with two comments on its line names neither: `fwd reply`, `fwd forward` and `fwd
/// message edit` given its id are refused, naming both, rather than act on one and settle the
/// notification of both, the first two telling its recipient to dismiss it afterwards, and the
/// edit, which settles nothing, and the person, whose dismiss of it would be refused, not.
#[test]
fn a_step_added_lists_the_comments_on_its_lines_and_a_reply_answers_the_one_it_names() -> Result<()> {
    let s = Scratch::new()?;
    s.run(&["--as", "impl-b", "principal", "add", "impl-b", "--role", "implementer"])?;
    std::fs::write(s.root.join("lib.rs"), "l1\nl2\nl3\nl4\nl5\nl6\n")?;
    for args in [
        &["config", "user.email", "t@example.com"][..],
        &["config", "user.name", "t"],
        &["config", "commit.gpgsign", "false"],
        &["add", "lib.rs"],
        &["commit", "-q", "-m", "init"],
    ] {
        git(&s.root, args)?;
    }
    let id = |v: &Value| -> Result<String> {
        Ok(v["record"]["id"].as_str().ok_or_else(|| anyhow!("no id under record in {v}"))?.to_string())
    };
    let review = id(&s.json(&["--as", "impl-a", "task", "create", "Review: lib.rs"])?)?;
    // `impl-a`'s unread notification of a step added, once the daemon's routing service has made it.
    let step_added = || -> Result<Value> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let unread = s.json(&["--as", "impl-a", "inbox", "--unread"])?;
            if let Some(n) = unread.as_array().into_iter().flatten().find(|n| n["type"] == "step_added") {
                return Ok(n.clone());
            }
            if Instant::now() >= deadline {
                return Err(anyhow!("impl-a was not told of the step within 10s: {unread}"));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    };

    let on = |lines: &'static str| ["--path", "lib.rs", "--lines", lines];
    let comment = id(&s.json(&[&["--as", "impl-b", "comment"][..], &on("3-3"), &["this line is dead"]].concat())?)?;
    s.json(&[&["step", "add", review.as_str()][..], &on("3-3")].concat())?;
    let told = step_added()?;
    let notification = told["id"].as_str().unwrap_or_default().to_string();
    let listed = &told["listed"];
    assert_eq!(listed.as_array().map(Vec::len), Some(1), "{told}");
    assert_eq!((&listed[0]["step"]["name"], &listed[0]["comments"][0]["id"]), (&json!("lib.rs:3"), &json!(comment)));
    assert_eq!(listed[0]["comments"][0]["author_handle"], "impl-b", "{told}");
    assert_eq!(
        told["reason"],
        format!(
            "tester added a step with a comment to your task Review: lib.rs; reply with fwd reply {}",
            &notification[..8]
        )
    );
    let text = s.run(&["--as", "impl-a", "inbox", "--unread", "--format", "text"])?;
    assert!(
        text.contains("(step_added, id ") && text.ends_with("\n    [lib.rs:3] @impl-b: this line is dead\n"),
        "{text}"
    );
    let replied = s.json(&["--as", "impl-a", "reply", &notification, "taken out"])?;
    assert_eq!((&replied["record"]["reply_to"], &replied["settled"]), (&json!(comment), &json!(notification)));
    assert_eq!(s.status(&notification)?.as_deref(), Some("read"));

    let comment = id(&s.json(&[&["comment"][..], &on("4-4"), &["who owns this?"]].concat())?)?;
    s.json(&[&["step", "add", review.as_str()][..], &on("4-4")].concat())?;
    let told = step_added()?;
    let notification = told["id"].as_str().unwrap_or_default().to_string();
    let forwarded =
        s.json(&["--as", "impl-a", "forward", &notification, "--to", "@impl-b", "--note", "yours, I think"])?;
    assert_eq!((&forwarded["record"]["reply_to"], &forwarded["settled"]), (&json!(comment), &json!(notification)));
    assert_eq!(s.status(&notification)?.as_deref(), Some("read"));

    let first = id(&s.json(&[&["comment"][..], &on("5-5"), &["one"]].concat())?)?;
    let second = id(&s.json(&[&["comment"][..], &on("5-5"), &["two"]].concat())?)?;
    s.json(&[&["step", "add", review.as_str()][..], &on("5-5")].concat())?;
    let told = step_added()?;
    let notification = told["id"].as_str().unwrap_or_default().to_string();
    let comments: Vec<&Value> =
        told["listed"][0]["comments"].as_array().into_iter().flatten().map(|m| &m["id"]).collect();
    assert_eq!(comments, vec![&json!(first), &json!(second)], "{told}");
    assert_eq!(told["reason"], "tester added a step with 2 comments to your task Review: lib.rs");
    let names = format!(
        "that notification lists 2 messages ({}, {}) and names none of them as the one to act on: give the id of each message you act on in place of the notification's",
        &first[..8],
        &second[..8]
    );
    let dismiss = format!("then fwd dismiss {}", &notification[..8]);
    let refusal = |args: &[&str]| s.run(args).expect_err("refused").to_string();
    for args in [
        vec!["--as", "impl-a", "reply", notification.as_str(), "both gone"],
        vec!["--as", "impl-a", "forward", notification.as_str(), "--to", "@impl-b", "--note", "yours"],
    ] {
        let refused = refusal(&args);
        assert!(refused.contains(&names) && refused.contains(&dismiss), "{args:?}: {refused}");
    }
    for args in [
        vec!["--as", "impl-a", "message", "edit", notification.as_str(), "three"],
        vec!["reply", notification.as_str(), "not yours to settle"],
    ] {
        let refused = refusal(&args);
        assert!(refused.contains(&names) && !refused.contains("dismiss"), "{args:?}: {refused}");
    }
    assert_eq!(s.status(&notification)?.as_deref(), Some("unread"), "the refusals settle nothing");
    Ok(())
}

/// `fwd inbox pop` marks what it prints read, so it pops only the inbox of the principal the
/// command acts as, as `fwd read` marks only its own notifications (task AfhuiHYS): `--for` naming
/// another principal is refused before anything is printed, naming whose inbox it is, and that
/// inbox stays unread. The person popping their own inbox and an agent popping its own, `--for`
/// naming itself, print their notifications and mark them read as before.
#[test]
fn inbox_pop_pops_only_the_inbox_of_the_principal_it_acts_as() -> Result<()> {
    let s = Scratch::new()?;
    let (question, theirs) = s.question_for_the_agent("which way?")?;

    // The person is refused the agent's inbox: nothing is printed, and nothing is marked.
    let refused = s.output(&["--format", "json", "inbox", "--for", "impl-a", "pop"])?;
    assert!(!refused.status.success(), "the pop of another's inbox ran");
    assert_eq!(String::from_utf8_lossy(&refused.stdout), "", "nothing is printed");
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(
        said.contains("`--for impl-a` names @impl-a's inbox, not @tester's, and `fwd inbox pop` marks what it prints read, which a principal does only to its own notifications")
            && said.contains("nothing was printed or marked; `fwd inbox --for impl-a` lists that inbox and marks nothing"),
        "{said}"
    );
    assert_eq!(s.status(&theirs)?.as_deref(), Some("unread"), "the agent's notification stays unread");

    // The agent pops its own inbox, named by --for too, and its notification is read.
    let popped = s.json(&["--as", "impl-a", "inbox", "--for", "impl-a", "pop"])?;
    let popped: Vec<&Value> = popped.as_array().into_iter().flatten().collect();
    assert_eq!(popped.len(), 1, "{popped:?}");
    assert_eq!(popped[0]["message"]["id"], question.as_str());
    assert_eq!(s.status(&theirs)?.as_deref(), Some("read"));

    // The person pops their own: the question the agent asks them.
    let asked = s.json(&["--as", "impl-a", "ask", "--to", "@tester", "and which day?"])?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while s.json(&["inbox", "--unread"])?.as_array().is_none_or(Vec::is_empty) {
        assert!(Instant::now() < deadline, "the person was not told of the question within 10s");
        std::thread::sleep(Duration::from_millis(100));
    }
    let popped = s.json(&["inbox", "pop"])?;
    let popped: Vec<&Value> = popped.as_array().into_iter().flatten().collect();
    assert_eq!(popped.len(), 1, "{popped:?}");
    assert_eq!(popped[0]["message"]["id"], asked["record"]["id"]);
    assert_eq!(s.json(&["inbox", "--unread"])?, Value::Array(Vec::new()), "the pop marked it read");
    Ok(())
}
