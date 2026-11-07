//! What the wallboard's frame shows of the store: which tasks it draws, and everything each card says
//! but its count of messages, which the frame's markup reads apart (`super::markup`).
// The Graph page's private items the cards are read with. A rename in graph.rs fails here.
use super::super::{also_part_of, implied, Edge};
use crate::wallboard::{Availability, Dependencies, Presentation, TaskSelection};
use crate::{clock, fmt, store};
use ankurah::EntityId;
use fwd_core::model::{RelationKind, RelationStatus, TaskStatus, TaskView};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

/// Every card of a frame, the arrows between them and the sizes the presentation gives them: what the
/// packing and the markup both read. It holds no count of messages, so that a new message leaves it
/// as it was.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Cards {
    pub(super) cards: Vec<Card>,
    /// The dependencies drawn, a chain's implied ones left out.
    pub(super) arrows: Vec<Arrow>,
    /// Each card's width on the page, the text scale included.
    pub(super) width: f64,
    /// The height each card's place in a column has: the tallest card's.
    pub(super) slot_height: f64,
    /// The width a card's content is laid out in before the text scale applies, and that scale.
    pub(super) content_width: f64,
    pub(super) text_scale: f64,
    pub(super) max_rows: usize,
}

/// What one card says.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Card {
    pub(super) id: EntityId,
    pub(super) title: String,
    /// Its WHAT, its whitespace collapsed.
    pub(super) what: String,
    pub(super) status: TaskStatus,
    pub(super) priority: Option<i64>,
    /// The tasks it is part of, as its line of their own says them; empty for none drawn.
    pub(super) parent: String,
    /// The markup of its footer: its availability, its scopes, its status and assignee, its priority.
    pub(super) footer: String,
    pub(super) height: f64,
    /// Blocked by an unfinished prerequisite or held, so drawn faded.
    pub(super) waiting: bool,
    /// What the runtime compares across frames to tell an edit of its task from no change (`changes`
    /// in wallboard-history.mjs), written into the frame as the card's data-revision.
    pub(super) revision: u64,
}

/// A dependency drawn between two cards, by their places in `Cards::cards`: from the one waited for
/// to the one that waits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Arrow {
    pub(super) first: usize,
    pub(super) then: usize,
    pub(super) proposed: bool,
}

/// What a card says that its task alone gives, as `read` takes it from the store: all of a `Card`
/// but the line naming the tasks it is part of, which the relations between the cards drawn give,
/// and the height that line adds.
struct DrawnTask {
    id: EntityId,
    title: String,
    what: String,
    status: TaskStatus,
    priority: Option<i64>,
    footer: String,
    footer_rows: usize,
    waiting: bool,
    revision: u64,
}

/// Reads the frame's cards from the store: the tasks `selection` keeps that are due, their relations
/// to each other, and their prerequisites' statuses through `prerequisites`, which watches them.
pub(super) fn read(selection: &TaskSelection, presentation: &Presentation, prerequisites: &mut Dependencies) -> Cards {
    let tasks = wallboard_tasks(selection);
    let all_relations = store::relations();
    let statuses = prerequisites.statuses(&tasks, &all_relations);
    let availability: Vec<_> = tasks
        .iter()
        .map(|task| {
            let dependencies: Vec<_> = all_relations
                .iter()
                .filter(|r| {
                    r.from_id().ok() == Some(task.id())
                        && r.kind().ok() == Some(RelationKind::DependsOn)
                        && r.status().ok() == Some(RelationStatus::Accepted)
                })
                .filter_map(|r| r.to_id().ok())
                .map(|id| (id, statuses.get(&id).copied()))
                .collect();
            Availability::of(task.id(), task.hold().ok().flatten(), &dependencies)
        })
        .collect();
    let scopes = store::scopes();
    let content_width = presentation.node_width as f64;
    let mut drawn = Vec::with_capacity(tasks.len());
    for (task, availability) in tasks.iter().zip(&availability) {
        let names: Vec<_> = store::scopes_of_task(task.id())
            .iter()
            .map(|id| {
                scopes.iter().find(|s| s.id() == *id).and_then(|s| s.title().ok()).unwrap_or_else(|| fmt::short(*id))
            })
            .collect();
        let status = task.status().unwrap_or(TaskStatus::Proposed);
        let mut metadata = fmt::status_label(status).to_string();
        if let Some(assignee) = fwd_core::ops::assignee_of(task).ok().flatten() {
            metadata.push_str(&format!(" · {}", store::handle_of(assignee)));
        }
        let priority = task.priority().ok().flatten();
        let (footer, footer_rows) = wallboard_footer(&names, &metadata, priority, content_width, availability);
        let title = task.title().unwrap_or_default();
        let what = task.what().unwrap_or_default().split_whitespace().collect::<Vec<_>>().join(" ");
        let mut revision = std::hash::DefaultHasher::new();
        // Hash full WHAT and the actual task update stamp, rather than only its visible excerpt, and
        // the task's relations, rather than the arrows and parent line drawn from them. A count of
        // messages is no part of it, so that a new message cannot read as an edit of its task.
        (
            &title,
            &what,
            task.updated().ok(),
            status.to_string(),
            task.priority().ok(),
            &footer,
            wallboard_relation_facts(task.id(), store::relations_of(task.id()).iter().filter_map(Edge::of)),
        )
            .hash(&mut revision);
        drawn.push(DrawnTask {
            id: task.id(),
            title,
            what,
            status,
            priority,
            footer,
            footer_rows,
            waiting: availability.quiet(),
            revision: revision.finish(),
        });
    }
    let edges: Vec<_> = all_relations.iter().filter_map(Edge::of).collect();
    wallboard_cards(drawn, &edges, presentation)
}

