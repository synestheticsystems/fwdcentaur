//! What `fwd task move` answers (`fwd_core::task_move`), so that whoever moved tasks sees, per
//! task, what the run did: moved it, with how much of what belongs to it, from which project and
//! off which scopes, or found it there already, keeping its scopes.

use crate::{App, Format};
use ankurah::EntityId;
use anyhow::Result;
use fwd_core::model::TaskView;
use fwd_core::ops;
use fwd_core::project_graph::{Followed, ProjectGraph};
use fwd_core::task_move::{Destination, Moved};
use serde_json::{json, Value};

/// Prints what the move did, as JSON ([`json`]) or for people ([`text`]), and beside it, for each
/// task there already that keeps its scopes without the one the move names, that the move did not
/// put it there and which commands change a task's scopes.
pub(crate) async fn print(app: &App, to: &Destination, moved: &[Moved]) -> Result<()> {
    let answer = json(app.graph(), to, moved).await?;
    if let Some((_, title)) = &to.scope {
        for (m, t) in moved.iter().zip(answer["tasks"].as_array().into_iter().flatten()) {
            if m.kept_scopes {
                app.note(kept_scopes(t, &to.project.name, title));
            }
        }
    }
    match app.format {
        Format::Text => {
            println!("{}", text(&answer));
            Ok(())
        }
        _ => app.print(answer),
    }
}

/// The project the tasks went to and its scope, and for each task its id and title, the project it
/// was in, whether this run moved it or found it there already, how many of each kind of what
/// belongs to it the run took in, the scopes it was taken off and those it is worked in, each named
/// as `task show` names a scope, whether it kept its scopes without the one the move names, and the
/// comment that records the move.
async fn json(graph: &ProjectGraph, to: &Destination, moved: &[Moved]) -> Result<Value> {
    let scope = to.scope.as_ref().map(|(id, title)| json!({"id": id.to_base64(), "title": title}));
    let mut tasks = Vec::with_capacity(moved.len());
    for m in moved {
        let title = match graph.follow::<TaskView>(m.task).await? {
            Followed::Held { found, .. } => json!(found.title()?),
            Followed::NotHeld => Value::Null,
        };
        let brought = m.brought;
        tasks.push(json!({
            "id": m.task.to_base64(),
            "title": title,
            "from": m.from.name,
            "moved": m.moved,
            "brought": {
                "messages": brought.messages,
                "relations": brought.relations,
                "former_ids": brought.former_ids,
            },
            "taken_off": scopes(graph, &m.taken_off).await?,
            "worked_in": scopes(graph, &m.worked_in).await?,
            "kept_scopes": m.kept_scopes,
            "comment": m.comment.map(|c| c.to_base64()),
        }));
    }
    Ok(json!({"to": {"project": to.project.name, "id": to.project.uuid.to_string(), "scope": scope}, "tasks": tasks}))
}

/// Each of `ids`, scopes, named as a reference is (`ops::reference_json`).
async fn scopes(graph: &ProjectGraph, ids: &[EntityId]) -> Result<Value> {
    let mut named = Vec::with_capacity(ids.len());
    for id in ids {
        named.push(ops::reference_json(graph, *id).await?);
    }
    Ok(Value::Array(named))
}

/// The scopes `t`, a task of the answer, lists under `key`, by name.
fn names(t: &Value, key: &str) -> Vec<String> {
    t[key].as_array().into_iter().flatten().filter_map(|s| s["name"].as_str()).map(str::to_string).collect()
}

/// Where a task is worked, as a line says it: in no scope, in the scope, or in the scopes.
fn worked_in(t: &Value) -> String {
    match names(t, "worked_in").as_slice() {
        [] => "in no scope".into(),
        [one] => format!("in the scope {one}"),
        several => format!("in the scopes {}", several.join(", ")),
    }
}

/// The first eight characters of `t`'s id and its title, as a line names a task.
fn task_words(t: &Value) -> String {
    let id: String = t["id"].as_str().unwrap_or_default().chars().take(8).collect();
    format!("{id} \"{}\"", t["title"].as_str().unwrap_or_default())
}

/// The answer for people: where the tasks went, then a line per task saying what the run moved of
/// it, or that it was there already, with the scopes it is worked in.
fn text(v: &Value) -> String {
    let to = v["to"]["project"].as_str().unwrap_or_default();
    let mut lines = vec![match v["to"]["scope"]["title"].as_str() {
        Some(scope) => format!("To {to}, in the scope {scope}:"),
        None => format!("To {to}, in no scope:"),
    }];
    let count = |n: u64, one: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {one}s") };
    for t in v["tasks"].as_array().into_iter().flatten() {
        let task = task_words(t);
        let b = &t["brought"];
        let n = |key: &str| b[key].as_u64().unwrap_or(0);
        let brought = format!(
            "{}, {} and {}",
            count(n("messages"), "message"),
            count(n("relations"), "relation"),
            count(n("former_ids"), "former id")
        );
        let worked_in = worked_in(t);
        lines.push(if t["moved"] == json!(true) {
            let off = names(t, "taken_off");
            let off = if off.is_empty() { String::new() } else { format!("; taken off {}", off.join(", ")) };
            format!("  moved {task} from {}: the task, {brought}{off}", t["from"].as_str().unwrap_or_default())
        } else if ["messages", "relations", "former_ids"].iter().all(|k| n(k) == 0) {
            format!("  {task} is there already, {worked_in}")
        } else {
            format!("  {task} is there already, {worked_in}; took in what was left behind: {brought}")
        });
    }
    lines.join("\n")
}

/// What is said beside the answer of a task, `t` in it, that was in `project` already and keeps its
/// scopes without `scope`, the one the move names.
fn kept_scopes(t: &Value, project: &str, scope: &str) -> String {
    let id: String = t["id"].as_str().unwrap_or_default().chars().take(8).collect();
    format!(
        "fwd: {} was in {project} already, and stays {}: --to-scope `{scope}` was not applied, since a move puts a task in a scope only as it arrives; `fwd link {id} worked_in <scope>` adds a scope, and `fwd unlink {id} worked_in <scope> --reason <why>` takes one off",
        task_words(t),
        worked_in(t),
    )
}
