//! The one scope filter the Questions, Tasks, Decisions and Graph pages share, so that a person
//! follows one scope's work from page to page rather than everything in the project, choosing the
//! scope once (Daniel, 2026-09-29: "Feature request: Questions, Task, Decisions, and Graph all need
//! a unified Scope selector. (in the header? or elsewhere? ...)"; task 10ObR8Fi). It is a select in
//! the header, right after the project's ([`HeaderScope`]), offering "all scopes" first, then
//! "no scope", then each scope by its name, active scopes first, in the same words on the four
//! pages, each page saying on hover what it keeps by them ([`Keeps`]). On the Tasks and Graph pages
//! a scope keeps the tasks worked in it, among other scopes or alone (`store::task_scopes`; a task
//! may be worked in several since D136, "A task may be worked in several scopes", and is kept under
//! each), "no scope" the tasks worked in none and "all scopes" every task; on the Questions page the
//! same choices keep the questions about the scope's work (`questions.rs`); on the Decisions page
//! every choice keeps the rulings that govern the whole project, since those apply to every
//! scope's work, a scope adds its own and "all scopes" keeps every ruling (`decisions.rs`, whose
//! caption beside the list says so). Until 2026-10-03 "no scope" read "unassigned", which the
//! page now says only of a task nobody is assigned (Daniel, that day: "what does worked in mean?
//! that is weird. I feel like it should just say Scope(s)"; task PHfp4WUc).
//!
//! The choice belongs to the routes of the four pages, so it survives choosing a task, a reload and
//! the browser's back button: the select shows what the route says, and choosing takes the page to
//! its route with the new choice. The page also keeps the last choice a route named for as long as
//! it lives, so the choice follows the person from page to page ([`follow`]): the nav's links carry
//! it, and a route that names none (a link naming a task or a ruling alone) takes it. While the page
//! has none yet, a route that names none opens on the scope [`start_on`] picks.

use crate::route::{self, Route};
use crate::{fmt, session, store};
use ankurah::EntityId;
use fwd_core::model::{ScopeStatus, TaskStatus};
use fwd_core::ops;
use leptos::prelude::*;
use std::sync::OnceLock;

/// What the scope filter keeps, by the scopes of the work each thing is part of: a task's scopes
/// are those it is worked in, a question's those it is about, a ruling's the one it governs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ScopeChoice {
    /// Everything, whatever scopes it is part of, if any.
    All,
    /// What is part of no scope: the tasks worked in none, for one.
    NoScope,
    /// What is part of this scope, among others or alone: the tasks worked in it, for one.
    Scope(EntityId),
}

/// How "all scopes" and "no scope" are written in a route and in the select: words no scope's id
/// can be.
const ALL: &str = "all";
const NONE: &str = "none";
/// How a route wrote "no scope" until 2026-10-03, which still reads as it, so that a link made
/// before then opens on what it named.
const UNASSIGNED: &str = "unassigned";

impl ScopeChoice {
    /// Whether the filter keeps what is part of `scopes`, none for what is part of no scope: a
    /// scope keeps it when it is one of them, so that a task worked in several scopes is listed
    /// under each. (The Decisions page keeps the rulings that govern the whole project whatever the
    /// choice.)
    pub fn keeps(self, scopes: &[EntityId]) -> bool {
        match self {
            ScopeChoice::All => true,
            ScopeChoice::NoScope => scopes.is_empty(),
            ScopeChoice::Scope(chosen) => scopes.contains(&chosen),
        }
    }

    /// The scope chosen, when the choice is one scope.
    pub fn scope(self) -> Option<EntityId> {
        match self {
            ScopeChoice::Scope(id) => Some(id),
            ScopeChoice::All | ScopeChoice::NoScope => None,
        }
    }

    /// The choice as a route's `?scope=` and the select's options write it: the scope's id, or
    /// "all" or "none".
    pub fn token(self) -> String {
        match self {
            ScopeChoice::All => ALL.into(),
            ScopeChoice::NoScope => NONE.into(),
            ScopeChoice::Scope(id) => id.to_base64(),
        }
    }

    /// The choice `token` wrote, or wrote before 2026-10-03, or none for anything else.
    pub fn parse(token: &str) -> Option<ScopeChoice> {
        match token {
            ALL => Some(ScopeChoice::All),
            NONE | UNASSIGNED => Some(ScopeChoice::NoScope),
            id => id.parse().ok().map(ScopeChoice::Scope),
        }
    }
}

