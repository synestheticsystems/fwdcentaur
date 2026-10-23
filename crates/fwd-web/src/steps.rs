//! A task's steps, the checklist its panel shows between its fields and its conversation
//! (docs/DECISIONS.md, 2026-09-28: "A step is an edge from any entity to a task, and the
//! highlight is the step"; "The checked state is the step edge's status"). Each step is a
//! `step_of` edge from anything (a highlight, a message, another task) to the task; the edge's
//! status is the check (`resolved` checked, `accepted` not), and its position, a fractional key
//! compared as text, orders the list, then when it was made, then its id, as `fwd_core::ops::steps`
//! orders them for the CLI. The heading says how many are checked ("3 of 8"), and so does the
//! task's line in the Tasks list. The check box checks and unchecks through
//! `fwd_core::ops::check_step`, which moves a highlight's first thread with its step
//! (docs/DECISIONS.md, D99, "Checking a step resolves its highlight's first thread, and resolving
//! that thread checks the step; each control says it does both"): so the box's title says what it
//! resolves as well as what it checks, and a highlight's line shows its first thread's state beside
//! the box, the two facts side by side, since a thread started after its step was checked is open
//! while the step is checked.
//!
//! Each step is drawn by its own kind's list item (`modes.rs`) beside a check box bound to the
//! edge. A click on the step's line goes where a click on its lozenge would (a highlight to its
//! page, or in the VS Code webview to the editor); its expander shows the step's embedded form in
//! place, the conversation about it beneath. The keys, on a step's line: ArrowUp and ArrowDown
//! move between the steps, Space checks or unchecks, Enter goes where the click goes, ArrowRight
//! and ArrowLeft open and close the expander, Home and End go to the first and last step, Delete
//! opens its removal; within a step opened in place the rows keep their own keys, and Up above its
//! first row or Down past its last comes back to the steps.
//!
//! A step that no longer belongs on the checklist comes off it, which checking it would record as
//! done (task 9dvGW0EI, "Take a step off a checklist and retire its highlight in one command: fwd
//! step remove"). The task's creator and its assignee, who alone may take one off
//! (`fwd_core::rules::can_remove_step`), see a "remove" control at the end of each line, which
//! opens beneath it a box for the reason, kept on the step, and for a highlight with an open
//! thread the choice to retire it as well: resolve its threads, so the editor stops drawing it,
//! and check its steps on other checklists, which its first thread checks. The choice names what
//! it resolves and which tasks' steps it checks. Both go through `fwd_core::ops::remove_step`, as
//! `fwd step remove` does, which tells no one.
//!
//! The order, the check state, the counts, who is offered the removal and what its choice says
//! are worked out as data first and tested natively.

use crate::lozenges::{self, Target};
use crate::referents::Mode;
use crate::session::{self, ctx};
use crate::{anchor_row, keys, modes, store};
use ankurah::EntityId;
use fwd_core::model::{MessageStatus, RelationKind, RelationStatus, RelationView};
use fwd_core::{ops, rules};
use leptos::prelude::*;
use std::collections::{BTreeMap, HashSet};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;

/// A relation as the checklist reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct Edge<I> {
    pub id: I,
    pub kind: RelationKind,
    pub from: I,
    pub to: I,
    pub status: RelationStatus,
    pub position: Option<String>,
    pub created: i64,
}

/// One step of a task.
#[derive(Clone, Debug, PartialEq)]
pub struct Step<I> {
    /// The `step_of` edge, whose status is the check.
    pub edge: I,
    /// What the step is: a highlight, a message, a task.
    pub entity: I,
    pub checked: bool,
}

/// Whether a `step_of` edge in `status` is a step, checked or not: one that holds (accepted, or
/// resolved, which is checked; `rules::holds`), and none for a proposed, retracted or rejected
/// edge.
fn checked(status: RelationStatus) -> Option<bool> {
    rules::holds(RelationKind::StepOf, status).then_some(status == RelationStatus::Resolved)
}

