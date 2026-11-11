//! The hop this converter performs: from the stored format of Ankurah f8e85d2 (the
//! feat/storage-node-sqlite-wasm branch, fwd's pin until 2026-11-11) to the stored format of Ankurah
//! main at 5d8a000b. Everything the converter knows about how the two formats differ is in this
//! module; the rest of the crate reads rows and hands them to the new engine.
//!
//! Three stored shapes changed between the two revisions with no version to tell them apart: both
//! record protocol version 1 in `_ankurah_meta`, and both tag an LWW state buffer 0xA2.
//!
//! - A clock (an entity's head, an event's parent) was a list of event ids. It is now a list of
//!   (generation, event id) pairs; an event's generation is 1 for a genesis and one above its
//!   highest parent's for an update.
//! - An update event's id hashes the binary encoding of its parent clock, so once the clock carries
//!   generations every update event gets a new id, and so do the clocks that name it. A genesis has
//!   no parent: its id, which is its entity's id, does not change.
//! - An LWW state entry named the event that wrote its value. It now ranks the write by that
//!   event's generation and (new) id.
//!
//! Event bodies, collaborative-text (yrs) state and memberships kept their shapes and are carried
//! byte for byte. Attestations kept theirs too, but only empty ones are accepted: an attestation
//! signs bytes this hop rewrites, and fwd's daemon attests nothing. Each rewritten state is decoded
//! again by Ankurah 5d8a000b's own LWW backend before it is accepted.

use std::collections::{BTreeMap, BTreeSet};

use ankurah::core::property::backend::{LWWBackend, PropertyBackend};
use ankurah::proto::{
    AttestationSet, Attested, Clock, EntityId, EntityState, Event, EventBody, EventId, ModelId, State, StateBuffers,
    EVENT_TAG,
};
use ankurah::{PropertyId, Value};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The Ankurah revision whose stored format this hop reads.
pub const FROM_REVISION: &str = "f8e85d2e2399c0c5bdf80ef06498b5f454b87ac0";
/// The Ankurah revision whose stored format this hop writes; fwd's pin.
pub const TO_REVISION: &str = "5d8a000ba9b1375be9307e2aab6887886be35dda";

/// The protocol version both revisions record in `_ankurah_meta`. It names the family of stores
/// this hop reads, not the hop: a store that records another is not one this hop was written for.
pub const PROTOCOL_VERSION: &str = "1";

/// The tag both revisions put before an LWW state buffer's entries.
const LWW_STATE_VERSION: u8 = 0xA2;

/// An `_ankurah_event` row as Ankurah f8e85d2 wrote it.
#[cfg_attr(test, derive(Clone))]
pub struct OldEvent {
    pub id: String,
    pub entity_id: String,
    pub body: Vec<u8>,
    pub parent: String,
    pub attestations: Vec<u8>,
}

/// An `_ankurah_entity` row as Ankurah f8e85d2 wrote it, with the model keys of its
/// `_ankurah_entity_model` rows.
pub struct OldEntity {
    pub id: String,
    pub state_buffer: Vec<u8>,
    pub head: String,
    pub attestations: Vec<u8>,
    pub memberships: Vec<Vec<u8>>,
}

/// One store's events and states in Ankurah 5d8a000b's shapes.
pub struct Converted {
    pub events: Vec<Attested<Event>>,
    pub states: Vec<Attested<EntityState>>,
    /// How many events have a different id than they had: every update event.
    pub ids_changed: usize,
}

