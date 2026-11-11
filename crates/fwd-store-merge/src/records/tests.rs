use rusqlite::Connection;
use serde_json::json;

use crate::fixture::{id, new_id, text, OldFolder, V};
use crate::judge::judge;
use crate::tests::{merged, message, now, principal, task};

/// Under the whole-string rule a string in any JSON field that is the whole id of a record of the
/// project names that record's new id, whatever the field means: here a scope's branch named by a
/// task's id, and that task's label named by a message's id, each named before it is made.
#[tokio::test]
async fn a_json_field_naming_a_record_made_after_its_own_is_repointed() {
    let root = tempfile::tempdir().unwrap();
    let mut folder = OldFolder::new(root.path());
    let (uuid, mut store) = folder.project("alpha");
    let [later_task, later_message] = [new_id(), new_id()];
    let me = store.record("Principal", principal("dnorman", "human", None));
    let scope = store.record(
        "Scope",
        vec![
            ("status", text("active")),
            ("title", text("work")),
            ("intention", text("to work")),
            ("context", V::Json(json!({"branches": [later_task]}))),
            ("created_by", id(&me)),
            ("created", V::Int(1)),
        ],
    );
    let mut labelled = task("labelled", &me, json!({}));
    labelled.retain(|(field, _)| *field != "labels");
    labelled.push(("labels", V::Json(json!([later_message]))));
    store.record_as("Task", &later_task, labelled);
    store.record_as("Message", &later_message, message("later", &me, None, json!({})));
    let (new, old) = merged(&folder, uuid).await;

    judge(&new, &[(uuid, old.as_path())]).unwrap();
    let db = Connection::open(&new).unwrap();
    let json = |sql: &str, id: &str| -> serde_json::Value {
        serde_json::from_str(&db.query_row(sql, [id], |row| row.get::<_, String>(0)).unwrap()).unwrap()
    };
    assert_eq!(
        json("SELECT json(context) FROM scope WHERE id = ?", &now(&db, &scope)),
        json!({"branches": [now(&db, &later_task)]})
    );
    assert_eq!(
        json("SELECT json(labels) FROM task WHERE id = ?", &now(&db, &later_task)),
        json!([now(&db, &later_message)])
    );
}