/// The steps of `task` among `edges`, in order: by position (a key compared as text, a step with
/// none, made by a client that gave it none, after every step with one), then oldest first, then
/// by the edge's id, so every client lists them alike.
pub fn checklist<I: Copy + Ord>(task: I, edges: impl IntoIterator<Item = Edge<I>>) -> Vec<Step<I>> {
    let mut found: Vec<(Option<String>, i64, Step<I>)> = edges
        .into_iter()
        .filter(|e| e.kind == RelationKind::StepOf && e.to == task)
        .filter_map(|e| {
            let checked = checked(e.status)?;
            Some((e.position, e.created, Step { edge: e.id, entity: e.from, checked }))
        })
        .collect();
    found.sort_by(|a, b| (a.0.is_none(), &a.0, a.1, a.2.edge).cmp(&(b.0.is_none(), &b.0, b.1, b.2.edge)));
    found.into_iter().map(|(_, _, step)| step).collect()
}

/// How many of `steps` are checked, and how many there are.
pub fn progress<I>(steps: &[Step<I>]) -> (usize, usize) {
    (steps.iter().filter(|s| s.checked).count(), steps.len())
}

/// "3 of 8".
pub fn progress_words((done, total): (usize, usize)) -> String {
    format!("{done} of {total}")
}

/// What pressing a step's box does, as its title says it: it checks an unchecked step and unchecks a
/// checked one, and for a highlight with a thread (`thread`), resolves its first thread or sets it
/// back to active with it (`fwd_core::rules::move_step_and_thread`).
pub fn box_words(checked: bool, thread: bool) -> &'static str {
    match (checked, thread) {
        (false, true) => "check the step and resolve its thread",
        (true, true) => "uncheck the step and unresolve its thread",
        (false, false) => "check the step",
        (true, false) => "uncheck the step",
    }
}

/// The state of a highlight's first thread as its step's line shows it beside the box: "thread
/// open" or "thread resolved"; none for a step with no thread.
pub fn thread_words(status: Option<MessageStatus>) -> Option<&'static str> {
    status.map(|s| if s == MessageStatus::Resolved { "thread resolved" } else { "thread open" })
}

/// Whether the acting principal `me` may take a step off the checklist of a task that `creator`
/// made and `assignee` is assigned, and so sees the "remove" control on its steps: the task's
/// creator and its assignee (`fwd_core::rules::can_remove_step`).
pub fn offers_remove(creator: EntityId, assignee: Option<EntityId>, me: Option<EntityId>) -> bool {
    me.is_some_and(|me| rules::can_remove_step(creator, assignee, me).is_ok())
}

/// What the choice to retire a highlight says it does as its step comes off: it resolves the
/// `threads` still open on the highlight, so the editor stops drawing it while its page keeps them,
/// and checks its steps on the tasks `others`, which its first thread checks, naming them, since
/// each control says what it moves (docs/DECISIONS.md, D99, "Checking a step resolves its
/// highlight's first thread, and resolving that thread checks the step; each control says it does
/// both"). None while no thread on it is open: the editor no longer draws it, and nothing is left
/// to retire.
pub fn retire_words(threads: usize, others: &[String]) -> Option<String> {
    let (resolved, kept) = match threads {
        0 => return None,
        1 => ("its open thread".to_string(), "it"),
        n => (format!("its {n} open threads"), "them"),
    };
    let mut words =
        format!("retire the highlight too: resolve {resolved}, so the editor stops drawing it (its page keeps {kept})");
    if !others.is_empty() {
        words.push_str(&format!(", and check its step on {}", listed(others)));
    }
    Some(words)
}

/// "A", "A and B", "A, B and C".
fn listed(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// What the removal's button says it does: take the step off, and retire the highlight with it
/// when that is chosen, which the choice beneath it spells out.
pub fn remove_words(retire: bool) -> &'static str {
    if retire {
        "Remove and retire"
    } else {
        "Remove the step"
    }
}

/// The tasks on whose checklists resolving `message` checks a step, among `edges` (the relations)
/// and given the first thread on the highlight `message` is on (`first`): each task the highlight
/// is a step of, when `message` is the first message of that thread; none for any other message.
pub fn checked_with<I: Copy + Ord>(
    message: I,
    highlight: Option<I>,
    first: Option<I>,
    edges: impl IntoIterator<Item = Edge<I>>,
) -> Vec<I> {
    let Some(highlight) = highlight.filter(|_| first == Some(message)) else { return Vec::new() };
    let mut tasks: Vec<I> = edges
        .into_iter()
        .filter(|e| e.kind == RelationKind::StepOf && e.from == highlight && checked(e.status).is_some())
        .map(|e| e.to)
        .collect();
    tasks.sort();
    tasks.dedup();
    tasks
}

