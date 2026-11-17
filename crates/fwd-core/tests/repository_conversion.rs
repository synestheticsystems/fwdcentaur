//! `fwd admin migrate`'s move of repositories from fwd's own model onto the Commons'
//! `git.Repository` (`fwd_core::conversion::repos::convert`), on a store the build before it wrote,
//! then opened as fwd opens one (`fwd_core::durable`): each repository keeps its id, its old
//! component, its name, its remote URLs and its default branch, gains a `Placement` in its project,
//! and is the one a checkout of its upstream registers; a second run changes nothing; and one that
//! would lose part of what it holds stops the run before anything is written.

use ankurah::model::Mutable;
use ankurah::policy::DEFAULT_CONTEXT;
use ankurah::property::Json;
use ankurah::EntityId;
use ankurah_commons::git::RepositoryView;
use ankurah_storage_sqlite::SqliteStorageEngine;
use anyhow::Result;
use fwd_core::anchoring::git;
use fwd_core::conversion::repos::{self, before_d261, Converted};
use fwd_core::durable;
use fwd_core::model::*;
use fwd_core::ops;
use fwd_core::project_graph::ProjectGraph;
use serde_json::json;
use std::path::PathBuf;
use tempfile::TempDir;

/// A store in a folder of its own, with its project alpha's id.
struct Store {
    _dir: TempDir,
    path: PathBuf,
    project: uuid::Uuid,
}

impl Store {
    /// A new store, and alpha's graph on it, for writing what the build before the move wrote.
    async fn new() -> Result<(Store, ProjectGraph)> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("fwd.sqlite");
        let node = durable::open(SqliteStorageEngine::open(&path).await?).await?;
        let project = uuid::Uuid::new_v4();
        let alpha = ProjectGraph::ensure(node.context_async(DEFAULT_CONTEXT).await?, project, "alpha").await?;
        Ok((Store { _dir: dir, path, project }, alpha))
    }

    /// The project's graph on the store opened again, as fwd opens one.
    async fn opened(&self) -> Result<ProjectGraph> {
        let node = durable::open(SqliteStorageEngine::open(&self.path).await?).await?;
        Ok(ProjectGraph::open(node.context_async(DEFAULT_CONTEXT).await?, self.project).await?)
    }
}

/// Writes a repository as the build before the move did; its id.
async fn written(
    graph: &ProjectGraph,
    name: &str,
    remote_urls: serde_json::Value,
    default_branch: &str,
) -> Result<EntityId> {
    let trx = graph.begin();
    let made = trx
        .create(&before_d261::Repo {
            project: graph.project(),
            name: name.into(),
            remote_urls: Json::new(remote_urls),
            default_branch: default_branch.into(),
        })
        .await?
        .id();
    trx.commit().await?;
    Ok(made)
}

/// A checkout at `root` whose origin is `origin`, with one commit.
fn checkout(root: &std::path::Path, origin: &str) -> Result<()> {
    std::fs::create_dir_all(root)?;
    git(root, &["init", "-q", "-b", "main"])?;
    git(root, &["config", "user.email", "t@example.com"])?;
    git(root, &["config", "user.name", "t"])?;
    git(root, &["remote", "add", "origin", origin])?;
    std::fs::write(root.join("f.txt"), "x\n")?;
    git(root, &["add", "f.txt"])?;
    git(root, &["commit", "-q", "-m", "init"])?;
    Ok(())
}

/// The project's repositories, each read as every reader reads one, by its id.
async fn repositories(graph: &ProjectGraph) -> Result<Vec<(EntityId, String, serde_json::Value, String)>> {
    let mut found = Vec::new();
    for (placement, r) in graph.all_placed::<RepositoryView>().await? {
        found.push((placement.id(), r.name()?, serde_json::Value::from(r.remote_urls()?), r.default_branch()?));
    }
    found.sort_by_key(|(id, ..)| *id);
    Ok(found)
}

/// A repository registered with its upstream and its branch, and one with neither, as the build
/// before the move wrote them: each comes through with what it held, keeping its id and its old
/// component, placed in its project; a checkout of the first's upstream, spelled another way,
/// registers no second repository but finds the first, by the upstreams it answers to as before;
/// and after a second run, which moves nothing, and a reopen, each reads the same.
#[tokio::test]
async fn each_repository_moves_whole_and_a_checkout_of_its_upstream_still_finds_it() -> Result<()> {
    let (store, before) = Store::new().await?;
    let upstream = "git@example.com:acme/simulator.git";
    let registered = written(&before, "simulator", json!([upstream]), "main").await?;
    let bare = written(&before, "scratch", json!([]), "").await?;
    drop(before);
    let alpha = store.opened().await?;
    assert!(repositories(&alpha).await?.is_empty(), "no repository reads as one before the move");

    let mut converted = repos::convert(&alpha).await?;
    converted.sort_by_key(|c| c.repository);
    let mut expected = vec![
        Converted { repository: registered, name: "simulator".into() },
        Converted { repository: bare, name: "scratch".into() },
    ];
    expected.sort_by_key(|c| c.repository);
    assert_eq!(converted, expected);
    let mut held = vec![
        (registered, "simulator".to_string(), json!([upstream]), "main".to_string()),
        (bare, "scratch".to_string(), json!([]), String::new()),
    ];
    held.sort_by_key(|(id, ..)| *id);
    assert_eq!(repositories(&alpha).await?, held);
    for id in [registered, bare] {
        let old: before_d261::RepoView = alpha.get(id).await?;
        assert_eq!(old.project()?, alpha.project(), "the old component stays");
    }
    let (_, moved): (PlacementView, RepositoryView) = alpha.get_placed(registered).await?;
    assert_eq!(ops::repo_upstreams(&moved)?, vec![ops::repo_key(upstream)]);

    let dir = tempfile::tempdir()?;
    let root = dir.path().join("simulator");
    checkout(&root, "https://example.com/acme/simulator")?;
    assert_eq!(ops::ensure_repo(&alpha, &root).await?, registered, "the repository the upstream was registered as");

    assert_eq!(repos::convert(&alpha).await?, vec![], "a second run");
    drop(alpha);
    let reopened = store.opened().await?;
    assert_eq!(repositories(&reopened).await?, held, "and after a reopen");
    Ok(())
}

/// A repository whose entity carries a `Placement` already would have its project written over,
/// so the run stops naming it, and the other, which would move whole, does not move either.
#[tokio::test]
async fn a_repository_placed_already_stops_the_run_before_anything_is_written() -> Result<()> {
    let (store, before) = Store::new().await?;
    let whole = written(&before, "simulator", json!(["git@example.com:acme/simulator.git"]), "main").await?;
    let placed = written(&before, "scratch", json!(["git@example.com:acme/scratch.git"]), "main").await?;
    let old: before_d261::RepoView = before.get(placed).await?;
    let trx = before.begin();
    let edited = trx.edit(&old)?;
    trx.add(Mutable::entity(&*edited), &Placement { project: before.project() }).await?;
    trx.commit().await?;
    drop(before);
    let alpha = store.opened().await?;

    let refused = format!("{:#}", repos::convert(&alpha).await.expect_err("refused"));
    assert!(refused.contains(&format!("{} (it carries `placement` already", placed.to_base64())), "{refused}");
    assert!(!refused.contains(&whole.to_base64()), "{refused}");
    let old: before_d261::RepoView = alpha.get(whole).await?;
    assert!(alpha.component_of::<RepositoryView>(&old).await?.is_none(), "nothing written");
    assert!(alpha.component_of::<PlacementView>(&old).await?.is_none());
    Ok(())
}
