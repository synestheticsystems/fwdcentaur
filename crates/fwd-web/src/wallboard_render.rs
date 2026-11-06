//! The wallboard renderer and its Rust/JavaScript adapter.
// main.rs lets what only the app uses go unused in the wallboard's build; the wallboard's own code may not.
#![warn(dead_code)]
// The graph page's private items this renderer shares. A rename in graph.rs fails here, in one place.
use super::{also_part_of, implied, path, Edge};
use crate::{clock, fmt, layout, session, store};
use ankurah::signals::Get as AnkurahGet;
use ankurah::EntityId;
use fwd_core::model::{MessageView, RelationKind, RelationStatus, RelationView, ScopeView, TaskStatus, TaskView};
use leptos::prelude::*;
use std::collections::{HashMap, HashSet};

/// The wallboard draws only the selected active statuses, and of those only the tasks that are due.
/// Hierarchy is named on the cards; dependency arrows retain their meaning while the compact layout
/// enforces a row cap. Only the presentation reacts: the selection is the one the store's task query
/// was opened with.
#[component]
pub fn WallboardGraph(
    presentation: RwSignal<crate::wallboard::Presentation>,
    selection: crate::wallboard::TaskSelection,
) -> impl IntoView {
    // Keeps what the drawing says about dates true while the page runs. Started here rather than at
    // the top of the display, because its checks read the store: a cold start while no daemon answers
    // can wait for the store for longer than a check takes to come round.
    clock::keep_time();
    let host = NodeRef::<leptos::html::Div>::new();
    let runtime_settings = {
        let selection = selection.clone();
        Memo::new(move |_| {
            let settings =
                crate::wallboard::Settings { selection: selection.clone(), presentation: presentation.get() };
            let mut value = settings.json();
            value["project_id"] = serde_json::json!(session::project().id);
            value.to_string()
        })
    };
    let prerequisites = StoredValue::new(send_wrapper::SendWrapper::new(crate::wallboard::Dependencies::default()));
    let drawing = Memo::new(move |_| {
        let begin = js_sys::Date::now();
        let presentation = presentation.get();
        let (tasks, relations) = wallboard_tasks(&selection);
        let all_relations = store::relations();
        let mut statuses = HashMap::new();
        prerequisites.update_value(|queries| statuses = queries.statuses(&tasks, &all_relations));
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
                crate::wallboard::Availability::of(task.id(), task.hold().ok().flatten(), &dependencies)
            })
            .collect();
        let svg =
            wallboard_render(&tasks, &relations, &store::scopes(), &store::messages(), &presentation, &availability);
        // Measurements are outside the memo's value: equal SVG must stay equal even when
        // computation time changes, or diagnostics themselves would trigger DOM work.
        if let Some(element) = host.get_untracked() {
            wallboard_record_render(element.as_ref(), js_sys::Date::now() - begin);
        }
        svg
    });
    Effect::new(move |mounted: Option<bool>| {
        let svg = drawing.get();
        let settings = runtime_settings.get();
        let loaded = store::standing_loaded();
        let connected = session::client().connection_state().get().to_string() == "Connected";
        let Some(element) = host.get() else {
            return false;
        };
        if !mounted.unwrap_or(false) {
            let layout =
                wasm_bindgen::closure::Closure::<dyn Fn(String) -> String>::new(wallboard_layout_json).into_js_value();
            wallboard_mount(element.as_ref(), &settings, &layout);
        }
        wallboard_update(element.as_ref(), &svg, loaded, connected, &settings);
        true
    });
    on_cleanup(move || {
        if let Some(element) = host.get_untracked() {
            wallboard_dispose(element.as_ref());
        }
    });
    view! {
        <div class="wallboard-graph" node_ref=host role="img" aria-label="Task graph"
            data-loaded=move || store::standing_loaded().to_string()></div>
    }
}

#[wasm_bindgen::prelude::wasm_bindgen(module = "/src/wallboard-runtime.mjs")]
extern "C" {
    #[wasm_bindgen(js_name = mount)]
    fn wallboard_mount(host: &web_sys::Element, settings: &str, layout: &wasm_bindgen::JsValue);
    #[wasm_bindgen(js_name = update)]
    fn wallboard_update(host: &web_sys::Element, svg: &str, loaded: bool, connected: bool, settings: &str);
    #[wasm_bindgen(js_name = recordRustRender)]
    fn wallboard_record_render(host: &web_sys::Element, elapsed_ms: f64);
    #[wasm_bindgen(js_name = dispose)]
    fn wallboard_dispose(host: &web_sys::Element);
}

