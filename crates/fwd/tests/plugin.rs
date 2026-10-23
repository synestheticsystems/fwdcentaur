//! The fwd plugin for Claude Code (integrations/claude-code), run as Claude Code runs it: its hook
//! script is given a PreToolUse hook's JSON on standard input, with this build's fwd on the PATH,
//! and what it prints is the answer Claude Code reads; the command it stamps then runs in a shell,
//! against a scratch daemon, and acts as the agent that ran it (docs/design/agents-and-inbox.md,
//! "An agent's identity"). The grammar of what it stamps is tested beside it, in
//! crates/fwd/src/claude_code.rs, and binding and the subject's precedence in fwd-core's
//! tests/agents.rs; these tests hold the script, the manifests and fwd together. The inbox hooks
//! (integrations/claude-code/settings.example.json) are run the same way, given their event's JSON
//! on standard input, and act for the agent whose hook fired.

use anyhow::{anyhow, Result};
use fwd_core::anchoring::git;
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The Claude Code session every hook input here names.
const SESSION: &str = "56693b92-71ca-49ec-8ab4-aee1de36a4d3";

/// The fwdcentaur checkout this test runs in.
fn checkout() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("the checkout")
}

fn plugin() -> PathBuf {
    checkout().join("integrations/claude-code")
}

/// A PreToolUse hook's input for a Bash command, as Claude Code gives it: `agent_id` only inside a
/// subagent.
fn hook_input(command: &str, agent: Option<&str>) -> String {
    let mut v = json!({
        "session_id": SESSION,
        "transcript_path": "/tmp/transcript.jsonl",
        "cwd": "/tmp",
        "permission_mode": "auto",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {"command": command, "description": "a command", "timeout": 120000, "run_in_background": true},
        "tool_use_id": "toolu_01",
    });
    if let Some(agent) = agent {
        v["agent_id"] = json!(agent);
        v["agent_type"] = json!("general-purpose");
    }
    v.to_string()
}

/// Runs the plugin's hook script with `input` on standard input and `path` as its PATH: its exit
/// code and what it printed. The script may exit before it reads its input, as it does when fwd is
/// not on the PATH, and writing the input to the pipe it closed then fails with a broken pipe
/// (whether it does depends on which of the two gets there first, and failed on Linux on CI, run
/// 36699690439): that is the script's answer, not an error, so only its exit and its output count.
fn run_hook(input: &str, path: &str) -> (i32, String) {
    let mut child = Command::new(plugin().join("hooks/stamp"))
        .env_clear()
        .env("PATH", path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the hook script runs");
    if let Err(e) = child.stdin.take().expect("stdin").write_all(input.as_bytes()) {
        assert_eq!(e.kind(), std::io::ErrorKind::BrokenPipe, "the input is written: {e}");
    }
    let out = child.wait_with_output().expect("the hook script ends");
    (out.status.code().unwrap_or(-1), String::from_utf8(out.stdout).expect("utf-8"))
}

/// A PATH holding this build's fwd, and the system's tools the script uses.
fn path_with_fwd() -> String {
    let bin = Path::new(env!("CARGO_BIN_EXE_fwd")).parent().expect("fwd's directory").to_path_buf();
    format!("{}:/usr/bin:/bin", bin.display())
}

#[test]
fn the_hook_stamps_a_command_of_fwd_alone_with_the_session_and_the_agent_and_approves_it() {
    let (code, out) =
        run_hook(&hook_input("fwd task show LK12MGks | head -40", Some("a7b8a4211fe9f76b7")), &path_with_fwd());
    assert_eq!(code, 0);
    let answer: Value = serde_json::from_str(out.trim()).expect("the hook prints one JSON object");
    assert_eq!(
        answer,
        json!({"hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "updatedInput": {
                "command": "export FWD_CLAUDE_SESSION='56693b92-71ca-49ec-8ab4-aee1de36a4d3' FWD_CLAUDE_AGENT='a7b8a4211fe9f76b7'; fwd task show LK12MGks | head -40",
                "description": "a command",
                "timeout": 120000,
                "run_in_background": true,
            },
        }}),
        "only the command is replaced, so the tool's other arguments stay as they were"
    );
    // The session's own thread has no agent id in the hook's input: it is `main`.
    let (_, out) = run_hook(&hook_input("fwd whoami", None), &path_with_fwd());
    let answer: Value = serde_json::from_str(out.trim()).expect("JSON");
    assert_eq!(
        answer["hookSpecificOutput"]["updatedInput"]["command"],
        "export FWD_CLAUDE_SESSION='56693b92-71ca-49ec-8ab4-aee1de36a4d3' FWD_CLAUDE_AGENT='main'; fwd whoami"
    );
}