/// The frame's cards, the tasks `drawn` in their order, each with the line naming the tasks it is part
/// of, and the arrows between them. Of `edges`, every relation the store holds, only those between two
/// tasks drawn count, so that nothing on the frame names a task it leaves out.
fn wallboard_cards(drawn: Vec<DrawnTask>, edges: &[Edge], presentation: &Presentation) -> Cards {
    let ids: HashSet<_> = drawn.iter().map(|task| task.id).collect();
    let edges: Vec<_> = edges.iter().copied().filter(|e| wallboard_relation(&ids, e)).collect();
    let titles: HashMap<_, _> = drawn.iter().map(|task| (task.id, task.title.clone())).collect();
    let places: HashMap<_, _> = drawn.iter().enumerate().map(|(i, task)| (task.id, i)).collect();
    let (width, base_height) = wallboard_card_size(presentation, 0, false);
    let cards: Vec<_> = drawn
        .into_iter()
        .map(|task| {
            let named: Vec<_> =
                edges.iter().copied().filter(|e| e.kind == RelationKind::SubtaskOf && e.first == task.id).collect();
            let parent = also_part_of(&named, false, &|id| titles[id].clone());
            Card {
                id: task.id,
                height: wallboard_card_size(presentation, task.footer_rows, !parent.is_empty()).1,
                title: task.title,
                what: task.what,
                status: task.status,
                priority: task.priority,
                parent,
                footer: task.footer,
                waiting: task.waiting,
                revision: task.revision,
            }
        })
        .collect();
    Cards {
        slot_height: cards.iter().map(|card| card.height).fold(base_height, f64::max),
        arrows: wallboard_arrows(&edges, &places),
        cards,
        width,
        content_width: presentation.node_width as f64,
        text_scale: presentation.text_scale_percent as f64 / 100.0,
        max_rows: presentation.max_rows,
    }
}

/// The arrows the frame draws for `edges`, the relations between the cards drawn, which `places`
/// gives the places of: each dependency, less the accepted ones a chain of others already implies.
fn wallboard_arrows(edges: &[Edge], places: &HashMap<EntityId, usize>) -> Vec<Arrow> {
    let dependencies: Vec<_> = edges.iter().copied().filter(|e| e.kind == RelationKind::DependsOn).collect();
    let implied = implied(&dependencies);
    dependencies
        .into_iter()
        .filter(|e| e.proposed || !implied.contains(&(e.first, e.then)))
        .map(|e| Arrow { first: places[&e.first], then: places[&e.then], proposed: e.proposed })
        .collect()
}

/// Whether the frame draws `edge`: only when it draws the tasks at both its ends, `ids`.
fn wallboard_relation(ids: &HashSet<EntityId>, edge: &Edge) -> bool {
    ids.contains(&edge.first) && ids.contains(&edge.then)
}

/// Whether the wallboard draws a task of `status`, given the date it comes due, if it is not due yet
/// (`clock::comes_due`).
fn wallboard_draws(selection: &TaskSelection, status: Option<TaskStatus>, comes_due: Option<i64>) -> bool {
    selection.keeps(status) && comes_due.is_none()
}

