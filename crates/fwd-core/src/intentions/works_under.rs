//! What a principal works under, read in one go: the intentions its accepted `works_under`
//! relations name, of any project, each with the principal's own reading of it and the speaker's
//! reading of that, so that an agent coming back to its work sees first what governs it, and where
//! its understanding is in doubt or was corrected. Where each stands is judged by revisions, the
//! wordings each reading read, and never by clocks.

use super::readings::{party_reading, readings_of, ReadingRecord};
use super::{closing, IntentionRecord, Parts};
use crate::model::RelationKind;
use crate::project_graph::{Followed, ProjectGraph, Whose};
use crate::rules;
use ankurah::EntityId;
use ankurah_commons::intention_graph::Stance;
use anyhow::Result;
use std::collections::BTreeSet;

/// One intention a principal works under, as it stands for that principal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkingIntention {
    pub intention: IntentionRecord,
    /// The project it is placed in, which a reader names beside it when it is another.
    pub project: Whose,
    /// The principal's own standing reading of it.
    pub reading: Option<ReadingRecord>,
    /// The intention's speaker's standing reading of that reading, whatever its stance.
    pub speakers_reading: Option<ReadingRecord>,
}

impl WorkingIntention {
    /// Whether the principal's reading read a wording of the intention that no longer stands.
    pub fn reworded_since_reading(&self) -> bool {
        self.reading.as_ref().is_some_and(|r| r.read != self.intention.revision)
    }

    /// The speaker's reading of the principal's reading when it read that reading as it reads now:
    /// only such an answer confirms the reading or puts it to discussion, so that a principal that
    /// rewords its reading to take a correction in leaves the discussion and waits for the speaker.
    fn speakers_answer(&self) -> Option<&ReadingRecord> {
        match (&self.reading, &self.speakers_reading) {
            (Some(reading), Some(speakers)) if speakers.read == reading.revision => Some(speakers),
            _ => None,
        }
    }

    /// Whether the speaker agrees with the principal's reading as it reads now.
    pub fn confirmed(&self) -> bool {
        self.speakers_answer().is_some_and(|speakers| speakers.stance == Some(Stance::Agree))
    }

    /// Where it stands, which orders a list: an intention since fulfilled or withdrawn first, whose
    /// link wants reviewing; then what is to discuss, a reading that questions or disagrees, the
    /// principal's, or the speaker's of the principal's reading as it reads now; then an intention
    /// reworded since the principal read it; then one with no reading; then a reading not
    /// confirmed; then the confirmed ones.
    pub fn standing(&self) -> Standing {
        let doubts = |r: Option<&ReadingRecord>| {
            r.is_some_and(|r| matches!(r.stance, Some(Stance::Question | Stance::Disagree)))
        };
        if closing(self.intention.status) {
            Standing::Ended
        } else if doubts(self.reading.as_ref()) || doubts(self.speakers_answer()) {
            Standing::ToDiscuss
        } else if self.reworded_since_reading() {
            Standing::Reworded
        } else if self.reading.is_none() {
            Standing::Unread
        } else if self.confirmed() {
            Standing::Confirmed
        } else {
            Standing::Unconfirmed
        }
    }
}

/// Where an intention stands for the principal that works under it, in the order a list gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, strum::Display)]
#[strum(serialize_all = "snake_case")]
pub enum Standing {
    Ended,
    ToDiscuss,
    Reworded,
    Unread,
    Unconfirmed,
    Confirmed,
}

/// What a principal works under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorksUnder {
    /// By [`WorkingIntention::standing`], then by id, so that an unchanged set reads the same.
    pub intentions: Vec<WorkingIntention>,
    /// The ids its relations name that the node holds no intention by.
    pub not_held: Vec<EntityId>,
}

