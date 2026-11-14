//! Intention records: what a person or an agent wants to be true, in their own words, so that an
//! agent reads back from fwd, first of all, the intentions it works under. Each is one entity of
//! the Commons' components, `Intention` and `Creation`, with `Attribution` when someone else wrote
//! the words down, then `LastEdit` once reworded and `Closure` once fulfilled or withdrawn, placed
//! in a project by fwd's `Placement` (`project_graph`). A party's account of how it understands an
//! intention is a reading (`readings`), and what a principal works under is read in one go
//! (`works_under`).

pub mod readings;
pub mod works_under;

use crate::model::PlacementView;
use crate::now_ms;
use crate::project_graph::{InProjectView, ProjectGraph, ProjectTransaction};
use ankurah::entity::LocalTrxEntity;
use ankurah::property::Ref;
use ankurah::EntityId;
use ankurah_commons::identity::Principal as CommonsPrincipal;
use ankurah_commons::intention_graph::{Intention, IntentionStatus, IntentionView};
use ankurah_commons::records::{
    Attribution, AttributionView, Closure, ClosureView, Creation, CreationView, LastEdit, LastEditView,
};
use anyhow::{bail, Result};

/// The components every intention carries, which a list reads it through.
pub type Parts = (IntentionView, CreationView);

/// The principal `id` as a Commons model refers to one (`Creation.created_by`,
/// `Attribution.speaker`, `LastEdit.edited_by`, `Closure.closed_by`): the id names a fwd
/// principal (`model::Principal`), which does not carry the Commons' principal component yet.
pub(crate) fn principal_ref(id: EntityId) -> Ref<CommonsPrincipal> {
    Ref::new(id)
}

/// When a record's words last changed, and who changed them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edit {
    pub at: i64,
    pub by: Option<EntityId>,
}

/// Who wrote a record down and when, and whose words it holds: its `Creation`, with its
/// `Attribution` when someone else wrote them down, and its `LastEdit` when they changed since.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Authorship {
    pub written_by: EntityId,
    pub written: i64,
    /// Whose words they are: the speaker an `Attribution` names, else whoever wrote them down.
    pub speaker: EntityId,
    pub edited: Option<Edit>,
}

impl Authorship {
    /// The authorship of the placed record `placement` is on, whose `Creation` is `creation`.
    pub(crate) async fn of(
        graph: &ProjectGraph,
        placement: &PlacementView,
        creation: &CreationView,
    ) -> Result<Authorship> {
        let written_by = *creation.created_by()?;
        let speaker = match graph.component_of::<AttributionView>(placement).await? {
            Some(a) => *a.speaker()?,
            None => written_by,
        };
        let edited = match graph.component_of::<LastEditView>(placement).await? {
            Some(e) => Some(Edit { at: e.edited()?, by: e.edited_by()?.map(|r| *r) }),
            None => None,
        };
        Ok(Authorship { written_by, written: creation.created()?, speaker, edited })
    }

    /// When the words last changed: their last edit, else when they were written down.
    pub fn changed(&self) -> i64 {
        self.edited.map_or(self.written, |e| e.at)
    }
}

/// When an intention was fulfilled or withdrawn, by whom, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Closed {
    pub at: Option<i64>,
    pub by: Option<EntityId>,
    pub reason: Option<String>,
}

/// An intention as it reads now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntentionRecord {
    pub id: EntityId,
    /// The record of the project it is placed in.
    pub project: EntityId,
    pub title: String,
    pub words: String,
    pub status: IntentionStatus,
    pub authorship: Authorship,
    pub closed: Option<Closed>,
}

impl IntentionRecord {
    /// The intention whose components `placement` and `parts` are.
    pub(crate) async fn read(
        graph: &ProjectGraph,
        placement: &PlacementView,
        (intention, creation): &Parts,
    ) -> Result<IntentionRecord> {
        let closed = match graph.component_of::<ClosureView>(placement).await? {
            Some(c) => Some(Closed { at: c.closed()?, by: c.closed_by()?.map(|r| *r), reason: c.reason()? }),
            None => None,
        };
        Ok(IntentionRecord {
            id: placement.id(),
            project: placement.project_id()?,
            title: intention.title()?,
            words: intention.words()?,
            status: intention.status()?,
            authorship: Authorship::of(graph, placement, creation).await?,
            closed,
        })
    }
}

