//! Projects are found through a repository's upstream, not a folder (Daniel, 2026-09-25): every
//! clone and worktree of a member repository resolves to the same project, with or without the
//! committed marker, and `init` joins before it creates.

use anyhow::Result;
use fwd_core::anchoring::git;
use fwd_core::project::{self, InitOptions, Paths, Project};
use std::path::Path;

fn paths(dir: &Path) -> Paths {
    Paths { runtime_dir: dir.join("run"), data_dir: dir.join("data") }
}

fn repo(root: &Path, origin: Option<&str>) -> Result<()> {
    std::fs::create_dir_all(root)?;
    git(root, &["init", "-q", "-b", "main"])?;
    git(root, &["config", "user.email", "t@example.com"])?;
    git(root, &["config", "user.name", "t"])?;
    if let Some(url) = origin {
        git(root, &["remote", "add", "origin", url])?;
    }
    std::fs::write(root.join("f.txt"), "x\n")?;
    git(root, &["add", "f.txt"])?;
    git(root, &["commit", "-q", "-m", "init"])?;
    Ok(())
}

/// A scratch folder for a test that makes a project outside git, refused when the temporary
/// directory is inside a git checkout, naming the checkout: `project::init` writes the marker at
/// the root of the checkout holding the folder it is given, so with TMPDIR inside a checkout, as an
/// agent set it on 2026-10-01 in its worktree, the plain folder's marker went over the
/// `.fwd/project.toml` committed at the worktree's root.
fn outside_git() -> Result<tempfile::TempDir> {
    let temporary = std::env::temp_dir();
    if let Some(checkout) = temporary.ancestors().find(|dir| dir.join(".git").exists()) {
        anyhow::bail!(
            "the temporary directory {} is inside the git checkout at {}, where project::init would write this test's marker over the checkout's own .fwd/project.toml: run the tests with TMPDIR outside any checkout",
            temporary.display(),
            checkout.display()
        );
    }
    Ok(tempfile::tempdir()?)
}

#[test]
fn every_checkout_of_an_upstream_resolves_to_one_project() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let paths = paths(dir.path());
    let main = dir.path().join("sim");
    repo(&main, Some("git@github.com:Acme/simulator.git"))?;
    let (first, created) = project::init(&paths, &main, InitOptions { name: Some("simulator"), ..Default::default() })?;
    assert!(created);
    assert!(main.join(project::MARKER).is_file(), "init writes the marker");

    // A worktree on a branch that never saw the marker (it is uncommitted in `main`).
    let worktree = dir.path().join("sim-feat");
    git(&main, &["worktree", "add", "-q", worktree.to_str().unwrap(), "-b", "feat"])?;
    assert!(!worktree.join(project::MARKER).exists());
    let via_worktree = project::resolve(&paths, &worktree, None)?;
    assert_eq!(via_worktree.project, first.project, "the upstream, not the marker, names the project");
    assert_eq!(via_worktree.checkout.as_ref().and_then(|c| c.branch.as_deref()), Some("feat"));
    assert_eq!(via_worktree.root, worktree.canonicalize()?);

    // A second clone spelling the same upstream differently, with no marker at all.
    let clone = dir.path().join("elsewhere");
    repo(&clone, Some("https://github.com/acme/simulator"))?;
    assert_eq!(project::resolve(&paths, &clone, None)?.project, first.project);

    // `init` there joins rather than minting a second project.
    let (joined, created) = project::init(&paths, &clone, InitOptions::default())?;
    assert!(!created);
    assert_eq!(joined.project, first.project);

    // `--new` is the only way to a second project for the same upstream; the clone's marker then
    // names it, and a worktree without a marker becomes ambiguous until `--project` says which.
    let (second, created) =
        project::init(&paths, &clone, InitOptions { name: Some("second"), new: true, ..Default::default() })?;
    assert!(created);
    assert_ne!(second.project, first.project);
    assert_eq!(project::resolve(&paths, &clone, None)?.project, second.project);
    assert!(project::resolve(&paths, &worktree, None).is_err(), "two projects claim the upstream");
    assert_eq!(project::resolve(&paths, &worktree, Some("simulator"))?.project, first.project);
    assert_eq!(project::resolve(&paths, &worktree, Some(&first.project.id.to_string()))?.project, first.project);
    // `--project` also takes a directory.
    assert_eq!(project::resolve(&paths, dir.path(), Some(main.to_str().unwrap()))?.project, first.project);

    let index = project::Index::load(&paths)?;
    assert_eq!(index.projects.len(), 2);
    assert_eq!(
        index.get("simulator")?.map(|e| e.upstreams.clone()),
        Some(vec!["github.com/acme/simulator".to_string()])
    );

    // A selector names the project only; the checkout is always the caller's.
    let from_clone = project::resolve(&paths, &clone, Some(main.to_str().unwrap()))?;
    assert_eq!(from_clone.project, first.project);
    assert_eq!(from_clone.checkout.as_ref().map(|c| c.root.clone()), Some(clone.canonicalize()?));
    Ok(())
}

