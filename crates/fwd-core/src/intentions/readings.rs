//! Readings: one party's account, in its own words, of how it understands an intention or
//! another reading, with where it stands on it (the Commons' `Reading`, with `Creation`, and
//! `Attribution` when written down for someone else), and the wording it read (fwd's
//! `WordingRead`). A party has one standing reading of a record, which a later one rewords. The
//! speaker of an intention confirms an agent's reading of it with a reading of that reading,
//! stance agree, which counts while it answers the wording that stands; a reading of a reading is
//! also how a person corrects an agent's account in their own words. A reading is placed in the
//! project of the command that writes it, and a record's readings are read wherever they were
//! placed.

use super::{principal_ref, record_edit, revision, said_by, Authorship, IntentionRecord, Parts};
use crate::model::{PlacementView, WordingRead, WordingReadView};
use crate::now_ms;
use crate::project_graph::{Followed, InProjectView, ProjectGraph};
use crate::rules;
use ankurah::EntityId;
use ankurah_commons::intention_graph::{Reading, ReadingView, Stance};
use ankurah_commons::records::{Attribution, Creation, CreationView};
use anyhow::{anyhow, bail, Result};

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
    /// The revision of the wording of `of` it read.
    pub read: String,
    /// [`revision`] of the wording it read, its stance and its words.
    pub revision: String,
}

