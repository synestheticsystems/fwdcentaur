//! Start on a task's panel: it starts the task as `fwd task start` does, making the acting
//! principal its assignee, and it shows only where that start would be let through ([`shows`]): on
//! an open task assigned to the acting principal or to nobody, never on one assigned to someone
//! else, which `ops::start` refuses (`rules::can_start`). Until 2026-10-03 it read "Claim, in
//! <scope>", a name left over from when starting also made an `assigned_to` relation, and showed on
//! every open task (Daniel, that day: "What does the [claim, in X] button do? that is weird af";
//! task fxd2OXdC).
//!
//! A start never moves a task out of the scopes it is worked in (docs/DECISIONS.md, D87,
//! "Starting a task worked in another scope is refused, with a prompt to switch to that scope
//! first", 2026-09-28). The command line refuses a start from a scope the task is not worked in and
//! names the scopes to switch to; the page knows the task's scopes, so Start starts the task in one
//! of them, the first, and its button names no scope (task gL1Qk7Jg, "Let the page's Claim button
//! start a task in the task's own scope"; until 2026-10-03 the button named it, as the prompt). A
//! task may be worked in several scopes since D136 ("A task may be worked in several scopes"), and
//! starting it in any of them leaves every one as it was, so which one the page passes changes
//! nothing but that the start goes through (`rules::can_start_in`). Only a task worked in no scope
//! takes a scope, which it is worked in from then on: any active scope, whoever made it, since
//! anyone works in any scope (D108, "A scope has no owner: scopes are not assigned to
//! principals"), which the button names ("Start in Cloud Hosting"), or while several are active a
//! picker after it ("Start in [Cloud Hosting]"). Until 2026-09-29 it took one of the acting
//! principal's own active scopes, and a person with none of their own was offered none.
//!
//! Where Start shows, what it says and which scope it offers are worked out as data ([`shows`],
//! [`Offer`]) and tested natively, the start rule itself (`rules::can_start_in`) judging that
//! nothing offered would be refused; [`offer`] reads the offer from the page and [`control`] draws
//! it.

use crate::route::{Route, ScopePanel};
use crate::session::{self, ctx};
use crate::{fmt, store};
use ankurah::EntityId;
use fwd_core::model::{ScopeStatus, TaskStatus};
use fwd_core::ops;
use fwd_core::rules::{self, ScopeSnapshot};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

/// Whether Start shows on the panel of a task with `status` and `assignee` for `viewer`, the
/// principal the page acts as: where `ops::start` would let the viewer start it
/// (`rules::can_start`: an open task assigned to the starter or to nobody; the transition table,
/// which `ops::start` reads too, lets Start out of Open alone). A task assigned to someone else is
/// reassigned first, and a page acting as no one starts nothing.
pub fn shows(status: TaskStatus, assignee: Option<EntityId>, viewer: Option<EntityId>) -> bool {
    viewer.is_some_and(|viewer| rules::can_start(status, assignee, viewer).is_ok())
}

/// Where Start on a task's panel may start the task, worked out before the click so that the
/// button can say it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Offer {
    /// The task is worked in these scopes, one or more, in the order it came to be worked in them,
    /// and starts in the first, staying in each.
    Its(Vec<ScopeSnapshot>),
    /// The task is worked in no scope and starts in the one chosen among the active scopes (one or
    /// more, by name), which it is then worked in.
    Active(Vec<ScopeSnapshot>),
    /// The task is worked in no scope, and no scope is active to start it in.
    Nowhere,
}

impl Offer {
    /// What Start offers for a task worked in the scopes `its` (empty when it is worked in no
    /// scope), `active` being the project's active scopes.
    fn new(its: Vec<ScopeSnapshot>, active: Vec<ScopeSnapshot>) -> Offer {
        match (its.is_empty(), active.is_empty()) {
            (false, _) => Offer::Its(its),
            (true, true) => Offer::Nowhere,
            (true, false) => Offer::Active(active),
        }
    }

