//! The layout the runtime asks Rust for when it places cards itself: when cards that are leaving
//! still need their places, and when it fits the graph to the screen. A request holds cards read back
//! from frames the renderer drew; one that no such frame could give is refused, never filled in.
use super::wallboard_card_ports;
use crate::{fmt, layout};
use fwd_core::model::TaskStatus;
use serde::Deserialize;
use std::collections::HashMap;

/// What the runtime sends: the cards to place, the arrows between them, the cards' width, the row
/// cap, and the screen's size when it fits the graph to the screen.
#[derive(Deserialize)]
struct LayoutRequest {
    nodes: Vec<LayoutCard>,
    edges: Vec<LayoutArrow>,
    node_width: f64,
    max_rows: usize,
    viewport_width: Option<f64>,
    viewport_height: Option<f64>,
}

/// A card to place: what orders it as the renderer's packing orders it, and its height.
#[derive(Deserialize)]
struct LayoutCard {
    id: String,
    title: String,
    priority: Option<i64>,
    status: String,
    height: f64,
}

/// An arrow, from the card of the task waited for to the card of the task that waits.
#[derive(Deserialize)]
struct LayoutArrow {
    from: String,
    to: String,
}

/// The widest and narrowest cards the settings contract allows (`node_width` from 240 to 640, at a
/// text size from 80% to 160%), and its row caps.
const CARD_WIDTHS: std::ops::RangeInclusive<f64> = 192.0..=1024.0;
const ROW_CAPS: std::ops::RangeInclusive<usize> = 3..=16;

/// The cards placed by the renderer's own packing, as JSON: the drawing's size, the row cap it used,
/// each card's place and each arrow's route. "null" for a request that is not JSON, or that no frame
/// could give: a card with no id or one another card has, a status that is no task's, a height or
/// a width that is not a card's, a row cap the settings refuse, an arrow to a card not placed, or
/// half a screen. The runtime reports that as the graph's failure.
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn wallboard_layout_json(input: String) -> String {
    let placed = serde_json::from_str::<LayoutRequest>(&input).ok().and_then(|request| request.place());
    placed.map_or_else(|| "null".into(), |placed| placed.to_string())
}

impl LayoutRequest {
    fn place(&self) -> Option<serde_json::Value> {
        if !CARD_WIDTHS.contains(&self.node_width) || !ROW_CAPS.contains(&self.max_rows) {
            return None;
        }
        let mut tasks = Vec::with_capacity(self.nodes.len());
        let mut heights = Vec::with_capacity(self.nodes.len());
        for card in &self.nodes {
            let status: TaskStatus = card.status.parse().ok()?;
            if card.id.is_empty() || !(card.height.is_finite() && card.height > 0.0) {
                return None;
            }
            tasks.push(layout::Task {
                id: card.id.clone(),
                title: card.title.clone(),
                priority: card.priority,
                status: fmt::status_rank(status),
                band: None,
            });
            heights.push(card.height);
        }
        let index: HashMap<_, _> = tasks.iter().enumerate().map(|(i, task)| (task.id.as_str(), i)).collect();
        if index.len() != tasks.len() {
            return None;
        }
        let pairs: Vec<_> = self
            .edges
            .iter()
            .map(|edge| Some((*index.get(edge.from.as_str())?, *index.get(edge.to.as_str())?)))
            .collect::<Option<_>>()?;
        let viewport = match (self.viewport_width, self.viewport_height) {
            (None, None) => None,
            (Some(width), Some(height)) if [width, height].iter().all(|n| n.is_finite() && *n >= 0.0) => {
                // A screen with no room yet, before the page is laid out, has nothing to fit to.
                (width > 0.0 && height > 0.0).then_some((width, height))
            }
            _ => return None,
        };
        let slot_height = heights.iter().copied().fold(0.0, f64::max);
        let rows = self.max_rows;
        let mut drawing = layout::Layout::wallboard(&tasks, &pairs, rows, self.node_width, slot_height);
        let mut selected_rows = rows;
        if let Some((viewport_width, viewport_height)) = viewport {
            let fit =
                |drawing: &layout::Drawing| (viewport_width / drawing.width).min(viewport_height / drawing.height);
            // Keep the familiar arrangement on ties. Only reflow when cards become visibly larger.
            for cap in (3..rows.min(tasks.len())).rev() {
                let candidate = layout::Layout::wallboard(&tasks, &pairs, cap, self.node_width, slot_height);
                if fit(&candidate) > fit(&drawing) * 1.01 {
                    drawing = candidate;
                    selected_rows = cap;
                }
            }
        }
        wallboard_card_ports(&mut drawing, &pairs, &heights, slot_height);
        Some(serde_json::json!({"width": drawing.width, "height": drawing.height, "rows": selected_rows,
            "boxes": tasks.iter().zip(&drawing.boxes).map(|(task, (x, y))|
                serde_json::json!({"id": task.id, "x": x, "y": y})).collect::<Vec<_>>(),
            "routes": drawing.routes}))
    }
}

#[cfg(test)]
mod tests;
