//! The task list with its filters (a title, a label, the tasks assigned to the acting principal,
//! ready only, show closed, and a scope the tasks are worked in, which the header's scope filter
//! chooses for the Questions, Tasks, Decisions and Graph pages at once: `scope_filter.rs`), each
//! task with its labels after its title, its scopes, its assignee and, when it has steps, how many
//! are checked; and the task's panel, its full mode: its short id (a click copies the whole id),
//! where it is worked and who is assigned (one control: `assignee.rs`), its labels (a box adds them
//! and each pill's button takes one off: `labels.rs`), fields, readiness with its reasons, its
//! steps (`steps.rs`), the lozenges of everything joined to it by a relation under "Relations"
//! (`lozenges.rs`: the rulings it cites, what it depends on and what waits for it, its subtasks,
//! where it came from, the messages that mention it, its code), the actions the transition table
//! allows the acting principal right now (and Start where the start would be let through, which
//! names a scope only for a task worked in none: `claim.rs`), and then the task's thread, the same
//! rows a task row shows beneath itself (`task_row.rs`: its comments and the messages about it),
//! with a comment box beneath it.
//! The "New task" button puts the form for a new task in the panel instead (`new_task.rs`).
//! Actions run the same `fwd_core::ops` the CLI runs.

use crate::comment_box::{CommentBox, Subject};
use crate::empty::{Empty, Nothing};
use crate::fmt::CopyId;
use crate::lozenges::Facet;
use crate::panel::{FullScreenToggle, PanelEdge, PanelSize};
use crate::referents::Models;
use crate::route::{self, Route};
use crate::scope_filter::ScopeChoice;
use crate::session::{self, ctx};
use crate::steps::{self, Steps};
use crate::thread::{self, Hooks, Model, ThreadState};
use crate::{
    assignee, back, claim, fmt, keys, labels, lozenges, modes, new_task, page_state, references, scope_row, store,
    task_scopes,
};
use ankurah::EntityId;
use fwd_core::model::*;
use fwd_core::ops;
use fwd_core::rules::{self, NotReadyReason, Readiness, UnknownReason, Verb};
use leptos::prelude::*;
use serde_json::Value;
use wasm_bindgen_futures::spawn_local;

/// The localStorage key under which this browser remembers how wide the task panel was dragged.
const PANEL_WIDTH: &str = "fwd.web.task-panel-width";

/// The Tasks page: the list, and beside it (beneath it on a narrow page) the panel of the task
/// the route names, or the form for a new task when `new_task` says the route asks for it. At
/// every width a button lets the panel fill the page, and beside the list dragging the panel's
/// left edge sets its width (`panel.rs`); the route stays what it was. In the VS Code webview a
/// task's panel opens filling the page, with a Back control that returns to where the task was
/// opened from (`back.rs`). `scope` is the scope the list keeps the tasks of, the page's choice
/// (`scope_filter::follow`): none until the page has picked the one it opens on.
#[component]
pub fn TasksPage(
    selected: Memo<Option<EntityId>>,
    scope: Memo<Option<ScopeChoice>>,
    new_task: Memo<bool>,
) -> impl IntoView {
    let size = PanelSize::new(PANEL_WIDTH);
    let back = session::locked().then(|| back::follow(size, selected, scope));
    // What was written for a new task, kept while the panel shows a task and back.
    let draft = new_task::Draft::empty();
    let open = move || new_task.get() || selected.with(Option::is_some);
    // With no task open the list comes back, so a task chosen from it opens with the list in view
    // (in the VS Code webview it opens filling the page all the same: `back.rs`).
    Effect::new(move |_| {
        if !open() {
            size.full.set(false);
        }
    });
    view! {
        <div class="split resizable" class:with-detail=open class:full-screen=move || open() && size.full.get()
            class:dragging=move || size.dragging.get() style=("--panel-width", move || size.width_property())>
            <section class="page list-pane">
                <TaskList selected=selected scope=scope/>
            </section>
            {move || {
                let toggle = move || view! { <FullScreenToggle size=size/> }.into_any();
                let (controls, shown) = if new_task.get() {
                    (toggle(), view! { <new_task::NewTask draft=draft scope=scope/> }.into_any())
                } else {
                    let task = selected.get()?;
                    let controls = match back {
                        // Back at the start of the panel's first line, the toggle at its end.
                        Some(back) => {
                            view! { <div class="panel-bar"><back::BackButton back=back/>{toggle()}</div> }.into_any()
                        }
                        None => toggle(),
                    };
                    (controls, modes::full(task, &Models::one(Model::Task)))
                };
                Some(view! {
                    <PanelEdge size=size/>
                    <aside class="detail-pane">
                        {controls}
                        {shown}
                    </aside>
                })
            }}
        </div>
    }
}

