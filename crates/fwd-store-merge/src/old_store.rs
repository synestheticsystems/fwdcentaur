//! One project's store as the per-project build left it: the current values of every record,
//! read with plain SQLite from the engine's materialization tables, each table found through the
//! engine's model map and each column through its column map and the store's catalog. Ankurah's
//! event history is not read: it stays in the old folder, which that build still reads.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use ankurah::proto::ModelId;
use ankurah::PropertyId;
use rusqlite::types::Value as Sql;
use rusqlite::{Connection, OpenFlags};

use crate::records::{fields_of, MODELS};

/// A value as a materialization column holds it, with a JSON property's value parsed.
#[derive(Debug, Clone, PartialEq)]
pub enum Stored {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Json(serde_json::Value),
}

/// One record: its id and, by property name, the values its columns hold.
#[derive(Debug, Clone)]
pub struct Record {
    pub id: String,
    pub values: BTreeMap<String, Stored>,
}

impl Record {
    pub fn get(&self, field: &str) -> &Stored {
        self.values.get(field).unwrap_or(&Stored::Null)
    }
}

/// The records of one project's store, by model.
pub struct OldStore {
    /// Each model's records, in the order their entities sit in `_ankurah_entity`.
    pub records: BTreeMap<&'static str, Vec<Record>>,
    /// Each model's record ids, in the order its materialization table holds its rows.
    pub materialized_order: BTreeMap<&'static str, Vec<String>>,
}

#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("{0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("table {table:?} holds the records of model {model:?}, which is not one of fwd's nine")]
    UnknownModel { table: String, model: String },
    #[error("column {column:?} of table {table:?} has values but names no field of {model}")]
    UnknownField { table: String, model: &'static str, column: String },
    #[error("column {column:?} of table {table:?} is not in the engine's column map or the catalog")]
    UnmappedColumn { table: String, column: String },
    #[error("{0}")]
    Undecodable(String),
}

/// Opens a database only to read it, as a file nothing else is writing: SQLite takes no locks and
/// creates no files beside it. The caller has made sure no write-ahead log or journal is beside it.
pub fn open_immutable(path: &Path) -> Result<Connection, rusqlite::Error> {
    let mut uri = String::from("file:");
    for byte in path.as_os_str().as_encoded_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => uri.push(*byte as char),
            _ => uri.push_str(&format!("%{byte:02X}")),
        }
    }
    uri.push_str("?immutable=1");
    Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
}

/// A model's materialization table, with each column's property.
pub struct ModelTable {
    pub model: String,
    pub table: String,
    /// (column, property name, the property's value type in the catalog)
    pub columns: Vec<(String, String, String)>,
}