/// Tasks not yet due leave the drawing here, not the task query: the clock finds the dates to come
/// among the store's tasks, and a date passing changes nothing a live query could hear.
fn wallboard_tasks(selection: &TaskSelection) -> Vec<TaskView> {
    let mut tasks: Vec<_> = store::tasks()
        .into_iter()
        .filter(|t| wallboard_draws(selection, t.status().ok(), clock::comes_due(t)))
        .collect();
    tasks.sort_by_key(|t| t.id());
    tasks
}

/// What the other task of a relation is to a card.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
enum WallboardOtherEnd {
    Parent,
    Prerequisite,
    Dependent,
}

/// What a card's revision states of its task's relations, so that a real relation change edits the
/// card and another card coming or going does not: each parent and each dependency in either direction,
/// with the other task and whether it is only proposed, in an order that does not change. `edges` are
/// every relation at the task, whether or not the other card is drawn. A relation linked twice is one
/// fact.
fn wallboard_relation_facts(
    task: EntityId,
    edges: impl Iterator<Item = Edge>,
) -> Vec<(WallboardOtherEnd, EntityId, bool)> {
    let mut facts: Vec<_> = edges
        .filter_map(|e| {
            let (end, other) = match e.kind {
                RelationKind::SubtaskOf if e.first == task => (WallboardOtherEnd::Parent, e.then),
                RelationKind::DependsOn if e.then == task => (WallboardOtherEnd::Prerequisite, e.first),
                RelationKind::DependsOn if e.first == task => (WallboardOtherEnd::Dependent, e.then),
                _ => return None,
            };
            Some((end, other, e.proposed))
        })
        .collect();
    facts.sort_unstable();
    facts.dedup();
    facts
}

fn wallboard_footer(
    names: &[String],
    metadata: &str,
    priority: Option<i64>,
    width: f64,
    availability: &Availability,
) -> (String, usize) {
    let (mut x, mut row) = (12.0, 0usize);
    let right = width - 12.0 - if priority.is_some() { 44.0 } else { 0.0 };
    let mut markup = String::new();
    let availability_labels = availability.labels();
    let chips = availability_labels
        .iter()
        .map(|name| (*name, "wallboard-availability"))
        .chain(names.iter().map(|name| (name.as_str(), "wallboard-scope")));
    for (name, class) in chips {
        let label = fmt::truncate(name, ((right - 30.0) / 7.6).floor() as usize);
        let chip_width = (label.chars().count() as f64 * 7.6 + 18.0).min(right - 12.0);
        if x > 12.0 && x + chip_width > right {
            row += 1;
            x = 12.0;
        }
        let y = 114.0 + row as f64 * 24.0;
        markup.push_str(&format!(r#"<g class="{class}" transform="translate({x},{y})"><title>{}</title><rect width="{chip_width}" height="21" rx="10"/><text x="9" y="15">{}</text></g>"#, fmt::escape(name), fmt::escape(&label)));
        x += chip_width + 10.0;
    }
    if !metadata.is_empty() {
        let label = fmt::truncate(metadata, ((right - 12.0) / 7.4).floor() as usize);
        if x > 12.0 && x + label.chars().count() as f64 * 7.4 > right {
            row += 1;
            x = 12.0;
        }
        let y = 129.0 + row as f64 * 24.0;
        markup.push_str(&format!(
            r#"<text class="sub" x="{x}" y="{y}"><title>{}</title>{}</text>"#,
            fmt::escape(metadata),
            fmt::escape(&label)
        ));
    }
    if let Some(priority) = priority {
        let x = width - 48.0;
        let y = 114.0 + row as f64 * 24.0;
        markup.push_str(&format!(r#"<g class="wallboard-priority p{}" transform="translate({x},{y})"><rect width="36" height="21" rx="5"/><text x="18" y="15" text-anchor="middle">P{priority}</text></g>"#, priority.clamp(0,3)));
    }
    (markup, if names.is_empty() && metadata.is_empty() { 0 } else { row + 1 })
}

/// A card's size at the configured text scale. The base card holds two title lines, two lines of
/// WHAT and one footer row; each further footer row and a parent line add height. Both dimensions
/// scale with the text, so larger text keeps the same amount of visible context.
fn wallboard_card_size(presentation: &Presentation, footer_rows: usize, parent: bool) -> (f64, f64) {
    let scale = presentation.text_scale_percent as f64 / 100.0;
    let height = 149.0 + footer_rows.saturating_sub(1) as f64 * 24.0 + if parent { 20.0 } else { 0.0 };
    (presentation.node_width as f64 * scale, height * scale)
}

#[cfg(test)]
mod tests;
