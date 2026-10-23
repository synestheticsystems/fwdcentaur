//! Back on the panels of the Tasks and Decisions pages in the VS Code webview (task cD1PloSQ, "Open
//! a clicked task full screen, with a back button, in the VS Code webview"; Daniel, 2026-09-28:
//! "btw, full screen (with back button) should be the default for when you click on a task in
//! vscode webview (not for the web browser view though)"). The webview is narrow, so a task opened
//! beneath the list gets half the view there, and VS Code gives the page no back button of its own.
//! So in the webview a task opens with its panel filling the page, the list hidden: a task clicked
//! anywhere on the page (a row of the Tasks list, a node of the Graph, which leads to the Tasks
//! page there, filtered to the Graph's scope, a task's lozenge, an inbox card), and a task VS Code
//! shows the page at. A decision a lozenge on a task's panel opens fills the Decisions page the
//! same way, with Back to the task (task toDjwytu, "Carry the Graph's scope filter and a page's
//! state through Back in the webview"), and so does a decision opened from another page, an inbox
//! card, the Graph page's panel or a question, with Back to that page (task lccNMCh_, "Give Back
//! to a decision opened from the Inbox, the Graph or the Questions page, and keep the Tasks and
//! Decisions lists' own filters through a Back that crosses pages, in the VS Code webview"). The
//! panel's Back control returns to where its task or decision was opened from: the list, the panel
//! closed; the Graph, the Inbox, or the Tasks or Decisions page's list, as the person left them
//! (`page_state.rs`); another page as its route had it; or, when a lozenge on another panel opened
//! it, that panel, filling the page again if it did. The page knows it runs in the webview by
//! `?lock=1` (`session::locked`). In a browser none of this runs: a task opens beside the list, and
//! the browser's own back button goes back.
//!
//! The page keeps its own trail of the panels it opened, one from another, rather than go back
//! through the window's history (`history.back()`), for three reasons. The webview's document is a
//! frame inside VS Code's window, whose history every frame there adds to (another extension's
//! webview, say), so `history.back()` could go back in one of those instead of in the page. The
//! page cannot read that history, so it could not say where Back leads, nor tell a task VS Code
//! showed it at, which has nothing in the page behind it, from one opened from the list. And the
//! history holds routes, not whether a panel filled the page, which Back to a panel puts back.
//!
//! Where Back leads is worked out as data ([`Trail`]) and tested natively; [`follow`],
//! [`follow_decisions`] and [`BackButton`] tie it to the pages.

use crate::panel::PanelSize;
use crate::route::{self, Route};
use crate::scope_filter::ScopeChoice;
use crate::{decision_row, task_row};
use ankurah::EntityId;
use leptos::prelude::*;
use std::sync::OnceLock;

/// The most panels the trail keeps; past it the oldest goes, and Back from the oldest one kept
/// leads to its page's list.
const LONGEST: usize = 50;

/// A panel the trail holds: a task's, on the Tasks page, or a decision's, on the Decisions page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Panel {
    Task(EntityId),
    Decision(EntityId),
}

impl Panel {
    /// The panel `route` shows, when it shows one.
    fn of(route: &Route) -> Option<Panel> {
        match route {
            Route::Tasks { task: Some(task), .. } => Some(Panel::Task(*task)),
            Route::Decisions { decision: Some(decision), .. } => Some(Panel::Decision(*decision)),
            _ => None,
        }
    }
}

/// A panel the page opened, and what Back on it returns to.
#[derive(Clone, Debug, PartialEq)]
struct Opened {
    panel: Panel,
    /// The route the page was on when the panel opened; none when nothing in the page is behind
    /// it: the page opened on the task (VS Code showed it there, or showed it again after dropping
    /// it while hidden), or the trail, holding `LONGEST` panels, let go of the one before it.
    from: Option<Route>,
    /// Whether it fills the page, as it last did while it showed, which Back to it puts back.
    full: bool,
}

