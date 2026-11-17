//! Whom a command acts as once principals are the Commons' `identity.Principal` and the subjects
//! that act as them are identity bindings: an agent's command, stamped as the fwd plugin stamps it
//! or run by its hook, acts as the principal its session binds in the project the command runs in;
//! a subject bound to no one, or no longer, acts as no one; a retired principal's writes are
//! refused; and in a project whose principals `fwd admin migrate` has not moved yet, nothing makes
//! a principal or binds a session, until the migration has run. Each test runs the CLI against a
//! scratch daemon of its own that hosts two projects, alpha and beta, that the person `tester`
//! made.

mod common;

use anyhow::{anyhow, Result};
use common::Scratch;
use fwd_core::conversion::principals::before_d261;
use fwd_core::daemon::{self, Autostart, Client};
use fwd_core::model::*;
use fwd_core::project::{NamedProject, Paths};
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// The Claude Code session the agents here run in.
const SESSION: &str = "5e55a0b1-0c2d-4e3f-8a9b-0c1d2e3f4a5b";

/// One daemon's folders, and a checkout of each of the two projects it hosts.
struct Daemon {
    _scratch: Scratch,
    paths: Paths,
    alpha: PathBuf,
    beta: PathBuf,
}

impl Daemon {
    /// The daemon's folders in a scratch folder, and a checkout of each of two projects that
    /// `tester` made.
    fn new() -> Result<Daemon> {
        let scratch = Scratch::new()?;
        let paths = scratch.paths("run", "data");
        let checkout = |name: &str| -> Result<PathBuf> {
            let root = scratch.path().join(name);
            std::fs::create_dir_all(&root)?;
            fwd_core::anchoring::git(&root, &["init", "-q", "-b", "main"])?;
            fwd_core::anchoring::git(&root, &["remote", "add", "origin", &format!("git@example.com:acme/{name}.git")])?;
            Ok(root)
        };
        let (alpha, beta) = (checkout("alpha")?, checkout("beta")?);
        let daemon = Daemon { paths, alpha, beta, _scratch: scratch };
        daemon.json(&daemon.alpha, None, &["init", "--name", "alpha", "--handle", "tester"])?;
        daemon.json(&daemon.beta, None, &["init", "--name", "beta", "--handle", "tester"])?;
        Ok(daemon)
    }

    /// `fwd <args>` in `checkout`, reaching this daemon only, with none of the caller's settings
    /// that would choose another project, principal or agent; stamped, as the fwd plugin stamps a
    /// command, with the session's agent `agent` when one is given.
    fn command(&self, checkout: &Path, agent: Option<&str>, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fwd"));
        command
            .current_dir(checkout)
            .args(args)
            .env("FWD_RUNTIME_DIR", &self.paths.runtime_dir)
            .env("FWD_DATA_DIR", &self.paths.data_dir);
        for name in
            ["FWD_PROJECT", "FWD_AS", "FWD_SCOPE", "FWD_CLAUDE_SESSION", "FWD_CLAUDE_AGENT", "CLAUDE_CODE_SESSION_ID"]
        {
            command.env_remove(name);
        }
        if let Some(agent) = agent {
            command.env("FWD_CLAUDE_SESSION", SESSION).env("FWD_CLAUDE_AGENT", agent);
        }
        command
    }

    /// `fwd --format json <args>`, which may fail.
    fn run(&self, checkout: &Path, agent: Option<&str>, args: &[&str]) -> Result<Output> {
        Ok(self.command(checkout, agent, &[&["--format", "json"][..], args].concat()).output()?)
    }