/// What the page showing keeps by the filter, which the select says on hover.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Keeps {
    /// Tasks, by the scopes each is worked in (the Tasks and Graph pages).
    Tasks,
    /// Questions, by the scope each is about (the Questions page).
    Questions,
    /// Rulings, by the scope each governs (the Decisions page). Every choice keeps the rulings that
    /// govern the whole project, since those apply to every scope's work (docs/DECISIONS.md, D93).
    Rulings,
}

impl Keeps {
    /// What the page whose route's section is `section` (`Route::section`) keeps.
    fn of(section: &str) -> Keeps {
        match section {
            "questions" => Keeps::Questions,
            "decisions" => Keeps::Rulings,
            _ => Keeps::Tasks,
        }
    }

    /// What the select says it keeps, on hover.
    fn hint(self) -> &'static str {
        match self {
            Keeps::Tasks => "The scope whose tasks this page shows: those in it",
            Keeps::Questions => {
                "The scope whose questions this page shows: those about the scope itself, a task in it or a decision attached to it"
            }
            Keeps::Rulings => {
                "The scope whose rulings this page shows beside those that govern the whole project, which every choice keeps"
            }
        }
    }
}

/// How the select names `choice` on every page, `label` being a scope's name.
fn name(choice: ScopeChoice, label: &str) -> String {
    match choice {
        ScopeChoice::All => "all scopes".into(),
        ScopeChoice::NoScope => "no scope".into(),
        ScopeChoice::Scope(_) => label.into(),
    }
}

/// Every scope's id and label (its name), active scopes first, then by name: the order a scope
/// filter and a picker of scopes list them in.
pub fn ordered_scopes() -> Vec<(EntityId, String)> {
    let mut scopes: Vec<(bool, String, EntityId)> = store::scopes()
        .iter()
        .map(|s| (s.status().ok() != Some(ScopeStatus::Active), s.title().unwrap_or_default().to_lowercase(), s.id()))
        .collect();
    scopes.sort();
    scopes.into_iter().map(|(_, _, id)| (id, store::scope_label(id))).collect()
}

/// The header's scope filter, right after the project's selector so that the header reads project,
/// then scope. `chosen` is the page's choice ([`follow`]); while it is none (the page has yet to
/// pick the scope it opens on) no option shows as chosen. It shows while the Questions, Tasks,
/// Decisions or Graph page shows, saying on hover what that page keeps by it ([`Keeps::of`]), and
/// choosing takes that page to its route with the new choice. On the Inbox, the Scopes page and a
/// highlight's page, which list nothing by scope, it is hidden but keeps its place, so that the
/// nav's links beside it stay where they are from page to page: its choices read alike on every
/// page, so it is as wide on each.
#[component]
pub fn HeaderScope(chosen: Memo<Option<ScopeChoice>>) -> impl IntoView {
    let route = route::route();
    // The section of the page showing, while it lists by scope.
    let listing = Memo::new({
        let route = route.clone();
        move |_| route.with(|here| here.filtered().then(|| here.section()))
    });
    let on_change = move |ev: leptos::ev::Event| {
        if let Some(choice) = ScopeChoice::parse(&event_target_value(&ev)) {
            route::go(route.get_untracked().with_scope(choice));
        }
    };
    // Each scope's id and label, in the select's order; a change to anything else about a scope
    // changes neither, so it does not draw the options again.
    let choices = Memo::new(move |_| ordered_scopes());
    let options = move || {
        let current = chosen.get().and_then(ScopeChoice::scope);
        let choices = choices.get();
        // A scope the page has not received (yet) still shows as the one chosen.
        let unknown = current.filter(|id| !choices.iter().any(|(c, _)| c == id)).map(|id| (id, fmt::short(id)));
        choices
            .into_iter()
            .chain(unknown)
            .map(|(id, label)| {
                let label = name(ScopeChoice::Scope(id), &label);
                view! { <option value=id.to_base64() selected=current == Some(id)>{label}</option> }
            })
            .collect::<Vec<_>>()
    };
    // The select shows the page's choice, whatever it showed before: set once the options are
    // drawn, since an option's `selected` attribute stops moving a select once a person (or a
    // script) has chosen in it, and the route also changes by links. A value no option has shows
    // none chosen.
    let select = NodeRef::<leptos::html::Select>::new();
    Effect::new(move |_| {
        let value = chosen.get().map(ScopeChoice::token).unwrap_or_default();
        choices.track();
        if let Some(select) = select.get() {
            select.set_value(&value);
        }
    });
    let is = move |choice: ScopeChoice| move || chosen.get() == Some(choice);
    let hint = move || listing.get().map(|section| Keeps::of(section).hint());
    view! {
        <span class="topbar-scope" class:elsewhere=move || listing.with(Option::is_none)>
            <label class="field scope-filter" title=hint>
                "scope "
                <select node_ref=select on:change=on_change>
                    <option value=ALL selected=is(ScopeChoice::All)>{name(ScopeChoice::All, "")}</option>
                    <option value=NONE selected=is(ScopeChoice::NoScope)>
                        {name(ScopeChoice::NoScope, "")}
                    </option>
                    {options}
                </select>
            </label>
        </span>
    }
}