/// The panels the page opened one from another, the one it shows last, and where Back last led.
/// One trail serves the whole page, so that it lasts from a task's panel to a decision's and back:
/// a panel opened from the one on top of the trail goes on top of it, and a task opened from
/// anywhere else (the list, another page, nothing), or a decision opened from another page, begins
/// it anew. The list, the form for a new task or the Decisions page's list taking the page empties
/// it: Back is on a panel only, so from there nothing behind could be gone back to. A reload starts
/// the page without one.
#[derive(Debug, Default)]
struct Trail {
    opened: Vec<Opened>,
    /// Where Back last led, until the page gets there: that page, or that panel, comes back as it
    /// was left.
    returning: Option<Route>,
}

impl Trail {
    /// The page came to `panel` from the route `from`, while its panel filled the page or not
    /// (`full`); answers whether the panel fills the page from now on. Coming back to a panel with
    /// Back ([`Trail::back`]) puts it as it was when the page went on from it. Any other way to a
    /// task opens it filling the page. A decision goes on the trail when the panel on top of it
    /// opened the decision: filling the page when that is a task's panel, as a task opened from
    /// another does, and otherwise leaving the page as it was, since the Decisions page's full
    /// screen lasts through a change of decision. A decision opened from another page (an inbox
    /// card, the Graph page's panel, a question) begins the trail anew and fills the page, as a
    /// task opened from there does. One opened from the Decisions page itself (its list, or a
    /// decision there without Back) or from nothing (VS Code showing the page at it) has no Back,
    /// and the trail is let go of, since the page has moved on from every panel on it. A decision
    /// without Back whose lozenge opened a task comes back from Back on the task without Back
    /// still, though it comes back from another page.
    fn arrive(&mut self, panel: Panel, from: Option<Route>, full: bool) -> bool {
        let returning = self.returning.take();
        let top = self.opened.last().map(|o| o.panel);
        let came_back = returning.as_ref().and_then(Panel::of) == Some(panel);
        if came_back && top == Some(panel) {
            return self.opened.last().map_or(full, |o| o.full);
        }
        let from_top = top.is_some() && from.as_ref().and_then(Panel::of) == top;
        let from_another_page = !came_back && from.as_ref().is_some_and(|from| from.section() != "decisions");
        let fills = match (panel, top) {
            (Panel::Task(_), _) => true,
            (Panel::Decision(_), Some(Panel::Task(_))) if from_top => true,
            (Panel::Decision(_), _) if from_top => full,
            (Panel::Decision(_), _) if from_another_page => true,
            (Panel::Decision(_), _) => {
                self.opened.clear();
                return full;
            }
        };
        if !from_top {
            self.opened.clear();
        }
        if self.opened.len() >= LONGEST {
            self.opened.remove(0);
            if let Some(oldest) = self.opened.first_mut() {
                oldest.from = None;
            }
        }
        self.opened.push(Opened { panel, from, full: fills });
        fills
    }

    /// The panel `shown`, on top of the trail, fills the page or not (`full`), as the person made
    /// it do with the full screen toggle or Escape: Back to it puts it so again.
    fn fill(&mut self, shown: Panel, full: bool) {
        if let Some(top) = self.opened.last_mut().filter(|o| o.panel == shown) {
            top.full = full;
        }
    }

    /// Whether `shown`'s panel is on top of the trail, where Back on it knows where it was opened
    /// from: a task's panel always is, and a decision's when a panel on the trail or another page
    /// opened it.
    fn holds(&self, shown: Panel) -> bool {
        self.opened.last().is_some_and(|o| o.panel == shown)
    }

    /// Where Back on `shown`'s panel leads: the route the panel was opened from, else `list`, its
    /// page's list as the page filters it. A task made with the form for a new task leads back to
    /// the list the form was over, not to the form, which is done with once the task is made.
    fn leads(&self, shown: Panel, list: Route) -> Route {
        match self.opened.last() {
            Some(Opened { panel, from: Some(from), .. }) if *panel == shown => match from {
                Route::NewTask { scope } => Route::tasks(None, *scope),
                from => from.clone(),
            },
            _ => list,
        }
    }

    /// Goes back from `shown`'s panel: lets go of it and answers where Back leads
    /// ([`Trail::leads`]), which the page then brings back as it was left.
    fn back(&mut self, shown: Panel, list: Route) -> Route {
        let to = self.leads(shown, list);
        if self.holds(shown) {
            self.opened.pop();
        }
        self.returning = Some(to.clone());
        to
    }

