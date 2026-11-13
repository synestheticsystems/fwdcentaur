//! Rows in Ankurah f8e85d2's shapes, written by hand, for the tests of the hop and of the
//! store's conversion.

use super::*;
use ankurah::proto::{AuthorId, OperationSet};

/// The `_ankurah_event` row Ankurah f8e85d2 stored for a creation, and the entity's id.
pub fn creation(seed: u8, operations: OperationSet) -> (EntityId, OldEvent) {
    let body = EventBody::Genesis {
        system: Some(EntityId::from_bytes([0xee; 32])),
        nonce: [seed; 32],
        timestamp: 1,
        author: AuthorId::Unknown,
        operations,
    };
    let entity_id =
        Event { entity_id: EntityId::from_bytes([0; 32]), parent: Clock::default(), body: body.clone() }.id().into();
    let (_, row) = row(entity_id, body, Vec::new());
    (entity_id, row)
}

/// The `_ankurah_event` row Ankurah f8e85d2 stored for an update on these parents, and its id.
pub fn update(entity_id: EntityId, seed: u8, operations: OperationSet, parents: &[&OldEvent]) -> (EventId, OldEvent) {
    let body = EventBody::Update { nonce: [seed; 32], timestamp: 2, author: AuthorId::Unknown, operations };
    row(entity_id, body, parents.iter().map(|parent| EventId::from_base64(&parent.id).unwrap()).collect())
}

fn row(entity_id: EntityId, body: EventBody, mut parents: Vec<EventId>) -> (EventId, OldEvent) {
    parents.sort();
    let event = DecodedEvent { id: EventId::from_bytes([0; 32]), entity_id, body, parents };
    let id = event.old_id();
    let row = OldEvent {
        id: id.to_base64(),
        entity_id: entity_id.to_base64(),
        body: bincode::serialize(&event.body).unwrap(),
        parent: serde_json::to_string(&event.parents).unwrap(),
        attestations: bincode::serialize(&AttestationSet::default()).unwrap(),
    };
    (id, row)
}

/// The `_ankurah_entity` row Ankurah f8e85d2 stored for an entity with this state.
pub fn entity(
    entity_id: EntityId,
    buffers: Vec<(&str, Vec<u8>)>,
    head: &[&OldEvent],
    memberships: &[ModelId],
) -> OldEntity {
    let buffers = StateBuffers(buffers.into_iter().map(|(backend, buffer)| (backend.to_owned(), buffer)).collect());
    let mut head: Vec<EventId> = head.iter().map(|tip| EventId::from_base64(&tip.id).unwrap()).collect();
    head.sort();
    OldEntity {
        id: entity_id.to_base64(),
        state_buffer: bincode::serialize(&buffers).unwrap(),
        head: serde_json::to_string(&head).unwrap(),
        attestations: bincode::serialize(&AttestationSet::default()).unwrap(),
        memberships: memberships.iter().map(|model| bincode::serialize(model).unwrap()).collect(),
    }
}

/// An LWW state buffer as Ankurah f8e85d2 encoded it: each value with the event that wrote it.
pub fn lww_buffer(entries: Vec<(PropertyId, Value, &OldEvent)>) -> Vec<u8> {
    let map: BTreeMap<PropertyId, OldLwwEntry> = entries
        .into_iter()
        .map(|(property, value, writer)| {
            (property, OldLwwEntry { value: Some(value), event_id: EventId::from_base64(&writer.id).unwrap() })
        })
        .collect();
    let mut buffer = vec![LWW_STATE_VERSION];
    bincode::serialize_into(&mut buffer, &map).unwrap();
    buffer
}
