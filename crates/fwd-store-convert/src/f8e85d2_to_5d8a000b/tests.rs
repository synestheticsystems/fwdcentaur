use std::collections::BTreeMap;

use ankurah::core::property::backend::{LWWBackend, PropertyBackend, YrsBackend};
use ankurah::proto::{Clock, EntityId, Event, EventId, OperationSet};
use ankurah::{PropertyId, Value};

use super::old_shapes::{creation, entity, lww_buffer, update};
use super::{convert, Converted, OldEvent, Refusal};

fn title() -> PropertyId {
    PropertyId::EntityId(EntityId::from_bytes([1; 32]))
}
fn notes() -> PropertyId {
    PropertyId::EntityId(EntityId::from_bytes([2; 32]))
}

/// The operations an event carries for one LWW write.
fn writes(property: PropertyId, value: &str) -> OperationSet {
    let backend = LWWBackend::new();
    backend.set(property, Some(Value::String(value.into())));
    OperationSet::from_backends(BTreeMap::from([("lww".to_owned(), backend.to_operations().unwrap().unwrap())]))
}

/// The id the event stored as `old` has now: the converted event with its body, which its nonce
/// makes unique.
fn new_id(converted: &Converted, old: &OldEvent) -> EventId {
    converted.events.iter().find(|event| event_body_matches(event, old)).expect("the event is carried").payload.id()
}

fn event_body_matches(event: &ankurah::proto::Attested<Event>, old: &OldEvent) -> bool {
    bincode::serialize(&event.payload.body).unwrap() == old.body
}

/// The generation and writer an LWW entry ranks its value by, read straight from the encoding:
/// bincode encodes a struct as its fields in order, as it does a tuple.
fn ranks(buffer: &[u8]) -> BTreeMap<PropertyId, (u32, EventId)> {
    let entries: BTreeMap<PropertyId, (Option<Value>, (u32, EventId))> = bincode::deserialize(&buffer[1..]).unwrap();
    entries.into_iter().map(|(property, (_, rank))| (property, rank)).collect()
}

#[test]
fn a_creation_keeps_its_id_and_is_generation_one() {
    let (entity_id, genesis) = creation(1, writes(title(), "first"));
    let state = entity(
        entity_id,
        vec![("lww", lww_buffer(vec![(title(), Value::String("first".into()), &genesis)]))],
        &[&genesis],
        &[],
    );
    let converted = convert(vec![genesis], vec![state]).unwrap();

    assert_eq!(converted.ids_changed, 0);
    let genesis_id: EventId = converted.events[0].payload.id();
    assert_eq!(EntityId::from(genesis_id.clone()), entity_id, "the entity's id is its genesis's");
    let head = &converted.states[0].payload.state.head;
    assert_eq!(head, &Clock::new(vec![(1, genesis_id.clone())]).unwrap());
    assert_eq!(ranks(&converted.states[0].payload.state.state_buffers["lww"])[&title()], (1, genesis_id));
}

#[test]
fn each_update_in_a_chain_gets_a_new_id_that_its_child_and_the_head_name() {
    let (entity_id, genesis) = creation(1, writes(title(), "first"));
    let (_, second) = update(entity_id, 2, writes(title(), "second"), &[&genesis]);
    let (_, third) = update(entity_id, 3, writes(notes(), "third"), &[&second]);
    let lww = lww_buffer(vec![
        (title(), Value::String("second".into()), &second),
        (notes(), Value::String("third".into()), &third),
    ]);
    let state = entity(entity_id, vec![("lww", lww)], &[&third], &[]);
    let (old_second, old_third) = (EventId::from_base64(&second.id).unwrap(), EventId::from_base64(&third.id).unwrap());
    let converted = convert(vec![third.clone(), genesis.clone(), second.clone()], vec![state]).unwrap();

    assert_eq!(converted.ids_changed, 2);
    let (new_second, new_third) = (new_id(&converted, &second), new_id(&converted, &third));
    assert_ne!(new_second, old_second);
    assert_ne!(new_third, old_third);
    let third_event = converted.events.iter().find(|event| event.payload.id() == new_third).unwrap();
    assert_eq!(
        third_event.payload.parent,
        Clock::new(vec![(2, new_second.clone())]).unwrap(),
        "the child names its parent's new id"
    );
    assert_eq!(third_event.payload.generation(), 3);
    assert_eq!(converted.states[0].payload.state.head, Clock::new(vec![(3, new_third.clone())]).unwrap());
    let ranks = ranks(&converted.states[0].payload.state.state_buffers["lww"]);
    assert_eq!(ranks[&title()], (2, new_second));
    assert_eq!(ranks[&notes()], (3, new_third));
}