    /// Whether Back led to the page `section` names (`Route::section`); where Back led is
    /// forgotten either way, so that the next visit to that page starts over.
    fn brought_back(&mut self, section: &str) -> bool {
        self.returning.take().is_some_and(|to| to.section() == section)
    }

    /// Whether Back led to the page `section` names and the page has yet to get there: its panel
    /// yet to arrive ([`Trail::arrive`]), or its list to take the page ([`Trail::clear`]), either
    /// of which forgets where Back led.
    fn returning_to(&self, section: &str) -> bool {
        self.returning.as_ref().is_some_and(|to| to.section() == section)
    }

    /// The list, the form for a new task or the Decisions page's list has the page: every panel on
    /// the trail is behind it.
    fn clear(&mut self) {
        self.opened.clear();
        self.returning = None;
    }
}

/// The page's one trail, for as long as the page lives.
fn trail() -> ArcRwSignal<Trail> {
    static TRAIL: OnceLock<ArcRwSignal<Trail>> = OnceLock::new();
    TRAIL.get_or_init(|| ArcRwSignal::new(Trail::default())).clone()
}

/// Whether Back led to the page now shown, which `section` names (`Route::section`). The Graph
/// and the Inbox ask once, as they open, so that they come back as the person left them when Back
/// led there (`page_state.rs`), and start over when anything else did.
pub fn brought_back(section: &str) -> bool {
    trail().try_update_untracked(|t| t.brought_back(section)).unwrap_or(false)
}

/// Whether Back led to the page now opening, which `section` names (`Route::section`), without
/// forgetting where it led: the page's panel still needs it when it arrives, once the page is
/// built ([`follow`], [`follow_decisions`]). The Tasks and Decisions pages' lists ask once, as
/// they are built, so that they come back with the filters the person left them with when Back
/// led there (`page_state.rs`), and start over when anything else did.
pub fn returning_to(section: &str) -> bool {
    trail().try_with_untracked(|t| t.returning_to(section)).unwrap_or(false)
}

/// A page whose panels go on the trail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Tasks,
    Decisions,
}

/// What the route shows of a page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shows {
    Panel(Panel),
    /// The page without a panel: its list, or the form for a new task.
    List,
    /// Another page: the route has moved on before the page has gone.
    Elsewhere,
}

impl Page {
    /// What a route in `section` (`Route::section`), with `selected` in its panel, shows of this
    /// page. A page takes no part once the route has moved on, so that the Tasks page, not gone
    /// yet when a lozenge on a task's panel takes the route to a decision, does not take the
    /// decision for a task.
    fn shows(self, section: &str, selected: Option<EntityId>) -> Shows {
        let (own, panel): (&str, fn(EntityId) -> Panel) = match self {
            Page::Tasks => ("tasks", Panel::Task),
            Page::Decisions => ("decisions", Panel::Decision),
        };
        if section != own {
            return Shows::Elsewhere;
        }
        selected.map_or(Shows::List, |id| Shows::Panel(panel(id)))
    }
}

impl Shows {
    fn panel(self) -> Option<Panel> {
        match self {
            Shows::Panel(panel) => Some(panel),
            Shows::List | Shows::Elsewhere => None,
        }
    }
}

/// Back on a page's panel: the page's trail and what the route shows of the page. `Copy` handles
/// only, so the page and its Back control share one.
#[derive(Clone, Copy)]
pub struct Back {
    trail: RwSignal<Trail>,
    shows: Memo<Shows>,
    /// Where Back leads when nothing on the trail is behind the panel shown: the page's list, as
    /// the page filters it.
    list: Signal<Route>,
}

impl Back {
    /// Whether the panel the page shows is on the trail, so that Back on it leads where it was
    /// opened from: every task's panel is, and a decision's when a panel on the trail or another
    /// page opened it.
    pub fn on_trail(self) -> bool {
        self.shows.get().panel().is_some_and(|shown| self.trail.with(|t| t.holds(shown)))
    }
}