/// What `principal` works under, read through `graph` wherever each intention is.
pub async fn works_under(graph: &ProjectGraph, principal: EntityId) -> Result<WorksUnder> {
    let mut named = BTreeSet::new();
    for r in graph.relations_from(principal, Some(RelationKind::WorksUnder)).await? {
        if rules::holds(RelationKind::WorksUnder, r.status()?) {
            named.insert(r.to_id()?);
        }
    }
    let mut intentions = Vec::new();
    let mut not_held = Vec::new();
    for id in named {
        let Followed::Held { project, found: (placement, parts), .. } = graph.follow_placed::<Parts>(id).await? else {
            not_held.push(id);
            continue;
        };
        let intention = IntentionRecord::read(graph, &placement, &parts).await?;
        let reading = party_reading(&readings_of(graph, intention.id).await?, principal).cloned();
        let speakers_reading = match &reading {
            Some(r) => party_reading(&readings_of(graph, r.id).await?, intention.authorship.speaker).cloned(),
            None => None,
        };
        intentions.push(WorkingIntention { intention, project, reading, speakers_reading });
    }
    intentions.sort_by_key(|w| (w.standing(), w.intention.id));
    Ok(WorksUnder { intentions, not_held })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intentions::Authorship;
    use ankurah_commons::intention_graph::IntentionStatus;

    fn id(n: u8) -> EntityId {
        EntityId::from_bytes([n; 32])
    }

    fn by(party: u8) -> Authorship {
        Authorship { written_by: id(party), written: 0, speaker: id(party), edited: None }
    }

    /// An intention of revision `revision`, with `status`.
    fn intention(revision: &str, status: IntentionStatus) -> IntentionRecord {
        IntentionRecord {
            id: id(1),
            project: id(0),
            title: String::new(),
            words: String::new(),
            status,
            authorship: by(9),
            closed: None,
            revision: revision.into(),
        }
    }

    /// A reading of revision `revision` by `party`, with `stance`, that read `read`.
    fn reading(party: u8, stance: Stance, read: &str, revision: &str) -> ReadingRecord {
        ReadingRecord {
            id: id(party),
            project: id(0),
            of: id(1),
            words: String::new(),
            stance: Some(stance),
            authorship: by(party),
            read: read.into(),
            revision: revision.into(),
        }
    }

    fn working(
        status: IntentionStatus,
        own: Option<ReadingRecord>,
        speakers: Option<ReadingRecord>,
    ) -> WorkingIntention {
        WorkingIntention {
            intention: intention("i2", status),
            project: Whose::This,
            reading: own,
            speakers_reading: speakers,
        }
    }

    #[test]
    fn the_speakers_agreement_confirms_the_reading_it_read_and_no_later_one() {
        let open = IntentionStatus::Committed;
        let agents = || Some(reading(7, Stance::Agree, "i2", "r2"));
        let agreeing = |read: &str| Some(reading(9, Stance::Agree, read, "s"));
        assert_eq!(working(open, agents(), agreeing("r2")).standing(), Standing::Confirmed);
        assert_eq!(working(open, agents(), agreeing("r1")).standing(), Standing::Unconfirmed, "an earlier wording");
        assert_eq!(working(open, agents(), None).standing(), Standing::Unconfirmed);
    }

    #[test]
    fn standing_puts_an_ended_intention_first_then_doubts_then_rewordings_then_the_rest() {
        let open = IntentionStatus::Committed;
        let agrees = Some(reading(7, Stance::Agree, "i2", "r2"));
        let confirms = Some(reading(9, Stance::Agree, "r2", "s"));
        let ended = working(IntentionStatus::Withdrawn, agrees.clone(), confirms.clone());
        assert_eq!(ended.standing(), Standing::Ended);
        let disputed = working(open, agrees.clone(), Some(reading(9, Stance::Disagree, "r2", "s")));
        assert_eq!(disputed.standing(), Standing::ToDiscuss, "the speaker's disagreement");
        let questioning = working(open, Some(reading(7, Stance::Question, "i2", "r2")), None);
        assert_eq!(questioning.standing(), Standing::ToDiscuss, "the principal's own question");
        let reworded = working(open, Some(reading(7, Stance::Agree, "i1", "r2")), confirms);
        assert!(reworded.reworded_since_reading());
        assert_eq!(reworded.standing(), Standing::Reworded, "read an earlier wording of the intention");
        assert_eq!(working(open, None, None).standing(), Standing::Unread);
        let order = [Standing::Ended, Standing::ToDiscuss, Standing::Reworded, Standing::Unread, Standing::Unconfirmed];
        assert!(order.windows(2).all(|pair| pair[0] < pair[1]) && Standing::Unconfirmed < Standing::Confirmed);
    }
}
