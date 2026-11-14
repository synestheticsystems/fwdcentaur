//! Readings: one party's account, in its own words, of how it understands an intention or
//! another reading, with where it stands on it (the Commons' `Reading`, with `Creation`, and
//! `Attribution` when written down for someone else). A party has one reading of a record, which
//! a later one rewords. The speaker of an intention confirms an agent's reading of it with a
//! reading of that reading, stance agree; a reading of a reading is also how a person corrects an
//! agent's account in their own words. A reading is placed in the project of the command that
//! writes it, and a record's readings are read wherever they were placed.

use super::{principal_ref, record_edit, Authorship, IntentionRecord, Parts};
use crate::model::PlacementView;
use crate::now_ms;
use crate::project_graph::{Followed, InProjectView, ProjectGraph};
use ankurah::EntityId;
use ankurah_commons::intention_graph::{Reading, ReadingView, Stance};
use ankurah_commons::records::{Attribution, Creation, CreationView};
use anyhow::{anyhow, bail, Result};
use sha2::{Digest, Sha256};

/// A reading as it reads now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadingRecord {
    pub id: EntityId,
    /// The record of the project it is placed in.
    pub project: EntityId,
    /// The intention or reading it reads.
    pub of: EntityId,
    pub words: String,
    pub stance: Option<Stance>,
    /// Who wrote it, and whose account it is: the party, [`Authorship::speaker`].
    pub authorship: Authorship,
    /// [`revision`] of its stance and words.
    pub revision: String,
}

impl ReadingRecord {
    async fn read(graph: &ProjectGraph, placement: &PlacementView, reading: &ReadingView) -> Result<ReadingRecord> {
        let creation = graph
            .component_of::<CreationView>(placement)
            .await?
            .ok_or_else(|| anyhow!("the reading {} carries no record of its creation", placement.id().to_base64()))?;
        let (words, stance) = (reading.words()?, reading.stance()?);
        Ok(ReadingRecord {
            id: placement.id(),
            project: placement.project_id()?,
            of: reading.of()?,
            revision: revision(stance, &words),
            words,
            stance,
            authorship: Authorship::of(graph, placement, &creation).await?,
        })
    }

    /// Whose account it is.
    pub fn party(&self) -> EntityId {
        self.authorship.speaker
    }
}

/// How many bytes of a reading's SHA-256 its revision gives, as hex: enough to tell one wording
/// from the next, short enough to pass back on a command line.
const REVISION_BYTES: usize = 6;

/// What a reader passes back to say which words it read (`--as-read`): the start of the SHA-256 of
/// the stance and the words, so that it changes whenever either does and not when an edit leaves
/// both as they were.
pub fn revision(stance: Option<Stance>, words: &str) -> String {
    let stance = stance.map(|s| s.to_string()).unwrap_or_default();
    let digest = Sha256::digest(format!("{stance}\n{words}").as_bytes());
    digest[..REVISION_BYTES].iter().map(|b| format!("{b:02x}")).collect()
}

/// A reading changed since the revision its reader passed: what the reader read is not what stands.
#[derive(Debug, thiserror::Error)]
#[error("the reading {} has changed since you read it (you read {read}, it is now {now}): read it again", .reading.to_base64())]
pub struct ReadingChanged {
    pub reading: EntityId,
    pub read: String,
    pub now: String,
}

/// The readings of the record `of`, wherever they were placed, by id.
pub async fn readings_of(graph: &ProjectGraph, of: EntityId) -> Result<Vec<ReadingRecord>> {
    let mut out = Vec::new();
    for (placement, reading) in graph.readings_of(of).await? {
        out.push(ReadingRecord::read(graph, &placement, &reading).await?);
    }
    out.sort_by_key(|r| r.id);
    Ok(out)
}

/// A record a reading may read: an intention, or another reading.
#[derive(Debug, Clone)]
pub enum Readable {
    Intention(IntentionRecord),
    Reading(ReadingRecord),
}