/// Why a store cannot be carried across this hop exactly. The converter writes nothing for a store
/// it refuses.
#[derive(Debug, thiserror::Error)]
pub enum Refusal {
    #[error("{what} is not in Ankurah f8e85d2's stored shape: {detail}")]
    Undecodable { what: String, detail: String },
    #[error(
        "event {stored} is stored under an id its content does not hash to under Ankurah f8e85d2's rules ({computed})"
    )]
    StoredIdMismatch { stored: String, computed: String },
    #[error("event {event} names parent {parent}, which the store does not hold, so its generation cannot be known")]
    MissingParent { event: String, parent: String },
    #[error("event {event} of one entity names parent {parent} of another")]
    ForeignParent { event: String, parent: String },
    #[error("{count} events form a cycle through their parents")]
    Cycle { count: usize },
    #[error("event {event} is malformed as Ankurah 5d8a000b reads it: {reason}")]
    Malformed { event: String, reason: String },
    #[error("entity {entity}'s head names event {tip}, which is not one of the entity's stored events")]
    UnknownHeadTip { entity: String, tip: String },
    #[error("entity {entity} has a state with an empty head, so it has no history to carry")]
    EmptyHead { entity: String },
    #[error("entity {entity} has state for property backend {backend:?}, which this hop does not know")]
    UnknownBackend { entity: String, backend: String },
    #[error("entity {entity}'s LWW value for {property} was written by event {event}, which is not one of the entity's stored events")]
    UnknownWriter { entity: String, property: String, event: String },
    #[error("entity {entity}'s rewritten LWW state reads back as different values in Ankurah 5d8a000b")]
    ValuesChanged { entity: String },
    #[error("{what} carries attestations, which sign bytes this hop rewrites; fwd's daemon attests nothing, so this store was not written by it")]
    Attested { what: String },
}

/// Rewrites one store's events and states, walking each entity's events from its genesis, parents
/// before children.
pub fn convert(events: Vec<OldEvent>, entities: Vec<OldEntity>) -> Result<Converted, Refusal> {
    let old = events.into_iter().map(DecodedEvent::decode).collect::<Result<Vec<_>, _>>()?;
    let rewritten = rewrite_events(old)?;
    let ids_changed = rewritten.iter().filter(|(old_id, event)| **old_id != event.id).count();
    let states = entities.into_iter().map(|entity| rewrite_state(entity, &rewritten)).collect::<Result<Vec<_>, _>>()?;
    let events = rewritten
        .into_values()
        .map(|event| Attested { payload: event.event, attestations: AttestationSet::default() })
        .collect();
    Ok(Converted { events, states, ids_changed })
}

/// An [`OldEvent`] with its columns decoded.
struct DecodedEvent {
    id: EventId,
    entity_id: EntityId,
    body: EventBody,
    /// The parent clock: event ids, sorted and without repeats, as f8e85d2's clock kept them.
    parents: Vec<EventId>,
}

impl DecodedEvent {
    fn decode(row: OldEvent) -> Result<Self, Refusal> {
        let what = || format!("event {}", row.id);
        let id = EventId::from_base64(&row.id).map_err(|e| undecodable(what(), e))?;
        let entity_id = EntityId::from_base64(&row.entity_id).map_err(|e| undecodable(what(), e))?;
        let body: EventBody =
            decode_exactly(&row.body).map_err(|detail| Refusal::Undecodable { what: what(), detail })?;
        let mut parents: Vec<EventId> =
            serde_json::from_str(&row.parent).map_err(|e| undecodable(format!("{}'s parent", what()), e))?;
        parents.sort();
        parents.dedup();
        refuse_attestations(&row.attestations, what)?;
        let event = Self { id, entity_id, body, parents };
        let computed = event.old_id();
        if computed != event.id {
            return Err(Refusal::StoredIdMismatch { stored: row.id, computed: computed.to_base64() });
        }
        Ok(event)
    }

    /// The id Ankurah f8e85d2 gave this event. A genesis's derivation did not change; an update's
    /// hashed the parent clock as a plain list of ids.
    fn old_id(&self) -> EventId {
        match &self.body {
            EventBody::Genesis { .. } => {
                Event { entity_id: self.entity_id, parent: Clock::default(), body: self.body.clone() }.id()
            }
            EventBody::Update { nonce, timestamp, author, operations } => {
                let mut hasher = Sha256::new();
                hasher.update(EVENT_TAG);
                hasher.update(bincode::serialize(&self.entity_id).expect("an entity id encodes"));
                hasher.update(bincode::serialize(author).expect("an author encodes"));
                hasher.update(bincode::serialize(nonce).expect("a nonce encodes"));
                hasher.update(bincode::serialize(timestamp).expect("a timestamp encodes"));
                hasher.update(bincode::serialize(operations).expect("operations encode"));
                hasher.update(bincode::serialize(&self.parents).expect("a clock encodes"));
                EventId::from_bytes(hasher.finalize().into())
            }
        }
    }
}