/// Keeps the trail as the Tasks page's panel changes, `selected` being the task the panel shows
/// and `scope` the scope the list keeps: each task the panel comes to fills the page
/// (`size.full`), except one Back came back to, which is as it was when the page went on from it,
/// and the list or the form for a new task taking the page empties the trail. Called in the page's
/// component, in the webview only.
pub fn follow(size: PanelSize, selected: Memo<Option<EntityId>>, scope: Memo<Option<ScopeChoice>>) -> Back {
    follow_page(Page::Tasks, size, selected, Signal::derive(move || Route::tasks(None, scope.get())))
}

/// Keeps the trail as the Decisions page's panel changes, `selected` being the decision the panel
/// shows and `scope` the list's scope filter: a decision a panel on the trail opened joins it, one
/// opened from another page begins it anew ([`Trail::arrive`] says whether either fills the page),
/// one opened any other way has no Back, and the list alone taking the page empties the trail.
/// Called in the page's component, in the webview only.
pub fn follow_decisions(size: PanelSize, selected: Memo<Option<EntityId>>, scope: Memo<Option<ScopeChoice>>) -> Back {
    let list = Signal::derive(move || Route::Decisions { decision: None, scope: scope.get() });
    follow_page(Page::Decisions, size, selected, list)
}

/// Keeps the trail as `page`'s panel changes ([`follow`], [`follow_decisions`]).
fn follow_page(page: Page, size: PanelSize, selected: Memo<Option<EntityId>>, list: Signal<Route>) -> Back {
    let trail = RwSignal::from(trail());
    let route = route::route();
    let shows = Memo::new(move |_| page.shows(route.with(Route::section), selected.get()));
    // A panel the page comes to joins the trail, or comes back as it was, and the page's list
    // empties the trail.
    Effect::new(move |_| match shows.get() {
        Shows::Panel(panel) => {
            let full = size.full.get_untracked();
            let fills = trail.try_update(|t| t.arrive(panel, route::previous(), full)).unwrap_or(full);
            size.full.set(fills);
        }
        Shows::List => trail.update(Trail::clear),
        Shows::Elsewhere => {}
    });
    // Whether the panel fills the page, recorded as it changes, for Back to put back: by the time
    // the page goes on to another page's panel, this page's size is gone. Not as the page opens,
    // when its size is new and says nothing of the panel Back may be bringing back.
    Effect::new(move |opened: Option<()>| {
        let full = size.full.get();
        if opened.is_none() {
            return;
        }
        if let Some(shown) = shows.get_untracked().panel() {
            trail.update_untracked(|t| t.fill(shown, full));
        }
    });
    Back { trail, shows, list }
}

/// The control at the head of a panel that goes back to where its task or decision was opened
/// from ([`Trail::leads`]), which it names on hover and to a screen reader.
#[component]
pub fn BackButton(back: Back) -> impl IntoView {
    let to = Memo::new(move |_| {
        let shown = back.shows.get().panel()?;
        let list = back.list.get();
        Some(back.trail.with(|t| t.leads(shown, list)))
    });
    let name = move || to.get().map(|to| format!("Back to {}", place(&to))).unwrap_or_default();
    let go_back = move |_| {
        let Some(shown) = back.shows.get_untracked().panel() else { return };
        let list = back.list.get_untracked();
        if let Some(to) = back.trail.try_update(|t| t.back(shown, list)) {
            route::go(to);
        }
    };
    view! {
        <button class="btn subtle panel-back" title=name aria-label=name on:click=go_back>"← Back"</button>
    }
}

