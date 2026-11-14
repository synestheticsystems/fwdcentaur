//! `fwd intention`: intentions, through `fwd_core::intentions` (docs/design/cli.md, "Intentions").
//! Each answer is built from the intention's views, as JSON or as text for people.

use crate::{given_text, made_json, principal_for, App, Format};
use ankurah::EntityId;
use ankurah_commons::intention_graph::{Intention, IntentionStatus};
use ankurah_commons::records::{AttributionView, ClosureView, LastEditView};
use anyhow::{bail, Result};
use clap::{Subcommand, ValueEnum};
use fwd_core::decision_log::date;
use fwd_core::intentions::{self, Parts};
use fwd_core::model::{PlacementView, RelationView};
use fwd_core::ops::{self, Kind};
use fwd_core::project_graph::{Followed, InProjectView, ProjectGraph, Whose};
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
    /// This project's intentions; with --active, yours.
    List {
        /// Your active intentions, of any project: those assigned to you that have not ended.
        #[arg(long)]
        active: bool,
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
    /// Assign an intention of any project to yourself: it is one of your active intentions until
    /// it ends or is unassigned.
    Assign {
        #[arg(allow_hyphen_values = true)]
        id: String,
        /// Whom to assign it to: yourself, the one principal it is assigned to for now.
        #[arg(long)]
        to: Option<String>,
    },
    /// Take an intention off yourself, or off the principal --to names.
    Unassign {
        #[arg(allow_hyphen_values = true)]
        id: String,
        #[arg(long)]
        to: Option<String>,
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
            let new = Intention { title, words: given_text(app, words)?, status };
            print_made(app, &intentions::create(graph, app.me, new, said_by).await?).await
        }
        IntentionCommand::List { active: false } => {
            let mut listed = graph.all_placed::<Parts>().await?;
            listed.sort_by_key(|(placement, _)| placement.id());
            print_intentions(app, &listed).await
        }
        IntentionCommand::List { active: true } => {
            let mut active = Vec::new();
            let mut not_held = Vec::new();
            for (id, assigned) in intentions::assigned_to(graph, app.me).await? {
                match assigned {
                    Followed::Held { found, .. } if !intentions::ended(found.1 .0.status()?) => active.push(found),
                    Followed::Held { .. } => {}
                    Followed::NotHeld => not_held.push(id),
                }
            }
            if let Some(first) = not_held.iter().min() {
                let n = not_held.len();
                app.note(format!(
                    "fwd: {n} intentions assigned to you could not be read, the first {}",
                    short_id(first)
                ));
            }
            print_intentions(app, &active).await
        }
        IntentionCommand::Show { id } => {
            let id = app.id(&id, INTENTION).await?;
            print_intention(app, &graph.get_placed::<Parts>(id).await?).await
        }
        IntentionCommand::Reword { id, title, words, said_by } => {
            let record = graph.get_placed::<Parts>(app.id(&id, INTENTION).await?).await?;
            let words = words.map(|w| given_text(app, w)).transpose()?;
            let said_by = speaker_named(app, said_by).await?;
            intentions::reword(graph, app.me, &record, title.as_deref(), words.as_deref(), said_by).await?;
            print_intention(app, &record).await
        }
        IntentionCommand::Fulfil { id, reason, said_by } => {
            close(app, &id, IntentionStatus::Fulfilled, reason, said_by).await
        }
        IntentionCommand::Withdraw { id, reason, said_by } => {
            close(app, &id, IntentionStatus::Withdrawn, reason, said_by).await
        }
        IntentionCommand::Assign { id, to } => {
            let record = followed(app, &id).await?;
            let to = principal_for(app, to.as_deref().map(|t| t.trim_start_matches('@'))).await?;
            let assignment = intentions::assign(graph, app.me, &record, to).await?;
            print_assignment(app, &record, &assignment).await
        }
        IntentionCommand::Unassign { id, to } => {
            let record = followed(app, &id).await?;
            let to = principal_for(app, to.as_deref().map(|t| t.trim_start_matches('@'))).await?;
            let retracted = intentions::unassign(graph, app.me, &record, to).await?;
            match app.format {
                Format::Text => {
                    let still = if retracted == 0 { "not" } else { "no longer" };
                    let whom = ops::handle_of(graph, to).await?;
                    println!("{}: {still} assigned to @{whom}", heading(graph, &record).await?);
                    Ok(())
                }
                _ => app.print(json!({"retracted": retracted})),
            }
        }
    }
}