#[test]
fn a_rename_reaches_the_index_and_this_checkouts_marker_and_refuses_a_taken_name() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let paths = paths(dir.path());
    let (main, worktree, other) = (dir.path().join("main"), dir.path().join("wt"), dir.path().join("other"));
    repo(&main, Some("git@github.com:org/sim.git"))?;
    repo(&other, Some("git@github.com:org/other.git"))?;
    let (sim, _) = project::init(&paths, &main, InitOptions { name: Some("simulator"), ..Default::default() })?;
    project::init(&paths, &other, InitOptions { name: Some("other"), ..Default::default() })?;
    git(&main, &["worktree", "add", "-q", "-b", "feat", worktree.to_str().unwrap()])?;

    let marker = project::rename(&paths, &sim.project, &sim.root, "fleet simulator")?;
    assert_eq!(marker, Some(sim.root.join(project::MARKER)), "the checkout's marker is rewritten");
    assert_eq!(project::resolve(&paths, &main, None)?.project.name, "fleet simulator", "through the marker");
    assert_eq!(project::resolve(&paths, &worktree, None)?.project.name, "fleet simulator", "through the index");
    assert_eq!(project::resolve(&paths, &worktree, Some("fleet simulator"))?.project.id, sim.project.id);
    assert!(project::resolve(&paths, &worktree, Some("simulator")).is_err(), "the old name selects nothing");

    assert!(project::rename(&paths, &sim.project, &sim.root, "other").is_err(), "another project has that name");
    assert!(project::rename(&paths, &sim.project, &sim.root, " ").is_err(), "a blank name is refused");
    assert_eq!(project::resolve(&paths, &main, None)?.project.name, "fleet simulator");
    // A directory whose marker names no project, or another one, is left alone.
    assert_eq!(project::rename(&paths, &sim.project, &other, "simulator")?, None);
    assert_eq!(project::marker_project(&other)?.map(|p| p.name), Some("other".to_string()));
    Ok(())
}

#[test]
fn a_project_name_wins_over_a_folder_of_the_same_name() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let paths = paths(dir.path());
    let app = dir.path().join("app");
    repo(&app, Some("git@github.com:org/app.git"))?;
    std::fs::create_dir_all(app.join("docs"))?;
    let elsewhere = dir.path().join("docs-repo");
    repo(&elsewhere, Some("git@github.com:org/docs.git"))?;
    let (docs, _) = project::init(&paths, &elsewhere, InitOptions { name: Some("docs"), ..Default::default() })?;
    let (app_project, _) = project::init(&paths, &app, InitOptions { name: Some("app"), ..Default::default() })?;
    // From inside `app`, whose root holds a `docs` folder, `--project docs` still means the project.
    assert_eq!(project::resolve(&paths, &app, Some("docs"))?.project, docs.project);
    // A selector with a separator is a directory.
    assert_eq!(project::resolve(&paths, &elsewhere, Some(app.to_str().unwrap()))?.project, app_project.project);
    Ok(())
}

