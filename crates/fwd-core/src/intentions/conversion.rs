//! `fwd admin migrate`, the part that moves a project's intentions from the Commons' models of
//! 84ea084, whose definitions fwd's node minted for itself (D205), onto the definitions of the
//! Commons' kept mint (D256).

use super::quotation::quote;
use super::{at, principal_ref, Outcome};
use crate::model::{Placement, PlacementView};
use crate::project_graph::ProjectGraph;
use ankurah::model::Mutable;
use ankurah::{EntityId, Model, ModelId, PropertyId, Value, View};
use ankurah_commons::cognition::{Intention, IntentionView};
use ankurah_commons::records::{Attribution, Closed, ClosedView, Created, CreatedView, Edited, EditedView};
use ankurah_commons::text::Quotation;
use anyhow::{bail, Result};
use serde_json::json;

/// The Commons' models of an intention as fwd's node minted them from ankurah/commons 84ea084,
/// before an intention became a description that belongs to no one (D229) and before the Commons
/// renamed the components of a record's course: what [`convert`] reads, and nothing else. Each
/// struct keeps the label, the name and the fields it had then, by which the node binds it to the
/// definition it minted, the only one of its label. Whose words they held is not declared here:
/// the kept mint has a definition of that label too ([`old_speakers`]).
pub mod before_d229 {
    use ankurah::{EntityId, Model, Ref};
    use ankurah_commons::identity::Principal;
    use serde::{Deserialize, Serialize};

    /// A title, the words of whoever wanted it and a status: `open`, `fulfilled` or `withdrawn`,
    /// read as the word it was stored as.
    #[derive(Model, Debug, Clone, Serialize, Deserialize)]
    #[model(label = "intention-graph.intention")]
    #[model(no_ffi)]
    pub struct Intention {
        #[active_type(LWW)]
        pub title: String,
        #[active_type(LWW)]
        pub words: String,
        #[active_type(LWW)]
        pub status: String,
    }

    /// Who wrote the record down, and when.
    #[derive(Model, Debug, Clone, Serialize, Deserialize)]
    #[model(label = "records.creation")]
    #[model(no_ffi)]
    pub struct Creation {
        pub created_by: Ref<Principal>,
        pub created: i64,
    }

    /// When the words last changed, and by whom.
    #[derive(Model, Debug, Clone, Serialize, Deserialize)]
    #[model(label = "records.last_edit")]
    #[model(no_ffi)]
    pub struct LastEdit {
        #[active_type(LWW)]
        pub edited: i64,
        pub edited_by: Option<Ref<Principal>>,
    }

    /// When, by whom and why the record was closed, and the message that said why at more length.
    #[derive(Model, Debug, Clone, Serialize, Deserialize)]
    #[model(label = "records.closure")]
    #[model(no_ffi)]
    pub struct Closure {
        #[active_type(LWW)]
        pub closed: Option<i64>,
        pub closed_by: Option<Ref<Principal>>,
        #[active_type(LWW)]
        pub reason: Option<String>,
        pub reason_message: Option<EntityId>,
    }
}

/// What a quotation the conversion made says it came from, in its relation's `via`: the words an
/// intention held itself until D229.
const MIGRATED_FROM: &str = "intention_words";

/// One intention [`convert`] moved: its id, its title, the labels of the components it gave it,
/// and the quotation it made of its words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Converted {
    pub intention: EntityId,
    pub title: String,
    pub added: Vec<&'static str>,
    pub quotation: EntityId,
}

/// What one intention gets, added to its entity: its new components, and the quotation of its
/// words with whose they are.
struct Additions {
    placement: PlacementView,
    intention: Intention,
    created: Created,
    edited: Option<Edited>,
    closed: Option<Closed>,
    quotation: Quotation,
    recorded: Created,
    speaker: Option<EntityId>,
}