/// The intention `typed` names, of any project, wherever it is: one is assigned from whichever
/// project the command runs in, to a principal of that project.
async fn followed(app: &App, typed: &str) -> Result<(PlacementView, Parts)> {
    let id = app.id(typed, INTENTION).await?;
    match app.graph().follow_placed::<Parts>(id).await? {
        Followed::Held { found, .. } => Ok(found),
        Followed::NotHeld => bail!("{} names no intention the node holds", short_id(&id)),
    }
}

/// What `assign` answers: the relation that assigns the intention `record`, as JSON `made`, or for
/// people the intention's heading and to whom it is assigned.
async fn print_assignment(app: &App, record: &(PlacementView, Parts), assignment: &RelationView) -> Result<()> {
    let graph = app.graph();
    match app.format {
        Format::Text => {
            let whom = ops::handle_of(graph, assignment.to_id()?).await?;
            println!("{}: assigned to @{whom}", heading(graph, record).await?);
            Ok(())
        }
        _ => app.print(made_json(ops::json_of_relation(assignment)?)),
    }
}

/// An intention's first line for people: its short id, its title, and its project when that is
/// not `graph`'s.
async fn heading(graph: &ProjectGraph, (placement, (intention, _)): &(PlacementView, Parts)) -> Result<String> {
    Ok(format!("{} {}{}", short_id(&placement.id()), intention.title()?, elsewhere(graph, placement).await?))
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
    let record = app.graph().get_placed::<Parts>(app.id(id, INTENTION).await?).await?;
    let said_by = speaker_named(app, said_by).await?;
    intentions::close(app.graph(), app.me, &record, status, reason, said_by).await?;
    print_intention(app, &record).await
}

async fn print_intention(app: &App, record: &(PlacementView, Parts)) -> Result<()> {
    match app.format {
        Format::Text => {
            println!("{}", intention_text(app.graph(), record).await?);
            Ok(())
        }
        _ => app.print(intention_json(app.graph(), record).await?),
    }
}

async fn print_intentions(app: &App, records: &[(PlacementView, Parts)]) -> Result<()> {
    let graph = app.graph();
    match app.format {
        Format::Text => {
            let mut blocks = Vec::with_capacity(records.len());
            for record in records {
                blocks.push(intention_text(graph, record).await?);
            }
            println!("{}", blocks.join("\n"));
            Ok(())
        }
        _ => {
            let mut listed = Vec::with_capacity(records.len());
            for record in records {
                listed.push(intention_json(graph, record).await?);
            }
            app.print(Value::Array(listed))
        }
    }
}

/// What `create` answers: the intention it made, or, when that cannot be read back, its id alone
/// and a note saying so, since it was made all the same and running the command again would make
/// it twice.
async fn print_made(app: &App, record: &(PlacementView, Parts)) -> Result<()> {
    let graph = app.graph();
    let id = record.0.id();
    let unread = |why: anyhow::Error| {
        app.note(format!(
            "NOTE: the intention {} was made, but it could not be read back ({why:#}); `fwd intention list` lists it, and running this command again would make the intention twice",
            short_id(&id)
        ))
    };
    match app.format {
        Format::Text => {
            match intention_text(graph, record).await {
                Ok(text) => println!("{text}"),
                Err(why) => {
                    unread(why);
                    println!("{}", id.to_base64());
                }
            }
            Ok(())
        }
        _ => match intention_json(graph, record).await {
            Ok(v) => app.print(made_json(v)),
            Err(why) => {
                unread(why);
                app.print(made_json(json!({"id": id.to_base64()})))
            }
        },
    }
}

/// The name of the project whose record is `project`, when it is not `graph`'s.
async fn other_project(graph: &ProjectGraph, project: EntityId) -> Result<Option<String>> {
    Ok(graph.projects_named(&[project]).await?.into_iter().find_map(|(_, whose)| match whose {
        Whose::Other(p) => Some(p.name),
        Whose::This => None,
    }))
}

