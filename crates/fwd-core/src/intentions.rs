//! Intention records: what a person or an agent wants to be true, in their own words, so that an
//! agent reads back from fwd, first of all, the intentions it works under. Each is one entity of
//! the Commons' components, `Intention` and `Creation`, with `Attribution` when someone else wrote
//! the words down, then `LastEdit` once reworded and `Closure` once fulfilled or withdrawn, placed
//! in a project by fwd's `Placement` (`project_graph`). A party's account of how it understands an
//! intention is a reading (`readings`), and what a principal works under is read in one go
//! (`works_under`).

pub mod readings;
pub mod works_under;

use crate::model::{PlacementView, PrincipalKind, PrincipalView};
use crate::now_ms;
use crate::project_graph::{Followed, InProjectView, ProjectGraph, ProjectTransaction};
use crate::rules;
use ankurah::entity::LocalTrxEntity;
use ankurah::property::Ref;
use ankurah::EntityId;
use ankurah_commons::identity::Principal as CommonsPrincipal;
use ankurah_commons::intention_graph::{Intention, IntentionStatus, IntentionView};
use ankurah_commons::records::{
    Attribution, AttributionView, Closure, ClosureView, Creation, CreationView, LastEdit, LastEditView,
};
use anyhow::{bail, Result};
use sha2::{Digest, Sha256};

/// The components every intention carries, which a list reads it through.
pub type Parts = (IntentionView, CreationView);

/// The principal `id` as a Commons model refers to one (`Creation.created_by`,
/// `Attribution.speaker`, `LastEdit.edited_by`, `Closure.closed_by`): the id names a fwd
/// principal (`model::Principal`), which does not carry the Commons' principal component yet.
pub(crate) fn principal_ref(id: EntityId) -> Ref<CommonsPrincipal> {
    Ref::new(id)
}

/// How many bytes of the SHA-256 a revision gives, as hex: enough to tell one wording from the
/// next, short enough to pass back on a command line.
const REVISION_BYTES: usize = 6;

/// The revision of a wording made of `parts`: the start of the SHA-256 of each part's length and
/// bytes in turn, so that no two lists of parts run together into one.
pub fn revision(parts: &[&str]) -> String {
    let mut hash = Sha256::new();
    for part in parts {
        hash.update((part.len() as u64).to_le_bytes());
        hash.update(part.as_bytes());
    }
    hash.finalize()[..REVISION_BYTES].iter().map(|b| format!("{b:02x}")).collect()
}

/// The principal `said_by` names, with its kind, wherever in the node it is: whose words a write
/// records ([`rules::party_of`]).
pub(crate) async fn said_by(
    graph: &ProjectGraph,
    said_by: Option<EntityId>,
) -> Result<Option<(EntityId, PrincipalKind)>> {
    let Some(id) = said_by else { return Ok(None) };
    match graph.follow::<PrincipalView>(id).await? {
        Followed::Held { record, found, .. } => Ok(Some((record, found.kind()?))),
        Followed::NotHeld => bail!("--said-by names no principal the node holds: {}", id.to_base64()),
    }
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
    /// [`revision`] of its title and words, which a reading of it records as the wording it read.
    pub revision: String,
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
        let (title, words) = (intention.title()?, intention.words()?);
        Ok(IntentionRecord {
            id: placement.id(),
            project: placement.project_id()?,
            revision: revision(&[&title, &words]),
            title,
            words,
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
    /// Whose words they are, a person's, when the principal writing them down is someone else.
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
    let speaker = rules::party_of(by, said_by(graph, new.said_by).await?)?;
    let trx = graph.begin();
    let made = trx.create_placed(&Intention { title: title.to_string(), words: new.words, status: new.status }).await?;
    let record = ankurah::model::Mutable::entity(&*made);
    trx.add(record, &Creation { created_by: principal_ref(by), created: now_ms() }).await?;
    if speaker != by {
        trx.add(record, &Attribution { speaker: principal_ref(speaker) }).await?;
    }
    let id = made.id();
    trx.commit().await?;
    Ok(id)
}

/// Gives the intention `id` a new title, new words, or both, as `by` rewords it, in its speaker's
/// words: `by`'s own, or those of the person `said_by` names ([`rules::can_change_words`]). One
/// fulfilled or withdrawn is not reworded. Whatever is given as it stands changes nothing, and
/// nothing at all is written when nothing changes. True when it changed.
pub async fn reword(
    graph: &ProjectGraph,
    by: EntityId,
    id: EntityId,
    title: Option<&str>,
    words: Option<&str>,
    said_by: Option<EntityId>,
) -> Result<bool> {
    let (placement, parts) = graph.get_placed::<Parts>(id).await?;
    let intention = IntentionRecord::read(graph, &placement, &parts).await?;
    if closing(intention.status) {
        bail!("the intention is {}, and is not reworded", intention.status);
    }
    rules::can_change_words(intention.authorship.speaker, rules::party_of(by, self::said_by(graph, said_by).await?)?)?;
    let title = title.map(str::trim);
    if title.is_some_and(str::is_empty) {
        bail!("an intention's title cannot be blank");
    }
    let new_title = title.filter(|t| *t != intention.title);
    let new_words = words.filter(|w| *w != intention.words);
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

/// Marks the intention `id` fulfilled or withdrawn, as `by` closes it in its speaker's words, as
/// [`reword`] takes them, with why: a withdrawal says why, as a task's cancellation does.
pub async fn close(
    graph: &ProjectGraph,
    by: EntityId,
    id: EntityId,
    status: IntentionStatus,
    reason: Option<String>,
    said_by: Option<EntityId>,
) -> Result<()> {
    if !closing(status) {
        bail!("an intention is closed as fulfilled or withdrawn, not {status}");
    }
    if status == IntentionStatus::Withdrawn && reason.as_deref().is_none_or(|r| r.trim().is_empty()) {
        bail!("an intention is withdrawn with a reason: give --reason");
    }
    let (placement, parts) = graph.get_placed::<Parts>(id).await?;
    let intention = IntentionRecord::read(graph, &placement, &parts).await?;
    if closing(intention.status) {
        bail!("the intention is {} already", intention.status);
    }
    rules::can_change_words(intention.authorship.speaker, rules::party_of(by, self::said_by(graph, said_by).await?)?)?;
    let trx = graph.begin();
    let m = trx.edit_placed::<Intention>(&placement)?;
    m.status()?.set(&status)?;
    let closure = Closure { closed: Some(now_ms()), closed_by: Some(principal_ref(by)), reason, reason_message: None };
    trx.add(ankurah::model::Mutable::entity(&*m), &closure).await?;
    Ok(trx.commit().await?)
}

/// Whether `status` is one an intention ends in.
pub(crate) fn closing(status: IntentionStatus) -> bool {
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

#[cfg(test)]
mod tests {
    use super::revision;

    #[test]
    fn a_revision_tells_apart_parts_that_would_run_together_with_a_separator() {
        assert_ne!(revision(&["a\n", "b"]), revision(&["a", "\nb"]));
        assert_eq!(revision(&["a", "b"]).len(), 12);
        assert_eq!(revision(&["a", "b"]), revision(&["a", "b"]));
    }
}
