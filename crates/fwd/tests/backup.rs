//! `fwd admin backup` and `fwd admin restore` on the real binary: a backup taken while the daemon
//! runs and writes restores, into a new folder, every project with its graph as it was at one
//! moment; `--keep` removes the oldest backups of this data folder and nothing else in the folder;
//! and a restore is refused into a folder that is not empty and into the data folder a daemon
//! serves, which it leaves as it was.

mod common;

use anyhow::Result;
use common::Scratch;
use fwd_core::daemon::{self, Autostart};
use fwd_core::model::*;
use fwd_core::ops::{self, CreateTask};
use fwd_core::project::{self, Paths, Project};
use serde_json::Value;
use std::cell::Cell;
use std::path::PathBuf;
use std::process::Output;
use std::time::Duration;

/// A data folder in the test's scratch folder, which stops its daemon before the folder goes, with
/// the projects named, each with a folder of its own to be its checkout.
fn setup(root: &Scratch, data: &str, names: &[&str]) -> Result<(Paths, Vec<Project>)> {
    let paths = root.paths(&format!("{data}-run"), data);
    let mut projects = Vec::new();
    for name in names {
        let checkout = root.path().join(format!("{data}-{name}"));
        std::fs::create_dir_all(&checkout)?;
        let options = project::InitOptions { name: Some(name), ..Default::default() };
        projects.push(project::init(&paths, &checkout, options)?.0.project);
    }
    Ok((paths, projects))
}

fn spawn() -> Autostart {
    Autostart::Spawn { daemon_bin: PathBuf::from(env!("CARGO_BIN_EXE_fwd")) }
}

/// `fwd` with `args` and JSON output, run against `paths` the way a person's shell with
/// `FWD_DATA_DIR` and `FWD_RUNTIME_DIR` set would run it.
fn fwd(paths: &Paths, args: &[&str]) -> std::process::Command {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_fwd"));
    command
        .args(args)
        .args(["--format", "json"])
        .env("FWD_RUNTIME_DIR", &paths.runtime_dir)
        .env("FWD_DATA_DIR", &paths.data_dir)
        .env_remove("FWD_PROJECT")
        .env_remove("FWD_AS")
        .env_remove("FWD_SCOPE")
        .env_remove("FWD_DAEMON_BIN");
    command
}

/// The JSON a command that succeeded printed.
fn printed(output: &Output) -> Result<Value> {
    assert!(output.status.success(), "fwd failed: {}", String::from_utf8_lossy(&output.stderr));
    Ok(serde_json::from_slice(&output.stdout)?)
}

/// What a command that failed said on its standard error.
fn refused(output: &Output) -> String {
    assert!(!output.status.success(), "fwd succeeded: {}", String::from_utf8_lossy(&output.stdout));
    String::from_utf8_lossy(&output.stderr).into_owned()
}

async fn titles(client: &daemon::Client) -> Result<Vec<String>> {
    let tasks = client.ctx.fetch::<TaskView>("created > 0").await?;
    let mut titles = tasks.iter().map(|task| task.title()).collect::<Result<Vec<_>, _>>()?;
    titles.sort();
    Ok(titles)
}

async fn create(client: &daemon::Client, me: ankurah::EntityId, title: &str) -> Result<()> {
    ops::create_task(&client.ctx, me, CreateTask { title: title.into(), ..Default::default() }).await?;
    Ok(())
}

