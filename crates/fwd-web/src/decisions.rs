//! The Decisions page, in the Tasks page's shape (Daniel, 2026-09-26: "each decision record should
//! show just the decision itself. If they open the decision, it should open a right hand panel
//! with all the references and comments and such."). On the left, the rulings the project has
//! recorded, by number, whatever they are about or linked to, one line each (`D<n>`, its title,
//! its short id, status, area, the scope it governs or "project-wide", who ruled and when or who
//! proposed it), filtered by the scope they govern, by status and by area; ArrowUp and ArrowDown
//! move between the lines. The scope is the header's choice, which the Questions, Tasks and Graph
//! pages list by too and the route carries (`scope_filter.rs`), its choices here keeping rulings:
//! each keeps the rulings that govern the whole project, "everything" keeps every ruling, and a
//! scope its own too, so in the context of one scope the page lists the project's rulings and that
//! scope's, and neither list is too long to read (docs/DECISIONS.md, D93, "A decision may be
//! attached to the scope it governs; one attached to none governs the whole project, and its area
//! is its type", 2026-09-28). On the right, for the decision a line or the route
//! (`#/decisions/<id>`, where a lozenge naming a ruling leads) opens, its panel: the full record,
//! the scope it governs with a picker that attaches it to another or to none (`Governs`), its
//! references as lozenges (`lozenges.rs`: what cites it, what it cites and where it was decided,
//! what supersedes it or it supersedes), then the decision's own row (`decision_row.rs`), which
//! brings its moves and hangs its comments beneath it, and a box for commenting on it. The panel
//! shows the routed decision even while the filters hide its line; the list then highlights none.
//! It has the Tasks page's edge and Full screen button (`panel.rs`; task CyqBDRUz, "Give the Graph
//! page's task panel and the decision panel the same Full screen control"): at every width the
//! button lets it fill the page, the list hidden, and beside the list dragging its left edge sets
//! its width. In the VS Code webview a decision a lozenge on a task's panel opens fills the page,
//! with a Back control that returns to the task, as a task's panel has, and so does a decision
//! opened from another page (an inbox card, the Graph page's panel, a question), with Back to that
//! page (`back.rs`); a Back from another page to this one brings the list back with the status and
//! area it was left with (`page_state.rs`).

use crate::comment_box::{CommentBox, Subject};
use crate::empty::{Empty, Nothing};
use crate::fmt::CopyId;
use crate::panel::{FullScreenToggle, PanelEdge, PanelSize};
use crate::referents::{Mode, Models};
use crate::route::{self, Route};
use crate::scope_filter::{self, ScopeChoice};
use crate::session::ctx;
use crate::thread::{self, Hooks, Model, OpenQueries, ThreadState};
use crate::{back, decision_row, fmt, keys, lozenges, modes, page_state, session, store};
use ankurah::EntityId;
use fwd_core::decision_log;
use fwd_core::model::{DecisionStatus, DecisionView};
use fwd_core::{ops, rules};
use leptos::prelude::*;
use std::collections::HashMap;
use wasm_bindgen_futures::spawn_local;

/// The localStorage key under which this browser remembers how wide the decision panel was
/// dragged, apart from the task panel's.
const PANEL_WIDTH: &str = "fwd.web.decision-panel-width";

/// The statuses the filter offers, in the order a ruling goes through them.
const STATUSES: [DecisionStatus; 5] = [
    DecisionStatus::Proposed,
    DecisionStatus::Accepted,
    DecisionStatus::Rejected,
    DecisionStatus::Superseded,
    DecisionStatus::Repealed,
];

/// Which decisions a filter keeps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filter {
    /// Only this status; every status when none.
    pub status: Option<DecisionStatus>,
    /// Only this area, compared without regard to case; `Some("")` keeps those with no area.
    pub area: Option<String>,
    /// The scope chosen: every ruling (all scopes), the rulings that govern the whole project
    /// (no scope), or those and one scope's.
    pub scope: ScopeChoice,
}