/// Moves the project's intentions onto the Commons' kept mint, as `by` runs the migration, adding
/// to each entity and keeping its id: Ankurah cannot take a component off an entity yet, so the
/// components of 84ea084 stay beside the new ones, and nothing is deleted. Each intention gets
/// `Intention`, its title and its words as the description; `Created`; `Edited` when it was
/// reworded; `Closed` with its outcome and reason when it was fulfilled or withdrawn; and a
/// quotation of its words as last written down, by whom and when, naming their speaker when that
/// was someone else ([`super::quotation`]). Its assignments are relations to its id, so they hold
/// as they were. Every intention is checked before anything is written: one that cannot move
/// without losing something refuses the run, naming each. Then one transaction each, so a run that
/// stops halfway leaves each intention whole, and an intention that carries the new `Intention`
/// has moved, so a second run changes nothing.
pub async fn convert(graph: &ProjectGraph, by: EntityId) -> Result<Vec<Converted>> {
    let speakers = old_speakers(graph).await?;
    let mut moves = Vec::new();
    let mut refused = Vec::new();
    for (placement, old) in graph.all_placed::<before_d229::IntentionView>().await? {
        if graph.component_of::<IntentionView>(&placement).await?.is_some() {
            continue;
        }
        match additions(graph, placement, &old, &speakers).await? {
            Ok(additions) => moves.push(additions),
            Err((id, why)) => refused.push(format!("{} ({why})", id.to_base64())),
        }
    }
    if !refused.is_empty() {
        bail!(
            "no intention was moved onto the Commons' new models, since moving these would lose part of what they hold: {}",
            refused.join("; ")
        );
    }
    let mut converted = Vec::with_capacity(moves.len());
    for m in moves {
        let intention = m.placement.id();
        let trx = graph.begin();
        let placed = trx.edit_placed::<Placement>(&m.placement)?;
        let record = Mutable::entity(&*placed);
        trx.add(record, &m.intention).await?;
        trx.add(record, &m.created).await?;
        let mut added = vec![Intention::descriptor().label, Created::descriptor().label];
        if let Some(edited) = &m.edited {
            trx.add(record, edited).await?;
            added.push(Edited::descriptor().label);
        }
        if let Some(closed) = &m.closed {
            trx.add(record, closed).await?;
            added.push(Closed::descriptor().label);
        }
        let via = json!({"actor_id": by.to_base64(), "migrated": MIGRATED_FROM});
        let quotation = quote(&trx, intention, &m.quotation, &m.recorded, m.speaker, by, via).await?;
        trx.commit().await?;
        converted.push(Converted { intention, title: m.intention.title, added, quotation });
    }
    Ok(converted)
}

/// The definitions of `records.attribution` the node minted for itself, by which a record of
/// 84ea084 said whose words it held, each with its property `speaker`. The kept mint has a
/// definition of that label too, to which the Commons' `Attribution` binds, and a model bound by its
/// label reaches whichever of the two has the lower id, which differs from store to store; so the
/// node's own are read from the catalog, as those of the label that are not the mint's.
async fn old_speakers(graph: &ProjectGraph) -> Result<Vec<(ModelId, PropertyId)>> {
    let descriptor = Attribution::descriptor();
    let minted = descriptor.explicit_id.map(EntityId::from_base64).transpose()?;
    let mut found = Vec::new();
    for model in graph.models_labelled(descriptor.label).await? {
        if Some(model) == minted {
            continue;
        }
        let Some(speaker) = graph.property_named(model, "speaker").await? else {
            bail!("the node's definition {} of `{}` has no property `speaker`", model.to_base64(), descriptor.label);
        };
        found.push((ModelId::EntityId(model), PropertyId::EntityId(speaker)));
    }
    Ok(found)
}

/// Whose words the record `placement` is on held, as its `Attribution` of 84ea084 says: none when
/// it carries none, else the speaker it names, none again for a speaker no one knew; or why it
/// cannot be read.
fn old_speaker(
    placement: &PlacementView,
    speakers: &[(ModelId, PropertyId)],
) -> Result<Option<Option<EntityId>>, String> {
    let entity = placement.entity();
    let mut carried = speakers.iter().filter(|(model, _)| entity.has_membership(model));
    let Some((_, speaker)) = carried.next() else { return Ok(None) };
    if carried.next().is_some() {
        return Err("it carries more than one attribution of 84ea084".into());
    }
    match entity.property_value(speaker) {
        Ok(Some(Value::EntityId(id))) => Ok(Some(Some(id))),
        Ok(None) => Ok(Some(None)),
        Ok(Some(other)) => Err(format!("its attribution names {other:?}, which is no principal's id")),
        Err(e) => Err(format!("its attribution cannot be read: {e}")),
    }
}