/// The tasks on whose checklists resolving `message` on the page checks a step ([`checked_with`]),
/// so that its resolve button says so. A message on no highlight checks none, and its button reads
/// nothing more.
pub fn checked_by_resolving(message: EntityId) -> Vec<EntityId> {
    let highlight = store::message(message).and_then(|m| m.anchor().ok().flatten()).map(EntityId::from);
    let Some(highlight) = highlight else { return Vec::new() };
    let first = anchor_row::first_thread(highlight).map(|m| m.id());
    checked_with(message, Some(highlight), first, edges(&store::relations_of(highlight)))
}

/// Every task's progress among `edges`, for the lists: checked of all, by task; a task with no
/// steps is not listed.
pub fn tally<I: Copy + Ord>(edges: impl IntoIterator<Item = Edge<I>>) -> BTreeMap<I, (usize, usize)> {
    let mut by_task: BTreeMap<I, (usize, usize)> = BTreeMap::new();
    for e in edges.into_iter().filter(|e| e.kind == RelationKind::StepOf) {
        let Some(checked) = checked(e.status) else { continue };
        let entry = by_task.entry(e.to).or_insert((0, 0));
        entry.0 += usize::from(checked);
        entry.1 += 1;
    }
    by_task
}

// ---------------------------------------------------------------------------------------------
// Drawing

/// `relations` as the checklist reads them.
fn edges(relations: &[RelationView]) -> Vec<Edge<EntityId>> {
    relations
        .iter()
        .filter_map(|r| {
            Some(Edge {
                id: r.id(),
                kind: r.kind().ok()?,
                from: r.from_id().ok()?,
                to: r.to_id().ok()?,
                status: r.status().ok()?,
                position: r.position().ok().flatten(),
                created: r.created().unwrap_or(0),
            })
        })
        .collect()
}

/// The steps of `task`, from the page's standing relations query.
fn steps_of(task: EntityId) -> Vec<Step<EntityId>> {
    checklist(task, edges(&store::relations_of(task)))
}

/// "3 of 8" for a task with steps; none for one without.
pub fn progress_of(task: EntityId) -> Option<String> {
    let steps = steps_of(task);
    (!steps.is_empty()).then(|| progress_words(progress(&steps)))
}

/// Every task's progress, for the Tasks list.
pub fn progress_by_task() -> BTreeMap<EntityId, (usize, usize)> {
    tally(edges(&store::relations()))
}

fn step_element(edge: EntityId) -> String {
    format!("step-{}", edge.to_base64())
}

/// The element id of the box for the reason a step comes off its checklist.
fn reason_element(edge: EntityId) -> String {
    format!("step-reason-{}", edge.to_base64())
}

fn focus_element(id: &str) {
    let element = web_sys::window().and_then(|w| w.document()).and_then(|d| d.get_element_by_id(id));
    if let Some(element) = element.and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok()) {
        let _ = element.focus();
    }
}

fn focus_step(edge: EntityId) {
    focus_element(&step_element(edge));
}

/// The title of the task whose checklist the step `edge` is on, from the page's relations.
fn task_of_step(edge: EntityId) -> Option<String> {
    let task = store::relation(edge)?.to_id().ok()?;
    store::task(task)?.title().ok()
}

/// Goes where a click on the step's lozenge would.
fn follow(entity: EntityId) {
    lozenges::follow(Target::by_models(entity, &modes::models_of(entity)));
}

