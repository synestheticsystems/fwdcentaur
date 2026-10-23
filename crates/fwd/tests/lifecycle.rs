//! The daemon's lifecycle: concurrent launchers start exactly one daemon; data persists across a
//! stop and restart, on the same port; a stale port file is recovered without killing any pid,
//! and one that names another daemon's port is not trusted; a project a per-project daemon from
//! an earlier build still holds is left to it; one daemon hosts every project, including one
//! created while it runs; sutra shows the daemon while it runs, and nothing of it once it stops;
//! a `fwd watch` with no timeout hears across a restart, on the same port or another, and says
//! nothing about it, and one given `--timeout` ends once it has passed; either exits within a
//! second of its last line. Any other command caught waiting on its daemon when the daemon stops
//! ends with exit 7 and an error saying what became of the daemon: at once when it comes back on
//! another port, and once the bound has passed when none comes back. A command that writes ends
//! so whenever its socket closed while it ran, with an error saying that what it wrote may not
//! have reached the daemon, even when Ankurah reported its commit made, which it then made to the
//! command's own node alone; and one whose socket closed before it began to write begins nothing.

mod common;

use ankurah::property::{Json, Ref};
use ankurah::EntityId;
use anyhow::{anyhow, Result};
use common::Scratch;
use fwd_core::daemon::{self, Autostart, Bind, DaemonConfig, PortFile, LOST_AFTER};
use fwd_core::model::*;
use fwd_core::naming;
use fwd_core::ops::{self, CreateTask, MessageContext};
use fwd_core::project::{self, Paths, Project};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

fn setup() -> Result<(Scratch, Paths, Project)> {
    let dir = Scratch::new()?;
    let paths = dir.paths("run", "data");
    let ws = new_project(&paths, &dir.path().join("repo"), "test")?;
    Ok((dir, paths, ws))
}

fn new_project(paths: &Paths, root: &Path, name: &str) -> Result<Project> {
    std::fs::create_dir_all(root)?;
    Ok(project::init(paths, root, project::InitOptions { name: Some(name), ..Default::default() })?.0.project)
}

fn spawn() -> Autostart {
    Autostart::Spawn { daemon_bin: PathBuf::from(env!("CARGO_BIN_EXE_fwd")) }
}

#[tokio::test]
async fn concurrent_launchers_start_exactly_one_daemon() -> Result<()> {
    let (_dir, paths, ws) = setup()?;
    let mut handles = Vec::new();
    for _ in 0..6 {
        let (paths, ws) = (paths.clone(), ws.clone());
        handles.push(tokio::spawn(async move {
            let client = daemon::ensure_daemon(&paths, &ws, spawn()).await?;
            anyhow::Ok(client.ws.server_node_id())
        }));
    }
    let mut server_ids = Vec::new();
    for h in handles {
        server_ids.push(h.await??.expect("connected clients know the server node id"));
    }
    assert!(server_ids.iter().all(|id| *id == server_ids[0]), "every launcher must reach the same daemon");
    assert!(daemon::daemon_alive(&paths)?);
    let pf = daemon::read_port_file(&paths.port_file())?.expect("port file");
    assert_eq!(pf.bind, Bind::Loopback, "the daemon listens on loopback unless asked otherwise");
    let running = daemon::status(&paths).await?.expect("the daemon answers");
    assert_eq!(running.pf, pf);
    assert_eq!(running.projects.iter().map(|p| p.id).collect::<Vec<_>>(), vec![ws.id]);

    assert!(daemon::stop(&paths).await?);
    assert!(!daemon::daemon_alive(&paths)?);
    assert!(daemon::read_port_file(&paths.port_file())?.is_none(), "the port file is removed on exit");
    assert!(!daemon::stop(&paths).await?, "a second stop finds nothing running");
    // One spawn for six launchers: the log holds one daemon that listened, and none that found
    // the lifetime lock taken and exited.
    let log = std::fs::read_to_string(paths.log_path())?;
    assert_eq!(log.matches("daemon listening").count(), 1, "exactly one daemon listened:\n{log}");
    assert_eq!(log.matches("another daemon holds").count(), 0, "no second daemon was spawned:\n{log}");
    Ok(())
}

#[tokio::test]
async fn data_persists_across_daemon_restarts() -> Result<()> {
    let (_dir, paths, ws) = setup()?;
    let task_id = {
        let client = daemon::ensure_daemon(&paths, &ws, spawn()).await?;
        let trx = client.ctx.begin();
        let me = trx
            .create(&Principal {
                kind: PrincipalKind::Human,
                handle: "dnorman".into(),
                display_name: "Daniel".into(),
                auth_subject: None,
                agent_role: None,
                active: true,
                member: Some(true),
                subject: None,
                succeeded_by: None,
                serves: None,
                last_active: None,
            })
            .await?
            .id();
        let now = fwd_core::now_ms();
        let task = trx
            .create(&Task {
                title: "Survive a restart".into(),
                what: "".into(),
                why: "".into(),
                done_when: "".into(),
                status: TaskStatus::Open,
                needs_review: false,
                hold: None,
                assignee: None,
                priority: None,
                labels: Json::new(serde_json::json!([])),
                context: Json::new(serde_json::json!({})),
                created_by: Ref::new(me),
                created: now,
                updated: now,
                closed: None,
                closed_reason: None,
            })
            .await?
            .id();
        trx.commit().await?;
        task
    };
    let first = daemon::read_port_file(&paths.port_file())?.expect("port file");
    assert!(daemon::stop(&paths).await?);

    let client = daemon::ensure_daemon(&paths, &ws, spawn()).await?;
    let second = daemon::read_port_file(&paths.port_file())?.expect("port file");
    assert_ne!(first.pid, second.pid, "a fresh daemon process serves after the stop");
    assert_eq!(second.port, first.port, "the fresh daemon listens on the port the first one had");
    assert_eq!(
        DaemonConfig::load(&paths)?.port,
        Some(second.port),
        "the daemon records its port to try it first next time"
    );
    let task: TaskView = client.ctx.get(task_id).await?;
    assert_eq!(task.title()?, "Survive a restart");
    assert!(daemon::stop(&paths).await?);
    Ok(())
}

