//! The renderer's tests: the drawing's stages.
use super::cards::{Arrow, Card};
use super::*;
use fwd_core::model::TaskStatus;
use std::sync::Mutex;

/// Two cards, the second waiting for the first, as the cards stage reads them.
fn waiting_pair() -> Cards {
    let card = |n: u8, title: &str| Card {
        id: EntityId::from_bytes([n; 32]),
        title: title.into(),
        what: "As it stands".into(),
        status: TaskStatus::Open,
        priority: Some(2),
        parent: String::new(),
        footer: String::new(),
        height: 149.0,
        waiting: false,
        revision: n.into(),
    };
    Cards {
        cards: vec![card(1, "Comes first"), card(2, "Comes next")],
        arrows: vec![Arrow { first: 0, then: 1, proposed: false }],
        width: 360.0,
        slot_height: 149.0,
        content_width: 360.0,
        text_scale: 1.0,
        max_rows: 8,
    }
}

#[test]
fn a_new_message_redraws_the_frame_without_packing_the_cards_again() {
    let runs = Arc::new(Mutex::new(Vec::new()));
    let stopwatch: Stopwatch = {
        let runs = runs.clone();
        Arc::new(move |stage| {
            runs.lock().unwrap().push(stage);
            Box::new(|| {})
        })
    };
    let ran = |stage| runs.lock().unwrap().iter().filter(|&&run| run == stage).count();
    let cards = RwSignal::new(waiting_pair());
    let counts = RwSignal::new(HashMap::new());
    let frame = frame(Memo::new(move |_| cards.get()), Memo::new(move |_| counts.get()), stopwatch);
    let first = frame.get();
    assert_eq!((ran(Stage::Packing), ran(Stage::Markup)), (1, 1));
    counts.set(HashMap::from([(EntityId::from_bytes([2; 32]), 1)]));
    let counted = frame.get();
    assert!(!first.contains(r#"class="count""#) && counted.contains(r#"<text text-anchor="middle" y="4">1</text>"#));
    assert_eq!((ran(Stage::Packing), ran(Stage::Markup)), (1, 2), "the count is drawn, and nothing packed again");
    cards.update(|cards| cards.cards[1].what = "Edited, moving nothing".into());
    assert!(frame.get().contains("Edited, moving nothing"));
    assert_eq!((ran(Stage::Packing), ran(Stage::Markup)), (1, 3), "nor for an edit that places nothing");
    cards.update(|cards| cards.cards[1].height += 24.0);
    frame.get();
    assert_eq!(ran(Stage::Packing), 2, "a card grown taller is packed again");
}