struct Row {
    id: EntityId,
    title: String,
    /// Its labels, as stored, shown after its title.
    labels: Vec<String>,
    status: TaskStatus,
    ready: Readiness,
    priority: Option<i64>,
    /// The scopes it is worked in: their names, in the order it came to be worked in them, commas
    /// between them.
    scope: String,
    /// Who is assigned: a handle.
    assignee: String,
    /// "3/8" when it has steps: how many are checked, of how many.
    steps: Option<(usize, usize)>,
    messages: usize,
    updated: i64,
}

#[component]
fn TaskList(selected: Memo<Option<EntityId>>, scope: Memo<Option<ScopeChoice>>) -> impl IntoView {
    // The filters as the list was left, when Back in the VS Code webview is bringing the page back
    // from another page; else as a visit starts (`page_state.rs`). The list keeps them there as
    // they change.
    let kept = page_state::open_task_list();
    let show_closed = RwSignal::new(kept.show_closed);
    let only_ready = RwSignal::new(kept.ready_only);
    // The tasks whose assignee is the acting principal (the CLI's `--mine`).
    let mine = RwSignal::new(kept.mine);
    let search = RwSignal::new(kept.title);
    // The label the list keeps the tasks of; none keeps every task.
    let label = RwSignal::new(kept.label);
    Effect::new(move |_| {
        page_state::keep_task_list(page_state::TaskListState {
            title: search.get(),
            label: label.get(),
            mine: mine.get(),
            ready_only: only_ready.get(),
            show_closed: show_closed.get(),
        });
    });
    // None while the page picks the scope it opens on.
    let rows = move || {
        let wanted_scope = scope.get()?;
        let tasks = store::tasks();
        let relations = store::relations();
        let messages = store::messages();
        let progress = steps::progress_by_task();
        let needle = search.get().to_lowercase();
        let wanted_label = label.get();
        let me = mine.get().then(|| session::acting().get().map(|p| p.id()));
        let mut rows: Vec<Row> = tasks
            .iter()
            .filter_map(|t| {
                let status = t.status().ok()?;
                if !show_closed.get() && fmt::is_closed(status) {
                    return None;
                }
                let title = t.title().unwrap_or_default();
                if !needle.is_empty() && !title.to_lowercase().contains(&needle) {
                    return None;
                }
                let task_labels = labels::of(t);
                if !labels::keeps(wanted_label.as_deref(), &task_labels) {
                    return None;
                }
                // Through the index of relations by their ends, rather than every relation per task.
                let scopes = store::scopes_of_task(t.id());
                if !wanted_scope.keeps(&scopes) {
                    return None;
                }
                let assignee = ops::assignee_of(t).ok().flatten();
                if me.is_some_and(|me| me.is_none() || assignee != me) {
                    return None;
                }
                let ready = store::readiness_of(t, &tasks, &relations);
                if only_ready.get() && !ready.is_ready() {
                    return None;
                }
                let scope = scopes.into_iter().map(store::scope_label).collect::<Vec<_>>().join(", ");
                Some(Row {
                    id: t.id(),
                    title,
                    labels: task_labels,
                    status,
                    ready,
                    priority: t.priority().unwrap_or(None),
                    scope,
                    assignee: assignee.map(store::handle_of).unwrap_or_default(),
                    steps: progress.get(&t.id()).copied(),
                    messages: store::message_count(t.id(), &messages),
                    updated: t.updated().unwrap_or(0),
                })
            })
            .collect();
        rows.sort_by(|a, b| {
            fmt::status_rank(a.status)
                .cmp(&fmt::status_rank(b.status))
                .then(a.priority.unwrap_or(9).cmp(&b.priority.unwrap_or(9)))
                .then(b.updated.cmp(&a.updated))
        });
        Some(rows)
    };
    // The list, or while it lists no task, why: loading, no tasks at all, or none the filters keep.
    let list = move || {
        let Some(rows) = rows() else {
            return view! { <Empty nothing=Nothing::Yet/> }.into_any();
        };
        if rows.is_empty() {
            let nothing = Nothing::Tasks { filtered_out: !store::tasks().is_empty() };
            return view! { <Empty nothing=nothing/> }.into_any();
        }
        view! {
            <table class="list tasks">
                <thead>
                    <tr>
                        <th>"Task"</th><th>"Status"</th><th>"Ready"</th><th>"Pri"</th><th>"Scope"</th><th>"Assignee"</th>
                        <th>"Msgs"</th><th>"Updated"</th>
                    </tr>
                </thead>
                <tbody>{rows.into_iter().map(|r| task_line(r, selected, scope)).collect::<Vec<_>>()}</tbody>
            </table>
        }
        .into_any()
    };
    view! {
        <header class="page-head">
            <h2>"Tasks"</h2>
            <button class="btn primary" title="Write a task with every field fwd task create takes"
                on:click=move |_| route::go(Route::NewTask { scope: scope.get_untracked() })>"New task"</button>
            <input class="search" type="search" placeholder="filter by title" prop:value=move || search.get()
                on:input=move |ev| search.set(event_target_value(&ev))/>
            <labels::LabelFilter chosen=label/>
            <label class="toggle" title="only the tasks assigned to you">
                <input type="checkbox" prop:checked=move || mine.get()
                    on:change=move |ev| mine.set(event_target_checked(&ev))/>
                " mine"
            </label>
            <label class="toggle">
                <input type="checkbox" prop:checked=move || only_ready.get()
                    on:change=move |ev| only_ready.set(event_target_checked(&ev))/>
                " ready only"
            </label>
            <label class="toggle">
                <input type="checkbox" prop:checked=move || show_closed.get()
                    on:change=move |ev| show_closed.set(event_target_checked(&ev))/>
                " show closed"
            </label>
        </header>
        // The scope the list is filtered to: its line, what names it and a link to its panel.
        {move || {
            scope.get().and_then(ScopeChoice::scope).map(|s| view! { <div class="scope-head">{scope_row::heading(s)}</div> })
        }}
        {list}
    }
}

