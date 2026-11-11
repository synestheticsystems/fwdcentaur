use std::collections::HashMap;
use std::path::PathBuf;

use fwd_core::anchoring::anchor_key;
use rusqlite::Connection;
use serde_json::json;
use uuid::Uuid;

use super::{run, Args};
use crate::fixture::{id, new_id, text, OldFolder, V};
use crate::judge::judge;
use crate::merge::{write, Moving};
use crate::old_store::OldStore;

/// A project of the fixture, with the ids its records have in its old store, by name.
struct Project {
    uuid: Uuid,
    ids: HashMap<&'static str, String>,
}

impl Project {
    fn store(&self, folder: &OldFolder) -> PathBuf {
        folder.data.join(format!("projects/{}/project.sqlite", self.uuid))
    }
}

/// A project with one of each kind of reference the move re-points: a reply made before the
/// message it replies to, a subtask made before its parent, a relation on a relation, a decision
/// closed by a later message, an anchor keyed by its repository, a triage agent serving a scope, a
/// notification about a task, and a relation whose end names nothing. 15 records.
fn project(folder: &mut OldFolder, name: &str) -> Project {
    let (uuid, mut store) = folder.project(name);
    let [reply, first, parent, closing, gone] = [new_id(), new_id(), new_id(), new_id(), new_id()];
    let me = store.record("Principal", principal("dnorman", "human", None));
    let scope = store.record(
        "Scope",
        vec![
            ("status", text("active")),
            ("title", text("work")),
            ("intention", text("to work")),
            ("context", V::Json(json!({"repo": "github.com/x/y", "branches": ["main"]}))),
            ("created_by", id(&me)),
            ("created", V::Int(1)),
        ],
    );
    let mut serving = principal("triage-bot", "agent", Some("triage"));
    serving.push(("serves", V::Text(json!({"scopes": [scope]}).to_string())));
    let triage = store.record("Principal", serving);
    let repo = store.record(
        "Repo",
        vec![
            ("name", text("y")),
            ("remote_urls", V::Json(json!(["git@github.com:x/y.git"]))),
            ("default_branch", text("main")),
        ],
    );
    let child =
        store.record("Task", task("child", &me, json!({"parent_id": parent, "assigned_by": me, "scope_id": scope})));
    store.record_as("Task", &parent, task("parent", &me, json!({})));
    let anchor = store.record(
        "CodeAnchor",
        vec![
            ("repo", id(&repo)),
            ("path", text("src/lib.rs")),
            ("anchor_key", V::Text(anchor_key(&repo, "src/lib.rs", "blob", Some((1, 3))))),
            ("basis_blob", text("blob")),
            ("side", text("worktree")),
            ("start_line", V::Int(1)),
            ("end_line", V::Int(3)),
            ("anchored_text", text("fn x() {}")),
            ("context_before", text("")),
            ("context_after", text("")),
            ("pieces", V::Json(json!([]))),
            ("created_by", id(&me)),
            ("created", V::Int(2)),
        ],
    );
    let decision = store.record(
        "Decision",
        vec![
            ("number", V::Int(7)),
            ("title", text("a ruling")),
            ("status", text("repealed")),
            ("context", text("why")),
            ("decision", text("what")),
            ("consequences", text("so")),
            ("author", id(&me)),
            ("created", V::Int(3)),
            ("closed_reason_message", id(&closing)),
        ],
    );
    store.record_as("Message", &reply, message("a reply", &me, Some(&first), json!({"written_in": scope})));
    store.record_as("Message", &first, message(&format!("about [[{child}]]"), &me, None, json!({})));
    store.record_as("Message", &closing, message("repealed", &me, None, json!({})));
    let link = store.record("Relation", relation("depends_on", &child, &parent, &me));
    store.record("Relation", relation("relates_to", &link, &child, &me));
    store.record("Relation", relation("relates_to", &child, &gone, &me));
    store.record(
        "Notification",
        vec![
            ("recipient", id(&me)),
            ("notification_type", text("task_commented")),
            ("status", text("unread")),
            ("priority", text("next")),
            ("created", V::Int(4)),
            ("target", V::Json(json!({"type": "Task", "id": child}))),
            (
                "originator",
                V::Json(json!({"message_id": first, "actor_id": me, "batch": {"items": [{"message_id": reply}]}})),
            ),
        ],
    );
    let checkouts = json!([{"root": "/x", "upstream": null, "branch": "main", "head": null, "scope": scope, "clear_scope": false, "seen": 1}]);
    folder.file(&format!("projects/{uuid}/checkouts.json"), checkouts.to_string().as_bytes());
    folder.file(&format!("projects/{uuid}/anchors.sqlite"), b"a cache");
    let ids = HashMap::from([
        ("me", me),
        ("triage", triage),
        ("scope", scope),
        ("repo", repo),
        ("child", child),
        ("parent", parent),
        ("anchor", anchor),
        ("decision", decision),
        ("reply", reply),
        ("first", first),
        ("closing", closing),
        ("gone", gone),
    ]);
    Project { uuid, ids }
}

