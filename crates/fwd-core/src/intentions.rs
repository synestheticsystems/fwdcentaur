//! Intention records: a statement of something wanted, a title and a description that belong to no
//! one (D229), built on the Commons' models. Each is one entity of the components `Intention` and
//! `Created`, then `Edited` once reworded and `Closed` once fulfilled or withdrawn, placed in a
//! project by fwd's `Placement` (`project_graph`). Whose words a description holds is a quotation
//! beside it (`quotation`). A read hands out the views of the components every intention carries
//! with its placement, `(PlacementView, Parts)`, and the other components are read through
//! `ProjectGraph::component_of`. An intention is assigned to a principal, or to several, by an
//! `assigned_to` relation for each, which lives with the intention in its project (D218, D219).

mod quotation;

pub use quotation::{quotation_of, speaker, whose_words, Quoted};

use crate::model::{PlacementView, PrincipalKind, PrincipalView, RelationKind, RelationStatus, RelationView};
use crate::now_ms;
use crate::ops;
use crate::project_graph::{Followed, ProjectGraph, ProjectTransaction};
use crate::rules;
use ankurah::entity::LocalTrxEntity;
use ankurah::model::{ComponentViews, Mutable};
use ankurah::property::{PropertyError, Ref};
use ankurah::EntityId;
use ankurah_commons::cognition::{Intention, IntentionView};
use ankurah_commons::identity::Principal as CommonsPrincipal;
use ankurah_commons::property::Timestamp;
use ankurah_commons::records::{Closed, ClosedView, Created, CreatedView, Edited, EditedView};
use anyhow::{bail, Result};
use serde_json::json;
use std::collections::HashSet;

/// The components every intention carries, which a read takes it by.
pub type Parts = (IntentionView, CreatedView);

/// How an intention ended, as its `Closed.outcome` says. The Commons keep no status on an
/// intention: it is open until it is closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, strum::EnumString, strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub enum Outcome {
    Fulfilled,
    Withdrawn,
}

/// How the intention whose `Closed` is `closed` ended: none while it is open, which it is until a
/// closing gives `Closed` a time, and again once a reopening takes the time off. A closed intention
/// whose outcome is neither word cannot be read as one.
pub fn outcome(closed: Option<&ClosedView>) -> Result<Option<Outcome>, PropertyError> {
    let Some(closed) = closed else { return Ok(None) };
    if closed.closed()?.is_none() {
        return Ok(None);
    }
    let word = closed.outcome()?.unwrap_or_default();
    match word.parse() {
        Ok(outcome) => Ok(Some(outcome)),
        Err(_) => Err(PropertyError::InvalidValue { value: word, ty: "an intention's outcome".into() }),
    }
}

/// Where an intention stands, as fwd prints it: `open`, else how it ended.
pub fn status(outcome: Option<Outcome>) -> &'static str {
    outcome.map_or("open", Into::into)
}

/// The principal `id` as a Commons model refers to one (`Created.by`, `Edited.by`, `Closed.by`,
/// `Attribution.speaker`): the id names a fwd principal (`model::Principal`), which does not carry
/// the Commons' principal component yet.
pub(crate) fn principal_ref(id: EntityId) -> Ref<CommonsPrincipal> {
    Ref::new(id)
}

/// `millis` since the Unix epoch, as the Commons' models hold a moment.
fn at(millis: i64) -> Timestamp {
    Timestamp::from_unix_millis(millis)
}

/// The principal `said_by` names, with its kind, wherever in the node it is: whose words a write
/// records ([`rules::speaker_of`]).
async fn followed_principal(
    graph: &ProjectGraph,
    said_by: Option<EntityId>,
) -> Result<Option<(EntityId, PrincipalKind)>> {
    let Some(id) = said_by else { return Ok(None) };
    match graph.follow::<PrincipalView>(id).await? {
        Followed::Held { record, found, .. } => Ok(Some((record, found.kind()?))),
        Followed::NotHeld => bail!("--said-by names no principal the node holds: {}", id.to_base64()),
    }
}

/// Records an open intention titled `title` in the graph's project, written down by `by`, its
/// description `words` quoted as the words of the person `said_by` names, else as `by`'s own; the
/// views of the intention it made.
pub async fn create(
    graph: &ProjectGraph,
    by: EntityId,
    title: &str,
    words: String,
    said_by: Option<EntityId>,
) -> Result<(PlacementView, Parts)> {
    let title = title.trim().to_string();
    if title.is_empty() {
        bail!("an intention's title cannot be blank");
    }
    let speaker = rules::speaker_of(by, followed_principal(graph, said_by).await?)?;
    let now = now_ms();
    let trx = graph.begin();
    let made = trx.create_placed(&Intention { title, description: words.clone() }).await?;
    let record = Mutable::entity(&*made);
    trx.add(record, &Created { created: at(now), by: principal_ref(by) }).await?;
    // Read through the transaction, the views follow the record once it is committed.
    let views = <(PlacementView, Parts)>::views_of(record.read())?;
    quotation::quote_given(&trx, record.id(), words, speaker, by, now).await?;
    trx.commit().await?;
    Ok(views)
}

