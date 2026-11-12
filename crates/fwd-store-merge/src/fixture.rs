//! Data folders in the per-project layout, written by hand with plain SQLite as the per-project
//! build's engine lays out a store: its catalog, its name maps, its entity table and one
//! materialization table per model. Only what the move reads is written; there is no event history.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ankurah::proto::ModelId;
use ankurah::{EntityId, PropertyId};
use rusqlite::Connection;
use uuid::Uuid;

/// A value as a materialization column holds it, with the property's type in the catalog.
pub enum V {
    Text(String),
    Int(i64),
    Bool(bool),
    /// An entity id: a reference, or a relation's end.
    Id(String),
    Json(serde_json::Value),
}

pub fn text(value: &str) -> V {
    V::Text(value.to_owned())
}

pub fn id(value: &str) -> V {
    V::Id(value.to_owned())
}

pub fn new_id() -> String {
    EntityId::random().to_base64()
}

/// One project's store, as the per-project build's engine laid it out.
pub struct OldStoreFile {
    db: Connection,
    models: BTreeMap<String, (Vec<u8>, String)>,
    properties: BTreeMap<(String, String), String>,
}

impl OldStoreFile {
    pub fn create(path: &Path) -> OldStoreFile {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let db = Connection::open(path).unwrap();
        db.execute_batch(
            r#"CREATE TABLE "_ankurah_meta" ("key" TEXT PRIMARY KEY, "value" TEXT);
               INSERT INTO "_ankurah_meta" VALUES ('protocol_version', '1');
               CREATE TABLE "_ankurah_entity" ("id" TEXT PRIMARY KEY, "state_buffer" BLOB NOT NULL, "head" TEXT NOT NULL, "attestations" BLOB NOT NULL);
               CREATE TABLE "_ankurah_sqlite_model_map" ("model_key" BLOB PRIMARY KEY, "materialization_table_name" TEXT NOT NULL UNIQUE);
               CREATE TABLE "_ankurah_sqlite_column_map" ("model_key" BLOB NOT NULL, "property_key" TEXT NOT NULL, "column_name" TEXT NOT NULL,
                   PRIMARY KEY ("model_key", "property_key"), UNIQUE ("model_key", "column_name"));
               CREATE TABLE "_ankurah_model" ("id" TEXT PRIMARY KEY, "label" TEXT, "name" TEXT);
               CREATE TABLE "_ankurah_property" ("id" TEXT PRIMARY KEY, "name" TEXT, "minted_for" TEXT, "backend" TEXT, "value_type" TEXT);"#,
        )
        .unwrap();
        OldStoreFile { db, models: BTreeMap::new(), properties: BTreeMap::new() }
    }

    /// Writes a record of `model` with these values and returns its id.
    pub fn record(&mut self, model: &str, values: Vec<(&str, V)>) -> String {
        let id = new_id();
        self.record_as(model, &id, values);
        id
    }

    pub fn record_as(&mut self, model: &str, id: &str, values: Vec<(&str, V)>) {
        let (key, table) = self.model(model);
        self.db.execute(r#"INSERT INTO "_ankurah_entity" VALUES (?, x'', '[]', x'')"#, [id]).unwrap();
        self.db.execute(&format!(r#"INSERT INTO "{table}" ("id") VALUES (?)"#), [id]).unwrap();
        for (field, value) in values {
            let value_type = match &value {
                V::Text(_) => "string",
                V::Int(_) => "i64",
                V::Bool(_) => "bool",
                V::Id(_) => "entityid",
                V::Json(_) => "json",
            };
            let column = self.column(model, &key, &table, field, value_type);
            let set = format!(
                r#"UPDATE "{table}" SET "{column}" = {} WHERE "id" = ?"#,
                if matches!(value, V::Json(_)) { "jsonb(?)" } else { "?" }
            );
            match value {
                V::Text(text) | V::Id(text) => self.db.execute(&set, (text, id)),
                V::Int(n) => self.db.execute(&set, (n, id)),
                V::Bool(b) => self.db.execute(&set, (b as i64, id)),
                V::Json(json) => self.db.execute(&set, (json.to_string(), id)),
            }
            .unwrap();
        }
    }

    fn model(&mut self, model: &str) -> (Vec<u8>, String) {
        if let Some(found) = self.models.get(model) {
            return found.clone();
        }
        let entity = EntityId::random();
        let key = bincode::serialize(&ModelId::EntityId(entity)).unwrap();
        let table = model.to_lowercase();
        self.db
            .execute(r#"INSERT INTO "_ankurah_model" VALUES (?, ?, ?)"#, (entity.to_base64(), &table, model))
            .unwrap();
        self.db.execute(r#"INSERT INTO "_ankurah_sqlite_model_map" VALUES (?, ?)"#, (&key, &table)).unwrap();
        self.db.execute(&format!(r#"CREATE TABLE "{table}" ("id" TEXT PRIMARY KEY)"#), []).unwrap();
        self.models.insert(model.to_owned(), (key.clone(), table.clone()));
        (key, table)
    }

    fn column(&mut self, model: &str, key: &[u8], table: &str, field: &str, value_type: &str) -> String {
        let at = (model.to_owned(), field.to_owned());
        if !self.properties.contains_key(&at) {
            let property = EntityId::random();
            self.db
                .execute(
                    r#"INSERT INTO "_ankurah_property" VALUES (?, ?, NULL, 'lww', ?)"#,
                    (property.to_base64(), field, value_type),
                )
                .unwrap();
            let property_key = serde_json::to_string(&PropertyId::EntityId(property)).unwrap();
            self.db
                .execute(r#"INSERT INTO "_ankurah_sqlite_column_map" VALUES (?, ?, ?)"#, (key, property_key, field))
                .unwrap();
            self.db.execute(&format!(r#"ALTER TABLE "{table}" ADD COLUMN "{field}""#), []).unwrap();
            self.properties.insert(at, field.to_owned());
        }
        field.to_owned()
    }
}

/// A data folder in the per-project layout: its index, and each project's directory.
pub struct OldFolder {
    pub data: PathBuf,
    projects: Vec<(Uuid, String)>,
}

impl OldFolder {
    pub fn new(root: &Path) -> OldFolder {
        let data = root.join("data");
        std::fs::create_dir_all(data.join("projects")).unwrap();
        OldFolder { data, projects: Vec::new() }
    }

    /// A project of the index, and its store.
    pub fn project(&mut self, name: &str) -> (Uuid, OldStoreFile) {
        let id = Uuid::new_v4();
        self.projects.push((id, name.to_owned()));
        let index: String = self
            .projects
            .iter()
            .map(|(id, name)| format!("[[project]]\nid = \"{id}\"\nname = \"{name}\"\n\n"))
            .collect();
        std::fs::write(self.data.join("projects.toml"), index).unwrap();
        (id, OldStoreFile::create(&self.data.join("projects").join(id.to_string()).join("project.sqlite")))
    }

    pub fn file(&self, relative: &str, content: &[u8]) {
        std::fs::write(self.data.join(relative), content).unwrap();
    }
}
