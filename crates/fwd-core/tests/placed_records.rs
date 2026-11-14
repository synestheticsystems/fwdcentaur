//! A record built on the Commons' models, which name no project, placed in one by fwd's
//! `Placement` component (`fwd_core::project_graph`, `placed`): one transaction makes an intention
//! with who wrote it down, whose words it holds and where it is placed; its project's graph lists
//! it and another project's does not; and a second node of the same SQLite store reads it, the
//! definitions the first node minted included.

use ankurah::{policy::DEFAULT_CONTEXT, property::Ref, Context, EntityId, Node, PermissiveAgent};
use ankurah_commons::intention_graph::{Intention, IntentionStatus, IntentionView};
use ankurah_commons::records::{Attribution, AttributionView, Creation, CreationView};
use ankurah_storage_sqlite::SqliteStorageEngine;
use anyhow::Result;
use fwd_core::model::{PlacementView, PrincipalKind};
use fwd_core::ops;
use fwd_core::project_graph::{Followed, ProjectGraph, ProjectGraphError, Whose};
use std::path::Path;
use std::sync::Arc;

/// An intention read with its record of creation and its speaker.
type Read = (IntentionView, CreationView, AttributionView);

/// A context on a durable node over the SQLite file at `path`, which `create` makes new.
async fn node_on(path: &Path, create: bool) -> Result<Context> {
    let node = Node::new_durable(Arc::new(SqliteStorageEngine::open(path).await?), PermissiveAgent::new());
    if create {
        node.system.create().await?;
    } else {
        node.system.wait_loaded().await?;
    }
    Ok(node.context_async(DEFAULT_CONTEXT).await?)
}

/// What a list of `graph`'s committed intentions holds: each one's id, title and speaker.
async fn committed(graph: &ProjectGraph) -> Result<Vec<(EntityId, String, EntityId)>> {
    let listed: Vec<(PlacementView, Read)> = graph.fetch_placed("status = 'committed'").await?;
    listed.iter().map(|(_, (i, _, a))| Ok((i.id(), i.title()?, *a.speaker()?))).collect()
}

#[tokio::test]
async fn an_intention_made_in_one_transaction_is_listed_in_its_project_alone_and_read_by_a_second_node() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("fwd.sqlite");
    let (alpha_uuid, beta_uuid) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4());

    let (intention, person) = {
        let ctx = node_on(&path, true).await?;
        let alpha = ProjectGraph::ensure(ctx.clone(), alpha_uuid, "alpha").await?;
        let beta = ProjectGraph::ensure(ctx, beta_uuid, "beta").await?;
        let person = ops::ensure_principal(&alpha, "ines", "Ines", PrincipalKind::Human, None).await?;
        let agent = ops::ensure_principal(&alpha, "impl-tide", "impl-tide", PrincipalKind::Agent, None).await?;

        let trx = alpha.begin();
        let made = trx
            .create_placed(&Intention {
                title: "Readers keep their place".into(),
                words: "When I come back, the list is where I left it.".into(),
                status: IntentionStatus::Committed,
            })
            .await?;
        let record = ankurah::model::Mutable::entity(&*made);
        trx.add(record, &Creation { created_by: Ref::new(agent), created: 1_700_000_000_000 }).await?;
        trx.add(record, &Attribution { speaker: Ref::new(person) }).await?;
        let twice = trx.add(record, &Attribution { speaker: Ref::new(agent) }).await;
        assert!(matches!(twice, Err(ProjectGraphError::AlreadyCarried { .. })), "a component it carries: {twice:?}");
        let intention = made.id();
        trx.commit().await?;

        let title = "Readers keep their place".to_string();
        assert_eq!(committed(&alpha).await?, vec![(intention, title, person)], "alpha lists it");
        assert_eq!(committed(&beta).await?, vec![], "beta does not");
        (intention, person)
    };

    // The same file as a second node, which holds nothing in memory from the first.
    let ctx = node_on(&path, false).await?;
    let alpha = ProjectGraph::open(ctx.clone(), alpha_uuid).await?;
    let beta = ProjectGraph::open(ctx, beta_uuid).await?;
    let title = "Readers keep their place".to_string();
    assert_eq!(committed(&alpha).await?, vec![(intention, title.clone(), person)]);
    assert_eq!(committed(&beta).await?, vec![]);

    let (_, (i, c, _)): (PlacementView, Read) = alpha.get_placed(intention).await?;
    assert_eq!(
        (i.words()?.as_str(), c.created()?),
        ("When I come back, the list is where I left it.", 1_700_000_000_000)
    );
    let refused = beta.get_placed::<Read>(intention).await.map(|_| ()).expect_err("alpha's record");
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