    /// The scope a click on Start starts the task in: the first of the task's own; else the active
    /// scope chosen in the picker (`chosen`) while it is still active, else the first; none when
    /// there is nowhere to start it.
    fn start_in(&self, chosen: Option<EntityId>) -> Option<&ScopeSnapshot> {
        match self {
            Offer::Its(its) => its.first(),
            Offer::Active(active) => active.iter().find(|s| Some(s.id) == chosen).or(active.first()),
            Offer::Nowhere => None,
        }
    }

    /// The active scopes the picker after the button offers, when a task worked in no scope could
    /// start in more than one.
    fn choices(&self) -> Option<&[ScopeSnapshot]> {
        match self {
            Offer::Active(active) if active.len() > 1 => Some(active),
            _ => None,
        }
    }

    /// What the button says, `target` being the scope a click starts the task in: "Start in" and
    /// its name for a task worked in no scope, unless the picker after the button names it; else
    /// "Start".
    fn label(&self, target: Option<&ScopeSnapshot>) -> String {
        match (self, target) {
            (Offer::Active(_), Some(target)) if self.choices().is_none() => format!("Start in {}", target.title),
            _ => "Start".into(),
        }
    }

    /// What the button says on hover: what starting does, which for a task worked in no scope
    /// includes the scope it takes, `target`.
    fn tooltip(&self, target: Option<&ScopeSnapshot>) -> String {
        let starts = "Take this task on: it moves to in progress with you as its assignee";
        match (self, target) {
            (Offer::Active(_), Some(target)) => format!("{starts}, and {} becomes its scope", target.title),
            _ => starts.into(),
        }
    }
}

/// What Start offers on `task`'s panel, from the page's standing queries: the task's scopes are
/// those the panel shows it worked in (`store::scopes_of_task`), and the active scopes are listed
/// by name, whoever made them.
pub fn offer(task: EntityId) -> Offer {
    let scopes = store::scopes();
    let snapshot = |id: EntityId| {
        let scope = scopes.iter().find(|s| s.id() == id);
        ScopeSnapshot {
            id,
            title: scope.and_then(|s| s.title().ok()).unwrap_or_else(|| fmt::short(id)),
            status: scope.and_then(|s| s.status().ok()),
        }
    };
    let its = store::scopes_of_task(task).into_iter().map(snapshot).collect();
    let mut active: Vec<ScopeSnapshot> =
        scopes.iter().filter(|s| s.status().ok() == Some(ScopeStatus::Active)).map(|s| snapshot(s.id())).collect();
    active.sort_by_key(|s| (s.title.to_lowercase(), s.id));
    Offer::new(its, active)
}

/// Start for `offer` on `task`'s panel, as an item of the panel's row of actions: the button, which
/// starts the task. For a task worked in no scope it names the scope it starts the task in, or,
/// when such a task could start in more than one active scope, "in" and a picker of them follow it
/// (`chosen` holds the choice, which outlives the panel's redraws). With nowhere to start the task,
/// a link to the form for a new scope takes the button's place. A refusal is put in `error`.
pub fn control(
    task: EntityId,
    offer: Offer,
    chosen: RwSignal<Option<EntityId>>,
    error: RwSignal<Option<String>>,
) -> AnyView {
    if offer == Offer::Nowhere {
        let new_scope = Route::Scopes(Some(ScopePanel::New)).hash();
        return view! {
            <span class="muted">
                "Start: it has no scope, and no scope is active to start it in; "
                <a href=new_scope>"create one"</a>
            </span>
        }
        .into_any();
    }
    let choices = offer.choices().map(<[ScopeSnapshot]>::to_vec);
    // The scope a click starts the task in: the button or the picker shows it.
    let target = Memo::new({
        let offer = offer.clone();
        move |_| offer.start_in(chosen.get()).cloned()
    });
    let label = {
        let offer = offer.clone();
        move || offer.label(target.get().as_ref())
    };
    let tooltip = move || offer.tooltip(target.get().as_ref());
    let start = move |_| {
        let (Some(me), Some(scope)) = (session::me(), target.get_untracked()) else { return };
        spawn_local(async move {
            // `ops::start` in that scope, the start `fwd task start --scope <its id>` runs.
            match ops::claim(&ctx(), me, task, scope.id).await {
                Ok(()) => {
                    session::wrote(me);
                    error.try_set(None)
                }
                Err(e) => error.try_set(Some(e.to_string())),
            };
        });
    };
    let button = view! { <button class="btn primary" title=tooltip on:click=start>{label}</button> };
    match choices {
        Some(active) => {
            view! { <span class="start-in">{button} "in" {picker(active, target, chosen)}</span> }.into_any()
        }
        None => button.into_any(),
    }
}