/// The project's intentions, by id.
pub async fn intentions(graph: &ProjectGraph) -> Result<Vec<IntentionRecord>> {
    let mut out = Vec::new();
    for (placement, parts) in graph.all_placed::<Parts>().await? {
        out.push(IntentionRecord::read(graph, &placement, &parts).await?);
    }
    out.sort_by_key(|i| i.id);
    Ok(out)
}

/// The project's intention `id`.
pub async fn intention(graph: &ProjectGraph, id: EntityId) -> Result<IntentionRecord> {
    let (placement, parts) = graph.get_placed::<Parts>(id).await?;
    IntentionRecord::read(graph, &placement, &parts).await
}

/// What [`create`] makes.
#[derive(Debug, Clone)]
pub struct NewIntention {
    pub title: String,
    pub words: String,
    /// Whose words they are, when the principal writing them down is someone else.
    pub said_by: Option<EntityId>,
    /// Wished or committed: an intention is fulfilled or withdrawn afterwards.
    pub status: IntentionStatus,
}

/// Records an intention in the graph's project, written down by `by`.
pub async fn create(graph: &ProjectGraph, by: EntityId, new: NewIntention) -> Result<EntityId> {
    let title = new.title.trim();
    if title.is_empty() {
        bail!("an intention's title cannot be blank");
    }
    if closing(new.status) {
        bail!("an intention is recorded wished or committed, and fulfilled or withdrawn afterwards");
    }
    let trx = graph.begin();
    let made = trx.create_placed(&Intention { title: title.to_string(), words: new.words, status: new.status }).await?;
    let record = ankurah::model::Mutable::entity(&*made);
    trx.add(record, &Creation { created_by: principal_ref(by), created: now_ms() }).await?;
    if let Some(speaker) = new.said_by.filter(|speaker| *speaker != by) {
        trx.add(record, &Attribution { speaker: principal_ref(speaker) }).await?;
    }
    let id = made.id();
    trx.commit().await?;
    Ok(id)
}

/// Gives the intention `id` a new title, new words, or both, as `by` rewords it; whatever is
/// given as it stands changes nothing, and nothing at all is written when nothing changes. True
/// when it changed.
pub async fn reword(
    graph: &ProjectGraph,
    by: EntityId,
    id: EntityId,
    title: Option<&str>,
    words: Option<&str>,
) -> Result<bool> {
    let (placement, intention) = graph.get_placed::<IntentionView>(id).await?;
    let title = title.map(str::trim);
    if title.is_some_and(str::is_empty) {
        bail!("an intention's title cannot be blank");
    }
    let (old_title, old_words) = (intention.title()?, intention.words()?);
    let new_title = title.filter(|t| *t != old_title);
    let new_words = words.filter(|w| *w != old_words);
    if new_title.is_none() && new_words.is_none() {
        return Ok(false);
    }
    let trx = graph.begin();
    let m = trx.edit_placed::<Intention>(&placement)?;
    if let Some(t) = new_title {
        m.title()?.set(&t.to_string())?;
    }
    if let Some(w) = new_words {
        m.words()?.set(&w.to_string())?;
    }
    record_edit(graph, &trx, &placement, ankurah::model::Mutable::entity(&*m), by).await?;
    trx.commit().await?;
    Ok(true)
}

/// Marks the intention `id` fulfilled or withdrawn, as `by` closes it, with why.
pub async fn close(
    graph: &ProjectGraph,
    by: EntityId,
    id: EntityId,
    status: IntentionStatus,
    reason: Option<String>,
) -> Result<()> {
    if !closing(status) {
        bail!("an intention is closed as fulfilled or withdrawn, not {status}");
    }
    let (placement, intention) = graph.get_placed::<IntentionView>(id).await?;
    let now = intention.status()?;
    if closing(now) {
        bail!("the intention is {now} already");
    }
    let trx = graph.begin();
    let m = trx.edit_placed::<Intention>(&placement)?;
    m.status()?.set(&status)?;
    let closure = Closure { closed: Some(now_ms()), closed_by: Some(principal_ref(by)), reason, reason_message: None };
    trx.add(ankurah::model::Mutable::entity(&*m), &closure).await?;
    Ok(trx.commit().await?)
}

/// Whether `status` is one an intention ends in.
fn closing(status: IntentionStatus) -> bool {
    matches!(status, IntentionStatus::Fulfilled | IntentionStatus::Withdrawn)
}

/// Records in `trx` that `by` changed the words of the placed record `placement` is on, which
/// `record` changes in it: its `LastEdit`, added at its first edit.
pub(crate) async fn record_edit(
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
