//! The judge of a merged store, which rests on nothing the move computed: the old stores and the
//! new one, read with plain SQLite, the old ids mapped through the new store's own `FormerId`
//! records and its references found through its own catalog. For every project and model: the same
//! number of records; every old id with exactly one former id, naming a record of the same model
//! and project; every reference and every id in a JSON value naming a record of the same project,
//! or carried unchanged from an old value that named nothing; every other value as it was; and the
//! records in the order they sat.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use fwd_core::anchoring::anchor_key;
use rusqlite::types::Value as Sql;
use rusqlite::{Connection, OpenFlags};
use uuid::Uuid;

use crate::old_store::{all_model_tables, open_immutable, ModelTable};
use crate::records::{id_shaped, MODELS};

/// One model's rows: by id, each column's value by property name; and the ids in row order.
struct Rows {
    by_id: HashMap<String, BTreeMap<String, Sql>>,
    order: Vec<String>,
}

fn rows(db: &Connection, table: &ModelTable, filter: Option<(&str, &str)>) -> rusqlite::Result<Rows> {
    let columns: Vec<String> = table
        .columns
        .iter()
        .map(|(column, _, value_type)| {
            if value_type == "json" {
                format!(r#"CASE typeof("{column}") WHEN 'blob' THEN json("{column}") ELSE "{column}" END"#)
            } else {
                format!(r#""{column}""#)
            }
        })
        .collect();
    let mut sql = format!(
        r#"SELECT "id"{} FROM "{}""#,
        columns.iter().map(|c| format!(", {c}")).collect::<String>(),
        table.table
    );
    let column_of = |property: &str| table.columns.iter().find(|(_, p, _)| p == property).map(|(c, _, _)| c.clone());
    let mut params = Vec::new();
    if let Some((property, value)) = filter {
        match column_of(property) {
            Some(column) => {
                sql.push_str(&format!(r#" WHERE "{column}" = ?"#));
                params.push(value.to_owned());
            }
            None => sql.push_str(" WHERE 0"),
        }
    }
    sql.push_str(" ORDER BY rowid");
    let mut statement = db.prepare(&sql)?;
    let mut cursor = statement.query(rusqlite::params_from_iter(params))?;
    let mut by_id = HashMap::new();
    let mut order = Vec::new();
    while let Some(row) = cursor.next()? {
        let id: String = row.get(0)?;
        let mut values = BTreeMap::new();
        for (i, (_, property, _)) in table.columns.iter().enumerate() {
            values.insert(property.clone(), row.get::<_, Sql>(i + 1)?);
        }
        order.push(id.clone());
        by_id.insert(id, values);
    }
    Ok(Rows { by_id, order })
}

fn by_model(tables: Vec<ModelTable>) -> HashMap<String, ModelTable> {
    tables.into_iter().map(|table| (table.model.clone(), table)).collect()
}

/// Judges the new store at `new` against each project's old store, `(uuid, path)`. Says what
/// differs, by project, model, record and field, never by value.
pub fn judge(new: &Path, old: &[(Uuid, &Path)]) -> Result<(), String> {
    let fail = |e: rusqlite::Error| e.to_string();
    let db = Connection::open_with_flags(new, OpenFlags::SQLITE_OPEN_READ_WRITE).map_err(fail)?;
    let new_tables = by_model(all_model_tables(&db).map_err(|e| e.to_string())?);
    let table = |model: &str| new_tables.get(model).ok_or_else(|| format!("the new store has no table for {model}"));
    let projects = rows(&db, table("Project")?, None).map_err(fail)?;
    let entity_order: HashMap<String, usize> = {
        let ids: Vec<String> = db
            .prepare(r#"SELECT "id" FROM "_ankurah_entity" ORDER BY rowid"#)
            .map_err(fail)?
            .query_map([], |r| r.get(0))
            .map_err(fail)?
            .collect::<Result<_, _>>()
            .map_err(fail)?;
        ids.into_iter().enumerate().map(|(i, id)| (id, i)).collect()
    };
    // Which properties of the new store are references, from its own catalog.
    let references: BTreeSet<(String, String)> = new_tables
        .values()
        .flat_map(|t| {
            t.columns
                .iter()
                .filter(|(_, _, value_type)| value_type == "entityid")
                .map(|(_, p, _)| (t.model.clone(), p.clone()))
        })
        .collect();

    for (uuid, old_path) in old {
        let at = |what: String| format!("project {uuid}: {what}");
        let project = projects
            .order
            .iter()
            .filter(|id| projects.by_id[*id].get("uuid") == Some(&Sql::Text(uuid.to_string())))
            .collect::<Vec<_>>();
        let [project] = project.as_slice() else {
            return Err(at(format!("{} project records name it", project.len())));
        };
        let in_project = Some(("project", project.as_str()));

        let old_db = open_immutable(old_path).map_err(fail)?;
        let old_tables = by_model(all_model_tables(&old_db).map_err(|e| e.to_string())?);
        let former = rows(&db, table("FormerId")?, in_project).map_err(fail)?;
        let mut now_of: HashMap<String, String> = HashMap::new();
        for values in former.by_id.values() {
            let (Some(Sql::Text(old_id)), Some(Sql::Text(record))) = (values.get("former"), values.get("record"))
            else {
                return Err(at("a former id lacks its id or its record".into()));
            };
            if now_of.insert(old_id.clone(), record.clone()).is_some() {
                return Err(at(format!("old id {old_id} has several former ids")));
            }
        }
        // Every record of the project, new id → model.
        let mut model_of: HashMap<String, &str> = HashMap::new();
        let mut new_rows: HashMap<&str, Rows> = HashMap::new();
        for model in MODELS {
            let found = match new_tables.get(model) {
                Some(t) => rows(&db, t, in_project).map_err(fail)?,
                None => Rows { by_id: HashMap::new(), order: Vec::new() },
            };
            for id in &found.order {
                model_of.insert(id.clone(), model);
            }
            new_rows.insert(model, found);
        }
        let mut old_count = 0;
        for model in MODELS {
            let old_rows = match old_tables.get(model) {
                Some(t) => rows(&old_db, t, None).map_err(fail)?,
                None => Rows { by_id: HashMap::new(), order: Vec::new() },
            };
            let new = &new_rows[model];
            old_count += old_rows.order.len();
            if old_rows.order.len() != new.order.len() {
                return Err(at(format!("{model}: {} records became {}", old_rows.order.len(), new.order.len())));
            }
            // Each old record's former id names a record of the same model and project, and the
            // records sit in the order they sat.
            let mapped: Vec<String> = old_rows
                .order
                .iter()
                .map(|old_id| {
                    now_of.get(old_id).cloned().ok_or_else(|| at(format!("{model} {old_id} has no former id")))
                })
                .collect::<Result<_, _>>()?;
            if mapped != new.order {
                let same = mapped.iter().collect::<BTreeSet<_>>() == new.order.iter().collect::<BTreeSet<_>>();
                return Err(at(if same {
                    format!("{model}: the records sit in another order")
                } else {
                    format!("{model}: a former id names a record of another model or project")
                }));
            }
            let old_entities: Vec<&String> = {
                let old_order: HashMap<String, usize> = old_db
                    .prepare(r#"SELECT "id" FROM "_ankurah_entity" ORDER BY rowid"#)
                    .map_err(fail)?
                    .query_map([], |r| r.get::<_, String>(0))
                    .map_err(fail)?
                    .enumerate()
                    .filter_map(|(i, id)| id.ok().map(|id| (id, i)))
                    .collect();
                let mut ids: Vec<&String> = old_rows.order.iter().collect();
                ids.sort_by_key(|id| old_order.get(*id));
                ids
            };
            let mut new_entities: Vec<&String> = new.order.iter().collect();
            new_entities.sort_by_key(|id| entity_order.get(*id));
            if old_entities.iter().map(|id| &now_of[*id]).ne(new_entities.iter().copied()) {
                return Err(at(format!("{model}: the records were made in another order")));
            }
            for old_id in &old_rows.order {
                compare(
                    model,
                    old_id,
                    &old_rows.by_id[old_id],
                    &new.by_id[&now_of[old_id]],
                    &now_of,
                    &model_of,
                    &references,
                    project,
                )
                .map_err(at)?;
            }
        }
        if now_of.len() != old_count {
            return Err(at(format!("{} former ids for {old_count} old records", now_of.len())));
        }
    }
    Ok(())
}

/// One record's values, old against new.
#[allow(clippy::too_many_arguments)]
fn compare(
    model: &str,
    old_id: &str,
    old: &BTreeMap<String, Sql>,
    new: &BTreeMap<String, Sql>,
    now_of: &HashMap<String, String>,
    model_of: &HashMap<String, &str>,
    references: &BTreeSet<(String, String)>,
    project: &str,
) -> Result<(), String> {
    let at = |field: &str, what: &str| format!("{model} {old_id}'s {field} {what}");
    if new.get("project") != Some(&Sql::Text(project.to_owned())) {
        return Err(at("project", "is not the project's record"));
    }
    let resolves = |id: &str| model_of.contains_key(id);
    for (field, value) in new.iter().filter(|(f, _)| f.as_str() != "project") {
        let was = old.get(field).unwrap_or(&Sql::Null);
        let reference = references.contains(&(model.to_owned(), field.clone()));
        let expected = match was {
            Sql::Text(text) if reference => match now_of.get(text) {
                Some(new_id) => Sql::Text(new_id.clone()),
                None => was.clone(),
            },
            Sql::Text(text) if field == "anchor_key" => {
                let text_of = |f: &str| match new.get(f) {
                    Some(Sql::Text(t)) => t.clone(),
                    _ => String::new(),
                };
                let int_of = |f: &str| match new.get(f) {
                    Some(Sql::Integer(n)) => *n,
                    _ => 0,
                };
                let lines = if new.get("whole_file") == Some(&Sql::Integer(1)) {
                    None
                } else {
                    Some((int_of("start_line"), int_of("end_line")))
                };
                let old_repo = match old.get("repo") {
                    Some(Sql::Text(t)) => t.clone(),
                    _ => String::new(),
                };
                if anchor_key(&old_repo, &text_of("path"), &text_of("basis_blob"), lines) != *text {
                    return Err(at(field, "was not the key of its old repository"));
                }
                Sql::Text(anchor_key(&text_of("repo"), &text_of("path"), &text_of("basis_blob"), lines))
            }
            Sql::Text(text) if text.starts_with(['{', '[']) && model_json(model, field) => {
                let old_json: serde_json::Value = serde_json::from_str(text).map_err(|e| at(field, &e.to_string()))?;
                Sql::Text(mapped_json(&old_json, now_of).to_string())
            }
            other => other.clone(),
        };
        let same = match (&expected, value) {
            (Sql::Text(a), Sql::Text(b)) if model_json(model, field) => {
                serde_json::from_str::<serde_json::Value>(a).ok() == serde_json::from_str::<serde_json::Value>(b).ok()
            }
            (a, b) => a == b,
        };
        if !same {
            return Err(at(field, "differs from the old value"));
        }
        // Every id in it names a record of the project, or was carried from an old value that named nothing.
        let mut ids = Vec::new();
        match value {
            Sql::Text(text) if reference => ids.push(text.clone()),
            Sql::Text(text) if model_json(model, field) => {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(text) {
                    collect_ids(&json, &mut ids);
                }
            }
            _ => {}
        }
        for id in ids {
            if !resolves(&id) && now_of.contains_key(&id) {
                return Err(at(field, "still holds an old id"));
            }
        }
    }
    for (field, value) in old {
        if *value != Sql::Null && !new.contains_key(field) {
            return Err(at(field, "has a value the new record lost"));
        }
    }
    Ok(())
}

/// Whether a model's property holds JSON: its JSON fields, and `serves`, JSON kept as text.
fn model_json(model: &str, field: &str) -> bool {
    matches!(
        (model, field),
        ("Task", "context" | "labels")
            | ("Repo", "remote_urls")
            | ("CodeAnchor", "pieces")
            | ("Message", "context")
            | ("Relation", "via")
            | ("Notification", "target" | "originator")
            | ("Scope", "context")
            | ("Principal", "serves")
    )
}

fn mapped_json(value: &serde_json::Value, now_of: &HashMap<String, String>) -> serde_json::Value {
    use serde_json::Value::*;
    match value {
        String(text) => now_of.get(text).map(|n| String(n.clone())).unwrap_or_else(|| value.clone()),
        Array(items) => Array(items.iter().map(|i| mapped_json(i, now_of)).collect()),
        Object(map) => Object(map.iter().map(|(k, v)| (k.clone(), mapped_json(v, now_of))).collect()),
        _ => value.clone(),
    }
}

fn collect_ids(value: &serde_json::Value, out: &mut Vec<String>) {
    use serde_json::Value::*;
    match value {
        String(text) if id_shaped(text) => out.push(text.clone()),
        Array(items) => items.iter().for_each(|i| collect_ids(i, out)),
        Object(map) => map.values().for_each(|v| collect_ids(v, out)),
        _ => {}
    }
}
