//! Intention records: what a person or an agent wants to be true, in their own words. Each is one
//! entity of the Commons' components, `Intention` and `Creation`, with `Attribution` when someone
//! else wrote the words down, then `LastEdit` once reworded and `Closure` once fulfilled or
//! withdrawn, placed in a project by fwd's `Placement` (`project_graph`). A read hands out the
//! views of the components every intention carries with its placement, `(PlacementView, Parts)`,
//! and the other components are read through `ProjectGraph::component_of`.

use crate::model::{PlacementView, PrincipalKind, PrincipalView};
use crate::now_ms;
use crate::project_graph::{Followed, ProjectGraph, ProjectTransaction};
use crate::rules;
use ankurah::entity::LocalTrxEntity;
use ankurah::model::{ComponentViews, Mutable};
use ankurah::property::{PropertyError, Ref};
use ankurah::EntityId;
use ankurah_commons::identity::Principal as CommonsPrincipal;
use ankurah_commons::intention_graph::{Intention, IntentionStatus, IntentionView};
use ankurah_commons::records::{Attribution, AttributionView, Closure, Creation, CreationView, LastEdit, LastEditView};
use anyhow::{bail, Result};

/// The components every intention carries, which a read takes it by.
pub type Parts = (IntentionView, CreationView);

/// The principal `id` as a Commons model refers to one (`Creation.created_by`,
/// `Attribution.speaker`, `LastEdit.edited_by`, `Closure.closed_by`): the id names a fwd
/// principal (`model::Principal`), which does not carry the Commons' principal component yet.
pub(crate) fn principal_ref(id: EntityId) -> Ref<CommonsPrincipal> {
    Ref::new(id)
}

/// Whose words an intention holds: the speaker its `Attribution` names, when it carries one, else
/// whoever wrote them down.
pub fn speaker(creation: &CreationView, attribution: Option<&AttributionView>) -> Result<EntityId, PropertyError> {
    Ok(match attribution {
        Some(a) => *a.speaker()?,
        None => *creation.created_by()?,
    })
}

/// Whether an intention of `status` has ended: fulfilled or withdrawn.
pub fn ended(status: IntentionStatus) -> bool {
    matches!(status, IntentionStatus::Fulfilled | IntentionStatus::Withdrawn)
}

/// The principal `said_by` names, with its kind, wherever in the node it is: whose words a write
/// records ([`rules::speaker_of`]).
async fn said_by(graph: &ProjectGraph, said_by: Option<EntityId>) -> Result<Option<(EntityId, PrincipalKind)>> {
    let Some(id) = said_by else { return Ok(None) };
    match graph.follow::<PrincipalView>(id).await? {
        Followed::Held { record, found, .. } => Ok(Some((record, found.kind()?))),
        Followed::NotHeld => bail!("--said-by names no principal the node holds: {}", id.to_base64()),
    }
}

/// Records `intention` in the graph's project, written down by `by`, in the words of the person
/// `said_by` names, else in `by`'s own; the views of the record it made.
pub async fn create(
    graph: &ProjectGraph,
    by: EntityId,
    intention: Intention,
    said_by: Option<EntityId>,
) -> Result<(PlacementView, Parts)> {
    let title = intention.title.trim().to_string();
    if title.is_empty() {
        bail!("an intention's title cannot be blank");
    }
    if ended(intention.status) {
        bail!("an intention is recorded wished or committed, and fulfilled or withdrawn afterwards");
    }
    let speaker = rules::speaker_of(by, self::said_by(graph, said_by).await?)?;
    let trx = graph.begin();
    let made = trx.create_placed(&Intention { title, ..intention }).await?;
    let record = Mutable::entity(&*made);
    trx.add(record, &Creation { created_by: principal_ref(by), created: now_ms() }).await?;
    if speaker != by {
        trx.add(record, &Attribution { speaker: principal_ref(speaker) }).await?;
    }
    // Read through the transaction, the views follow the record once it is committed.
    let views = <(PlacementView, Parts)>::views_of(record.read())?;
    trx.commit().await?;
    Ok(views)
}

