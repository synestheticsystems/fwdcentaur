use std::path::PathBuf;

use rusqlite::Connection;
use serde_json::json;
use uuid::Uuid;

use super::judge;
use crate::fixture::{id, text, OldFolder, V};
use crate::merge::{write, Moving};
use crate::old_store::OldStore;
use crate::tests::{now, principal, task};

/// Writes the folder's one project into a store beside the folder, and gives that store and the
/// project's old one.
async fn merged(folder: &OldFolder, uuid: Uuid) -> (PathBuf, PathBuf) {
    let old = folder.data.join(format!("projects/{uuid}/project.sqlite"));
    let new = folder.data.with_file_name("store.sqlite");
    write(&new, &[Moving { uuid, name: "alpha".into(), old: OldStore::read(&old).unwrap() }]).await.unwrap();
    (new, old)
}

/// `Principal.serves` is JSON kept as text, and the move carries text in it that is not JSON as it
/// was, so the judge holds that text to its bytes: two different such values are not the same.
#[tokio::test]
async fn a_serves_value_that_is_not_json_is_judged_byte_for_byte() {
    let root = tempfile::tempdir().unwrap();
    let mut folder = OldFolder::new(root.path());
    let (uuid, mut store) = folder.project("alpha");
    let mut serving = principal("triage-bot", "agent", Some("triage"));
    serving.push(("serves", text("legacy")));
    store.record("Principal", serving);
    let (new, old) = merged(&folder, uuid).await;
    let old = [(uuid, old.as_path())];
    judge(&new, &old).unwrap();

    Connection::open(&new).unwrap().execute("UPDATE principal SET serves = 'different'", []).unwrap();
    assert!(judge(&new, &old).unwrap_err().contains("serves differs from the old value"));
}

/// JSON with leading whitespace is JSON to the writer, which re-points the scope id in it; the judge
/// maps it the same way and passes the store.
#[tokio::test]
async fn a_serves_value_of_json_with_leading_whitespace_is_mapped_as_the_writer_maps_it() {
    let root = tempfile::tempdir().unwrap();
    let mut folder = OldFolder::new(root.path());
    let (uuid, mut store) = folder.project("alpha");
    let me = store.record("Principal", principal("dnorman", "human", None));
    let scope = store.record(
        "Scope",
        vec![
            ("status", text("active")),
            ("title", text("work")),
            ("intention", text("to work")),
            ("context", V::Json(json!({}))),
            ("created_by", id(&me)),
            ("created", V::Int(1)),
        ],
    );
    let mut serving = principal("triage-bot", "agent", Some("triage"));
    serving.push(("serves", V::Text(format!(" {}", json!({"scopes": [scope]})))));
    let triage = store.record("Principal", serving);
    let (new, old) = merged(&folder, uuid).await;

    judge(&new, &[(uuid, old.as_path())]).unwrap();
    let db = Connection::open(&new).unwrap();
    let serves: String =
        db.query_row("SELECT serves FROM principal WHERE id = ?", [now(&db, &triage)], |row| row.get(0)).unwrap();
    assert_eq!(serde_json::from_str::<serde_json::Value>(&serves).unwrap(), json!({"scopes": [now(&db, &scope)]}));
}

/// A task's description and reasons, written and then cleared in the old store, which the old
/// engine materializes as the empty string; the new engine materializes no value for an empty text.
/// One cleared text sits in a column beside a text that was kept, and the other has no column in the
/// new store, so the judge meets both: a NULL against "", and a missing value against "".
#[tokio::test]
async fn text_written_and_then_cleared_merges_and_passes_the_judge() {
    let root = tempfile::tempdir().unwrap();
    let mut folder = OldFolder::new(root.path());
    let (uuid, mut store) = folder.project("alpha");
    let me = store.record("Principal", principal("dnorman", "human", None));
    let mut cleared = task("cleared", &me, json!({}));
    cleared.extend([("what", text("")), ("why", text(""))]);
    let cleared = store.record("Task", cleared);
    let mut kept = task("kept", &me, json!({}));
    kept.push(("what", text("typed")));
    store.record("Task", kept);
    let (new, old) = merged(&folder, uuid).await;

    judge(&new, &[(uuid, old.as_path())]).unwrap();
    let db = Connection::open(&new).unwrap();
    let what: Option<String> =
        db.query_row("SELECT what FROM task WHERE id = ?", [now(&db, &cleared)], |row| row.get(0)).unwrap();
    assert_eq!(what, None, "the cleared text beside a kept one");
    let why: i64 = db
        .query_row("SELECT count(*) FROM pragma_table_info('task') WHERE name = 'why'", [], |row| row.get(0))
        .unwrap();
    assert_eq!(why, 0, "the cleared text no record of the new store holds");
}