/// The intention or reading `id`, wherever it is placed; an id that names neither is refused.
pub async fn read_record(graph: &ProjectGraph, id: EntityId) -> Result<Readable> {
    if let Followed::Held { found: (placement, parts), .. } = graph.follow_placed::<Parts>(id).await? {
        return Ok(Readable::Intention(IntentionRecord::read(graph, &placement, &parts).await?));
    }
    if let Followed::Held { found: (placement, reading), .. } = graph.follow_placed::<ReadingView>(id).await? {
        return Ok(Readable::Reading(ReadingRecord::read(graph, &placement, &reading).await?));
    }
    bail!("a reading reads an intention or another reading, and {} is neither", id.to_base64())
}

/// What [`set`] writes.
#[derive(Debug, Clone)]
pub struct SetReading {
    /// The intention or reading read.
    pub of: EntityId,
    pub words: String,
    pub stance: Stance,
    /// Whose account it is, when the principal writing it down is someone else.
    pub said_by: Option<EntityId>,
    /// The [`revision`] of the reading read, which must be what stands: required when `of` is a
    /// reading, refused when it is an intention.
    pub as_read: Option<String>,
}

/// Writes the party's reading of `spec.of` in the graph's project, as `by` writes it down: its
/// first, or its next words and stance in place of those it had, with when and by whom they
/// changed; a reading that stands as given is left as it is. Its id.
pub async fn set(graph: &ProjectGraph, by: EntityId, spec: SetReading) -> Result<EntityId> {
    match (read_record(graph, spec.of).await?, spec.as_read.as_deref()) {
        (Readable::Reading(read), Some(revision)) if read.revision != revision => {
            return Err(ReadingChanged { reading: read.id, read: revision.to_string(), now: read.revision }.into())
        }
        (Readable::Reading(read), None) => bail!(
            "a reading of a reading takes the revision read, so that it answers the words that stand: pass --as-read {}",
            read.revision
        ),
        (Readable::Intention(_), Some(_)) => bail!("--as-read is the revision of a reading read, and this reads an intention"),
        _ => {}
    }
    let party = spec.said_by.unwrap_or(by);
    let existing: Vec<ReadingRecord> =
        readings_of(graph, spec.of).await?.into_iter().filter(|r| r.party() == party).collect();
    match existing.as_slice() {
        [] => {
            let trx = graph.begin();
            let reading = Reading { of: spec.of, words: spec.words, stance: Some(spec.stance) };
            let made = trx.create_placed(&reading).await?;
            let record = ankurah::model::Mutable::entity(&*made);
            trx.add(record, &Creation { created_by: principal_ref(by), created: now_ms() }).await?;
            if party != by {
                trx.add(record, &Attribution { speaker: principal_ref(party) }).await?;
            }
            let id = made.id();
            trx.commit().await?;
            Ok(id)
        }
        [one] if one.words == spec.words && one.stance == Some(spec.stance) => Ok(one.id),
        [one] => {
            let (placement, _) = graph.get_placed::<ReadingView>(one.id).await?;
            let trx = graph.begin();
            let m = trx.edit_placed::<Reading>(&placement)?;
            m.words()?.set(&spec.words)?;
            m.stance()?.set(&Some(spec.stance))?;
            record_edit(graph, &trx, &placement, ankurah::model::Mutable::entity(&*m), by).await?;
            trx.commit().await?;
            Ok(one.id)
        }
        several => bail!(
            "the record {} has {} readings by one party ({}), where a party has one: say which stands",
            spec.of.to_base64(),
            several.len(),
            several.iter().map(|r| r.id.to_base64()).collect::<Vec<_>>().join(", ")
        ),
    }
}

/// Confirms the reading `reading`, which reads an intention, in the words of the intention's
/// speaker, who alone confirms it: a reading of it with stance agree, as `by` writes it down.
/// `words` are the speaker's own, none when the speaker confirms it themselves; an agent that
/// records a speaker's confirmation (`said_by`) gives their words. `as_read` is the revision read.
pub async fn confirm(
    graph: &ProjectGraph,
    by: EntityId,
    reading: EntityId,
    as_read: String,
    words: Option<String>,
    said_by: Option<EntityId>,
) -> Result<EntityId> {
    let Readable::Reading(read) = read_record(graph, reading).await? else {
        bail!("only a reading is confirmed, and {} is an intention", reading.to_base64())
    };
    let Readable::Intention(intention) = read_record(graph, read.of).await? else {
        bail!("only a reading of an intention is confirmed, and {} reads a reading", reading.to_base64())
    };
    let words = words.unwrap_or_default();
    if said_by.is_some() && words.trim().is_empty() {
        bail!("a confirmation written down for someone else carries their own words: give --words");
    }
    let speaker = intention.authorship.speaker;
    if said_by.unwrap_or(by) != speaker {
        bail!(
            "only the intention's speaker, {}, confirms a reading of it: another's agreement is a reading like any other (`fwd reading set`)",
            crate::ops::handle_of(graph, speaker).await?
        );
    }
    set(graph, by, SetReading { of: reading, words, stance: Stance::Agree, said_by, as_read: Some(as_read) }).await
}