impl Default for Filter {
    /// Every decision.
    fn default() -> Self {
        Filter { status: None, area: None, scope: ScopeChoice::All }
    }
}

/// What the page knows of one decision for listing it.
#[derive(Clone, Debug)]
pub struct Listed {
    pub id: EntityId,
    pub number: i64,
    pub status: Option<DecisionStatus>,
    pub area: Option<String>,
    /// The scope it governs; none when it governs the whole project.
    pub scope: Option<EntityId>,
}

/// The decisions a filter keeps, by number.
pub fn listed(all: &[Listed], filter: &Filter) -> Vec<EntityId> {
    let mut kept: Vec<&Listed> = all
        .iter()
        .filter(|d| {
            let status_ok = filter.status.is_none_or(|s| d.status == Some(s));
            let area_ok = match filter.area.as_deref() {
                None => true,
                Some("") => d.area.as_deref().is_none_or(|a| a.trim().is_empty()),
                Some(wanted) => d.area.as_deref().is_some_and(|a| a.trim().eq_ignore_ascii_case(wanted)),
            };
            status_ok && area_ok && kept_by(filter.scope, d.scope)
        })
        .collect();
    kept.sort_by_key(|d| (d.number, d.id));
    kept.into_iter().map(|d| d.id).collect()
}

/// Whether the scope filter's `choice` keeps a ruling attached to `scope`, none for one that
/// governs the whole project: "all scopes" keeps every ruling, "no scope" the project's, and a
/// scope the project's and its own, the rulings that apply to its work.
fn kept_by(choice: ScopeChoice, scope: Option<EntityId>) -> bool {
    match choice {
        ScopeChoice::All => true,
        ScopeChoice::NoScope => scope.is_none(),
        ScopeChoice::Scope(chosen) => rules::applies_to(scope, chosen),
    }
}

/// The areas the decisions carry, each once whatever its case, sorted.
pub fn areas(all: &[Listed]) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for area in all.iter().filter_map(|d| d.area.as_deref()).map(str::trim).filter(|a| !a.is_empty()) {
        if !found.iter().any(|f| f.eq_ignore_ascii_case(area)) {
            found.push(area.to_string());
        }
    }
    found.sort_by_key(|a| a.to_lowercase());
    found
}

fn snapshot() -> Vec<Listed> {
    store::decisions()
        .iter()
        .map(|d| Listed {
            id: d.id(),
            number: d.number().unwrap_or(0),
            status: d.status().ok(),
            area: d.area().ok().flatten(),
            scope: ops::governs(d).ok().flatten(),
        })
        .collect()
}

/// The Decisions page: the list and its filters, and beside it (beneath it on a narrow page) the
/// panel of the decision the route selects. At every width a button lets the panel fill the page,
/// the list hidden, and beside the list dragging the panel's left edge sets its width
/// (`panel.rs`); the route stays what it was. `scope` is the scope whose rulings the list keeps
/// with the project's, the page's choice (`scope_filter::follow`): none until the page has picked
/// the one it opens on.
#[component]
pub fn DecisionsPage(selected: Memo<Option<EntityId>>, scope: Memo<Option<ScopeChoice>>) -> impl IntoView {
    let size = PanelSize::new(PANEL_WIDTH);
    // In the VS Code webview a decision opened from a task's panel fills the page, with Back to the
    // task, and one opened from that decision's panel has Back to it (`back.rs`; task toDjwytu,
    // "Carry the Graph's scope filter and a page's state through Back in the webview"); one opened
    // from another page fills it too, with Back to that page (task lccNMCh_).
    let back = session::locked().then(|| back::follow_decisions(size, selected, scope));
    let with_back = Memo::new(move |_| back.is_some_and(back::Back::on_trail));
    let open = move || selected.with(Option::is_some);
    // With no decision open the list comes back, so the next decision opened opens beside it.
    Effect::new(move |_| {
        if !open() {
            size.full.set(false);
        }
    });
    view! {
        <div class="split resizable" class:with-detail=open class:full-screen=move || open() && size.full.get()
            class:dragging=move || size.dragging.get() style=("--panel-width", move || size.width_property())>
            <section class="page list-pane decisions">
                <DecisionList selected=selected scope=scope full=size.full/>
            </section>
            {move || {
                selected.get().map(|id| {
                    view! {
                        <PanelEdge size=size/>
                        <aside class="detail-pane decision-detail">
                            // With Back, Back at the start of the panel's first line and the toggle
                            // at its end, as on a task's panel.
                            {move || match back.filter(|_| with_back.get()) {
                                Some(back) => view! {
                                    <div class="panel-bar">
                                        <back::BackButton back=back/>
                                        <FullScreenToggle size=size/>
                                    </div>
                                }
                                .into_any(),
                                None => view! { <FullScreenToggle size=size/> }.into_any(),
                            }}
                            {crate::modes::full(id, &Models::one(Model::Decision))}
                        </aside>
                    }
                })
            }}
        </div>
    }
}

