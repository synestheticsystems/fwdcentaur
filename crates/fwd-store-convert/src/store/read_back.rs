//! The first judge of a new store: Ankurah 5d8a000b reads it back. Every event row decodes into an
//! event whose id, as that revision computes it, is the row's own key, and which is the event that
//! was converted; every converted event is stored. Every state reads back through a fresh engine as
//! it was converted, every backend decodes it, its head names stored events at their generations,
//! and every LWW value names a stored event as its writer.

use std::collections::BTreeMap;
use std::path::Path;

use ankurah::core::property::backend::{backend_from_string, LWWBackend, PropertyBackend};
use ankurah::core::storage::StorageEngine;
use ankurah::proto::{AttestationSet, Attested, Clock, EntityId, Event, EventBody, EventId};
use ankurah_storage_sqlite::SqliteStorageEngine;
use rusqlite::{Connection, OpenFlags};

use super::{column_of, StoreError};
use crate::f8e85d2_to_5d8a000b::Converted;

pub(super) async fn judge(to: &Path, converted: &Converted) -> Result<(), StoreError> {
    let generations = judge_events(to, converted)?;
    judge_states(to, converted, &generations).await
}

/// Each stored event row against the events converted, and the generation of each stored event.
fn judge_events(to: &Path, converted: &Converted) -> Result<BTreeMap<EventId, u32>, StoreError> {
    let db = Connection::open_with_flags(to, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(fail)?;
    let mut expected: BTreeMap<EventId, &Attested<Event>> =
        converted.events.iter().map(|event| (event.payload.id(), event)).collect();
    let mut generations = BTreeMap::new();
    let mut rows = db
        .prepare(r#"SELECT "id", "entity_id", "body", "parent", "attestations" FROM "_ankurah_event""#)
        .map_err(fail)?;
    let rows = rows
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Vec<u8>>(4)?,
            ))
        })
        .map_err(fail)?;
    for row in rows {
        let (key, entity_id, body, parent, attestations) = row.map_err(fail)?;
        let undecodable =
            |e: &dyn std::fmt::Display| StoreError::ReadBack(format!("event row {key} does not decode: {e}"));
        let stored = Attested {
            payload: Event {
                entity_id: EntityId::from_base64(&entity_id).map_err(|e| undecodable(&e))?,
                parent: serde_json::from_str::<Clock>(&parent).map_err(|e| undecodable(&e))?,
                body: bincode::deserialize::<EventBody>(&body).map_err(|e| undecodable(&e))?,
            },
            attestations: bincode::deserialize::<AttestationSet>(&attestations).map_err(|e| undecodable(&e))?,
        };
        let id = stored.payload.id();
        if id.to_base64() != key {
            return Err(StoreError::ReadBack(format!("event row {key} holds the event whose id is {id}")));
        }
        if expected.remove(&id) != Some(&stored) {
            return Err(StoreError::ReadBack(format!("event {id} is stored, but not as it was converted")));
        }
        stored.payload.validate_structure().map_err(|e| StoreError::ReadBack(format!("event {id}: {e}")))?;
        generations.insert(id, stored.payload.generation());
    }
    if !expected.is_empty() {
        return Err(StoreError::ReadBack(format!("{} converted events are not stored", expected.len())));
    }
    Ok(generations)
}

async fn judge_states(
    to: &Path,
    converted: &Converted,
    generations: &BTreeMap<EventId, u32>,
) -> Result<(), StoreError> {
    let engine = SqliteStorageEngine::open(to).await.map_err(|e| StoreError::ReadBack(format!("{e:#}")))?;
    let db = Connection::open_with_flags(to, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(fail)?;
    let stored = column_of::<String>(&db, r#"SELECT "id" FROM "_ankurah_entity""#).map_err(fail)?;
    if stored.len() != converted.states.len() {
        return Err(StoreError::ReadBack(format!(
            "{} states were converted and {} are stored",
            converted.states.len(),
            stored.len()
        )));
    }
    for written in &converted.states {
        let entity = written.payload.entity_id;
        let read = engine.get_state(entity).await.map_err(fail)?;
        if read.payload != written.payload {
            return Err(StoreError::ReadBack(format!("entity {entity} reads back other than it was converted")));
        }
        for (generation, tip) in read.payload.state.head.entries() {
            if generations.get(tip) != Some(generation) {
                return Err(StoreError::ReadBack(format!(
                    "entity {entity}'s head names {tip} at generation {generation}, which no stored event has"
                )));
            }
        }
        for (backend, buffer) in read.payload.state.state_buffers.iter() {
            backend_from_string(backend, Some(buffer)).map_err(fail)?;
            if backend == "lww" {
                let lww = LWWBackend::from_state_buffer(buffer).map_err(fail)?;
                for property in lww.property_values().keys() {
                    let writer = lww.get_event_id(property);
                    if !writer.as_ref().is_some_and(|writer| generations.contains_key(writer)) {
                        return Err(StoreError::ReadBack(format!(
                            "entity {entity}'s value for {property} names no stored event as its writer"
                        )));
                    }
                }
            }
        }
    }
    Ok(())
}

fn fail(e: impl std::fmt::Display) -> StoreError {
    StoreError::ReadBack(e.to_string())
}