#[test]
fn the_hook_leaves_every_other_command_alone_and_never_blocks_one() {
    for command in [
        "cargo test --workspace && fwd task submit X --evidence green",
        "fwd comment X \"$(cat notes.md)\"",
        "fwd task list > tasks.json",
        "fwd admin migrate",
        "fwd daemon stop",
    ] {
        let (code, out) = run_hook(&hook_input(command, Some("a1")), &path_with_fwd());
        assert_eq!((code, out.as_str()), (0, ""), "left alone, to the permission flow: {command}");
    }
    // Without fwd on the PATH the hook prints nothing and exits 0: a hook that exits 2 would block
    // the command, and the plugin runs before every Bash command on the machine.
    let (code, out) = run_hook(&hook_input("fwd whoami", None), "/usr/bin:/bin");
    assert_eq!((code, out.as_str()), (0, ""));
    // Nor does input that is no hook's.
    let (code, out) = run_hook("not json", &path_with_fwd());
    assert_eq!((code, out.as_str()), (0, ""));
}

#[test]
fn the_manifests_name_the_plugin_its_hook_and_its_skills() {
    let read = |path: PathBuf| -> Value {
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
            .expect("JSON")
    };
    let manifest = read(plugin().join(".claude-plugin/plugin.json"));
    assert_eq!(manifest["name"], "fwd", "the skills are fwd:fwd-lead and the rest, and the install id fwd@fwdcentaur");
    let marketplace = read(checkout().join(".claude-plugin/marketplace.json"));
    assert_eq!(marketplace["name"], "fwdcentaur");
    assert_eq!(marketplace["plugins"][0]["name"], manifest["name"], "the entry's name is the manifest's");
    assert_eq!(marketplace["plugins"][0]["source"], "./integrations/claude-code");
    let hooks = read(plugin().join("hooks/hooks.json"));
    let bash = &hooks["hooks"]["PreToolUse"][0];
    assert_eq!(bash["matcher"], "Bash");
    assert_eq!(bash["hooks"][0]["command"], "${CLAUDE_PLUGIN_ROOT}/hooks/stamp");
    for skill in ["fwd-lead", "fwd-triage", "fwd-implementer"] {
        assert!(plugin().join("skills").join(skill).join("SKILL.md").is_file(), "{skill}");
    }
}

/// The command the hook gives Claude Code to run in place of `command`, for `agent` (none for the
/// session's own thread): it must be one the hook stamps.
fn stamped(command: &str, agent: Option<&str>) -> String {
    let (code, out) = run_hook(&hook_input(command, agent), &path_with_fwd());
    assert_eq!(code, 0);
    let answer: Value = serde_json::from_str(out.trim()).unwrap_or_else(|_| panic!("not stamped: {command}"));
    answer["hookSpecificOutput"]["updatedInput"]["command"].as_str().expect("a command").to_string()
}