/// The list of decisions and its filters. `full` is whether the decision panel fills the page,
/// the list hidden.
#[component]
fn DecisionList(
    selected: Memo<Option<EntityId>>,
    scope: Memo<Option<ScopeChoice>>,
    full: RwSignal<bool>,
) -> impl IntoView {
    // The status and area as the list was left, when Back in the VS Code webview is bringing the
    // page back from another page; else every status and every area, as a visit starts
    // (`page_state.rs`). The list keeps them there as they change.
    let kept = page_state::open_decision_list();
    let status = RwSignal::new(kept.status);
    let area = RwSignal::new(kept.area);
    Effect::new(move |_| {
        page_state::keep_decision_list(page_state::DecisionListState { status: status.get(), area: area.get() });
    });
    // None while the page picks the scope it opens on.
    let shown = Memo::new(move |_| {
        let scope = scope.get()?;
        Some(listed(&snapshot(), &Filter { status: status.get(), area: area.get(), scope }))
    });
    arrows_move_the_selection(shown, selected, scope, full);
    let on_status = move |ev: leptos::ev::Event| {
        let chosen = event_target_value(&ev);
        status.set(STATUSES.into_iter().find(|s| s.to_string() == chosen));
    };
    let on_area = move |ev: leptos::ev::Event| {
        let chosen = event_target_value(&ev);
        // "*" is every area; "" is no area at all.
        area.set((chosen != "*").then_some(chosen));
    };
    // The areas the select offers: those the decisions carry, and the area chosen should no
    // decision carry it any more, which stays chosen, and shows so, until another is chosen. A
    // change to anything else about a decision leaves the options as they are.
    let area_choices = Memo::new(move |_| {
        let mut choices = areas(&snapshot());
        let gone = area.get().filter(|a| !a.is_empty() && !choices.iter().any(|c| c.eq_ignore_ascii_case(a)));
        choices.extend(gone);
        choices
    });
    let area_options = move || {
        area_choices
            .get()
            .into_iter()
            .map(|a| view! { <option value=a.clone()>{a.clone()}</option> })
            .collect::<Vec<_>>()
    };
    // The select shows the area chosen, as the list opens and once its options are drawn again,
    // since a select whose options are replaced shows its first one.
    let area_select = NodeRef::<leptos::html::Select>::new();
    Effect::new(move |_| {
        let value = area.get().unwrap_or_else(|| "*".into());
        area_choices.track();
        if let Some(select) = area_select.get() {
            select.set_value(&value);
        }
    });
    let list = move || {
        let Some(ids) = shown.get() else {
            return view! { <Empty nothing=Nothing::Yet/> }.into_any();
        };
        if ids.is_empty() {
            let nothing = Nothing::Decisions { filtered_out: !store::decisions().is_empty() };
            return view! { <Empty nothing=nothing/> }.into_any();
        }
        let mut by_id: HashMap<EntityId, DecisionView> = store::decisions().into_iter().map(|d| (d.id(), d)).collect();
        let lines = ids
            .into_iter()
            .filter_map(|id| by_id.remove(&id))
            .map(|d| decision_line(d, selected, scope))
            .collect::<Vec<_>>();
        view! {
            <table class="list decisions">
                <thead>
                    <tr>
                        <th>"Decision"</th><th>"Id"</th><th>"Status"</th><th>"Area"</th><th>"Governs"</th><th>"Ruled"</th>
                    </tr>
                </thead>
                <tbody>{lines}</tbody>
            </table>
        }
        .into_any()
    };
    // What the list holds, beside the filters.
    let caption = move || match scope.get() {
        Some(ScopeChoice::All) | None => "every ruling recorded, by number",
        Some(ScopeChoice::NoScope) => "the rulings that govern the whole project, by number",
        Some(ScopeChoice::Scope(_)) => "the rulings that govern the whole project and this scope's, by number",
    };
    view! {
        <header class="page-head">
            <h2>"Decisions"</h2>
            <label class="field">
                "status "
                // Its options never change, so the status the list opens with is chosen once.
                <select on:change=on_status>
                    <option value="" selected=status.get_untracked().is_none()>"every status"</option>
                    {STATUSES
                        .into_iter()
                        .map(|s| {
                            view! { <option value=s.to_string() selected=status.get_untracked() == Some(s)>{s.to_string()}</option> }
                        })
                        .collect::<Vec<_>>()}
                </select>
            </label>
            <label class="field">
                "area "
                <select node_ref=area_select on:change=on_area>
                    <option value="*" selected=true>"every area"</option>
                    {area_options}
                    <option value="">"no area"</option>
                </select>
            </label>
            <span class="muted">{caption}</span>
            <OpenQueries/>
        </header>
        {list}
    }
}