/// One task's line; a click on it opens the task's panel, the list keeping its scope filter.
fn task_line(r: Row, selected: Memo<Option<EntityId>>, scope: Memo<Option<ScopeChoice>>) -> impl IntoView {
    let id = r.id;
    let steps = r.steps.map(|(done, total)| {
        let words = steps::progress_words((done, total));
        view! { " " <span class="step-progress" title=format!("{words} steps checked")>{format!("{done}/{total}")}</span> }
    });
    view! {
        <tr class="row" class:selected=move || selected.get() == Some(id)
            on:click=move |_| route::go(Route::tasks(Some(id), scope.get_untracked()))>
            // Each cell names its field, which a narrow page shows beside it once the rows stack
            // (app.css).
            <td class="title">{r.title}{labels::pills(r.labels)}{steps}</td>
            <td data-label="status"><span class=format!("pill status-{}", r.status)>{fmt::status_label(r.status)}</span></td>
            <td data-label="ready">{readiness_mark(&r.ready)}</td>
            <td data-label="priority">{fmt::priority_label(r.priority)}</td>
            <td data-label="scope">{r.scope}</td>
            <td data-label="assignee">{r.assignee}</td>
            <td data-label="messages">{(r.messages > 0).then(|| r.messages.to_string())}</td>
            <td data-label="updated" class="muted">{fmt::ago(r.updated)}</td>
        </tr>
    }
}

fn readiness_mark(r: &Readiness) -> impl IntoView {
    let (class, text) = match r {
        Readiness::Ready => ("ready", "ready"),
        Readiness::NotReady(_) => ("not-ready", "no"),
        Readiness::Unknown(_) => ("unknown", "?"),
    };
    view! { <span class=format!("ready {class}")>{text}</span> }
}