/// An intention as JSON: the fields of its components, each principal by its id with its name
/// beside it, whose words they are (`speaker`), and its project when that is not `graph`'s.
pub(crate) async fn intention_json(graph: &ProjectGraph, record: &(PlacementView, Parts)) -> Result<Value> {
    let (placement, (intention, creation)) = record;
    let attribution = graph.component_of::<AttributionView>(placement).await?;
    let edit = graph.component_of::<LastEditView>(placement).await?;
    let closure = graph.component_of::<ClosureView>(placement).await?;
    let created_by = *creation.created_by()?;
    let speaker = intentions::speaker(creation, attribution.as_ref())?;
    let edited_by = match &edit {
        Some(e) => e.edited_by()?.map(|by| *by),
        None => None,
    };
    let closed_by = match &closure {
        Some(c) => c.closed_by()?.map(|by| *by),
        None => None,
    };
    let mut v = json!({
        "id": placement.id().to_base64(),
        "kind": "intention",
        "title": intention.title()?,
        "words": intention.words()?,
        "status": intention.status()?.to_string(),
        "created_by": created_by.to_base64(),
        "created_by_handle": ops::handle_of(graph, created_by).await?,
        "created": creation.created()?,
        "speaker": speaker.to_base64(),
        "speaker_handle": ops::handle_of(graph, speaker).await?,
        "edited_by": edited_by.map(|by| by.to_base64()),
        "edited_by_handle": handle_of(graph, edited_by).await?,
        "edited": edit.as_ref().map(LastEditView::edited).transpose()?,
        "closed_by": closed_by.map(|by| by.to_base64()),
        "closed_by_handle": handle_of(graph, closed_by).await?,
        "closed": closure.as_ref().map(ClosureView::closed).transpose()?.flatten(),
        "closed_reason": closure.as_ref().map(ClosureView::reason).transpose()?.flatten(),
    });
    if let Some(project) = other_project(graph, placement.project_id()?).await? {
        v["project"] = json!(project);
    }
    Ok(v)
}

/// The name of the principal `id` names, when it names one.
async fn handle_of(graph: &ProjectGraph, id: Option<EntityId>) -> Result<Option<String>> {
    Ok(match id {
        Some(id) => Some(ops::handle_of(graph, id).await?),
        None => None,
    })
}

/// An intention for people: its short id, its title, its status and its project when that is not
/// `graph`'s; under it whose words they are and when they were written down, and by whom when that
/// was someone else; then the words, and why it ended when its closure says.
async fn intention_text(graph: &ProjectGraph, record: &(PlacementView, Parts)) -> Result<String> {
    let (placement, (intention, _)) = record;
    let status = intention.status()?;
    let (speaker, day, writer) = spoken(graph, record).await?;
    let mut text = format!(
        "{} {} [{status}]{}\n  @{speaker}, {day}",
        short_id(&placement.id()),
        intention.title()?,
        elsewhere(graph, placement).await?
    );
    if let Some(writer) = writer {
        text.push_str(&format!(", written down by @{writer}"));
    }
    text.push_str(&words_after(&intention.words()?));
    let reason = match graph.component_of::<ClosureView>(placement).await? {
        Some(closure) => closure.reason()?,
        None => None,
    };
    if let Some(reason) = reason {
        text.push_str(&format!("\n  {status}: {reason}"));
    }
    Ok(text)
}

/// ` in <project>` for an intention of another project than `graph`'s, else nothing.
async fn elsewhere(graph: &ProjectGraph, placement: &PlacementView) -> Result<String> {
    Ok(other_project(graph, placement.project_id()?).await?.map(|p| format!(" in {p}")).unwrap_or_default())
}

/// Whose words an intention holds, by name; the day they were written down, the day of their last
/// rewording when they were reworded; and the name of whoever wrote them down, when that was not
/// their speaker.
pub(crate) async fn spoken(
    graph: &ProjectGraph,
    record: &(PlacementView, Parts),
) -> Result<(String, String, Option<String>)> {
    let (placement, (_, creation)) = record;
    let attribution = graph.component_of::<AttributionView>(placement).await?;
    let speaker = intentions::speaker(creation, attribution.as_ref())?;
    let (written, written_by) = match graph.component_of::<LastEditView>(placement).await? {
        Some(edit) => (edit.edited()?, edit.edited_by()?.map(|by| *by)),
        None => (creation.created()?, Some(*creation.created_by()?)),
    };
    let writer = handle_of(graph, written_by.filter(|by| *by != speaker)).await?;
    Ok((ops::handle_of(graph, speaker).await?, date(written), writer))
}

/// `words` after the line that says whose they are: a colon, then each of their lines indented;
/// nothing for no words.
pub(crate) fn words_after(words: &str) -> String {
    if words.is_empty() {
        return String::new();
    }
    let lines: String =
        words.lines().map(|l| if l.is_empty() { "\n".to_string() } else { format!("\n    {l}") }).collect();
    format!(":{lines}")
}
