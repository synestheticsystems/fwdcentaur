//! The daemon's port file survives macOS's temp cleaner, which removes a file from the per-user
//! temporary folder once it has gone three days without an access and leaves a file another
//! process holds a lock on, by taking an exclusive lock without waiting before it removes one: the
//! daemon holds a shared lock on its port file for as long as it serves, and none once it stops.

mod common;

use anyhow::Result;
use fwd_core::daemon::{self, Autostart};
use fwd_core::project;
use std::path::PathBuf;

#[tokio::test]
async fn the_port_file_refuses_an_exclusive_lock_while_its_daemon_serves() -> Result<()> {
    let dir = common::Scratch::new()?;
    let paths = dir.paths("run", "data");
    let root = dir.path().join("repo");
    std::fs::create_dir_all(&root)?;
    let ws =
        project::init(&paths, &root, project::InitOptions { name: Some("port file"), ..Default::default() })?.0.project;
    let spawn = Autostart::Spawn { daemon_bin: PathBuf::from(env!("CARGO_BIN_EXE_fwd")) };
    drop(daemon::ensure_daemon(&paths, &ws, spawn).await?);
    assert!(
        daemon::try_lock(&paths.port_file())?.is_none(),
        "the port file refuses an exclusive lock taken without waiting while its daemon serves"
    );

    assert!(daemon::stop(&paths).await?);
    let port_file = paths.port_file();
    assert!(
        !port_file.exists() || daemon::try_lock(&port_file)?.is_some(),
        "the stopped daemon left its port file locked"
    );
    Ok(())
}