/// The status verbs the page offers as plain buttons. `Start` is not one: its button shows only
/// where the start would be let through, and for a task worked in no scope it names the scope the
/// task starts in, or a picker after it does while several are active (`claim.rs`).
const VERBS: [Verb; 9] = [
    Verb::Approve,
    Verb::Reject,
    Verb::Submit,
    Verb::Accept,
    Verb::Return,
    Verb::Done,
    Verb::Cancel,
    Verb::Reopen,
    Verb::Rejustify,
];

fn verb_label(v: Verb) -> &'static str {
    match v {
        Verb::Approve => "Approve",
        Verb::Reject => "Reject",
        Verb::Start => "Start",
        Verb::Submit => "Submit for review",
        Verb::Accept => "Accept",
        Verb::Return => "Return",
        Verb::Done => "Mark done",
        Verb::Cancel => "Cancel",
        Verb::Reopen => "Reopen",
        Verb::Rejustify => "Rejustify",
    }
}

/// What the verb's note is called when it asks for one; `None` runs the verb at once.
fn note_prompt(v: Verb) -> Option<&'static str> {
    match v {
        Verb::Submit => Some("evidence (commits, test runs)"),
        Verb::Return => Some("what should change"),
        Verb::Reject | Verb::Cancel | Verb::Reopen => Some("why"),
        Verb::Rejustify => Some("the new reason"),
        _ => None,
    }
}

