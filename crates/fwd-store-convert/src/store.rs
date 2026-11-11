//! One project's store: its rows read from the file Ankurah f8e85d2 wrote, loaded into a new file
//! through Ankurah 5d8a000b's SQLite engine, then judged twice before the store counts as converted:
//! read back through the new engine against what was converted, and inventoried against the old
//! file with plain SQLite.

mod inventory;
mod read_back;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use ankurah::core::storage::{StorageEngine, StorageTransaction};
use ankurah::proto::{Attested, Clock, EntityState};
use ankurah_storage_sqlite::SqliteStorageEngine;
use rusqlite::{Connection, OpenFlags, OptionalExtension};

use crate::f8e85d2_to_5d8a000b::{self as hop, Converted, OldEntity, OldEvent, Refusal};

/// The tables Ankurah's SQLite engine keeps for itself, the same at both ends of the hop. Every
/// other table in a store is a materialization the engine registered in the model map, which it
/// rebuilds from the states it is given.
const ENGINE_TABLES: [&str; 6] = [
    "_ankurah_meta",
    "_ankurah_sqlite_model_map",
    "_ankurah_sqlite_column_map",
    "_ankurah_entity",
    "_ankurah_event",
    "_ankurah_entity_model",
];
const MODEL_MAP: &str = "_ankurah_sqlite_model_map";
const COLUMN_MAP: &str = "_ankurah_sqlite_column_map";

