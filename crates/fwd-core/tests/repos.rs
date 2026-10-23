//! Repositories are identified by their upstream remote, never by a worktree or a directory
//! (Daniel, 2026-09-25): clones and worktrees of one upstream share one `Repo` entity.

use ankurah::{policy::DEFAULT_CONTEXT, Context, Node, PermissiveAgent};
use ankurah_storage_sqlite::SqliteStorageEngine;
use anyhow::Result;
use fwd_core::anchoring::git;
use fwd_core::model::RepoView;
use fwd_core::ops::{self, repo_key};
use std::path::Path;
use std::sync::Arc;

async fn context() -> Result<Context> {
    let storage = SqliteStorageEngine::open_in_memory().await?;
    let node = Node::new_durable(Arc::new(storage), PermissiveAgent::new());
    node.system.create().await?;
    Ok(node.context_async(DEFAULT_CONTEXT).await?)
}

fn init_repo(root: &Path, origin: Option<&str>) -> Result<()> {
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

#[tokio::test]
async fn clones_and_worktrees_of_one_upstream_share_one_repo() -> Result<()> {
    let ctx = context().await?;
    let dir = tempfile::tempdir()?;
    let main = dir.path().join("simulator");
    init_repo(&main, Some("git@github.com:Acme/simulator.git"))?;
    let worktree = dir.path().join("simulator-feat");
    git(&main, &["worktree", "add", "-q", worktree.to_str().unwrap(), "-b", "feat"])?;
    let clone = dir.path().join("elsewhere");
    init_repo(&clone, Some("https://github.com/acme/simulator"))?;

    let from_main = ops::ensure_repo(&ctx, &main).await?;
    let from_worktree = ops::ensure_repo(&ctx, &worktree).await?;
    let from_clone = ops::ensure_repo(&ctx, &clone).await?;
    assert_eq!(from_main, from_worktree, "a worktree is the same repository");
    assert_eq!(from_main, from_clone, "another clone spelling the same upstream differently is the same repository");
    let repos: Vec<RepoView> = ctx.fetch("default_branch != ''").await?;
    assert_eq!(repos.len(), 1);
    assert_eq!(repos[0].name()?, "simulator", "named after the directory that registered it");

    // No remote, no identity: refused, never keyed by a directory name.
    let local = dir.path().join("local-only");
    init_repo(&local, None)?;
    let err = ops::ensure_repo(&ctx, &local).await.unwrap_err().to_string();
    assert!(err.contains("no origin remote"), "{err}");
    Ok(())
}

#[test]
fn repo_key_names_one_upstream_for_every_spelling() {
    for url in [
        "git@github.com:Org/X.git",
        "https://github.com/org/x.git",
        "https://github.com/org/x",
        "ssh://git@github.com/org/x.git",
        "ssh://git@github.com/org/x/",
        "git@github.com:/org/x.git",
        "https://github.com/org//x/",
    ] {
        assert_eq!(repo_key(url), "github.com/org/x", "{url}");
    }
    assert_ne!(repo_key("git@github.com:org/x.git"), repo_key("git@github.com:org/y.git"));
    // A port stays with its host, never as a path segment; a local path is kept as it is.
    assert_eq!(repo_key("ssh://git@gitlab.example.com:2222/team/app.git"), "gitlab.example.com:2222/team/app");
    assert_eq!(repo_key("https://Host:443/Org/X.git"), "host:443/org/x");
    assert_eq!(repo_key("/srv/git/Repo.git/"), "/srv/git/Repo");
}
