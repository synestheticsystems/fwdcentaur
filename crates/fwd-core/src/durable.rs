//! The durable nodes fwd opens: the daemon's over its store, and any other a tool or a test opens
//! over a store of its own. Each imports the Commons' kept mint (`ankurah_commons::seed`) as soon as
//! its system root is ready, before anything can bind a model to it: a Commons model declares the id
//! its kept mint gives it, a fetch, a query, a get or a create binds the models it names as
//! registration does, and a model bound before the import fails to find its definition, and keeps
//! that binding until the node restarts. [`open`] is the one place fwd makes a durable node, and it
//! hands the node out only once it is seeded, so no caller can bind first.

use ankurah::storage::StorageEngine;
use ankurah::{Node, PermissiveAgent};
use anyhow::{anyhow, Context as _, Result};
use std::sync::Arc;

/// A durable node over `storage`: its system root loaded, or created when the store holds none,
/// then seeded with the Commons' kept mint. Seeding a store that holds the mint already changes
/// nothing, so a store is seeded at every start.
pub async fn open<SE>(storage: SE) -> Result<Node<SE, PermissiveAgent>>
where
    SE: StorageEngine + Send + Sync + 'static,
{
    let node = Node::new_durable(Arc::new(storage), PermissiveAgent::new());
    node.system.wait_loaded().await?;
    if node.system.root().is_none() {
        tracing::info!("creating the Ankurah system");
        node.system.create().await?;
    }
    node.system.wait_system_ready().await.map_err(|e| anyhow!("the Ankurah system did not become ready: {e}"))?;
    ankurah_commons::seed(&node).await.context("seeding the node with the Commons' kept mint")?;
    Ok(node)
}
