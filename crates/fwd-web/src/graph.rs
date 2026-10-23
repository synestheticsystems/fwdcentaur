//! The dependency graph as a layered drawing: a task sits to the right of everything it waits
//! for (its `depends_on` targets), so work flows left to right, and each parent task is a band
//! across the drawing that holds its subtasks, so that the plan's hierarchy reads as its shape
//! (`banded`; D134, "The Graph page draws parent tasks as bands holding their subtasks, and only
//! the dependencies no chain implies"; task 5Ulut6YH, "Draw each parent task on the Graph page as
//! a band holding its subtasks").
//!
//! It draws one scope's tasks at a time, so that no task reads as part of a scope's work merely
//! for being drawn beside it (Daniel, 2026-09-28, after a later-phase proposal in no scope, drawn
//! beside the current scope's tasks, passed for part of that work: "I suppose we need to add a
//! Scope filter on the Graph, which should be set to a scope by default - thus limiting the tasks
//! that appear"). The scope is the header's choice, which the Questions, Tasks and Decisions pages
//! list by too and the route carries (`scope_filter.rs`). A task outside the scope that is joined
//! to one inside it by a dependency, either way, or as its subtask, is drawn as a stub (`plan`):
//! what a task waits for shows without the other scope's graph drawn in. A parent outside the scope
//! of a task inside it heads that task's band instead.
//!
//! Where each task and each edge goes is `layout.rs`'s to say, stubs included: a long edge passes
//! between the tasks of the columns it crosses, and the order control in the page's heading says
//! how each column is ordered, "auto" (the default) crossing edges as little as it can, "priority"
//! and "status" sorting each column (Daniel, 2026-09-28: "maybe a sort by priority/status vs auto
//! (default. sort to minimize crossings)?"). This browser remembers the order chosen
//! (localStorage), as it remembers how wide each page's panel was dragged; the route does not
//! carry it.
//!
//! So that every line drawn says something about the order the work can go in, a dependency that
//! a chain of other accepted dependencies the drawing holds already says is left out (`plan`), and
//! each line meets a task at a point of its own (`layout.rs`): a scope of 71 tasks drew 234 lines,
//! 58 of them dependencies a chain already said, crossing 1,763 times, and 32 lines left its
//! busiest task at one point (task bL58YOSa, "Draw only the dependencies no chain implies, give
//! each line its own end, and light a selected task's chain"). Selecting a task lights its chain of
//! dependencies both ways and dims the rest, so that the task can be followed through the drawing,
//! and draws its implied dependencies, dashed (`focus`); they take no place in the layout, so
//! selecting a task moves nothing. A parent is selected by its band's label, and lights what its
//! band holds too.
//!
//! The panel of the task a click selects has the Tasks page's edge and Full screen button, the
//! drawing standing where that page has its list (`panel.rs`; task CyqBDRUz, "Give the Graph
//! page's task panel and the decision panel the same Full screen control"). In the VS Code webview
//! a click opens the task on the Tasks page instead, its list filtered to the Graph's scope, with
//! Back to the Graph (`back.rs`), which brings the drawing back zoomed, scrolled and showing closed
//! tasks or not as the person left it: the page keeps those beside the route while it lives
//! (`page_state.rs`; task toDjwytu, "Carry the Graph's scope filter and a page's state through
//! Back in the webview").
//!
//! The SVG is built as markup from the live snapshots and re-rendered whole on any change; clicks
//! are delegated from the container to the node's `data-id`.

use crate::empty::{Empty, Nothing};
use crate::layout::{self, Order, H, W};
use crate::panel::{FullScreenToggle, PanelEdge, PanelSize};
use crate::route::{self, Route};
use crate::scope_filter::ScopeChoice;
use crate::tasks::TaskDetail;
use crate::{fmt, page_state, session, store};
use ankurah::EntityId;
use fwd_core::model::*;
use leptos::prelude::*;
use std::collections::{BTreeSet, HashMap, HashSet};
use wasm_bindgen::JsCast;

/// The localStorage key under which this browser remembers the order the page draws its columns in.
const ORDER: &str = "fwd.web.graph-order";

/// The localStorage key under which this browser remembers how wide the page's task panel was
/// dragged, apart from the Tasks page's.
const PANEL_WIDTH: &str = "fwd.web.graph-panel-width";

/// The width of the column at the drawing's left that holds the bands' labels, and how much
/// further right each band nested in another begins, its label with it.
const LABEL_W: f64 = 200.0;
const INDENT: f64 = 14.0;

/// How many letters of the parents named above a task's box show, so that at 10 px, about 5 px a
/// letter, they end before the count of messages at the box's top right corner, 22 px from its
/// right edge.
const ALSO_LETTERS: usize = 28;

// The parents named above a task's box lie inside its band and end before its count of messages:
// the note's baseline is 4 px above the box and its 10 px letters reach about 8 px above that; the
// count is a circle of 10 px about a point 6 px above the box, 12 px in from its right edge; a
// band's edge lies `BAND_PAD` above its first task; and the note starts 12 px in, about 5.5 px a
// letter at most.
const _: () = assert!(layout::BAND_PAD >= 4.0 + 8.0 && layout::BAND_PAD >= 6.0 + 10.0);
const _: () = assert!(12.0 + ALSO_LETTERS as f64 * 5.5 < W - 12.0 - 10.0);

/// The Graph page: its filters and the drawing, and beside it (beneath it on a narrow page) the
/// panel of the task the route selects. At every width a button lets the panel fill the page, the
/// drawing hidden, and beside the drawing dragging the panel's left edge sets its width
/// (`panel.rs`); the route stays what it was. `scope` is the scope whose tasks the page draws, the
/// page's choice (`scope_filter::follow`): none until the page has picked the one it opens on.
#[component]
pub fn GraphPage(selected: Memo<Option<EntityId>>, scope: Memo<Option<ScopeChoice>>) -> impl IntoView {
    // The zoom, the show-closed choice and the canvas's scroll as the person left them, when Back
    // brought the page here; else as a visit starts (`page_state.rs`). The page keeps them there
    // as they change.
    let kept = page_state::open_graph();
    let show_closed = RwSignal::new(kept.show_closed);
    let order = RwSignal::new(remembered_order());
    let zoom = RwSignal::new(kept.zoom);
    Effect::new(move |_| {
        let (zoom, show_closed) = (zoom.get(), show_closed.get());
        page_state::keep_graph(|state| {
            state.zoom = zoom;
            state.show_closed = show_closed;
        });
    });
    let canvas = NodeRef::<leptos::html::Div>::new();
    let keep_scroll = move |_| {
        if let Some(canvas) = canvas.get_untracked() {
            let scroll = (canvas.scroll_left(), canvas.scroll_top());
            page_state::keep_graph(|state| state.scroll = scroll);
        }
    };
    let size = PanelSize::new(PANEL_WIDTH);
    let open = move || selected.with(Option::is_some);
    // With no task selected the drawing comes back, so the next task selected opens beside it.
    Effect::new(move |_| {
        if !open() {
            size.full.set(false);
        }
    });
    // A click on a node selects its task into the panel beside the drawing; in the VS Code webview
    // it opens the task on the Tasks page instead, filling the page, with Back to the Graph
    // (`back.rs`).
    let on_click = move |ev: leptos::ev::MouseEvent| {
        let Some(target) = ev.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) else { return };
        let Some(node) = target.closest("[data-id]").ok().flatten() else { return };
        if let Some(id) = node.get_attribute("data-id").and_then(|s| s.parse::<EntityId>().ok()) {
            route::go(if session::locked() {
                on_the_tasks_page(id, scope.get_untracked())
            } else {
                Route::Graph { task: Some(id), scope: scope.get_untracked() }
            });
        }
    };
    // What the drawing holds and what its layout is made from; none while the page picks the scope
    // it opens on, and none when the filter keeps no task. A change that leaves it as it was, a
    // message's arrival say, leaves the layout be.
    let planned = Memo::new(move |_| {
        let choice = scope.get()?;
        let tasks = store::tasks();
        let relations = store::relations();
        planned(&tasks, &relations, choice, show_closed.get())
    });
    // The layout, made again only when what it is made from or the order changes: it costs the
    // most, and a selection moves no box, so selecting a task only draws the layout again with the
    // task's implied dependencies threaded through it.
    let laid = Memo::new(move |_| planned.with(|planned| planned.as_ref().map(|planned| planned.layout(order.get()))));
    // The drawing's markup.
    let drawing = Memo::new(move |_| {
        let choice = scope.get()?;
        let tasks = store::tasks();
        let relations = store::relations();
        let scopes = store::scopes();
        let messages = store::messages();
        let selected = selected.get();
        planned.with(|planned| {
            laid.with(|laid| {
                let (planned, laid) = (planned.as_ref()?, laid.as_ref()?);
                Some(render(planned, laid, &tasks, &relations, &scopes, &messages, choice, selected))
            })
        })
    });
    // Back brings the canvas back scrolled where it was, once the drawing is there to scroll.
    if kept.scroll != (0, 0) {
        let (left, top) = kept.scroll;
        Effect::new(move |restored: Option<bool>| {
            if restored == Some(true) {
                return true;
            }
            let drawn = drawing.with(Option::is_some);
            if drawn {
                request_animation_frame(move || {
                    if let Some(canvas) = canvas.get_untracked() {
                        canvas.set_scroll_left(left);
                        canvas.set_scroll_top(top);
                    }
                });
            }
            drawn
        });
    }
    let choose_order = move |ev: leptos::ev::Event| {
        if let Some(chosen) = Order::named(&event_target_value(&ev)) {
            order.set(chosen);
            remember_order(chosen);
        }
    };
    let orders = move || {
        Order::ALL
            .into_iter()
            .map(|o| view! { <option value=o.name() selected=move || order.get() == o>{o.name()}</option> })
            .collect::<Vec<_>>()
    };
    // What the canvas says instead of a drawing: until the page has picked its scope and the
    // daemon has answered, that the drawing is on its way rather than that the scope is empty.
    let nothing = Signal::derive(move || match scope.get() {
        Some(choice) => Nothing::ToDraw { choice, show_closed: show_closed.get() },
        None => Nothing::Yet,
    });
    // What a stub is, in the legend, while the filter leaves anything out.
    let stubs = move || {
        let label = match scope.get()? {
            ScopeChoice::Scope(_) => " outside this scope",
            ScopeChoice::NoScope => " in a scope",
            ScopeChoice::All => return None,
        };
        Some(view! { <span class="legend-item"><span class="swatch stub"></span>{label}</span> })
    };
    view! {
        <div class="split resizable" class:with-detail=open class:full-screen=move || open() && size.full.get()
            class:dragging=move || size.dragging.get() style=("--panel-width", move || size.width_property())>
            <section class="page graph-pane">
                <header class="page-head">
                    <h2>"Graph"</h2>
                    <label class="field graph-order"
                        title="how each column is ordered: auto, so that edges cross as little as they can; or by priority, or by status">
                        "order "
                        <select on:change=choose_order>{orders}</select>
                    </label>
                    <label class="toggle">
                        <input type="checkbox" prop:checked=move || show_closed.get()
                            on:change=move |ev| show_closed.set(event_target_checked(&ev))/>
                        " show closed"
                    </label>
                    <span class="zoom">
                        <button class="btn subtle" on:click=move |_| zoom.update(|z| *z = (*z - 0.1).max(0.3))>"−"</button>
                        <button class="btn subtle" on:click=move |_| zoom.set(1.0)>{move || format!("{:.0}%", zoom.get() * 100.0)}</button>
                        <button class="btn subtle" on:click=move |_| zoom.update(|z| *z = (*z + 0.1).min(2.5))>"+"</button>
                    </span>
                    <span class="legend">
                        <span class="legend-item"><span class="swatch edge-depends"></span>" unblocks"</span>
                        <span class="legend-item"
                            title="a dependency that a chain of others already says, drawn only while one of its tasks is selected">
                            <span class="swatch edge-implied"></span>" implied, when selected"
                        </span>
                        <span class="legend-item" title="a parent task, labelled at its left, holding its subtasks">
                            <span class="swatch band"></span>" parent task"
                        </span>
                        <span class="legend-item"><span class="swatch ghost"></span>" proposed"</span>
                        {stubs}
                    </span>
                </header>
                <div class="canvas" node_ref=canvas on:click=on_click on:scroll=keep_scroll>
                    {move || drawing.with(Option::is_none).then(|| view! { <Empty nothing=nothing/> })}
                    <div class="canvas-inner" style:transform=move || format!("scale({})", zoom.get())
                        inner_html=move || drawing.get().unwrap_or_default()></div>
                </div>
            </section>
            {move || {
                selected.get().map(|id| {
                    view! {
                        <PanelEdge size=size/>
                        <aside class="detail-pane">
                            <FullScreenToggle size=size hides="the graph"/>
                            <TaskDetail id=id/>
                        </aside>
                    }
                })
            }}
        </div>
    }
}