/// Gives the intention `record` a new title, new words, or both, as `by` rewords it, in its
/// speaker's words: `by`'s own, or those of the person `said_by` names
/// ([`rules::can_change_words`]). One fulfilled or withdrawn is not reworded. Whatever is given as
/// it stands changes nothing, and nothing at all is written when nothing changes. True when it
/// changed.
pub async fn reword(
    graph: &ProjectGraph,
    by: EntityId,
    (placement, (intention, creation)): &(PlacementView, Parts),
    title: Option<&str>,
    words: Option<&str>,
    said_by: Option<EntityId>,
) -> Result<bool> {
    let status = intention.status()?;
    if ended(status) {
        bail!("the intention is {status}, and is not reworded");
    }
    let attribution = graph.component_of::<AttributionView>(placement).await?;
    let written_for = rules::speaker_of(by, self::said_by(graph, said_by).await?)?;
    rules::can_change_words(speaker(creation, attribution.as_ref())?, written_for)?;
    let title = title.map(str::trim);
    if title.is_some_and(str::is_empty) {
        bail!("an intention's title cannot be blank");
    }
    let (now_title, now_words) = (intention.title()?, intention.words()?);
    let new_title = title.filter(|t| *t != now_title);
    let new_words = words.filter(|w| *w != now_words);
    if new_title.is_none() && new_words.is_none() {
        return Ok(false);
    }
    let trx = graph.begin();
    let m = trx.edit_placed::<Intention>(placement)?;
    if let Some(t) = new_title {
        m.title()?.set(&t.to_string())?;
    }
    if let Some(w) = new_words {
        m.words()?.set(&w.to_string())?;
    }
    record_edit(graph, &trx, placement, Mutable::entity(&*m), by).await?;
    trx.commit().await?;
    Ok(true)
}

/// Marks the intention `record` fulfilled or withdrawn, as `by` closes it in its speaker's words,
/// as [`reword`] takes them, with why: a withdrawal says why, as a task's cancellation does.
pub async fn close(
    graph: &ProjectGraph,
    by: EntityId,
    (placement, (intention, creation)): &(PlacementView, Parts),
    status: IntentionStatus,
    reason: Option<String>,
    said_by: Option<EntityId>,
) -> Result<()> {
    if !ended(status) {
        bail!("an intention is closed as fulfilled or withdrawn, not {status}");
    }
    if status == IntentionStatus::Withdrawn && reason.as_deref().is_none_or(|r| r.trim().is_empty()) {
        bail!("an intention is withdrawn with a reason: give --reason");
    }
    let now = intention.status()?;
    if ended(now) {
        bail!("the intention is {now} already");
    }
    let attribution = graph.component_of::<AttributionView>(placement).await?;
    let written_for = rules::speaker_of(by, self::said_by(graph, said_by).await?)?;
    rules::can_change_words(speaker(creation, attribution.as_ref())?, written_for)?;
    let trx = graph.begin();
    let m = trx.edit_placed::<Intention>(placement)?;
    m.status()?.set(&status)?;
    let closure = Closure { closed: Some(now_ms()), closed_by: Some(principal_ref(by)), reason, reason_message: None };
    trx.add(Mutable::entity(&*m), &closure).await?;
    Ok(trx.commit().await?)
}

/// Records in `trx` that `by` changed the words of the placed record `placement` is on, which
/// `record` changes in it: its `LastEdit`, added at its first edit.
async fn record_edit(
    graph: &ProjectGraph,
    trx: &ProjectTransaction,
    placement: &PlacementView,
    record: &LocalTrxEntity,
    by: EntityId,
) -> Result<()> {
    let edit = LastEdit { edited: now_ms(), edited_by: Some(principal_ref(by)) };
    match graph.component_of::<LastEditView>(placement).await? {
        Some(_) => {
            let m = trx.edit_placed::<LastEdit>(placement)?;
            m.edited()?.set(&edit.edited)?;
            m.edited_by()?.set(&edit.edited_by)?;
        }
        None => trx.add(record, &edit).await?,
    }
    Ok(())
}