#[test]
fn a_repository_nested_inside_another_projects_checkout_keeps_its_own_project() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let paths = paths(dir.path());
    let outer = dir.path().join("outer");
    repo(&outer, Some("git@github.com:org/outer.git"))?;
    let (outer_project, _) = project::init(&paths, &outer, InitOptions { name: Some("outer"), ..Default::default() })?;
    let lib_main = dir.path().join("lib");
    repo(&lib_main, Some("git@github.com:org/lib.git"))?;
    let (lib_project, _) = project::init(&paths, &lib_main, InitOptions { name: Some("lib"), ..Default::default() })?;
    // A clone of lib inside outer's checkout, with no marker of its own.
    let nested = outer.join("vendor").join("lib");
    repo(&nested, Some("git@github.com:org/lib.git"))?;
    let found = project::resolve(&paths, &nested, None)?;
    assert_eq!(
        found.project, lib_project.project,
        "the nested repository's upstream, not the marker above it, names its project"
    );
    let index = project::Index::load(&paths)?;
    assert_eq!(index.get("outer")?.map(|e| e.upstreams.clone()), Some(vec!["github.com/org/outer".to_string()]));
    assert_eq!(project::resolve(&paths, &outer, None)?.project, outer_project.project);
    Ok(())
}

#[test]
fn a_worktree_whose_main_checkout_is_gone_is_an_error_not_a_fresh_directory() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let paths = paths(dir.path());
    let main = dir.path().join("app");
    repo(&main, Some("git@github.com:org/app.git"))?;
    project::init(&paths, &main, InitOptions { name: Some("app"), ..Default::default() })?;
    let worktree = dir.path().join("app-wt");
    git(&main, &["worktree", "add", "-q", worktree.to_str().unwrap(), "-b", "feat"])?;
    std::fs::remove_dir_all(&main)?;
    let err = project::checkout_at(&worktree).unwrap_err().to_string();
    assert!(err.contains("worktree prune"), "{err}");
    assert!(project::resolve(&paths, &worktree, None).is_err());
    assert!(
        project::init(&paths, &worktree, InitOptions::default()).is_err(),
        "init must not mint a project for a broken checkout"
    );
    assert_eq!(project::Index::load(&paths)?.projects.len(), 1);
    Ok(())
}

/// Every worktree of a clone reads one object database, so `git_common_dir` gives them one
/// directory, and resolving an anchor asks git whether they hold a blob once for all of them. A
/// second clone has its own, and a directory that is no checkout has none. A worktree whose `.git`
/// file names its git directory by a relative path (`worktree.useRelativePaths`, git 2.48 and
/// later; older versions write an absolute one) shares it all the same.
#[test]
fn the_worktrees_of_a_clone_share_one_git_directory_and_another_clone_has_its_own() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let main = dir.path().join("app");
    repo(&main, Some("git@github.com:org/app.git"))?;
    let worktree = dir.path().join("app-wt");
    git(&main, &["worktree", "add", "-q", worktree.to_str().unwrap(), "-b", "feat"])?;
    let relative = dir.path().join("app-rel");
    git(
        &main,
        &["-c", "worktree.useRelativePaths=true", "worktree", "add", "-q", relative.to_str().unwrap(), "-b", "rel"],
    )?;
    let clone = dir.path().join("app-clone");
    git(dir.path(), &["clone", "-q", "--no-local", main.to_str().unwrap(), clone.to_str().unwrap()])?;

    let shared = project::git_common_dir(&main).expect("a clone has a git directory");
    assert_eq!(shared, main.join(".git").canonicalize()?);
    assert_eq!(project::git_common_dir(&worktree), Some(shared.clone()), "a worktree shares its clone's");
    assert_eq!(project::git_common_dir(&relative), Some(shared.clone()), "so does one named by a relative path");
    let own = project::git_common_dir(&clone).expect("the second clone has one");
    assert_ne!(own, shared, "a second clone has its own");
    assert_eq!(project::git_common_dir(dir.path()), None, "a directory that is no checkout has none");
    Ok(())
}

