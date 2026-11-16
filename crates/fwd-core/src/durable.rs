//! The durable nodes fwd opens: the daemon's over its store, and any other a tool or a test opens
//! over a store of its own. [`open`] is the one place fwd makes a durable node, so that what every
//! such node needs before it is used is done in one place.

use ankurah::storage::StorageEngine;
use ankurah::{Node, PermissiveAgent};
use anyhow::{anyhow, Result};
use std::sync::Arc;

/// A durable node over `storage`: its system root loaded, or created when the store holds none.
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
    Ok(node)
}