/// A scratch directory holding a daemon's runtime and data directories and a checkout of a new
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
        git(&root, &["remote", "add", "origin", "git@example.com:t/agents.git"])?;
        let scratch = Scratch { dir, root };
        let out = scratch.program("fwd").args(["init", "--name", "agents", "--handle", "tester"]).output()?;
        assert!(out.status.success(), "fwd init: {}", String::from_utf8_lossy(&out.stderr));
        Ok(scratch)
    }

    /// `program` in the checkout, reaching this scratch daemon only, with this build's fwd first on
    /// the PATH and none of the caller's settings that would choose a project, a principal or an
    /// agent: no FWD_AS, no stamp, and no Claude Code session unless a test gives one.
    fn program(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(&self.root)
            .env("PATH", path_with_fwd())
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
        command
    }

    /// Runs `line` in a shell, as Claude Code runs a Bash command, with Claude Code's session id in
    /// its environment as Claude Code sets it, and `env` besides: its JSON, or its standard error
    /// when it fails.
    fn shell(&self, line: &str, env: &[(&str, &str)]) -> Result<Value, String> {
        let mut command = self.program("sh");
        command.args(["-c", line]).env("CLAUDE_CODE_SESSION_ID", SESSION);
        for (name, value) in env {
            command.env(name, value);
        }
        let out = command.output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).into_owned());
        }
        serde_json::from_slice(&out.stdout).map_err(|e| format!("{e}: {}", String::from_utf8_lossy(&out.stdout)))
    }

    /// Runs `line` as [`Scratch::shell`] does, and gives its JSON with what it said on standard
    /// error, for a command that succeeds and may say something beside its answer.
    fn shell_saying(&self, line: &str, env: &[(&str, &str)]) -> Result<(Value, String)> {
        let mut command = self.program("sh");
        command.args(["-c", line]).env("CLAUDE_CODE_SESSION_ID", SESSION);
        for (name, value) in env {
            command.env(name, value);
        }
        let out = command.output()?;
        let said = String::from_utf8_lossy(&out.stderr).into_owned();
        if !out.status.success() {
            return Err(anyhow!("{line}: {said}"));
        }
        Ok((serde_json::from_slice(&out.stdout)?, said))
    }

    /// Runs the hook command `line` as Claude Code runs an inbox hook: in a shell with the id of the
    /// session `input` names in its environment, and `fwd_as`, when given, as FWD_AS, as a settings
    /// file's `env` block gives it to every command of the session, with `input`, the hook's JSON, on
    /// its standard input. What it printed, or its standard error when it fails.
    fn hook(&self, line: &str, input: &Value, fwd_as: Option<&str>) -> Result<String, String> {
        let mut command = self.program("sh");
        command.args(["-c", line]).env("CLAUDE_CODE_SESSION_ID", input["session_id"].as_str().expect("a session"));
        if let Some(name) = fwd_as {
            command.env("FWD_AS", name);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        child.stdin.take().expect("stdin").write_all(input.to_string().as_bytes()).map_err(|e| e.to_string())?;
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).into_owned());
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// How many unread notifications the principal `who` has, read from outside any Claude Code
    /// session, as `--as` names it.
    fn unread(&self, who: &str) -> usize {
        let out = self.program("fwd").args(["--as", who, "inbox", "--unread", "--format", "json"]).output();
        let listed: Option<Value> = out.ok().and_then(|o| serde_json::from_slice(&o.stdout).ok());
        listed.and_then(|v| v.as_array().map(Vec::len)).unwrap_or(0)
    }
}

/// The JSON Claude Code hands the hook of `event` on standard input, fired in the own thread of the
/// session `session` or, given `agent`, inside that subagent, which Claude Code names with
/// `agent_id` and `agent_type`.
fn event_input(session: &str, event: &str, agent: Option<&str>) -> Value {
    let mut v = json!({
        "session_id": session,
        "transcript_path": "/tmp/transcript.jsonl",
        "cwd": "/tmp",
        "permission_mode": "auto",
        "hook_event_name": event,
    });
    if let Some(agent) = agent {
        v["agent_id"] = json!(agent);
        v["agent_type"] = json!("general-purpose");
    }
    v
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = self.program("fwd").args(["daemon", "stop"]).output();
    }
}