#[test]
fn a_new_project_may_not_take_a_name_this_machine_knows() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let paths = paths(dir.path());
    let one = dir.path().join("one").join("service");
    repo(&one, Some("git@github.com:org/service.git"))?;
    project::init(&paths, &one, InitOptions::default())?;
    let two = dir.path().join("two").join("service");
    repo(&two, Some("git@github.com:org/service.git"))?;
    let err = project::init(&paths, &two, InitOptions { new: true, ..Default::default() }).unwrap_err().to_string();
    assert!(err.contains("already exists"), "{err}");
    let (second, created) =
        project::init(&paths, &two, InitOptions { name: Some("service-2"), new: true, ..Default::default() })?;
    assert!(created);
    // Both projects claim the upstream; the ambiguity is reported, and `--project` resolves it.
    assert!(project::resolve(&paths, &two, None).is_ok(), "the clone's own marker names service-2");
    assert_eq!(project::resolve(&paths, &two, Some(&second.project.id.to_string()))?.project, second.project);
    assert!(project::resolve(&paths, &one, Some("service"))?.project.name == "service");
    Ok(())
}

#[test]
fn a_marker_alone_names_a_project_outside_git_and_the_old_marker_name_still_reads() -> Result<()> {
    let dir = outside_git()?;
    let paths = paths(dir.path());
    let plain = dir.path().join("plain");
    std::fs::create_dir_all(&plain)?;
    let (r, created) = project::init(&paths, &plain, InitOptions { name: Some("plain"), ..Default::default() })?;
    assert!(created);
    assert!(r.checkout.is_none());
    let nested = plain.join("sub").join("dir");
    std::fs::create_dir_all(&nested)?;
    assert_eq!(project::resolve(&paths, &nested, None)?.project, r.project, "found by walking up to the marker");

    let legacy = dir.path().join("legacy");
    std::fs::create_dir_all(legacy.join(".fwd"))?;
    let id = uuid::Uuid::new_v4();
    std::fs::write(legacy.join(".fwd/workspace.toml"), format!("id = \"{id}\"\nname = \"old\"\n"))?;
    let found = project::resolve(&paths, &legacy, None)?;
    assert_eq!(found.project, Project { id, name: "old".into() });
    assert!(project::Index::load(&paths)?.get("old")?.is_some(), "a marker teaches the index");
    Ok(())
}

/// A command that only reads still records in this machine's index what a checkout's marker
/// teaches it, and one that cannot stops with a sentence naming projects.toml, its directory and
/// what it was doing, the error from the write kept beneath it (docs/DECISIONS.md, D86, "A
/// read-only command that cannot write the project index stops with an error that names
/// projects.toml", 2026-09-28). Here the index's lock file may not be written; a read-only data
/// directory would not do, since the write first gives the directory back to its owner.
#[cfg(unix)]
#[test]
fn a_resolution_that_cannot_write_the_index_names_projects_toml_and_why() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    // SAFETY: geteuid has no preconditions and cannot fail. Permissions do not bind root.
    if unsafe { libc::geteuid() } == 0 {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let app = dir.path().join("app");
    repo(&app, Some("git@github.com:org/app.git"))?;
    let (joined, _) = project::init(&paths(dir.path()), &app, InitOptions { name: Some("app"), ..Default::default() })?;
    // Another data directory, whose index has never seen the project the marker names.
    let fresh = Paths { runtime_dir: dir.path().join("run-fresh"), data_dir: dir.path().join("data-fresh") };
    std::fs::create_dir_all(&fresh.data_dir)?;
    let lock = fresh.data_dir.join("projects.toml.lock");
    std::fs::write(&lock, "")?;
    std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o444))?;

    let err = project::resolve(&fresh, &app, None).unwrap_err();
    assert_eq!(
        err.to_string(),
        format!(
            "could not write projects.toml in the data directory {}: this command was joining the checkout's upstream github.com/org/app to the project \"app\", which the checkout's marker names and this machine's index did not yet list for that upstream; let a command write that directory once (a sandbox may forbid it), and from then on commands here only read the index",
            fresh.data_dir.display()
        )
    );
    let causes: Vec<String> = err.chain().skip(1).map(ToString::to_string).collect();
    assert_eq!(causes[0], format!("opening {}", lock.display()), "{causes:?}");
    assert!(!fresh.data_dir.join("projects.toml").exists(), "nothing was written");

    std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o644))?;
    assert_eq!(project::resolve(&fresh, &app, None)?.project, joined.project, "once it may write, it does");
    Ok(())
}