pub(crate) fn principal(handle: &str, kind: &str, role: Option<&str>) -> Vec<(&'static str, V)> {
    let mut fields = vec![
        ("kind", text(kind)),
        ("handle", text(handle)),
        ("display_name", text(handle)),
        ("active", V::Bool(true)),
        ("member", V::Bool(true)),
    ];
    if let Some(role) = role {
        fields.push(("agent_role", text(role)));
    }
    fields
}

pub(crate) fn task(title: &str, by: &str, context: serde_json::Value) -> Vec<(&'static str, V)> {
    vec![
        ("title", text(title)),
        ("status", text("open")),
        ("needs_review", V::Bool(false)),
        ("labels", V::Json(json!([]))),
        ("context", V::Json(context)),
        ("created_by", id(by)),
        ("created", V::Int(5)),
        ("updated", V::Int(6)),
    ]
}

pub(crate) fn message(
    content: &str,
    by: &str,
    reply_to: Option<&str>,
    context: serde_json::Value,
) -> Vec<(&'static str, V)> {
    let mut fields = vec![
        ("author", id(by)),
        ("created", V::Int(7)),
        ("content", text(content)),
        ("priority", text("next")),
        ("status", text("active")),
        ("context", V::Json(context)),
    ];
    if let Some(parent) = reply_to {
        fields.push(("reply_to", id(parent)));
        fields.push(("thread", id(parent)));
    }
    fields
}

fn relation(kind: &str, from: &str, to: &str, by: &str) -> Vec<(&'static str, V)> {
    vec![
        ("kind", text(kind)),
        ("from_id", id(from)),
        ("to_id", id(to)),
        ("status", text("accepted")),
        ("created_by", id(by)),
        ("created", V::Int(8)),
        ("via", V::Json(json!({"actor_id": by}))),
    ]
}

/// Writes the folder's one project into a store beside the folder, and gives that store and the
/// project's old one.
pub(crate) async fn merged(folder: &OldFolder, uuid: Uuid) -> (PathBuf, PathBuf) {
    let old = folder.data.join(format!("projects/{uuid}/project.sqlite"));
    let new = folder.data.with_file_name("store.sqlite");
    write(&new, &[Moving { uuid, name: "alpha".into(), old: OldStore::read(&old).unwrap() }]).await.unwrap();
    (new, old)
}

/// The id the record that had `old` has in the merged store, through its former ids.
pub(crate) fn now(db: &Connection, old: &str) -> String {
    db.query_row("SELECT record FROM formerid WHERE former = ?", [old], |row| row.get(0)).unwrap()
}

fn one(db: &Connection, sql: &str, id: &str) -> Option<String> {
    db.query_row(sql, [id], |row| row.get(0)).unwrap()
}

