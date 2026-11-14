//! What a principal works under, read in one go: the intentions its accepted `works_under`
//! relations name, of any project, each with the principal's own reading of it and whether the
//! intention's speaker confirmed that reading, so that an agent coming back to its work sees first
//! what governs it, and where its understanding is in doubt.

use super::readings::{confirmation, readings_of, Confirmation, ReadingRecord};
use super::{IntentionRecord, Parts};
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
    /// The principal's own reading of it.
    pub reading: Option<ReadingRecord>,
    /// The speaker's confirmation of that reading, while its words stand.
    pub confirmation: Option<Confirmation>,
    /// Whether the intention's words changed after the reading last did.
    pub reworded_since_reading: bool,
}

impl WorkingIntention {
    /// Where it stands, which orders a list: those to discuss first, meaning a reading that
    /// questions or disagrees, then those with no reading, then a reading not confirmed, then the
    /// confirmed ones.
    pub fn standing(&self) -> Standing {
        match (&self.reading, &self.confirmation) {
            (Some(r), _) if matches!(r.stance, Some(Stance::Question | Stance::Disagree)) => Standing::ToDiscuss,
            (None, _) => Standing::Unread,
            (Some(_), None) => Standing::Unconfirmed,
            (Some(_), Some(_)) => Standing::Confirmed,
        }
    }
}

/// Where an intention stands for the principal that works under it, in the order a list gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, strum::Display)]
#[strum(serialize_all = "snake_case")]
pub enum Standing {
    ToDiscuss,
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
        // Two readings by one party come only from two writes that raced, and the later stands.
        let reading = readings_of(graph, intention.id)
            .await?
            .into_iter()
            .filter(|r| r.party() == principal)
            .max_by_key(|r| (r.authorship.changed(), r.id));
        let confirmation = match &reading {
            Some(r) => confirmation(r, intention.authorship.speaker, &readings_of(graph, r.id).await?),
            None => None,
        };
        let reworded_since_reading =
            reading.as_ref().is_some_and(|r| intention.authorship.changed() > r.authorship.changed());
        intentions.push(WorkingIntention { intention, project, reading, confirmation, reworded_since_reading });
    }
    intentions.sort_by_key(|w| (w.standing(), w.intention.id));
    Ok(WorksUnder { intentions, not_held })
}