/// The element id of a decision's line, by which the arrows scroll it into view.
fn line_id(id: EntityId) -> String {
    format!("decision-line-{}", id.to_base64())
}

/// One decision's line: a click opens its panel, the list keeping its scope filter.
fn decision_line(d: DecisionView, selected: Memo<Option<EntityId>>, scope: Memo<Option<ScopeChoice>>) -> impl IntoView {
    let id = d.id();
    let number = format!("D{}", d.number().unwrap_or(0));
    // The scope's name, as the Tasks page's lines name a task's scope: words, not a lozenge, since
    // a click anywhere on the line opens the decision.
    let governs = match ops::governs(&d).ok().flatten() {
        Some(scope) => view! { <span>{store::scope_label(scope)}</span> }.into_any(),
        None => view! { <span class="governs muted">"project-wide"</span> }.into_any(),
    };
    view! {
        <tr class="row" id=line_id(id) class:selected=move || selected.get() == Some(id)
            on:click=move |_| route::go(Route::Decisions { decision: Some(id), scope: scope.get_untracked() })>
            // Each cell but the first names its field, which a narrow page shows beside it once the
            // rows stack (app.css).
            <td class="title"><span class="decision-number">{number}</span>" "{d.title().unwrap_or_default()}</td>
            <td data-label="id"><CopyId id=id/></td>
            <td data-label="status">
                {d.status().ok().map(|s| view! { <span class=format!("pill decision-{s}")>{s.to_string()}</span> })}
            </td>
            <td data-label="area">{d.area().ok().flatten().map(|a| view! { <span class="pill area">{a}</span> })}</td>
            <td data-label="governs">{governs}</td>
            <td data-label="ruled" class="muted">{ruled(&d)}</td>
        </tr>
    }
}

