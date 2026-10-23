//! What the suites that start daemons from the test process share (lifecycle.rs, backup.rs,
//! routing.rs): the scratch folder a test keeps its daemons' runtime and data folders in, which
//! readies the process before its first daemon and stops each daemon before the folder is deleted,
//! so that a suite holds whether it runs alone, first or after others, and a failing test leaves
//! no daemon behind. The suites that start their daemons through the CLI (anchors.rs, plugin.rs,
//! task_show.rs) need none of it: there the command that spawns the daemon has run already, as a
//! person's does, and their own scratch folders stop the daemon.

use fwd_core::project::Paths;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// A test's temporary folder, which holds the runtime and data folders of the daemons the test
/// starts and the projects' checkouts, and which stops each of those daemons before the folder is
/// deleted, whether the test passed or failed: a test that fails before its own stop would
/// otherwise leave its daemon running with its folder gone.
pub struct Scratch {
    dir: tempfile::TempDir,
    daemons: RefCell<Vec<Paths>>,
}

impl Scratch {
    /// A new folder, outside any git checkout (`outside_any_checkout`), in a process readied for
    /// the daemons its tests start (`before_the_first_daemon`).
    pub fn new() -> std::io::Result<Scratch> {
        outside_any_checkout()?;
        before_the_first_daemon();
        Ok(Scratch { dir: tempfile::tempdir()?, daemons: RefCell::default() })
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// The runtime folder `runtime` and the data folder `data` in this folder, for a daemon that is
    /// stopped, when one runs there, before the folder goes.
    pub fn paths(&self, runtime: &str, data: &str) -> Paths {
        let paths = Paths { runtime_dir: self.path().join(runtime), data_dir: self.path().join(data) };
        self.daemons.borrow_mut().push(paths.clone());
        paths
    }
}

impl Drop for Scratch {
    /// `fwd daemon stop` against each daemon's folders, as the suites that run the CLI stop theirs
    /// (anchors.rs, plugin.rs), since a drop cannot await `daemon::stop`. It finds no daemon where
    /// the test stopped its own.
    fn drop(&mut self) {
        for paths in self.daemons.get_mut().iter() {
            let _ = Command::new(env!("CARGO_BIN_EXE_fwd"))
                .args(["daemon", "stop"])
                .env("FWD_RUNTIME_DIR", &paths.runtime_dir)
                .env("FWD_DATA_DIR", &paths.data_dir)
                .output();
        }
    }
}

/// Refuses to make scratch folders in a temporary directory inside a git checkout, naming the
/// checkout. These tests make their projects in plain scratch folders, each meant to be its
/// project's root, but `fwd init` asks git for the checkout holding the folder it is given and
/// writes the project's marker at that checkout's root. With TMPDIR inside a checkout, as an agent
/// set it on 2026-10-01 to keep its tests' daemons in its worktree, each test wrote its project's
/// marker over the `.fwd/project.toml` committed at the worktree's root, and the next fwd command
/// run there acted in the project the last of them made.
fn outside_any_checkout() -> std::io::Result<()> {
    let temporary = std::env::temp_dir();
    match temporary.ancestors().find(|dir| dir.join(".git").exists()) {
        Some(checkout) => Err(std::io::Error::other(format!(
            "the temporary directory {} is inside the git checkout at {}, where fwd init would write each test project's marker over the checkout's own .fwd/project.toml: run the tests with TMPDIR outside any checkout",
            temporary.display(),
            checkout.display()
        ))),
        None => Ok(()),
    }
}

/// Readies this process for the daemons its tests start, once, before the first one starts.
///
/// The daemons inherit this process's environment, so `FWD_DEV_RUNNER_DIR` first names a scratch
/// registry (`registry`): without it they would show on the sutra dashboard of whoever runs the
/// tests.
///
/// Then the fwd binary runs once, and this waits for it for as long as it takes, so that macOS's
/// check of a new executable is over before any launcher's deadline starts. A launcher gives the
/// daemon it spawns ten seconds to answer (`fwd_core::daemon`), time for the daemon to open its
/// projects and listen, which took it under a second even with other builds loading the machine
/// (load averages up to 139, 2026-09-29). But macOS checks an executable it has never run before
/// it runs it, and new executables wait their turn: one alone took a second, sixteen started at
/// once took sixteen seconds to all run. Cargo writes `target/debug/fwd` anew at every
/// `cargo test`, so when a suite was the first to run it, its first daemons waited for that check
/// inside the ten seconds: with sixteen new executables starting alongside, two runs in three
/// failed every lifecycle test with "no daemon answered within 10s" (2026-09-29), as one test had
/// failed while eight agents built at once (2026-09-28); the backup and routing suites escaped
/// only when another suite had run the binary before them, and paid for the check when run alone
/// (`cargo test -p fwd --test backup`). The `fwd` a person runs has run, and been checked, before
/// it spawns the daemon from its own file; running the binary here first puts these tests in the
/// same position.
fn before_the_first_daemon() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        std::env::set_var("FWD_DEV_RUNNER_DIR", registry());
        let status = Command::new(env!("CARGO_BIN_EXE_fwd"))
            .arg("--version")
            .stdout(Stdio::null())
            .status()
            .expect("running the fwd binary");
        assert!(status.success(), "fwd --version exited with {status}");
    });
}

/// The sutra registry of every daemon these tests start.
pub fn registry() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("dev-runner")
}