/// The choice the four pages share, for as long as the page lives: the one the last of their
/// routes named, or the one the page picked to open on. None until one of them has shown.
fn kept() -> ArcRwSignal<Option<ScopeChoice>> {
    static KEPT: OnceLock<ArcRwSignal<Option<ScopeChoice>>> = OnceLock::new();
    KEPT.get_or_init(|| ArcRwSignal::new(None)).clone()
}

/// What the page does with its choice when the route has changed to `here`, the page holding
/// `kept` ([`follow`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Settle {
    /// The route's page lists nothing by scope: the choice stays as it is, for the nav's links.
    Stay,
    /// The route names this choice, which is the page's from now on.
    Take(ScopeChoice),
    /// The route names none: put the page's choice, this one, in it.
    FillIn(ScopeChoice),
    /// The route names none and the page has no choice yet: pick the one it opens on, once the
    /// daemon has answered, and put that in it.
    Pick,
}

fn settle(here: &Route, kept: Option<ScopeChoice>) -> Settle {
    if !here.filtered() {
        return Settle::Stay;
    }
    match (here.scope(), kept) {
        (Some(named), _) => Settle::Take(named),
        (None, Some(kept)) => Settle::FillIn(kept),
        (None, None) => Settle::Pick,
    }
}

/// Keeps the page's scope choice as the route changes, and answers the choice the page showing
/// lists by, which the header's links and select show on every page. A choice the route names is
/// the page's from then on. A route of the four pages that names none takes the page's choice,
/// which the page puts in it in place of the route that named none, without a new step in the
/// browser's history, so a reload and a link copied from the address bar keep it; while the page
/// has none yet, it picks the one it opens on ([`start_on`]) once the daemon has answered the
/// standing queries (their first answer, from the browser's own storage, which starts empty, would
/// say the project had no scopes). Called once, by the shell.
pub fn follow(route: ArcRwSignal<Route>) -> Memo<Option<ScopeChoice>> {
    let kept = RwSignal::from(kept());
    Effect::new({
        let route = route.clone();
        move |_| {
            let here = route.get();
            match settle(&here, kept.get_untracked()) {
                Settle::Stay => {}
                Settle::Take(named) => {
                    if kept.get_untracked() != Some(named) {
                        kept.set(Some(named));
                    }
                }
                Settle::FillIn(choice) => route::replace(here.with_scope(choice)),
                Settle::Pick => {
                    if store::standing_loaded() {
                        let choice = untrack(|| start_on(&here));
                        kept.set(Some(choice));
                        route::replace(here.with_scope(choice));
                    }
                }
            }
        }
    });
    // The route's choice, else the page's: the page's until the page has put it in the route, so
    // that a page reached by a route that names none lists by it at once.
    Memo::new(move |_| route.with(Route::scope).or_else(|| kept.get()))
}

/// The choice a page opens on when its route names none and the page has none yet. A route naming
/// a task the page holds opens on a scope it is worked in, the first of them (`store::task_scopes`),
/// or on "no scope" when it is worked in none, so that the Tasks page lists it and the Graph draws
/// it; one naming a decision attached to a scope opens on that scope, so that the Decisions page
/// lists its line; any other opens on the scope [`opening_scope`] picks for the principal the page
/// acts as. A question the route names is listed whatever the scope, so it has no say.
fn start_on(here: &Route) -> ScopeChoice {
    let named = match here {
        Route::Tasks { task: Some(task), .. } | Route::Graph { task: Some(task), .. } => store::task(*task)
            .map(|_| store::scopes_of_task(*task).first().copied().map_or(ScopeChoice::NoScope, ScopeChoice::Scope)),
        Route::Decisions { decision: Some(decision), .. } => {
            store::decision(*decision).and_then(|d| ops::governs(&d).ok().flatten()).map(ScopeChoice::Scope)
        }
        _ => None,
    };
    named.unwrap_or_else(opening_scope)
}