/// Where a click on `task`'s node leads in the VS Code webview, the Graph showing `choice`: the
/// task's panel on the Tasks page, its list keeping the scope the Graph shows, so that the list
/// behind the task is the work the drawing showed.
fn on_the_tasks_page(task: EntityId, choice: Option<ScopeChoice>) -> Route {
    Route::tasks(Some(task), choice)
}

/// What the drawing needs of a task to decide whether it is drawn.
struct TaskFacts {
    id: EntityId,
    closed: bool,
    /// The scopes it is worked in, any number (docs/DECISIONS.md, D136, "A task may be worked in
    /// several scopes"); none when it is worked in none. A scope chosen draws the task when it is
    /// one of them.
    scopes: Vec<EntityId>,
}

/// A relation the drawing shows: a dependency, drawn as a line from `first` to `then`, which waits
/// for it and sits to the right of it; or a subtask relation, `first` the subtask and `then` the
/// task it is part of, drawn as the band that task heads (`banded`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Edge {
    first: EntityId,
    then: EntityId,
    kind: RelationKind,
    proposed: bool,
}

impl Edge {
    /// The edge of a dependency or a subtask relation that stands (accepted, or proposed).
    fn of(r: &RelationView) -> Option<Edge> {
        let (Ok(from), Ok(to), Ok(kind), Ok(status)) = (r.from_id(), r.to_id(), r.kind(), r.status()) else {
            return None;
        };
        let proposed = match status {
            RelationStatus::Accepted => false,
            RelationStatus::Proposed => true,
            _ => return None,
        };
        match kind {
            RelationKind::DependsOn => Some(Edge { first: to, then: from, kind, proposed }),
            RelationKind::SubtaskOf => Some(Edge { first: from, then: to, kind, proposed }),
            _ => None,
        }
    }
}

/// What the drawing holds.
#[derive(Clone, Debug, PartialEq)]
struct Plan {
    /// The tasks the filter keeps.
    kept: Vec<EntityId>,
    /// The tasks outside the filter that a dependency joins to one it keeps, or that are subtasks
    /// of one it keeps, each once.
    stubs: Vec<EntityId>,
    /// The dependencies drawn as lines: between two kept tasks, and between a kept task and a
    /// stub, less the implied ones.
    edges: Vec<Edge>,
    /// The accepted dependencies among those that a chain of other accepted ones implies
    /// (`implied`), drawn only while one of their tasks is selected.
    implied: Vec<Edge>,
    /// The bands, each holding a parent task's subtasks (`banded`), in the order of their heads'
    /// ids.
    bands: Vec<Band>,
    /// The band each task the drawing holds is drawn in: a task that heads a band, in its own.
    band_of: HashMap<EntityId, usize>,
    /// Per task the drawing holds, its subtask relations that the band it is drawn in does not
    /// show, each named beside it: to a parent other than the one whose band holds it, or
    /// proposed.
    also: HashMap<EntityId, Vec<Edge>>,
}

impl Plan {
    /// The tasks drawn as boxes: each kept task, then each stub, but for a task that heads a band
    /// and has no dependency drawn, which its band's label stands for alone.
    fn boxed(&self) -> Vec<EntityId> {
        let lined: HashSet<EntityId> = self.edges.iter().flat_map(|e| [e.first, e.then]).collect();
        let heads: HashSet<EntityId> = self.bands.iter().map(|band| band.head).collect();
        self.kept.iter().chain(&self.stubs).copied().filter(|id| !heads.contains(id) || lined.contains(id)).collect()
    }

    /// How many bands band `b` is nested in.
    fn depth(&self, b: usize) -> usize {
        let (mut depth, mut at) = (0, self.bands[b].within);
        while let Some(outer) = at.filter(|_| depth <= self.bands.len()) {
            depth += 1;
            at = self.bands[outer].within;
        }
        depth
    }
}

/// A band of the drawing: the strip that holds a parent task's subtasks, labelled with the parent,
/// in place of a box placed after them with a line from each (D134, "The Graph page draws parent
/// tasks as bands holding their subtasks, and only the dependencies no chain implies").
#[derive(Clone, Debug, PartialEq)]
struct Band {
    /// The parent task.
    head: EntityId,
    /// The band it is drawn in, the one its head's own parent heads, as a band within that one.
    within: Option<usize>,
}

/// Which tasks and edges the drawing holds for `choice`: the tasks the filter keeps, and as a stub
/// each task outside it that a dependency joins to one of them, so a task that waits on work
/// elsewhere (or that work elsewhere waits on) shows it, without that work's own graph drawn in: a
/// stub brings no edge of its own. A subtask outside the filter of a task it keeps is a stub too,
/// drawn in its parent's band. Closed tasks, stubs among them, are drawn only with `show_closed`.
/// Of the dependencies between them, one that a chain of the others already says is left out
/// (`implied`), so that each line drawn says something about the order the work can go in; a
/// closed task left out of the drawing hands its chain back to the direct dependency. Subtask
/// relations draw no lines: they make the bands (`banded`).
fn plan(tasks: &[TaskFacts], edges: &[Edge], choice: ScopeChoice, show_closed: bool) -> Plan {
    let shown: Vec<&TaskFacts> = tasks.iter().filter(|t| show_closed || !t.closed).collect();
    let kept: Vec<EntityId> = shown.iter().filter(|t| choice.keeps(&t.scopes)).map(|t| t.id).collect();
    let inside: HashSet<EntityId> = kept.iter().copied().collect();
    let outside: HashSet<EntityId> = shown.iter().filter(|t| !choice.keeps(&t.scopes)).map(|t| t.id).collect();
    let mut stubs: Vec<EntityId> = Vec::new();
    let mut drawn: Vec<Edge> = Vec::new();
    for edge in edges {
        let stub = match (inside.contains(&edge.first), inside.contains(&edge.then)) {
            (true, true) => None,
            (true, false) if outside.contains(&edge.then) => Some(edge.then),
            (false, true) if outside.contains(&edge.first) => Some(edge.first),
            _ => continue,
        };
        let stub = match edge.kind {
            RelationKind::DependsOn => {
                drawn.push(*edge);
                stub
            }
            // An accepted subtask outside the filter of a kept task is a stub, drawn in its
            // parent's band; the task a kept task is part of heads that task's band instead
            // (`banded`).
            _ => stub.filter(|&stub| stub == edge.first && !edge.proposed),
        };
        if let Some(stub) = stub.filter(|stub| !stubs.contains(stub)) {
            stubs.push(stub);
        }
    }
    let implied_pairs = implied(&drawn);
    let (implied, drawn) = drawn.into_iter().partition(|e| !e.proposed && implied_pairs.contains(&(e.first, e.then)));
    let held: Vec<EntityId> = kept.iter().chain(&stubs).copied().collect();
    let (bands, band_of, also) = banded(edges, &kept, &held);
    Plan { kept, stubs, edges: drawn, implied, bands, band_of, also }
}

/// The bands the drawing holds: every parent of a task the filter keeps heads one, holding the
/// subtasks of it the drawing holds, as does every kept task with a stub among its subtasks or a
/// band among them, so that a deeper hierarchy nests bands, each in the band its head's parent
/// heads. A parent outside the filter still heads the band of its subtasks inside it; a stub makes
/// no band of its own parent outside. A task with more than one parent is drawn in the band of
/// one, chosen by a fixed order, a parent the filter keeps first, then by id, and its other parents
/// are named beside it, as a proposed subtask relation is, which makes no band. A task that heads a
/// band is drawn in its own. Only accepted subtask relations make bands; a loop of them, which the
/// rules refuse but sync can bring, is cut where it closes (`parents`), so that every task on it
/// is still drawn. Returns the bands in the order of their heads' ids, the band each task in
/// `held` is drawn in, and per task the relations named beside it.
#[allow(clippy::type_complexity)]
fn banded(
    edges: &[Edge],
    kept: &[EntityId],
    held: &[EntityId],
) -> (Vec<Band>, HashMap<EntityId, usize>, HashMap<EntityId, Vec<Edge>>) {
    let inside: HashSet<EntityId> = kept.iter().copied().collect();
    let mut parts: HashMap<EntityId, Vec<Edge>> = HashMap::new();
    for e in edges.iter().filter(|e| e.kind == RelationKind::SubtaskOf && e.first != e.then) {
        parts.entry(e.first).or_default().push(*e);
    }
    let parent = parents(&parts, &inside);
    let parent_of = |t: &EntityId| parent.get(t).copied();
    let mut heads: BTreeSet<EntityId> =
        held.iter().filter_map(|t| parent_of(t).filter(|p| inside.contains(t) || inside.contains(p))).collect();
    loop {
        let more: Vec<EntityId> =
            heads.iter().filter_map(parent_of).filter(|p| inside.contains(p) && !heads.contains(p)).collect();
        if more.is_empty() {
            break;
        }
        heads.extend(more);
    }
    let index: HashMap<EntityId, usize> = heads.iter().enumerate().map(|(i, head)| (*head, i)).collect();
    let bands: Vec<Band> = heads
        .iter()
        .map(|head| Band { head: *head, within: parent_of(head).and_then(|p| index.get(&p).copied()) })
        .collect();
    let mut band_of = HashMap::new();
    let mut also = HashMap::new();
    for t in held {
        let parent = parent_of(t).filter(|p| index.contains_key(p));
        if let Some(band) = index.get(t).copied().or_else(|| parent.map(|p| index[&p])) {
            band_of.insert(*t, band);
        }
        let named: Vec<Edge> =
            parts.get(t).into_iter().flatten().filter(|e| e.proposed || Some(e.then) != parent).copied().collect();
        if !named.is_empty() {
            also.insert(*t, named);
        }
    }
    (bands, band_of, also)
}

