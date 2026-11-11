//! Writes every project's records into one fresh store through a node of this build, as the
//! daemon's node would hold them: one `Project` record per project, made as the daemon makes it,
//! then each project's records with their project named and their ids re-pointed, then one
//! `FormerId` per old id. Nothing runs but the node: no routing, no notification, no field set to
//! now; every value is the one the old store held.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use ankurah::core::storage::{StorageEngine, StorageTransaction};
use ankurah::policy::DEFAULT_CONTEXT;
use ankurah::{EntityId, Node, PermissiveAgent};
use ankurah_storage_sqlite::SqliteStorageEngine;
use anyhow::{Context as _, Result};
use fwd_core::model::FormerId;
use fwd_core::project_graph::ProjectGraph;
use uuid::Uuid;

use crate::old_store::{OldStore, Record};
use crate::records::{build, patch, Dangling, Fields, Ids, MODELS};

/// How many former ids one transaction makes. Their order is never read.
const FORMER_IDS_PER_TRANSACTION: usize = 500;

/// One project to move: its id and name from the index, and its old store's records.
pub struct Moving {
    pub uuid: Uuid,
    pub name: String,
    pub old: OldStore,
}

/// What moving one project did.
#[derive(Default)]
pub struct Moved {
    /// By model, how many records were made.
    pub made: Vec<(&'static str, usize)>,
    /// How many fields waited for a record made after theirs and were set in a second pass.
    pub patched: usize,
    /// The ids that name no record of the project, carried as they were.
    pub dangling: Vec<Dangling>,
    /// Each old id with the id its record has now.
    pub renumbered: HashMap<String, EntityId>,
}

/// Writes `projects` into a fresh store at `path`.
pub async fn write(path: &Path, projects: &[Moving]) -> Result<Vec<Moved>> {
    let engine = SqliteStorageEngine::open(path).await.context("opening the new store")?;
    let node = Node::new_durable(Arc::new(engine), PermissiveAgent::new());
    // As the daemon opens its node over a store it makes (crates/fwd/src/serve.rs, DaemonNode).
    node.system.wait_loaded().await?;
    node.system.create().await?;
    node.system
        .wait_system_ready()
        .await
        .map_err(|e| anyhow::anyhow!("the Ankurah system did not become ready: {e}"))?;
    let ctx = node.context_async(DEFAULT_CONTEXT).await?;

    let mut moved = Vec::new();
    for project in projects {
        let graph = ProjectGraph::ensure(ctx.clone(), project.uuid, &project.name).await?;
        moved.push(move_project(&graph, &project.old).await.with_context(|| format!("project {}", project.uuid))?);
    }
    drop(ctx);
    drop(node);
    reorder(path, projects, &moved).await?;
    Ok(moved)
}

async fn move_project(graph: &ProjectGraph, old: &OldStore) -> Result<Moved> {
    let mut ids = Ids::default();
    for model in MODELS {
        for record in old.records.get(model).into_iter().flatten() {
            ids.add(model, &record.id);
        }
    }
    let placeholder = *graph.project();
    let mut done = Moved::default();
    let mut waiting: Vec<(&'static str, EntityId, &Record, &'static str)> = Vec::new();
    for model in MODELS {
        let records = old.records.get(model).map(Vec::as_slice).unwrap_or_default();
        for record in records {
            let mut fields = Fields::new(model, record, &ids, placeholder);
            let made = build(model, &mut fields, graph.project())?;
            let (waits, dangling) = (std::mem::take(&mut fields.waiting), std::mem::take(&mut fields.dangling));
            // One record per transaction: a transaction writes its records in the order of their
            // ids, and each must sit after the one made before it, as it did in the old store.
            let trx = graph.begin();
            let new = made.create(&trx).await?;
            trx.commit().await?;
            ids.made(&record.id, new);
            done.renumbered.insert(record.id.clone(), new);
            waiting.extend(waits.into_iter().map(|field| (model, new, record, field)));
            done.dangling.extend(dangling);
        }
        done.made.push((model, records.len()));
    }
    for (model, new, record, field) in waiting {
        let trx = graph.begin();
        patch(graph, &trx, model, new, record, &ids, field).await?;
        trx.commit().await?;
        done.patched += 1;
    }
    let mut former: Vec<(&String, &EntityId)> = done.renumbered.iter().collect();
    former.sort();
    for chunk in former.chunks(FORMER_IDS_PER_TRANSACTION) {
        let trx = graph.begin();
        for (old, new) in chunk {
            trx.create(&FormerId { project: graph.project(), former: (*old).clone(), record: **new }).await?;
        }
        trx.commit().await?;
    }
    Ok(done)
}

/// Puts each project's rows back in the order its old materialization tables held them, since fwd
/// lists what a query finds in the order rows sit: the engine replaces a record's materialized row,
/// moving it to the end, at every write, so the second pass and the old store's own later writes
/// left them in another order. Each record's current state is written again, unchanged, in its own
/// transaction, through the engine alone.
async fn reorder(path: &Path, projects: &[Moving], moved: &[Moved]) -> Result<()> {
    let engine = SqliteStorageEngine::open(path).await.context("reopening the new store")?;
    for (project, done) in projects.iter().zip(moved) {
        for model in MODELS {
            for old in project.old.materialized_order.get(model).into_iter().flatten() {
                let new = done.renumbered[old];
                let state = engine.get_state(new).await?;
                let mut transaction = engine.transaction();
                transaction.set_state(&state.payload.state.head, &state).await?;
                transaction.commit().await?.committed()?;
            }
        }
    }
    Ok(())
}