#[test]
fn a_head_with_two_tips_keeps_both_and_the_values_it_had() {
    let (entity_id, genesis) = creation(1, writes(title(), "first"));
    let (_, left) = update(entity_id, 2, writes(title(), "left"), &[&genesis]);
    let (_, right) = update(entity_id, 3, writes(title(), "right"), &[&genesis]);
    // Whichever write f8e85d2 kept stays kept: here the right one.
    let lww = lww_buffer(vec![(title(), Value::String("right".into()), &right)]);
    let state = entity(entity_id, vec![("lww", lww)], &[&left, &right], &[]);
    let converted = convert(vec![genesis.clone(), left.clone(), right.clone()], vec![state]).unwrap();

    let head = &converted.states[0].payload.state.head;
    let mut tips = vec![(2, new_id(&converted, &left)), (2, new_id(&converted, &right))];
    tips.sort_by(|a, b| a.1.cmp(&b.1));
    assert_eq!(head.entries(), tips.as_slice());
    let values = LWWBackend::from_state_buffer(&converted.states[0].payload.state.state_buffers["lww"])
        .unwrap()
        .property_values();
    assert_eq!(values[&title()], Some(Value::String("right".into())));
}

#[test]
fn collaborative_text_state_and_event_bodies_are_carried_byte_for_byte() {
    let text = YrsBackend::new();
    text.insert(&notes(), 0, "shared words").unwrap();
    let operations =
        OperationSet::from_backends(BTreeMap::from([("yrs".to_owned(), text.to_operations().unwrap().unwrap())]));
    let (entity_id, genesis) = creation(1, operations);
    let (_, edit) = update(entity_id, 2, writes(title(), "titled"), &[&genesis]);
    let yrs_state = text.to_state_buffer().unwrap();
    let lww = lww_buffer(vec![(title(), Value::String("titled".into()), &edit)]);
    let state = entity(entity_id, vec![("lww", lww), ("yrs", yrs_state.clone())], &[&edit], &[]);
    let converted = convert(vec![genesis.clone(), edit.clone()], vec![state]).unwrap();

    assert_eq!(converted.states[0].payload.state.state_buffers["yrs"], yrs_state);
    for old in [&genesis, &edit] {
        assert!(converted.events.iter().any(|event| event_body_matches(event, old)), "the body is carried as it was");
    }
    let read = YrsBackend::from_state_buffer(&yrs_state).unwrap();
    assert_eq!(read.get_string(&notes()).as_deref(), Some("shared words"));
}

#[test]
fn an_event_stored_under_an_id_its_content_does_not_hash_to_is_refused() {
    let (entity_id, genesis) = creation(1, writes(title(), "first"));
    let (_, mut second) = update(entity_id, 2, writes(title(), "second"), &[&genesis]);
    second.id = EventId::from_bytes([7; 32]).to_base64();
    let refusal = convert(vec![genesis, second], vec![]).err().unwrap();
    assert!(matches!(refusal, Refusal::StoredIdMismatch { .. }), "{refusal}");
}

#[test]
fn an_event_whose_parent_is_not_stored_is_refused() {
    let (entity_id, genesis) = creation(1, writes(title(), "first"));
    let (_, second) = update(entity_id, 2, writes(title(), "second"), &[&genesis]);
    let refusal = convert(vec![second], vec![]).err().unwrap();
    assert!(matches!(refusal, Refusal::MissingParent { .. }), "{refusal}");
}

#[test]
fn a_value_whose_writer_is_not_stored_is_refused() {
    let (entity_id, genesis) = creation(1, writes(title(), "first"));
    let (_, unstored) = update(entity_id, 2, writes(title(), "second"), &[&genesis]);
    let lww = lww_buffer(vec![(title(), Value::String("second".into()), &unstored)]);
    let state = entity(entity_id, vec![("lww", lww)], &[&genesis], &[]);
    let refusal = convert(vec![genesis], vec![state]).err().unwrap();
    assert!(matches!(refusal, Refusal::UnknownWriter { .. }), "{refusal}");
}

#[test]
fn a_head_naming_an_unstored_event_is_refused() {
    let (entity_id, genesis) = creation(1, writes(title(), "first"));
    let (_, unstored) = update(entity_id, 2, writes(title(), "second"), &[&genesis]);
    let state = entity(entity_id, vec![], &[&unstored], &[]);
    let refusal = convert(vec![genesis], vec![state]).err().unwrap();
    assert!(matches!(refusal, Refusal::UnknownHeadTip { .. }), "{refusal}");
}

#[test]
fn an_attested_event_is_refused() {
    let (_, mut genesis) = creation(1, writes(title(), "first"));
    genesis.attestations =
        bincode::serialize(&ankurah::proto::AttestationSet(vec![ankurah::proto::Attestation(vec![1, 2, 3])])).unwrap();
    let refusal = convert(vec![genesis], vec![]).err().unwrap();
    assert!(matches!(refusal, Refusal::Attested { .. }), "{refusal}");
}

#[test]
fn state_for_an_unknown_backend_is_refused() {
    let (entity_id, genesis) = creation(1, writes(title(), "first"));
    let state = entity(entity_id, vec![("counter", vec![1])], &[&genesis], &[]);
    let refusal = convert(vec![genesis], vec![state]).err().unwrap();
    assert!(matches!(refusal, Refusal::UnknownBackend { .. }), "{refusal}");
}