/// Who ruled on a decision and when, or who proposed it and when: "decided by dnorman,
/// 2026-09-24", "proposed by claude, 2026-09-26", "rejected by dnorman, 2026-09-26".
fn ruled(d: &DecisionView) -> String {
    let who = |p: Option<EntityId>| p.map(store::handle_of).unwrap_or_default();
    let author = d.author().ok().map(EntityId::from);
    let decider = d.decided_by().ok().flatten().map(EntityId::from);
    let day = d.decided().ok().flatten().or(d.created().ok()).map(decision_log::date).unwrap_or_default();
    match (d.status().ok(), decider) {
        (Some(DecisionStatus::Proposed), _) | (_, None) => format!("proposed by {}, {day}", who(author)),
        (Some(DecisionStatus::Rejected), _) => format!("rejected by {}, {day}", who(decider)),
        _ => format!("decided by {}, {day}", who(decider)),
    }
}

/// ArrowUp and ArrowDown open the decision on the line above or below, keeping the scope filter,
/// while no text field or select has the keyboard and the focus is not in the panel, whose rows
/// use the arrows themselves. While the panel fills the page (`full`) the lines are hidden, and the
/// arrows keep the meaning the browser gives them, scrolling, rather than change the decision the
/// panel shows. The listener goes with the list.
fn arrows_move_the_selection(
    shown: Memo<Option<Vec<EntityId>>>,
    selected: Memo<Option<EntityId>>,
    scope: Memo<Option<ScopeChoice>>,
    full: RwSignal<bool>,
) {
    let listener = window_event_listener(leptos::ev::keydown, move |ev| {
        let delta: isize = match ev.key().as_str() {
            "ArrowUp" => -1,
            "ArrowDown" => 1,
            _ => return,
        };
        if ev.default_prevented() || !keys::is_plain(&ev) || full.get_untracked() {
            return;
        }
        let document = web_sys::window().and_then(|w| w.document());
        let active = document.as_ref().and_then(|d| d.active_element());
        let elsewhere =
            active.is_some_and(|a| keys::is_text_field(&a) || a.closest(".detail-pane").ok().flatten().is_some());
        if elsewhere {
            return;
        }
        let ids = shown.get_untracked().unwrap_or_default();
        let at = selected.get_untracked().and_then(|id| ids.iter().position(|i| *i == id));
        let next = match at {
            Some(at) => at.checked_add_signed(delta),
            None if delta > 0 => Some(0),
            None => ids.len().checked_sub(1),
        };
        let Some(id) = next.and_then(|i| ids.get(i)).copied() else { return };
        ev.prevent_default();
        route::go(Route::Decisions { decision: Some(id), scope: scope.get_untracked() });
        request_animation_frame(move || {
            let line = web_sys::window().and_then(|w| w.document()).and_then(|d| d.get_element_by_id(&line_id(id)));
            if let Some(line) = line {
                let options = web_sys::ScrollIntoViewOptions::new();
                options.set_block(web_sys::ScrollLogicalPosition::Nearest);
                line.scroll_into_view_with_scroll_into_view_options(&options);
            }
        });
    });
    on_cleanup(move || listener.remove());
}

/// The full mode of a decision: its panel.
pub fn full(id: EntityId) -> AnyView {
    view! { <DecisionDetail id=id/> }.into_any()
}