#[derive(Debug)]
pub struct StoreReport {
    pub entities: usize,
    pub events: usize,
    pub ids_changed: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Refused(#[from] Refusal),
    #[error("the store records protocol version {found:?}, and this hop reads only stores that record {expected:?}")]
    ProtocolVersion { found: Option<String>, expected: &'static str },
    #[error("table {0:?} is neither one of Ankurah's own nor a materialization its engine registered, so converting would lose it")]
    UnknownTable(String),
    #[error("reading the old store: {0}")]
    Read(#[from] rusqlite::Error),
    #[error("writing the new store: {0}")]
    Write(String),
    #[error("the new store does not read back as it was converted: {0}")]
    ReadBack(String),
    #[error("the new store's rows differ from the old store's: {0}")]
    Inventory(String),
}

/// Converts the store at `from` into a new store at `to`, which must not exist.
pub async fn convert(from: &Path, to: &Path) -> Result<StoreReport, StoreError> {
    let old = OldStore::read(from)?;
    let converted = hop::convert(old.events, old.entities)?;
    load(to, &old.name_maps, &old.materialized_order, &converted).await?;
    read_back::judge(to, &converted).await?;
    inventory::compare(from, to).map_err(StoreError::Inventory)?;
    Ok(StoreReport {
        entities: converted.states.len(),
        events: converted.events.len(),
        ids_changed: converted.ids_changed,
    })
}

/// Opens a database only to read it, as a file nothing else is writing: SQLite takes no locks and
/// creates no files beside it. The caller has made sure no write-ahead log or journal is beside it,
/// so everything in it is in this one file.
pub fn open_immutable(path: &Path) -> Result<Connection, rusqlite::Error> {
    let uri = format!("file:{}?immutable=1", percent_encoded(path));
    Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
}

/// A path as a URI path: SQLite reads `?` and `#` in a URI filename as the start of its query or
/// fragment, and `%` as an escape.
fn percent_encoded(path: &Path) -> String {
    let mut out = String::new();
    for byte in path.as_os_str().as_encoded_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => out.push(*byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// A store's rows as Ankurah f8e85d2 wrote them, its entities in the order their rows sit in.
struct OldStore {
    events: Vec<OldEvent>,
    entities: Vec<OldEntity>,
    name_maps: NameMaps,
    /// For each materialization table, its entities in the order their rows sit in.
    materialized_order: Vec<Vec<String>>,
}

impl OldStore {
    fn read(path: &Path) -> Result<Self, StoreError> {
        let db = open_immutable(path)?;
        let recorded: Option<String> = db
            .query_row(r#"SELECT "value" FROM "_ankurah_meta" WHERE "key" = 'protocol_version'"#, [], |row| row.get(0))
            .optional()?;
        if recorded.as_deref() != Some(hop::PROTOCOL_VERSION) {
            return Err(StoreError::ProtocolVersion { found: recorded, expected: hop::PROTOCOL_VERSION });
        }
        let name_maps = NameMaps::read(&db)?;
        // GLOB, since LIKE reads the underscore as any character and would pass over a table such as
        // `sqlitebackup`; SQLite reserves the `sqlite_` prefix for its own tables.
        let tables = column_of::<String>(
            &db,
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT GLOB 'sqlite_*'",
        )?;
        if let Some(unknown) = tables
            .into_iter()
            .find(|table| !ENGINE_TABLES.contains(&table.as_str()) && !name_maps.tables.contains(table))
        {
            return Err(StoreError::UnknownTable(unknown));
        }

        let events = db
            .prepare(r#"SELECT "id", "entity_id", "body", "parent", "attestations" FROM "_ankurah_event""#)?
            .query_map([], |row| {
                Ok(OldEvent {
                    id: row.get(0)?,
                    entity_id: row.get(1)?,
                    body: row.get(2)?,
                    parent: row.get(3)?,
                    attestations: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut memberships: BTreeMap<String, Vec<Vec<u8>>> = BTreeMap::new();
        let mut rows = db.prepare(r#"SELECT "entity_id", "model_key" FROM "_ankurah_entity_model""#)?;
        for row in rows.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)))? {
            let (entity, key) = row?;
            memberships.entry(entity).or_default().push(key);
        }
        let entities = db
            .prepare(r#"SELECT "id", "state_buffer", "head", "attestations" FROM "_ankurah_entity" ORDER BY rowid"#)?
            .query_map([], |row| {
                let id: String = row.get(0)?;
                Ok(OldEntity {
                    memberships: memberships.remove(&id).unwrap_or_default(),
                    id,
                    state_buffer: row.get(1)?,
                    head: row.get(2)?,
                    attestations: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let materialized_order = name_maps
            .tables
            .iter()
            .map(|table| column_of::<String>(&db, &format!(r#"SELECT "id" FROM "{table}" ORDER BY rowid"#)))
            .collect::<Result<_, _>>()?;
        Ok(Self { events, entities, name_maps, materialized_order })
    }
}

/// The SQLite engine's own record of the names it gave each model's table and each property's
/// column, which it reads before anything else when it meets a model. It names them from the
/// catalog's labels when it first meets them; this converter runs no node, so it has no catalog,
/// and without the record the engine would name the tables and columns after ids. Carrying the
/// record keeps every name the old engine gave. Both tables have the same layout at both ends of
/// the hop.
struct NameMaps {
    /// The `CREATE TABLE` statements the old engine ran for the two maps, from `sqlite_master`.
    definitions: Vec<String>,
    models: Vec<(Vec<u8>, String)>,
    columns: Vec<(Vec<u8>, String, String)>,
    /// The materialization tables the model map names.
    tables: BTreeSet<String>,
}

impl NameMaps {
    fn read(db: &Connection) -> Result<Self, rusqlite::Error> {
        let definition = |table: &str| {
            db.query_row("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?", [table], |row| {
                row.get::<_, String>(0)
            })
            .optional()
        };
        let (model_map, column_map) = (definition(MODEL_MAP)?, definition(COLUMN_MAP)?);
        let models: Vec<(Vec<u8>, String)> = match model_map {
            Some(_) => db
                .prepare(&format!(r#"SELECT "model_key", "materialization_table_name" FROM "{MODEL_MAP}""#))?
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<_, _>>()?,
            None => Vec::new(),
        };
        let columns = match column_map {
            Some(_) => db
                .prepare(&format!(r#"SELECT "model_key", "property_key", "column_name" FROM "{COLUMN_MAP}""#))?
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<Result<_, _>>()?,
            None => Vec::new(),
        };
        let tables = models.iter().map(|(_, table)| table.clone()).collect();
        Ok(Self { definitions: model_map.into_iter().chain(column_map).collect(), models, columns, tables })
    }

    /// Writes the record into the new store, before the engine meets any model there.
    fn seed(&self, path: &Path) -> Result<(), rusqlite::Error> {
        let mut db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        let tx = db.transaction()?;
        for definition in &self.definitions {
            tx.execute(definition, [])?;
        }
        for (model, table) in &self.models {
            tx.execute(
                &format!(r#"INSERT INTO "{MODEL_MAP}" ("model_key", "materialization_table_name") VALUES (?, ?)"#),
                (model, table),
            )?;
        }
        for (model, property, column) in &self.columns {
            tx.execute(
                &format!(r#"INSERT INTO "{COLUMN_MAP}" ("model_key", "property_key", "column_name") VALUES (?, ?, ?)"#),
                (model, property, column),
            )?;
        }
        tx.commit()
    }
}

/// Writes the converted store through the new engine's own transactions, so the engine lays out its
/// tables and materializes every state itself, and records the protocol version in a fresh file.
///
/// fwd lists what a query finds in the order the store yields it, which is the order the rows sit
/// in their tables, so the rows are written to sit as they did. An entity's row in
/// `_ankurah_entity` stays where its first write put it, while the engine replaces its materialized
/// row, moving it to the end, at every write; and one transaction writes its entities in the order
/// of their ids. So each entity is written in a transaction of its own, in the old entity table's
/// order, and then written again, unchanged, in its materialization table's old order. That
/// reproduces every table's order when each entity belongs to one model, as fwd's do; an entity in
/// two models would be moved in both tables by each pass, and the inventory then refuses the store.
async fn load(
    to: &Path,
    name_maps: &NameMaps,
    materialized_order: &[Vec<String>],
    converted: &Converted,
) -> Result<(), StoreError> {
    let engine = SqliteStorageEngine::open(to).await.map_err(|e| StoreError::Write(format!("{e:#}")))?;
    name_maps.seed(to).map_err(|e| StoreError::Write(format!("recording the table and column names: {e}")))?;
    let mut transaction = engine.transaction();
    transaction.add_events(&converted.events).await.map_err(|e| StoreError::Write(e.to_string()))?;
    commit(transaction).await?;
    for state in &converted.states {
        // Each entity is new to this store, so its expected head is the empty one.
        write_state(&engine, &Clock::default(), state).await?;
    }
    let by_id: BTreeMap<String, &Attested<EntityState>> =
        converted.states.iter().map(|state| (state.payload.entity_id.to_base64(), state)).collect();
    for id in materialized_order.iter().flatten() {
        let state = by_id
            .get(id)
            .ok_or_else(|| StoreError::Write(format!("a materialized row names entity {id}, which has no state")))?;
        write_state(&engine, &state.payload.state.head, state).await?;
    }
    Ok(())
}

async fn write_state(
    engine: &SqliteStorageEngine,
    expected_head: &Clock,
    state: &Attested<EntityState>,
) -> Result<(), StoreError> {
    let mut transaction = engine.transaction();
    transaction
        .set_state(expected_head, state)
        .await
        .map_err(|e| StoreError::Write(format!("entity {}: {e}", state.payload.entity_id)))?;
    commit(transaction).await
}

async fn commit(transaction: impl StorageTransaction) -> Result<(), StoreError> {
    let outcome = transaction.commit().await.map_err(|e| StoreError::Write(e.to_string()))?;
    outcome.committed().map_err(|e| StoreError::Write(e.to_string()))?;
    Ok(())
}

fn column_of<T: rusqlite::types::FromSql>(db: &Connection, sql: &str) -> Result<Vec<T>, rusqlite::Error> {
    db.prepare(sql)?.query_map([], |row| row.get(0))?.collect()
}

#[cfg(test)]
pub(crate) mod tests;