#[tokio::test]
async fn two_projects_move_into_one_store_with_every_reference_on_the_new_ids() {
    let root = tempfile::tempdir().unwrap();
    let mut folder = OldFolder::new(root.path());
    let alpha = project(&mut folder, "alpha");
    let beta = project(&mut folder, "beta");
    let to = root.path().join("merged");

    run(&Args { from: folder.data.clone(), to: to.clone() }).await.unwrap();

    let db = Connection::open(to.join("store.sqlite")).unwrap();
    for p in [&alpha, &beta] {
        let project: String =
            db.query_row("SELECT id FROM project WHERE uuid = ?", [p.uuid.to_string()], |r| r.get(0)).unwrap();
        let child = now(&db, &p.ids["child"]);
        assert_eq!(one(&db, "SELECT project FROM task WHERE id = ?", &child), Some(project));
        let context: serde_json::Value =
            serde_json::from_str(&one(&db, "SELECT json(context) FROM task WHERE id = ?", &child).unwrap()).unwrap();
        assert_eq!(context["parent_id"], json!(now(&db, &p.ids["parent"])), "a parent made after its child");
        assert_eq!(context["scope_id"], json!(now(&db, &p.ids["scope"])));
        let reply = now(&db, &p.ids["reply"]);
        assert_eq!(
            one(&db, "SELECT reply_to FROM message WHERE id = ?", &reply),
            Some(now(&db, &p.ids["first"])),
            "a reply made first"
        );
        let decision = now(&db, &p.ids["decision"]);
        assert_eq!(
            one(&db, "SELECT closed_reason_message FROM decision WHERE id = ?", &decision),
            Some(now(&db, &p.ids["closing"]))
        );
        let serves = one(&db, "SELECT serves FROM principal WHERE id = ?", &now(&db, &p.ids["triage"])).unwrap();
        assert!(serves.contains(&now(&db, &p.ids["scope"])), "the scopes a triage agent serves");
        assert_eq!(
            one(&db, "SELECT content FROM message WHERE id = ?", &now(&db, &p.ids["first"])),
            Some(format!("about [[{}]]", p.ids["child"])),
            "text is carried as typed"
        );
        let carried: i64 =
            db.query_row("SELECT count(*) FROM relation WHERE to_id = ?", [&p.ids["gone"]], |r| r.get(0)).unwrap();
        assert_eq!(carried, 1, "an end that names nothing is carried as it was");
        assert_eq!(
            one(&db, "SELECT anchor_key FROM codeanchor WHERE id = ?", &now(&db, &p.ids["anchor"])),
            Some(anchor_key(&now(&db, &p.ids["repo"]), "src/lib.rs", "blob", Some((1, 3)))),
            "the key is made again from the repository's new id"
        );
        let checkouts: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(to.join(format!("projects/{}/checkouts.json", p.uuid))).unwrap(),
        )
        .unwrap();
        assert_eq!(checkouts[0]["scope"], json!(now(&db, &p.ids["scope"])), "the checkout's scope binding");
        assert!(!to.join(format!("projects/{}/project.sqlite", p.uuid)).exists());
        assert!(!to.join(format!("projects/{}/anchors.sqlite", p.uuid)).exists(), "the anchors' cache is left behind");
    }
    let formers: i64 = db.query_row("SELECT count(*) FROM formerid", [], |r| r.get(0)).unwrap();
    assert_eq!(formers, 2 * 15, "one former id per old record");
}

#[tokio::test]
async fn the_judge_refuses_a_changed_value_and_a_lost_former_id() {
    let root = tempfile::tempdir().unwrap();
    let mut folder = OldFolder::new(root.path());
    let alpha = project(&mut folder, "alpha");
    let path = alpha.store(&folder);
    let store = root.path().join("store.sqlite");
    write(&store, &[Moving { uuid: alpha.uuid, name: "alpha".into(), old: OldStore::read(&path).unwrap() }])
        .await
        .unwrap();
    let old = [(alpha.uuid, path.as_path())];
    judge(&store, &old).unwrap();

    let db = Connection::open(&store).unwrap();
    db.execute("UPDATE task SET title = 'changed' WHERE title = 'parent'", []).unwrap();
    assert!(judge(&store, &old).unwrap_err().contains("title"));
    db.execute("UPDATE task SET title = 'parent' WHERE title = 'changed'", []).unwrap();
    db.execute("DELETE FROM formerid WHERE rowid = (SELECT min(rowid) FROM formerid)", []).unwrap();
    assert!(judge(&store, &old).is_err());
}

#[tokio::test]
async fn a_record_the_current_models_cannot_represent_is_refused_and_nothing_is_written() {
    let property = new_id();
    for spoil in [
        r#"UPDATE "principal" SET "agent_role" = 'supervisor'"#.to_owned(),
        r#"UPDATE "task" SET "title" = NULL"#.to_owned(),
        format!(
            r#"INSERT INTO "_ankurah_property" VALUES ('{property}', 'owner', NULL, 'lww', 'entityid');
               INSERT INTO "_ankurah_sqlite_column_map" SELECT "model_key", '{{"EntityId":"{property}"}}', 'owner'
                   FROM "_ankurah_sqlite_model_map" WHERE "materialization_table_name" = 'scope';
               ALTER TABLE "scope" ADD COLUMN "owner";
               UPDATE "scope" SET "owner" = "created_by";"#
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let mut folder = OldFolder::new(root.path());
        let alpha = project(&mut folder, "alpha");
        Connection::open(alpha.store(&folder)).unwrap().execute_batch(&spoil).unwrap();
        let to = root.path().join("merged");
        assert!(run(&Args { from: folder.data.clone(), to: to.clone() }).await.is_err(), "{spoil}");
        assert!(!to.exists(), "{spoil}");
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1, "no partial folder is left: {spoil}");
    }
}
