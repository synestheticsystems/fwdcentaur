//! `fwd intention` and `fwd reading`: intentions, the readings of them and their confirmation,
//! through `fwd_core::intentions` (docs/design/cli.md, "Intentions").

use crate::{given_text, made_json, principal_for, App, Format};
use ankurah::EntityId;
use ankurah_commons::intention_graph::{IntentionStatus, Stance};
use anyhow::Result;
use clap::{Subcommand, ValueEnum};
use fwd_core::intentions::readings::{self, ReadingRecord, SetReading};
use fwd_core::intentions::works_under::{self, WorkingIntention};
use fwd_core::intentions::{self, Authorship, IntentionRecord, NewIntention};
use fwd_core::ops::{self, Kind};
use fwd_core::project_graph::{ProjectGraph, Whose};
use fwd_core::reason::ago;
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
        /// Whose words they are, when you write them down for someone else: a name or an id.
        #[arg(long)]
        said_by: Option<String>,
        #[arg(long, value_enum, default_value_t = Recorded::Wished)]
        status: Recorded,
    },
    /// This project's intentions; with --mine, those you work under (`fwd link me works_under
    /// <intention>`), in any project, each with your reading of it and whether its speaker
    /// confirmed that reading: those you question or disagree with first, then those you have no
    /// reading of, then those not confirmed, then the confirmed ones.
    List {
        #[arg(long)]
        mine: bool,
    },
    Show {
        #[arg(allow_hyphen_values = true)]
        id: String,
    },
    /// Give an intention a new title, new words, or both.
    Reword {
        #[arg(allow_hyphen_values = true)]
        id: String,
        #[arg(long, allow_hyphen_values = true)]
        title: Option<String>,
        /// `-` reads them from standard input.
        #[arg(long, allow_hyphen_values = true)]
        words: Option<String>,
    },
    Fulfil {
        #[arg(allow_hyphen_values = true)]
        id: String,
        #[arg(long, allow_hyphen_values = true)]
        reason: Option<String>,
    },
    Withdraw {
        #[arg(allow_hyphen_values = true)]
        id: String,
        #[arg(long, allow_hyphen_values = true)]
        reason: Option<String>,
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

#[derive(Subcommand, Debug)]
pub(crate) enum ReadingCommand {
    /// Your reading of an intention, or of another reading: how you understand it, in your own
    /// words, and where you stand on it. You have one reading of a record, which a second `set`
    /// rewords.
    Set {
        /// The intention or reading you read.
        #[arg(allow_hyphen_values = true)]
        record: String,
        /// `-` reads them from standard input.
        #[arg(allow_hyphen_values = true)]
        words: String,
        #[arg(long, value_enum)]
        stance: StanceArg,
        /// The revision of the reading you read, as `fwd reading list` gives it: required when the
        /// record is a reading, and refused when that reading changed since.
        #[arg(long)]
        as_read: Option<String>,
        /// Whose reading it is, when you write it down for someone else: a name or an id.
        #[arg(long)]
        said_by: Option<String>,
    },
    /// Confirm a reading of an intention whose speaker you are, as you read it: a reading of it
    /// with stance agree. An agent writing down a speaker's confirmation names them with
    /// --said-by and gives their words.
    Confirm {
        #[arg(allow_hyphen_values = true)]
        reading: String,
        /// The revision you read, as `fwd reading list` gives it: refused when the reading changed
        /// since.
        #[arg(long)]
        as_read: String,
        /// The speaker's own words; `-` reads them from standard input.
        #[arg(long, allow_hyphen_values = true)]
        words: Option<String>,
        #[arg(long, requires = "words")]
        said_by: Option<String>,
    },
    /// The readings of an intention or a reading, wherever they were written.
    List {
        #[arg(long, allow_hyphen_values = true)]
        of: String,
    },
}

#[derive(ValueEnum, Clone, Copy, Debug)]
pub(crate) enum StanceArg {
    Agree,
    Disagree,
    Question,
}

impl From<StanceArg> for Stance {
    fn from(stance: StanceArg) -> Stance {
        match stance {
            StanceArg::Agree => Stance::Agree,
            StanceArg::Disagree => Stance::Disagree,
            StanceArg::Question => Stance::Question,
        }
    }
}

pub(crate) async fn intention(app: &App, command: IntentionCommand) -> Result<()> {
    let graph = app.graph();
    match command {
        IntentionCommand::Create { title, words, said_by, status } => {
            let said_by = speaker_named(app, said_by).await?;
            let status = match status {
                Recorded::Wished => IntentionStatus::Wished,
                Recorded::Committed => IntentionStatus::Committed,
            };
            let new = NewIntention { title, words: given_text(app, words)?, said_by, status };
            let id = intentions::create(graph, app.me, new).await?;
            let made = intention_json(graph, &intentions::intention(graph, id).await?).await?;
            app.print_made(made_json(made), intention_text)
        }
        IntentionCommand::List { mine: false } => {
            let mut listed = Vec::new();
            for i in intentions::intentions(graph).await? {
                listed.push(intention_json(graph, &i).await?);
            }
            app.print(Value::Array(listed))
        }
        IntentionCommand::List { mine: true } => {
            let held = works_under::works_under(graph, app.me).await?;
            let mut listed = Vec::new();
            for w in &held.intentions {
                listed.push(working_json(graph, w).await?);
            }
            let not_held: Vec<String> = held.not_held.iter().map(EntityId::to_base64).collect();
            let v = json!({"intentions": listed, "not_held": not_held});
            match app.format {
                Format::Text => {
                    println!("{}", works_under_text(&v));
                    Ok(())
                }
                _ => app.print(v),
            }
        }
        IntentionCommand::Show { id } => {
            let id = app.id(&id, INTENTION).await?;
            print_intention(app, id).await
        }
        IntentionCommand::Reword { id, title, words } => {
            let id = app.id(&id, INTENTION).await?;
            let words = words.map(|w| given_text(app, w)).transpose()?;
            intentions::reword(graph, app.me, id, title.as_deref(), words.as_deref()).await?;
            print_intention(app, id).await
        }
        IntentionCommand::Fulfil { id, reason } => close(app, &id, IntentionStatus::Fulfilled, reason).await,
        IntentionCommand::Withdraw { id, reason } => close(app, &id, IntentionStatus::Withdrawn, reason).await,
    }
}

/// The principal `--said-by` names, whose words a record holds when someone else writes them down.
async fn speaker_named(app: &App, said_by: Option<String>) -> Result<Option<EntityId>> {
    match said_by {
        Some(name) => Ok(Some(principal_for(app, Some(&name)).await?)),
        None => Ok(None),
    }
}

async fn close(app: &App, id: &str, status: IntentionStatus, reason: Option<String>) -> Result<()> {
    let id = app.id(id, INTENTION).await?;
    intentions::close(app.graph(), app.me, id, status, reason).await?;
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

pub(crate) async fn reading(app: &App, command: ReadingCommand) -> Result<()> {
    let graph = app.graph();
    let id = match command {
        ReadingCommand::Set { record, words, stance, as_read, said_by } => {
            let of = app.id(&record, &[Kind::Intention, Kind::Reading]).await?;
            let said_by = speaker_named(app, said_by).await?;
            let spec = SetReading { of, words: given_text(app, words)?, stance: stance.into(), said_by, as_read };
            readings::set(graph, app.me, spec).await?
        }
        ReadingCommand::Confirm { reading, as_read, words, said_by } => {
            let reading = app.id(&reading, &[Kind::Reading]).await?;
            let words = words.map(|w| given_text(app, w)).transpose()?;
            let said_by = speaker_named(app, said_by).await?;
            readings::confirm(graph, app.me, reading, as_read, words, said_by).await?
        }
        ReadingCommand::List { of } => {
            let of = app.id(&of, &[Kind::Intention, Kind::Reading]).await?;
            let mut listed = Vec::new();
            for r in readings::readings_of(graph, of).await? {
                listed.push(reading_json(graph, &r).await?);
            }
            return match app.format {
                Format::Text => {
                    let lines: Vec<String> = listed.iter().map(reading_text).collect();
                    println!("{}", lines.join("\n"));
                    Ok(())
                }
                _ => app.print(Value::Array(listed)),
            };
        }
    };
    let readings::Readable::Reading(written) = readings::read_record(graph, id).await? else {
        anyhow::bail!("{} is no reading", id.to_base64())
    };
    app.print_made(made_json(reading_json(graph, &written).await?), reading_text)
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

/// A reading as `reading list` gives it, with the revision a reader passes back as `--as-read`.
async fn reading_json(graph: &ProjectGraph, r: &ReadingRecord) -> Result<Value> {
    let mut v = json!({
        "id": r.id.to_base64(),
        "kind": "reading",
        "of": r.of.to_base64(),
        "words": r.words,
        "stance": r.stance.map(|s| s.to_string()),
        "revision": r.revision,
    });
    merge(&mut v, authorship_json(graph, &r.authorship).await?);
    if let Some(project) = other_project(graph, r.project).await? {
        v["project"] = json!(project);
    }
    Ok(v)
}

/// One intention a principal works under, as `intention list --mine` gives it.
async fn working_json(graph: &ProjectGraph, w: &WorkingIntention) -> Result<Value> {
    let reading = match &w.reading {
        Some(r) => Some(reading_json(graph, r).await?),
        None => None,
    };
    let confirmed = match &w.confirmation {
        Some(c) => Some(json!({
            "reading": c.reading.to_base64(),
            "by": c.by.to_base64(),
            "by_handle": handle(graph, c.by).await?,
            "at": c.at,
        })),
        None => None,
    };
    Ok(json!({
        "standing": w.standing().to_string(),
        "intention": intention_json(graph, &w.intention).await?,
        "reading": reading,
        "confirmed": confirmed,
        "reworded_since_reading": w.reworded_since_reading,
    }))
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

/// `words` indented under a line, each of its lines.
fn indented(words: &str) -> String {
    words.lines().map(|l| format!("    {l}")).collect::<Vec<_>>().join("\n")
}

/// Who wrote a record's words down, whose they are, and when, as a line: `@ines, written down by
/// @impl-tide 2h ago, reworded 5m ago by @ines`.
fn written_line(v: &Value) -> String {
    let mut line = format!("@{}", text(v, "speaker_handle"));
    if v["written_by"] != v["speaker"] {
        line.push_str(&format!(", written down by @{}", text(v, "written_by_handle")));
    }
    line.push_str(&format!(" {}", v["written"].as_i64().map(ago).unwrap_or_default()));
    if let Some(at) = v["edited"].as_i64() {
        line.push_str(&format!(", reworded {} by @{}", ago(at), text(v, "edited_by_handle")));
    }
    line
}

/// An intention for people: its id, title and status, then whose words and when, then the words.
fn intention_text(v: &Value) -> String {
    let project = v["project"].as_str().map(|p| format!(" in {p}")).unwrap_or_default();
    let mut out = format!(
        "{} {} [{}]{project}\n  {}:\n{}",
        short(v),
        text(v, "title"),
        text(v, "status"),
        written_line(v),
        indented(text(v, "words"))
    );
    if let Some(reason) = v["closed"]["reason"].as_str() {
        out.push_str(&format!("\n  {}: {reason}", text(v, "status")));
    }
    out
}

/// A reading for people: its id and stance, what it reads, whose it is and when, its revision,
/// then its words.
fn reading_text(v: &Value) -> String {
    let of: String = text(v, "of").chars().take(8).collect();
    let (stance, revision) = (text(v, "stance"), text(v, "revision"));
    format!(
        "{} {stance}, reading {of}: {}, revision {revision}\n{}",
        short(v),
        written_line(v),
        indented(text(v, "words"))
    )
}

/// What a principal works under, for people: each intention, then the principal's reading of it
/// and whether its speaker confirmed it.
fn works_under_text(v: &Value) -> String {
    let mut blocks = Vec::new();
    for w in v["intentions"].as_array().into_iter().flatten() {
        let i = &w["intention"];
        let mut block = format!("{}: {}", text(w, "standing").replace('_', " "), intention_text(i));
        match w["reading"].as_object() {
            None => block.push_str("\n  no reading of yours yet"),
            Some(_) => {
                let r = &w["reading"];
                block.push_str(&format!(
                    "\n  your reading {} ({}, revision {}):\n{}",
                    short(r),
                    text(r, "stance"),
                    text(r, "revision"),
                    indented(text(r, "words"))
                ));
                match w["confirmed"].as_object() {
                    Some(_) => block.push_str(&format!(
                        "\n  confirmed by @{} {}",
                        text(&w["confirmed"], "by_handle"),
                        w["confirmed"]["at"].as_i64().map(ago).unwrap_or_default()
                    )),
                    None => block.push_str("\n  not confirmed"),
                }
                if w["reworded_since_reading"] == json!(true) {
                    block.push_str("; the intention was reworded since your reading");
                }
            }
        }
        blocks.push(block);
    }
    for id in v["not_held"].as_array().into_iter().flatten() {
        blocks.push(format!("{} (cannot be followed here)", id.as_str().unwrap_or_default()));
    }
    if blocks.is_empty() {
        return "you work under no intention: link one with `fwd link me works_under <intention>`".into();
    }
    blocks.join("\n\n")
}