/// The same layout also places a temporary union containing cards leaving the filter. Keeping
/// them in the layout until their countdown ends prevents another card moving underneath one.
/// Production hands this function to the runtime through `mount`. It is also a public WASM export
/// so that the synthetic WASM browser check and the profiler can drive the production layout
/// without booting the Rust display or connecting to a daemon.
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn wallboard_layout_json(input: String) -> String {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&input) else {
        return "null".into();
    };
    let Some(nodes) = value["nodes"].as_array() else {
        return "null".into();
    };
    let tasks: Vec<_> = nodes
        .iter()
        .map(|node| layout::Task {
            id: node["id"].as_str().unwrap_or_default().into(),
            title: node["title"].as_str().unwrap_or_default().into(),
            priority: node["priority"].as_i64(),
            status: fmt::status_rank(
                node["status"].as_str().and_then(|s| s.parse().ok()).unwrap_or(TaskStatus::Proposed),
            ),
            band: None,
        })
        .collect();
    let index: HashMap<_, _> = tasks.iter().enumerate().map(|(i, t)| (t.id.as_str(), i)).collect();
    let pairs: Vec<_> = value["edges"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|edge| Some((*index.get(edge["from"].as_str()?)?, *index.get(edge["to"].as_str()?)?)))
        .collect();
    let width = value["node_width"].as_f64().filter(|n| (192.0..=1024.0).contains(n)).unwrap_or(360.0);
    let rows = value["max_rows"].as_u64().filter(|n| (3..=16).contains(n)).unwrap_or(8) as usize;
    let height = nodes.iter().filter_map(|node| node["height"].as_f64()).reduce(f64::max).unwrap_or(149.0);
    let mut drawing = layout::Layout::wallboard(&tasks, &pairs, rows, width, height);
    let mut selected_rows = rows;
    if let (Some(viewport_width), Some(viewport_height)) = (
        value["viewport_width"].as_f64().filter(|n| n.is_finite() && *n > 0.0),
        value["viewport_height"].as_f64().filter(|n| n.is_finite() && *n > 0.0),
    ) {
        let fit = |drawing: &layout::Drawing| (viewport_width / drawing.width).min(viewport_height / drawing.height);
        // Keep the familiar arrangement on ties. Only reflow when cards become visibly larger.
        for cap in (3..rows.min(tasks.len())).rev() {
            let candidate = layout::Layout::wallboard(&tasks, &pairs, cap, width, height);
            if fit(&candidate) > fit(&drawing) * 1.01 {
                drawing = candidate;
                selected_rows = cap;
            }
        }
    }
    wallboard_card_ports(
        &mut drawing,
        &pairs,
        &nodes.iter().map(|node| node["height"].as_f64().unwrap_or(height)).collect::<Vec<_>>(),
        height,
    );
    serde_json::json!({"width": drawing.width, "height": drawing.height, "rows": selected_rows,
        "boxes": tasks.iter().zip(&drawing.boxes).map(|(task, (x,y))|
            serde_json::json!({"id": task.id, "x": x, "y": y})).collect::<Vec<_>>(),
        "routes": drawing.routes})
    .to_string()
}

fn wallboard_card_ports(drawing: &mut layout::Drawing, pairs: &[(usize, usize)], heights: &[f64], slot_height: f64) {
    for (route, &(a, b)) in drawing.routes.iter_mut().zip(pairs) {
        if let Some(first) = route.first_mut() {
            first.1 += (heights[a] - slot_height) / 2.0;
        }
        if let Some(last) = route.last_mut() {
            last.1 += (heights[b] - slot_height) / 2.0;
        }
    }
}

fn wallboard_relation(ids: &HashSet<EntityId>, from: Option<EntityId>, to: Option<EntityId>) -> bool {
    from.is_some_and(|id| ids.contains(&id)) && to.is_some_and(|id| ids.contains(&id))
}

/// Whether the wallboard draws a task of `status`, given the date it comes due, if it is not due yet
/// (`clock::comes_due`).
fn wallboard_draws(
    selection: &crate::wallboard::TaskSelection,
    status: Option<TaskStatus>,
    comes_due: Option<i64>,
) -> bool {
    selection.keeps(status) && comes_due.is_none()
}