#[tokio::test]
async fn a_stale_port_file_is_replaced_without_killing_anything() -> Result<()> {
    let (_dir, paths, ws) = setup()?;
    paths.ensure()?;
    // A port nobody listens on, and a pid that is not ours to touch.
    let unused = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = unused.local_addr()?.port();
    drop(unused);
    daemon::write_port_file(
        &paths.port_file(),
        &PortFile { port, pid: 1, instance_id: "stale".into(), started: 0, bind: Bind::Loopback },
    )?;
    // What a per-project daemon from an earlier build left behind when it died.
    let leftovers = ["ws", "bootstrap.lock"].map(|ext| paths.runtime_dir.join(format!("{}.{ext}", ws.id)));
    let project_lock = paths.runtime_dir.join(format!("{}.lock", ws.id));
    for file in leftovers.iter().chain([&project_lock]) {
        std::fs::write(file, "")?;
    }
    assert!(daemon::status(&paths).await?.is_none(), "status never trusts a port file");

    let client = daemon::ensure_daemon(&paths, &ws, spawn()).await?;
    let pf = daemon::read_port_file(&paths.port_file())?.expect("port file");
    assert_ne!(pf.instance_id, "stale");
    assert_eq!(pf.port, client.port);
    for file in &leftovers {
        assert!(!file.exists(), "{} is retired at start", file.display());
    }
    assert!(daemon::try_lock(&project_lock)?.is_none(), "the daemon holds the lock of the project it hosts");
    assert!(daemon::stop(&paths).await?);
    Ok(())
}

/// A crash can leave a port file naming a port that another daemon (a scratch one, another
/// user's) listens on now. Only a daemon that answers as the instance the file names counts,
/// so neither `status` nor `stop` reaches the other one, and the next command starts a daemon
/// of its own.
#[tokio::test]
async fn a_port_file_naming_another_daemons_port_is_not_trusted() -> Result<()> {
    let (_dir, paths, ws) = setup()?;
    let (_other_dir, other_paths, other_ws) = setup()?;
    let other = daemon::ensure_daemon(&other_paths, &other_ws, spawn()).await?;
    paths.ensure()?;
    daemon::write_port_file(
        &paths.port_file(),
        &PortFile { port: other.port, pid: 1, instance_id: "crashed".into(), started: 0, bind: Bind::Loopback },
    )?;
    assert!(daemon::status(&paths).await?.is_none(), "status does not take the other daemon for this one");
    assert!(!daemon::stop(&paths).await?, "stop finds no daemon of its own");
    assert_eq!(daemon::status(&other_paths).await?.expect("the other daemon still runs").pf, other.pf);

    let mine = daemon::ensure_daemon(&paths, &ws, spawn()).await?;
    assert_ne!(mine.pf.instance_id, other.pf.instance_id, "a daemon of its own serves this data directory");
    drop(mine);
    drop(other);
    assert!(daemon::stop(&paths).await?);
    assert!(daemon::stop(&other_paths).await?);
    Ok(())
}

/// A per-project daemon from an earlier build that still runs, played by a process of its own:
/// this test binary run again as `EARLIER_DAEMON`, holding a project's `<uuid>.lock` until the
/// test stops it. The lock has to be held by another process, as such a daemon holds it, and not
/// by the test's own: a child this process is spawning (the other tests start daemons all the
/// time) holds a copy of every descriptor this process has open until its exec closes them, so a
/// lock this process had just released could still be held for a moment, and the daemon asked
/// to open the project in that moment refused it as held ("a daemon from an earlier build of fwd
/// still has this project open", 2026-09-28 and again 2026-09-29, under load).
struct EarlierDaemon {
    process: std::process::Child,
    /// Its standard output, kept open so it can go on writing (the test harness reports a test
    /// running for over a minute there) without being stopped by a closed pipe.
    output: std::io::Lines<std::io::BufReader<std::process::ChildStdout>>,
}

/// The test `EarlierDaemon` runs in its process.
const EARLIER_DAEMON: &str = "hold_a_projects_lock_as_an_earlier_builds_daemon";
/// The variable that names the lock that process holds.
const EARLIER_DAEMON_LOCK: &str = "FWD_TEST_EARLIER_DAEMON_LOCK";
/// The line that process prints once it holds the lock.
const HOLDING: &str = "holding the project's lock";

impl EarlierDaemon {
    /// Starts the process, and returns once it holds `lock`.
    fn holding(lock: &Path) -> Result<EarlierDaemon> {
        let mut process = std::process::Command::new(std::env::current_exe()?)
            .args([EARLIER_DAEMON, "--exact", "--ignored", "--nocapture"])
            .env(EARLIER_DAEMON_LOCK, lock)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        let output = std::io::BufRead::lines(std::io::BufReader::new(process.stdout.take().expect("piped")));
        // Ended by `Drop` should the process fail to take the lock.
        let mut earlier = EarlierDaemon { process, output };
        loop {
            match earlier.output.next().transpose()? {
                Some(line) if line.ends_with(HOLDING) => return Ok(earlier),
                Some(_) => {}
                None => return Err(anyhow!("the earlier build's daemon ended before it held {}", lock.display())),
            }
        }
    }

    /// The process's pid, which the daemon names when it refuses the project.
    fn pid(&self) -> u32 {
        self.process.id()
    }

    /// Stops the process and returns once it has exited: its lock is free then, as an earlier
    /// build's daemon's is once `kill <pid>` has stopped it.
    fn stop(mut self) -> Result<()> {
        self.process.kill()?;
        self.process.wait()?;
        Ok(())
    }
}