#[test]
fn a_stamped_command_acts_as_the_agent_that_named_itself_and_an_unstamped_write_needs_a_typed_as() -> Result<()> {
    let scratch = Scratch::new()?;
    // A subagent's first command, stamped: it acts as no principal until it names itself.
    let whoami = scratch.shell(&stamped("fwd whoami", Some("a1")), &[]).map_err(|e| anyhow!(e))?;
    assert_eq!(whoami["subject"], format!("claude-code:{SESSION}/a1"));
    assert_eq!(whoami["principal"], Value::Null);
    let first = scratch.shell(&stamped("fwd task create 'Retry on disconnect'", Some("a1")), &[]).unwrap_err();
    assert!(first.contains("which has not named itself in this project"), "{first}");
    // It names itself, and is bound: its commands act as it from then on, with no --as.
    let named = scratch
        .shell(&stamped("fwd principal add impl-a1 --role implementer", Some("a1")), &[])
        .map_err(|e| anyhow!(e))?;
    let named = &named["record"];
    assert_eq!(named["subject"], format!("claude-code:{SESSION}/a1"));
    let whoami = scratch.shell(&stamped("fwd whoami", Some("a1")), &[]).map_err(|e| anyhow!(e))?;
    assert_eq!(whoami["principal"]["handle"], "impl-a1");
    // FWD_AS, which a settings file's env block gives every agent of the session, is not read.
    let task = scratch
        .shell(&stamped("fwd task create 'Retry on disconnect'", Some("a1")), &[("FWD_AS", "tester")])
        .map_err(|e| anyhow!(e))?;
    assert_eq!(task["record"]["created_by"], named["id"], "made by the agent, not by FWD_AS's principal");
    // A typed --as naming another principal is refused.
    let other = scratch.shell(&stamped("fwd --as tester task create 'Other'", Some("a1")), &[]).unwrap_err();
    assert!(other.contains("`--as tester` asks to act as another principal"), "{other}");
    // The session's own thread is another agent, which has not named itself: its reads about the
    // graph alone run, for no one.
    let main = scratch.shell(&stamped("fwd task list", None), &[]).map_err(|e| anyhow!(e))?;
    assert!(main.is_array(), "a read about the graph alone runs: {main}");
    // A write in a command the hook left alone, in the session one of whose agents is bound, is
    // refused, since fwd cannot tell which agent ran it; a read about the graph alone runs.
    let unstamped = scratch.shell("fwd task create 'Unstamped'", &[]).unwrap_err();
    assert!(unstamped.contains("but it carries no agent's stamp"), "{unstamped}");
    scratch.shell("fwd task list", &[]).map_err(|e| anyhow!(e))?;
    // FWD_AS does not tell which agent ran it either, since every agent of the session inherits it,
    // and the refusal names both ways out: the plugin's stamp, and a typed --as.
    let inherited = scratch.shell("fwd task create 'Inherited'", &[("FWD_AS", "tester")]).unwrap_err();
    for part in [
        "FWD_AS cannot tell it either",
        "run fwd on its own, which the fwd plugin stamps",
        "or type `--as <name>` on the command",
    ] {
        assert!(inherited.contains(part), "{part}: {inherited}");
    }
    // A --as typed on the command line is a choice no agent inherits, so it acts as the principal it
    // names (D120): an agent that does not use the plugin names itself so, bound to no agent, and
    // writes as itself, while FWD_AS naming it still writes nothing.
    let plain = scratch
        .shell("fwd --as impl-plain principal add impl-plain --role implementer", &[])
        .map_err(|e| anyhow!(e))?;
    assert_eq!(plain["record"]["subject"], Value::Null, "{plain}");
    let typed = scratch.shell("fwd --as impl-plain task create 'Typed'", &[]).map_err(|e| anyhow!(e))?;
    assert_eq!(typed["record"]["created_by"], plain["record"]["id"], "made by the principal --as names");
    let inherited = scratch.shell("fwd task create 'Inherited'", &[("FWD_AS", "impl-plain")]).unwrap_err();
    assert!(inherited.contains("FWD_AS cannot tell it either"), "{inherited}");
    // Another session has no agent bound, so its unstamped write is the person's, as before.
    let elsewhere = scratch.shell("fwd task create 'From a terminal'", &[("CLAUDE_CODE_SESSION_ID", "other")]);
    assert!(elsewhere.is_ok(), "{elsewhere:?}");
    Ok(())
}