/// A decision's panel: its number and title, its short id, status and area, who ruled, the scope
/// it governs with the picker that changes it, the full record, its references as lozenges (among
/// them the messages that mention it), then its own row with its moves and its comments beneath
/// it, and a box for commenting.
#[component]
fn DecisionDetail(id: EntityId) -> impl IntoView {
    // The head and the record, each redrawn whenever the decision changes.
    let head = move || {
        let Some(d) = store::decision(id) else {
            return view! { <Empty nothing=Nothing::Decision/> }.into_any();
        };
        view! {
            <header class="detail-head">
                <h3>{format!("D{} {}", d.number().unwrap_or(0), d.title().unwrap_or_default())}</h3>
                <div class="pills">
                    <CopyId id=id/>
                    {d.status().ok().map(|s| view! { <span class=format!("pill decision-{s}")>{s.to_string()}</span> })}
                    {d.area().ok().flatten().map(|a| view! { <span class="pill area">{a}</span> })}
                </div>
                <div class="muted">{ruled(&d)}</div>
            </header>
        }
        .into_any()
    };
    let record = move || {
        let d = store::decision(id)?;
        let prose = |label: &'static str, text: String| {
            (!text.trim().is_empty()).then(|| view! { <section class="prose"><h4>{label}</h4><p>{text}</p></section> })
        };
        Some(view! {
            {prose("Context", ops::text(d.context()).unwrap_or_default())}
            {prose("Ruling", ops::text(d.decision()).unwrap_or_default())}
            {prose("Consequences", ops::text(d.consequences()).unwrap_or_default())}
        })
    };
    view! {
        <div class="detail">
            {head}
            // Outside the closures that redraw with every change to the decision, so the picker
            // keeps its state and a group opened here stays open.
            <Governs decision=id/>
            {record}
            // Why it closed, from the comment its closing move wrote, with names drawn: it redraws
            // with that note, not with the rest of the record.
            {decision_row::closing_section(id)}
            {lozenges::section(id, "References", &[])}
            <section class="prose">
                <h4>"Moves and comments"</h4>
                // A fresh thread per acting principal: moves and comments are theirs.
                {move || session::acting().get().map(|p| view! { <DecisionThread id=id me=p.id()/> })}
                <CommentBox about=Subject::Decision(id)/>
            </section>
        </div>
    }
}

/// One choice of the panel's picker of the scope a decision governs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reach {
    /// The scope; none for the whole project, which detaches the decision from any scope.
    pub scope: Option<EntityId>,
    pub label: String,
}

/// The choices the panel's picker offers, in order: the whole project first, then each scope as
/// `ordered` lists them (`scope_filter::ordered_scopes`: its name, active scopes first, then by
/// name). A scope the decision governs that the page does not hold (yet) is
/// offered last, by the start of its id, so the picker never shows another scope than the one the
/// decision governs.
pub fn reaches(governs: Option<EntityId>, ordered: &[(EntityId, String)]) -> Vec<Reach> {
    let whole = Reach { scope: None, label: "no scope: the whole project".into() };
    let unknown = governs.filter(|g| !ordered.iter().any(|(id, _)| id == g)).map(|g| (g, fmt::short(g)));
    let scopes = ordered.iter().cloned().chain(unknown).map(|(id, label)| Reach { scope: Some(id), label });
    std::iter::once(whole).chain(scopes).collect()
}

/// What choosing in the panel's picker does to a decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reattach {
    /// Attach it to this scope, in place of any other (`ops::attach_decision`).
    Attach(EntityId),
    /// Detach it from its scope, so that it governs the whole project (`ops::detach_decision`).
    Detach,
    /// Nothing: it governs what was chosen already.
    Keep,
}

/// What choosing `chosen` (a scope, or none for the whole project) does to a decision that
/// governs `governs`.
pub fn reattach(governs: Option<EntityId>, chosen: Option<EntityId>) -> Reattach {
    if governs == chosen {
        return Reattach::Keep;
    }
    chosen.map_or(Reattach::Detach, Reattach::Attach)
}

