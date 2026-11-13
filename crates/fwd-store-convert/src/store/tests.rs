use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ankurah::core::property::backend::{LWWBackend, PropertyBackend, YrsBackend};
use ankurah::core::storage::StorageEngine;
use ankurah::proto::{EntityId, ModelId, OperationSet};
use ankurah::{PropertyId, Value};
use ankurah_storage_sqlite::SqliteStorageEngine;
use rusqlite::Connection;

use super::{convert, inventory, read_back, OldStore, StoreError};
use crate::f8e85d2_to_5d8a000b::old_shapes::{creation, entity, lww_buffer, update};
use crate::f8e85d2_to_5d8a000b::{self as hop, Converted, OldEntity, OldEvent};

/// The tables Ankurah f8e85d2's SQLite engine made, as it made them, and one materialization.
const OLD_SCHEMA: &str = r#"
    CREATE TABLE "_ankurah_meta" ("key" TEXT PRIMARY KEY, "value" TEXT);
    CREATE TABLE "_ankurah_entity" ("id" TEXT PRIMARY KEY, "state_buffer" BLOB NOT NULL, "head" TEXT NOT NULL, "attestations" BLOB NOT NULL);
    CREATE TABLE "_ankurah_event" ("id" TEXT PRIMARY KEY, "entity_id" TEXT NOT NULL, "body" BLOB NOT NULL, "parent" TEXT NOT NULL, "attestations" BLOB NOT NULL);
    CREATE INDEX "_ankurah_event_entity_id_idx" ON "_ankurah_event" ("entity_id");
    CREATE TABLE "_ankurah_entity_model" ("entity_id" TEXT NOT NULL, "model_key" BLOB NOT NULL, PRIMARY KEY ("entity_id", "model_key"));
    CREATE TABLE "_ankurah_sqlite_model_map" ("model_key" BLOB PRIMARY KEY, "materialization_table_name" TEXT NOT NULL UNIQUE);
    CREATE TABLE "_ankurah_sqlite_column_map"("model_key" BLOB NOT NULL, "property_key" TEXT NOT NULL, "column_name" TEXT NOT NULL,
        PRIMARY KEY ("model_key", "property_key"), UNIQUE ("model_key", "column_name"));
    CREATE TABLE "note"("id" TEXT PRIMARY KEY, "title" TEXT, "body" TEXT);
"#;

fn title() -> PropertyId {
    PropertyId::EntityId(EntityId::from_bytes([1; 32]))
}
fn body() -> PropertyId {
    PropertyId::EntityId(EntityId::from_bytes([2; 32]))
}
fn note() -> ModelId {
    ModelId::EntityId(EntityId::from_bytes([5; 32]))
}

fn writes(property: PropertyId, value: &str) -> OperationSet {
    let backend = LWWBackend::new();
    backend.set(property, Some(Value::String(value.into())));
    OperationSet::from_backends(BTreeMap::from([("lww".to_owned(), backend.to_operations().unwrap().unwrap())]))
}