/// Refuses a change to the intention `placement` is on unless it is written in its speaker's words
/// ([`rules::can_change_words`]): `by`'s own, or those of the person `said_by` names. An intention
/// no quotation names a speaker for is refused, since no one is known to change its words. Whose
/// words the change records.
async fn in_speakers_words(
    graph: &ProjectGraph,
    placement: &PlacementView,
    by: EntityId,
    said_by: Option<EntityId>,
) -> Result<EntityId> {
    let written_for = rules::speaker_of(by, followed_principal(graph, said_by).await?)?;
    let Some((Some(speaker), _)) = whose_words(graph, placement.id()).await? else {
        bail!("no quotation says whose words the intention holds, so no one is known to change them");
    };
    rules::can_change_words(speaker, written_for)?;
    Ok(written_for)
}

/// Gives the intention a new title, new words, or both, as `by` rewords it, in its speaker's
/// words ([`in_speakers_words`]): new words are quoted as theirs, beside the quotations of the words
/// before. One fulfilled or withdrawn is not reworded. Whatever is given as it stands changes
/// nothing, and nothing at all is written when nothing changes. True when it changed.
pub async fn reword(
    graph: &ProjectGraph,
    by: EntityId,
    (placement, (intention, _)): &(PlacementView, Parts),
    title: Option<&str>,
    words: Option<&str>,
    said_by: Option<EntityId>,
) -> Result<bool> {
    if let Some(ended) = outcome(graph.component_of::<ClosedView>(placement).await?.as_ref())? {
        bail!("the intention is {ended}, and is not reworded");
    }
    let written_for = in_speakers_words(graph, placement, by, said_by).await?;
    let title = title.map(str::trim);
    if title.is_some_and(str::is_empty) {
        bail!("an intention's title cannot be blank");
    }
    let (now_title, now_words) = (intention.title()?, intention.description()?);
    let new_title = title.filter(|t| *t != now_title);
    let new_words = words.filter(|w| *w != now_words);
    if new_title.is_none() && new_words.is_none() {
        return Ok(false);
    }
    let now = now_ms();
    let trx = graph.begin();
    let m = trx.edit_placed::<Intention>(placement)?;
    if let Some(t) = new_title {
        m.title()?.set(&t.to_string())?;
    }
    if let Some(w) = new_words {
        m.description()?.set(&w.to_string())?;
        quotation::quote_given(&trx, placement.id(), w.to_string(), written_for, by, now).await?;
    }
    record_edit(graph, &trx, placement, Mutable::entity(&*m), by, now).await?;
    trx.commit().await?;
    Ok(true)
}

/// Closes the intention as fulfilled or withdrawn, as `by` closes it in its speaker's words, as
/// [`reword`] takes them, with why: a withdrawal says why, as a task's cancellation does.
pub async fn close(
    graph: &ProjectGraph,
    by: EntityId,
    (placement, _): &(PlacementView, Parts),
    ending: Outcome,
    reason: Option<String>,
    said_by: Option<EntityId>,
) -> Result<()> {
    if ending == Outcome::Withdrawn && reason.as_deref().is_none_or(|r| r.trim().is_empty()) {
        bail!("an intention is withdrawn with a reason: give --reason");
    }
    if let Some(ended) = outcome(graph.component_of::<ClosedView>(placement).await?.as_ref())? {
        bail!("the intention is {ended} already");
    }
    in_speakers_words(graph, placement, by, said_by).await?;
    let trx = graph.begin();
    let m = trx.edit_placed::<Intention>(placement)?;
    let closed =
        Closed { closed: Some(at(now_ms())), by: Some(principal_ref(by)), outcome: Some(ending.to_string()), reason };
    trx.add(Mutable::entity(&*m), &closed).await?;
    Ok(trx.commit().await?)
}

/// Records in `trx` that `by` changed the words of the placed record `placement` is on at `now`,
/// which `record` changes in it: its `Edited`, added at its first edit.
async fn record_edit(
    graph: &ProjectGraph,
    trx: &ProjectTransaction,
    placement: &PlacementView,
    record: &LocalTrxEntity,
    by: EntityId,
    now: i64,
) -> Result<()> {
    let edit = Edited { edited: at(now), by: Some(principal_ref(by)) };
    match graph.component_of::<EditedView>(placement).await? {
        Some(_) => {
            let m = trx.edit_placed::<Edited>(placement)?;
            m.edited()?.set(&edit.edited)?;
            m.by()?.set(&edit.by)?;
        }
        None => trx.add(record, &edit).await?,
    }
    Ok(())
}

