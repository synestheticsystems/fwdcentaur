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
    /// This project's intentions; with --mine, those you work under (`fwd link me works_under
    /// <intention>`), in any project, each with your reading of it and the speaker's reading of
    /// yours: one since fulfilled or withdrawn first, then those you or the speaker question or
    /// disagree with, then one reworded since you read it, then those you have no reading of, then
    /// those not confirmed, then the confirmed ones.
    List {
        #[arg(long)]
        mine: bool,
    },
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

#[derive(Subcommand, Debug)]
pub(crate) enum ReadingCommand {
    /// Your reading of an intention, or of another reading: how you understand it, in your own
    /// words, and where you stand on it. You have one reading of a record, which a second `set`
    /// rewords, or, saying the same of a new wording, records that you read it.
    Set {
        /// The intention or reading you read.
        #[arg(allow_hyphen_values = true)]
        record: String,
        /// `-` reads them from standard input.
        #[arg(allow_hyphen_values = true)]
        words: String,
        #[arg(long, value_enum)]
        stance: StanceArg,
        /// The revision of the wording you read, as `fwd intention show` and `fwd reading list`
        /// give it: required when the record is a reading; refused when the record changed since.
        #[arg(long)]
        as_read: Option<String>,
        /// The person whose reading it is, when you write it down for them: a name in this project,
        /// or an id of any project.
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
    let read = async {
        match readings::read_record(graph, id).await? {
            readings::Readable::Reading(written) => reading_json(graph, &written).await,
            readings::Readable::Intention(_) => anyhow::bail!("{} is no reading", id.to_base64()),
        }
    };
    print_made(app, "reading", id, read.await, reading_text)
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

/// An intention as `show` and `list` give it, with the revision a reading of it records as read.
async fn intention_json(graph: &ProjectGraph, i: &IntentionRecord) -> Result<Value> {
    let mut v = json!({
        "id": i.id.to_base64(),
        "kind": "intention",
        "title": i.title,
        "words": i.words,
        "status": i.status.to_string(),
        "revision": i.revision,
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

/// A reading as `reading list` gives it: the revision of the wording it read (`read`), and its
/// own, which a reader passes back as `--as-read`.
async fn reading_json(graph: &ProjectGraph, r: &ReadingRecord) -> Result<Value> {
    let mut v = json!({
        "id": r.id.to_base64(),
        "kind": "reading",
        "of": r.of.to_base64(),
        "words": r.words,
        "stance": r.stance.map(|s| s.to_string()),
        "read": r.read,
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
    let speakers_reading = match &w.speakers_reading {
        Some(r) => Some(reading_json(graph, r).await?),
        None => None,
    };
    Ok(json!({
        "standing": w.standing().to_string(),
        "intention": intention_json(graph, &w.intention).await?,
        "reading": reading,
        "speakers_reading": speakers_reading,
        "confirmed": w.confirmed(),
        "reworded_since_reading": w.reworded_since_reading(),
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

/// `words` on the lines after a heading, each indented; nothing for no words.
fn indented(words: &str) -> String {
    words.lines().map(|l| format!("\n    {l}")).collect()
}

/// `:` and `words` on the lines after it, indented; nothing for no words.
fn said(words: &str) -> String {
    if words.is_empty() {
        String::new()
    } else {
        format!(":{}", indented(words))
    }
}

/// ` in <project>` for a record of another project, else nothing.
fn elsewhere(v: &Value) -> String {
    v["project"].as_str().map(|p| format!(" in {p}")).unwrap_or_default()
}

/// Whose words a record holds, and who wrote them down when that is someone else: `@ines`,
/// `@ines, written down by @impl-tide`. No time, so that the text reads the same whenever it is
/// read while the records are unchanged.
fn whose(v: &Value) -> String {
    let mut line = format!("@{}", text(v, "speaker_handle"));
    if v["written_by"] != v["speaker"] {
        line.push_str(&format!(", written down by @{}", text(v, "written_by_handle")));
    }
    if v["edited_by"].is_string() && v["edited_by"] != v["written_by"] {
        line.push_str(&format!(", reworded by @{}", text(v, "edited_by_handle")));
    }
    line
}

/// An intention for people: its id, title, status and revision, whose words, then the words.
fn intention_text(v: &Value) -> String {
    let mut out = format!(
        "{} {} [{}]{} (revision {})\n  {}:{}",
        short(v),
        text(v, "title"),
        text(v, "status"),
        elsewhere(v),
        text(v, "revision"),
        whose(v),
        indented(text(v, "words"))
    );
    if let Some(reason) = v["closed"]["reason"].as_str() {
        out.push_str(&format!("\n  {}: {reason}", text(v, "status")));
    }
    out
}

/// A reading for people: its id and stance, what it reads and where it is, whose it is, its
/// revision, then its words.
fn reading_text(v: &Value) -> String {
    let of: String = text(v, "of").chars().take(8).collect();
    let (stance, revision) = (text(v, "stance"), text(v, "revision"));
    format!(
        "{} {stance}, reading {of}{}: {}, revision {revision}:{}",
        short(v),
        elsewhere(v),
        whose(v),
        indented(text(v, "words"))
    )
}

/// What a principal works under, for people: each intention, the principal's reading of it, the
/// speaker's reading of that, and where it stands. No time, so that the text reads the same
/// whenever it is read while the records are unchanged.
fn works_under_text(v: &Value) -> String {
    let mut blocks = Vec::new();
    for w in v["intentions"].as_array().into_iter().flatten() {
        let i = &w["intention"];
        let mut block = format!("{}: {}", text(w, "standing").replace('_', " "), intention_text(i));
        if w["standing"] == "ended" {
            block.push_str(&format!(
                "\n  it is {}, and you still work under it: review the link (`fwd unlink me works_under {} --reason ...`)",
                text(i, "status"),
                short(i)
            ));
        }
        let r = &w["reading"];
        if !r.is_object() {
            block.push_str("\n  no reading of yours yet");
            blocks.push(block);
            continue;
        }
        block.push_str(&format!(
            "\n  your reading {} ({}, revision {}){}:{}",
            short(r),
            text(r, "stance"),
            text(r, "revision"),
            elsewhere(r),
            indented(text(r, "words"))
        ));
        if w["reworded_since_reading"] == json!(true) {
            block.push_str("\n  the intention was reworded since your reading read it");
        }
        let s = &w["speakers_reading"];
        if !s.is_object() {
            block.push_str("\n  not confirmed");
        } else if w["confirmed"] == json!(true) {
            block.push_str(&format!("\n  confirmed by {}{}", whose(s), said(text(s, "words"))));
        } else {
            block.push_str(&format!("\n  {} by {}{}", text(s, "stance"), whose(s), said(text(s, "words"))));
            if s["read"] != r["revision"] {
                block.push_str("\n  (an answer to an earlier revision of your reading)");
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A reading with `stance` by `party`, written down by `writer`, of the wording `read`.
    fn reading(id: &str, party: &str, writer: &str, stance: &str, words: &str, read: &str, revision: &str) -> Value {
        json!({
            "id": id, "kind": "reading", "words": words, "stance": stance, "read": read, "revision": revision,
            "speaker": party, "speaker_handle": party, "written_by": writer, "written_by_handle": writer,
            "written": 1_700_000_000_000_i64, "edited": null, "edited_by": null,
        })
    }

    fn intention(id: &str, title: &str, status: &str) -> Value {
        json!({
            "id": id, "kind": "intention", "title": title, "words": "the words", "status": status, "revision": "i1",
            "speaker": "ines", "speaker_handle": "ines", "written_by": "impl-tide", "written_by_handle": "impl-tide",
            "written": 1_600_000_000_000_i64, "edited": null, "edited_by": null, "closed": null,
        })
    }

    /// The text of what a principal works under holds no time: rendered from the same records
    /// whatever the clock says, it is these bytes, so that a prompt holding it stays the same.
    #[test]
    fn the_text_of_what_a_principal_works_under_reads_the_same_whenever_it_is_read() {
        let agents = reading("RRRRRRRRone", "impl-tide", "impl-tide", "agree", "as I read it", "i1", "r1");
        let relayed = reading("SSSSSSSSone", "ines", "impl-tide", "agree", "yes", "r1", "s1");
        let objecting = reading("SSSSSSSStwo", "ines", "ines", "disagree", "no, only the scroll", "r0", "s2");
        let v = json!({"intentions": [
            {"standing": "to_discuss", "intention": intention("IIIIIIIItwo", "Builds", "committed"),
             "reading": agents, "speakers_reading": objecting, "confirmed": false, "reworded_since_reading": false},
            {"standing": "confirmed", "intention": intention("IIIIIIIIone", "Readers", "committed"),
             "reading": agents, "speakers_reading": relayed, "confirmed": true, "reworded_since_reading": false},
        ], "not_held": []});
        let expected = "to discuss: IIIIIIII Builds [committed] (revision i1)
  @ines, written down by @impl-tide:
    the words
  your reading RRRRRRRR (agree, revision r1):
    as I read it
  disagree by @ines:
    no, only the scroll
  (an answer to an earlier revision of your reading)

confirmed: IIIIIIII Readers [committed] (revision i1)
  @ines, written down by @impl-tide:
    the words
  your reading RRRRRRRR (agree, revision r1):
    as I read it
  confirmed by @ines, written down by @impl-tide:
    yes";
        assert_eq!(works_under_text(&v), expected);
        assert_eq!(works_under_text(&v), works_under_text(&v.clone()));
    }
}
