//! `fwd intention`: intentions, through `fwd_core::intentions` (docs/design/cli.md, "Intentions").

use crate::{given_text, made_json, principal_for, App, Format};
use ankurah::EntityId;
use ankurah_commons::intention_graph::IntentionStatus;
use anyhow::Result;
use clap::{Subcommand, ValueEnum};
use fwd_core::intentions::{self, Authorship, IntentionRecord, NewIntention};
use fwd_core::ops::{self, Kind};
use fwd_core::project_graph::{ProjectGraph, Whose};
use fwd_core::rules::short_id;
use serde_json::{json, Value};

const INTENTION: &[Kind] = &[Kind::Intention];

#[derive(Subcommand, Debug)]
pub(crate) enum IntentionCommand {
    /// Record an intention in this project, in the words of whoever wants it.
    Create {
        #[arg(allow_hyphen_values = true)]
        title: String,
        /// The words, as their speaker gave them; `-` reads them from standard input.
        #[arg(long, allow_hyphen_values = true)]
        words: String,
        /// The person whose words they are, when you write them down for them: a name in this
        /// project, or an id of any project.
        #[arg(long)]
        said_by: Option<String>,
        #[arg(long, value_enum, default_value_t = Recorded::Wished)]
        status: Recorded,
    },
    /// This project's intentions.
    List,
    Show {
        #[arg(allow_hyphen_values = true)]
        id: String,
    },
    /// Give an intention a new title, new words, or both, as its speaker, or writing down what the
    /// speaker says (--said-by).
    Reword {
        #[arg(allow_hyphen_values = true)]
        id: String,
        #[arg(long, allow_hyphen_values = true)]
        title: Option<String>,
        /// `-` reads them from standard input.
        #[arg(long, allow_hyphen_values = true)]
        words: Option<String>,
        #[arg(long)]
        said_by: Option<String>,
    },
    Fulfil {
        #[arg(allow_hyphen_values = true)]
        id: String,
        #[arg(long, allow_hyphen_values = true)]
        reason: Option<String>,
        #[arg(long)]
        said_by: Option<String>,
    },
    /// Withdraw an intention, saying why.
    Withdraw {
        #[arg(allow_hyphen_values = true)]
        id: String,
        #[arg(long, allow_hyphen_values = true)]
        reason: Option<String>,
        #[arg(long)]
        said_by: Option<String>,
    },
}

/// The status an intention is recorded with; it is fulfilled or withdrawn afterwards.
#[derive(ValueEnum, Clone, Copy, Debug)]
pub(crate) enum Recorded {
    /// Wanted, with no one yet committed to act on it.
    Wished,
    /// Someone has committed to act on it.
    Committed,
}

pub(crate) async fn intention(app: &App, command: IntentionCommand) -> Result<()> {
    let graph = app.graph();
    match command {
        IntentionCommand::Create { title, words, said_by, status } => {
            let status = match status {
                Recorded::Wished => IntentionStatus::Wished,
                Recorded::Committed => IntentionStatus::Committed,
            };
            let said_by = speaker_named(app, said_by).await?;
            let new = NewIntention { title, words: given_text(app, words)?, said_by, status };
            let id = intentions::create(graph, app.me, new).await?;
            let read = async { intention_json(graph, &intentions::intention(graph, id).await?).await };
            print_made(app, "intention", id, read.await, intention_text)
        }
        IntentionCommand::List => {
            let mut listed = Vec::new();
            for i in intentions::intentions(graph).await? {
                listed.push(intention_json(graph, &i).await?);
            }
            app.print(Value::Array(listed))
        }
        IntentionCommand::Show { id } => {
            let id = app.id(&id, INTENTION).await?;
            print_intention(app, id).await
        }
        IntentionCommand::Reword { id, title, words, said_by } => {
            let id = app.id(&id, INTENTION).await?;
            let words = words.map(|w| given_text(app, w)).transpose()?;
            let said_by = speaker_named(app, said_by).await?;
            intentions::reword(graph, app.me, id, title.as_deref(), words.as_deref(), said_by).await?;
            print_intention(app, id).await
        }
        IntentionCommand::Fulfil { id, reason, said_by } => {
            close(app, &id, IntentionStatus::Fulfilled, reason, said_by).await
        }
        IntentionCommand::Withdraw { id, reason, said_by } => {
            close(app, &id, IntentionStatus::Withdrawn, reason, said_by).await
        }
    }
}

/// The person `--said-by` names: by name, one of this project's principals; by id, whole or its
/// start, one of any project's.
async fn speaker_named(app: &App, said_by: Option<String>) -> Result<Option<EntityId>> {
    let Some(named) = said_by else { return Ok(None) };
    match principal_for(app, Some(&named)).await {
        Ok(here) => Ok(Some(here)),
        Err(_) if ops::may_start_an_id(&named) => Ok(Some(app.id(&named, &[Kind::Principal]).await?)),
        Err(none) => Err(none),
    }
}

async fn close(
    app: &App,
    id: &str,
    status: IntentionStatus,
    reason: Option<String>,
    said_by: Option<String>,
) -> Result<()> {
    let id = app.id(id, INTENTION).await?;
    let said_by = speaker_named(app, said_by).await?;
    intentions::close(app.graph(), app.me, id, status, reason, said_by).await?;
    print_intention(app, id).await
}

