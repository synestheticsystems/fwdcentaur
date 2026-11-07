//! The wallboard packing's tests, which build their tasks with the layout's test helpers.
use super::*;
use crate::layout::tests::{crossings_drawn, open, task};
use fwd_core::model::TaskStatus;

#[test]
fn wallboard_row_cap_keeps_every_task_and_dependency_without_crossing_cards() {
    let tasks: Vec<_> = (0..28).map(|n| open(&format!("Task {n:02}"))).collect();
    let edges = [(0, 21), (1, 22), (2, 23), (3, 24), (4, 25), (5, 26), (20, 27), (0, 27)];
    let drawing = Layout::wallboard(&tasks, &edges, 8, 360.0, 166.0);
    assert_eq!(drawing.boxes.len(), 28);
    let mut columns = BTreeMap::<i64, usize>::new();
    for &(x, _) in &drawing.boxes {
        *columns.entry(x as i64).or_default() += 1;
    }
    assert_eq!(columns.len(), 4);
    assert!(columns.values().all(|&rows| rows <= 8));
    assert!(drawing.height <= PAD * 2.0 + 8.0 * 166.0 + 7.0 * 22.0);
    assert!(
        drawing.routes.iter().all(|route| route.len() == 2),
        "unrelated cards must not force these components into long routes"
    );
    assert_ne!(drawing.routes[0][0].1, drawing.routes[7][0].1, "each outgoing arrow needs its own port");
    assert_ne!(
        drawing.routes[6].last().unwrap().1,
        drawing.routes[7].last().unwrap().1,
        "each incoming arrow needs its own port"
    );
    for (&(a, b), route) in edges.iter().zip(&drawing.routes) {
        assert!(drawing.boxes[a].0 < drawing.boxes[b].0);
        assert_eq!(route.first().unwrap().0, drawing.boxes[a].0 + 360.0);
        assert_eq!(route.last().unwrap().0, drawing.boxes[b].0);
        for segment in route.windows(2) {
            let ((x0, y0), (x1, y1)) = (segment[0], segment[1]);
            let mid = (x0 + x1) / 2.0;
            for step in 1..100 {
                let t = step as f64 / 100.0;
                let u = 1.0 - t;
                let px = u.powi(3) * x0 + 3.0 * u * u * t * mid + 3.0 * u * t * t * mid + t.powi(3) * x1;
                let py = u.powi(3) * y0 + 3.0 * u * u * t * y0 + 3.0 * u * t * t * y1 + t.powi(3) * y1;
                assert!(
                    drawing.boxes.iter().all(|&(x, y)| px <= x || px >= x + 360.0 || py <= y || py >= y + 166.0),
                    "smooth dependency route crosses a card"
                );
            }
        }
    }
}

#[test]
fn wallboard_packs_isolated_tasks_and_keeps_cycles_visible() {
    let tasks: Vec<_> = (0..29).map(|n| open(&format!("Task {n:02}"))).collect();
    for cap in [3, 8, 16] {
        let drawing = Layout::wallboard(&tasks, &[], cap, 360.0, 166.0);
        let mut columns = BTreeMap::<i64, usize>::new();
        for &(x, _) in &drawing.boxes {
            *columns.entry(x as i64).or_default() += 1;
        }
        assert_eq!(drawing.boxes.len(), tasks.len());
        assert_eq!(columns.len(), tasks.len().div_ceil(cap));
        assert!(columns.values().all(|&rows| rows <= cap));
    }
    let drawing = Layout::wallboard(&tasks, &[(0, 1), (1, 2), (2, 0)], 8, 360.0, 166.0);
    assert_eq!(drawing.boxes.len(), 29);
    assert!(drawing.routes.iter().all(|r| !r.is_empty()));
}

#[test]
fn wallboard_untangles_a_fan_out_and_gives_only_busy_gutters_more_room() {
    let tasks: Vec<_> = (0..28).map(|n| open(&format!("Task {n:02}"))).collect();
    let edges =
        [(0, 7), (1, 7), (2, 11), (2, 7), (3, 11), (4, 10), (5, 11), (5, 7), (5, 6), (5, 10), (5, 9), (5, 8), (12, 13)];
    let drawing = Layout::wallboard(&tasks, &edges, 8, 440.0, 149.0);
    assert_eq!(crossings_drawn(&drawing.routes, drawing.width), 2);
    let mut columns: Vec<_> = drawing.boxes.iter().map(|&(x, _)| x).collect();
    columns.sort_by(f64::total_cmp);
    columns.dedup();
    assert_eq!(columns.len(), 4);
    assert_eq!(columns[1] - columns[0] - 440.0, 300.0);
    assert_eq!(columns[2] - columns[1] - 440.0, 64.0);
    let ports: Vec<_> =
        edges.iter().zip(&drawing.routes).filter(|(edge, _)| edge.0 == 5).map(|(_, route)| route[0].1).collect();
    let span =
        ports.iter().copied().fold(f64::NEG_INFINITY, f64::max) - ports.iter().copied().fold(f64::INFINITY, f64::min);
    assert_eq!(span, 112.0);
}

#[test]
fn wallboard_promoted_card_rises_above_connected_lower_priority_work() {
    let mut tasks = vec![
        task("First prerequisite", Some(2), TaskStatus::Open),
        task("First dependent", Some(2), TaskStatus::Open),
        open("Second prerequisite"),
        open("Second dependent"),
        task("Standalone promoted later", Some(3), TaskStatus::Open),
        open("Other work"),
    ];
    let edges = [(0, 1), (2, 3)];
    let before = Layout::wallboard(&tasks, &edges, 4, 440.0, 149.0);
    assert!(before.boxes[4].1 > before.boxes[0].1);
    tasks[4].priority = Some(1);
    let after = Layout::wallboard(&tasks, &edges, 4, 440.0, 149.0);
    assert_eq!(after.boxes[4].0, before.boxes[4].0);
    assert_eq!(after.boxes[4].1, PAD);
    assert!(after.boxes[4].1 < before.boxes[4].1);
    for drawing in [&before, &after] {
        for &(a, b) in &edges {
            assert!(drawing.boxes[a].0 < drawing.boxes[b].0);
        }
        assert_eq!(drawing.boxes.len(), tasks.len());
    }
    for (a, &(x, y)) in after.boxes.iter().enumerate() {
        for (b, &(other_x, other_y)) in after.boxes.iter().enumerate() {
            if x == other_x && y < other_y {
                let key = |i: usize| (tasks[i].priority.is_none(), tasks[i].priority.unwrap_or(0));
                assert!(key(a) <= key(b), "priority inverted within a column");
            }
        }
    }
}
