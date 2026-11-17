//! The steps of the operations that a test cannot reach through their callers.

use super::*;
use crate::conversion::principals::{self, before_d261};
use crate::durable;
use ankurah::policy::DEFAULT_CONTEXT;
use ankurah_storage_sqlite::SqliteStorageEngine;

/// The person a command looks for and misses, as `fwd admin migrate` finishes moving them between
/// that lookup and the guard, is the one moved, found by the second lookup, and no second is made.
#[tokio::test]
async fn a_person_the_migration_moves_after_the_lookup_missed_is_found_and_not_made_twice() -> Result<()> {
    let node = durable::open(SqliteStorageEngine::open_in_memory().await?).await?;
    let graph = ProjectGraph::ensure(node.context_async(DEFAULT_CONTEXT).await?, uuid::Uuid::new_v4(), "alpha").await?;
    let trx = graph.begin();
    let person = trx
        .create(&before_d261::Principal {
            project: graph.project(),
            kind: PrincipalKind::Human,
            handle: "ines".into(),
            display_name: "Ines".into(),
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
    trx.commit().await?;

    assert_eq!(found_by_name(&graph, "ines", PrincipalKind::Human).await?, None, "the lookup misses");
    principals::convert(&graph).await?;
    let made = made_unless_unmoved(&graph, "ines", "Ines", PrincipalKind::Human, None).await?;
    assert_eq!(made, person, "the one moved");
    assert_eq!(super::principals(&graph).await?.len(), 1, "and no second");
    Ok(())
}
