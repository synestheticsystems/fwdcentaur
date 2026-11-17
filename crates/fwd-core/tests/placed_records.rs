//! A record built on the Commons' models, which name no project, placed in one by fwd's
//! `Placement` component (`fwd_core::project_graph`, `placed`): one transaction makes an intention
//! with who wrote it down and where it is placed; its project's graph lists it and another
//! project's does not; a second node of the same SQLite store reads it; and one no project placed
//! is no record of fwd's.

use ankurah::{policy::DEFAULT_CONTEXT, property::Ref, Context, EntityId};
use ankurah_commons::cognition::{Intention, IntentionView};
use ankurah_commons::property::Timestamp;
use ankurah_commons::records::Created;
use ankurah_storage_sqlite::SqliteStorageEngine;
use anyhow::Result;
use fwd_core::durable;
use fwd_core::intentions::{self, Parts};
use fwd_core::model::{PlacementView, PrincipalKind};
use fwd_core::ops;
use fwd_core::project_graph::{Followed, ProjectGraph, ProjectGraphError, Whose};
use std::path::Path;

/// A context on a durable node over the SQLite file at `path`, made new when it does not exist.
async fn node_on(path: &Path) -> Result<Context> {
    let node = durable::open(SqliteStorageEngine::open(path).await?).await?;
    Ok(node.context_async(DEFAULT_CONTEXT).await?)
}

/// What `graph` lists of the intentions titled "Readers keep their place": each one's id, title and
/// who wrote it down.
async fn listed(graph: &ProjectGraph) -> Result<Vec<(EntityId, String, EntityId)>> {
    let listed: Vec<(PlacementView, Parts)> = graph.fetch_placed("title = 'Readers keep their place'").await?;
    listed.iter().map(|(_, (i, c))| Ok((i.id(), i.title()?, *c.by()?))).collect()
}

#[tokio::test]
async fn an_intention_made_in_one_transaction_is_listed_in_its_project_alone_and_read_by_a_second_node() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("fwd.sqlite");
    let (alpha_uuid, beta_uuid) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4());

    let (intention, person) = {
        let ctx = node_on(&path).await?;
        let alpha = ProjectGraph::ensure(ctx.clone(), alpha_uuid, "alpha").await?;
        let beta = ProjectGraph::ensure(ctx, beta_uuid, "beta").await?;
        let person = ops::ensure_principal(&alpha, "ines", "Ines", PrincipalKind::Human, None).await?;
        let agent = ops::ensure_principal(&alpha, "impl-tide", "impl-tide", PrincipalKind::Agent, None).await?;

        let trx = alpha.begin();
        let made = trx
            .create_placed(&Intention {
                title: "Readers keep their place".into(),
                description: "When I come back, the list is where I left it.".into(),
            })
            .await?;
        let record = ankurah::model::Mutable::entity(&*made);
        let written = Timestamp::from_unix_millis(1_700_000_000_000);
        trx.add(record, &Created { created: written, by: Ref::new(person) }).await?;
        let twice = trx.add(record, &Created { created: written, by: Ref::new(agent) }).await;
        assert!(matches!(twice, Err(ProjectGraphError::AlreadyCarried { .. })), "a component it carries: {twice:?}");
        let intention = made.id();
        trx.commit().await?;

        let title = "Readers keep their place".to_string();
        assert_eq!(listed(&alpha).await?, vec![(intention, title, person)], "alpha lists it");
        assert_eq!(listed(&beta).await?, vec![], "beta does not");
        (intention, person)
    };

    // The same file as a second node, which holds nothing in memory from the first.
    let ctx = node_on(&path).await?;
    let alpha = ProjectGraph::open(ctx.clone(), alpha_uuid).await?;
    let beta = ProjectGraph::open(ctx, beta_uuid).await?;
    let title = "Readers keep their place".to_string();
    assert_eq!(listed(&alpha).await?, vec![(intention, title.clone(), person)]);
    assert_eq!(listed(&beta).await?, vec![]);

    let (_, (i, c)): (PlacementView, Parts) = alpha.get_placed(intention).await?;
    assert_eq!(
        (i.description()?.as_str(), c.created()?.unix_millis()),
        ("When I come back, the list is where I left it.", 1_700_000_000_000)
    );
    let refused = beta.get_placed::<Parts>(intention).await.map(|_| ()).expect_err("alpha's record");
    assert!(
        matches!(refused, ProjectGraphError::InAnotherProject { record, .. } if record == intention),
        "{refused:?}"
    );
    match beta.follow_placed::<IntentionView>(intention).await? {
        Followed::Held { project: Whose::Other(p), found: (_, i), .. } => {
            assert_eq!((p.name.as_str(), i.title()?), ("alpha", title))
        }
        other => panic!("alpha's intention, followed from beta: {other:?}"),
    }
    Ok(())
}

/// An intention no project placed, as another application of the Commons' models may write one
/// into the node, is no record of fwd's: a reference to it is one that cannot be followed here,
/// not a read that fails, beside one that fwd placed, which is followed.
#[tokio::test]
async fn an_intention_no_project_placed_is_a_reference_that_cannot_be_followed() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let ctx = node_on(&dir.path().join("fwd.sqlite")).await?;
    let alpha = ProjectGraph::ensure(ctx.clone(), uuid::Uuid::new_v4(), "alpha").await?;
    let person = ops::ensure_principal(&alpha, "ines", "Ines", PrincipalKind::Human, None).await?;
    let placed = intentions::create(&alpha, person, "Readers keep their place", String::new(), None).await?.0.id();
    let trx = ctx.begin();
    let unplaced =
        trx.create(&Intention { title: "Builds stay under a minute".into(), description: String::new() }).await?.id();
    trx.commit().await?;
    assert!(matches!(ops::reference(&alpha, placed).await?, ops::Reference::Followed { .. }));
    assert_eq!(ops::reference(&alpha, unplaced).await?, ops::Reference::NotFollowed);
    Ok(())
}
