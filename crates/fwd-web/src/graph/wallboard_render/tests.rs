//! The renderer's tests: the drawing's stages, and the layout the runtime asks Rust for.
use super::cards::{Arrow, Card};
use super::*;
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

#[test]
fn wallboard_viewport_fit_uses_wide_space_without_dropping_cards_or_ports() {
    let mut input = serde_json::json!({"node_width":480,"max_rows":8,
        "nodes":(0..13).map(|i|serde_json::json!({"id":format!("t{i}"),"title":format!("Task {i}"),"priority":if i==0 {1} else {2},"status":"open","height":180})).collect::<Vec<_>>(),
        "edges":[{"from":"t0","to":"t12"}]});
    let old: serde_json::Value = serde_json::from_str(&wallboard_layout_json(input.to_string())).unwrap();
    input["viewport_width"] = serde_json::json!(3784);
    input["viewport_height"] = serde_json::json!(2020);
    let wide: serde_json::Value = serde_json::from_str(&wallboard_layout_json(input.to_string())).unwrap();
    let scale =
        |d: &serde_json::Value| (3784.0 / d["width"].as_f64().unwrap()).min(2020.0 / d["height"].as_f64().unwrap());
    assert!(scale(&wide) > scale(&old) * 1.2);
    assert!(wide["rows"].as_u64().unwrap() < 8);
    assert_eq!(wide["boxes"].as_array().unwrap().len(), 13);
    let boxes = wide["boxes"].as_array().unwrap();
    let from = boxes.iter().find(|b| b["id"] == "t0").unwrap();
    let to = boxes.iter().find(|b| b["id"] == "t12").unwrap();
    let points = wide["routes"][0].as_array().unwrap();
    assert_eq!(points[0][0].as_f64().unwrap(), from["x"].as_f64().unwrap() + 480.0);
    assert_eq!(points.last().unwrap()[0], to["x"]);
    input["viewport_width"] = serde_json::json!(1080);
    input["viewport_height"] = serde_json::json!(1860);
    let portrait: serde_json::Value = serde_json::from_str(&wallboard_layout_json(input.to_string())).unwrap();
    assert_eq!(portrait["boxes"].as_array().unwrap().len(), 13);
    assert!(portrait["width"].as_f64().unwrap() < wide["width"].as_f64().unwrap());
}

#[test]
fn wallboard_union_uses_real_layout_and_ports_fit_cards_with_different_footer_heights() {
    let input = serde_json::json!({"node_width":440,"max_rows":8,
        "nodes":[{"id":"a","title":"First","priority":1,"status":"open","height":149},
            {"id":"b","title":"Second","priority":2,"status":"open","height":221}],
        "edges":[{"from":"a","to":"b"}]});
    let placed: serde_json::Value = serde_json::from_str(&wallboard_layout_json(input.to_string())).unwrap();
    assert_eq!(placed["boxes"].as_array().unwrap().len(), 2);
    let a_y = placed["boxes"][0]["y"].as_f64().unwrap();
    let b_y = placed["boxes"][1]["y"].as_f64().unwrap();
    let from_y = placed["routes"][0][0][1].as_f64().unwrap();
    let to_y = placed["routes"][0][1][1].as_f64().unwrap();
    assert!((a_y..a_y + 149.0).contains(&from_y));
    assert!((b_y..b_y + 221.0).contains(&to_y));
    assert_eq!(from_y, a_y + 74.5);
    assert_eq!(to_y, b_y + 110.5);
    assert_eq!(placed["boxes"][1]["x"].as_f64().unwrap() - placed["boxes"][0]["x"].as_f64().unwrap(), 740.0);
}
