//! Whom a command acts as once principals are the Commons' `identity.Principal` and the subjects
//! that act as them are identity bindings: an agent's command, stamped as the fwd plugin stamps it
//! or run by its hook, acts as the principal its session binds in the project the command runs in;
//! a subject bound to no one acts as no one; a retired principal's writes are refused; who retires
//! a principal is recorded; and in a project whose principals `fwd admin migrate` has not moved yet,
//! as every project's are when this build is first installed, every command that needs to know
//! whom it acts as is refused, naming the migration, until the migration, run by the person or by
//! an agent bound there, has moved them. Each test runs the CLI against a scratch daemon of its own
//! that hosts two projects, alpha and beta.

mod common;

use anyhow::{anyhow, Result};
use common::Scratch;
use fwd_core::conversion::principals::before_d261;
use fwd_core::daemon::{self, Autostart, Client};
use fwd_core::model::*;
use fwd_core::project::{self, NamedProject, Paths};
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
/// stamped and by its hook; a subject bound to no one acts as no one, and its write is refused; and
/// a retired principal's own write is refused, naming how it comes back.
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

/// What the person `tester` and the agent `impl-old`, bound to the session's agent a1, are in beta
/// as the build before the move onto the Commons wrote them: their ids.
struct Unmoved {
    person: String,
    agent: String,
}

impl Daemon {
    /// A daemon whose project alpha `tester` made with this build, and whose project beta holds
    /// only principals of the build before the move, the person's among them, as every project the
    /// daemon hosts does when this build is first installed: beta is made with no principal, and
    /// its principals are written as that build wrote them.
    async fn unmoved() -> Result<(Daemon, Unmoved)> {
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
        let options = project::InitOptions { name: Some("beta"), ..Default::default() };
        let beta = project::init(&daemon.paths, &daemon.beta, options)?.0.project;
        let spawn = Autostart::Spawn { daemon_bin: PathBuf::from(env!("CARGO_BIN_EXE_fwd")) };
        let client = daemon::ensure_daemon(&daemon.paths, &beta, spawn).await?;
        let graph = &client.graph;
        let trx = graph.begin();
        let mut ids = Vec::new();
        for (kind, handle, role, subject) in [
            (PrincipalKind::Human, "tester", None, None),
            (PrincipalKind::Agent, "impl-old", Some(AgentRole::Implementer), Some(subject("a1"))),
        ] {
            let old = before_d261::Principal {
                project: graph.project(),
                kind,
                handle: handle.into(),
                display_name: handle.into(),
                auth_subject: None,
                agent_role: role,
                active: true,
                member: Some(true),
                subject,
                succeeded_by: None,
                serves: None,
                last_active: None,
            };
            ids.push(trx.create(&old).await?.id().to_base64());
        }
        trx.commit().await?;
        let [person, agent] = <[String; 2]>::try_from(ids).map_err(|_| anyhow!("two principals"))?;
        Ok((daemon, Unmoved { person, agent }))
    }

    /// The ids `fwd principal list --all` lists in `checkout`'s project, as the person.
    fn listed(&self, checkout: &Path) -> Result<Vec<String>> {
        let all = self.json(checkout, None, &["principal", "list", "--all"])?;
        let mut ids: Vec<String> =
            all.as_array().into_iter().flatten().filter_map(|p| p["id"].as_str().map(str::to_string)).collect();
        ids.sort();
        Ok(ids)
    }

    /// Asserts that, before the migration, each command beta's person or its bound agent a1 runs
    /// that needs to know whom it acts as is refused, naming the migration, and a1's hook prints
    /// nothing.
    fn refused_before_the_migration(&self) -> Result<()> {
        let migrate = "the project beta holds principals that `fwd admin migrate` has not moved";
        for (agent, args) in [
            (None, &["whoami"][..]),
            (None, &["task", "create", "Cap the retry delay"][..]),
            (Some("a1"), &["whoami"][..]),
            (Some("a1"), &["task", "list"][..]),
        ] {
            let out = self.run(&self.beta, agent, args)?;
            let said = String::from_utf8_lossy(&out.stderr);
            assert!(!out.status.success() && said.contains(migrate), "{agent:?} {args:?}: {said}");
        }
        assert_eq!(self.hook(&self.beta, "a1")?, "", "the hook acts for no one");
        Ok(())
    }

    /// Asserts that, after the migration, beta's person and a1 each act as itself, keeping its id,
    /// and that beta holds those two principals alone.
    fn each_acts_as_itself(&self, unmoved: &Unmoved) -> Result<()> {
        let person = self.json(&self.beta, None, &["whoami"])?;
        assert_eq!(person["principal"]["id"], unmoved.person.as_str());
        assert_eq!(self.whoami(&self.beta, "a1")?["id"], unmoved.agent.as_str());
        assert!(self.hook(&self.beta, "a1")?.contains("impl-old"), "the hook acts for it");
        let mut both = vec![unmoved.person.clone(), unmoved.agent.clone()];
        both.sort();
        assert_eq!(self.listed(&self.beta)?, both, "none beside them");
        Ok(())
    }
}

