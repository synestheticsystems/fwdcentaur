//! The wallboard's renderer and its Rust/JavaScript adapter. A frame is drawn in three stages, each a
//! memo that runs again only when what it reads changes: the cards, read from the store
//! (`cards::read`); their packing, which reads only what places them; and the frame's SVG
//! (`markup::frame`), the only stage that reads the counts of messages.
// main.rs lets what only the app uses go unused in the wallboard's build; the wallboard's own code may not.
#![warn(dead_code)]
use crate::wallboard::{Dependencies, Presentation, Settings, TaskSelection};
use crate::{fmt, layout, session, store};
use ankurah::signals::Get as AnkurahGet;
use ankurah::EntityId;
use cards::Cards;
use fwd_core::model::TaskStatus;
use leptos::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;

mod cards;
mod markup;

/// The wallboard draws only the selected active statuses, and of those only the tasks that are due.
/// Hierarchy is named on the cards; dependency arrows retain their meaning while the compact layout
/// enforces a row cap. Only the presentation reacts: the selection is the one the store's task query
/// was opened with.
#[component]
pub fn WallboardGraph(presentation: RwSignal<Presentation>, selection: TaskSelection) -> impl IntoView {
    // Keeps what the drawing says about dates true while the page runs. Started here rather than at
    // the top of the display, because its checks read the store: a cold start while no daemon answers
    // can wait for the store for longer than a check takes to come round.
    crate::clock::keep_time();
    let host = NodeRef::<leptos::html::Div>::new();
    let runtime_settings = {
        let selection = selection.clone();
        Memo::new(move |_| {
            let settings = Settings { selection: selection.clone(), presentation: presentation.get() };
            let mut value = settings.json();
            value["project_id"] = serde_json::json!(session::project().id);
            value.to_string()
        })
    };
    // Each stage's time, outside the memos' values: equal frames must stay equal however long they
    // took, or diagnostics themselves would cause DOM work. Timed on the monotonic clock, which a
    // correction of the wall clock never moves.
    let stopwatch: Stopwatch = Arc::new(move |stage: Stage| {
        let begin = performance_now();
        Box::new(move || {
            if let Some(element) = host.get_untracked() {
                wallboard_record_rust(element.as_ref(), stage.timing(), performance_now() - begin);
            }
        })
    });
    let prerequisites = StoredValue::new(send_wrapper::SendWrapper::new(Dependencies::default()));
    let cards = Memo::new({
        let stopwatch = stopwatch.clone();
        move |_| {
            let presentation = presentation.get();
            let stop = stopwatch(Stage::Cards);
            let mut read = Cards::default();
            prerequisites.update_value(|queries| read = cards::read(&selection, &presentation, queries));
            stop();
            read
        }
    });
    let counts = Memo::new(move |_| markup::message_counts(&store::messages()));
    let frame = frame(cards, counts, stopwatch);
    Effect::new(move |mounted: Option<bool>| {
        let svg = frame.get();
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

/// A stage of the drawing, as diagnostics time it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Cards,
    Packing,
    Markup,
}

impl Stage {
    /// The timing diagnostics record it under.
    fn timing(self) -> &'static str {
        match self {
            Stage::Cards => "rustCards",
            Stage::Packing => "rustPacking",
            Stage::Markup => "rustMarkup",
        }
    }
}

/// Starts timing a stage's work, and gives what stops it.
type Stopwatch = Arc<dyn Fn(Stage) -> Box<dyn FnOnce()> + Send + Sync>;

/// The frame's SVG, from `cards` and the counts of messages on them, packing the cards again only
/// when what places them changes: their order, the arrows between them, their sizes and the row cap.
/// A new message changes a count alone, so it redraws the markup without packing the cards again.
fn frame(cards: Memo<Cards>, counts: Memo<HashMap<EntityId, usize>>, stopwatch: Stopwatch) -> Memo<String> {
    let placing = Memo::new(move |_| cards.with(Placing::of));
    let packing = Memo::new({
        let stopwatch = stopwatch.clone();
        move |_| {
            placing.with(|placing| {
                let stop = stopwatch(Stage::Packing);
                let drawing = placing.pack();
                stop();
                drawing
            })
        }
    });
    Memo::new(move |_| {
        cards.with(|cards| {
            packing.with(|drawing| {
                counts.with(|counts| {
                    let stop = stopwatch(Stage::Markup);
                    let svg = markup::frame(cards, drawing, counts);
                    stop();
                    svg
                })
            })
        })
    })
}

/// What the packing reads of the cards, and all it reads, so that the packing runs again only when
/// this changes: their order, as the layout weighs their titles and priorities, the arrows between
/// them, their sizes and the row cap.
#[derive(Clone, Debug, PartialEq)]
struct Placing {
    tasks: Vec<layout::Task>,
    pairs: Vec<(usize, usize)>,
    heights: Vec<f64>,
    width: f64,
    slot_height: f64,
    max_rows: usize,
}

impl Placing {
    fn of(cards: &Cards) -> Placing {
        Placing {
            tasks: cards
                .cards
                .iter()
                .map(|card| layout::Task {
                    id: card.id.to_base64(),
                    title: card.title.clone(),
                    priority: card.priority,
                    status: fmt::status_rank(card.status),
                    band: None,
                })
                .collect(),
            pairs: cards.arrows.iter().map(|arrow| (arrow.first, arrow.then)).collect(),
            heights: cards.cards.iter().map(|card| card.height).collect(),
            width: cards.width,
            slot_height: cards.slot_height,
            max_rows: cards.max_rows,
        }
    }

    /// The packing: each card in its column, every arrow meeting its cards at their middles.
    fn pack(&self) -> layout::Drawing {
        let mut drawing =
            layout::Layout::wallboard(&self.tasks, &self.pairs, self.max_rows, self.width, self.slot_height);
        wallboard_card_ports(&mut drawing, &self.pairs, &self.heights, self.slot_height);
        drawing
    }
}

#[wasm_bindgen::prelude::wasm_bindgen(module = "/src/wallboard/wallboard-runtime.mjs")]
extern "C" {
    #[wasm_bindgen(js_name = mount)]
    fn wallboard_mount(host: &web_sys::Element, settings: &str, layout: &wasm_bindgen::JsValue);
    #[wasm_bindgen(js_name = update)]
    fn wallboard_update(host: &web_sys::Element, svg: &str, loaded: bool, connected: bool, settings: &str);
    #[wasm_bindgen(js_name = recordRustTiming)]
    fn wallboard_record_rust(host: &web_sys::Element, stage: &str, elapsed_ms: f64);
    #[wasm_bindgen(js_name = dispose)]
    fn wallboard_dispose(host: &web_sys::Element);
}

#[wasm_bindgen::prelude::wasm_bindgen]
extern "C" {
    /// The page's monotonic clock, in milliseconds.
    #[wasm_bindgen(js_namespace = performance, js_name = now)]
    fn performance_now() -> f64;
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

#[cfg(test)]
mod tests;