/// The choice a page opens on when neither its route nor what the route names says:
/// [`starting_scope`] of the scopes and tasks the page holds, for the principal it acts as: the
/// scopes their own tasks in progress are worked in, each of a task's scopes with when that task
/// last changed.
fn opening_scope() -> ScopeChoice {
    let me = session::me();
    let working: Vec<(i64, EntityId)> = store::tasks()
        .iter()
        .filter(|t| me.is_some() && t.status().ok() == Some(TaskStatus::InProgress))
        .filter(|t| ops::assignee_of(t).ok().flatten() == me)
        .flat_map(|t| {
            let changed = t.updated().unwrap_or(0);
            store::scopes_of_task(t.id()).into_iter().map(move |scope| (changed, scope))
        })
        .collect();
    let scopes: Vec<ScopeFacts> = store::scopes()
        .iter()
        .map(|s| ScopeFacts {
            id: s.id(),
            active: s.status().ok() == Some(ScopeStatus::Active),
            started: s.started().ok().flatten(),
        })
        .collect();
    starting_scope(&scopes, &working)
}

/// A scope, as `starting_scope` weighs it.
struct ScopeFacts {
    id: EntityId,
    active: bool,
    /// When it first left planned; none while it is planned.
    started: Option<i64>,
}

/// The scope a page opens on, so that it opens on work under way: a scope the acting principal's
/// own tasks in progress are worked in, since they are those tasks' assignee (docs/DECISIONS.md,
/// D45), `working` holding each such task's last change with each of its scopes (the scope of the
/// one changed last, should they be in several, and of its scopes the last by id, so that every
/// page picks the same); else the active scope started most
/// recently, whoever made it; else all scopes. A scope has no owner (D108, "A scope has no owner:
/// scopes are not assigned to principals"), so nothing here asks whose a scope is: until
/// 2026-09-29 the page opened on the acting principal's own active scope, else the active scope
/// with the latest heartbeat or start, else only while no scope was active the scope with the
/// latest heartbeat or start (task TdAroscT, "Open the Graph page on an active scope, not the
/// newest one, for a viewer without a scope"). A scope made planned has no start, and only
/// active scopes count, so a scope made for later work never becomes where the page opens.
fn starting_scope(scopes: &[ScopeFacts], working: &[(i64, EntityId)]) -> ScopeChoice {
    if let Some((_, scope)) = working.iter().max() {
        return ScopeChoice::Scope(*scope);
    }
    let latest = scopes.iter().filter(|s| s.active).max_by_key(|s| (s.started, s.id));
    latest.map_or(ScopeChoice::All, |s| ScopeChoice::Scope(s.id))
}

#[cfg(test)]
mod tests {
    use super::ScopeChoice::{self, *};
    use super::{name, settle, starting_scope, Keeps, ScopeFacts, Settle};
    use crate::route::{Route, ScopePanel};
    use ankurah::EntityId;

    fn id(n: u8) -> EntityId {
        EntityId::from_bytes([n; 32])
    }

    fn scope(n: u8, active: bool, started: Option<i64>) -> ScopeFacts {
        ScopeFacts { id: id(n), active, started }
    }

    #[test]
    fn the_page_opens_on_the_scope_the_acting_principals_tasks_in_progress_are_worked_in() {
        let scopes = [scope(1, true, Some(90)), scope(2, true, Some(10)), scope(3, false, Some(5))];
        // A scope started earlier than another, or one not active, still wins while their task is
        // in progress there: it is where their work is under way.
        assert_eq!(starting_scope(&scopes, &[(50, id(2))]), Scope(id(2)));
        assert_eq!(starting_scope(&scopes, &[(50, id(3))]), Scope(id(3)));
        // Of tasks in progress in several scopes, the scope of the one changed last.
        assert_eq!(starting_scope(&scopes, &[(50, id(2)), (70, id(3)), (60, id(1))]), Scope(id(3)));
        // One task worked in two scopes counts in both, and of those the last by id, so that every
        // page picks the same.
        assert_eq!(starting_scope(&scopes, &[(70, id(1)), (70, id(2)), (60, id(3))]), Scope(id(2)));
    }

    #[test]
    fn with_no_task_in_progress_the_page_opens_on_the_active_scope_started_most_recently() {
        // Whoever made it: nothing says whose a scope is. A scope not active does not count,
        // whenever it started, and a planned one has not started.
        let scopes =
            [scope(1, true, Some(10)), scope(2, true, Some(20)), scope(3, false, Some(50)), scope(4, false, None)];
        assert_eq!(starting_scope(&scopes, &[]), Scope(id(2)));
        // An active scope that never recorded a start still comes before one not active.
        assert_eq!(starting_scope(&[scope(1, true, None), scope(2, false, Some(30))], &[]), Scope(id(1)));
    }