/// An event in its new shape, with the id and generation Ankurah 5d8a000b gives it.
struct RewrittenEvent {
    id: EventId,
    event: Event,
    generation: u32,
}

/// Every event in its new shape, by its old id. An event is rewritten only once all its parents
/// are, since its parent clock carries their generations and new ids.
fn rewrite_events(old: Vec<DecodedEvent>) -> Result<BTreeMap<EventId, RewrittenEvent>, Refusal> {
    let by_id: BTreeMap<EventId, &DecodedEvent> = old.iter().map(|event| (event.id.clone(), event)).collect();
    let mut waiting_on: BTreeMap<&EventId, usize> = BTreeMap::new();
    let mut children: BTreeMap<&EventId, Vec<&EventId>> = BTreeMap::new();
    let mut ready = Vec::new();
    for event in &old {
        for parent in &event.parents {
            let Some(parent_event) = by_id.get(parent) else {
                return Err(Refusal::MissingParent { event: event.id.to_base64(), parent: parent.to_base64() });
            };
            if parent_event.entity_id != event.entity_id {
                return Err(Refusal::ForeignParent { event: event.id.to_base64(), parent: parent.to_base64() });
            }
            children.entry(parent).or_default().push(&event.id);
        }
        waiting_on.insert(&event.id, event.parents.len());
        if event.parents.is_empty() {
            ready.push(&event.id);
        }
    }

    let mut rewritten: BTreeMap<EventId, RewrittenEvent> = BTreeMap::new();
    while let Some(id) = ready.pop() {
        let old_event = by_id[id];
        let parent = old_event
            .parents
            .iter()
            .map(|parent| {
                let parent = &rewritten[parent];
                (parent.generation, parent.id.clone())
            })
            .collect::<Vec<_>>();
        let parent =
            Clock::new(parent).map_err(|e| Refusal::Malformed { event: id.to_base64(), reason: e.to_string() })?;
        let event = Event { entity_id: old_event.entity_id, parent, body: old_event.body.clone() };
        event.validate_structure().map_err(|e| Refusal::Malformed { event: id.to_base64(), reason: e.to_string() })?;
        let (new_id, generation) = (event.id(), event.generation());
        rewritten.insert(id.clone(), RewrittenEvent { id: new_id, event, generation });
        for &child in children.get(id).into_iter().flatten() {
            let count = waiting_on.get_mut(child).expect("every child is counted");
            *count -= 1;
            if *count == 0 {
                ready.push(child);
            }
        }
    }
    if rewritten.len() != old.len() {
        return Err(Refusal::Cycle { count: old.len() - rewritten.len() });
    }
    Ok(rewritten)
}

/// An entity's state with its head and LWW entries naming the rewritten events.
fn rewrite_state(
    entity: OldEntity,
    events: &BTreeMap<EventId, RewrittenEvent>,
) -> Result<Attested<EntityState>, Refusal> {
    let what = || format!("entity {}", entity.id);
    let entity_id = EntityId::from_base64(&entity.id).map_err(|e| undecodable(what(), e))?;
    refuse_attestations(&entity.attestations, what)?;
    let memberships = entity
        .memberships
        .iter()
        .map(|key| decode_exactly::<ModelId>(key))
        .collect::<Result<BTreeSet<_>, _>>()
        .map_err(|detail| Refusal::Undecodable { what: format!("{}'s membership", what()), detail })?;
    let event_of_entity = |id: &EventId| events.get(id).filter(|event| event.event.entity_id == entity_id);

    let tips: Vec<EventId> =
        serde_json::from_str(&entity.head).map_err(|e| undecodable(format!("{}'s head", what()), e))?;
    if tips.is_empty() {
        return Err(Refusal::EmptyHead { entity: entity.id });
    }
    let mut head = Vec::with_capacity(tips.len());
    for tip in &tips {
        let Some(event) = event_of_entity(tip) else {
            return Err(Refusal::UnknownHeadTip { entity: entity.id, tip: tip.to_base64() });
        };
        head.push((event.generation, event.id.clone()));
    }
    let head = Clock::new(head).map_err(|e| undecodable(format!("{}'s head", what()), e))?;

    let StateBuffers(buffers) =
        decode_exactly(&entity.state_buffer).map_err(|detail| Refusal::Undecodable { what: what(), detail })?;
    let mut rewritten = BTreeMap::new();
    for (backend, buffer) in buffers {
        let buffer = match backend.as_str() {
            "lww" => rewrite_lww(&entity.id, &buffer, &event_of_entity)?,
            "yrs" => buffer,
            _ => return Err(Refusal::UnknownBackend { entity: entity.id, backend }),
        };
        rewritten.insert(backend, buffer);
    }

    Ok(Attested {
        payload: EntityState { entity_id, state: State { state_buffers: StateBuffers(rewritten), memberships, head } },
        attestations: AttestationSet::default(),
    })
}