#[tokio::test]
async fn a_backup_taken_while_the_daemon_writes_restores_every_project_as_it_was_at_one_moment() -> Result<()> {
    let root = Scratch::new()?;
    let (paths, projects) = setup(&root, "data", &["widgets", "gadgets"])?;
    let (widgets, gadgets) = (&projects[0], &projects[1]);
    let writer = daemon::ensure_daemon(&paths, widgets, spawn()).await?;
    let me = ops::ensure_principal(&writer.ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    let other = daemon::ensure_daemon(&paths, gadgets, spawn()).await?;
    let you = ops::ensure_principal(&other.ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    for n in 0..3 {
        create(&other, you, &format!("gadget {n}")).await?;
    }

    // Tasks go into widgets one after another, each committed before the next begins, until the
    // backup, started once three are in, has finished.
    let written = Cell::new(0usize);
    let done = Cell::new(false);
    let writing = async {
        while !done.get() {
            create(&writer, me, &format!("widget {:03}", written.get())).await?;
            written.set(written.get() + 1);
        }
        anyhow::Ok(())
    };
    let backing_up = async {
        while written.get() < 3 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let before = written.get();
        let mut command = tokio::process::Command::from(fwd(&paths, &["admin", "backup"]));
        let output = command.output().await;
        done.set(true);
        anyhow::Ok((before, output?))
    };
    let (writing, backing_up) = tokio::join!(writing, backing_up);
    writing?;
    let (before, output) = backing_up?;
    // Counted once the writer has stopped: a task committed while the backup read, but not yet
    // counted when it finished, is counted by now.
    let after = written.get();
    let taken = printed(&output)?;
    let backup = PathBuf::from(taken["backup"].as_str().expect("the backup's folder"));
    assert_eq!(backup.parent(), Some(root.path().join("data-backups").canonicalize()?.as_path()), "{taken}");
    let names: Vec<&str> =
        taken["projects"].as_array().expect("projects").iter().filter_map(|p| p["name"].as_str()).collect();
    assert_eq!(names, ["widgets", "gadgets"]);
    let databases = taken["files"].as_array().expect("files").iter().filter(|f| f["database"] == true).count();
    assert_eq!(databases, 2, "each project's database is copied through SQLite: {taken}");
    drop(writer);
    drop(other);

    let into = root.path().join("restored");
    let restored = printed(
        &fwd(&paths, &["admin", "restore", backup.to_str().expect("utf-8"), "--into", into.to_str().expect("utf-8")])
            .output()?,
    )?;
    assert_eq!(restored["into"], into.canonicalize()?.display().to_string());

    // A daemon of its own opens the restored folder; the one serving the original runs on.
    let there = root.paths("restored-run", "restored");
    let widgets_there = daemon::ensure_daemon(&there, widgets, spawn()).await?;
    let gadgets_there = daemon::ensure_daemon(&there, gadgets, spawn()).await?;
    let hosted = daemon::status(&there).await?.expect("the restored folder's daemon answers").projects;
    assert!(hosted.iter().all(|p| p.error.is_none()) && hosted.len() == 2, "{hosted:?}");
    assert_eq!(titles(&gadgets_there).await?, ["gadget 0", "gadget 1", "gadget 2"]);
    // One moment of the graph: the tasks written before the backup began, and those written while
    // it ran up to the moment it read, in order, with none missing between them.
    let found = titles(&widgets_there).await?;
    assert!((before..=after).contains(&found.len()), "{before} to {after} tasks, found {found:?}");
    let expected: Vec<String> = (0..found.len()).map(|n| format!("widget {n:03}")).collect();
    assert_eq!(found, expected);

    drop(widgets_there);
    drop(gadgets_there);
    assert!(daemon::stop(&there).await?);
    assert!(daemon::stop(&paths).await?);
    Ok(())
}

#[tokio::test]
async fn keep_removes_the_oldest_backups_of_this_data_folder_and_nothing_else() -> Result<()> {
    let root = Scratch::new()?;
    let (paths, _) = setup(&root, "data", &["widgets"])?;
    let (elsewhere, _) = setup(&root, "elsewhere", &["gadgets"])?;
    let backups = root.path().join("synced");
    let to = backups.to_str().expect("utf-8");
    // A folder of the person's, and a backup of another data folder, beside this one's.
    std::fs::create_dir_all(backups.join("notes"))?;
    std::fs::write(backups.join("notes").join("todo.txt"), "keep me")?;
    let other = printed(&fwd(&elsewhere, &["admin", "backup", "--to", to, "--keep", "1"]).output()?)?;

    let mut taken = Vec::new();
    for _ in 0..3 {
        taken.push(printed(&fwd(&paths, &["admin", "backup", "--to", to, "--keep", "2"]).output()?)?);
    }
    let folder = |v: &Value| PathBuf::from(v["backup"].as_str().expect("the backup's folder"));
    let paths_of = |v: &Value, key: &str| -> Vec<PathBuf> {
        v[key].as_array().expect("a list").iter().map(|p| PathBuf::from(p.as_str().expect("a path"))).collect()
    };
    assert_eq!(paths_of(&taken[1], "removed"), Vec::<PathBuf>::new(), "two backups fit");
    assert_eq!(paths_of(&taken[2], "kept"), [folder(&taken[2]), folder(&taken[1])], "newest first");
    assert_eq!(paths_of(&taken[2], "removed"), [folder(&taken[0])]);
    assert!(!folder(&taken[0]).exists());
    assert!(folder(&taken[1]).exists() && folder(&taken[2]).exists());
    assert!(folder(&other).exists(), "another data folder's backup is not this one's to remove");
    assert_eq!(std::fs::read_to_string(backups.join("notes").join("todo.txt"))?, "keep me");
    let mut left: Vec<String> = std::fs::read_dir(&backups)?
        .map(|e| Ok(e?.file_name().to_string_lossy().into_owned()))
        .collect::<Result<_>>()?;
    left.sort();
    assert_eq!(left.len(), 4, "two backups of this data folder, the other one and notes: {left:?}");
    Ok(())
}

#[tokio::test]
async fn a_restore_is_refused_into_a_folder_that_is_not_empty_and_into_the_data_folder_a_daemon_serves() -> Result<()> {
    let root = Scratch::new()?;
    let (paths, projects) = setup(&root, "data", &["widgets"])?;
    let client = daemon::ensure_daemon(&paths, &projects[0], spawn()).await?;
    let me = ops::ensure_principal(&client.ctx, "dnorman", "Daniel", PrincipalKind::Human, None).await?;
    create(&client, me, "stays").await?;
    let taken = printed(&fwd(&paths, &["admin", "backup"]).output()?)?;
    let backup = taken["backup"].as_str().expect("the backup's folder").to_string();

    let occupied = root.path().join("occupied");
    std::fs::create_dir_all(&occupied)?;
    std::fs::write(occupied.join("notes.txt"), "mine")?;
    let said =
        refused(&fwd(&paths, &["admin", "restore", &backup, "--into", occupied.to_str().expect("utf-8")]).output()?);
    assert!(said.contains("is not empty"), "{said}");
    assert_eq!(std::fs::read_dir(&occupied)?.count(), 1, "the folder is as it was");

    // Without --into the restore goes to this environment's data folder, which the daemon serves.
    let pid = client.pf.pid;
    let said = refused(&fwd(&paths, &["admin", "restore", &backup]).output()?);
    assert!(said.contains(&format!("a daemon (pid {pid}) is serving")) && said.contains("fwd daemon stop"), "{said}");
    drop(client);
    assert!(daemon::stop(&paths).await?);
    let said = refused(&fwd(&paths, &["admin", "restore", &backup]).output()?);
    assert!(said.contains("is not empty"), "a stopped daemon's folder still holds its graph: {said}");

    let backups = root.path().join("data-backups");
    let said = refused(&fwd(&paths, &["admin", "restore", backups.to_str().expect("utf-8")]).output()?);
    assert!(said.contains("holds backups rather than being one") && said.contains(&backup), "{said}");

    let client = daemon::ensure_daemon(&paths, &projects[0], spawn()).await?;
    assert_eq!(titles(&client).await?, ["stays"], "the data folder is as it was");
    drop(client);
    assert!(daemon::stop(&paths).await?);
    Ok(())
}