/// The scope a decision governs, as its lozenge or "the whole project", and a picker that
/// attaches the decision to a scope or to none, as `fwd decision attach` and `detach` do: whoever
/// the page acts as may change it, as anyone who may comment on the decision may, and nobody is
/// told (docs/DECISIONS.md, D93). The picker lists the whole project first, then every scope,
/// active ones first ([`reaches`]); an operation's refusal shows beside it.
#[component]
fn Governs(decision: EntityId) -> impl IntoView {
    let error = RwSignal::new(None::<String>);
    // Nothing to say about a decision that has not arrived yet.
    let loaded = Memo::new(move |_| store::decision(decision).is_some());
    let governs = Memo::new(move |_| store::decision(decision).and_then(|d| ops::governs(&d).ok().flatten()));
    let offered = Memo::new(move |_| reaches(governs.get(), &scope_filter::ordered_scopes()));
    let choose = move |ev: leptos::ev::Event| {
        let chosen = event_target_value(&ev).parse::<EntityId>().ok();
        let Some(me) = session::me() else { return };
        let change = reattach(governs.get_untracked(), chosen);
        spawn_local(async move {
            let done = match change {
                Reattach::Attach(scope) => ops::attach_decision(&ctx(), me, decision, scope).await,
                Reattach::Detach => ops::detach_decision(&ctx(), me, decision).await,
                Reattach::Keep => Ok(()),
            };
            match done {
                Ok(()) => {
                    if !matches!(change, Reattach::Keep) {
                        session::wrote(me);
                    }
                    error.try_set(None)
                }
                Err(e) => error.try_set(Some(e.to_string())),
            };
        });
    };
    // The picker shows the scope the decision governs, whatever was chosen in it before: set once
    // its options are drawn, since an option's `selected` attribute stops moving a select once a
    // person has chosen in it, and the scope also changes from the command line.
    let select = NodeRef::<leptos::html::Select>::new();
    Effect::new(move |_| {
        let value = governs.get().map(|s| s.to_base64()).unwrap_or_default();
        offered.track();
        if let Some(select) = select.get() {
            select.set_value(&value);
        }
    });
    let options = move || {
        let current = governs.get();
        offered
            .get()
            .into_iter()
            .map(|r| {
                let value = r.scope.map(|s| s.to_base64()).unwrap_or_default();
                view! { <option value=value selected=r.scope == current>{r.label}</option> }
            })
            .collect::<Vec<_>>()
    };
    view! {
        <div class="assignment governs" hidden=move || !loaded.get()>
            <span class="field-label">"governs"</span>
            {move || match governs.get() {
                Some(scope) => modes::draw(Mode::Full, Mode::Lozenge, scope, &Models::one(Model::Scope)),
                None => view! { <span class="muted">"the whole project"</span> }.into_any(),
            }}
            <label class="field"
                title="attach the ruling to the scope whose work it governs, or to none, so that it governs the whole project">
                "attach to "
                <select node_ref=select on:change=choose>{options}</select>
            </label>
            {move || error.get().map(|e| view! { <span class="error">{e}</span> })}
        </div>
    }
}