/// Each subtask's parent by the relations in `parts` (per subtask, its subtask relations): of its
/// accepted parents, one in `inside` first, then the first by id. A loop of parents, which the rules
/// refuse but sync can bring, is cut where it closes: the task on it whose parent is the loop's
/// first task by id gets none. Uncut, each task on the loop would head a band nested in the band of
/// the next, and a parent with no dependency drawn is drawn only as its band's label, so the loop
/// would hold no box for a band to be drawn around, and none of its tasks would be drawn.
fn parents(parts: &HashMap<EntityId, Vec<Edge>>, inside: &HashSet<EntityId>) -> HashMap<EntityId, EntityId> {
    let mut parent: HashMap<EntityId, EntityId> = parts
        .iter()
        .filter_map(|(t, relations)| {
            let first =
                relations.iter().filter(|e| !e.proposed).map(|e| e.then).min_by_key(|p| (!inside.contains(p), *p));
            first.map(|p| (*t, p))
        })
        .collect();
    let mut subtasks: Vec<EntityId> = parent.keys().copied().collect();
    subtasks.sort_unstable();
    let mut walked: HashSet<EntityId> = HashSet::new();
    for start in subtasks {
        // From `start` up its parents, until a task walked before or a loop.
        let mut path: Vec<EntityId> = Vec::new();
        let mut at = Some(start);
        while let Some(t) = at.filter(|t| !walked.contains(t)) {
            if let Some(k) = path.iter().position(|&p| p == t) {
                let first = path[k..].iter().min().copied();
                if let Some(closing) = path[k..].iter().copied().find(|q| parent.get(q).copied() == first) {
                    parent.remove(&closing);
                }
                break;
            }
            path.push(t);
            at = parent.get(&t).copied();
        }
        walked.extend(path);
    }
    parent
}

/// The accepted dependencies among `edges` that a chain of two or more of the other accepted
/// ones implies, each as its pair `(first, then)`: a task that waits for another through such a
/// chain is drawn waiting for it by the chain alone. Only accepted dependencies make a chain, and
/// only they are left out: a proposed relation hides nothing, and nothing hides it, since it waits
/// on someone's word. Each pair is taken in turn and left out while the pairs still drawn lead from
/// its first task to its other, so every task the drawing showed reaching another still does,
/// even should relations arriving through sync close a loop of accepted dependencies, which the
/// rules never make (`rules::find_cycles`).
fn implied(edges: &[Edge]) -> HashSet<(EntityId, EntityId)> {
    let mut pairs: Vec<(EntityId, EntityId)> =
        edges.iter().filter(|e| e.kind == RelationKind::DependsOn && !e.proposed).map(|e| (e.first, e.then)).collect();
    pairs.sort_unstable();
    pairs.dedup();
    let mut next: HashMap<EntityId, Vec<EntityId>> = HashMap::new();
    for &(first, then) in &pairs {
        next.entry(first).or_default().push(then);
    }
    let mut implied = HashSet::new();
    for (first, then) in pairs {
        // Whether `then` is reached from `first` other than by this pair: from every other task
        // `first` leads to, never back through `first`.
        let mut seen = HashSet::from([first]);
        let mut todo: Vec<EntityId> = next[&first].iter().copied().filter(|&t| t != then).collect();
        while let Some(task) = todo.pop() {
            if task == then {
                implied.insert((first, then));
                next.entry(first).or_default().retain(|&t| t != then);
                break;
            }
            if seen.insert(task) {
                todo.extend(next.get(&task).into_iter().flatten().copied());
            }
        }
    }
    implied
}

/// What selecting a task lights, so that a reader can follow it through the drawing: the task,
/// everything it waits for all the way back and everything that waits for it all the way on, by
/// the dependencies drawn, proposed ones too, and the lines between them; for a task that heads a
/// band, also the tasks its band holds and the lines between them, so that its subtasks do not
/// fade with the rest; everything else is dimmed. Its implied dependencies are drawn too, dashed,
/// while it is selected.
#[derive(Debug, PartialEq)]
struct Focus {
    /// The selected task, the tasks of its chain and, for a task that heads a band, the tasks its
    /// band holds.
    tasks: HashSet<EntityId>,
    /// Per edge the plan draws, in its order: whether it is lit.
    lit: Vec<bool>,
    /// The plan's implied dependencies of the selected task.
    implied: Vec<Edge>,
}

/// What selecting `selected` lights in `plan`, or nothing when the drawing does not hold it, as a
/// box or as a band's label.
fn focus(plan: &Plan, selected: EntityId) -> Option<Focus> {
    let heads = plan.bands.iter().position(|band| band.head == selected);
    if !plan.kept.contains(&selected) && !plan.stubs.contains(&selected) && heads.is_none() {
        return None;
    }
    // The tasks the selected task's band holds, nested bands' too.
    let within = |mut band: usize, head: usize| {
        for _ in 0..=plan.bands.len() {
            if band == head {
                return true;
            }
            let Some(outer) = plan.bands[band].within else { return false };
            band = outer;
        }
        false
    };
    let held: HashSet<EntityId> = heads
        .map(|head| plan.band_of.iter().filter(|(_, &band)| within(band, head)).map(|(id, _)| *id).collect())
        .unwrap_or_default();
    // Per task, the tasks it waits for, and the tasks that wait for it.
    let mut before: HashMap<EntityId, Vec<EntityId>> = HashMap::new();
    let mut after: HashMap<EntityId, Vec<EntityId>> = HashMap::new();
    for e in &plan.edges {
        before.entry(e.then).or_default().push(e.first);
        after.entry(e.first).or_default().push(e.then);
    }
    let reach = |next: &HashMap<EntityId, Vec<EntityId>>| {
        let mut reached = HashSet::from([selected]);
        let mut todo = vec![selected];
        while let Some(task) = todo.pop() {
            for &other in next.get(&task).into_iter().flatten() {
                if reached.insert(other) {
                    todo.push(other);
                }
            }
        }
        reached
    };
    let (back, on) = (reach(&before), reach(&after));
    // A line of the chain leads into the selected task or a task it waits for, or out of the
    // selected task or a task that waits for it.
    let lit = plan
        .edges
        .iter()
        .map(|e| back.contains(&e.then) || on.contains(&e.first) || (held.contains(&e.first) && held.contains(&e.then)))
        .collect();
    let implied = plan.implied.iter().filter(|e| e.first == selected || e.then == selected).copied().collect();
    let tasks = back.iter().chain(&on).chain(&held).copied().collect();
    Some(Focus { tasks, lit, implied })
}

/// What the drawing holds for `choice`, and what its layout is made from, or none when the filter
/// keeps no task.
fn planned(tasks: &[TaskView], relations: &[RelationView], choice: ScopeChoice, show_closed: bool) -> Option<Planned> {
    let facts: Vec<TaskFacts> = tasks
        .iter()
        .map(|t| TaskFacts {
            id: t.id(),
            closed: t.status().map(fmt::is_closed).unwrap_or(false),
            // Through the index of relations by their ends, rather than every relation per task.
            scopes: store::scopes_of_task(t.id()),
        })
        .collect();
    let edges: Vec<Edge> = relations.iter().filter_map(Edge::of).collect();
    let plan = plan(&facts, &edges, choice, show_closed);
    if plan.kept.is_empty() {
        return None;
    }
    let by_id: HashMap<EntityId, &TaskView> = tasks.iter().map(|t| (t.id(), t)).collect();
    let weigh = |id: &EntityId| {
        let task = by_id.get(id);
        layout::Task {
            title: task.and_then(|t| t.title().ok()).unwrap_or_default(),
            id: id.to_base64(),
            priority: task.and_then(|t| t.priority().ok().flatten()),
            status: fmt::status_rank(task.and_then(|t| t.status().ok()).unwrap_or(TaskStatus::Proposed)),
            band: None,
        }
    };
    Some(Planned::new(plan, weigh))
}