/// Tasks not yet due leave the drawing here, not the task query: the clock finds the dates to come
/// among the store's tasks, and a date passing changes nothing a live query could hear.
fn wallboard_tasks(selection: &crate::wallboard::TaskSelection) -> (Vec<TaskView>, Vec<RelationView>) {
    let mut tasks: Vec<_> = store::tasks()
        .into_iter()
        .filter(|t| wallboard_draws(selection, t.status().ok(), clock::comes_due(t)))
        .collect();
    tasks.sort_by_key(|t| t.id());
    let ids: HashSet<_> = tasks.iter().map(|t| t.id()).collect();
    let relations =
        store::relations().into_iter().filter(|r| wallboard_relation(&ids, r.from_id().ok(), r.to_id().ok())).collect();
    (tasks, relations)
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

fn wallboard_footer(
    names: &[String],
    metadata: &str,
    priority: Option<i64>,
    width: f64,
    availability: &crate::wallboard::Availability,
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
fn wallboard_card_size(presentation: &crate::wallboard::Presentation, footer_rows: usize, parent: bool) -> (f64, f64) {
    let scale = presentation.text_scale_percent as f64 / 100.0;
    let height = 149.0 + footer_rows.saturating_sub(1) as f64 * 24.0 + if parent { 20.0 } else { 0.0 };
    (presentation.node_width as f64 * scale, height * scale)
}

fn wallboard_render(
    tasks: &[TaskView],
    relations: &[RelationView],
    scopes: &[ScopeView],
    messages: &[MessageView],
    presentation: &crate::wallboard::Presentation,
    availability: &[crate::wallboard::Availability],
) -> String {
    let text_scale = presentation.text_scale_percent as f64 / 100.0;
    let content_width = presentation.node_width as f64;
    let (width, base_height) = wallboard_card_size(presentation, 0, false);
    let titles: HashMap<_, _> = tasks.iter().map(|t| (t.id(), t.title().unwrap_or_default())).collect();
    let all_edges: Vec<_> = relations.iter().filter_map(Edge::of).collect();
    let parents: Vec<_> = tasks
        .iter()
        .map(|task| {
            let named: Vec<_> = all_edges
                .iter()
                .copied()
                .filter(|e| e.kind == RelationKind::SubtaskOf && e.first == task.id())
                .collect();
            also_part_of(&named, false, &|id| titles.get(id).cloned().unwrap_or_default())
        })
        .collect();
    let footers: Vec<_> = tasks
        .iter()
        .zip(availability)
        .map(|(task, availability)| {
            let names: Vec<_> = store::scopes_of_task(task.id())
                .iter()
                .map(|id| {
                    scopes
                        .iter()
                        .find(|s| s.id() == *id)
                        .and_then(|s| s.title().ok())
                        .unwrap_or_else(|| fmt::short(*id))
                })
                .collect();
            let mut metadata = fmt::status_label(task.status().unwrap_or(TaskStatus::Proposed)).to_string();
            if let Some(assignee) = fwd_core::ops::assignee_of(task).ok().flatten() {
                metadata.push_str(&format!(" · {}", store::handle_of(assignee)));
            }
            wallboard_footer(&names, &metadata, task.priority().ok().flatten(), content_width, availability)
        })
        .collect();
    let card_heights: Vec<_> = footers
        .iter()
        .zip(&parents)
        .map(|((_, rows), parent)| wallboard_card_size(presentation, *rows, !parent.is_empty()).1)
        .collect();
    let height = card_heights.iter().copied().fold(base_height, f64::max);
    let weighed: Vec<_> = tasks
        .iter()
        .map(|t| layout::Task {
            id: t.id().to_base64(),
            title: titles[&t.id()].clone(),
            priority: t.priority().ok().flatten(),
            status: fmt::status_rank(t.status().unwrap_or(TaskStatus::Proposed)),
            band: None,
        })
        .collect();
    let dependencies: Vec<_> = all_edges.iter().copied().filter(|e| e.kind == RelationKind::DependsOn).collect();
    let implied = implied(&dependencies);
    let edges: Vec<_> =
        dependencies.into_iter().filter(|e| e.proposed || !implied.contains(&(e.first, e.then))).collect();
    let places: HashMap<_, _> = tasks.iter().enumerate().map(|(i, t)| (t.id(), i)).collect();
    let pairs: Vec<_> = edges.iter().map(|e| (places[&e.first], places[&e.then])).collect();
    let mut drawing = layout::Layout::wallboard(&weighed, &pairs, presentation.max_rows, width, height);
    wallboard_card_ports(&mut drawing, &pairs, &card_heights, height);
    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" class="dag wallboard-dag" width="{}" height="{}" viewBox="0 0 {} {}" data-max-rows="{}" data-node-height="{height}" data-node-width="{width}"><defs><marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto"><path d="M0,0 L10,5 L0,10 z" class="arrowhead"/></marker></defs>"#,
        drawing.width, drawing.height, drawing.width, drawing.height, presentation.max_rows
    );
    for (edge, route) in edges.iter().zip(&drawing.routes) {
        if let Some(path) = path(route) {
            svg.push_str(&format!(r#"<path class="edge edge-depends{}" d="{path}" marker-end="url(#arrow)" data-from="{}" data-to="{}" data-route="{}"/>"#, if edge.proposed { " edge-proposed" } else { "" }, edge.first.to_base64(), edge.then.to_base64(), fmt::escape(&serde_json::to_string(route).unwrap_or_default())));
        }
    }
    for (i, task) in tasks.iter().enumerate() {
        let (x, y) = drawing.boxes[i];
        let status = task.status().unwrap_or(TaskStatus::Proposed);
        let parent = &parents[i];
        let height = card_heights[i];
        let what = task.what().unwrap_or_default().split_whitespace().collect::<Vec<_>>().join(" ");
        use std::hash::{Hash, Hasher};
        let mut revision = std::hash::DefaultHasher::new();
        // Hash full WHAT and the actual task update stamp, rather than only its visible excerpt, and
        // the task's relations, rather than the arrows and parent line drawn from them.
        // Message-count changes repaint quietly and cannot turn into task-edit storms.
        (
            &titles[&task.id()],
            &what,
            task.updated().ok(),
            status.to_string(),
            task.priority().ok(),
            &footers[i].0,
            wallboard_relation_facts(task.id(), store::relations_of(task.id()).iter().filter_map(Edge::of)),
        )
            .hash(&mut revision);
        let priority = task.priority().ok().flatten();
        let mut tint = priority.map(|p| format!(" priority-p{}", p.clamp(0, 3))).unwrap_or_default();
        if availability[i].quiet() {
            tint.push_str(" availability-waiting");
        }
        let what = fmt::truncate(&what, 100);
        let title_lines = wallboard_wrap(&titles[&task.id()], ((content_width - 24.0) / 11.8) as usize, 2);
        let what_lines = wallboard_wrap(&what, ((content_width - 24.0) / 8.0) as usize, 2);
        svg.push_str(&format!(r#"<g class="node wallboard-node status-{status}{tint}{}" data-id="{}" data-layout-title="{}" data-priority="{}" data-revision="{:x}" transform="translate({x},{y})"><title>{}</title><rect class="wallboard-card" width="{width}" height="{height}" rx="9"/>"#, if status == TaskStatus::Proposed { " ghost" } else { "" }, task.id().to_base64(), fmt::escape(&titles[&task.id()]), priority.map(|p| p.to_string()).unwrap_or_default(), revision.finish(), fmt::escape(&format!("{}\n{what}\n{parent}", titles[&task.id()]))));
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
        svg.push_str(&footers[i].0);
        if !parent.is_empty() {
            svg.push_str(&format!(
                r#"<text class="wallboard-parent" x="12" y="{}">{}</text>"#,
                height / text_scale - 12.0,
                fmt::escape(&fmt::truncate(parent, ((content_width - 24.0) / 7.0) as usize))
            ));
        }
        let count = store::message_count(task.id(), messages);
        if count > 0 {
            svg.push_str(&format!(r#"<g class="count" transform="translate({},0)"><circle r="10"/><text text-anchor="middle" y="4">{count}</text></g>"#, content_width - 12.0));
        }
        svg.push_str("</g></g>");
    }
    svg.push_str("</svg>");
    svg
}

#[cfg(test)]
#[path = "wallboard_render_tests.rs"]
mod tests;
