//! The layout requests' tests: what the runtime is answered, and what it is refused.
use super::*;

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

/// A layout request that breaks one rule, said in words and as the edit of a good request that
/// makes it.
type BrokenRule = (&'static str, fn(&mut serde_json::Value));

#[test]
fn a_layout_request_no_frame_could_give_is_refused() {
    let request = serde_json::json!({"node_width": 440, "max_rows": 8,
        "nodes": [{"id": "a", "title": "First", "priority": 1, "status": "open", "height": 149},
            {"id": "b", "title": "Second", "priority": null, "status": "in_progress", "height": 221}],
        "edges": [{"from": "a", "to": "b"}]});
    assert_ne!(wallboard_layout_json(request.to_string()), "null", "the request each case below breaks is laid out");
    let cases: [BrokenRule; 13] = [
        ("a card with no id", |r| r["nodes"][0]["id"] = "".into()),
        ("two cards with one id", |r| r["nodes"][1]["id"] = "a".into()),
        ("a card with no status", |r| drop(r["nodes"][0].as_object_mut().unwrap().remove("status"))),
        ("a status no task has", |r| r["nodes"][0]["status"] = "finished".into()),
        ("a card with no height", |r| drop(r["nodes"][0].as_object_mut().unwrap().remove("height"))),
        ("a card of no height", |r| r["nodes"][0]["height"] = 0.into()),
        ("a priority that is no number", |r| r["nodes"][0]["priority"] = "high".into()),
        ("a card wider than any the settings allow", |r| r["node_width"] = 2000.into()),
        ("a row cap the settings refuse", |r| r["max_rows"] = 40.into()),
        ("an arrow to a card not placed", |r| r["edges"][0]["to"] = "c".into()),
        ("no cards at all", |r| drop(r.as_object_mut().unwrap().remove("nodes"))),
        ("half a screen", |r| r["viewport_width"] = 1920.into()),
        ("a screen of negative width", |r| {
            r["viewport_width"] = (-1).into();
            r["viewport_height"] = 1080.into();
        }),
    ];
    for (case, break_it) in cases {
        let mut broken = request.clone();
        break_it(&mut broken);
        assert_eq!(wallboard_layout_json(broken.to_string()), "null", "{case}");
    }
    assert_eq!(wallboard_layout_json("not JSON".into()), "null");
}