#[component]
pub fn TaskDetail(id: EntityId) -> impl IntoView {
    let error = RwSignal::new(None::<String>);
    let note = RwSignal::new(String::new());
    let pending = RwSignal::new(None::<Verb>);
    // The scope chosen in Start's picker, kept while the panel redraws.
    let chosen_scope = RwSignal::new(None::<EntityId>);

    let act = move |verb: Verb, note_text: Option<String>| {
        let Some(me) = session::me() else { return };
        spawn_local(async move {
            match ops::transition(&ctx(), me, id, verb, note_text.as_deref()).await {
                Ok(_) => {
                    session::wrote(me);
                    error.set(None);
                    pending.set(None);
                    note.set(String::new());
                }
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };
    // A verb that needs a note runs once the note is given: from its button, or Enter in the box.
    let act_with_note = move |v: Verb| {
        let n = note.get_untracked();
        if n.trim().is_empty() {
            error.set(Some(format!("{} needs {}", verb_label(v), note_prompt(v).unwrap_or("a note"))));
        } else {
            act(v, Some(n));
        }
    };

    // The task's title, id, status and who made it, redrawn whenever the task changes.
    let head = move || {
        let Some(t) = store::task(id) else {
            return view! { <Empty nothing=Nothing::Task/> }.into_any();
        };
        let status = t.status().unwrap_or(TaskStatus::Proposed);
        let hold = t.hold().unwrap_or(None);
        let created_by = t.created_by().ok().map(|p| store::handle_of(EntityId::from(p))).unwrap_or_default();
        view! {
            <header class="detail-head">
                <h3>{t.title().unwrap_or_default()}</h3>
                <div class="pills">
                    <CopyId id=id/>
                    <span class=format!("pill status-{status}")>{fmt::status_label(status)}</span>
                    {t.priority().ok().flatten().map(|p| view! { <span class="pill">{format!("P{p}")}</span> })}
                    {t.needs_review().unwrap_or(false).then(|| view! { <span class="pill">"review required"</span> })}
                    {hold.map(|h| view! { <span class="pill hold" title=h>"on hold"</span> })}
                </div>
                <div class="muted">"created by " {created_by} " " {fmt::ago(t.created().unwrap_or(0))}</div>
            </header>
        }
        .into_any()
    };

    // The rest above the steps, redrawn whenever the task or what it depends on changes.
    let summary = move || {
        let Some(t) = store::task(id) else {
            return ().into_any();
        };
        let me = session::acting().get().map(|p| p.id());
        let tasks = store::tasks();
        let relations = store::relations();
        let status = t.status().unwrap_or(TaskStatus::Proposed);
        let needs_review = t.needs_review().unwrap_or(false);
        let context: Value = t.context().map(Into::into).unwrap_or(Value::Null);
        let submitted_by = context["submitted_by"].as_str().and_then(|s| s.parse::<EntityId>().ok());
        let verbs: Vec<Verb> = VERBS
            .iter()
            .copied()
            .filter(|v| rules::transition(status, *v, needs_review).is_ok())
            // The submitter never accepts their own work (rules::can_accept).
            .filter(|v| *v != Verb::Accept || submitted_by.is_none() || submitted_by != me)
            .collect();
        let readiness = store::readiness_of(&t, &tasks, &relations);
        let what = ops::text(t.what()).unwrap_or_default();
        let why = ops::text(t.why()).unwrap_or_default();
        let done_when = ops::text(t.done_when()).unwrap_or_default();
        let evidence = context["evidence"].as_str().unwrap_or("").to_string();
        let hold = t.hold().unwrap_or(None);
        let closed_reason = t.closed_reason().unwrap_or(None).filter(|note| !note.trim().is_empty());

        let prose = |label: &'static str, text: String| {
            (!text.is_empty()).then(|| view! { <section class="prose"><h4>{label}</h4><p>{text}</p></section> })
        };

        let action_buttons = verbs
            .iter()
            .copied()
            .map(|v| {
                let label = verb_label(v);
                let onclick = move |_| match note_prompt(v) {
                    Some(_) => pending.set(Some(v)),
                    None => act(v, None),
                };
                let class = match v {
                    Verb::Approve | Verb::Accept | Verb::Start | Verb::Submit | Verb::Done => "btn primary",
                    _ => "btn",
                };
                view! { <button class=class on:click=onclick>{label}</button> }
            })
            .collect::<Vec<_>>();

        // Start starts the task, as `fwd task start` does, where that start would be let through:
        // an open task assigned to the acting principal or to nobody (`claim::shows`); in the
        // task's own scope, or for a task worked in none in an active scope (`claim.rs`). It shows
        // once the daemon has answered the standing queries: each first answers from this load's
        // own browser storage, which starts empty, so until then a task worked in a scope looks
        // worked in none, and Start would offer the active scopes for it.
        let assignee = ops::assignee_of(&t).ok().flatten();
        let start_view = (store::standing_loaded() && claim::shows(status, assignee, me))
            .then(|| claim::control(id, claim::offer(id), chosen_scope, error));

        view! {
            <div class="actions">
                {start_view}
                {action_buttons}
            </div>
            {move || pending.get().map(|v| view! {
                <div class="note-box">
                    <input type="text" placeholder=note_prompt(v).unwrap_or("") prop:value=move || note.get()
                        on:input=move |ev| note.set(event_target_value(&ev))
                        on:keydown=move |ev| {
                            if keys::is_send(&ev) {
                                ev.prevent_default();
                                act_with_note(v);
                            }
                        }/>
                    <button class="btn primary" on:click=move |_| act_with_note(v)>{verb_label(v)}</button>
                    <button class="btn subtle" on:click=move |_| pending.set(None)>"Cancel"</button>
                </div>
            })}
            {move || error.get().map(|e| view! { <p class="error">{e}</p> })}
            <section class="prose">
                <h4>"Readiness"</h4>
                {readiness_view(&readiness, &tasks)}
            </section>
            {prose("What", what)}
            {prose("Why", why)}
            {prose("Done when", done_when)}
            {prose("Evidence", evidence)}
            {hold.map(|h| prose("On hold", h))}
            // So that a closing note reads as the names of what it names: the note the task was
            // closed with, each whole id or handle in it drawn as the lozenge of what it names, as
            // the decision panel draws why a ruling closed (`decision_row::closing_section`; task
            // aHAMvPU1, "Draw names wherever a closing note or a closed reason still prints raw").
            // A task's close keeps its note as typed, unresolved, so the start of an id stays text.
            {closed_reason.map(|note| view! {
                <section class="prose"><h4>"Closed because"</h4><p>{references::content(&note)}</p></section>
            })}
        }
        .into_any()
    };

    view! {
        <div class="detail">
            {head}
            // Outside the closures that redraw with every change to the task, so the picker and
            // the labels' box keep their state, the steps their focus and the groups opened here
            // stay open.
            <Assignment task=id/>
            <labels::TaskLabels task=id/>
            {summary}
            <Steps task=id/>
            // The scope shows beside the assignee above, so its lozenge is left out here.
            {lozenges::section(id, "Relations", &[Facet::AssignedTo])}
            <section class="prose">
                <h4>"Thread"</h4>
                // A fresh thread per acting principal: replies and resolutions are theirs.
                {move || session::acting().get().map(|p| view! { <TaskThread id=id me=p.id()/> })}
                <CommentBox about=Subject::Task(id)/>
            </section>
        </div>
    }
}

/// The full mode of a task: its panel.
pub fn full(id: EntityId) -> AnyView {
    view! { <TaskDetail id=id/> }.into_any()
}

/// Where the task is worked and who is assigned, side by side (docs/DECISIONS.md, "Assignment is
/// an optional field on the task, never an edge; the scope edge means where the work happens",
/// 2026-09-28): the scopes it is worked in, any number since D136 ("A task may be worked in several
/// scopes"), as tags a click takes off, with "+ scope" to add one (`task_scopes.rs`); then the
/// assignee, as one control that unassigns the task or assigns it (`assignee.rs`).
#[component]
fn Assignment(task: EntityId) -> impl IntoView {
    // Nothing to say about a task that has not arrived yet.
    let loaded = Memo::new(move |_| store::task(task).is_some());
    view! {
        <div class="assignment" hidden=move || !loaded.get()>
            <task_scopes::TaskScopes task=task/>
            <assignee::TaskAssignee task=task/>
        </div>
    }
}

/// The rows beneath the task, as the inbox shows them beneath a task's row: its comments with
/// their replies and the messages about it, worked from the keyboard the same way. No row restates
/// the task, whose short id, title, labels and status the panel shows at its top (Daniel,
/// 2026-09-30: "the Thread section of a task should not restate the name/id/status/tags of the
/// task. that is redundant."), and whose lozenges show under "Relations".
#[component]
fn TaskThread(id: EntityId, me: EntityId) -> impl IntoView {
    thread::rows_beneath(ThreadState::new(me), id, id, Models::one(Model::Task), Hooks::default())
}

fn readiness_view(r: &Readiness, tasks: &[TaskView]) -> AnyView {
    let title = |id: EntityId| {
        tasks.iter().find(|t| t.id() == id).and_then(|t| t.title().ok()).unwrap_or_else(|| fmt::short(id))
    };
    let link = |id: EntityId, text: String| view! { <a href=Route::task(id).hash()>{text}</a> };
    match r {
        Readiness::Ready => view! { <p class="ready ready-yes">"Ready: nothing blocks it."</p> }.into_any(),
        Readiness::NotReady(reasons) => {
            let items = reasons
                .iter()
                .map(|reason| match reason {
                    NotReadyReason::Status(s) => view! { <li>"status is " {fmt::status_label(*s)}</li> }.into_any(),
                    NotReadyReason::Held(why) => view! { <li>"on hold: " {why.clone()}</li> }.into_any(),
                    NotReadyReason::BlockedBy(id) => view! { <li>"blocked by " {link(*id, title(*id))}</li> }.into_any(),
                    NotReadyReason::DependencyClosed(id, s) => {
                        view! { <li>"dependency " {link(*id, title(*id))} " is " {fmt::status_label(*s)} "; retract the relation or cancel this task"</li> }.into_any()
                    }
                })
                .collect::<Vec<_>>();
            view! { <ul class="reasons">{items}</ul> }.into_any()
        }
        Readiness::Unknown(reasons) => {
            let items = reasons
                .iter()
                .map(|reason| match reason {
                    UnknownReason::DependencyUnavailable(id) => {
                        view! { <li>"dependency " {fmt::short(*id)} " is unavailable"</li> }
                    }
                })
                .collect::<Vec<_>>();
            view! { <ul class="reasons">{items}</ul> }.into_any()
        }
    }
}