/// The Steps section of a task's panel; nothing at all while the task has none.
#[component]
pub fn Steps(task: EntityId) -> impl IntoView {
    let steps = Memo::new(move |_| steps_of(task));
    let expanded = RwSignal::new(HashSet::<EntityId>::new());
    // The step the Tab key reaches: the one last focused, else the first.
    let focused = RwSignal::new(None::<EntityId>);
    let tabbable = Memo::new(move |_| {
        let all = steps.get();
        focused.get().filter(|edge| all.iter().any(|s| s.edge == *edge)).or(all.first().map(|s| s.edge))
    });
    let error = RwSignal::new(None::<String>);
    // Pressing the box checks an unchecked step and unchecks a checked one.
    let press = move |edge: EntityId, was_checked: bool| {
        let Some(me) = session::me() else { return };
        spawn_local(async move {
            match ops::check_step(&ctx(), me, edge, !was_checked).await {
                Ok(_) => {
                    session::wrote(me);
                    error.try_set(None)
                }
                Err(e) => error.try_set(Some(e.to_string())),
            };
        });
    };
    let set_open = move |edge: EntityId, open: bool| {
        expanded.update(|e| {
            if open {
                e.insert(edge);
            } else {
                e.remove(&edge);
            }
        })
    };
    // Whether the acting principal may take a step off this checklist: the task's creator and its
    // assignee, who alone see the "remove" control.
    let may_remove = Memo::new(move |_| {
        let me = session::acting().get().map(|p| p.id());
        store::task(task)
            .and_then(|t| Some(offers_remove(t.created_by().ok()?.into(), ops::assignee_of(&t).ok().flatten(), me)))
            .unwrap_or(false)
    });
    // The step whose removal is open beneath its line, the reason written for it, whether to retire
    // its highlight, and why the last try was refused. Opening, cancelling or making a removal
    // starts the next from nothing, so the form never keeps a choice it no longer shows.
    let removing = RwSignal::new(None::<EntityId>);
    let reason = RwSignal::new(String::new());
    let retire = RwSignal::new(false);
    let refusal = RwSignal::new(None::<String>);
    let open_removal = move |edge: Option<EntityId>| {
        removing.try_set(edge);
        reason.try_set(String::new());
        retire.try_set(false);
        refusal.try_set(None);
        if let Some(edge) = edge {
            request_animation_frame(move || focus_element(&reason_element(edge)));
        }
    };
    let cancel_removal = move |edge: EntityId| {
        open_removal(None);
        focus_step(edge);
    };
    // Takes the step off, retiring its highlight with `retiring`; the step after it, else the one
    // before, takes the focus once it has gone.
    let remove = move |edge: EntityId, retiring: bool| {
        let Some(me) = session::me() else { return };
        let why = reason.get_untracked();
        let all = steps.get_untracked();
        let at = all.iter().position(|s| s.edge == edge);
        let next = at.and_then(|i| all.get(i + 1).or(i.checked_sub(1).and_then(|j| all.get(j)))).map(|s| s.edge);
        spawn_local(async move {
            match ops::remove_step(&ctx(), me, task, edge, &why, retiring).await {
                Ok(_) => {
                    session::wrote(me);
                    open_removal(None);
                    next.into_iter().for_each(focus_step);
                }
                Err(e) => {
                    refusal.try_set(Some(e.to_string()));
                }
            }
        });
    };
    let on_key = move |ev: web_sys::KeyboardEvent| {
        if ev.default_prevented() || !keys::is_plain(&ev) {
            return;
        }
        let Some(target) = ev.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) else { return };
        let Some(item) = target.closest("li.step").ok().flatten() else { return };
        let Some(edge) = item.get_attribute("data-edge").and_then(|e| e.parse::<EntityId>().ok()) else { return };
        let all = steps.get_untracked();
        let Some(at) = all.iter().position(|s| s.edge == edge) else { return };
        let step = &all[at];
        let neighbour = |delta: isize| at.checked_add_signed(delta).and_then(|i| all.get(i)).map(|s| s.edge);
        // Within a step opened in place the rows keep their keys; Up above them, or Down past
        // them, comes back to the steps.
        if !target.is_same_node(Some(item.as_ref())) {
            let to = match ev.key().as_str() {
                "ArrowUp" if !keys::is_text_field(&target) => Some(edge),
                "ArrowDown" if !keys::is_text_field(&target) => neighbour(1),
                _ => None,
            };
            if let Some(to) = to {
                ev.prevent_default();
                focus_step(to);
            }
            return;
        }
        match ev.key().as_str() {
            "ArrowUp" => neighbour(-1).into_iter().for_each(focus_step),
            "ArrowDown" => neighbour(1).into_iter().for_each(focus_step),
            "Home" => all.first().into_iter().for_each(|s| focus_step(s.edge)),
            "End" => all.last().into_iter().for_each(|s| focus_step(s.edge)),
            " " => press(edge, step.checked),
            "Enter" => follow(step.entity),
            "ArrowRight" => set_open(edge, true),
            "ArrowLeft" => set_open(edge, false),
            "Delete" if may_remove.get_untracked() => open_removal(Some(edge)),
            _ => return,
        }
        ev.prevent_default();
    };
    let item = move |step: Step<EntityId>| {
        let (edge, entity) = (step.edge, step.entity);
        let checked = Memo::new(move |_| steps.with(|all| all.iter().any(|s| s.edge == edge && s.checked)));
        // The first thread on a highlight, which the box moves with the step; none for any other step.
        let thread = Memo::new(move |_| anchor_row::first_thread(entity).and_then(|m| m.status().ok()));
        let pressing = move || box_words(checked.get(), thread.get().is_some());
        let models = Memo::new(move |_| modes::models_of(entity));
        let open = Memo::new(move |_| expanded.with(|e| e.contains(&edge)));
        view! {
            <li class="step" class:checked=move || checked.get() id=step_element(edge) data-edge=edge.to_base64()
                tabindex=move || if tabbable.get() == Some(edge) { "0" } else { "-1" }
                on:focus=move |_| focused.set(Some(edge))>
                <div class="step-line">
                    <button class="caret" tabindex="-1" title="show it here, with the conversation about it"
                        aria-label="show it here" aria-expanded=move || open.get().to_string()
                        on:click=move |_| set_open(edge, !open.get_untracked())>
                        {move || if open.get() { "▾" } else { "▸" }}
                    </button>
                    <input type="checkbox" tabindex="-1" title=pressing
                        aria-label=move || format!("{} checked", modes::kind(&models.get()).map_or("step", |k| k.name))
                        prop:checked=move || checked.get()
                        on:change=move |ev| {
                            // The box shows the edge's status: it changes once the edge does.
                            if let Some(input) = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) {
                                input.set_checked(checked.get_untracked());
                            }
                            press(edge, checked.get_untracked());
                        }/>
                    <span class="step-body"
                        on:click=move |ev| {
                            // A link or a button inside the line does its own thing.
                            let inner = ev
                                .target()
                                .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                                .and_then(|t| t.closest("a, button").ok().flatten());
                            if inner.is_none() {
                                follow(entity);
                            }
                        }>
                        {move || modes::draw(Mode::Full, Mode::ListItem, entity, &models.get())}
                    </span>
                    {move || thread_words(thread.get()).map(|words| view! {
                        <span class="pill thread-state" class:resolved=move || thread.get() == Some(MessageStatus::Resolved)
                            title="the state of the first thread on the highlight, which the box resolves and unresolves with the step">
                            {words}
                        </span>
                    })}
                    {move || may_remove.get().then(|| view! {
                        <button class="btn subtle tiny step-remove-open" tabindex="-1"
                            title="take the step off the checklist, saying why; what it is stays (Delete)"
                            aria-expanded=move || (removing.get() == Some(edge)).to_string()
                            on:click=move |_| open_removal((removing.get_untracked() != Some(edge)).then_some(edge))>
                            "remove"
                        </button>
                    })}
                </div>
                {move || (removing.get() == Some(edge)).then(|| {
                    // What retiring the highlight would move, worked out again as its threads and
                    // steps change; none for a step that is no highlight, or one with no open thread.
                    let offer = Memo::new(move |_| {
                        store::anchor(entity)?;
                        let retired = anchor_row::retiring(entity, edge);
                        let others: Vec<String> = retired.steps.iter().filter_map(|(id, _)| task_of_step(*id)).collect();
                        retire_words(retired.messages.len(), &others)
                    });
                    // Retiring is chosen while the choice is offered and ticked.
                    let retiring = move || retire.get() && offer.with(Option::is_some);
                    let apply = move || remove(edge, retire.get_untracked() && offer.with_untracked(Option::is_some));
                    view! {
                        <div class="step-remove">
                            <div class="note-box">
                                <input type="text" id=reason_element(edge)
                                    placeholder="why it comes off the checklist (kept on the step)"
                                    prop:value=move || reason.get()
                                    on:input=move |ev| {
                                        reason.set(event_target_value(&ev));
                                        refusal.set(None);
                                    }
                                    on:keydown=move |ev| {
                                        if keys::is_send(&ev) {
                                            ev.prevent_default();
                                            apply();
                                        } else if ev.key() == "Escape" {
                                            ev.prevent_default();
                                            cancel_removal(edge);
                                        }
                                    }/>
                                <button class="btn primary" on:click=move |_| apply()>{move || remove_words(retiring())}</button>
                                <button class="btn subtle" on:click=move |_| cancel_removal(edge)>"Cancel"</button>
                            </div>
                            {move || offer.get().map(|words| view! {
                                <label class="retire">
                                    <input type="checkbox" prop:checked=move || retire.get()
                                        on:change=move |ev| retire.set(event_target_checked(&ev))/>
                                    {words}
                                </label>
                            })}
                            {move || refusal.get().map(|e| view! { <p class="error">{e}</p> })}
                        </div>
                    }
                })}
                {move || {
                    open.get().then(|| {
                        let embedded = modes::draw_beneath(Mode::Full, Mode::Embedded, entity, &models.get(), Some(task));
                        view! { <div class="step-embedded">{embedded}</div> }
                    })
                }}
            </li>
        }
    };
    view! {
        <section class="prose steps" class:empty=move || steps.with(Vec::is_empty)>
            <h4>"Steps " <span class="step-progress">{move || progress_words(steps.with(|s| progress(s)))}</span></h4>
            {move || error.get().map(|e| view! { <p class="error">{e}</p> })}
            <ul class="step-list" on:keydown=on_key>
                <For each=move || steps.get() key=|step| step.edge children=item/>
            </ul>
        </section>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use RelationStatus::*;

    const TASK: u32 = 100;

    fn edge(id: u32, from: u32, status: RelationStatus, position: Option<&str>, created: i64) -> Edge<u32> {
        Edge { id, kind: RelationKind::StepOf, from, to: TASK, status, position: position.map(str::to_string), created }
    }

    fn entities(steps: &[Step<u32>]) -> Vec<u32> {
        steps.iter().map(|s| s.entity).collect()
    }

    #[test]
    fn steps_list_by_position_then_age_then_edge() {
        let edges = vec![
            edge(1, 11, Accepted, Some("m"), 50),
            edge(2, 12, Accepted, Some("b"), 90),
            edge(3, 13, Resolved, Some("bm"), 10),
            edge(4, 14, Accepted, None, 70),
            edge(6, 16, Accepted, None, 30),
            edge(5, 15, Accepted, None, 30),
        ];
        // The keys as text ("b" < "bm" < "m"), then the steps with no position, oldest first and
        // by edge between equal times, as `fwd step list` lists them.
        assert_eq!(entities(&checklist(TASK, edges)), vec![12, 13, 11, 15, 16, 14]);
    }

    #[test]
    fn the_check_is_the_edges_status_and_only_standing_step_edges_count() {
        let mut edges = vec![
            edge(1, 11, Accepted, Some("a"), 1),
            edge(2, 12, Resolved, Some("b"), 2),
            edge(3, 13, Proposed, Some("c"), 3),
            edge(4, 14, Retracted, Some("d"), 4),
            edge(5, 15, Rejected, Some("e"), 5),
        ];
        // A step of another task, and a relation of another kind to this one.
        edges.push(Edge { to: 200, ..edge(6, 16, Accepted, Some("a"), 6) });
        edges.push(Edge { kind: RelationKind::SubtaskOf, ..edge(7, 17, Accepted, Some("a"), 7) });
        let steps = checklist(TASK, edges.clone());
        assert_eq!(steps.iter().map(|s| (s.entity, s.checked)).collect::<Vec<_>>(), vec![(11, false), (12, true)]);
        assert_eq!(progress(&steps), (1, 2));
        assert_eq!(progress_words(progress(&steps)), "1 of 2");
        let tallied = tally(edges);
        assert_eq!(tallied.get(&TASK), Some(&(1, 2)));
        assert_eq!(tallied.get(&200), Some(&(0, 1)));
        assert_eq!(tallied.len(), 2, "a task with no steps is not listed");
    }

    #[test]
    fn the_box_says_it_moves_a_highlights_thread_and_the_line_shows_the_threads_state() {
        assert_eq!(box_words(false, true), "check the step and resolve its thread");
        assert_eq!(box_words(true, true), "uncheck the step and unresolve its thread");
        assert_eq!(box_words(false, false), "check the step", "a step with no thread checks alone");
        assert_eq!(box_words(true, false), "uncheck the step");
        assert_eq!(thread_words(Some(MessageStatus::Resolved)), Some("thread resolved"));
        assert_eq!(thread_words(Some(MessageStatus::Active)), Some("thread open"));
        assert_eq!(thread_words(None), None, "no thread, no word");
    }

    #[test]
    fn resolving_a_highlights_first_thread_checks_each_step_it_is_and_nothing_else_does() {
        const HIGHLIGHT: u32 = 11;
        const FIRST: u32 = 50;
        let edges = vec![
            edge(1, HIGHLIGHT, Accepted, Some("a"), 1),
            Edge { to: 200, ..edge(2, HIGHLIGHT, Resolved, Some("a"), 2) },
            Edge { to: 300, ..edge(3, HIGHLIGHT, Retracted, Some("a"), 3) },
            edge(4, 12, Accepted, Some("b"), 4),
            Edge { kind: RelationKind::RelatesTo, to: 400, ..edge(5, HIGHLIGHT, Accepted, None, 5) },
        ];
        assert_eq!(checked_with(FIRST, Some(HIGHLIGHT), Some(FIRST), edges.clone()), vec![TASK, 200]);
        assert_eq!(checked_with(51, Some(HIGHLIGHT), Some(FIRST), edges.clone()), Vec::<u32>::new(), "a later thread");
        assert_eq!(checked_with(FIRST, None, None, edges), Vec::<u32>::new(), "a message on no highlight");
    }

    #[test]
    fn only_the_tasks_creator_and_its_assignee_are_offered_the_removal() {
        let (creator, assignee, other) = (EntityId::random(), EntityId::random(), EntityId::random());
        assert!(offers_remove(creator, Some(assignee), Some(creator)));
        assert!(offers_remove(creator, Some(assignee), Some(assignee)));
        assert!(offers_remove(creator, None, Some(creator)), "a task no one is assigned: its creator");
        assert!(!offers_remove(creator, Some(assignee), Some(other)));
        assert!(!offers_remove(creator, None, Some(other)));
        assert!(!offers_remove(creator, Some(assignee), None), "no one acting yet");
    }

    #[test]
    fn the_retire_choice_says_what_it_resolves_and_which_steps_it_checks() {
        assert_eq!(retire_words(0, &[]), None, "no open thread: the editor draws it no more, nothing to retire");
        assert_eq!(retire_words(0, &["Review: the backoff".into()]), None);
        assert_eq!(
            retire_words(1, &[]).as_deref(),
            Some(
                "retire the highlight too: resolve its open thread, so the editor stops drawing it (its page keeps it)"
            )
        );
        assert_eq!(
            retire_words(3, &["Review: the backoff".into()]).as_deref(),
            Some(
                "retire the highlight too: resolve its 3 open threads, so the editor stops drawing it (its page keeps them), and check its step on Review: the backoff"
            )
        );
        let three = ["A".to_string(), "B".to_string(), "C".to_string()];
        assert!(retire_words(2, &three).unwrap().ends_with("check its step on A, B and C"));
        assert!(retire_words(2, &three[..2]).unwrap().ends_with("check its step on A and B"));
        assert_eq!(remove_words(false), "Remove the step");
        assert_eq!(remove_words(true), "Remove and retire");
    }

    #[test]
    fn a_step_the_box_checks_reads_checked_and_one_it_unchecks_reads_unchecked() {
        // Pressing the box asks `ops::check_step` to check an unchecked step (`!was_checked`),
        // whose rule moves the edge; the list reads the edge's new status as the box's state.
        let checked_now = rules::check_step(RelationKind::StepOf, Accepted, true).unwrap().unwrap();
        assert_eq!(checked(checked_now), Some(true));
        let unchecked_now = rules::check_step(RelationKind::StepOf, Resolved, false).unwrap().unwrap();
        assert_eq!(checked(unchecked_now), Some(false));
    }
}
