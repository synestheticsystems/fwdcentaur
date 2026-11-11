//! A project's record in the daemon's store (`fwd_core::project_graph`): the daemon makes one per
//! project as it first hosts it, and finds that one again at every later start, with the name the
//! index gives; a client finds it by the project's UUID, and a UUID the store holds no record of is
//! refused.

use ankurah::{policy::DEFAULT_CONTEXT, Context, Node, PermissiveAgent};
use ankurah_storage_sqlite::SqliteStorageEngine;
use anyhow::Result;
use fwd_core::model::ProjectView;
use fwd_core::project_graph::{ProjectGraph, ProjectGraphError};
use std::sync::Arc;

/// A context on a store in memory.
async fn store() -> Result<Context> {
    let storage = SqliteStorageEngine::open_in_memory().await?;
    let node = Node::new_durable(Arc::new(storage), PermissiveAgent::new());
    node.system.create().await?;
    Ok(node.context_async(DEFAULT_CONTEXT).await?)
}

/// Every project record the store holds, by name.
async fn named(ctx: &Context) -> Result<Vec<String>> {
    let mut names =
        ctx.fetch::<ProjectView>("uuid != ''").await?.iter().map(|p| p.name()).collect::<Result<Vec<_>, _>>()?;
    names.sort();
    Ok(names)
}

#[tokio::test]
async fn the_daemon_makes_one_record_per_project_and_finds_it_again_under_the_indexs_name() -> Result<()> {
    let ctx = store().await?;
    let uuid = uuid::Uuid::new_v4();
    let first = ProjectGraph::ensure(ctx.clone(), uuid, "fwdcentaur").await?;
    let again = ProjectGraph::ensure(ctx.clone(), uuid, "fwd centaur").await?;
    assert_eq!(first.project(), again.project(), "a later start finds the record the first made");
    assert_eq!(named(&ctx).await?, vec!["fwd centaur"], "one record, under the name the index has now");
    ProjectGraph::ensure(ctx.clone(), uuid::Uuid::new_v4(), "another").await?;
    assert_eq!(named(&ctx).await?, vec!["another", "fwd centaur"]);

    again.rename("fwd").await?;
    assert_eq!(named(&ctx).await?, vec!["another", "fwd"], "a rename while the daemon runs");
    Ok(())
}

#[tokio::test]
async fn a_client_finds_the_project_by_its_uuid_and_no_project_the_store_has_no_record_of() -> Result<()> {
    let ctx = store().await?;
    let uuid = uuid::Uuid::new_v4();
    let hosted = ProjectGraph::ensure(ctx.clone(), uuid, "fwdcentaur").await?;
    let opened = ProjectGraph::open(ctx.clone(), uuid).await?;
    assert_eq!(opened.project(), hosted.project());
    let unknown = uuid::Uuid::new_v4();
    let refused = ProjectGraph::open(ctx, unknown).await.map(|g| g.project()).expect_err("no record");
    assert!(matches!(refused, ProjectGraphError::NotHosted(id) if id == unknown), "{refused:?}");
    Ok(())
}