#[test]
fn legacy_storage_moves_with_its_write_ahead_log_and_finishes_an_interrupted_move() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let paths = paths(dir.path());
    let project = Project { id: uuid::Uuid::new_v4(), name: "x".into() };
    let old = paths.data_dir.join("workspaces").join(project.id.to_string());
    std::fs::create_dir_all(&old)?;
    std::fs::write(old.join("workspace.sqlite"), b"db")?;
    std::fs::write(old.join("workspace.sqlite-wal"), b"wal")?;
    std::fs::write(old.join("workspace.sqlite-shm"), b"shm")?;
    std::fs::write(old.join("daemon.log"), b"old log")?;
    // The launcher already created the new directory for its log.
    let new = paths.storage_dir(&project);
    std::fs::create_dir_all(&new)?;
    std::fs::write(new.join("daemon.log"), b"new log")?;
    paths.migrate_legacy(&project)?;
    assert_eq!(std::fs::read(paths.db_path(&project))?, b"db");
    assert_eq!(std::fs::read(new.join("project.sqlite-wal"))?, b"wal", "the WAL travels with its database");
    assert_eq!(std::fs::read(new.join("project.sqlite-shm"))?, b"shm");
    assert_eq!(std::fs::read(new.join("daemon.log"))?, b"new log");
    assert_eq!(std::fs::read(new.join("daemon.log.legacy"))?, b"old log");
    assert!(!old.exists());
    paths.migrate_legacy(&project)?; // idempotent
    assert_eq!(std::fs::read(paths.db_path(&project))?, b"db");

    // An interrupted move: the directory moved but the database still has its old name.
    let project = Project { id: uuid::Uuid::new_v4(), name: "y".into() };
    let new = paths.storage_dir(&project);
    std::fs::create_dir_all(&new)?;
    std::fs::write(new.join("workspace.sqlite"), b"db")?;
    std::fs::write(new.join("workspace.sqlite-wal"), b"wal")?;
    paths.migrate_legacy(&project)?;
    assert_eq!(std::fs::read(paths.db_path(&project))?, b"db");
    assert_eq!(std::fs::read(new.join("project.sqlite-wal"))?, b"wal");
    assert!(!new.join("workspace.sqlite").exists());

    // A database on both sides is refused, never guessed about.
    let project = Project { id: uuid::Uuid::new_v4(), name: "z".into() };
    let old = paths.data_dir.join("workspaces").join(project.id.to_string());
    std::fs::create_dir_all(&old)?;
    std::fs::write(old.join("workspace.sqlite"), b"old")?;
    std::fs::create_dir_all(paths.storage_dir(&project))?;
    std::fs::write(paths.db_path(&project), b"new")?;
    assert!(paths.migrate_legacy(&project).is_err());
    assert_eq!(std::fs::read(paths.db_path(&project))?, b"new");
    assert_eq!(std::fs::read(old.join("workspace.sqlite"))?, b"old");
    Ok(())
}