impl Drop for EarlierDaemon {
    /// Stops the process of a test that failed before it stopped it.
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

/// The earlier build's daemon, as `EarlierDaemon` runs it in a process of its own: takes the lock
/// `EARLIER_DAEMON_LOCK` names, says so, and holds it until the test stops the process, or until
/// the test's own process ends and so closes this one's standard input. Run any other way, it
/// does nothing.
#[test]
#[ignore = "the lock tests run it in a process of its own, as a daemon from an earlier build"]
fn hold_a_projects_lock_as_an_earlier_builds_daemon() -> Result<()> {
    let Some(lock) = std::env::var_os(EARLIER_DAEMON_LOCK) else { return Ok(()) };
    let _held = daemon::try_lock(Path::new(&lock))?.ok_or_else(|| anyhow!("something else holds {lock:?}"))?;
    println!("{HOLDING}");
    std::io::Read::read_to_end(&mut std::io::stdin(), &mut Vec::new())?;
    Ok(())
}

/// A per-project daemon from an earlier build that still runs holds `<uuid>.lock` and has the
/// project's database open. The daemon leaves that project to it, naming its pid, and hosts the
/// others; it holds the lock of every project it hosts, so such a build cannot open one of them
/// a second time; and it opens the held project on request once that daemon has stopped.
#[tokio::test]
async fn a_project_an_earlier_builds_daemon_holds_is_left_to_it_until_its_lock_frees() -> Result<()> {
    let (dir, paths, held) = setup()?;
    let other = new_project(&paths, &dir.path().join("other"), "other")?;
    paths.ensure()?;
    let lock_of = |project: &Project| paths.runtime_dir.join(format!("{}.lock", project.id));
    // What that daemon holds and wrote: the project's lock, and a port file with its pid.
    let earlier = EarlierDaemon::holding(&lock_of(&held))?;
    let pid = earlier.pid();
    let earlier_port_file = paths.runtime_dir.join(format!("{}.ws", held.id));
    let written =
        serde_json::json!({"port": 1, "pid": pid, "instance_id": "earlier", "started": 0, "project": held.id});
    std::fs::write(&earlier_port_file, written.to_string())?;

    let client = daemon::ensure_daemon(&paths, &other, spawn()).await?;
    let running = daemon::status(&paths).await?.expect("the daemon answers");
    let error_of =
        |id| running.projects.iter().find(|p| p.id == id).expect("every indexed project is listed").error.clone();
    let error = error_of(held.id).expect("the held project is not opened");
    assert!(error.contains(&format!("pid {pid}")), "the error names the earlier daemon's pid: {error}");
    assert_eq!(error_of(other.id), None, "the other project opens");
    assert!(daemon::try_lock(&lock_of(&other))?.is_none(), "the daemon holds the lock of the project it hosts");
    assert!(earlier_port_file.exists(), "the files of a daemon that still runs stay");
    let refused = daemon::ensure_daemon(&paths, &held, spawn()).await.err().expect("the held project is refused");
    assert!(format!("{refused:#}").contains(&format!("pid {pid}")), "{refused:#}");

    earlier.stop()?;
    daemon::host(&client.pf, &held).await?;
    let running = daemon::status(&paths).await?.expect("the daemon answers");
    assert!(running.projects.iter().any(|p| p.id == held.id && p.error.is_none()), "{:?}", running.projects);
    assert!(daemon::try_lock(&lock_of(&held))?.is_none(), "the daemon holds the lock now");
    let reached = daemon::ensure_daemon(&paths, &held, spawn()).await?;
    assert_eq!(reached.pf.instance_id, client.pf.instance_id);

    drop(reached);
    drop(client);
    assert!(daemon::stop(&paths).await?);
    Ok(())
}

/// The storage layout moved on 2026-09-25 (`workspaces/<id>/workspace.sqlite` to
/// `projects/<id>/project.sqlite`). SQLite keeps committed writes in the write-ahead log until a
/// checkpoint, so a daemon that died without one leaves them there: the migration must move the
/// log with the database, or the writes are gone.
#[tokio::test]
async fn a_killed_daemons_uncheckpointed_writes_survive_the_layout_migration() -> Result<()> {
    let (_dir, paths, ws) = setup()?;
    let client = daemon::ensure_daemon(&paths, &ws, spawn()).await?;
    let pid = client.pf.pid;
    let me = ops::ensure_principal(&client.ctx, "d", "D", PrincipalKind::Human, None).await?;
    let task = ops::create_task(&client.ctx, me, CreateTask { title: "survives".into(), ..Default::default() }).await?;
    drop(client);

    // No graceful stop: nothing checkpoints the log. The kernel frees the killed daemon's lock as
    // its process exits, within milliseconds on a loaded machine too; the ten seconds only bound a
    // daemon that does not die.
    assert!(std::process::Command::new("kill").args(["-9", &pid.to_string()]).status()?.success());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while daemon::daemon_alive(&paths)? {
        assert!(std::time::Instant::now() < deadline, "the killed daemon still holds its lock");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let _ = std::fs::remove_file(paths.port_file());

    // Lay the files out the way the old binary left them.
    let new = paths.storage_dir(&ws);
    let old = paths.data_dir.join("workspaces").join(ws.id.to_string());
    std::fs::create_dir_all(old.parent().unwrap())?;
    std::fs::rename(&new, &old)?;
    for suffix in ["", "-wal", "-shm"] {
        let from = old.join(format!("project.sqlite{suffix}"));
        if from.exists() {
            std::fs::rename(&from, old.join(format!("workspace.sqlite{suffix}")))?;
        }
    }
    assert!(old.join("workspace.sqlite-wal").exists(), "the kill left the write-ahead log behind");

    let client = daemon::ensure_daemon(&paths, &ws, spawn()).await?;
    let found: TaskView = client.ctx.get(task).await?;
    assert_eq!(found.title()?, "survives");
    assert!(!old.exists(), "the legacy directory is gone");
    assert!(paths.db_path(&ws).exists());
    drop(client);
    assert!(daemon::stop(&paths).await?);
    Ok(())
}

#[tokio::test]
async fn one_daemon_hosts_every_project_and_each_is_reachable() -> Result<()> {
    let (dir, paths, first) = setup()?;
    let second = new_project(&paths, &dir.path().join("other"), "other")?;
    let a = daemon::ensure_daemon(&paths, &first, spawn()).await?;
    let b = daemon::ensure_daemon(&paths, &second, spawn()).await?;
    assert_eq!(a.pf.instance_id, b.pf.instance_id, "one daemon serves both projects");
    assert_ne!(a.ws.server_node_id(), b.ws.server_node_id(), "each project has its own durable node");

    let running = daemon::status(&paths).await?.expect("the daemon answers");
    let listed: Vec<(&str, bool)> = running.projects.iter().map(|p| (p.name.as_str(), p.error.is_none())).collect();
    assert_eq!(listed, vec![("other", true), ("test", true)], "listed by name, both open");

    // Each client writes into its own project's graph, and only there.
    let me = ops::ensure_principal(&a.ctx, "d", "D", PrincipalKind::Human, None).await?;
    let task = ops::create_task(&a.ctx, me, CreateTask { title: "first only".into(), ..Default::default() }).await?;
    let you = ops::ensure_principal(&b.ctx, "e", "E", PrincipalKind::Human, None).await?;
    ops::create_task(&b.ctx, you, CreateTask { title: "second only".into(), ..Default::default() }).await?;
    let titles = |tasks: Vec<TaskView>| tasks.iter().map(|t| t.title().unwrap_or_default()).collect::<Vec<_>>();
    assert_eq!(titles(a.ctx.fetch::<TaskView>("created > 0").await?), vec!["first only"]);
    assert_eq!(titles(b.ctx.fetch::<TaskView>("created > 0").await?), vec!["second only"]);
    assert_eq!(a.ctx.get::<TaskView>(task).await?.title()?, "first only");

    drop(a);
    drop(b);
    assert!(daemon::stop(&paths).await?);
    Ok(())
}

#[tokio::test]
async fn a_project_created_while_the_daemon_runs_is_hosted_without_a_restart() -> Result<()> {
    let (dir, paths, first) = setup()?;
    let client = daemon::ensure_daemon(&paths, &first, spawn()).await?;
    let before = client.pf.clone();

    let repo = dir.path().join("later");
    std::fs::create_dir_all(&repo)?;
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_fwd"))
        .args(["init", "--new", "--name", "later", "--handle", "d", "--format", "json"])
        .current_dir(&repo)
        .env("FWD_RUNTIME_DIR", &paths.runtime_dir)
        .env("FWD_DATA_DIR", &paths.data_dir)
        .env_remove("FWD_PROJECT")
        .env_remove("FWD_AS")
        .env_remove("FWD_SCOPE")
        .env_remove("FWD_DAEMON_BIN")
        .output()?;
    assert!(out.status.success(), "fwd init --new failed: {}", String::from_utf8_lossy(&out.stderr));
    let init: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    assert_eq!(init["project"]["created"], true);
    let later =
        Project { id: init["project"]["id"].as_str().expect("the new project's id").parse()?, name: "later".into() };

    let running = daemon::status(&paths).await?.expect("the daemon answers");
    assert_eq!(running.pf.instance_id, before.instance_id, "the daemon that was running hosts it; no restart");
    assert!(running.projects.iter().any(|p| p.id == later.id && p.error.is_none()), "{:?}", running.projects);
    let reached = daemon::ensure_daemon(&paths, &later, spawn()).await?;
    assert_eq!(reached.pf.instance_id, before.instance_id);
    assert!(
        naming::principal_named(&reached.ctx, "d", None).await?.is_some(),
        "init recorded its person in the new project's graph"
    );

    drop(reached);
    drop(client);
    assert!(daemon::stop(&paths).await?);
    Ok(())
}

/// Sutra, a dashboard of the dev runners on a machine, shows the daemon from the files it keeps
/// in sutra's registry: the card, naming the data directory, the daemon's pid and its port, with
/// one row, `daemon`, that counts the projects open (not one another daemon holds, until it opens
/// on request) and no row per project; and nothing once the daemon has stopped.
#[tokio::test]
async fn sutra_shows_the_daemon_while_it_runs_and_nothing_once_it_stops() -> Result<()> {
    let (dir, paths, first) = setup()?;
    new_project(&paths, &dir.path().join("second"), "My Project!")?;
    let held = new_project(&paths, &dir.path().join("held"), "held")?;
    paths.ensure()?;
    // As a per-project daemon from an earlier build would.
    let earlier = EarlierDaemon::holding(&paths.runtime_dir.join(format!("{}.lock", held.id)))?;

    let client = daemon::ensure_daemon(&paths, &first, spawn()).await?;
    let pf = daemon::read_port_file(&paths.port_file())?.expect("port file");
    let registry = common::registry();
    let id = format!("{:x}", Sha256::digest(paths.data_dir.as_os_str().as_encoded_bytes()))[..16].to_string();
    // This daemon's files; the registry also holds those of the daemons the other tests run.
    let files = || -> Result<Vec<String>> {
        let mut names: Vec<String> = std::fs::read_dir(&registry)?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| *name == id || name.starts_with(&format!("{id}.")))
            .collect();
        names.sort();
        Ok(names)
    };
    assert_eq!(files()?, [id.clone(), format!("{id}.daemon.status")], "the card and its one row");
    let meta: BTreeMap<String, String> = std::fs::read_to_string(registry.join(&id))?
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    assert_eq!(meta.keys().collect::<Vec<_>>(), ["DAEMON_PORT", "DIR", "PID", "STARTED"], "{meta:?}");
    assert_eq!(meta["DIR"], paths.data_dir.display().to_string());
    assert_eq!(meta["PID"], pf.pid.to_string(), "the pid sutra checks and signals is the daemon's");
    assert!(meta["STARTED"].parse::<u64>().is_ok(), "STARTED is Unix seconds: {meta:?}");
    assert_eq!(meta["DAEMON_PORT"], pf.port.to_string(), "the row carries the daemon's port");
    let row = || std::fs::read_to_string(registry.join(format!("{id}.daemon.status")));
    assert_eq!(row()?, "ready: 2 projects\n", "the project another daemon holds is not counted");

    assert!(daemon::host(&client.pf, &held).await.is_err(), "the earlier daemon still holds it");
    assert_eq!(row()?, "ready: 2 projects\n", "a project that fails to open changes nothing on the card");
    earlier.stop()?;
    daemon::host(&client.pf, &held).await?;
    assert_eq!(row()?, "ready: 3 projects\n", "a project that opens on request is counted");

    drop(client);
    assert!(daemon::stop(&paths).await?);
    let left = files()?;
    assert!(left.is_empty(), "the daemon's files are gone after the stop: {left:?}");
    Ok(())
}

/// Every agent loop waits in `fwd watch`, with no timeout, so a daemon restart must neither end a
/// watch nor leave it deaf, and says nothing on its output (Daniel, 2026-09-26): after the restart,
/// a question to the watcher is printed like any other notification, and nothing else reaches its
/// standard output or its standard error.
#[tokio::test]
async fn a_watch_hears_across_a_daemon_restart_and_says_nothing_of_it() -> Result<()> {
    watch_across_a_restart(Duration::ZERO, Restart::SamePort).await
}

/// A daemon whose last port is taken when it starts listens on another; Ankurah's socket client
/// keeps knocking on the old port, and the watch follows the port file to the new one.
#[tokio::test]
async fn a_watch_follows_the_daemon_to_another_port() -> Result<()> {
    watch_across_a_restart(Duration::ZERO, Restart::PortTaken).await
}

/// The failure the watch was changed for: a watcher that had waited ten minutes had lost its store
/// (`fwd_core::daemon`, `ephemeral_store`), so it missed the next notification, and after a
/// restart every one of its queries failed for good. The watch, with no timeout, also outlives the
/// 540 seconds that were its default until 2026-09-29. Run it on request:
/// `cargo test -p fwd --test lifecycle -- --ignored`.
#[tokio::test]
#[ignore = "waits eleven minutes"]
async fn a_watch_that_waited_ten_minutes_still_hears_before_and_after_a_restart() -> Result<()> {
    watch_across_a_restart(Duration::from_secs(11 * 60), Restart::SamePort).await
}

/// `--timeout`, when given, still ends a watch: one given two seconds, with nothing to print,
/// prints `timeout` no sooner, and exits 0 with nothing on standard error.
#[tokio::test]
async fn a_watch_given_a_timeout_ends_once_it_has_passed() -> Result<()> {
    let (dir, paths, ws) = setup()?;
    let client = daemon::ensure_daemon(&paths, &ws, spawn()).await?;
    // `--as` names a principal the project has, and never makes one.
    ops::ensure_principal(&client.ctx, "impl-watch", "impl-watch", PrincipalKind::Agent, Some(AgentRole::Implementer))
        .await?;
    let started = std::time::Instant::now();
    let mut watch = Watch::start(dir.path(), &paths, &["--count", "1", "--timeout", "2"])?;
    let line = next_line(&mut watch.lines).await?;
    assert_eq!(line, serde_json::json!({"event": "timeout"}));
    assert!(started.elapsed() >= Duration::from_secs(2), "the watch ended {:?} after it started", started.elapsed());
    watch.ends().await?;

    drop(client);
    assert!(daemon::stop(&paths).await?);
    Ok(())
}

enum Restart {
    SamePort,
    /// Something else holds the daemon's last port while it starts again.
    PortTaken,
}

/// Starts `fwd watch --count 3`, with no timeout as an agent runs it, for a principal with a
/// question waiting, waits `idle`, asks a second question, restarts the daemon and asks a third.
/// The watch prints the waiting question as its snapshot, the other two as notifications, and
/// `done`, and nothing else; it exits 0 and writes nothing on standard error.
async fn watch_across_a_restart(idle: Duration, restart: Restart) -> Result<()> {
    let (dir, paths, ws) = setup()?;
    let client = daemon::ensure_daemon(&paths, &ws, spawn()).await?;
    let asker = ops::ensure_principal(&client.ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let watcher = ops::ensure_principal(
        &client.ctx,
        "impl-watch",
        "impl-watch",
        PrincipalKind::Agent,
        Some(AgentRole::Implementer),
    )
    .await?;
    ask(&client, asker, watcher, "waiting when the watch starts").await?;
    // Waiting until the notification exists makes it the snapshot, the watch's first line, which it
    // prints once its query is registered with the daemon. The routing service makes it within
    // milliseconds of the question, on a loaded machine too; the ten seconds only bound a service
    // that never does.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while ops::inbox(&client.ctx, watcher, true).await?.is_empty() {
        assert!(std::time::Instant::now() < deadline, "routing made no notification of the first question");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let mut watch = Watch::start(dir.path(), &paths, &["--count", "3"])?;

    let snapshot = next_line(&mut watch.lines).await?;
    assert_eq!(snapshot["event"], "snapshot", "{snapshot}");
    tokio::time::sleep(idle).await;
    ask(&client, asker, watcher, "after the wait").await?;
    expect_notification(&mut watch.lines, "after the wait").await?;

    drop(client);
    let before = daemon::read_port_file(&paths.port_file())?.expect("port file");
    assert!(daemon::stop(&paths).await?);
    // Bound while the daemon is down, so the next one finds its last port taken.
    let _holder = match restart {
        Restart::SamePort => None,
        Restart::PortTaken => Some(std::net::TcpListener::bind(("127.0.0.1", before.port))?),
    };
    let client = daemon::ensure_daemon(&paths, &ws, spawn()).await?;
    match restart {
        Restart::SamePort => assert_eq!(client.pf.port, before.port, "the daemon came back on its port"),
        Restart::PortTaken => assert_ne!(client.pf.port, before.port, "the daemon came back on another port"),
    }
    ask(&client, asker, watcher, "after the restart").await?;
    expect_notification(&mut watch.lines, "after the restart").await?;

    let done = next_line(&mut watch.lines).await?;
    assert_eq!(done, serde_json::json!({"event": "done", "count": 3}));
    watch.ends().await?;

    drop(client);
    assert!(daemon::stop(&paths).await?);
    Ok(())
}

/// A `fwd watch` run as `impl-watch` in the project's checkout, as an agent runs it: its output
/// line by line, and its standard error whole once it has exited.
struct Watch {
    child: tokio::process::Child,
    lines: WatchOutput,
    stderr: tokio::task::JoinHandle<std::io::Result<String>>,
}

impl Watch {
    /// Starts `fwd --as impl-watch watch --format json` with `args`, against the daemon of `paths`.
    fn start(dir: &Path, paths: &Paths, args: &[&str]) -> Result<Watch> {
        let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_fwd"))
            .args(["--as", "impl-watch", "watch", "--format", "json"])
            .args(args)
            .current_dir(dir.join("repo"))
            .env("FWD_RUNTIME_DIR", &paths.runtime_dir)
            .env("FWD_DATA_DIR", &paths.data_dir)
            .env_remove("FWD_PROJECT")
            .env_remove("FWD_AS")
            .env_remove("FWD_SCOPE")
            .env_remove("FWD_DAEMON_BIN")
            // The CLI's own log filter, which keeps a restart quiet, applies only without RUST_LOG.
            .env_remove("RUST_LOG")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let lines = BufReader::new(child.stdout.take().expect("piped")).lines();
        let mut stderr = child.stderr.take().expect("piped");
        let stderr = tokio::spawn(async move {
            let mut text = String::new();
            stderr.read_to_string(&mut text).await.map(|_| text)
        });
        Ok(Watch { child, lines, stderr })
    }

    /// After the watch's last line has been read: within a second its output ends and it exits 0,
    /// and it wrote nothing on standard error.
    async fn ends(mut self) -> Result<()> {
        // The watch ends its process as it prints its last line (`watch::end`), and its output
        // ends within a few milliseconds: 4 ms at most in ninety exits while two builds loaded the
        // machine (load averages 16 to 62, 2026-09-29). When it returned through its runtime
        // instead, twice in thirty runs under load it never exited: the runtime waited for a thread
        // that Ankurah had deadlocked while closing the node the watch had left for the daemon's
        // new port. The second fails such a watch rather than waiting for it.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        let lingers = || anyhow!("the watch had not exited a second after its last line");
        let after = tokio::time::timeout_at(deadline, self.lines.next_line()).await.map_err(|_| lingers())??;
        assert!(after.is_none(), "the watch printed nothing after its last line: {after:?}");
        let status = tokio::time::timeout_at(deadline, self.child.wait()).await.map_err(|_| lingers())??;
        assert!(status.success(), "the watch exited with {status}");
        let stderr = self.stderr.await??;
        assert!(stderr.is_empty(), "the watch wrote on standard error:\n{stderr}");
        Ok(())
    }
}

type WatchOutput = tokio::io::Lines<BufReader<tokio::process::ChildStdout>>;

/// The watch's next line of output, as JSON; an error after a minute of silence, or at its end.
/// Each line comes within a few seconds of what it reports, the one after a restart included (the
/// whole test takes three to five seconds on a loaded machine); the minute only bounds a watch
/// that has stopped hearing.
async fn next_line(lines: &mut WatchOutput) -> Result<Value> {
    let line = tokio::time::timeout(Duration::from_secs(60), lines.next_line())
        .await
        .map_err(|_| anyhow!("the watch printed nothing for 60s"))??
        .ok_or_else(|| anyhow!("the watch's output ended"))?;
    Ok(serde_json::from_str(&line)?)
}

/// The watch's next line is the notification of the question `content`, with its message.
async fn expect_notification(lines: &mut WatchOutput, content: &str) -> Result<()> {
    let heard = next_line(lines).await?;
    assert_eq!(heard["event"], "notification", "{heard}");
    assert_eq!(heard["message"]["content"], content, "{heard}");
    Ok(())
}

/// Asks `to` a question, as `fwd ask --to` does.
async fn ask(client: &daemon::Client, from: EntityId, to: EntityId, text: &str) -> Result<()> {
    let to = MessageContext { to: Some(to.to_base64()), ..Default::default() };
    ops::post_message(&client.ctx, from, text, Priority::Now, to).await?;
    Ok(())
}

/// A command caught waiting on its daemon when the daemon stops, which comes back on another port,
/// ends at once, with exit 7 and an error naming the port it joined and the one the daemon listens
/// on now (task tXSgMMXC). Until 2026-09-30 such a command never ended: its node retried only the
/// port it had joined, and waited for good for the answers to its first queries.
#[tokio::test]
async fn a_command_whose_daemon_comes_back_on_another_port_ends_with_an_error_naming_it() -> Result<()> {
    command_losing_its_daemon(Comeback::OtherPort).await
}

/// A command caught waiting on its daemon when the daemon stops, and none comes back, ends once
/// its socket has been closed for `LOST_AFTER`, with exit 7 and an error saying so.
#[tokio::test]
async fn a_command_whose_daemon_does_not_come_back_ends_once_the_bound_has_passed() -> Result<()> {
    command_losing_its_daemon(Comeback::None).await
}

enum Comeback {
    /// The daemon starts again, on its own last port, which is not the port the command joined.
    OtherPort,
    /// Nothing starts a daemon again.
    None,
}

/// Starts `fwd task list` against a daemon it joins through a [`Relay`], which holds back what the
/// daemon says after its presence, so the command waits for its node to be ready, where commands
/// waited for good before; then stops the daemon, and starts it again when `comeback` says so. The
/// command must end with exit 7 and the error for what became of its daemon, within the bound.
async fn command_losing_its_daemon(comeback: Comeback) -> Result<()> {
    let (dir, paths, ws) = setup()?;
    let client = daemon::ensure_daemon(&paths, &ws, spawn()).await?;
    let joined = client.pf.clone();
    drop(client);
    let relay = Relay::start(joined.port, Hold::AfterPresence).await?;
    // The port file names the relay's port, so the command joins the daemon through the relay: the
    // daemon's `/health` through the relay gives the instance and the pid the file names.
    daemon::write_port_file(&paths.port_file(), &PortFile { port: relay.port, ..joined.clone() })?;
    let command = fwd(dir.path(), &paths, &["task", "list"]).spawn()?;
    // The daemon has answered one of the first queries of the command's node, and the relay holds
    // the answer back: the command waits on the daemon.
    tokio::time::timeout(Duration::from_secs(30), relay.holding.notified())
        .await
        .map_err(|_| anyhow!("the daemon answered no query of the command's node within 30s"))?;

    // The daemon stops. The relay's port refuses from then on, as a port the daemon left does, and
    // the port file names the daemon again, so that the stop reaches it.
    let port = relay.close().await;
    daemon::write_port_file(&paths.port_file(), &joined)?;
    assert!(daemon::stop(&paths).await?);
    let stopped = std::time::Instant::now();
    let (then, within) = match comeback {
        Comeback::OtherPort => {
            let client = daemon::ensure_daemon(&paths, &ws, spawn()).await?;
            assert_ne!(client.pf.port, port, "the daemon came back on another port than the command joined");
            (format!(", and the daemon came back on port {}", client.pf.port), LOST_AFTER)
        }
        Comeback::None => (format!(" and has not come back in {}s", LOST_AFTER.as_secs()), LOST_AFTER * 2),
    };
    let ended = tokio::time::timeout(LOST_AFTER * 3, command.wait_with_output())
        .await
        .map_err(|_| anyhow!("the command still ran {}s after its daemon stopped", (LOST_AFTER * 3).as_secs()))??;
    let took = stopped.elapsed();

    let stderr = String::from_utf8(ended.stderr)?;
    let error = format!(
        "Error: the connection to the daemon on port {port} closed while this command ran{then}; run the command again"
    );
    assert!(stderr.lines().any(|line| line == error), "the command said, on standard error:\n{stderr}");
    assert_eq!(ended.status.code(), Some(7), "the command said, on standard error:\n{stderr}");
    assert!(ended.stdout.is_empty(), "the command printed {:?}", String::from_utf8_lossy(&ended.stdout));
    assert!(took < within, "the command ended {took:?} after its daemon stopped");
    if let Comeback::None = comeback {
        // The command waited for its daemon to come back before it gave up, as a daemon restarting
        // on the same port would have let it carry on.
        assert!(took >= LOST_AFTER - Duration::from_secs(1), "the command ended {took:?} after its daemon stopped");
    }

    if let Comeback::OtherPort = comeback {
        assert!(daemon::stop(&paths).await?);
    }
    Ok(())
}

/// A write whose socket closes and comes back while it runs is not reported as made, although
/// Ankurah reported its commit made: made while the socket was closed, the commit went to the
/// command's own node alone, and the daemon, back on its port, holds nothing of it; the command
/// ends with the error saying so (task NRrogFlb). Until 2026-09-30 the command reported it made.
#[tokio::test]
async fn a_write_committed_while_its_daemon_was_gone_is_not_reported_as_made() -> Result<()> {
    let (_dir, paths, ws) = setup()?;
    let client = daemon::ensure_daemon(&paths, &ws, spawn()).await?;
    let port = client.pf.port;
    let (gone, when_gone) = tokio::sync::oneshot::channel();
    let (committed, when_committed) = tokio::sync::oneshot::channel();
    let (back, when_back) = tokio::sync::oneshot::channel();
    // The write commits once the daemon has gone, and ends once the socket is open again.
    let write = client.while_writing(async {
        when_gone.await?;
        let trx = client.ctx.begin();
        let made = trx
            .create(&Principal {
                kind: PrincipalKind::Agent,
                handle: "impl-unheard".into(),
                display_name: "impl-unheard".into(),
                auth_subject: None,
                agent_role: Some(AgentRole::Implementer),
                active: true,
                member: Some(true),
                subject: None,
                succeeded_by: None,
                serves: None,
                last_active: None,
            })
            .await?
            .id();
        trx.commit().await?;
        let _ = committed.send(made);
        when_back.await?;
        Ok(made)
    });
    let away_and_back = async {
        assert!(daemon::stop(&paths).await?);
        // With no durable peer left, the node commits to its own store alone.
        until("the command's node to lose the daemon", || client.node.get_durable_peers().is_empty()).await?;
        let _ = gone.send(());
        let made = when_committed.await.map_err(|_| anyhow!("the write never committed"))?;
        let again = daemon::ensure_daemon(&paths, &ws, spawn()).await?;
        assert_eq!(again.pf.port, port, "the daemon came back on the port the command joined");
        until("the command's socket to open again", || client.ws.is_connected()).await?;
        let _ = back.send(());
        anyhow::Ok((again, made))
    };
    let (written, away_and_back) = tokio::join!(write, away_and_back);
    let (again, made) = away_and_back.map_err(|e| anyhow!("{e:#}; the write ended with {written:?}"))?;
    let error = written.expect_err("the write was reported as made");
    assert_eq!(error.downcast_ref::<daemon::Lost>(), Some(&daemon::Lost::Unconfirmed { port }), "{error:#}");
    assert!(ops::principals(&again.ctx).await?.iter().all(|p| p.id() != made), "the daemon holds the write");

    drop((client, again));
    assert!(daemon::stop(&paths).await?);
    Ok(())
}

/// A write whose socket closed after the command joined the daemon, and before the write began,
/// begins nothing, and ends with the error saying so (task NRrogFlb).
#[tokio::test]
async fn a_write_whose_daemon_went_before_it_began_begins_nothing() -> Result<()> {
    let (_dir, paths, ws) = setup()?;
    let client = daemon::ensure_daemon(&paths, &ws, spawn()).await?;
    let port = client.pf.port;
    assert!(daemon::stop(&paths).await?);
    until("the command's socket to close", || !client.ws.is_connected()).await?;
    let mut began = false;
    let refused = client
        .while_writing(async {
            began = true;
            anyhow::Ok(())
        })
        .await;
    assert!(!began, "the write began");
    let error = refused.expect_err("the write was not refused");
    assert_eq!(error.downcast_ref::<daemon::Lost>(), Some(&daemon::Lost::BeforeWriting { port }), "{error:#}");
    Ok(())
}

/// `fwd comment` caught in its pre-checks when its daemon stops ends at once, with exit 7 and the
/// error saying that what it wrote may not have reached the daemon, and the daemon, once back,
/// holds no comment on the task (task NRrogFlb).
#[tokio::test]
async fn a_write_caught_before_its_commit_ends_saying_its_write_may_not_have_reached_the_daemon() -> Result<()> {
    caught_on_the_way(Caught::Write).await
}

/// `fwd task show` caught at the same spot ends as any command whose daemon goes away does.
#[tokio::test]
async fn a_read_caught_at_the_same_spot_ends_as_before() -> Result<()> {
    caught_on_the_way(Caught::Read).await
}

enum Caught {
    /// `fwd comment` on the task.
    Write,
    /// `fwd task show` of the task.
    Read,
}

/// Starts a command on a task through a [`Relay`] that holds back what the daemon says from the
/// command's first request naming the task, which the command makes once its node is ready and it
/// knows whom it acts as; then stops the daemon. The command must end at once, with exit 7 and the
/// error for what it does, and nothing of a write may reach the daemon.
async fn caught_on_the_way(caught: Caught) -> Result<()> {
    let (dir, paths, ws) = setup()?;
    let client = daemon::ensure_daemon(&paths, &ws, spawn()).await?;
    let me = ops::ensure_principal(&client.ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let spec = CreateTask { title: "Hear of a lost write".into(), ..Default::default() };
    let task = ops::create_task(&client.ctx, me, spec).await?;
    let joined = client.pf.clone();
    drop(client);
    let relay = Relay::start(joined.port, Hold::AfterNaming(task)).await?;
    daemon::write_port_file(&paths.port_file(), &PortFile { port: relay.port, ..joined.clone() })?;
    let id = task.to_base64();
    let args = match caught {
        Caught::Write => ["comment", id.as_str(), "Written as the daemon went"],
        Caught::Read => ["task", "show", id.as_str()],
    };
    let command = fwd(dir.path(), &paths, &args).spawn()?;
    tokio::time::timeout(Duration::from_secs(30), relay.holding.notified())
        .await
        .map_err(|_| anyhow!("the command named the task to the daemon in no request within 30s"))?;

    let port = relay.close().await;
    daemon::write_port_file(&paths.port_file(), &joined)?;
    assert!(daemon::stop(&paths).await?);
    // Every read made once the socket has closed fails at once, so the command ends at once.
    let ended = tokio::time::timeout(LOST_AFTER, command.wait_with_output())
        .await
        .map_err(|_| anyhow!("the command still ran {}s after its daemon stopped", LOST_AFTER.as_secs()))??;
    let stderr = String::from_utf8(ended.stderr)?;
    let error = match caught {
        Caught::Write => format!(
            "Error: the connection to the daemon on port {port} closed while this command ran, so what it wrote may not have reached the daemon; look (fwd task show, fwd message show) before writing it again"
        ),
        Caught::Read => {
            format!("Error: the connection to the daemon on port {port} closed while this command ran; run the command again")
        }
    };
    assert!(stderr.lines().any(|line| line == error), "the command said, on standard error:\n{stderr}");
    assert_eq!(ended.status.code(), Some(7), "the command said, on standard error:\n{stderr}");
    assert!(ended.stdout.is_empty(), "the command printed {:?}", String::from_utf8_lossy(&ended.stdout));

    if let Caught::Write = caught {
        let client = daemon::ensure_daemon(&paths, &ws, spawn()).await?;
        let said = ops::messages_about(&client.ctx, "task", task).await?;
        assert!(said.is_empty(), "the daemon holds {} message(s) on the task", said.len());
        drop(client);
        assert!(daemon::stop(&paths).await?);
    }
    Ok(())
}

/// `fwd --no-autostart --format json <args>` run as the person `dnorman`, in the project's
/// checkout, against the daemon of `paths`, with its output piped, and killed when dropped.
fn fwd(dir: &Path, paths: &Paths, args: &[&str]) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_fwd"));
    command
        .args(["--no-autostart", "--format", "json"])
        .args(args)
        .current_dir(dir.join("repo"))
        .env("FWD_RUNTIME_DIR", &paths.runtime_dir)
        .env("FWD_DATA_DIR", &paths.data_dir)
        .env("FWD_HANDLE", "dnorman")
        .env_remove("FWD_PROJECT")
        .env_remove("FWD_AS")
        .env_remove("FWD_SCOPE")
        .env_remove("FWD_DAEMON_BIN")
        .env_remove("FWD_CLAUDE_SESSION")
        .env_remove("FWD_CLAUDE_AGENT")
        .env_remove("CLAUDE_CODE_SESSION_ID")
        // The CLI's own log filter applies only without RUST_LOG.
        .env_remove("RUST_LOG")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

/// Waits until `now` holds, looking every 20ms; an error naming what it waited for after 30s.
async fn until(what: &str, now: impl Fn() -> bool) -> Result<()> {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !now() {
        if std::time::Instant::now() >= deadline {
            return Err(anyhow!("waited 30s for {what}"));
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Ok(())
}

/// A stand-in for the daemon's port, between a command and the daemon, which lets a test catch a
/// command waiting on the daemon: it relays each connection to the daemon, and on a socket holds
/// back whatever the daemon says from the moment [`Hold`] names, so the command's node waits for
/// the answer to a query of its own.
struct Relay {
    port: u16,
    listener: tokio::task::JoinHandle<()>,
    /// Told once the relay holds back something the daemon said.
    holding: std::sync::Arc<tokio::sync::Notify>,
}

/// From when a [`Relay`] holds back what the daemon says on a socket.
#[derive(Clone, Copy)]
enum Hold {
    /// After the daemon's first message, its presence, which lets the command's node join the
    /// project's system: the node waits for the answers to its first queries, and is never ready.
    AfterPresence,
    /// From the first request of the command's node that names this entity, which a command whose
    /// node is ready makes once it comes to the entity.
    AfterNaming(EntityId),
}

impl Relay {
    async fn start(daemon: u16, hold: Hold) -> Result<Relay> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
        let port = listener.local_addr()?.port();
        let holding = std::sync::Arc::new(tokio::sync::Notify::new());
        let held = holding.clone();
        let listener = tokio::spawn(async move {
            while let Ok((inbound, _)) = listener.accept().await {
                tokio::spawn(relay(inbound, port, daemon, hold, held.clone()));
            }
        });
        Ok(Relay { port, listener, holding })
    }

    /// Stops listening, and returns the relay's port, which refuses connections from then on. The
    /// connections it relays last until the daemon closes them.
    async fn close(self) -> u16 {
        self.listener.abort();
        // Awaiting the aborted task waits until it has dropped the listener.
        let _ = self.listener.await;
        self.port
    }
}

/// Relays one connection to the daemon: the request's head with its `Host` naming the daemon's
/// port, the only one the daemon answers, then the rest both ways, except that on a socket, what
/// the daemon says from the moment `hold` names is read and held back. When the daemon closes the
/// connection, so does the relay.
async fn relay(
    inbound: tokio::net::TcpStream,
    port: u16,
    daemon: u16,
    hold: Hold,
    holding: std::sync::Arc<tokio::sync::Notify>,
) -> Result<()> {
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::io::AsyncWriteExt;
    let (mut from_command, mut to_command) = inbound.into_split();
    let (mut from_daemon, mut to_daemon) = tokio::net::TcpStream::connect(("127.0.0.1", daemon)).await?.into_split();
    let head = String::from_utf8(read_head(&mut from_command).await?)?;
    to_daemon.write_all(head.replace(&format!("127.0.0.1:{port}"), &format!("127.0.0.1:{daemon}")).as_bytes()).await?;
    // Set once the command's node has sent a request naming the entity `hold` names, before the
    // request goes on to the daemon, so that the daemon's answer to it is held back.
    let named = std::sync::Arc::new(AtomicBool::new(false));
    let naming = match hold {
        Hold::AfterNaming(entity) if head.to_ascii_lowercase().contains("upgrade: websocket") => Some(entity),
        _ => None,
    };
    let requests = tokio::spawn({
        let named = named.clone();
        async move {
            match naming {
                None => {
                    tokio::io::copy(&mut from_command, &mut to_daemon).await?;
                }
                // A request carries an entity's id as its 32 bytes.
                Some(entity) => loop {
                    let (frame, payload) = read_frame(&mut from_command).await?;
                    if payload.windows(EntityId::BYTE_LEN).any(|bytes| bytes == entity.to_bytes()) {
                        named.store(true, Ordering::SeqCst);
                    }
                    to_daemon.write_all(&frame).await?;
                },
            }
            anyhow::Ok(())
        }
    });
    let answer = read_head(&mut from_daemon).await?;
    to_command.write_all(&answer).await?;
    if !answer.starts_with(b"HTTP/1.1 101") {
        tokio::io::copy(&mut from_daemon, &mut to_command).await?;
    } else if let Hold::AfterPresence = hold {
        to_command.write_all(&read_frame(&mut from_daemon).await?.0).await?;
        let mut held = [0; 4096];
        while from_daemon.read(&mut held).await? > 0 {
            holding.notify_one();
        }
    } else {
        // Frame by frame, so that none is cut short when the relay starts holding back.
        while let Ok((frame, _)) = read_frame(&mut from_daemon).await {
            if named.load(Ordering::SeqCst) {
                holding.notify_one();
            } else {
                to_command.write_all(&frame).await?;
            }
        }
    }
    requests.abort();
    Ok(())
}

/// An HTTP head, up to and including the blank line that ends it, read a byte at a time so that
/// nothing after it is read.
async fn read_head(from: &mut tokio::net::tcp::OwnedReadHalf) -> Result<Vec<u8>> {
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        head.push(from.read_u8().await?);
    }
    Ok(head)
}

/// One WebSocket frame as its sender sent it, and its payload unmasked: two bytes, the extended
/// length when the second says there is one, the masking key when the frame is masked (a client
/// masks every frame, a server none), and the payload.
async fn read_frame(from: &mut tokio::net::tcp::OwnedReadHalf) -> Result<(Vec<u8>, Vec<u8>)> {
    let mut frame = vec![from.read_u8().await?, from.read_u8().await?];
    let len = match frame[1] & 0x7f {
        126 => {
            let len = from.read_u16().await?;
            frame.extend(len.to_be_bytes());
            u64::from(len)
        }
        127 => {
            let len = from.read_u64().await?;
            frame.extend(len.to_be_bytes());
            len
        }
        len => u64::from(len),
    };
    let mut key = [0; 4];
    if frame[1] & 0x80 != 0 {
        from.read_exact(&mut key).await?;
        frame.extend(key);
    }
    let start = frame.len();
    frame.resize(start + usize::try_from(len)?, 0);
    from.read_exact(&mut frame[start..]).await?;
    let payload = frame[start..].iter().zip(key.iter().cycle()).map(|(byte, mask)| byte ^ mask).collect();
    Ok((frame, payload))
}