    /// The JSON `fwd --format json <args>` prints, which must succeed.
    fn json(&self, checkout: &Path, agent: Option<&str>, args: &[&str]) -> Result<Value> {
        let out = self.run(checkout, agent, args)?;
        if !out.status.success() {
            return Err(anyhow!("fwd {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr)));
        }
        Ok(serde_json::from_slice(&out.stdout)?)
    }

    /// What the hook of the session's agent `agent` prints in `checkout` as a session starts, as
    /// Claude Code runs `fwd prime --format hook` with the hook's JSON on standard input.
    fn hook(&self, checkout: &Path, agent: &str) -> Result<String> {
        let mut child = self.command(checkout, None, &["prime", "--format", "hook"]);
        let mut child = child.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
        let mut input = json!({"session_id": SESSION, "hook_event_name": "SessionStart"});
        if agent != "main" {
            input["agent_id"] = json!(agent);
        }
        child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?.write_all(input.to_string().as_bytes())?;
        let out = child.wait_with_output()?;
        if !out.status.success() {
            return Err(anyhow!("the hook failed: {}", String::from_utf8_lossy(&out.stderr)));
        }
        Ok(String::from_utf8(out.stdout)?)
    }

    /// The principal `fwd whoami` names for the session's agent `agent` in `checkout`'s project.
    fn whoami(&self, checkout: &Path, agent: &str) -> Result<Value> {
        Ok(self.json(checkout, Some(agent), &["whoami"])?["principal"].clone())
    }

    /// A client of the project whose checkout is `checkout`, to write what only the build before
    /// the move onto the Commons wrote.
    async fn client(&self, checkout: &Path) -> Result<Client> {
        let shown = self.json(checkout, None, &["project", "show"])?;
        let project = NamedProject {
            id: shown["id"].as_str().ok_or_else(|| anyhow!("no id in {shown}"))?.parse()?,
            name: shown["name"].as_str().unwrap_or_default().to_string(),
        };
        daemon::ensure_daemon(&self.paths, &project, Autostart::Never).await
    }
}

/// The subject of the session's agent `agent`.
fn subject(agent: &str) -> String {
    format!("claude-code:{SESSION}/{agent}")
}

/// One agent named in alpha and in beta acts as alpha's principal in alpha and beta's in beta,
/// stamped and by its hook; a subject bound to no one acts as no one, and its write is refused;
/// unbound, the subject a retired principal held acts as no one; and a retired principal's own
/// write is refused, naming how it comes back.
#[test]
fn a_command_acts_as_the_principal_its_session_binds_in_the_project_it_runs_in() -> Result<()> {
    let s = Daemon::new()?;
    let ash = s.json(&s.alpha, Some("a1"), &["principal", "add", "impl-ash", "--role", "implementer"])?;
    let their_ash = s.json(&s.beta, Some("a1"), &["principal", "add", "impl-ash", "--role", "implementer"])?;
    assert_ne!(ash["id"], their_ash["id"], "a principal in each project");
    for (dir, made) in [(&s.alpha, &ash), (&s.beta, &their_ash)] {
        let me = s.whoami(dir, "a1")?;
        assert_eq!((&me["id"], &me["subject"]), (&made["id"], &json!(subject("a1"))));
    }
    let hooked = s.hook(&s.beta, "a1")?;
    assert!(hooked.contains("impl-ash"), "the hook acts for the agent: {hooked}");

    assert_eq!(s.whoami(&s.alpha, "a9")?, Value::Null, "a subject bound to no one acts as no one");
    let unnamed = s.run(&s.alpha, Some("a9"), &["task", "create", "Cap the retry delay"])?;
    assert!(!unnamed.status.success());
    assert!(String::from_utf8_lossy(&unnamed.stderr).contains("which has not named itself in this project"));

    s.json(&s.alpha, Some("a1"), &["principal", "retire", "impl-ash"])?;
    let retired = s.run(&s.alpha, Some("a1"), &["task", "create", "Cap the retry delay"])?;
    assert!(!retired.status.success());
    let said = String::from_utf8_lossy(&retired.stderr);
    assert!(said.contains("`@impl-ash` is retired") && said.contains("principal add impl-ash"), "{said}");
    assert_eq!(s.whoami(&s.alpha, "a1")?["active"], false, "it is still found, retired");
    assert_eq!(s.whoami(&s.beta, "a1")?["active"], true, "and is at work in beta");
    Ok(())
}

/// In a project that still holds a principal the build before the move onto the Commons wrote, an
/// agent's command made nothing: its session acts as no one, by its hook too, naming itself is
/// refused naming `fwd admin migrate` and the project, and no principal or binding is made. Once
/// the migration has run, the same commands act as the principal moved, and none is made beside it.
#[tokio::test]
async fn nothing_makes_a_principal_beside_one_not_moved_until_the_migration_runs() -> Result<()> {
    let s = Daemon::new()?;
    let client = s.client(&s.alpha).await?;
    let graph = &client.graph;
    let trx = graph.begin();
    let old = trx
        .create(&before_d261::Principal {
            project: graph.project(),
            kind: PrincipalKind::Agent,
            handle: "impl-old".into(),
            display_name: "impl-old".into(),
            auth_subject: None,
            agent_role: Some(AgentRole::Implementer),
            active: true,
            member: Some(true),
            subject: Some(subject("a1")),
            succeeded_by: None,
            serves: None,
            last_active: None,
        })
        .await?
        .id();
    trx.commit().await?;
    drop(client);
    let listed = || s.json(&s.alpha, None, &["principal", "list", "--all"]);
    let before = listed()?;

    assert_eq!(s.hook(&s.alpha, "a1")?, "", "the hook acts for no one");
    assert_eq!(s.whoami(&s.alpha, "a1")?, Value::Null);
    let named = s.run(&s.alpha, Some("a1"), &["principal", "add", "impl-old", "--role", "implementer"])?;
    assert!(!named.status.success());
    let said = String::from_utf8_lossy(&named.stderr);
    assert!(said.contains("the project alpha holds principals that `fwd admin migrate` has not moved"), "{said}");
    assert_eq!(listed()?, before, "no principal was made");

    let migrated = s.json(&s.alpha, None, &["admin", "migrate"])?;
    assert_eq!(migrated["principals_moved"][0]["id"], old.to_base64());
    let me = s.whoami(&s.alpha, "a1")?;
    assert_eq!((&me["id"], &me["handle"]), (&json!(old.to_base64()), &json!("impl-old")));
    assert!(s.hook(&s.alpha, "a1")?.contains("impl-old"), "the hook acts for it");
    s.json(&s.alpha, Some("a1"), &["principal", "add", "impl-old", "--role", "implementer"])?;
    let handles: Vec<Value> = listed()?.as_array().into_iter().flatten().map(|p| p["handle"].clone()).collect();
    assert_eq!(handles.iter().filter(|h| **h == "impl-old").count(), 1, "and none beside it: {handles:?}");
    Ok(())
}