/// The ids `fwd admin migrate` printed as the principals it moved.
fn moved(migrated: &Value) -> Vec<String> {
    let mut ids: Vec<String> = migrated["principals_moved"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p["id"].as_str().map(str::to_string))
        .collect();
    ids.sort();
    ids
}

/// Where every principal predates the move, the person's included, every command that needs to
/// know whom it acts as is refused, naming the migration; the person's `fwd admin migrate` moves
/// them, keeping their ids, and each acts as itself after it.
#[tokio::test]
async fn the_person_moves_a_project_whose_every_principal_predates_the_move() -> Result<()> {
    let (d, unmoved) = Daemon::unmoved().await?;
    d.refused_before_the_migration()?;
    let migrated = d.json(&d.beta, None, &["admin", "migrate"])?;
    let mut both = vec![unmoved.person.clone(), unmoved.agent.clone()];
    both.sort();
    assert_eq!(moved(&migrated), both);
    d.each_acts_as_itself(&unmoved)?;
    assert_eq!(moved(&d.json(&d.beta, None, &["admin", "migrate"])?), Vec::<String>::new(), "a second run");
    Ok(())
}

/// The agent bound there may run the migration first, as itself.
#[tokio::test]
async fn the_bound_agent_moves_a_project_whose_every_principal_predates_the_move() -> Result<()> {
    let (d, unmoved) = Daemon::unmoved().await?;
    d.refused_before_the_migration()?;
    let migrated = d.json(&d.beta, Some("a1"), &["admin", "migrate"])?;
    let mut both = vec![unmoved.person.clone(), unmoved.agent.clone()];
    both.sort();
    assert_eq!(moved(&migrated), both);
    d.each_acts_as_itself(&unmoved)?;
    Ok(())
}

/// A migration refused for its caller once it has moved the principals, as an agent that has not
/// named itself is, says on standard error what it moved, since that is written: the next run moves
/// nothing.
#[tokio::test]
async fn a_migration_refused_after_it_moved_the_principals_says_what_it_moved() -> Result<()> {
    let (d, unmoved) = Daemon::unmoved().await?;
    let out = d.run(&d.beta, Some("a2"), &["admin", "migrate"])?;
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success() && said.contains("which has not named itself in this project"), "{said}");
    assert!(said.contains("principals moved onto the Commons' identity.Principal"), "{said}");
    for (id, handle) in [(&unmoved.person, "@tester"), (&unmoved.agent, "@impl-old")] {
        assert!(said.contains(&format!("{} {handle}", &id[..8])), "{said}");
    }
    assert_eq!(moved(&d.json(&d.beta, None, &["admin", "migrate"])?), Vec::<String>::new());
    d.each_acts_as_itself(&unmoved)?;
    Ok(())
}

/// Whoever a command acts as retires a principal: one a typed `--as` names, by its name, its id or
/// the start of its id, and an agent that retires itself.
#[tokio::test]
async fn a_retirement_records_whom_the_command_acts_as() -> Result<()> {
    let d = Daemon::new()?;
    let made = |name: &str| -> Result<String> {
        let added = d.json(&d.alpha, None, &["--as", name, "principal", "add", name, "--role", "implementer"])?;
        Ok(added["record"]["id"].as_str().ok_or_else(|| anyhow!("no id in {added}"))?.to_string())
    };
    let a = made("impl-a")?;
    let client = d.client(&d.alpha).await?;
    let retired_by = |id: String| {
        let graph = client.graph.clone();
        async move {
            let role: RoleView = graph.get(id.parse()?).await?;
            let closed: ankurah_commons::records::ClosedView = graph.component_of(&role).await?.expect("retired");
            anyhow::Ok(closed.by()?.map(|by| (*by).to_base64()))
        }
    };
    for (by, retired) in [("impl-a", "impl-b"), (a.as_str(), "impl-c"), (&a[..8], "impl-d")] {
        let id = made(retired)?;
        d.json(&d.alpha, None, &["--as", by, "principal", "retire", retired])?;
        assert_eq!(retired_by(id).await?, Some(a.clone()), "--as {by}");
    }
    let itself = d.json(&d.alpha, Some("a1"), &["principal", "add", "impl-e", "--role", "implementer"])?;
    d.json(&d.alpha, Some("a1"), &["principal", "retire", "impl-e"])?;
    let itself = itself["record"]["id"].as_str().ok_or_else(|| anyhow!("no id"))?.to_string();
    assert_eq!(retired_by(itself.clone()).await?, Some(itself), "an agent retiring itself");
    Ok(())
}