/// Where a route takes the page, in a few words for the Back control: a task by its short id and
/// title, a decision by its number and title, a page by its name.
fn place(to: &Route) -> String {
    let page = match to {
        Route::Tasks { task: Some(task), .. } => {
            return task_row::name(*task).unwrap_or_else(|| "the task".into());
        }
        Route::Decisions { decision: Some(decision), .. } => {
            return decision_row::name(*decision).unwrap_or_else(|| "the decision".into());
        }
        Route::Tasks { task: None, .. } | Route::NewTask { .. } => "the list",
        Route::Inbox { .. } => "the Inbox",
        Route::Questions { .. } => "the Questions page",
        Route::Decisions { decision: None, .. } => "the Decisions page",
        Route::Graph { .. } => "the Graph",
        Route::Scopes(_) => "the Scopes page",
        Route::Anchor(_) => "the highlight",
    };
    page.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> EntityId {
        EntityId::from_bytes([n; 32])
    }

    fn task(n: u8) -> Panel {
        Panel::Task(id(n))
    }

    fn decision(n: u8) -> Panel {
        Panel::Decision(id(n))
    }

    /// The route of a panel, with the scope on all scopes (everything, on the Decisions page).
    fn at(panel: Panel) -> Route {
        match panel {
            Panel::Task(task) => Route::Tasks { task: Some(task), scope: Some(ScopeChoice::All) },
            Panel::Decision(decision) => Route::Decisions { decision: Some(decision), scope: Some(ScopeChoice::All) },
        }
    }

    /// The Tasks page's list, on all scopes.
    fn list() -> Route {
        Route::Tasks { task: None, scope: Some(ScopeChoice::All) }
    }

    /// The Decisions page's list, on everything.
    fn decisions() -> Route {
        Route::Decisions { decision: None, scope: Some(ScopeChoice::All) }
    }

    fn graph() -> Route {
        Route::Graph { task: None, scope: Some(ScopeChoice::Scope(id(90))) }
    }

    #[test]
    fn a_task_opened_from_the_list_goes_back_to_the_list_with_the_panel_closed() {
        let (a, scope) = (task(1), id(9));
        let mut trail = Trail::default();
        assert!(trail.arrive(a, Some(list()), false), "it opens filling the page");
        assert_eq!(trail.back(a, list()), list());
        // A list filtered to a scope stays filtered.
        let mut trail = Trail::default();
        let filtered = Route::scope_tasks(scope);
        trail.arrive(a, Some(filtered.clone()), false);
        assert_eq!(trail.back(a, filtered.clone()), filtered);
    }

    #[test]
    fn back_brings_a_page_back_with_the_scope_its_route_had_whatever_was_chosen_since() {
        // A from the Graph drawing scope 90; on A's page the person then chose scope 91, which
        // moves the route's scope and leaves the panel where it was.
        let a = task(1);
        let mut trail = Trail::default();
        trail.arrive(a, Some(graph()), false);
        let chosen_since = Route::Tasks { task: None, scope: Some(ScopeChoice::Scope(id(91))) };
        assert_eq!(trail.back(a, chosen_since), graph(), "the Graph comes back on scope 90");
    }

    #[test]
    fn a_task_opened_from_another_page_goes_back_to_that_page_as_its_route_had_it() {
        let a = task(1);
        let decision = Route::Decisions { decision: Some(id(5)), scope: Some(ScopeChoice::All) };
        for from in [Route::inbox(), graph(), Route::question(id(4)), decision] {
            let mut trail = Trail::default();
            assert!(trail.arrive(a, Some(from.clone()), false), "from {from:?} it opens filling the page");
            assert_eq!(trail.leads(a, list()), from, "the control names where it goes before it goes");
            assert_eq!(trail.back(a, list()), from);
        }
    }

    #[test]
    fn a_task_opened_from_another_tasks_panel_goes_back_to_that_task_as_full_as_it_was() {
        let (a, b, c) = (task(1), task(2), task(3));
        let mut trail = Trail::default();
        trail.arrive(a, Some(list()), false);
        // A's lozenge naming B, clicked while A fills the page; then B's naming C, clicked once
        // Restore has given the list its place back.
        assert!(trail.arrive(b, Some(at(a)), true));
        trail.fill(b, false);
        assert!(trail.arrive(c, Some(at(b)), false), "C opens filling the page all the same");
        assert_eq!(trail.back(c, list()), at(b));
        assert!(!trail.arrive(b, Some(at(c)), true), "back at B, the list shows again, as it did");
        assert_eq!(trail.back(b, list()), at(a));
        assert!(trail.arrive(a, Some(at(b)), false), "back at A, it fills the page again");
        assert_eq!(trail.back(a, list()), list(), "and from A, where A was opened from");
        assert!(trail.opened.is_empty());
    }

    #[test]
    fn a_task_reached_again_by_a_click_is_opened_anew_not_gone_back_to() {
        // A, then B from A's panel, then A again from B's: Back walks the three back in turn.
        let (a, b) = (task(1), task(2));
        let mut trail = Trail::default();
        trail.arrive(a, Some(Route::inbox()), false);
        trail.arrive(b, Some(at(a)), true);
        assert!(trail.arrive(a, Some(at(b)), true));
        assert_eq!(trail.back(a, list()), at(b));
        assert!(trail.arrive(b, Some(at(a)), false));
        assert_eq!(trail.back(b, list()), at(a));
        assert!(trail.arrive(a, Some(at(b)), false));
        assert_eq!(trail.back(a, list()), Route::inbox());
    }

    #[test]
    fn a_task_opened_from_another_page_begins_the_trail_anew() {
        // A from the Graph; the page then goes to the Inbox by the nav, not by Back, where a card
        // opens A again: Back leads to the Inbox, and nothing is left of the Graph's trail.
        let a = task(1);
        let mut trail = Trail::default();
        trail.arrive(a, Some(graph()), false);
        trail.arrive(task(2), Some(at(a)), true);
        assert!(trail.arrive(a, Some(Route::inbox()), false), "it opens filling the page");
        assert_eq!(trail.back(a, list()), Route::inbox());
        assert!(trail.opened.is_empty());
    }

    #[test]
    fn with_nothing_behind_the_task_back_leads_to_the_list_as_the_page_filters_it() {
        let (a, scope) = (task(1), id(9));
        let filtered = Route::scope_tasks(scope);
        // VS Code showed the page at the task, or showed it again after dropping it.
        let mut trail = Trail::default();
        assert!(trail.arrive(a, None, false));
        assert_eq!(trail.leads(a, filtered.clone()), filtered);
        assert_eq!(trail.back(a, list()), list());
        // A trail that holds nothing for the task shown, however that came about, leads there too.
        let mut trail = Trail::default();
        trail.arrive(task(2), Some(Route::inbox()), false);
        assert_eq!(trail.back(a, list()), list());
        assert_eq!(trail.opened.len(), 1, "and lets go of nothing");
        assert_eq!(Trail::default().leads(a, list()), list());
    }

    #[test]
    fn a_task_made_with_the_form_goes_back_to_the_list_the_form_was_over() {
        let (a, scope) = (task(1), id(9));
        let mut trail = Trail::default();
        trail.arrive(a, Some(Route::NewTask { scope: Some(ScopeChoice::Scope(scope)) }), false);
        assert_eq!(trail.back(a, list()), Route::scope_tasks(scope));
    }

    #[test]
    fn the_list_taking_the_page_leaves_nothing_behind() {
        let (a, b) = (task(1), task(2));
        let mut trail = Trail::default();
        trail.arrive(a, Some(list()), false);
        trail.arrive(b, Some(at(a)), true);
        trail.clear();
        // B opened from the list anew: Back leads to the list, not to A.
        assert!(trail.arrive(b, Some(list()), false));
        assert_eq!(trail.back(b, list()), list());
    }

    #[test]
    fn the_trail_keeps_the_latest_panels_and_back_from_the_oldest_kept_leads_to_the_list() {
        let mut trail = Trail::default();
        trail.arrive(task(0), Some(Route::inbox()), false);
        for n in 1..=LONGEST as u8 {
            trail.arrive(task(n), Some(at(task(n - 1))), true);
        }
        assert_eq!(trail.opened.len(), LONGEST);
        for n in (2..=LONGEST as u8).rev() {
            assert_eq!(trail.back(task(n), list()), at(task(n - 1)));
            assert!(trail.arrive(task(n - 1), Some(at(task(n))), false), "{n}");
        }
        assert_eq!(trail.back(task(1), list()), list(), "the panel before it is no longer kept");
        assert!(trail.opened.is_empty());
    }

    #[test]
    fn a_decision_opened_from_a_tasks_panel_has_back_to_the_task_which_still_leads_where_it_did() {
        let (a, d) = (task(1), decision(4));
        let mut trail = Trail::default();
        trail.arrive(a, Some(graph()), false);
        // A's lozenge naming D, clicked while A fills the page: the Decisions page, whose panel
        // does not fill the page yet, opens D filling it, as A did.
        assert!(trail.arrive(d, Some(at(a)), false));
        assert!(trail.holds(d), "D has Back");
        assert_eq!(trail.leads(d, decisions()), at(a));
        assert_eq!(trail.back(d, decisions()), at(a));
        // Back at A, on the Tasks page again, whose panel does not fill the page yet: A fills it,
        // as it did, and its Back still leads to the Graph.
        assert!(trail.arrive(a, Some(at(d)), false));
        assert_eq!(trail.back(a, list()), graph());
        assert!(trail.opened.is_empty());
    }

    #[test]
    fn a_decision_opened_from_the_decision_before_it_leaves_the_page_as_it_was() {
        let (a, d, e, f) = (task(1), decision(4), decision(5), decision(6));
        let mut trail = Trail::default();
        trail.arrive(a, Some(list()), false);
        assert!(trail.arrive(d, Some(at(a)), false));
        // Restore gives the Decisions page's list its place back beside D; a line of the list, or
        // an arrow, opens E beside it, with Back to D.
        trail.fill(d, false);
        assert!(!trail.arrive(e, Some(at(d)), false), "the list stays beside E");
        // Full screen on E, then a lozenge on E's panel naming F: F fills the page, as E did.
        trail.fill(e, true);
        assert!(trail.arrive(f, Some(at(e)), true));
        assert_eq!(trail.back(f, decisions()), at(e));
        assert!(trail.arrive(e, Some(at(f)), true), "back at E, full as it was");
        assert_eq!(trail.back(e, decisions()), at(d));
        assert!(!trail.arrive(d, Some(at(e)), true), "back at D, beside the list as it was");
        assert_eq!(trail.back(d, decisions()), at(a));
        assert!(trail.arrive(a, Some(at(d)), false), "and back at A, which fills the page as it did");
    }

    #[test]
    fn a_decision_opened_from_another_page_fills_the_page_with_back_to_that_page() {
        // From an inbox card, the Graph page's panel, a question, the Tasks list, the Scopes page.
        let d = decision(4);
        let graphs_panel = Route::Graph { task: Some(id(1)), scope: Some(ScopeChoice::Scope(id(90))) };
        for from in [Route::inbox(), graphs_panel, Route::question(id(3)), list(), Route::Scopes(None)] {
            let mut trail = Trail::default();
            trail.arrive(task(2), Some(Route::inbox()), false);
            assert!(trail.arrive(d, Some(from.clone()), false), "from {from:?} it opens filling the page");
            assert!(trail.holds(d), "with Back");
            assert_eq!(trail.opened.len(), 1, "and begins the trail anew");
            assert_eq!(trail.leads(d, decisions()), from, "the control names where it goes before it goes");
            assert_eq!(trail.back(d, decisions()), from);
            assert!(trail.opened.is_empty());
            assert!(trail.brought_back(from.section()), "and that page knows Back led there");
        }
    }

    #[test]
    fn back_from_a_task_a_decision_opened_leads_to_the_decision_and_on_to_the_page_it_was_opened_from() {
        let (d, a) = (decision(4), task(1));
        let mut trail = Trail::default();
        assert!(trail.arrive(d, Some(Route::inbox()), false));
        // A lozenge on D's panel, which fills the page, naming A.
        assert!(trail.arrive(a, Some(at(d)), true));
        assert_eq!(trail.back(a, list()), at(d));
        assert!(trail.arrive(d, Some(at(a)), false), "back at D, which fills the page as it did");
        assert!(trail.holds(d), "and still has Back");
        assert_eq!(trail.back(d, decisions()), Route::inbox());
        assert!(trail.brought_back("inbox"));
        assert!(trail.opened.is_empty());
    }

    #[test]
    fn a_decision_opened_from_the_decisions_page_itself_or_from_nothing_has_no_back_and_lets_go_of_the_trail() {
        let (a, b, d) = (task(1), task(2), decision(4));
        for from in [Some(decisions()), Some(at(decision(5))), None] {
            let mut trail = Trail::default();
            trail.arrive(a, Some(list()), false);
            assert!(!trail.arrive(d, from.clone(), false), "from {from:?} it opens as the page has it");
            assert!(trail.arrive(d, from.clone(), true), "whichever way the page has it");
            assert!(!trail.holds(d), "no Back");
            assert!(trail.opened.is_empty(), "the page has moved on from A");
        }
        // A task opened from that decision's panel has Back to the decision as its route had it,
        // which the decision, not on the trail, comes back to without a Back of its own, though
        // it comes back from another page.
        let mut trail = Trail::default();
        trail.arrive(d, Some(decisions()), false);
        trail.arrive(b, Some(at(d)), false);
        assert_eq!(trail.back(b, list()), at(d));
        assert!(!trail.arrive(d, Some(at(b)), false));
        assert!(!trail.holds(d));
    }

    #[test]
    fn back_to_another_page_is_known_there_until_its_panel_arrives_or_its_list_takes_the_page() {
        // A from the list, D from A's lozenge, and Back: the Tasks page, built again, asks for
        // its list's filters before A arrives, and A's arrival forgets where Back led.
        let (a, b, d) = (task(1), task(2), decision(4));
        let mut trail = Trail::default();
        assert!(!trail.returning_to("tasks"), "nothing has gone back yet");
        trail.arrive(a, Some(list()), false);
        trail.arrive(d, Some(at(a)), false);
        trail.back(d, decisions());
        assert!(trail.returning_to("tasks"));
        assert!(trail.returning_to("tasks"), "asking forgets nothing");
        assert!(!trail.returning_to("decisions"));
        assert!(trail.arrive(a, Some(at(d)), false), "A comes back filling the page, as it did");
        assert!(!trail.returning_to("tasks"), "the next visit to the Tasks page starts over");
        // B from the panel of a decision the Decisions page's list opened, and Back: the Decisions
        // page, built again, asks before the decision arrives.
        let mut trail = Trail::default();
        trail.arrive(d, Some(decisions()), false);
        trail.arrive(b, Some(at(d)), false);
        trail.back(b, list());
        assert!(trail.returning_to("decisions"));
        trail.arrive(d, Some(at(b)), false);
        assert!(!trail.returning_to("decisions"));
        // D from the form for a new task, and Back: the list the form was over takes the page.
        let mut trail = Trail::default();
        trail.arrive(d, Some(Route::NewTask { scope: Some(ScopeChoice::All) }), false);
        assert_eq!(trail.back(d, decisions()), list());
        assert!(trail.returning_to("tasks"));
        trail.clear();
        assert!(!trail.returning_to("tasks"));
    }

    #[test]
    fn back_to_the_graph_or_the_inbox_brings_that_page_back_once() {
        for (from, section) in [(graph(), "graph"), (Route::inbox(), "inbox")] {
            let mut trail = Trail::default();
            trail.arrive(task(1), Some(from.clone()), false);
            assert_eq!(trail.back(task(1), list()), from);
            assert!(trail.brought_back(section), "{section} opens as it was left");
            assert!(!trail.brought_back(section), "and the next visit to it starts over");
        }
        // Back to one page brings back no other, and once a page has asked, where Back led is
        // forgotten.
        let mut trail = Trail::default();
        trail.arrive(task(1), Some(Route::inbox()), false);
        trail.back(task(1), list());
        assert!(!trail.brought_back("graph"));
        assert!(!trail.brought_back("inbox"));
        // Nor does any other way to a page.
        assert!(!Trail::default().brought_back("graph"));
        let mut trail = Trail::default();
        trail.arrive(task(1), Some(graph()), false);
        trail.back(task(1), list());
        trail.clear();
        assert!(!trail.brought_back("graph"), "the list took the page after Back led to the Graph");
    }

    #[test]
    fn a_page_on_its_way_out_takes_no_part() {
        let (a, d) = (id(1), id(4));
        assert_eq!(Page::Tasks.shows("tasks", Some(a)), Shows::Panel(task(1)));
        assert_eq!(Page::Tasks.shows("tasks", None), Shows::List);
        assert_eq!(Page::Decisions.shows("decisions", Some(d)), Shows::Panel(decision(4)));
        assert_eq!(Page::Decisions.shows("decisions", None), Shows::List);
        // A lozenge on A's panel took the route to D before the Tasks page went: the page does not
        // take D for a task, nor empty the trail as if its list had the page.
        assert_eq!(Page::Tasks.shows("decisions", Some(d)), Shows::Elsewhere);
        assert_eq!(Page::Tasks.shows("graph", None), Shows::Elsewhere);
        assert_eq!(Page::Decisions.shows("tasks", Some(a)), Shows::Elsewhere);
    }
}