async fn print_intention(app: &App, id: EntityId) -> Result<()> {
    let v = intention_json(app.graph(), &intentions::intention(app.graph(), id).await?).await?;
    match app.format {
        Format::Text => {
            println!("{}", intention_text(&v));
            Ok(())
        }
        _ => app.print(v),
    }
}

/// What a command that made `what`, `id`, answers: the record as `read` gives it, or, when it
/// could not be read back, its id alone and a note saying so, since it was made all the same and
/// running the command again would make it twice.
fn print_made(app: &App, what: &str, id: EntityId, read: Result<Value>, text: fn(&Value) -> String) -> Result<()> {
    let record = read.unwrap_or_else(|why| {
        let short = short_id(&id);
        app.note(format!(
            "NOTE: the {what} {short} was made, but it could not be read back ({why:#}); `fwd {what} list` lists it, and running this command again would make the {what} twice"
        ));
        json!({"id": id.to_base64()})
    });
    match (app.format, record.get("kind")) {
        (Format::Text, None) => {
            println!("{}", record["id"].as_str().unwrap_or_default());
            Ok(())
        }
        _ => app.print_made(made_json(record), text),
    }
}

/// The name of the principal `id` for a reader in `graph`'s project.
async fn handle(graph: &ProjectGraph, id: EntityId) -> Result<String> {
    ops::handle_of(graph, id).await
}

/// Who wrote a record and when, whose words it holds, and when they were last changed.
async fn authorship_json(graph: &ProjectGraph, a: &Authorship) -> Result<Value> {
    let edited_by = match a.edited.and_then(|e| e.by) {
        Some(by) => Some(handle(graph, by).await?),
        None => None,
    };
    Ok(json!({
        "written_by": a.written_by.to_base64(),
        "written_by_handle": handle(graph, a.written_by).await?,
        "written": a.written,
        "speaker": a.speaker.to_base64(),
        "speaker_handle": handle(graph, a.speaker).await?,
        "edited": a.edited.map(|e| e.at),
        "edited_by": a.edited.and_then(|e| e.by).map(|by| by.to_base64()),
        "edited_by_handle": edited_by,
    }))
}

/// The name of the project whose record is `project`, when it is not `graph`'s.
async fn other_project(graph: &ProjectGraph, project: EntityId) -> Result<Option<String>> {
    Ok(graph.projects_named(&[project]).await?.into_iter().find_map(|(_, whose)| match whose {
        Whose::Other(p) => Some(p.name),
        Whose::This => None,
    }))
}

/// An intention as `show` and `list` give it.
async fn intention_json(graph: &ProjectGraph, i: &IntentionRecord) -> Result<Value> {
    let mut v = json!({
        "id": i.id.to_base64(),
        "kind": "intention",
        "title": i.title,
        "words": i.words,
        "status": i.status.to_string(),
        "closed": i.closed.as_ref().map(|c| json!({
            "at": c.at,
            "by": c.by.map(|by| by.to_base64()),
            "reason": c.reason,
        })),
    });
    merge(&mut v, authorship_json(graph, &i.authorship).await?);
    if let Some(project) = other_project(graph, i.project).await? {
        v["project"] = json!(project);
    }
    Ok(v)
}

fn merge(into: &mut Value, from: Value) {
    if let (Some(into), Value::Object(from)) = (into.as_object_mut(), from) {
        into.extend(from);
    }
}

/// `field` of `v` as text, empty where it has none.
fn text<'a>(v: &'a Value, field: &str) -> &'a str {
    v[field].as_str().unwrap_or_default()
}

/// The first eight characters of `v`'s id, by which a command takes it.
fn short(v: &Value) -> String {
    text(v, "id").chars().take(8).collect()
}

/// `words` on the lines after a heading, each indented; nothing for no words.
fn indented(words: &str) -> String {
    words.lines().map(|l| format!("\n    {l}")).collect()
}

/// ` in <project>` for a record of another project, else nothing.
fn elsewhere(v: &Value) -> String {
    v["project"].as_str().map(|p| format!(" in {p}")).unwrap_or_default()
}

/// Whose words a record holds, and who wrote them down when that is someone else: `@ines`,
/// `@ines, written down by @impl-tide`. No time, so that the text reads the same whenever it is
/// read while the records are unchanged.
fn whose(v: &Value) -> String {
    format!("@{}{}", text(v, "speaker_handle"), written_down(v))
}

/// Who wrote a record's words down, when not its speaker, and who reworded them, when not whoever
/// wrote them down: `, written down by @impl-tide`; nothing when the speaker did both.
fn written_down(v: &Value) -> String {
    let mut line = String::new();
    if v["written_by"] != v["speaker"] {
        line.push_str(&format!(", written down by @{}", text(v, "written_by_handle")));
    }
    if v["edited_by"].is_string() && v["edited_by"] != v["written_by"] {
        line.push_str(&format!(", reworded by @{}", text(v, "edited_by_handle")));
    }
    line
}

/// An intention for people: its id, title and status, whose words, then the words.
fn intention_text(v: &Value) -> String {
    let mut out = format!(
        "{} {} [{}]{}\n  {}:{}",
        short(v),
        text(v, "title"),
        text(v, "status"),
        elsewhere(v),
        whose(v),
        indented(text(v, "words"))
    );
    if let Some(reason) = v["closed"]["reason"].as_str() {
        out.push_str(&format!("\n  {}: {reason}", text(v, "status")));
    }
    out
}