/// Two notes as Ankurah f8e85d2 stored them: one created, retitled twice and given collaborative
/// text; one retitled on two branches that never merged. Both are members of the `note` model,
/// whose table and columns the old engine named.
pub(crate) fn write_old_store(path: &Path) -> (EntityId, EntityId) {
    let text = YrsBackend::new();
    text.insert(&body(), 0, "shared words").unwrap();
    let mut operations = writes(title(), "draft");
    operations.0.extend(
        OperationSet::from_backends(BTreeMap::from([("yrs".to_owned(), text.to_operations().unwrap().unwrap())])).0,
    );
    let (chained, genesis) = creation(1, operations);
    let (_, second) = update(chained, 2, writes(title(), "second"), &[&genesis]);
    let (_, third) = update(chained, 3, writes(title(), "third"), &[&second]);
    let chained_state = entity(
        chained,
        vec![
            ("lww", lww_buffer(vec![(title(), Value::String("third".into()), &third)])),
            ("yrs", text.to_state_buffer().unwrap()),
        ],
        &[&third],
        &[note()],
    );

    let (forked, root) = creation(4, writes(title(), "root"));
    let (_, left) = update(forked, 5, writes(title(), "left"), &[&root]);
    let (_, right) = update(forked, 6, writes(title(), "right"), &[&root]);
    let forked_state = entity(
        forked,
        vec![("lww", lww_buffer(vec![(title(), Value::String("right".into()), &right)]))],
        &[&left, &right],
        &[note()],
    );

    let db = Connection::open(path).unwrap();
    db.execute_batch(OLD_SCHEMA).unwrap();
    db.execute(r#"INSERT INTO "_ankurah_meta" VALUES ('protocol_version', '1')"#, []).unwrap();
    for event in [genesis, second, third, root, left, right] {
        insert_event(&db, &event);
    }
    for state in [chained_state, forked_state] {
        insert_entity(&db, &state);
    }
    let model_key = bincode::serialize(&note()).unwrap();
    db.execute(r#"INSERT INTO "_ankurah_sqlite_model_map" VALUES (?, 'note')"#, [&model_key]).unwrap();
    db.execute(
        r#"INSERT INTO "_ankurah_sqlite_column_map" VALUES (?, ?, 'id')"#,
        (&model_key, serde_json::to_string(&PropertyId::Id).unwrap()),
    )
    .unwrap();
    db.execute(
        r#"INSERT INTO "_ankurah_sqlite_column_map" VALUES (?, ?, 'title')"#,
        (&model_key, serde_json::to_string(&title()).unwrap()),
    )
    .unwrap();
    db.execute(
        r#"INSERT INTO "_ankurah_sqlite_column_map" VALUES (?, ?, 'body')"#,
        (&model_key, serde_json::to_string(&body()).unwrap()),
    )
    .unwrap();
    db.execute(
        r#"INSERT INTO "note" VALUES (?, 'third', 'shared words'), (?, 'right', NULL)"#,
        (chained.to_base64(), forked.to_base64()),
    )
    .unwrap();
    (chained, forked)
}

fn insert_event(db: &Connection, event: &OldEvent) {
    db.execute(
        r#"INSERT INTO "_ankurah_event" VALUES (?, ?, ?, ?, ?)"#,
        (&event.id, &event.entity_id, &event.body, &event.parent, &event.attestations),
    )
    .unwrap();
}

fn insert_entity(db: &Connection, entity: &OldEntity) {
    db.execute(
        r#"INSERT INTO "_ankurah_entity" VALUES (?, ?, ?, ?)"#,
        (&entity.id, &entity.state_buffer, &entity.head, &entity.attestations),
    )
    .unwrap();
    for key in &entity.memberships {
        db.execute(r#"INSERT INTO "_ankurah_entity_model" VALUES (?, ?)"#, (&entity.id, key)).unwrap();
    }
}

#[tokio::test]
async fn a_store_converts_into_one_the_new_engine_reads_under_the_names_the_old_one_gave() {
    let dir = tempfile::tempdir().unwrap();
    let (from, to) = (dir.path().join("old.sqlite"), dir.path().join("new.sqlite"));
    let (chained, forked) = write_old_store(&from);

    let report = convert(&from, &to).await.unwrap();
    assert_eq!((report.entities, report.events, report.ids_changed), (2, 6, 4));

    let engine = SqliteStorageEngine::open(&to).await.unwrap();
    let state = engine.get_state(chained).await.unwrap().payload.state;
    assert_eq!(state.head.max_generation(), Some(3));
    assert_eq!(
        YrsBackend::from_state_buffer(&state.state_buffers["yrs"]).unwrap().get_string(&body()).as_deref(),
        Some("shared words")
    );
    let forked_head = engine.get_state(forked).await.unwrap().payload.state.head;
    assert_eq!(forked_head.entries().iter().map(|(generation, _)| *generation).collect::<Vec<_>>(), vec![2, 2]);

    let db = Connection::open(&to).unwrap();
    let titles: Vec<(String, String)> = db
        .prepare(r#"SELECT "id", "title" FROM "note" ORDER BY "title""#)
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(titles, vec![(forked.to_base64(), "right".to_owned()), (chained.to_base64(), "third".to_owned())]);
    let version: String = db
        .query_row(r#"SELECT "value" FROM "_ankurah_meta" WHERE "key" = 'protocol_version'"#, [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, "1");
}

#[tokio::test]
async fn a_table_the_engine_did_not_register_is_refused_before_anything_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let (from, to) = (dir.path().join("old.sqlite"), dir.path().join("new.sqlite"));
    write_old_store(&from);
    Connection::open(&from).unwrap().execute(r#"CREATE TABLE "kept_by_hand" ("x" TEXT)"#, []).unwrap();

    let refusal = convert(&from, &to).await.err().unwrap();
    assert!(matches!(&refusal, StoreError::UnknownTable(table) if table == "kept_by_hand"), "{refusal}");
    assert!(!to.exists());
}

#[tokio::test]
async fn a_store_that_records_another_protocol_version_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (from, to) = (dir.path().join("old.sqlite"), dir.path().join("new.sqlite"));
    write_old_store(&from);
    Connection::open(&from).unwrap().execute(r#"UPDATE "_ankurah_meta" SET "value" = '2'"#, []).unwrap();

    let refusal = convert(&from, &to).await.err().unwrap();
    assert!(matches!(&refusal, StoreError::ProtocolVersion { found: Some(found), .. } if found == "2"), "{refusal}");
}

#[tokio::test]
async fn a_table_named_like_sqlites_own_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (from, to) = (dir.path().join("old.sqlite"), dir.path().join("new.sqlite"));
    write_old_store(&from);
    Connection::open(&from).unwrap().execute(r#"CREATE TABLE "sqlitebackup" ("x" TEXT)"#, []).unwrap();

    let refusal = convert(&from, &to).await.err().unwrap();
    assert!(matches!(&refusal, StoreError::UnknownTable(table) if table == "sqlitebackup"), "{refusal}");
}

/// The old store, the new store the conversion wrote from it, what was converted, and the two
/// notes' ids.
struct Written {
    from: PathBuf,
    to: PathBuf,
    converted: Converted,
    chained: EntityId,
    forked: EntityId,
}

async fn written(dir: &Path) -> Written {
    let (from, to) = (dir.join("old.sqlite"), dir.join("new.sqlite"));
    let (chained, forked) = write_old_store(&from);
    convert(&from, &to).await.unwrap();
    let old = OldStore::read(&from).unwrap();
    let converted = hop::convert(old.events, old.entities).unwrap();
    Written { from, to, converted, chained, forked }
}

#[tokio::test]
async fn the_judge_refuses_two_event_rows_whose_payloads_were_exchanged() {
    let dir = tempfile::tempdir().unwrap();
    let store = written(dir.path()).await;
    read_back::judge(&store.to, &store.converted).await.unwrap();

    // Two rows keep their keys and take each other's payloads.
    Connection::open(&store.to)
        .unwrap()
        .execute_batch(
            r#"CREATE TEMP TABLE "pair" AS SELECT * FROM "_ankurah_event" LIMIT 2;
               UPDATE "_ankurah_event" SET ("entity_id", "body", "parent", "attestations") =
                   (SELECT "entity_id", "body", "parent", "attestations" FROM "pair" WHERE "pair"."id" <> "_ankurah_event"."id")
               WHERE "id" IN (SELECT "id" FROM "pair");"#,
        )
        .unwrap();

    let refusal = read_back::judge(&store.to, &store.converted).await.err().unwrap();
    assert!(matches!(refusal, StoreError::ReadBack(_)), "{refusal}");
}

#[tokio::test]
async fn the_inventory_refuses_a_dropped_entity() {
    let dir = tempfile::tempdir().unwrap();
    let store = written(dir.path()).await;
    inventory::compare(&store.from, &store.to).unwrap();

    let db = Connection::open(&store.to).unwrap();
    for table in ["_ankurah_entity", "note"] {
        db.execute(&format!(r#"DELETE FROM "{table}" WHERE "id" = ?"#), [store.forked.to_base64()]).unwrap();
    }
    assert!(inventory::compare(&store.from, &store.to).is_err());
}

#[tokio::test]
async fn the_inventory_refuses_a_changed_value() {
    let dir = tempfile::tempdir().unwrap();
    let store = written(dir.path()).await;
    Connection::open(&store.to)
        .unwrap()
        .execute(r#"UPDATE "note" SET "title" = 'changed' WHERE "id" = ?"#, [store.chained.to_base64()])
        .unwrap();
    assert!(inventory::compare(&store.from, &store.to).is_err());
}

#[tokio::test]
async fn the_inventory_refuses_rows_in_another_order() {
    let dir = tempfile::tempdir().unwrap();
    let store = written(dir.path()).await;
    // Replacing a row moves it to the end of its table, as the engine's own writes do.
    Connection::open(&store.to)
        .unwrap()
        .execute(r#"INSERT OR REPLACE INTO "note" SELECT * FROM "note" ORDER BY rowid LIMIT 1"#, [])
        .unwrap();
    let refusal = inventory::compare(&store.from, &store.to).err().unwrap();
    assert!(refusal.contains(r#""note""#), "{refusal}");
}
