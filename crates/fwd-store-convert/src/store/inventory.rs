//! The second judge of a new store, which rests on nothing the conversion computed: the old file
//! and the new one, read with plain SQLite, hold the same entities, the same number of events, and
//! in every materialization table the same rows with the same values, in the same order, which are
//! what fwd's queries read and the order in which they list them.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use rusqlite::types::Value;
use rusqlite::{Connection, OpenFlags, OptionalExtension};

use super::{column_of, open_immutable, MODEL_MAP};

/// A materialization table's rows: by entity id, each row's value in each column.
type Rows = BTreeMap<String, BTreeMap<String, Value>>;

/// Says what differs between the old store at `old` and the new one at `new`, by entity id, table
/// and column, never by value.
pub(super) fn compare(old: &Path, new: &Path) -> Result<(), String> {
    // The new file is opened as SQLite opens any database, so that a write-ahead log the engine left
    // beside it is read too; nothing is written through this connection.
    let new = Connection::open_with_flags(new, OpenFlags::SQLITE_OPEN_READ_WRITE).map_err(|e| e.to_string())?;
    let old = open_immutable(old).map_err(|e| e.to_string())?;

    let ids =
        |db: &Connection| column_of::<String>(db, r#"SELECT "id" FROM "_ankurah_entity""#).map(BTreeSet::from_iter);
    let (old_ids, new_ids) = (ids(&old).map_err(|e| e.to_string())?, ids(&new).map_err(|e| e.to_string())?);
    if old_ids != new_ids {
        return Err(format!(
            "{} entities are missing and {} are new",
            old_ids.difference(&new_ids).count(),
            new_ids.difference(&old_ids).count()
        ));
    }
    let events =
        |db: &Connection| db.query_row(r#"SELECT count(*) FROM "_ankurah_event""#, [], |row| row.get::<_, i64>(0));
    let (old_events, new_events) = (events(&old).map_err(|e| e.to_string())?, events(&new).map_err(|e| e.to_string())?);
    if old_events != new_events {
        return Err(format!("{old_events} events became {new_events}"));
    }

    let tables = |db: &Connection| -> rusqlite::Result<BTreeSet<String>> {
        let has_map = db
            .query_row("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?", [MODEL_MAP], |_| Ok(()))
            .optional()?
            .is_some();
        if !has_map {
            return Ok(BTreeSet::new());
        }
        column_of::<String>(db, &format!(r#"SELECT "materialization_table_name" FROM "{MODEL_MAP}""#))
            .map(BTreeSet::from_iter)
    };
    let all: BTreeSet<String> =
        tables(&old).map_err(|e| e.to_string())?.union(&tables(&new).map_err(|e| e.to_string())?).cloned().collect();
    for table in all {
        let (old_rows, new_rows) =
            (rows(&old, &table).map_err(|e| e.to_string())?, rows(&new, &table).map_err(|e| e.to_string())?);
        compare_rows(&table, &old_rows, &new_rows)?;
        // fwd lists what a query finds in row order. The load writes each entity once more per
        // materialization it belongs to, in that table's old order, which reproduces every table's
        // order when each entity belongs to one model, as fwd's do; an order it could not reproduce
        // is refused here rather than changed silently.
        if row_order(&old, &table).map_err(|e| e.to_string())? != row_order(&new, &table).map_err(|e| e.to_string())? {
            return Err(format!("table {table:?} lists its rows in another order"));
        }
    }
    Ok(())
}

/// The entity ids in `table`, in the order its rows sit, or none when there is no such table.
fn row_order(db: &Connection, table: &str) -> rusqlite::Result<Vec<String>> {
    let exists = db
        .query_row("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?", [table], |_| Ok(()))
        .optional()?;
    if exists.is_none() {
        return Ok(Vec::new());
    }
    column_of(db, &format!(r#"SELECT "id" FROM "{table}" ORDER BY rowid"#))
}

/// Every row of `table`, or none when the store has no such table: the engine makes a
/// materialization table when it first writes a row into it.
fn rows(db: &Connection, table: &str) -> rusqlite::Result<Rows> {
    let exists = db
        .query_row("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?", [table], |_| Ok(()))
        .optional()?;
    if exists.is_none() {
        return Ok(Rows::new());
    }
    let mut statement = db.prepare(&format!(r#"SELECT * FROM "{table}""#))?;
    let names: Vec<String> = statement.column_names().into_iter().map(str::to_owned).collect();
    let id = names.iter().position(|name| name == "id").expect("a materialization table has an id column");
    let rows = statement.query_map([], |row| {
        let values = (0..names.len()).map(|i| row.get::<_, Value>(i)).collect::<rusqlite::Result<Vec<_>>>()?;
        let key = match &values[id] {
            Value::Text(key) => key.clone(),
            other => format!("{other:?}"),
        };
        Ok((key, names.iter().cloned().zip(values).collect()))
    })?;
    rows.collect()
}

/// The same entities in both, and in each row the same value in every column the new table has.
/// The engine adds a column when a value first needs it and reads a column it never added as
/// empty, so a column only the old table has must be empty in every old row.
fn compare_rows(table: &str, old: &Rows, new: &Rows) -> Result<(), String> {
    if old.keys().ne(new.keys()) {
        return Err(format!("table {table:?} holds other entities ({} rows became {})", old.len(), new.len()));
    }
    for (entity, old_row) in old {
        let new_row = &new[entity];
        for (column, value) in new_row {
            if old_row.get(column) != Some(value) {
                return Err(format!("entity {entity}'s {column:?} in table {table:?} differs"));
            }
        }
        if let Some((column, _)) =
            old_row.iter().find(|(column, value)| !new_row.contains_key(*column) && **value != Value::Null)
        {
            return Err(format!(
                "entity {entity}'s {column:?} in table {table:?} has a value, and the new table has no such column"
            ));
        }
    }
    Ok(())
}
