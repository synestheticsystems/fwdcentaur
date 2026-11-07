//! The SVG of one frame: the cards where the packing put them, with their counts of messages, and the
//! arrows between them.
// The Graph page's private item the arrows are drawn with. A rename in graph.rs fails here.
use super::super::path;
use super::cards::Cards;
use super::columns;
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
        // The card's text, clipped to the card: a nested svg clips what it holds to its own size, so that
        // text wider than its columns reckon, as capitals are in most fonts, cannot run past the card.
        svg.push_str(&format!(r#"<svg class="wallboard-card-clip" width="{width}" height="{}"><g class="wallboard-card-content" transform="scale({text_scale})">"#, card.height));
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
                fmt::escape(&columns::truncate(&card.parent, ((content_width - 24.0) / 7.0) as usize))
            ));
        }
        svg.push_str("</g></svg>");
        // The count sits across the card's top edge, so it is drawn outside the clip.
        let count = counts.get(&card.id).copied().unwrap_or(0);
        if count > 0 {
            svg.push_str(&format!(r#"<g class="count" transform="scale({text_scale}) translate({},0)"><circle r="10"/><text text-anchor="middle" y="4">{count}</text></g>"#, content_width - 12.0));
        }
        svg.push_str("</g>");
    }
    svg.push_str("</svg>");
    svg
}

/// Whitespace-normalized, Unicode-safe SVG lines of `max_columns` columns at most (`columns`), and
/// `lines` lines at most. The final line says when text was shortened.
fn wallboard_wrap(text: &str, max_columns: usize, lines: usize) -> Vec<String> {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars: Vec<char> = normalized.chars().collect();
    let mut result = Vec::new();
    // Two columns at the least, so that a wide character fits on a line.
    let max_columns = max_columns.max(2);
    while !chars.is_empty() && result.len() < lines {
        let fit = columns::fitting(&chars, max_columns);
        if fit == chars.len() {
            result.push(chars.iter().collect());
            break;
        }
        if result.len() + 1 == lines {
            let fit = columns::fitting(&chars, max_columns - 1);
            result.push(format!("{}…", chars[..fit].iter().collect::<String>().trim_end()));
            break;
        }
        let end = if chars[fit] == ' ' {
            fit
        } else {
            chars[..fit].iter().rposition(|c| *c == ' ').filter(|&n| n > 0).unwrap_or(fit)
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

    use super::super::cards::Card;

    #[test]
    fn wallboard_wraps_unicode_text_on_spaces_and_says_where_it_shortened() {
        let lines = wallboard_wrap("  First\nsecond   third fourth fifth sixth ", 12, 2);
        assert_eq!(lines, ["First second", "third fourt…"]);
        let lines = wallboard_wrap("日本語の長い文字列でも途中で壊れない", 5, 3);
        assert_eq!(lines.len(), 3);
        assert!(lines.iter().all(|line| columns::width(line) <= 5));
    }

    #[test]
    fn a_title_of_wide_characters_wraps_at_half_as_many_of_them() {
        // A default card's title has 28 columns: fourteen wide characters, or thirteen and an ellipsis.
        let lines = wallboard_wrap("新しい表示の配置を確認してから全てのカードを並べ直す作業を終える", 28, 2);
        assert_eq!(lines.iter().map(|line| line.chars().count()).collect::<Vec<_>>(), [14, 14]);
        assert!(lines[1].ends_with('…'), "the last line shortened, its thirteen and an ellipsis");
    }

    #[test]
    fn a_title_with_emoji_wraps_where_its_pictures_fill_the_line() {
        // A default card's 336-pixel title area holds 28 columns at the title's 11.8 pixels a column.
        let title = "Launch 🚀✅⌚🪄🩷🚀✅⌚🪄🩷 checklist";
        assert_eq!(columns::width(title), 37, "ten emoji of two columns each");
        let lines = wallboard_wrap(title, (336.0 / 11.8) as usize, 2);
        assert_eq!(lines, ["Launch 🚀✅⌚🪄🩷🚀✅⌚🪄🩷", "checklist"]);
        assert_eq!(columns::width(&lines[0]), 27);
    }

    #[test]
    fn a_cards_text_is_clipped_to_the_card_and_its_count_drawn_outside_the_clip() {
        let id = EntityId::from_bytes([1; 32]);
        let card = Card {
            id,
            title: "AN ALL-CAPITALS TITLE WIDER THAN ITS COLUMNS RECKON".into(),
            what: "An excerpt ".repeat(30),
            status: TaskStatus::Open,
            priority: Some(1),
            parent: String::new(),
            footer: String::new(),
            height: 173.0,
            waiting: false,
            revision: 1,
        };
        let cards = Cards {
            cards: vec![card],
            arrows: Vec::new(),
            width: 360.0,
            slot_height: 173.0,
            content_width: 360.0,
            text_scale: 1.0,
            max_rows: 8,
        };
        let drawing = layout::Drawing {
            boxes: vec![(24.0, 24.0)],
            routes: Vec::new(),
            threaded: Vec::new(),
            bands: Vec::new(),
            width: 408.0,
            height: 221.0,
        };
        let svg = frame(&cards, &drawing, &HashMap::from([(id, 3)]));
        let clip = r#"<svg class="wallboard-card-clip" width="360" height="173">"#;
        let (_, clipped) = svg.split_once(clip).expect("the card's text is in a nested svg of the card's size");
        let (inside, after) = clipped.split_once("</svg>").unwrap();
        assert!(inside.contains(r#"class="title""#) && inside.contains(r#"class="wallboard-what""#));
        assert!(!inside.contains(r#"class="count""#), "the count, across the card's edge, is not clipped");
        assert!(after.starts_with(r#"<g class="count""#));
    }
}