/// The active scopes to start a task in, showing `target`, the one a click on Start starts it in,
/// and putting a new choice in `chosen`. Each option is marked chosen or not as a property, so the
/// picker shows `target` even after a person has chosen in it and the options have changed since.
fn picker(
    active: Vec<ScopeSnapshot>,
    target: Memo<Option<ScopeSnapshot>>,
    chosen: RwSignal<Option<EntityId>>,
) -> AnyView {
    let options = active
        .into_iter()
        .map(|s| {
            let shown = move || target.with(|t| t.as_ref().map(|t| t.id)) == Some(s.id);
            view! { <option value=s.id.to_base64() prop:selected=shown>{s.title.clone()}</option> }
        })
        .collect::<Vec<_>>();
    view! {
        <select title="the scope to start it in" on:change=move |ev| chosen.set(event_target_value(&ev).parse().ok())>
            {options}
        </select>
    }
    .into_any()
}

/// Start on a row of the Inbox's Waiting on you (`request_row.rs`), as [`control`] draws it on a
/// task's panel, but the page's primary button only while `primary` says so, while the row is
/// selected (a column of rows is not a column of blue buttons inviting a click before the
/// reading). `started`, asked at the click, says what to do once the start is made (the row marks
/// its notifications read), which runs in the start's own continuation: the start can redraw the
/// row, or take it off the page, before the daemon answers, so nothing the row owns may carry it.
/// With nowhere to start the task it says so, in the row's few words.
pub fn row_control<Then: FnOnce() + 'static>(
    task: EntityId,
    offer: Offer,
    chosen: RwSignal<Option<EntityId>>,
    error: RwSignal<Option<String>>,
    primary: Signal<bool>,
    started: impl Fn() -> Then + Send + Sync + 'static,
) -> AnyView {
    if offer == Offer::Nowhere {
        let why = "It has no scope, and no scope is active to start it in";
        return view! { <span class="muted" title=why>"no scope to start it in"</span> }.into_any();
    }
    let choices = offer.choices().map(<[ScopeSnapshot]>::to_vec);
    let target = Memo::new({
        let offer = offer.clone();
        move |_| offer.start_in(chosen.get()).cloned()
    });
    let label = {
        let offer = offer.clone();
        move || offer.label(target.get().as_ref())
    };
    let tooltip = move || offer.tooltip(target.get().as_ref());
    let start = move |_| {
        let (Some(me), Some(scope)) = (session::me(), target.get_untracked()) else { return };
        let then = started();
        spawn_local(async move {
            match ops::claim(&ctx(), me, task, scope.id).await {
                Ok(()) => {
                    session::wrote(me);
                    error.try_set(None);
                    then();
                }
                Err(e) => {
                    error.try_set(Some(e.to_string()));
                }
            };
        });
    };
    let class = move || if primary.get() { "btn primary" } else { "btn" };
    let button = view! { <button class=class title=tooltip on:click=start>{label}</button> };
    match choices {
        Some(active) => {
            view! { <span class="start-in">{button} "in" {picker(active, target, chosen)}</span> }.into_any()
        }
        None => button.into_any(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fwd_core::rules::{RuleError, Verb};

    fn id(n: u8) -> EntityId {
        EntityId::from_bytes([n; 32])
    }

    // Three active scopes of the project; who made each one says nothing about who may work in it.
    fn scope(n: u8, title: &str) -> ScopeSnapshot {
        ScopeSnapshot { id: id(n), title: title.into(), status: Some(ScopeStatus::Active) }
    }
    fn hosting() -> ScopeSnapshot {
        scope(10, "Cloud Hosting")
    }
    fn initial() -> ScopeSnapshot {
        scope(11, "Initial Implementation")
    }
    fn reviews() -> ScopeSnapshot {
        scope(12, "Reviews and Highlights")
    }

    const STATUSES: [TaskStatus; 7] = [
        TaskStatus::Proposed,
        TaskStatus::Open,
        TaskStatus::InProgress,
        TaskStatus::InReview,
        TaskStatus::Done,
        TaskStatus::Cancelled,
        TaskStatus::Rejected,
    ];

    #[test]
    fn start_shows_on_an_open_task_assigned_to_the_viewer_or_to_nobody_and_not_to_someone_else() {
        let (viewer, someone_else) = (id(1), id(2));
        assert!(shows(TaskStatus::Open, Some(viewer), Some(viewer)), "assigned to the viewer");
        assert!(shows(TaskStatus::Open, None, Some(viewer)), "assigned to nobody");
        // `ops::start` refuses it: the task is reassigned first.
        assert!(!shows(TaskStatus::Open, Some(someone_else), Some(viewer)), "assigned to someone else");
        for status in STATUSES.into_iter().filter(|s| *s != TaskStatus::Open) {
            for assignee in [None, Some(viewer)] {
                assert!(!shows(status, assignee, Some(viewer)), "a task {status} assigned to {assignee:?}");
            }
        }
        assert!(!shows(TaskStatus::Open, None, None), "a page acting as no one starts nothing");
    }

    #[test]
    fn start_shows_exactly_where_the_checks_ops_start_runs_let_the_start_through() {
        let (viewer, someone_else) = (id(1), id(2));
        for status in STATUSES {
            for assignee in [None, Some(viewer), Some(someone_else)] {
                for needs_review in [false, true] {
                    let let_through = rules::can_start(status, assignee, viewer).is_ok()
                        && rules::transition(status, Verb::Start, needs_review).is_ok();
                    assert_eq!(
                        shows(status, assignee, Some(viewer)),
                        let_through,
                        "a task {status} assigned to {assignee:?}, review {needs_review}"
                    );
                }
            }
        }
    }

    /// Every scope Start could start the task in: each one it offers, and each one a choice in
    /// the picker, whatever it holds, leads to.
    fn offered(offer: &Offer) -> Vec<ScopeSnapshot> {
        let mut all: Vec<ScopeSnapshot> = match offer {
            Offer::Its(its) => its.clone(),
            Offer::Active(active) => active.clone(),
            Offer::Nowhere => Vec::new(),
        };
        for chosen in [None, Some(hosting().id), Some(initial().id), Some(reviews().id), Some(id(99))] {
            all.extend(offer.start_in(chosen).cloned());
        }
        all
    }

    #[test]
    fn a_task_worked_in_a_scope_starts_there_whatever_scopes_are_active() {
        for active in [vec![], vec![initial()], vec![initial(), reviews()]] {
            let offer = Offer::new(vec![hosting()], active.clone());
            assert_eq!(offer, Offer::Its(vec![hosting()]), "with {active:?}");
            for chosen in [None, Some(initial().id), Some(reviews().id)] {
                assert_eq!(offer.start_in(chosen), Some(&hosting()), "a choice made in a picker changes nothing");
            }
        }
    }

    /// A task worked in several scopes (docs/DECISIONS.md, D136, "A task may be worked in several
    /// scopes") starts in the first of them, in which it is worked as in the others, with no picker
    /// and a button that names none, since starting changes none of its scopes.
    #[test]
    fn a_task_worked_in_several_scopes_starts_in_the_first_naming_none() {
        let offer = Offer::new(vec![hosting(), reviews()], vec![initial()]);
        assert_eq!(offer, Offer::Its(vec![hosting(), reviews()]));
        for chosen in [None, Some(initial().id), Some(reviews().id)] {
            assert_eq!(offer.start_in(chosen), Some(&hosting()));
        }
        assert_eq!(offer.choices(), None, "no picker");
        assert_eq!(offer.label(offer.start_in(None)), "Start");
        assert_eq!(
            offer.tooltip(offer.start_in(None)),
            "Take this task on: it moves to in progress with you as its assignee"
        );
    }

    #[test]
    fn a_task_worked_in_no_scope_starts_in_the_active_scope_chosen_whoever_made_it() {
        let offer = Offer::new(vec![], vec![hosting(), initial(), reviews()]);
        assert_eq!(offer, Offer::Active(vec![hosting(), initial(), reviews()]));
        assert_eq!(offer.start_in(None), Some(&hosting()), "before a choice, the first");
        assert_eq!(offer.start_in(Some(reviews().id)), Some(&reviews()));
        assert_eq!(offer.start_in(Some(initial().id)), Some(&initial()));
        // A choice that is no longer among the active scopes (the scope was paused or finished)
        // leads to the first, which the picker then shows.
        assert_eq!(offer.start_in(Some(id(99))), Some(&hosting()));
        // With one active scope there is nothing to choose.
        assert_eq!(Offer::new(vec![], vec![initial()]).start_in(None), Some(&initial()));
    }

    #[test]
    fn a_task_worked_in_no_scope_starts_nowhere_while_no_scope_is_active() {
        let offer = Offer::new(vec![], vec![]);
        assert_eq!(offer, Offer::Nowhere);
        assert_eq!(offer.start_in(None), None);
    }

    #[test]
    fn the_button_names_a_scope_only_for_a_task_worked_in_none_and_says_on_hover_what_starting_does() {
        let starts = "Take this task on: it moves to in progress with you as its assignee";
        // A task worked in a scope starts there, which the button does not say, whatever is active.
        for active in [vec![], vec![initial()], vec![initial(), reviews()]] {
            let its = Offer::new(vec![hosting()], active);
            assert_eq!(its.choices(), None, "no picker");
            assert_eq!(its.label(its.start_in(None)), "Start");
            assert_eq!(its.tooltip(its.start_in(None)), starts);
        }
        // A task worked in no scope takes the only active one, which the button names.
        let one = Offer::new(vec![], vec![initial()]);
        assert_eq!(one.choices(), None, "no picker");
        assert_eq!(one.label(one.start_in(None)), "Start in Initial Implementation");
        assert_eq!(one.tooltip(one.start_in(None)), format!("{starts}, and Initial Implementation becomes its scope"));
        // With several active, the picker after the button names the one chosen, and the button's
        // hover the one a click starts the task in.
        let several = Offer::new(vec![], vec![hosting(), initial(), reviews()]);
        assert_eq!(several.choices(), Some(&[hosting(), initial(), reviews()][..]));
        let chosen = several.start_in(Some(reviews().id));
        assert_eq!(several.label(chosen), "Start");
        assert_eq!(several.tooltip(chosen), format!("{starts}, and Reviews and Highlights becomes its scope"));
    }

    #[test]
    fn nothing_is_offered_that_the_start_rule_would_refuse() {
        // The page once started every task in one of the acting principal's own scopes, which the
        // rule refuses for a task worked in another.
        assert!(matches!(
            rules::can_start_in(Some(initial().id), &[hosting()]),
            Err(RuleError::WorkedInAnotherScope { .. })
        ));
        for its in [vec![], vec![hosting()], vec![initial()], vec![reviews(), hosting()]] {
            for active in [vec![], vec![initial()], vec![hosting(), initial(), reviews()]] {
                let worked_in = its.clone();
                let offer = Offer::new(its.clone(), active.clone());
                for scope in offered(&offer) {
                    assert_eq!(
                        rules::can_start_in(Some(scope.id), &worked_in),
                        Ok(()),
                        "{scope:?} offered for a task worked in {its:?} with {active:?} active"
                    );
                }
            }
        }
    }
}