/// That a speaker confirmed a reading: by its own reading of it, with stance agree, when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirmation {
    pub reading: EntityId,
    pub by: EntityId,
    pub at: i64,
}

/// The confirmation of `reading` among `its_readings`, the readings of it: the latest by `speaker`
/// with stance agree, when it is not older than the reading's own last change. None when the
/// speaker has not agreed, or agreed with words the reading has changed since.
pub fn confirmation(
    reading: &ReadingRecord,
    speaker: EntityId,
    its_readings: &[ReadingRecord],
) -> Option<Confirmation> {
    its_readings
        .iter()
        .filter(|r| r.party() == speaker && r.stance == Some(Stance::Agree))
        .map(|r| Confirmation { reading: r.id, by: speaker, at: r.authorship.changed() })
        .max_by_key(|c| (c.at, c.reading))
        .filter(|c| c.at >= reading.authorship.changed())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intentions::{Authorship, Edit};

    /// A reading of `of` with `stance`, by `party`, written at `written` and changed at `edited`.
    fn reading(id: u8, of: u8, party: u8, stance: Stance, written: i64, edited: Option<i64>) -> ReadingRecord {
        let party = EntityId::from_bytes([party; 32]);
        ReadingRecord {
            id: EntityId::from_bytes([id; 32]),
            project: EntityId::from_bytes([0; 32]),
            of: EntityId::from_bytes([of; 32]),
            words: String::new(),
            stance: Some(stance),
            authorship: Authorship {
                written_by: party,
                written,
                speaker: party,
                edited: edited.map(|at| Edit { at, by: Some(party) }),
            },
            revision: String::new(),
        }
    }

    #[test]
    fn the_speakers_latest_agreement_confirms_a_reading_until_the_reading_changes_after_it() {
        let speaker = EntityId::from_bytes([9; 32]);
        let agents = reading(1, 50, 7, Stance::Agree, 100, Some(200));
        let at = |c: Option<Confirmation>| c.map(|c| c.at);

        assert_eq!(at(confirmation(&agents, speaker, &[])), None, "no reading of it");
        let others = reading(2, 1, 8, Stance::Agree, 300, None);
        assert_eq!(at(confirmation(&agents, speaker, &[others])), None, "another party's agreement");
        let questions = reading(3, 1, 9, Stance::Question, 300, None);
        assert_eq!(at(confirmation(&agents, speaker, &[questions])), None, "the speaker's question");
        let before_the_edit = reading(4, 1, 9, Stance::Agree, 150, None);
        assert_eq!(at(confirmation(&agents, speaker, std::slice::from_ref(&before_the_edit))), None, "older than the edit");
        let with_the_edit = reading(5, 1, 9, Stance::Agree, 200, None);
        assert_eq!(at(confirmation(&agents, speaker, &[with_the_edit])), Some(200), "not older than the edit");
        let reworded_after = reading(6, 1, 9, Stance::Agree, 150, Some(250));
        assert_eq!(at(confirmation(&agents, speaker, &[before_the_edit, reworded_after])), Some(250), "the latest");
    }

    #[test]
    fn a_revision_changes_with_the_stance_or_the_words_and_not_otherwise() {
        let first = revision(Some(Stance::Agree), "the list keeps its place");
        assert_eq!(first.len(), 2 * REVISION_BYTES);
        assert_eq!(first, revision(Some(Stance::Agree), "the list keeps its place"));
        assert_ne!(first, revision(Some(Stance::Question), "the list keeps its place"));
        assert_ne!(first, revision(Some(Stance::Agree), "the list keeps its place."));
    }
}