/// The drawing's SVG markup for `planned`, laid out as `laid`, with `selected` selected.
#[allow(clippy::too_many_arguments)]
fn render(
    planned: &Planned,
    laid: &layout::Layout,
    tasks: &[TaskView],
    relations: &[RelationView],
    scopes: &[ScopeView],
    messages: &[MessageView],
    choice: ScopeChoice,
    selected: Option<EntityId>,
) -> String {
    let plan = &planned.plan;
    let edges: Vec<Edge> = relations.iter().filter_map(Edge::of).collect();
    let by_id: HashMap<EntityId, &TaskView> = tasks.iter().map(|t| (t.id(), t)).collect();
    let title_of = |id: &EntityId| by_id.get(id).and_then(|t| t.title().ok()).unwrap_or_default();
    let status_of = |id: &EntityId| by_id.get(id).and_then(|t| t.status().ok()).unwrap_or(TaskStatus::Proposed);
    let scope_title = |scope: EntityId| scopes.iter().find(|s| s.id() == scope).and_then(|s| s.title().ok());
    let scopes_of = |id: &EntityId| store::scopes_of_task(*id);
    let scope_name = |id: &EntityId| scope_words(&scopes_of(id), scope_title);
    let focus = selected.and_then(|selected| focus(plan, selected));
    let drawing = planned.draw(laid, focus.as_ref().map_or(&[], |focus| &focus.implied));
    let ids = &planned.ids;
    let titles: HashMap<EntityId, String> = ids.iter().map(|id| (*id, title_of(id))).collect();
    // What each band's label says of its parent.
    let progress = progress(&edges, &status_of);
    let heads: Vec<Head> = plan
        .bands
        .iter()
        .map(|band| {
            let id = band.head;
            let progress = progress.get(&id).copied().unwrap_or_default();
            Head::new(id, title_of(&id), status_of(&id), progress, &scopes_of(&id), choice, scope_title)
        })
        .collect();
    let (bands, left) = band_markup(plan, &heads, &drawing, selected, focus.as_ref());
    let (width, height) = (left + drawing.width, drawing.height);
    let at: HashMap<EntityId, (f64, f64)> = ids.iter().copied().zip(drawing.boxes.iter().copied()).collect();
    let dimmed = |id: &EntityId| focus.as_ref().is_some_and(|focus| !focus.tasks.contains(id));

    let mut s = String::new();
    s.push_str(&format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" class="dag" width="{width}" height="{height}" viewBox="0 0 {width} {height}">"#
    ));
    s.push_str(r#"<defs><marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto"><path d="M0,0 L10,5 L0,10 z" class="arrowhead"/></marker></defs>"#);
    s.push_str(&bands);
    s.push_str(&format!(r#"<g transform="translate({left:.1},0)">"#));
    s.push_str(&lines(plan, focus.as_ref(), &drawing));
    // Beside a task, above its box, the parents whose band does not hold it: within the room a
    // band leaves above its first task (`layout::BAND_PAD`), and cut short of the count of messages
    // at the box's corner.
    let also = |id: &EntityId| -> String {
        let Some(named) = plan.also.get(id) else { return String::new() };
        let words = also_part_of(named, plan.band_of.contains_key(id), &title_of);
        format!(
            r#"<text class="also" x="12" y="-4"><title>{}</title>{}</text>"#,
            fmt::escape(&words),
            fmt::escape(&fmt::truncate(&words, ALSO_LETTERS))
        )
    };
    for id in &plan.kept {
        let (Some(t), Some(&(x, y))) = (by_id.get(id), at.get(id)) else { continue };
        let status = t.status().unwrap_or(TaskStatus::Proposed);
        // Who is doing it: its assignee (docs/DECISIONS.md, D45), where the node showed the owner
        // of its scope until scopes lost their owner (D108).
        let assignee = fwd_core::ops::assignee_of(t).ok().flatten().map(store::handle_of);
        let mut sub = fmt::status_label(status);
        if let Some(pr) = t.priority().ok().flatten() {
            sub.push_str(&format!(" · P{pr}"));
        }
        if let Some(assignee) = assignee {
            sub.push_str(&format!(" · {assignee}"));
        }
        let count = store::message_count(*id, messages);
        let mut class = format!("node status-{status}");
        if status == TaskStatus::Proposed {
            class.push_str(" ghost");
        }
        if selected == Some(*id) {
            class.push_str(" selected");
        }
        if dimmed(id) {
            class.push_str(" dim");
        }
        s.push_str(&format!(
            r#"<g class="{class}" data-id="{}" transform="translate({x:.1},{y:.1})"><title>{}</title><rect width="{W}" height="{H}" rx="7"/><text class="title" x="12" y="24">{}</text><text class="sub" x="12" y="43">{}</text>"#,
            id.to_base64(),
            fmt::escape(&titles[id]),
            fmt::escape(&fmt::truncate(&titles[id], 27)),
            fmt::escape(&sub)
        ));
        if count > 0 {
            s.push_str(&format!(
                r#"<g class="count" transform="translate({:.1},-6)"><circle r="10"/><text text-anchor="middle" y="4">{count}</text></g>"#,
                W - 12.0
            ));
        }
        s.push_str(&also(id));
        s.push_str("</g>");
    }
    // A stub: muted, with the task's title and its scopes' names; its hover adds its status.
    for id in &plan.stubs {
        let (Some(t), Some(&(x, y))) = (by_id.get(id), at.get(id)) else { continue };
        let status = fmt::status_label(t.status().unwrap_or(TaskStatus::Proposed));
        let name = scope_name(id);
        let mut class = String::from("node stub");
        if selected == Some(*id) {
            class.push_str(" selected");
        }
        if dimmed(id) {
            class.push_str(" dim");
        }
        s.push_str(&format!(
            r#"<g class="{class}" data-id="{}" transform="translate({x:.1},{y:.1})"><title>{}</title><rect width="{W}" height="{H}" rx="7"/><text class="title" x="12" y="24">{}</text><text class="sub" x="12" y="43">{}</text>{}</g>"#,
            id.to_base64(),
            fmt::escape(&format!("{}: {status}, {name}", titles[id])),
            fmt::escape(&fmt::truncate(&titles[id], 27)),
            fmt::escape(&fmt::truncate(&name, 30)),
            also(id)
        ));
    }
    s.push_str("</g></svg>");
    s
}

/// What the drawing holds, and everything its layout is made from, so that the page lays out
/// again only when one of them changes (`GraphPage`).
#[derive(Clone, Debug, PartialEq)]
struct Planned {
    plan: Plan,
    /// The tasks drawn as boxes (`Plan::boxed`), in the drawing's order.
    ids: Vec<EntityId>,
    /// Each of them as the layout weighs it, drawn in its band.
    weighed: Vec<layout::Task>,
    bands: Vec<layout::Band>,
    /// The dependencies drawn, by the places of their tasks among `ids`.
    pairs: Vec<(usize, usize)>,
}

impl Planned {
    /// What `plan` gives the layout, each task as `weigh` weighs it.
    fn new(plan: Plan, weigh: impl Fn(&EntityId) -> layout::Task) -> Planned {
        let ids = plan.boxed();
        let weighed = ids.iter().map(|id| layout::Task { band: plan.band_of.get(id).copied(), ..weigh(id) }).collect();
        let bands =
            plan.bands.iter().map(|band| layout::Band { within: band.within, id: band.head.to_base64() }).collect();
        let mut planned = Planned { plan, ids, weighed, bands, pairs: Vec::new() };
        planned.pairs = planned.places(&planned.plan.edges);
        planned
    }

    /// `edges` by the places of their tasks among `ids`; an end the drawing does not hold (the
    /// plan makes none) is a place past them all, which the layout draws no route for.
    fn places(&self, edges: &[Edge]) -> Vec<(usize, usize)> {
        let place: HashMap<EntityId, usize> = self.ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        let place_of = |id: &EntityId| place.get(id).copied().unwrap_or(self.ids.len());
        edges.iter().map(|e| (place_of(&e.first), place_of(&e.then))).collect()
    }

    /// The layout in `order`, which a selection does not change.
    fn layout(&self, order: Order) -> layout::Layout {
        layout::Layout::new(&self.weighed, &self.bands, &self.pairs, order)
    }

    /// The drawing of `laid`, this plan's layout, with `threaded`, a selected task's implied
    /// dependencies, routed through it.
    fn draw(&self, laid: &layout::Layout, threaded: &[Edge]) -> layout::Drawing {
        laid.draw(&self.places(threaded))
    }
}

/// Where a task outside the filter is worked, as its stub and a band's label name it: each of its
/// scopes, any number (docs/DECISIONS.md, D136, "A task may be worked in several scopes"), by the
/// name `title` gives it, else by its short id, commas between them, or "no scope" for a task
/// worked in none (task PHfp4WUc, where "unassigned" came to say only that nobody is assigned).
fn scope_words(scopes: &[EntityId], title: impl Fn(EntityId) -> Option<String>) -> String {
    if scopes.is_empty() {
        return "no scope".to_string();
    }
    scopes.iter().map(|scope| title(*scope).unwrap_or_else(|| fmt::short(*scope))).collect::<Vec<_>>().join(", ")
}

/// What a band's label says of the parent heading it: its title and status, how many of its
/// subtasks are done of how many, and, when the scope filter leaves the parent's scope out, where
/// it is worked.
struct Head {
    id: EntityId,
    title: String,
    status: TaskStatus,
    done: usize,
    total: usize,
    outside: Option<String>,
}

impl Head {
    /// The head of the band task `id` heads, worked in `scopes`, which `choice` may leave out: a
    /// parent is outside by its scopes alone, so that a closed one in the scope shown, which
    /// "show closed" leaves out of the drawing while it still bands its open subtasks, reads as
    /// closed, not as worked elsewhere, and one worked in the scope shown among others is inside.
    /// `scope_title` names a scope.
    fn new(
        id: EntityId,
        title: String,
        status: TaskStatus,
        (done, total): (usize, usize),
        scopes: &[EntityId],
        choice: ScopeChoice,
        scope_title: impl Fn(EntityId) -> Option<String>,
    ) -> Head {
        let outside = (!choice.keeps(scopes)).then(|| scope_words(scopes, scope_title));
        Head { id, title, status, done, total, outside }
    }
}

/// The markup of `plan`'s bands where `drawing` puts them, each band's head in `heads` by the
/// band's index, and how wide the column at the left that holds their labels is, wide enough for
/// the most deeply nested; none without bands. Each band is a strip across the drawing, a nested
/// one further right, drawn after the band it is in so that it lies over it; then each band's
/// label at its band's top in the column, which a click selects the parent by. A selected parent's
/// band and label are marked, and with a task selected (`focus`), a label whose parent it does not
/// light is dimmed.
fn band_markup(
    plan: &Plan,
    heads: &[Head],
    drawing: &layout::Drawing,
    selected: Option<EntityId>,
    focus: Option<&Focus>,
) -> (String, f64) {
    let depths: Vec<usize> = (0..plan.bands.len()).map(|b| plan.depth(b)).collect();
    let left = match depths.iter().max() {
        Some(&deepest) => LABEL_W + INDENT * deepest as f64,
        None => return (String::new(), 0.0),
    };
    let width = left + drawing.width;
    let mut outermost_first: Vec<usize> = (0..plan.bands.len()).collect();
    outermost_first.sort_by_key(|&b| depths[b]);
    let (mut strips, mut labels) = (String::new(), String::new());
    for &b in &outermost_first {
        let (Some((top, bottom)), Some(head)) = (drawing.bands[b], heads.get(b)) else { continue };
        let x = INDENT * depths[b] as f64;
        let mut class = String::from(if depths[b] % 2 == 1 { "band nested" } else { "band" });
        if head.status == TaskStatus::Proposed {
            class.push_str(" ghost");
        }
        if selected == Some(head.id) {
            class.push_str(" selected");
        }
        strips.push_str(&format!(
            r#"<rect class="{class}" x="{x:.1}" y="{top:.1}" width="{:.1}" height="{:.1}" rx="8"/>"#,
            width - x,
            bottom - top
        ));
        let sub = format!("{} · {} of {} done", fmt::status_label(head.status), head.done, head.total);
        let mut class = String::from("band-label");
        let mut about = format!("{}: {sub}", head.title);
        if let Some(scope) = &head.outside {
            class.push_str(" outside");
            about.push_str(&format!(", {scope}"));
        }
        if fmt::is_closed(head.status) {
            class.push_str(" closed");
        }
        if selected == Some(head.id) {
            class.push_str(" selected");
        }
        if focus.is_some_and(|focus| !focus.tasks.contains(&head.id)) {
            class.push_str(" dim");
        }
        // About seven pixels a letter at the label's size.
        let letters = ((left - x - 20.0) / 7.0).max(8.0) as usize;
        labels.push_str(&format!(
            r#"<g class="{class}" data-id="{}" transform="translate({:.1},{top:.1})"><title>{}</title><text class="title" y="22">{}</text><text class="sub" y="38">{}</text></g>"#,
            head.id.to_base64(),
            x + 10.0,
            fmt::escape(&about),
            fmt::escape(&fmt::truncate(&head.title, letters)),
            fmt::escape(&fmt::truncate(&sub, letters + 4))
        ));
    }
    (strips + &labels, left)
}

/// How many of each parent's subtasks are done, and how many it has, by the accepted subtask
/// relations among `edges`, for its band's label.
fn progress(edges: &[Edge], status_of: &dyn Fn(&EntityId) -> TaskStatus) -> HashMap<EntityId, (usize, usize)> {
    let mut progress: HashMap<EntityId, (usize, usize)> = HashMap::new();
    for e in edges.iter().filter(|e| e.kind == RelationKind::SubtaskOf && !e.proposed) {
        let (done, total) = progress.entry(e.then).or_default();
        *total += 1;
        if status_of(&e.first) == TaskStatus::Done {
            *done += 1;
        }
    }
    progress
}

/// What a task's box says of the parents `named` beside it (`Plan::also`), by their titles: "part
/// of" the accepted ones, "also part of" them when a band holds the task (`banded`), and "might be
/// part of" the proposed ones.
fn also_part_of(named: &[Edge], banded: bool, title_of: &dyn Fn(&EntityId) -> String) -> String {
    let titles = |proposed: bool| -> Vec<String> {
        named.iter().filter(|e| e.proposed == proposed).map(|e| title_of(&e.then)).collect()
    };
    let (accepted, proposed) = (titles(false), titles(true));
    let mut words = Vec::new();
    if !accepted.is_empty() {
        let also = if banded { "also " } else { "" };
        words.push(format!("{also}part of {}", accepted.join(", ")));
    }
    if !proposed.is_empty() {
        words.push(format!("might be part of {}", proposed.join(", ")));
    }
    words.join("; ")
}

/// The markup of the drawing's lines, each dependency `plan` draws along its route in `drawing`.
/// With a task selected (`focus`), the lit lines are drawn over the rest, which are dimmed, and its
/// implied dependencies are drawn over them all, dashed, along their threaded routes.
fn lines(plan: &Plan, focus: Option<&Focus>, drawing: &layout::Drawing) -> String {
    let (mut under, mut over) = (String::new(), String::new());
    for (i, (edge, route)) in plan.edges.iter().zip(&drawing.routes).enumerate() {
        let Some(d) = path(route) else { continue };
        let mut class = String::from("edge edge-depends");
        if edge.proposed {
            class.push_str(" edge-proposed");
        }
        let lit = focus.map(|focus| focus.lit[i]);
        match lit {
            Some(true) => class.push_str(" lit"),
            Some(false) => class.push_str(" dim"),
            None => {}
        }
        let markup = if lit == Some(true) { &mut over } else { &mut under };
        markup.push_str(&format!(r#"<path class="{class}" d="{d}" marker-end="url(#arrow)"/>"#));
    }
    for route in &drawing.threaded {
        let Some(d) = path(route) else { continue };
        over.push_str(&format!(
            r#"<path class="edge edge-depends edge-implied lit" d="{d}" marker-end="url(#arrow)"/>"#
        ));
    }
    under + &over
}

/// An edge's route (`layout::Drawing::routes`) as an SVG path: level from a point to the next at the
/// same height, and to a point at another height a curve that leaves level and arrives level, its
/// control points halfway across. None for a route of fewer than two points.
fn path(route: &[(f64, f64)]) -> Option<String> {
    let (&(x, y), rest) = route.split_first()?;
    if rest.is_empty() {
        return None;
    }
    let mut d = format!("M{x:.1},{y:.1}");
    let mut from = (x, y);
    for &(x, y) in rest {
        if y == from.1 {
            d.push_str(&format!(" L{x:.1},{y:.1}"));
        } else {
            let halfway = (from.0 + x) / 2.0;
            d.push_str(&format!(" C{halfway:.1},{:.1} {halfway:.1},{y:.1} {x:.1},{y:.1}", from.1));
        }
        from = (x, y);
    }
    Some(d)
}

/// The order this browser remembers, when it has one and lets the page read it; else auto.
fn remembered_order() -> Order {
    let storage = web_sys::window().and_then(|w| w.local_storage().ok().flatten());
    storage.and_then(|s| s.get_item(ORDER).ok().flatten()).and_then(|name| Order::named(&name)).unwrap_or_default()
}

/// Remembers `order` when the browser lets the page write there; it holds while the page is open
/// either way.
fn remember_order(order: Order) {
    if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = storage.set_item(ORDER, order.name());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> EntityId {
        EntityId::from_bytes([n; 32])
    }

    fn task(n: u8, scope: Option<u8>, closed: bool) -> TaskFacts {
        TaskFacts { id: id(n), closed, scopes: scope.map(id).into_iter().collect() }
    }

    /// `then` depends on `first`.
    fn waits(then: u8, first: u8) -> Edge {
        Edge { first: id(first), then: id(then), kind: RelationKind::DependsOn, proposed: false }
    }

    /// `plan` laid out in `order` with `threaded` threaded through it, as the page lays it out
    /// (`Planned`): the tasks drawn as boxes, in the drawing's order, and the drawing.
    fn laid_out(
        plan: &Plan,
        weigh: impl Fn(&EntityId) -> layout::Task,
        threaded: &[Edge],
        order: Order,
    ) -> (Vec<EntityId>, layout::Drawing) {
        let planned = Planned::new(plan.clone(), weigh);
        let drawing = planned.draw(&planned.layout(order), threaded);
        (planned.ids, drawing)
    }

    /// The same, proposed.
    fn might_wait(then: u8, first: u8) -> Edge {
        Edge { proposed: true, ..waits(then, first) }
    }

    /// `child` is a subtask of `parent`.
    fn part_of(child: u8, parent: u8) -> Edge {
        Edge { first: id(child), then: id(parent), kind: RelationKind::SubtaskOf, proposed: false }
    }

    /// The same, proposed.
    fn might_part_of(child: u8, parent: u8) -> Edge {
        Edge { proposed: true, ..part_of(child, parent) }
    }

    /// A scope of 71 tasks made up from a fixed seed, shaped as the scope the Graph study measured
    /// (task odEHLbf7, "Study why the Graph page draws a tidy task tree as a tangle, and mock up a
    /// better layout") is: a kickoff task (0); five decisions that wait for it (11 to 15) and a
    /// foundation that waits for a decision (16); 54 more tasks (17 to 70), each waiting for one to
    /// four of the tasks before it, a task already waited for the likelier to be waited for again,
    /// so that a few become busy, and most of them also waiting for a task that one of those waits
    /// for, a dependency a chain already says; and ten parent tasks (1 to 10), with no
    /// dependencies of their own, holding the sixty after the kickoff between them, three to
    /// fifteen each, mostly runs of neighbouring tasks. Every relation is accepted, and every task
    /// open; each task's priority is drawn from the seed.
    fn synthetic_scope() -> (Vec<layout::Task>, Vec<Edge>) {
        let mut state: u64 = 0x2545_F491_4F6C_DD1D;
        let mut next = |below: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % below as u64) as usize
        };
        let tasks: Vec<layout::Task> = (0..71u8)
            .map(|n| layout::Task {
                title: format!("task {n}"),
                id: id(n).to_base64(),
                priority: [None, Some(0), Some(1), Some(1), Some(2), Some(2), Some(3)][next(7)],
                status: fmt::status_rank(TaskStatus::Open),
                band: None,
            })
            .collect();
        // What each task waits for, and how many wait for each.
        let mut waits_for: Vec<Vec<u8>> = vec![Vec::new(); 71];
        let mut waited: Vec<usize> = vec![0; 71];
        fn wait(then: u8, first: u8, waits_for: &mut [Vec<u8>], waited: &mut [usize]) {
            if first != then && !waits_for[then as usize].contains(&first) {
                waits_for[then as usize].push(first);
                waited[first as usize] += 1;
            }
        }
        for decision in 11..=15 {
            wait(decision, 0, &mut waits_for, &mut waited);
        }
        wait(16, 11, &mut waits_for, &mut waited);
        for then in 17..=70u8 {
            let count = [1, 2, 2, 2, 3, 3, 3, 4, 4][next(9)];
            for _ in 0..count {
                // A task before this one, each as likely as one more than twice the number
                // of tasks waiting for it.
                let weights: Vec<usize> = (11..then).map(|t| 1 + 2 * waited[t as usize]).collect();
                let mut pick = next(weights.iter().sum());
                let mut first = 11;
                for (t, weight) in (11..then).zip(&weights) {
                    if pick < *weight {
                        first = t;
                        break;
                    }
                    pick -= weight;
                }
                wait(then, first, &mut waits_for, &mut waited);
            }
            // And, three times in four, a task that one of those waits for.
            if next(4) < 3 {
                let direct = waits_for[then as usize].clone();
                let through = direct[next(direct.len())];
                let further = &waits_for[through as usize];
                if !further.is_empty() {
                    let first = further[next(further.len())];
                    wait(then, first, &mut waits_for, &mut waited);
                }
            }
        }
        let mut edges: Vec<Edge> = Vec::new();
        for (then, firsts) in waits_for.iter().enumerate() {
            edges.extend(firsts.iter().map(|&first| waits(then as u8, first)));
        }
        // Ten runs of the sixty, three to fifteen tasks long; then twelve times two of the sixty
        // trade parents.
        let mut sizes = [3usize; 10];
        for _ in 0..30 {
            let parent = loop {
                let parent = next(10);
                if sizes[parent] < 15 {
                    break parent;
                }
            };
            sizes[parent] += 1;
        }
        let mut parent_of: Vec<u8> = sizes.iter().enumerate().flat_map(|(p, &size)| vec![1 + p as u8; size]).collect();
        for _ in 0..12 {
            let (a, b) = (next(60), next(60));
            parent_of.swap(a, b);
        }
        edges.extend(parent_of.iter().enumerate().map(|(k, &parent)| part_of(11 + k as u8, parent)));
        (tasks, edges)
    }

    /// Every pair of tasks that the dependencies among `edges` join, directly or by a chain.
    fn reached(edges: &[Edge]) -> HashSet<(EntityId, EntityId)> {
        let mut reached: HashSet<(EntityId, EntityId)> =
            edges.iter().filter(|e| e.kind == RelationKind::DependsOn).map(|e| (e.first, e.then)).collect();
        loop {
            let further: Vec<(EntityId, EntityId)> = reached
                .iter()
                .flat_map(|&(a, b)| reached.iter().filter(move |&&(c, _)| c == b).map(move |&(_, d)| (a, d)))
                .filter(|pair| !reached.contains(pair))
                .collect();
            if further.is_empty() {
                return reached;
            }
            reached.extend(further);
        }
    }

    /// The most lines meeting a task at one point.
    fn most_at_one_point(routes: &[Vec<(f64, f64)>]) -> usize {
        let mut ends: HashMap<(i64, i64), usize> = HashMap::new();
        for route in routes.iter().filter(|route| route.len() > 1) {
            for (x, y) in [route[0], route[route.len() - 1]] {
                *ends.entry(((x * 100.0).round() as i64, (y * 100.0).round() as i64)).or_default() += 1;
            }
        }
        ends.into_values().max().unwrap_or(0)
    }

    /// The pairs the layout takes for `edges` among tasks whose places are their numbers.
    fn places(edges: &[Edge]) -> Vec<(usize, usize)> {
        edges.iter().map(|e| (e.first.to_bytes()[0] as usize, e.then.to_bytes()[0] as usize)).collect()
    }

    #[test]
    fn a_dependency_that_a_chain_of_accepted_ones_already_says_is_left_out() {
        // 2 waits for 1, 3 for 2 and 4 for 3; 3 also waits for 1, and 4 for 1 and for 2, which the
        // chain already says.
        let tasks: Vec<TaskFacts> = (1..=4).map(|n| task(n, Some(50), false)).collect();
        let edges = [waits(2, 1), waits(3, 1), waits(3, 2), waits(4, 1), waits(4, 2), waits(4, 3)];
        let drawn = plan(&tasks, &edges, ScopeChoice::Scope(id(50)), false);
        assert_eq!(drawn.edges, vec![waits(2, 1), waits(3, 2), waits(4, 3)]);
        assert_eq!(drawn.implied, vec![waits(3, 1), waits(4, 1), waits(4, 2)]);
        // Two relations that say the same dependency are both drawn, the one over the other: a
        // chain is two dependencies or more.
        let twice = [waits(2, 1), waits(2, 1)];
        assert_eq!(plan(&tasks, &twice, ScopeChoice::Scope(id(50)), false).edges, twice.to_vec());
    }

    #[test]
    fn a_proposed_relation_hides_nothing_and_nothing_hides_it() {
        let tasks: Vec<TaskFacts> = (1..=3).map(|n| task(n, Some(50), false)).collect();
        let scope = ScopeChoice::Scope(id(50));
        // A chain with a proposed link says nothing yet, so 3 is drawn waiting for 1.
        let edges = [might_wait(2, 1), waits(3, 2), waits(3, 1)];
        assert_eq!(plan(&tasks, &edges, scope, false).edges, edges.to_vec());
        // A proposed dependency that an accepted chain already says is drawn, since it waits on
        // someone's word.
        let edges = [waits(2, 1), waits(3, 2), might_wait(3, 1)];
        let drawn = plan(&tasks, &edges, scope, false);
        assert_eq!((drawn.edges, drawn.implied), (edges.to_vec(), vec![]));
    }

    #[test]
    fn subtask_relations_make_no_chain_and_draw_no_line() {
        let tasks: Vec<TaskFacts> = (1..=3).map(|n| task(n, Some(50), false)).collect();
        let scope = ScopeChoice::Scope(id(50));
        // 1 is a subtask of 2, which 3 waits for: 3 is still drawn waiting for 1, and 1 is drawn
        // in the band 2 heads.
        let edges = [part_of(1, 2), waits(3, 2), waits(3, 1)];
        let drawn = plan(&tasks, &edges, scope, false);
        assert_eq!(drawn.edges, vec![waits(3, 2), waits(3, 1)]);
        assert_eq!(drawn.bands, vec![Band { head: id(2), within: None }]);
        assert_eq!(drawn.band_of.get(&id(1)), Some(&0));
    }

    /// A plan with no bands.
    fn unbanded(kept: &[u8], stubs: &[u8], edges: &[Edge]) -> Plan {
        Plan {
            kept: kept.iter().map(|&n| id(n)).collect(),
            stubs: stubs.iter().map(|&n| id(n)).collect(),
            edges: edges.to_vec(),
            implied: vec![],
            bands: vec![],
            band_of: HashMap::new(),
            also: HashMap::new(),
        }
    }

    #[test]
    fn a_parent_task_heads_a_band_of_its_subtasks_and_a_deeper_hierarchy_nests_bands() {
        // 1 holds 2 and 3, and 3 holds 4 and 5; 6 is part of nothing; 4 waits for 6.
        let tasks: Vec<TaskFacts> = (1..=6).map(|n| task(n, Some(50), false)).collect();
        let edges = [part_of(2, 1), part_of(3, 1), part_of(4, 3), part_of(5, 3), waits(4, 6)];
        let drawn = plan(&tasks, &edges, ScopeChoice::Scope(id(50)), false);
        assert_eq!(drawn.edges, vec![waits(4, 6)]);
        assert_eq!(drawn.bands, vec![Band { head: id(1), within: None }, Band { head: id(3), within: Some(0) }]);
        let band_of = |n: u8| drawn.band_of.get(&id(n)).copied();
        // A parent is drawn in its own band: 1 in its own, 3 in its own, nested in 1's.
        let bands: Vec<Option<usize>> = (1..=6).map(band_of).collect();
        assert_eq!(bands, [Some(0), Some(0), Some(1), Some(1), Some(1), None]);
        assert!(drawn.also.is_empty());
        // Neither parent has a dependency drawn, so each is its band's label alone.
        assert_eq!(drawn.boxed(), [id(2), id(4), id(5), id(6)]);
        assert_eq!((drawn.depth(0), drawn.depth(1)), (0, 1));
    }

    #[test]
    fn a_task_with_two_parents_sits_in_the_band_of_one_and_names_the_other() {
        // 3 is part of 1 and of 2, and might be part of 4.
        let tasks: Vec<TaskFacts> = (1..=4).map(|n| task(n, Some(50), false)).collect();
        let edges = [part_of(3, 2), part_of(3, 1), might_part_of(3, 4)];
        let drawn = plan(&tasks, &edges, ScopeChoice::Scope(id(50)), false);
        // Both kept, the first by id, 1, holds it; 2 heads no band, holding nothing else.
        assert_eq!(drawn.bands, vec![Band { head: id(1), within: None }]);
        assert_eq!(drawn.band_of.get(&id(3)), Some(&0));
        assert_eq!(drawn.also.get(&id(3)), Some(&vec![part_of(3, 2), might_part_of(3, 4)]));
        let title = |id: &EntityId| format!("task {}", id.to_bytes()[0]);
        assert_eq!(also_part_of(&drawn.also[&id(3)], true, &title), "also part of task 2; might be part of task 4");
        // A task no band holds is part of its parents, not also.
        assert_eq!(also_part_of(&[part_of(3, 2)], false, &title), "part of task 2");
        // A parent the filter keeps comes before one it does not, whatever their ids.
        let tasks = [task(1, Some(60), false), task(2, Some(50), false), task(3, Some(50), false)];
        let drawn = plan(&tasks, &edges[..2], ScopeChoice::Scope(id(50)), false);
        assert_eq!(drawn.bands, vec![Band { head: id(2), within: None }]);
        assert_eq!(drawn.also.get(&id(3)), Some(&vec![part_of(3, 1)]));
    }

    #[test]
    fn a_band_is_labelled_with_its_parent_its_status_and_how_many_of_its_subtasks_are_done() {
        // 1, in another scope, holds 2, 3 and 4, and 4 holds 5; 5 waits for 2.
        let tasks = [
            task(1, Some(60), false),
            task(2, Some(50), false),
            task(3, Some(50), false),
            task(4, Some(50), false),
            task(5, Some(50), false),
        ];
        let edges = [part_of(2, 1), part_of(3, 1), part_of(4, 1), part_of(5, 4), waits(5, 2)];
        let drawn = plan(&tasks, &edges, ScopeChoice::Scope(id(50)), false);
        assert_eq!(drawn.bands, vec![Band { head: id(1), within: None }, Band { head: id(4), within: Some(0) }]);
        let weigh = |id: &EntityId| {
            let n = id.to_bytes()[0];
            layout::Task { title: format!("{n}"), id: format!("{n}"), priority: None, status: 0, band: None }
        };
        let (_, drawing) = laid_out(&drawn, weigh, &[], Order::Auto);
        let head = |n: u8, title: &str, status: TaskStatus, (done, total): (usize, usize), outside: Option<&str>| {
            Head { id: id(n), title: title.into(), status, done, total, outside: outside.map(String::from) }
        };
        let heads = [
            head(1, "Ship the hub", TaskStatus::Open, (1, 3), Some("Later")),
            head(4, "Polish", TaskStatus::Proposed, (0, 1), None),
        ];
        let (markup, left) = band_markup(&drawn, &heads, &drawing, Some(id(4)), None);
        assert_eq!(left, LABEL_W + INDENT);
        // The outer band, then the nested one over it, further right, its parent proposed and
        // selected; then their labels.
        let outer = markup.find(r#"<rect class="band" x="0.0""#).expect("the outer band");
        let inner = markup.find(r#"<rect class="band nested ghost selected" x="14.0""#).expect("the nested band");
        let label = markup.find(r#"<g class="band-label"#).expect("a label");
        assert!(outer < inner && inner < label, "{markup}");
        assert!(markup.contains(r#"<g class="band-label outside" data-id=""#), "{markup}");
        assert!(markup.contains("<title>Ship the hub: open · 1 of 3 done, Later</title>"), "{markup}");
        assert!(markup.contains(r#"<text class="sub" y="38">proposed · 0 of 1 done</text>"#), "{markup}");
        assert!(markup.contains(r#"<g class="band-label selected""#), "{markup}");
        // Selecting 2 lights 5, which waits for it, but neither parent, whose labels dim.
        let focus = focus(&drawn, id(2)).expect("a task the drawing holds");
        let (markup, _) = band_markup(&drawn, &heads, &drawing, Some(id(2)), Some(&focus));
        assert!(
            markup.contains(r#"<g class="band-label outside dim""#) && markup.contains(r#"<g class="band-label dim""#)
        );
        // With no bands, no column for their labels.
        let unbanded = plan(&tasks[1..], &[waits(5, 2)], ScopeChoice::Scope(id(50)), false);
        assert_eq!(band_markup(&unbanded, &[], &drawing, None, None), (String::new(), 0.0));
    }

    #[test]
    fn a_loop_of_subtask_relations_is_cut_where_it_closes_and_every_task_on_it_is_drawn() {
        let scope = ScopeChoice::Scope(id(50));
        // Whether every task the filter keeps is drawn: as a box, or as the label of a band that
        // holds a box, in it or in a band nested in it, without which the band is not drawn.
        let all_drawn = |drawn: &Plan| {
            let boxed = drawn.boxed();
            let holds_a_box = |band: usize| {
                boxed.iter().any(|t| {
                    let mut at = drawn.band_of.get(t).copied();
                    for _ in 0..=drawn.bands.len() {
                        match at {
                            Some(b) if b == band => return true,
                            Some(b) => at = drawn.bands[b].within,
                            None => return false,
                        }
                    }
                    false
                })
            };
            drawn.kept.iter().all(|t| {
                boxed.contains(t) || drawn.bands.iter().position(|band| band.head == *t).is_some_and(holds_a_box)
            })
        };
        // 1 is part of 2 and 2 part of 1, as relations arriving through sync can say: the loop's
        // first task, 1, keeps its parent, and 2, whose parent 1 is, gets none.
        let tasks: Vec<TaskFacts> = (1..=2).map(|n| task(n, Some(50), false)).collect();
        let drawn = plan(&tasks, &[part_of(1, 2), part_of(2, 1)], scope, false);
        assert_eq!(drawn.bands, vec![Band { head: id(2), within: None }]);
        assert_eq!(drawn.band_of.get(&id(1)), Some(&0));
        assert_eq!(drawn.boxed(), [id(1)]);
        assert!(all_drawn(&drawn));
        // A longer loop, 1 in 2, 2 in 3 and 3 in 1, with 4 in 1 beside it: 3 gets no parent, and
        // the bands nest 3, then 2, then 1.
        let tasks: Vec<TaskFacts> = (1..=4).map(|n| task(n, Some(50), false)).collect();
        let edges = [part_of(1, 2), part_of(2, 3), part_of(3, 1), part_of(4, 1)];
        let drawn = plan(&tasks, &edges, scope, false);
        let heads: Vec<(EntityId, Option<EntityId>)> =
            drawn.bands.iter().map(|band| (band.head, band.within.map(|w| drawn.bands[w].head))).collect();
        assert_eq!(heads, [(id(1), Some(id(2))), (id(2), Some(id(3))), (id(3), None)]);
        assert_eq!(drawn.also.get(&id(3)), Some(&vec![part_of(3, 1)]), "the cut relation is named beside its task");
        assert!(all_drawn(&drawn));
    }

    #[test]
    fn a_closed_parent_still_bands_its_open_subtasks_and_reads_as_closed_not_outside() {
        // 1, done, holds 2, open, both in scope 50; 3, done, in scope 60, holds 4, open, in 50.
        let tasks =
            [task(1, Some(50), true), task(2, Some(50), false), task(3, Some(60), true), task(4, Some(50), false)];
        let edges = [part_of(2, 1), part_of(4, 3)];
        let scope = ScopeChoice::Scope(id(50));
        let drawn = plan(&tasks, &edges, scope, false);
        assert_eq!(drawn.kept, vec![id(2), id(4)], "show closed is off");
        assert_eq!(drawn.bands, vec![Band { head: id(1), within: None }, Band { head: id(3), within: None }]);
        let bands: Vec<Option<usize>> = [2, 4].iter().map(|&n| drawn.band_of.get(&id(n)).copied()).collect();
        assert_eq!(bands, [Some(0), Some(1)]);
        let title = |scope: EntityId| (scope == id(60)).then(|| "Later".to_string());
        let heads = [
            Head::new(id(1), "Ship".into(), TaskStatus::Done, (0, 1), &[id(50)], scope, title),
            Head::new(id(3), "Polish".into(), TaskStatus::Cancelled, (0, 1), &[id(60)], scope, title),
        ];
        // 1 is in the scope shown: closed, not outside; 3 is worked elsewhere, and closed too.
        assert_eq!((heads[0].outside.as_deref(), heads[1].outside.as_deref()), (None, Some("Later")));
        // A parent worked in the scope shown among others is inside; one in two other scopes names
        // both (docs/DECISIONS.md, D136, "A task may be worked in several scopes").
        let both = Head::new(id(1), "Ship".into(), TaskStatus::Done, (0, 1), &[id(60), id(50)], scope, title);
        assert_eq!(both.outside, None);
        let elsewhere = Head::new(id(3), "Polish".into(), TaskStatus::Done, (0, 1), &[id(60), id(70)], scope, title);
        assert_eq!(elsewhere.outside, Some(format!("Later, {}", fmt::short(id(70)))));
        let weigh = |id: &EntityId| {
            let n = id.to_bytes()[0];
            layout::Task { title: format!("{n}"), id: format!("{n}"), priority: None, status: 0, band: None }
        };
        let (_, drawing) = laid_out(&drawn, weigh, &[], Order::Auto);
        let (markup, _) = band_markup(&drawn, &heads, &drawing, None, None);
        assert!(markup.contains(r#"<g class="band-label closed" data-id=""#), "{markup}");
        assert!(markup.contains(r#"<g class="band-label outside closed" data-id=""#), "{markup}");
    }

    #[test]
    fn a_parent_outside_the_scope_still_bands_the_subtasks_inside_it() {
        // 1, in another scope, holds 2 and 3, in this one; 4, in this one, holds 5, in another,
        // which is drawn as a stub in 4's band.
        let tasks = [
            task(1, Some(60), false),
            task(2, Some(50), false),
            task(3, Some(50), false),
            task(4, Some(50), false),
            task(5, Some(60), false),
        ];
        let edges = [part_of(2, 1), part_of(3, 1), part_of(5, 4), waits(3, 2)];
        let drawn = plan(&tasks, &edges, ScopeChoice::Scope(id(50)), false);
        assert_eq!((drawn.kept, drawn.stubs.clone()), (vec![id(2), id(3), id(4)], vec![id(5)]));
        assert_eq!(drawn.bands, vec![Band { head: id(1), within: None }, Band { head: id(4), within: None }]);
        let bands: Vec<Option<usize>> = [2, 3, 5].iter().map(|&n| drawn.band_of.get(&id(n)).copied()).collect();
        assert_eq!(bands, [Some(0), Some(0), Some(1)]);
        // A stub makes no band: 5's own subtask, 6, outside, is not drawn.
        let edges = [part_of(5, 4), part_of(6, 5)];
        let tasks = [task(4, Some(50), false), task(5, Some(60), false), task(6, Some(60), false)];
        let drawn = plan(&tasks, &edges, ScopeChoice::Scope(id(50)), false);
        assert_eq!((drawn.stubs, drawn.bands), (vec![id(5)], vec![Band { head: id(4), within: None }]));
    }

    #[test]
    fn only_a_chain_the_drawing_holds_implies_a_dependency() {
        let scope = ScopeChoice::Scope(id(50));
        // 3 waits for 2, which waits for 1, and for 1 too. Through a closed task, drawn only
        // with "show closed", the chain says nothing until it is drawn.
        let tasks = [task(1, Some(50), false), task(2, Some(50), true), task(3, Some(50), false)];
        let edges = [waits(2, 1), waits(3, 2), waits(3, 1)];
        assert_eq!(plan(&tasks, &edges, scope, false).edges, vec![waits(3, 1)]);
        let every = plan(&tasks, &edges, scope, true);
        assert_eq!((every.edges, every.implied), (vec![waits(2, 1), waits(3, 2)], vec![waits(3, 1)]));
        // Through a stub, whose two lines are drawn.
        let tasks = [task(1, Some(50), false), task(2, Some(60), false), task(3, Some(50), false)];
        let drawn = plan(&tasks, &edges, scope, false);
        assert_eq!((drawn.edges, drawn.implied), (vec![waits(2, 1), waits(3, 2)], vec![waits(3, 1)]));
        // Through two stubs, between which no line is drawn, since a stub brings none of its own.
        let tasks =
            [task(1, Some(50), false), task(2, Some(60), false), task(3, Some(60), false), task(4, Some(50), false)];
        let edges = [waits(2, 1), waits(3, 2), waits(4, 3), waits(4, 1)];
        assert_eq!(plan(&tasks, &edges, scope, false).edges, vec![waits(2, 1), waits(4, 3), waits(4, 1)]);
    }

    #[test]
    fn every_task_still_reaches_what_it_reached_should_accepted_dependencies_close_a_loop() {
        // 1 and 2 wait for each other, as relations arriving through sync can say, and 3 waits
        // for both.
        let tasks: Vec<TaskFacts> = (1..=3).map(|n| task(n, Some(50), false)).collect();
        let edges = [waits(2, 1), waits(1, 2), waits(3, 1), waits(3, 2)];
        let drawn = plan(&tasks, &edges, ScopeChoice::Scope(id(50)), false);
        assert_eq!(drawn.implied, vec![waits(3, 1)]);
        assert_eq!(reached(&drawn.edges), reached(&edges));
    }

    #[test]
    fn selecting_a_task_lights_its_chain_both_ways_and_its_implied_dependencies() {
        // 1, 2, 3 and 4 make a chain, which 5 joins at 3 and 6 leaves from 3; 3's wait for 1 is
        // implied; 10 might wait for 4; 8 waits for 7, apart; 9 is a subtask of 3, and 11 of 12,
        // which has no dependency.
        let tasks: Vec<TaskFacts> = (1..=12).map(|n| task(n, Some(50), false)).collect();
        let edges = [
            waits(2, 1),
            waits(3, 2),
            waits(3, 1),
            waits(4, 3),
            waits(3, 5),
            waits(6, 3),
            might_wait(10, 4),
            waits(8, 7),
            part_of(9, 3),
            part_of(11, 12),
        ];
        let drawn = plan(&tasks, &edges, ScopeChoice::Scope(id(50)), false);
        assert_eq!(drawn.implied, vec![waits(3, 1)]);
        // What selecting a task lights: its chain's tasks, in order, its chain's lines, and its
        // implied dependencies.
        let lit = |selected: u8| {
            let focus = focus(&drawn, id(selected)).expect("a task the drawing holds");
            let mut tasks: Vec<EntityId> = focus.tasks.into_iter().collect();
            tasks.sort();
            let lines: Vec<Edge> =
                drawn.edges.iter().zip(&focus.lit).filter(|(_, lit)| **lit).map(|(e, _)| *e).collect();
            (tasks, lines, focus.implied)
        };
        let ids = |tasks: &[u8]| tasks.iter().map(|&n| id(n)).collect::<Vec<_>>();
        // 3: back to 1 and 5, on to 4, 6 and, by a proposal, 10, and 9, which its band holds; 7
        // and 8 dim.
        let chain = vec![waits(2, 1), waits(3, 2), waits(4, 3), waits(3, 5), waits(6, 3), might_wait(10, 4)];
        assert_eq!(lit(3), (ids(&[1, 2, 3, 4, 5, 6, 9, 10]), chain, vec![waits(3, 1)]));
        // 2: back to 1, on through 3; 5 joins after 2, so it and its line dim.
        let chain = vec![waits(2, 1), waits(3, 2), waits(4, 3), waits(6, 3), might_wait(10, 4)];
        assert_eq!(lit(2), (ids(&[1, 2, 3, 4, 6, 10]), chain, vec![]));
        // 1: the implied dependency it is the other task of is drawn too.
        assert_eq!(lit(1).2, vec![waits(3, 1)]);
        assert_eq!(lit(7), (ids(&[7, 8]), vec![waits(8, 7)], vec![]));
        // 12, drawn as its band's label alone, lights what its band holds.
        assert_eq!(lit(12), (ids(&[11, 12]), vec![], vec![]));
        // A task the drawing does not hold lights nothing.
        assert_eq!(focus(&drawn, id(99)), None);
    }

    #[test]
    fn a_selected_tasks_chain_is_drawn_over_the_rest_and_its_implied_dependencies_dashed_over_both() {
        // 2 waits for 1, which waits for 0, and for 0 by that chain; 3 waits for 0 alone.
        let tasks: Vec<TaskFacts> = (0..4).map(|n| task(n, Some(50), false)).collect();
        let edges = [waits(3, 0), waits(1, 0), waits(2, 1), waits(2, 0)];
        let drawn = plan(&tasks, &edges, ScopeChoice::Scope(id(50)), false);
        let weigh = |id: &EntityId| {
            let n = id.to_bytes()[0];
            layout::Task { title: format!("{n}"), id: format!("{n}"), priority: None, status: 0, band: None }
        };
        let classes = |markup: String| -> Vec<String> {
            markup
                .split(r#"<path class=""#)
                .skip(1)
                .map(|path| path.split('"').next().unwrap_or_default().to_string())
                .collect()
        };
        let (_, plain) = laid_out(&drawn, weigh, &[], Order::Auto);
        assert_eq!(classes(lines(&drawn, None, &plain)), ["edge edge-depends"; 3]);
        let focus = focus(&drawn, id(2)).expect("a task the drawing holds");
        let (_, drawing) = laid_out(&drawn, weigh, &focus.implied, Order::Auto);
        assert_eq!(drawing.boxes, plain.boxes, "selecting a task moves nothing");
        assert_eq!(
            classes(lines(&drawn, Some(&focus), &drawing)),
            [
                "edge edge-depends dim",
                "edge edge-depends lit",
                "edge edge-depends lit",
                "edge edge-depends edge-implied lit"
            ]
        );
    }

    /// The most lines that met a task at one point on `synthetic_scope` as the page drew it before
    /// it left implied dependencies out and gave each line its own end (`graph.rs` and `layout.rs`
    /// at f946d74).
    const MOST_AT_ONE_POINT_BEFORE: usize = 32;

    #[test]
    fn on_a_scope_shaped_as_the_study_measured_the_page_draws_fewer_lines_crossing_less() {
        let (weighed, edges) = synthetic_scope();
        let dependencies = edges.iter().filter(|e| e.kind == RelationKind::DependsOn).count();
        assert_eq!((weighed.len(), dependencies, edges.len() - dependencies), (71, 174, 60));
        let tasks: Vec<TaskFacts> = (0..71).map(|n| task(n, Some(200), false)).collect();
        let drawn = plan(&tasks, &edges, ScopeChoice::Scope(id(200)), false);
        // 69 dependencies are implied, and the 60 subtask relations make 10 bands, one for each
        // parent, which has no dependency and so no box: 105 lines among 61 boxes.
        assert_eq!((drawn.edges.len(), drawn.implied.len(), drawn.bands.len()), (105, 69, 10));
        assert_eq!(reached(&drawn.edges), reached(&edges), "every chain is drawn");
        let weigh = |id: &EntityId| weighed[id.to_bytes()[0] as usize].clone();
        let (ids, now) = laid_out(&drawn, weigh, &[], Order::Auto);
        assert_eq!(ids.len(), 61);
        // Every relation drawn as a line, as the page drew them, crosses 2,232 times; the 105 lines
        // drawn now, each parent a band, 718 times, and no two meet a task at one point.
        let every = layout::tests::layout(&weighed, &[], &places(&edges), &[], Order::Auto);
        assert_eq!(layout::tests::crossings_drawn(&every.routes, every.width), 2232);
        assert_eq!(layout::tests::crossings_drawn(&now.routes, now.width), 718);
        assert!(most_at_one_point(&now.routes) < MOST_AT_ONE_POINT_BEFORE);
        assert_eq!(most_at_one_point(&now.routes), 1);
        // Selecting a decision lights it, the kickoff before it and the 48 tasks after it, by 56
        // lines, and draws the 20 dependencies of it that chains imply, threaded across no box and
        // meeting no task where another line does; it moves nothing.
        let focus = focus(&drawn, id(15)).expect("a task the drawing holds");
        let (_, selected) = laid_out(&drawn, weigh, &focus.implied, Order::Auto);
        assert_eq!(selected.boxes, now.boxes);
        let lit: Vec<&Edge> = drawn.edges.iter().zip(&focus.lit).filter(|(_, lit)| **lit).map(|(e, _)| e).collect();
        assert_eq!((focus.tasks.len(), lit.len(), focus.implied.len()), (50, 56, 20));
        assert!(lit.iter().all(|e| focus.tasks.contains(&e.first) && focus.tasks.contains(&e.then)));
        assert!(focus.implied.iter().all(|e| e.first == id(15) || e.then == id(15)));
        let place: HashMap<EntityId, usize> = ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        let pairs = |edges: &[Edge]| edges.iter().map(|e| (place[&e.first], place[&e.then])).collect::<Vec<_>>();
        let threaded = pairs(&focus.implied);
        assert_eq!(layout::tests::edges_across_boxes(&selected.boxes, &selected.threaded, &threaded), 0);
        assert_eq!(layout::tests::ends_shared(&selected, &pairs(&drawn.edges), &threaded), 0);
    }
    #[test]
    fn a_dependency_on_a_task_in_another_scope_is_drawn_to_a_stub_that_brings_no_edges_of_its_own() {
        // Scope 50 holds tasks 1 and 2; scope 60 holds 3 and 4; task 5 is in no scope. Task 2
        // waits for 1 and for 3; 3 waits for 4; 4 waits for 5; 5 waits for 2.
        let tasks = [
            task(1, Some(50), false),
            task(2, Some(50), false),
            task(3, Some(60), false),
            task(4, Some(60), false),
            task(5, None, false),
        ];
        let edges = [waits(2, 1), waits(2, 3), waits(3, 4), waits(4, 5), waits(5, 2)];
        let drawn = plan(&tasks, &edges, ScopeChoice::Scope(id(50)), false);
        assert_eq!(drawn.kept, vec![id(1), id(2)]);
        // 3 (another scope's) and 5 (in none) are stubs; 4 joins no kept task, and the edges
        // among 3, 4 and 5 stay out.
        assert_eq!(drawn.stubs, vec![id(3), id(5)]);
        assert_eq!(drawn.edges, vec![waits(2, 1), waits(2, 3), waits(5, 2)]);
    }

    #[test]
    fn a_stub_or_a_band_label_says_where_its_task_is_worked_and_no_scope_for_none() {
        let named = |scope: EntityId| (scope == id(50)).then(|| "Replication".to_string());
        assert_eq!(scope_words(&[], |_| Some("Replication".into())), "no scope");
        assert_eq!(scope_words(&[id(50)], named), "Replication");
        assert_eq!(scope_words(&[id(60)], |_| None), fmt::short(id(60)), "a scope with no name, by its short id");
        // Each of a task's scopes (docs/DECISIONS.md, D136), in the order given.
        assert_eq!(scope_words(&[id(50), id(60)], named), format!("Replication, {}", fmt::short(id(60))));
    }

    #[test]
    fn no_scope_keeps_the_tasks_in_none_and_stubs_their_neighbours_in_one() {
        let tasks = [task(1, Some(50), false), task(2, None, false), task(3, None, false)];
        let subtask = Edge { first: id(3), then: id(1), kind: RelationKind::SubtaskOf, proposed: true };
        let edges = [waits(2, 1), subtask, waits(3, 2)];
        let drawn = plan(&tasks, &edges, ScopeChoice::NoScope, false);
        assert_eq!(drawn.kept, vec![id(2), id(3)]);
        assert_eq!(drawn.stubs, vec![id(1)], "one stub, however many edges reach it");
        // The proposed subtask relation makes no band; it is named beside its subtask.
        assert_eq!(drawn.edges, vec![waits(2, 1), waits(3, 2)]);
        assert_eq!((drawn.bands, drawn.also.get(&id(3))), (vec![], Some(&vec![subtask])));
    }

    /// A task worked in two scopes (docs/DECISIONS.md, D136) is drawn with either scope chosen, and
    /// is a stub of neither.
    #[test]
    fn a_task_in_two_scopes_is_drawn_with_either() {
        let both = TaskFacts { id: id(9), closed: false, scopes: vec![id(50), id(60)] };
        let tasks = [task(1, Some(50), false), task(3, Some(60), false), both];
        let edges = [waits(9, 1), waits(3, 9)];
        assert_eq!(plan(&tasks, &edges, ScopeChoice::Scope(id(50)), false).kept, vec![id(1), id(9)]);
        assert_eq!(plan(&tasks, &edges, ScopeChoice::Scope(id(60)), false).kept, vec![id(3), id(9)]);
        assert_eq!(plan(&tasks, &edges, ScopeChoice::NoScope, false).kept, Vec::<EntityId>::new());
    }

    #[test]
    fn all_scopes_draws_every_task_and_no_stub() {
        let tasks = [task(1, Some(50), false), task(2, Some(60), false), task(3, None, false)];
        let edges = [waits(2, 1), waits(3, 2)];
        let drawn = plan(&tasks, &edges, ScopeChoice::All, false);
        assert_eq!(drawn, unbanded(&[1, 2, 3], &[], &edges));
    }

    #[test]
    fn an_edge_runs_level_through_a_waypoints_column_and_bends_between_columns() {
        // From a task's right side, across the next column through a waypoint's place, to the left
        // side of a task two columns on.
        let route = [(224.0, 52.0), (304.0, 97.0), (504.0, 97.0), (584.0, 74.5)];
        assert_eq!(
            path(&route).as_deref(),
            Some("M224.0,52.0 C264.0,52.0 264.0,97.0 304.0,97.0 L504.0,97.0 C544.0,97.0 544.0,74.5 584.0,74.5")
        );
        assert_eq!(path(&[(224.0, 52.0)]), None);
        assert_eq!(path(&[]), None);
    }

    #[test]
    fn a_click_in_the_webview_opens_the_task_over_the_list_of_the_scope_the_graph_shows() {
        let task = id(1);
        // Whatever the Graph shows, one scope's tasks, those in no scope or every scope's, the
        // list behind the task keeps the same.
        for choice in [ScopeChoice::Scope(id(50)), ScopeChoice::NoScope, ScopeChoice::All] {
            assert_eq!(
                on_the_tasks_page(task, Some(choice)),
                Route::Tasks { task: Some(task), scope: Some(choice) },
                "{choice:?}"
            );
        }
    }

    #[test]
    fn a_closed_task_is_drawn_stub_or_not_only_with_show_closed() {
        let tasks = [task(1, Some(50), false), task(2, Some(60), true), task(3, Some(50), true)];
        let edges = [waits(1, 2), waits(1, 3)];
        let open = plan(&tasks, &edges, ScopeChoice::Scope(id(50)), false);
        assert_eq!(open, unbanded(&[1], &[], &[]));
        let every = plan(&tasks, &edges, ScopeChoice::Scope(id(50)), true);
        assert_eq!(every, unbanded(&[1, 3], &[2], &edges));
    }
}