/// An LWW entry as Ankurah f8e85d2 encoded it (`CommittedEntry` in core/src/property/backend/lww.rs).
#[derive(Serialize, Deserialize)]
struct OldLwwEntry {
    value: Option<Value>,
    event_id: EventId,
}

/// An LWW entry as Ankurah 5d8a000b encodes it (`CommittedEntry` and `WriteRank` in the same file).
/// The field order is the encoding, so it follows Ankurah's.
#[derive(Serialize)]
struct LwwEntry {
    value: Option<Value>,
    rank: WriteRank,
}

#[derive(Serialize)]
struct WriteRank {
    generation: u32,
    event_id: EventId,
}

fn rewrite_lww<'a>(
    entity: &str,
    buffer: &[u8],
    event_of_entity: &impl Fn(&EventId) -> Option<&'a RewrittenEvent>,
) -> Result<Vec<u8>, Refusal> {
    let what = || format!("entity {entity}'s LWW state");
    let Some((&LWW_STATE_VERSION, entries)) = buffer.split_first() else {
        return Err(Refusal::Undecodable {
            what: what(),
            detail: format!("it does not start with the tag {LWW_STATE_VERSION:#04x}"),
        });
    };
    let old: BTreeMap<PropertyId, OldLwwEntry> =
        decode_exactly(entries).map_err(|detail| Refusal::Undecodable { what: what(), detail })?;
    let mut new = BTreeMap::new();
    for (property, entry) in &old {
        let Some(writer) = event_of_entity(&entry.event_id) else {
            return Err(Refusal::UnknownWriter {
                entity: entity.to_owned(),
                property: property.to_string(),
                event: entry.event_id.to_base64(),
            });
        };
        let rank = WriteRank { generation: writer.generation, event_id: writer.id.clone() };
        new.insert(*property, LwwEntry { value: entry.value.clone(), rank });
    }
    let mut rewritten = vec![LWW_STATE_VERSION];
    bincode::serialize_into(&mut rewritten, &new).expect("LWW entries encode");

    // Ankurah 5d8a000b is the judge: its LWW backend must read the same values back.
    let read_back = LWWBackend::from_state_buffer(&rewritten)
        .map_err(|e| Refusal::Undecodable { what: what(), detail: e.to_string() })?;
    let values: BTreeMap<PropertyId, Option<Value>> =
        old.into_iter().map(|(property, entry)| (property, entry.value)).collect();
    if read_back.property_values() != values {
        return Err(Refusal::ValuesChanged { entity: entity.to_owned() });
    }
    Ok(rewritten)
}

/// Decodes `bytes` as one `T` and nothing more: the bytes must be exactly `T`'s encoding, so that a
/// value that only starts like one (which bincode alone accepts) is refused rather than misread.
fn decode_exactly<T: Serialize + DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    let value: T = bincode::deserialize(bytes).map_err(|e| e.to_string())?;
    if bincode::serialize(&value).map_err(|e| e.to_string())? != bytes {
        return Err("it decodes, but does not encode back to the same bytes".into());
    }
    Ok(value)
}

fn refuse_attestations(bytes: &[u8], what: impl Fn() -> String) -> Result<(), Refusal> {
    let attestations: AttestationSet = decode_exactly(bytes)
        .map_err(|detail| Refusal::Undecodable { what: format!("{}'s attestations", what()), detail })?;
    if attestations.is_empty() {
        Ok(())
    } else {
        Err(Refusal::Attested { what: what() })
    }
}

fn undecodable(what: String, error: impl std::fmt::Display) -> Refusal {
    Refusal::Undecodable { what, detail: error.to_string() }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub mod old_shapes;