impl ReadingRecord {
    async fn read(graph: &ProjectGraph, placement: &PlacementView, reading: &ReadingView) -> Result<ReadingRecord> {
        let id = placement.id();
        let creation = graph
            .component_of::<CreationView>(placement)
            .await?
            .ok_or_else(|| anyhow!("the reading {} carries no record of its creation", id.to_base64()))?;
        let read = graph
            .component_of::<WordingReadView>(placement)
            .await?
            .ok_or_else(|| anyhow!("the reading {} carries no record of the wording it read", id.to_base64()))?
            .revision()?;
        let (words, stance) = (reading.words()?, reading.stance()?);
        let stance_name = stance.map(|s| s.to_string()).unwrap_or_default();
        Ok(ReadingRecord {
            id,
            project: placement.project_id()?,
            of: reading.of()?,
            revision: revision(&[&read, &stance_name, &words]),
            read,
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

/// A record whose wording changed since the revision its reader passed: what the reader read is
/// not what stands.
#[derive(Debug, thiserror::Error)]
#[error("{} has changed since you read it (you read {read}, it is now {now}): read it again", .record.to_base64())]
pub struct ChangedSinceRead {
    pub record: EntityId,
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

/// `party`'s standing reading among `readings`, the readings of one record: the one every reader
/// takes and a later write rewords. A party has one; two come only from two first writes that
/// raced, and the first by id stands, whatever either says.
pub(crate) fn party_reading(readings: &[ReadingRecord], party: EntityId) -> Option<&ReadingRecord> {
    readings.iter().filter(|r| r.party() == party).min_by_key(|r| r.id)
}

/// A record a reading may read: an intention, or another reading.
#[derive(Debug, Clone)]
pub enum Readable {
    Intention(IntentionRecord),
    Reading(ReadingRecord),
}

impl Readable {
    pub fn id(&self) -> EntityId {
        match self {
            Readable::Intention(i) => i.id,
            Readable::Reading(r) => r.id,
        }
    }

    /// The revision of its wording, which a reading of it records as read.
    pub fn revision(&self) -> &str {
        match self {
            Readable::Intention(i) => &i.revision,
            Readable::Reading(r) => &r.revision,
        }
    }
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
    /// Whose account it is, a person's, when the principal writing it down is someone else.
    pub said_by: Option<EntityId>,
    /// The [`revision`] of the wording read, which must be what stands: required when `of` is a
    /// reading; for an intention, without it, the revision that stands as the reading is written is
    /// recorded.
    pub as_read: Option<String>,
}

/// Writes the party's standing reading of `spec.of`, as `by` writes it down: its first, placed in
/// the graph's project, or what it says now in place of what it said, with when and by whom the
/// words changed, in the project it is placed in. A reading that stands as given, of the same
/// wording, is left as it is; one whose account stands but that read a new wording records that
/// wording alone. Its id.
pub async fn set(graph: &ProjectGraph, by: EntityId, spec: SetReading) -> Result<EntityId> {
    let target = read_record(graph, spec.of).await?;
    let read = match (&target, spec.as_read) {
        (Readable::Reading(r), None) => bail!(
            "a reading of a reading takes the revision read, so that it answers the words that stand: pass --as-read {}",
            r.revision
        ),
        (_, Some(given)) if given != target.revision() => {
            return Err(ChangedSinceRead { record: target.id(), read: given, now: target.revision().to_string() }.into())
        }
        _ => target.revision().to_string(),
    };
    if let Readable::Reading(r) = &target {
        standing_reading(graph, r).await?;
    }
    let party = rules::party_of(by, said_by(graph, spec.said_by).await?)?;
    // An agreement with a reading written down for someone else is their confirmation of it, and
    // carries their own words as the record of what they said.
    let relayed_agreement = matches!(target, Readable::Reading(_)) && spec.stance == Stance::Agree && party != by;
    if relayed_agreement && spec.words.trim().is_empty() {
        bail!("an agreement written down for someone else carries their own words: give them");
    }
    let readings = readings_of(graph, spec.of).await?;
    let Some(one) = party_reading(&readings, party) else {
        let trx = graph.begin();
        let reading = Reading { of: spec.of, words: spec.words, stance: Some(spec.stance) };
        let made = trx.create_placed(&reading).await?;
        let record = ankurah::model::Mutable::entity(&*made);
        trx.add(record, &Creation { created_by: principal_ref(by), created: now_ms() }).await?;
        trx.add(record, &WordingRead { revision: read }).await?;
        if party != by {
            trx.add(record, &Attribution { speaker: principal_ref(party) }).await?;
        }
        let id = made.id();
        trx.commit().await?;
        return Ok(id);
    };
    let account_stands = one.words == spec.words && one.stance == Some(spec.stance);
    if account_stands && one.read == read {
        return Ok(one.id);
    }
    let Followed::Held { found: (placement, _), .. } = graph.follow_placed::<ReadingView>(one.id).await? else {
        bail!("the reading {} is no longer held", one.id.to_base64());
    };
    // Changed in the project it is placed in, by the same writer.
    let there = graph.graph_of(&placement)?;
    let trx = there.begin();
    if !account_stands {
        let m = trx.edit_placed::<Reading>(&placement)?;
        m.words()?.set(&spec.words)?;
        m.stance()?.set(&Some(spec.stance))?;
        record_edit(&there, &trx, &placement, ankurah::model::Mutable::entity(&*m), by).await?;
    }
    trx.edit_placed::<WordingRead>(&placement)?.revision()?.set(&read)?;
    trx.commit().await?;
    Ok(one.id)
}

/// Refuses a reading that is not its party's standing reading of what it reads ([`party_reading`]),
/// naming the one that is: an answer to it would answer what no reader takes.
async fn standing_reading(graph: &ProjectGraph, reading: &ReadingRecord) -> Result<()> {
    let readings = readings_of(graph, reading.of).await?;
    match party_reading(&readings, reading.party()) {
        Some(standing) if standing.id != reading.id => bail!(
            "{} is not its party's standing reading of {}, which is {}: answer that one",
            reading.id.to_base64(),
            reading.of.to_base64(),
            standing.id.to_base64()
        ),
        _ => Ok(()),
    }
}

/// Confirms the reading `reading`, which reads an intention, in the words of the intention's
/// speaker, who alone confirms it: their standing reading of it, stance agree, of the wording
/// `as_read`, as `by` writes it down. `words` are the speaker's own, none when the speaker confirms
/// it themselves; an agent that writes down a speaker's confirmation (`said_by`) gives their words.
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
    let speaker = intention.authorship.speaker;
    if rules::party_of(by, self::said_by(graph, said_by).await?)? != speaker {
        bail!(
            "only the intention's speaker, {}, confirms a reading of it: another's agreement is a reading like any other (`fwd reading set`)",
            crate::ops::handle_of(graph, speaker).await?
        );
    }
    let words = words.unwrap_or_default();
    set(graph, by, SetReading { of: reading, words, stance: Stance::Agree, said_by, as_read: Some(as_read) }).await
}