    #[test]
    fn making_a_planned_scope_leaves_the_page_where_it_opened() {
        // A scope made planned has no start and is not active, so it never becomes where the page
        // opens (task TdAroscT), however recently it was made.
        let before = || vec![scope(1, true, Some(20)), scope(2, false, Some(10))];
        let mut after = before();
        after.push(scope(3, false, None));
        assert_eq!(starting_scope(&before(), &[]), Scope(id(1)));
        assert_eq!(starting_scope(&after, &[]), Scope(id(1)));
    }

    #[test]
    fn with_no_scope_active_and_no_task_in_progress_the_page_opens_on_all() {
        assert_eq!(starting_scope(&[scope(1, false, Some(10)), scope(2, false, Some(30))], &[]), All);
        assert_eq!(starting_scope(&[], &[]), All);
    }

    #[test]
    fn the_choices_read_alike_on_every_page_which_says_on_hover_what_it_keeps() {
        assert_eq!(name(All, ""), "all scopes");
        assert_eq!(name(NoScope, ""), "no scope");
        assert_eq!(name(Scope(id(1)), "Cloud Hosting"), "Cloud Hosting");
        assert_eq!(Keeps::of("tasks"), Keeps::Tasks);
        assert_eq!(Keeps::of("graph"), Keeps::Tasks);
        assert_eq!(Keeps::of("questions"), Keeps::Questions);
        assert_eq!(Keeps::of("decisions"), Keeps::Rulings);
        assert!(Keeps::Tasks.hint().contains("tasks"));
        assert!(Keeps::Questions.hint().contains("questions"));
        // The Decisions page keeps the rulings for the whole project whatever the choice, and says so.
        assert!(Keeps::Rulings.hint().contains("govern the whole project"));
    }

    #[test]
    fn a_route_that_names_a_choice_makes_it_the_pages_and_one_that_names_none_takes_the_pages() {
        let (task, a, b) = (id(1), Scope(id(2)), Scope(id(3)));
        // Named: the route's choice is the page's from then on, whatever the page held.
        let named = Route::task(task).with_scope(b);
        assert_eq!(settle(&named, Some(a)), Settle::Take(b));
        assert_eq!(settle(&named, None), Settle::Take(b));
        // None named: the page's choice goes in; while the page has none, it picks one.
        for bare in [
            Route::task(task),
            Route::Tasks { task: None, scope: None },
            Route::NewTask { scope: None },
            Route::decision(task),
            Route::Graph { task: None, scope: None },
            Route::question(task),
            Route::Questions { question: None, whose: Some("claude".into()), scope: None },
        ] {
            assert_eq!(settle(&bare, Some(a)), Settle::FillIn(a), "{}", bare.hash());
            assert_eq!(settle(&bare, None), Settle::Pick, "{}", bare.hash());
        }
        // A page that lists nothing by scope leaves the choice as it is, for the nav's links.
        for elsewhere in
            [Route::inbox(), Route::Scopes(None), Route::Scopes(Some(ScopePanel::New)), Route::Anchor(task)]
        {
            assert_eq!(settle(&elsewhere, Some(a)), Settle::Stay, "{}", elsewhere.hash());
            assert_eq!(settle(&elsewhere, None), Settle::Stay, "{}", elsewhere.hash());
        }
    }

    #[test]
    fn each_choice_keeps_its_own_tasks() {
        let (a, b, c) = (id(1), id(2), id(3));
        assert!(All.keeps(&[a]) && All.keeps(&[]) && All.keeps(&[a, b]));
        assert!(NoScope.keeps(&[]) && !NoScope.keeps(&[a]) && !NoScope.keeps(&[a, b]));
        assert!(Scope(a).keeps(&[a]) && !Scope(a).keeps(&[b]) && !Scope(a).keeps(&[]));
        // A task worked in several scopes is kept under each of them, and under no other
        // (docs/DECISIONS.md, D136, "A task may be worked in several scopes").
        assert!(Scope(a).keeps(&[a, b]) && Scope(b).keeps(&[a, b]) && !Scope(c).keeps(&[a, b]));
    }

    #[test]
    fn a_choice_reads_back_as_it_was_written_and_nothing_else_reads_as_one() {
        let a = EntityId::from_bytes([1; 32]);
        for choice in [All, NoScope, Scope(a)] {
            assert_eq!(ScopeChoice::parse(&choice.token()), Some(choice), "{}", choice.token());
        }
        assert_eq!(All.token(), "all");
        assert_eq!(NoScope.token(), "none");
        assert_eq!(ScopeChoice::parse("unassigned"), Some(NoScope), "as a route wrote it until 2026-10-03");
        assert_eq!(ScopeChoice::parse(""), None);
        assert_eq!(ScopeChoice::parse("nonsense"), None);
    }
}