/// Every model's materialization table of the store `db`, through the engine's maps and the
/// catalog. A store that never made a model's record has no table for it.
pub fn all_model_tables(db: &Connection) -> Result<Vec<ModelTable>, ReadError> {
    let mut properties: BTreeMap<String, (String, String)> = BTreeMap::new();
    for row in db.prepare(r#"SELECT "id", "name", "value_type" FROM "_ankurah_property""#)?.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, Option<String>>(2)?))
    })? {
        let (id, name, value_type) = row?;
        properties.insert(id, (name, value_type.unwrap_or_default()));
    }
    let mut models: BTreeMap<String, String> = BTreeMap::new();
    for row in db
        .prepare(r#"SELECT "id", "name" FROM "_ankurah_model""#)?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
    {
        let (id, name): (String, String) = row?;
        models.insert(id, name);
    }
    let maps: Vec<(Vec<u8>, String)> = db
        .prepare(r#"SELECT "model_key", "materialization_table_name" FROM "_ankurah_sqlite_model_map""#)?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()?;

    let mut tables = Vec::new();
    for (key, table) in maps {
        let model_id: ModelId =
            bincode::deserialize(&key).map_err(|e| ReadError::Undecodable(format!("model key of {table}: {e}")))?;
        let ModelId::EntityId(entity) = model_id else { continue };
        let model = models.get(&entity.to_base64()).cloned().unwrap_or_else(|| entity.to_base64());
        let mut column_properties: BTreeMap<String, String> = BTreeMap::new();
        for row in db
            .prepare(r#"SELECT "column_name", "property_key" FROM "_ankurah_sqlite_column_map" WHERE "model_key" = ?"#)?
            .query_map([&key], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?
        {
            let (column, property_key) = row?;
            column_properties.insert(column, property_key);
        }
        let mut columns = Vec::new();
        let names: Vec<String> = db
            .prepare(&format!(r#"SELECT name FROM pragma_table_info('{table}')"#))?
            .query_map([], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        for column in names.into_iter().filter(|c| c != "id") {
            let property = column_properties
                .get(&column)
                .and_then(|key| serde_json::from_str::<PropertyId>(key).ok())
                .and_then(|id| match id {
                    PropertyId::EntityId(property) => properties.get(&property.to_base64()).cloned(),
                    _ => None,
                });
            let Some((name, value_type)) = property else {
                return Err(ReadError::UnmappedColumn { table: table.clone(), column });
            };
            columns.push((column, name, value_type));
        }
        tables.push(ModelTable { model, table, columns });
    }
    Ok(tables)
}

/// The nine models' tables of an old store, each column kept only when it names a field of its
/// model: one that names none and holds no value is left out, and one that holds a value is
/// refused, since the current models could not carry it.
fn model_tables(db: &Connection) -> Result<Vec<(&'static str, ModelTable)>, ReadError> {
    let mut nine = Vec::new();
    for mut table in all_model_tables(db)? {
        let Some(&model) = MODELS.iter().find(|m| **m == table.model) else {
            return Err(ReadError::UnknownModel { table: table.table, model: table.model });
        };
        let mut kept = Vec::new();
        for (column, name, value_type) in std::mem::take(&mut table.columns) {
            if fields_of(model).contains(&name.as_str()) {
                kept.push((column, name, value_type));
                continue;
            }
            let filled: i64 = db.query_row(
                &format!(r#"SELECT count(*) FROM "{}" WHERE "{column}" IS NOT NULL"#, table.table),
                [],
                |row| row.get(0),
            )?;
            if filled > 0 {
                return Err(ReadError::UnknownField { table: table.table, model, column });
            }
        }
        table.columns = kept;
        nine.push((model, table));
    }
    Ok(nine)
}

impl OldStore {
    pub fn read(path: &Path) -> Result<OldStore, ReadError> {
        let db = open_immutable(path)?;
        let mut records = BTreeMap::new();
        let mut materialized_order = BTreeMap::new();
        for (model, table) in model_tables(&db)? {
            let select = table
                .columns
                .iter()
                .map(|(column, _, _)| {
                    format!(r#"CASE typeof(t."{column}") WHEN 'blob' THEN json(t."{column}") ELSE t."{column}" END"#)
                })
                .collect::<Vec<_>>();
            let sql = format!(
                r#"SELECT t."id"{} FROM "{}" t JOIN "_ankurah_entity" e ON e."id" = t."id" ORDER BY e.rowid"#,
                select.iter().map(|s| format!(", {s}")).collect::<String>(),
                table.table
            );
            let mut rows = Vec::new();
            let mut statement = db.prepare(&sql)?;
            let mut cursor = statement.query([])?;
            while let Some(row) = cursor.next()? {
                let id: String = row.get(0)?;
                let mut values = BTreeMap::new();
                for (i, (_, name, value_type)) in table.columns.iter().enumerate() {
                    let value = match row.get::<_, Sql>(i + 1)? {
                        Sql::Null => Stored::Null,
                        Sql::Integer(n) => Stored::Integer(n),
                        Sql::Real(f) => Stored::Real(f),
                        Sql::Text(text) if value_type == "json" => Stored::Json(
                            serde_json::from_str(&text)
                                .map_err(|e| ReadError::Undecodable(format!("{name} of {id}: {e}")))?,
                        ),
                        Sql::Text(text) => Stored::Text(text),
                        Sql::Blob(_) => return Err(ReadError::Undecodable(format!("{name} of {id} holds bytes"))),
                    };
                    values.insert(name.clone(), value);
                }
                rows.push(Record { id, values });
            }
            let in_table: i64 =
                db.query_row(&format!(r#"SELECT count(*) FROM "{}""#, table.table), [], |row| row.get(0))?;
            if in_table as usize != rows.len() {
                return Err(ReadError::Undecodable(format!(
                    "table {} holds rows of entities the store does not hold",
                    table.table
                )));
            }
            let order: Vec<String> = db
                .prepare(&format!(r#"SELECT "id" FROM "{}" ORDER BY rowid"#, table.table))?
                .query_map([], |row| row.get(0))?
                .collect::<Result<_, _>>()?;
            records.insert(model, rows);
            materialized_order.insert(model, order);
        }
        Ok(OldStore { records, materialized_order })
    }

    /// The records `fwd admin migrate` would still change, by kind: reported, and carried as
    /// they are, since the current models represent them.
    pub fn migrate_would_change(&self) -> Vec<(&'static str, usize)> {
        let of = |model: &str| self.records.get(model).map(Vec::as_slice).unwrap_or_default();
        let text = |record: &Record, field: &str| match record.get(field) {
            Stored::Text(text) => Some(text.clone()),
            _ => None,
        };
        let json_text = |record: &Record, field: &str, key: &str| match record.get(field) {
            Stored::Json(value) => value.get(key).and_then(|v| v.as_str()).map(str::to_owned),
            _ => None,
        };
        let scopes: BTreeSet<&str> = of("Scope").iter().map(|s| s.id.as_str()).collect();
        let worked_in: BTreeSet<String> = of("Relation")
            .iter()
            .filter(|r| text(r, "kind").as_deref() == Some("worked_in"))
            .filter_map(|r| text(r, "from_id"))
            .collect();
        let kinds = [
            (
                "relations named assigned_to",
                of("Relation").iter().filter(|r| text(r, "kind").as_deref() == Some("assigned_to")).count(),
            ),
            (
                "principals whose membership is unrecorded",
                of("Principal").iter().filter(|p| matches!(p.get("member"), Stored::Null)).count(),
            ),
            (
                "questions addressed to the supervisor role",
                of("Message").iter().filter(|m| json_text(m, "context", "to").as_deref() == Some("supervisor")).count(),
            ),
            (
                "tasks whose recorded scope has no worked_in relation",
                of("Task")
                    .iter()
                    .filter(|t| {
                        json_text(t, "context", "scope_id").is_some_and(|s| scopes.contains(s.as_str()))
                            && !worked_in.contains(&t.id)
                    })
                    .count(),
            ),
        ];
        kinds.into_iter().filter(|(_, n)| *n > 0).collect()
    }
}
