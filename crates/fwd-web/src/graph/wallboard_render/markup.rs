//! The SVG of one frame: the cards where the packing put them, with their counts of messages, and the
//! arrows between them.
// The Graph page's private item the arrows are drawn with. A rename in graph.rs fails here.
use super::super::path;
use super::cards::Cards;
use crate::{fmt, layout};
use ankurah::EntityId;
use fwd_core::model::{MessageView, TaskStatus};
use std::collections::HashMap;

/// The number of messages on each task that has any, counted in one pass over them all.
pub(super) fn message_counts(messages: &[MessageView]) -> HashMap<EntityId, usize> {
    let mut counts = HashMap::new();
    for task in messages.iter().filter_map(|m| m.task().ok().flatten().map(EntityId::from)) {
        *counts.entry(task).or_default() += 1;
    }
    counts
}

/// The frame's SVG: `cards` at the places `drawing` gives them, each with its count from `counts`.
pub(super) fn frame(cards: &Cards, drawing: &layout::Drawing, counts: &HashMap<EntityId, usize>) -> String {
    let Cards { width, slot_height: height, content_width, text_scale, max_rows, .. } = *cards;
    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" class="dag wallboard-dag" width="{}" height="{}" viewBox="0 0 {} {}" data-max-rows="{max_rows}" data-node-height="{height}" data-node-width="{width}"><defs><marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto"><path d="M0,0 L10,5 L0,10 z" class="arrowhead"/></marker></defs>"#,
        drawing.width, drawing.height, drawing.width, drawing.height
    );
    for (arrow, route) in cards.arrows.iter().zip(&drawing.routes) {
        if let Some(path) = path(route) {
            svg.push_str(&format!(r#"<path class="edge edge-depends{}" d="{path}" marker-end="url(#arrow)" data-from="{}" data-to="{}" data-route="{}"/>"#, if arrow.proposed { " edge-proposed" } else { "" }, cards.cards[arrow.first].id.to_base64(), cards.cards[arrow.then].id.to_base64(), fmt::escape(&serde_json::to_string(route).unwrap_or_default())));
        }
    }
    for (card, &(x, y)) in cards.cards.iter().zip(&drawing.boxes) {
        let status = card.status;
        let mut tint = card.priority.map(|p| format!(" priority-p{}", p.clamp(0, 3))).unwrap_or_default();
        if card.waiting {
            tint.push_str(" availability-waiting");
        }
        let what = fmt::truncate(&card.what, 100);
        let title_lines = wallboard_wrap(&card.title, ((content_width - 24.0) / 11.8) as usize, 2);
        let what_lines = wallboard_wrap(&what, ((content_width - 24.0) / 8.0) as usize, 2);
        svg.push_str(&format!(r#"<g class="node wallboard-node status-{status}{tint}{}" data-id="{}" data-layout-title="{}" data-priority="{}" data-revision="{:x}" transform="translate({x},{y})"><title>{}</title><rect class="wallboard-card" width="{width}" height="{}" rx="9"/>"#, if status == TaskStatus::Proposed { " ghost" } else { "" }, card.id.to_base64(), fmt::escape(&card.title), card.priority.map(|p| p.to_string()).unwrap_or_default(), card.revision, fmt::escape(&format!("{}\n{what}\n{}", card.title, card.parent)), card.height));
        svg.push_str(&format!(r#"<g class="wallboard-card-content" transform="scale({text_scale})">"#));
        for (line, text) in title_lines.iter().enumerate() {
            svg.push_str(&format!(r#"<text class="title" x="12" y="{}">{}</text>"#, 28 + line * 25, fmt::escape(text)));
        }
        for (line, text) in what_lines.iter().enumerate() {
            svg.push_str(&format!(
                r#"<text class="wallboard-what" x="12" y="{}">{}</text>"#,
                79 + line * 20,
                fmt::escape(text)
            ));
        }
        svg.push_str(&card.footer);
        if !card.parent.is_empty() {
            svg.push_str(&format!(
                r#"<text class="wallboard-parent" x="12" y="{}">{}</text>"#,
                card.height / text_scale - 12.0,
                fmt::escape(&fmt::truncate(&card.parent, ((content_width - 24.0) / 7.0) as usize))
            ));
        }
        let count = counts.get(&card.id).copied().unwrap_or(0);
        if count > 0 {
            svg.push_str(&format!(r#"<g class="count" transform="translate({},0)"><circle r="10"/><text text-anchor="middle" y="4">{count}</text></g>"#, content_width - 12.0));
        }
        svg.push_str("</g></g>");
    }
    svg.push_str("</svg>");
    svg
}

/// Whitespace-normalized, Unicode-safe SVG lines. The final line says when text was shortened.
fn wallboard_wrap(text: &str, columns: usize, lines: usize) -> Vec<String> {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars: Vec<char> = normalized.chars().collect();
    let mut result = Vec::new();
    let columns = columns.max(2);
    while !chars.is_empty() && result.len() < lines {
        if chars.len() <= columns {
            result.push(chars.iter().collect());
            break;
        }
        if result.len() + 1 == lines {
            result.push(format!("{}…", chars[..columns - 1].iter().collect::<String>().trim_end()));
            break;
        }
        let end = if chars[columns] == ' ' {
            columns
        } else {
            chars[..columns].iter().rposition(|c| *c == ' ').filter(|&n| n > 0).unwrap_or(columns)
        };
        result.push(chars[..end].iter().collect());
        chars.drain(..end);
        while chars.first() == Some(&' ') {
            chars.remove(0);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wallboard_wraps_unicode_text_on_spaces_and_says_where_it_shortened() {
        let lines = wallboard_wrap("  First\nsecond   third fourth fifth sixth ", 12, 2);
        assert_eq!(lines, ["First second", "third fourt…"]);
        let lines = wallboard_wrap("日本語の長い文字列でも途中で壊れない", 5, 3);
        assert_eq!(lines.len(), 3);
        assert!(lines.iter().all(|s| s.chars().count() <= 5));
    }
}