/// What the intention `placement` is on gets ([`convert`]), read from `old` and the components
/// of 84ea084 beside it, its speaker by `speakers` ([`old_speakers`]); or its id and why it cannot
/// move without losing part of what it holds.
async fn additions(
    graph: &ProjectGraph,
    placement: PlacementView,
    old: &before_d229::IntentionView,
    speakers: &[(ModelId, PropertyId)],
) -> Result<Result<Additions, (EntityId, String)>> {
    let id = placement.id();
    let refuse = |why: &str| -> Result<Result<Additions, (EntityId, String)>> { Ok(Err((id, why.to_string()))) };
    let Some(creation) = graph.component_of::<before_d229::CreationView>(&placement).await? else {
        return refuse("no record of who wrote it down, or when");
    };
    for (label, carried) in [
        (Created::descriptor().label, graph.component_of::<CreatedView>(&placement).await?.is_some()),
        (Edited::descriptor().label, graph.component_of::<EditedView>(&placement).await?.is_some()),
        (Closed::descriptor().label, graph.component_of::<ClosedView>(&placement).await?.is_some()),
    ] {
        if carried {
            return refuse(&format!("it carries `{label}` already, which the conversion would not overwrite"));
        }
    }
    let last_edit = graph.component_of::<before_d229::LastEditView>(&placement).await?;
    let closure = graph.component_of::<before_d229::ClosureView>(&placement).await?;
    let (written_by, created) = (*creation.created_by()?, creation.created()?);
    // Whose words they are: the speaker the record's `Attribution` names, else whoever wrote it down.
    let speaker = match old_speaker(&placement, speakers) {
        Ok(Some(named)) => named,
        Ok(None) => Some(written_by),
        Err(why) => return refuse(&why),
    };
    // The words as they stand were last written down at their last rewording, else with the record.
    let (last_writer, last_written) = match &last_edit {
        Some(edit) => match edit.edited_by()? {
            Some(editor) => (*editor, edit.edited()?),
            None => return refuse("its last rewording names no one who wrote it down"),
        },
        None => (written_by, created),
    };
    let status = old.status()?;
    let closed = match (status.as_str(), &closure) {
        ("open", None) => None,
        ("open", Some(c)) if c.closed()?.is_none() && c.closed_by()?.is_none() && c.reason()?.is_none() => None,
        ("open", Some(_)) => return refuse("it is open, yet its closure records a closing"),
        (ended, Some(c)) if ended.parse::<Outcome>().is_ok() => {
            if c.reason_message()?.is_some() {
                return refuse("its closure names a message giving the reason, which `Closed` has no place for");
            }
            let Some(when) = c.closed()? else {
                return refuse(&format!("it is {ended}, but its closure records no time"));
            };
            Some(Closed {
                closed: Some(at(when)),
                by: c.closed_by()?.map(|closer| principal_ref(*closer)),
                outcome: Some(ended.to_string()),
                reason: c.reason()?,
            })
        }
        (ended, None) if ended.parse::<Outcome>().is_ok() => return refuse("it has ended, but carries no closure"),
        _ => return refuse(&format!("its status `{status}` is none an intention has")),
    };
    let words = old.words()?;
    Ok(Ok(Additions {
        intention: Intention { title: old.title()?, description: words.clone() },
        created: Created { created: at(created), by: principal_ref(written_by) },
        edited: match &last_edit {
            Some(edit) => {
                Some(Edited { edited: at(edit.edited()?), by: edit.edited_by()?.map(|editor| principal_ref(*editor)) })
            }
            None => None,
        },
        closed,
        // When the words were said is known only where whoever wrote them down gave their own.
        quotation: Quotation {
            text: words,
            said: (speaker == Some(last_writer)).then(|| at(last_written)),
            source: None,
        },
        recorded: Created { created: at(last_written), by: principal_ref(last_writer) },
        speaker,
        placement,
    }))
}