/// A command the plugin did not stamp, in a session one of whose agents is bound, with neither a
/// typed --as nor FWD_AS, acts as no one rather than as the person, whose inbox an agent would
/// otherwise read as its own (task JFASz_E2): its reads of the caller's own inbox and tasks are
/// refused with the sentence a write gets, naming the two ways out; the reads about the graph alone
/// run and say nothing; and whoami prints no principal, saying why on standard error. FWD_AS and a
/// typed --as still name whom such a read is for, and the person is the way in outside Claude Code
/// sessions and in a session none of whose agents is bound. A stamped agent that has not named
/// itself acts as no one the same way.
#[test]
fn an_unstamped_command_in_a_session_whose_agents_are_bound_acts_as_no_one() -> Result<()> {
    let scratch = Scratch::new()?;
    let person = |args: &[&str]| -> Result<Value> {
        let out = scratch.program("fwd").args(args).args(["--format", "json"]).output()?;
        assert!(out.status.success(), "fwd {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        Ok(serde_json::from_slice(&out.stdout)?)
    };
    // The person holds a ready task, and a ready task no one holds stands beside it.
    let mine = person(&["task", "create", "The person's"])?["record"]["id"].as_str().expect("an id").to_string();
    person(&["task", "assign", &mine, "tester"])?;
    let nobodys = person(&["task", "create", "No one's"])?["record"]["id"].as_str().expect("an id").to_string();
    // In a session none of whose agents is bound, the command acts as the person, as before.
    let before = scratch.shell("fwd task list --mine", &[]).map_err(|e| anyhow!(e))?;
    assert_eq!(before.as_array().map(Vec::len), Some(1), "{before}");
    // An agent of the session names itself, and asks the person a question.
    scratch.shell(&stamped("fwd principal add impl-a1 --role implementer", Some("a1")), &[]).map_err(|e| anyhow!(e))?;
    scratch.shell(&stamped("fwd ask --to tester 'for the person'", Some("a1")), &[]).map_err(|e| anyhow!(e))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while scratch.unread("tester") != 1 {
        assert!(std::time::Instant::now() < deadline, "the question was not delivered within 10s");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    // Unstamped, with neither --as nor FWD_AS: each read about the caller is refused with the
    // sentence a write gets, naming both ways out.
    for line in [
        "fwd inbox --unread",
        "fwd inbox --for me",
        "fwd watch --count 1 --timeout 5",
        "fwd prime",
        "fwd session stop-check",
        "fwd task list --mine",
        "fwd task next",
    ] {
        let refused = scratch.shell(line, &[]).expect_err(line);
        for part in [
            "but it carries no agent's stamp",
            "run fwd on its own, which the fwd plugin stamps",
            "or type `--as <name>` on the command",
        ] {
            assert!(refused.contains(part), "{line}: {part}: {refused}");
        }
    }
    assert_eq!(scratch.unread("tester"), 1, "nothing read the person's notification");
    // The reads about the graph alone run and say nothing.
    for line in [
        format!("fwd task show {mine}"),
        "fwd task list".to_string(),
        "fwd principal list".to_string(),
        "fwd decision list".to_string(),
        "fwd inbox --for tester".to_string(),
    ] {
        let (_, said) = scratch.shell_saying(&line, &[])?;
        assert_eq!(said, "", "{line} says nothing");
    }
    // The frontier is the one no one holds, not the person's, and the graph is no one's.
    let (ready, _) = scratch.shell_saying("fwd task list --ready", &[])?;
    let ready: Vec<&str> = ready.as_array().expect("a list").iter().filter_map(|t| t["id"].as_str()).collect();
    assert_eq!(ready, [nobodys.as_str()], "the person's own ready task is not listed");
    let (graph, _) = scratch.shell_saying("fwd graph", &[])?;
    assert_eq!(graph["me"], Value::Null);
    // whoami prints no principal, the rest about the machine and the graph, and says why.
    let (whoami, said) = scratch.shell_saying("fwd whoami", &[])?;
    assert_eq!(whoami["principal"], Value::Null, "{whoami}");
    assert_eq!(whoami["project"]["name"], "agents");
    assert!(
        said.starts_with(&format!("fwd: this command runs in the Claude Code session {SESSION}, whose agents act as themselves (@impl-a1 is bound to one of them), but it carries no agent's stamp"))
            && said.contains("or type `--as <name>` on the command"),
        "{said}"
    );

    // FWD_AS names whom such a read is for, as before, and so does a typed --as.
    let listed = scratch.shell("fwd inbox --unread", &[("FWD_AS", "tester")]).map_err(|e| anyhow!(e))?;
    assert_eq!(listed.as_array().map(Vec::len), Some(1), "{listed}");
    let listed = scratch.shell("fwd --as tester task list --mine", &[]).map_err(|e| anyhow!(e))?;
    assert_eq!(listed.as_array().map(Vec::len), Some(1), "{listed}");
    // The person is the way in in a session none of whose agents is bound, and outside any.
    let elsewhere =
        scratch.shell("fwd inbox --unread", &[("CLAUDE_CODE_SESSION_ID", "other")]).map_err(|e| anyhow!(e))?;
    assert_eq!(elsewhere.as_array().map(Vec::len), Some(1), "{elsewhere}");
    assert_eq!(person(&["whoami"])?["principal"]["handle"], "tester");

    // A stamped agent that has not named itself acts as no one the same way: its reads of its own
    // inbox and tasks are refused with how to name itself, and its frontier is no one's.
    for line in ["fwd task list --mine", "fwd task next", "fwd inbox"] {
        let refused = scratch.shell(&stamped(line, Some("a2")), &[]).expect_err(line);
        assert!(refused.contains("which has not named itself in this project"), "{line}: {refused}");
    }
    let ready = scratch.shell(&stamped("fwd task list --ready", Some("a2")), &[]).map_err(|e| anyhow!(e))?;
    let ready: Vec<&str> = ready.as_array().expect("a list").iter().filter_map(|t| t["id"].as_str()).collect();
    assert_eq!(ready, [nobodys.as_str()]);
    Ok(())
}

/// The inbox hooks act for the agent whose hook fired, which the hook's JSON on standard input
/// names, and not for FWD_AS, which a settings file's `env` block gives every agent of the session
/// alike (task GPL5NABZ): a subagent's hook pops the subagent's own notifications and none of its
/// lead's, and the session's own thread's hooks act for the lead bound to it. A subagent that has
/// not named itself is acted for by no one; the own thread of a session that has not bound a
/// principal yet falls back to FWD_AS, as the hooks acted before, so that a lead that has not bound
/// itself keeps its notifications. Without a hook's JSON, as when a person runs the command, FWD_AS
/// names the principal.
#[test]
fn the_inbox_hooks_act_for_the_agent_whose_hook_fired() -> Result<()> {
    let scratch = Scratch::new()?;
    let run = |line: &str, agent: Option<&str>| scratch.shell(&stamped(line, agent), &[]).map_err(|e| anyhow!(e));
    run("fwd principal add lead-main --role lead", None)?;
    run("fwd principal add impl-a1 --role implementer", Some("a1"))?;
    run("fwd ask --to tester 'for the person'", Some("a1"))?;
    for (to, text) in [("lead-main", "for the lead"), ("impl-a1", "for the subagent")] {
        let out = scratch.program("fwd").args(["ask", "--to", to, text]).output()?;
        assert!(out.status.success(), "fwd ask: {}", String::from_utf8_lossy(&out.stderr));
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while ["lead-main", "impl-a1", "tester"].iter().any(|who| scratch.unread(who) != 1) {
        assert!(std::time::Instant::now() < deadline, "the questions were not delivered within 10s");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    // Every hook of the session runs with FWD_AS naming the person, as a settings file would give it.
    let hook = |line: &str, event: &str, agent: Option<&str>| {
        scratch.hook(line, &event_input(SESSION, event, agent), Some("tester")).map_err(|e| anyhow!(e))
    };
    let unread = || [scratch.unread("lead-main"), scratch.unread("impl-a1"), scratch.unread("tester")];
    let pop = "fwd inbox pop --format hook --hook-event PostToolBatch";
    let stop_check = "fwd session stop-check --format hook";

    // A subagent that has not named itself: its hooks act for no one, neither the person FWD_AS
    // names nor the session's lead, print nothing, and mark nothing read.
    for (line, event) in [(pop, "PostToolBatch"), (stop_check, "Stop"), ("fwd prime --format hook", "SessionStart")] {
        assert_eq!(hook(line, event, Some("a2"))?, "", "{line}");
    }
    assert_eq!(unread(), [1, 1, 1]);

    // The subagent that named itself: its hook pops its own notification, and not its lead's.
    let popped: Value = serde_json::from_str(&hook(pop, "PostToolBatch", Some("a1"))?)?;
    assert_eq!(popped["hookSpecificOutput"]["hookEventName"], "PostToolBatch");
    let context = popped["hookSpecificOutput"]["additionalContext"].as_str().expect("the context");
    assert!(context.contains("as impl-a1") && context.contains("for the subagent"), "{context}");
    assert!(!context.contains("for the lead"), "{context}");
    assert_eq!(unread(), [1, 0, 1]);

    // The session's own thread, which has no agent id in the hook's JSON, is the lead.
    let primed = hook("fwd prime --format hook", "SessionStart", None)?;
    assert!(primed.starts_with("fwdcentaur context for lead-main:") && primed.contains("for the lead"), "{primed}");
    assert!(
        primed.contains("no agent serves the whole project, so no triage agent answers"),
        "the lead is told how to create one: {primed}"
    );
    let stop: Value = serde_json::from_str(&hook(stop_check, "Stop", None)?)?;
    assert_eq!(stop["decision"], "block");
    let reason = stop["reason"].as_str().expect("the reason");
    assert!(reason.contains("for lead-main") && reason.contains("for the lead"), "{reason}");
    let prompt = "fwd inbox pop --format hook --hook-event UserPromptSubmit";
    let popped: Value = serde_json::from_str(&hook(prompt, "UserPromptSubmit", None)?)?;
    let context = popped["hookSpecificOutput"]["additionalContext"].as_str().expect("the context");
    assert!(context.contains("as lead-main") && context.contains("for the lead"), "{context}");
    assert_eq!(hook(stop_check, "Stop", None)?.trim(), "{}");
    assert_eq!(unread(), [0, 0, 1], "FWD_AS was read by none of the hooks");

    // Another session, none of whose agents is bound: its own thread's hooks act for the principal
    // FWD_AS names, and for no one without it; its subagent's, for no one.
    let other = "7f3c2a10-5d4e-4b6a-9c8d-0e1f2a3b4c5d";
    let hook_in = |line: &str, event: &str, agent: Option<&str>, fwd_as: Option<&str>| {
        scratch.hook(line, &event_input(other, event, agent), fwd_as).map_err(|e| anyhow!(e))
    };
    let primed = hook_in("fwd prime --format hook", "SessionStart", None, Some("tester"))?;
    assert!(primed.starts_with("fwdcentaur context for tester:") && primed.contains("for the person"), "{primed}");
    let stop: Value = serde_json::from_str(&hook_in(stop_check, "Stop", None, Some("tester"))?)?;
    let reason = stop["reason"].as_str().expect("the reason");
    assert!(reason.contains("for tester") && reason.contains("for the person"), "{reason}");
    assert_eq!(hook_in(stop_check, "Stop", None, None)?, "", "the thread's hook without FWD_AS");
    assert_eq!(hook_in(pop, "PostToolBatch", Some("b1"), Some("tester"))?, "", "a subagent's hook, FWD_AS or not");
    assert_eq!(unread(), [0, 0, 1]);

    // Without a hook's JSON, FWD_AS names the principal, as for a person's command...
    let out = scratch.program("fwd").args(["inbox", "pop", "--format", "hook"]).env("FWD_AS", "tester").output()?;
    let popped: Value = serde_json::from_slice(&out.stdout)?;
    let context = popped["hookSpecificOutput"]["additionalContext"].as_str().expect("the context");
    assert!(context.contains("as tester") && context.contains("for the person"), "{context}");
    // ...but in the session, whose agents are bound, such a command is refused as any unstamped
    // write is, since only the hook's JSON says which agent runs it.
    let refused = scratch
        .program("sh")
        .args(["-c", "fwd inbox pop --format hook"])
        .env("CLAUDE_CODE_SESSION_ID", SESSION)
        .env("FWD_AS", "tester")
        .output()?;
    let refused = String::from_utf8_lossy(&refused.stderr);
    assert!(refused.contains("but it carries no agent's stamp"), "{refused}");
    Ok(())
}

/// A retired principal writes nothing until it comes back (task Wbkkwpyq): a principal retires when
/// its job ends and no picker offers it from then on, so its write is refused with the command that
/// brings it back, `fwd principal add <its name> --role <its role>` run as it, which runs; its reads
/// run as before, and its hooks say nothing, printing nothing and marking nothing read. Back, it
/// writes again. A principal `--as` names is refused the same, with `--as` in the way back; a
/// principal that was succeeded, and so retired, is refused naming its successor, `fwd ui` opening
/// the page as it included, and a successor that retired in turn is refused as any other.
#[test]
fn a_retired_principal_writes_nothing_until_principal_add_run_as_it_brings_it_back() -> Result<()> {
    let scratch = Scratch::new()?;
    let run = |line: &str| scratch.shell(&stamped(line, Some("a1")), &[]).map_err(|e| anyhow!(e));
    let refused = |line: &str| scratch.shell(&stamped(line, Some("a1")), &[]).expect_err(line);
    run("fwd principal add impl-a1 --role implementer")?;
    let task = run("fwd task create 'Retry on disconnect'")?["record"]["id"].as_str().expect("an id").to_string();
    // A question reaches the agent, which then retires itself, as a subagent's last command does.
    let out = scratch.program("fwd").args(["ask", "--to", "impl-a1", "are you there?"]).output()?;
    assert!(out.status.success(), "fwd ask: {}", String::from_utf8_lossy(&out.stderr));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while scratch.unread("impl-a1") != 1 {
        assert!(std::time::Instant::now() < deadline, "the question was not delivered within 10s");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    run("fwd principal retire impl-a1")?;

    // Its writes are refused, naming the way back; marking its own notifications is a write too.
    let way_back = "`@impl-a1` is retired, and a retired principal writes nothing, since no picker offers it: to write again, bring it back first with `fwd principal add impl-a1 --role implementer`, run as it";
    for line in [
        format!("fwd comment {task} 'still here'"),
        "fwd task create 'More work'".to_string(),
        "fwd inbox pop".to_string(),
        "fwd principal retire impl-a1".to_string(),
    ] {
        let said = refused(&line);
        assert!(said.contains(way_back), "{line}: {said}");
    }
    // Its reads run as before.
    assert_eq!(run(&format!("fwd task show {task}"))?["title"], "Retry on disconnect");
    let unread = run("fwd inbox --unread")?;
    assert_eq!(unread.as_array().map(Vec::len), Some(1), "{unread}");
    let notification = unread[0]["id"].as_str().expect("an id").to_string();
    for line in [format!("fwd read {notification}"), format!("fwd dismiss {notification}")] {
        let said = refused(&line);
        assert!(said.contains(way_back), "{line}: {said}");
    }
    // Its hooks say nothing: they print nothing and mark nothing read.
    for (line, event) in [
        ("fwd inbox pop --format hook --hook-event PostToolBatch", "PostToolBatch"),
        ("fwd prime --format hook", "SessionStart"),
        ("fwd session stop-check --format hook", "Stop"),
    ] {
        let said = scratch.hook(line, &event_input(SESSION, event, Some("a1")), None).map_err(|e| anyhow!(e))?;
        assert_eq!(said, "", "{line}");
    }
    assert_eq!(scratch.unread("impl-a1"), 1, "the hooks marked nothing read");

    // The command the refusal names brings it back, and it writes again.
    let back = run("fwd principal add impl-a1 --role implementer")?;
    assert_eq!(back["record"]["active"], true, "{back}");
    run(&format!("fwd comment {task} 'back again'"))?;

    // A principal a typed --as names: its way back names --as, and FWD_AS naming it, none.
    let fwd = |fwd_as: Option<&str>, args: &[&str]| -> Result<std::process::Output> {
        let mut command = scratch.program("fwd");
        command.args(args);
        if let Some(name) = fwd_as {
            command.env("FWD_AS", name);
        }
        Ok(command.output()?)
    };
    let said = |out: std::process::Output| String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(fwd(None, &["--as", "impl-old", "principal", "add", "impl-old", "--role", "reviewer"])?.status.success());
    // It is succeeded, as a relaunched agent succeeds the one before it, which retires it.
    let succeeded = fwd(None, &["--as", "impl-new", "principal", "add", "impl-new", "--succeeds", "impl-old"])?;
    assert!(succeeded.status.success(), "{}", said(succeeded));
    let old = said(fwd(None, &["--as", "impl-old", "task", "create", "Old work"])?);
    let way_back_old = "`@impl-old` is retired, succeeded by @impl-new, which carries on its job, and a retired principal writes nothing, since no picker offers it: to write again, bring it back first with `fwd --as impl-old principal add impl-old --role lead|triage|implementer|reviewer`, run as it";
    assert!(old.contains(way_back_old), "{old}");
    // The page is opened to write as the principal it acts as, so `fwd ui` is refused the same, and
    // not for the role it lost to its successor, which `fwd principal role`, a write, cannot give it
    // back while it is retired.
    let page = said(fwd(None, &["--as", "impl-old", "ui", "--no-open"])?);
    assert!(page.contains(way_back_old) && !page.contains("holds no role"), "{page}");
    // The successor retires in turn, and is refused as any other.
    assert!(fwd(None, &["principal", "retire", "impl-new"])?.status.success());
    let new = said(fwd(None, &["--as", "impl-new", "task", "create", "New work"])?);
    assert!(
        new.contains("bring it back first with `fwd --as impl-new principal add impl-new --role reviewer`"),
        "{new}"
    );
    let by_env = said(fwd(Some("impl-new"), &["task", "create", "New work"])?);
    assert!(by_env.contains("bring it back first with `fwd principal add impl-new --role reviewer`"), "{by_env}");
    // Back by --as, it writes again.
    assert!(fwd(None, &["--as", "impl-new", "principal", "add", "impl-new"])?.status.success());
    let wrote = fwd(None, &["--as", "impl-new", "task", "create", "New work"])?;
    assert!(wrote.status.success(), "{}", said(wrote));
    Ok(())
}