/// The decision's own row and every row beneath it, as a card shows a decision: its moves (each
/// asking its reason, a supersession its successor) and its comments, worked from the keyboard as
/// every thread is. Its references show as lozenges under "References" above, so its own row
/// leaves them out.
#[component]
fn DecisionThread(id: EntityId, me: EntityId) -> impl IntoView {
    let hooks = Hooks { strip_above: true, ..Hooks::default() };
    thread::card_row(ThreadState::new(me), id, id, Models::one(Model::Decision), hooks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decision(number: i64, status: DecisionStatus, area: Option<&str>) -> Listed {
        Listed {
            id: EntityId::from_bytes([number as u8; 32]),
            number,
            status: Some(status),
            area: area.map(str::to_string),
            scope: None,
        }
    }

    fn scope(n: u8) -> EntityId {
        EntityId::from_bytes([100 + n; 32])
    }

    fn ids(numbers: &[i64]) -> Vec<EntityId> {
        numbers.iter().map(|n| EntityId::from_bytes([*n as u8; 32])).collect()
    }

    #[test]
    fn the_filters_keep_by_status_and_area_and_the_list_runs_by_number() {
        use DecisionStatus::*;
        let all = vec![
            decision(3, Proposed, Some("UX")),
            decision(1, Accepted, Some("architecture")),
            decision(2, Repealed, None),
            decision(4, Accepted, Some("ux")),
        ];
        let by = |status, area: Option<&str>| Filter { status, area: area.map(str::to_string), ..Filter::default() };
        assert_eq!(listed(&all, &Filter::default()), ids(&[1, 2, 3, 4]));
        assert_eq!(listed(&all, &by(Some(Accepted), None)), ids(&[1, 4]));
        assert_eq!(listed(&all, &by(None, Some("ux"))), ids(&[3, 4]));
        assert_eq!(listed(&all, &by(None, Some(""))), ids(&[2]));
        // The panel shows a decision the filters hide; the list does not keep its line for it.
        assert_eq!(listed(&all, &by(Some(Proposed), None)), ids(&[3]));
        assert_eq!(areas(&all), vec!["architecture".to_string(), "UX".to_string()]);
    }

    #[test]
    fn a_scope_lists_the_project_wide_rulings_and_its_own_and_everything_lists_every_ruling() {
        use DecisionStatus::*;
        let attached = |number, status, to: u8| Listed { scope: Some(scope(to)), ..decision(number, status, None) };
        let all = vec![
            decision(1, Accepted, None),
            attached(2, Accepted, 1),
            attached(3, Proposed, 2),
            decision(4, Repealed, None),
            attached(5, Accepted, 1),
        ];
        let within = |choice| Filter { scope: choice, ..Filter::default() };
        assert_eq!(listed(&all, &within(ScopeChoice::Scope(scope(1)))), ids(&[1, 2, 4, 5]));
        assert_eq!(listed(&all, &within(ScopeChoice::Scope(scope(2)))), ids(&[1, 3, 4]));
        assert_eq!(listed(&all, &within(ScopeChoice::Scope(scope(3)))), ids(&[1, 4]), "a scope with none of its own");
        assert_eq!(listed(&all, &within(ScopeChoice::NoScope)), ids(&[1, 4]), "project-wide only");
        assert_eq!(listed(&all, &within(ScopeChoice::All)), ids(&[1, 2, 3, 4, 5]), "everything");
        // The scope filter narrows what the status and area filters keep, and they it.
        let in_force = Filter { status: Some(Accepted), scope: ScopeChoice::Scope(scope(1)), area: None };
        assert_eq!(listed(&all, &in_force), ids(&[1, 2, 5]));
    }

    #[test]
    fn the_panel_offers_the_whole_project_first_then_every_scope_and_says_what_a_choice_does() {
        let ordered = vec![(scope(1), "Cloud Hosting".to_string()), (scope(2), "Open Core".into())];
        let offered = |governs| reaches(governs, &ordered).into_iter().map(|r| (r.scope, r.label)).collect::<Vec<_>>();
        let whole = (None, "no scope: the whole project".to_string());
        assert_eq!(
            offered(None),
            vec![whole.clone(), (Some(scope(1)), "Cloud Hosting".into()), (Some(scope(2)), "Open Core".into())]
        );
        // A scope the page does not hold (yet) is still offered, last, by the start of its id.
        let far = scope(9);
        assert_eq!(offered(Some(far)).last(), Some(&(Some(far), fmt::short(far))));
        assert_eq!(offered(Some(scope(1))).len(), 3, "a scope the page holds is offered once");
        // Choosing another scope attaches the decision there; the whole project detaches it; what
        // it governs already changes nothing.
        assert_eq!(reattach(None, Some(scope(1))), Reattach::Attach(scope(1)));
        assert_eq!(reattach(Some(scope(1)), Some(scope(2))), Reattach::Attach(scope(2)));
        assert_eq!(reattach(Some(scope(1)), None), Reattach::Detach);
        assert_eq!(reattach(Some(scope(1)), Some(scope(1))), Reattach::Keep);
        assert_eq!(reattach(None, None), Reattach::Keep);
    }
}