/// Assigns the intention to the principal `to`, as `by` does: an accepted `assigned_to` relation
/// from the intention to the principal, made in the intention's project, where it lives,
/// whichever project the command runs in. One that holds already is the answer, so that assigning
/// twice keeps one. A principal assigns an intention to itself alone: nothing yet tells another
/// that an intention was assigned to it, as a task's assignee is told.
pub async fn assign(
    graph: &ProjectGraph,
    by: EntityId,
    (placement, _): &(PlacementView, Parts),
    to: EntityId,
) -> Result<RelationView> {
    if to != by {
        bail!(
            "refused: an intention is assigned to whoever assigns it, since nothing yet tells another principal that one was assigned to them; leave out --to"
        );
    }
    let intention = placement.id();
    if let Some(holding) = assignments(graph, intention, to).await?.into_iter().next() {
        return Ok(holding);
    }
    let trx = graph.graph_of(placement)?.begin();
    let via = json!({"actor_id": by.to_base64()});
    let ends = (intention, to);
    let relation =
        ops::new_relation(trx.project(), by, RelationKind::AssignedTo, ends, RelationStatus::Accepted, via, None, None);
    // Read through the transaction, the view follows the relation once it is committed.
    let made = trx.create(&relation).await?.read()?;
    trx.commit().await?;
    Ok(made)
}

/// Takes the intention off the principal `to`, as `by` does: retracts each accepted `assigned_to`
/// from the intention to the principal, in the intention's project, where they live.
/// Anyone takes an intention off themselves and a person takes one off anyone, but an agent taking
/// one off another principal is refused. How many it retracted, none when the intention was not
/// assigned to them.
pub async fn unassign(
    graph: &ProjectGraph,
    by: EntityId,
    (placement, _): &(PlacementView, Parts),
    to: EntityId,
) -> Result<usize> {
    if to != by && graph.get::<PrincipalView>(by).await?.kind()? != PrincipalKind::Human {
        bail!(
            "refused: an agent takes an intention off itself alone, since whether one stays assigned to another principal is for that principal or a person to decide; leave out --to"
        );
    }
    let holding = assignments(graph, placement.id(), to).await?;
    if holding.is_empty() {
        return Ok(0);
    }
    let trx = graph.graph_of(placement)?.begin();
    for r in &holding {
        let m = trx.edit(r)?;
        m.status()?.set(&RelationStatus::Retracted)?;
        m.closed_by()?.set(&Some(Ref::new(by)))?;
        m.closed()?.set(&Some(now_ms()))?;
    }
    trx.commit().await?;
    Ok(holding.len())
}

/// The accepted `assigned_to` relations from the intention `intention` to the principal `to`.
async fn assignments(graph: &ProjectGraph, intention: EntityId, to: EntityId) -> Result<Vec<RelationView>> {
    let mut holding = Vec::new();
    for r in graph.relations_from(intention, Some(RelationKind::AssignedTo)).await? {
        if r.to_id()? == to && rules::holds(RelationKind::AssignedTo, r.status()?) {
            holding.push(r);
        }
    }
    Ok(holding)
}

/// The intentions assigned to `principal`, of any project: the intention each of its accepted
/// `assigned_to` relations names, by that id, followed wherever it is, the first assigned first,
/// and each once however many relations assign it. Those since fulfilled or withdrawn are among
/// them, so that whoever holds them can take them off. A relation whose status, time or
/// intention cannot be read comes after them, by its own id, as one that cannot be followed, so
/// that one unreadable value leaves the rest of the read standing.
pub async fn assigned_to(
    graph: &ProjectGraph,
    principal: EntityId,
) -> Result<Vec<(EntityId, Followed<(PlacementView, Parts)>)>> {
    let (mut holding, mut unreadable) = (Vec::new(), Vec::new());
    for r in graph.relations_to(principal, Some(RelationKind::AssignedTo)).await? {
        let assignment = || -> Result<Option<(i64, EntityId)>, PropertyError> {
            if !rules::holds(RelationKind::AssignedTo, r.status()?) {
                return Ok(None);
            }
            Ok(Some((r.created()?, r.from_id()?)))
        };
        match assignment() {
            Ok(Some((created, intention))) => holding.push((created, r.id(), intention)),
            Ok(None) => {}
            Err(_) => unreadable.push(r.id()),
        }
    }
    holding.sort();
    let mut listed = HashSet::new();
    let mut assigned = Vec::new();
    for (_, _, intention) in holding {
        if listed.insert(intention) {
            assigned.push((intention, graph.follow_placed::<Parts>(intention).await?));
        }
    }
    assigned.extend(unreadable.into_iter().map(|relation| (relation, Followed::NotHeld)));
    Ok(assigned)
}
