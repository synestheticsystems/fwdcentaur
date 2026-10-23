//! Hash routes (`#/inbox`, `#/questions` and `#/questions/<id>`, which show the acting
//! principal's own, and with `?for=<handle>` after them another principal's, `#/tasks/<id>`,
//! `#/decisions/<id>`, `#/graph/<id>`, `#/scopes` with its panel for a new scope, `#/scopes/new`, or for editing one,
//! `#/scopes/<id>/edit`, a highlight's page `#/anchors/<id>`, and the form for a new task,
//! `#/tasks/new`; the routes of the Questions, Tasks, Decisions and Graph pages, the form's among
//! them, carry the header's scope choice last, `?scope=<id>`, `?scope=none` or `?scope=all`,
//! and `&scope=` after a `?for=`: `scope_filter.rs` says what each page keeps by it): the daemon
//! serves one page, so the path never changes and a reload lands where it was. A `hashchange` listener keeps the signal
//! current, and remembers the route the page left (`previous`); `go` sets the hash and lets the
//! listener do the rest, and `replace` does the same without a new step in the browser's history.

use crate::scope_filter::ScopeChoice;
use ankurah::EntityId;
use leptos::prelude::*;
use std::sync::{Mutex, OnceLock};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Route {
    /// The Inbox: the notifications of the principal the page acts as (the header's `as`), or,
    /// when `whose` names another principal (by its handle, or by its id where two or more
    /// principals share the handle), that principal's, to read without acting as them
    /// (`whose.rs`). The nav's link names no one, so it leads to one's own.
    Inbox { whose: Option<String> },
    /// The Questions page: the questions asked of the principal the page acts as, or of the one
    /// `whose` names (as the Inbox's does), about the scope `scope` chooses, with the question
    /// whose card is selected: where a lozenge naming a question, or an answer to one, leads.
    Questions { question: Option<EntityId>, whose: Option<String>, scope: Option<ScopeChoice> },
    /// The Tasks page, with the task its panel shows and the scope its list keeps the tasks of:
    /// with a scope's id, where a scope's name on the Scopes page and a scope's lozenge lead.
    Tasks { task: Option<EntityId>, scope: Option<ScopeChoice> },
    /// The Tasks page with the form for a new task in its panel, its list keeping `scope` as the
    /// page's did.
    NewTask { scope: Option<ScopeChoice> },
    /// The Decisions page, with the decision its panel shows and the rulings the scope keeps:
    /// every choice keeps the project's, and a scope adds its own.
    Decisions { decision: Option<EntityId>, scope: Option<ScopeChoice> },
    /// The Graph page, with the task its panel shows and the scope whose tasks it draws.
    Graph { task: Option<EntityId>, scope: Option<ScopeChoice> },
    /// The Scopes page, with the panel its route opens, if any.
    Scopes(Option<ScopePanel>),
    /// A highlight's own page, where its lozenge and a step naming it lead in a browser.
    Anchor(EntityId),
}

/// What the Scopes page's panel shows (`DECISIONS.md`, "A scope is created and edited in a side
/// panel opened from an Edit control; its name still leads to the filtered Tasks page").
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ScopePanel {
    /// A new scope's empty fields, where the "New scope" button leads.
    New,
    /// This scope's fields, to edit, where a row's Edit control leads.
    Edit(EntityId),
}

impl Route {
    /// The Inbox of the principal the page acts as.
    pub fn inbox() -> Route {
        Route::Inbox { whose: None }
    }

    /// The Questions page of the principal the page acts as, with `question`'s card selected, which
    /// shows there whomever it asks and whatever it is about: where a lozenge naming a question, or
    /// an answer to one, leads.
    pub fn question(question: EntityId) -> Route {
        Route::Questions { question: Some(question), whose: None, scope: None }
    }

    /// The Decisions page with `decision` in its panel: where a lozenge naming a ruling leads.
    pub fn decision(decision: EntityId) -> Route {
        Route::Decisions { decision: Some(decision), scope: None }
    }

    /// The Tasks page with `task` in its panel: where a link naming a task leads.
    pub fn task(task: EntityId) -> Route {
        Route::Tasks { task: Some(task), scope: None }
    }

    /// The Tasks page listing the tasks worked in `scope`: where a scope's name on the Scopes page
    /// and a scope's lozenge lead.
    pub fn scope_tasks(scope: EntityId) -> Route {
        Route::Tasks { task: None, scope: Some(ScopeChoice::Scope(scope)) }
    }

    /// The Tasks page with `task` in its panel, if any, its list keeping `scope`.
    pub fn tasks(task: Option<EntityId>, scope: Option<ScopeChoice>) -> Route {
        Route::Tasks { task, scope }
    }

    /// Whether the route's page lists by the header's scope choice: the Questions, Tasks (with
    /// the form for a new task too), Decisions and Graph pages.
    pub fn filtered(&self) -> bool {
        matches!(
            self,
            Route::Questions { .. }
                | Route::Tasks { .. }
                | Route::NewTask { .. }
                | Route::Decisions { .. }
                | Route::Graph { .. }
        )
    }

    /// The scope choice the route names. None on a page that does not list by scope, and on one
    /// that does when its route names none: a link that names a task or a ruling alone, or a page
    /// loaded on such a route, which the page fills in (`scope_filter::follow`).
    pub fn scope(&self) -> Option<ScopeChoice> {
        match self {
            Route::Questions { scope, .. }
            | Route::Tasks { scope, .. }
            | Route::NewTask { scope }
            | Route::Decisions { scope, .. }
            | Route::Graph { scope, .. } => *scope,
            Route::Inbox { .. } | Route::Scopes(_) | Route::Anchor(_) => None,
        }
    }

    /// The same route naming `choice` as its scope; the route of a page that does not list by
    /// scope as it is.
    pub fn with_scope(&self, choice: ScopeChoice) -> Route {
        let mut route = self.clone();
        match &mut route {
            Route::Questions { scope, .. }
            | Route::Tasks { scope, .. }
            | Route::NewTask { scope }
            | Route::Decisions { scope, .. }
            | Route::Graph { scope, .. } => *scope = Some(choice),
            Route::Inbox { .. } | Route::Scopes(_) | Route::Anchor(_) => {}
        }
        route
    }

    /// Whether this route is `before` with a scope choice put in where `before` named none: the
    /// page filling in its choice, which leaves the page where it was.
    pub fn fills_in(&self, before: &Route) -> bool {
        before.scope().is_none() && self.scope().is_some_and(|choice| before.with_scope(choice) == *self)
    }

    pub fn hash(&self) -> String {
        let id = |id: &Option<EntityId>| id.map(|id| format!("/{}", id.to_base64())).unwrap_or_default();
        // What follows the path: whose page shows, then the scope, after one `?` and joined by `&`.
        let query = |whose: &Option<String>, scope: &Option<ScopeChoice>| {
            let pairs: Vec<String> = whose
                .iter()
                .map(|handle| format!("for={handle}"))
                .chain(scope.iter().map(|s| format!("scope={}", s.token())))
                .collect();
            if pairs.is_empty() {
                String::new()
            } else {
                format!("?{}", pairs.join("&"))
            }
        };
        match self {
            Route::Inbox { whose } => format!("#/inbox{}", query(whose, &None)),
            Route::Questions { question, whose, scope } => {
                format!("#/questions{}{}", id(question), query(whose, scope))
            }
            Route::Tasks { task, scope } => format!("#/tasks{}{}", id(task), query(&None, scope)),
            Route::NewTask { scope } => format!("#/tasks/new{}", query(&None, scope)),
            Route::Decisions { decision, scope } => format!("#/decisions{}{}", id(decision), query(&None, scope)),
            Route::Graph { task, scope } => format!("#/graph{}{}", id(task), query(&None, scope)),
            Route::Scopes(None) => "#/scopes".into(),
            Route::Scopes(Some(ScopePanel::New)) => "#/scopes/new".into(),
            Route::Scopes(Some(ScopePanel::Edit(scope))) => format!("#/scopes/{}/edit", scope.to_base64()),
            Route::Anchor(anchor) => format!("#/anchors/{}", anchor.to_base64()),
        }
    }

    /// The nav entry this route lights up.
    pub fn section(&self) -> &'static str {
        match self {
            Route::Inbox { .. } => "inbox",
            Route::Questions { .. } => "questions",
            Route::Tasks { .. } | Route::NewTask { .. } => "tasks",
            Route::Decisions { .. } => "decisions",
            Route::Graph { .. } => "graph",
            Route::Scopes(_) => "scopes",
            // No nav entry: a highlight is reached from what names it.
            Route::Anchor(_) => "anchors",
        }
    }

    fn parse(hash: &str) -> Route {
        let path = hash.trim_start_matches('#').trim_start_matches('/');
        let (path, query) = path.split_once('?').unwrap_or((path, ""));
        let mut parts = path.split('/');
        let id = |s: Option<&str>| s.and_then(|s| s.parse::<EntityId>().ok());
        // The scope choice the route names, on a page that lists by scope; a word that is no
        // choice names none.
        let scope = query.split('&').find_map(|pair| pair.strip_prefix("scope=")).and_then(ScopeChoice::parse);
        // The handle of the principal whose Inbox or Questions page the route shows, when it names
        // one.
        let whose =
            query.split('&').find_map(|pair| pair.strip_prefix("for=")).filter(|h| !h.is_empty()).map(str::to_string);
        match parts.next() {
            Some("inbox") => Route::Inbox { whose },
            Some("questions") => Route::Questions { question: id(parts.next()), whose, scope },
            Some("tasks") => match parts.next() {
                Some("new") => Route::NewTask { scope },
                task => Route::Tasks { task: id(task), scope },
            },
            Some("decisions") => Route::Decisions { decision: id(parts.next()), scope },
            Some("graph") => Route::Graph { task: id(parts.next()), scope },
            Some("scopes") => match (parts.next(), parts.next()) {
                (Some("new"), _) => Route::Scopes(Some(ScopePanel::New)),
                (named, Some("edit")) => Route::Scopes(id(named).map(ScopePanel::Edit)),
                // A scope's own route, from when the Scopes page showed each scope's tasks beneath
                // it: those tasks, on the Tasks page.
                (named, _) => id(named).map_or(Route::Scopes(None), Route::scope_tasks),
            },
            // A highlight has a page; there is no list of them.
            Some("anchors") => id(parts.next()).map_or_else(Route::inbox, Route::Anchor),
            _ => Route::inbox(),
        }
    }
}

static ROUTE: OnceLock<ArcRwSignal<Route>> = OnceLock::new();
/// The route the page was on before the one `ROUTE` holds, once the page has moved.
static PREVIOUS: Mutex<Option<Route>> = Mutex::new(None);

/// The current route, reactively. The listener is installed on first use and lives as long as
/// the page.
pub fn route() -> ArcRwSignal<Route> {
    ROUTE
        .get_or_init(|| {
            let signal = ArcRwSignal::new(Route::parse(&current_hash()));
            let target = signal.clone();
            let listener = Closure::<dyn FnMut()>::new(move || {
                let next = Route::parse(&current_hash());
                let left = target.get_untracked();
                if next != left && !next.fills_in(&left) {
                    if let Ok(mut previous) = PREVIOUS.lock() {
                        *previous = Some(left);
                    }
                }
                target.set(next);
            });
            if let Some(w) = web_sys::window() {
                let _ = w.add_event_listener_with_callback("hashchange", listener.as_ref().unchecked_ref());
            }
            listener.forget();
            signal
        })
        .clone()
}

/// The route the page was on before it came to the one it is on; none until the page has moved.
/// The listener keeps it whatever moved the page: `go`, `replace`, a link, the browser's own back
/// button, or VS Code sending the webview's page to a route. The page putting its scope choice in
/// a route that named none (`scope_filter::follow`) does not move it. It is where a task was
/// opened from, which Back on the task's panel returns to in the VS Code webview (`back.rs`).
pub fn previous() -> Option<Route> {
    PREVIOUS.lock().ok()?.clone()
}

pub fn go(route: Route) {
    if let Some(w) = web_sys::window() {
        let _ = w.location().set_hash(&route.hash());
    }
}

/// Takes the page to `route` in place of the route it is on, so the browser's back button skips
/// the one replaced: for a page that fills in what its route left out, as the page does with its
/// scope choice (`scope_filter::follow`).
pub fn replace(route: Route) {
    if let Some(w) = web_sys::window() {
        let _ = w.location().replace(&route.hash());
    }
}

fn current_hash() -> String {
    web_sys::window().and_then(|w| w.location().hash().ok()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::Route::{self, *};
    use super::ScopePanel;
    use crate::scope_filter::ScopeChoice;
    use ankurah::EntityId;

    #[test]
    fn every_route_reads_back_as_it_was_written() {
        let (task, scope) = (EntityId::from_bytes([1; 32]), EntityId::from_bytes([2; 32]));
        for route in [
            Route::inbox(),
            Inbox { whose: Some("claude".into()) },
            Questions { question: None, whose: None, scope: None },
            Route::question(task),
            Questions { question: None, whose: Some("claude".into()), scope: None },
            Questions { question: Some(task), whose: Some("impl-alice".into()), scope: None },
            Questions { question: None, whose: None, scope: Some(ScopeChoice::Scope(scope)) },
            Questions { question: Some(task), whose: Some("claude".into()), scope: Some(ScopeChoice::NoScope) },
            Questions { question: None, whose: Some("claude".into()), scope: Some(ScopeChoice::All) },
            Tasks { task: None, scope: None },
            Route::task(task),
            Route::scope_tasks(scope),
            Tasks { task: Some(task), scope: Some(ScopeChoice::Scope(scope)) },
            Tasks { task: None, scope: Some(ScopeChoice::NoScope) },
            Tasks { task: Some(task), scope: Some(ScopeChoice::All) },
            NewTask { scope: None },
            NewTask { scope: Some(ScopeChoice::Scope(scope)) },
            NewTask { scope: Some(ScopeChoice::NoScope) },
            Decisions { decision: None, scope: None },
            Decisions { decision: Some(task), scope: None },
            Decisions { decision: None, scope: Some(ScopeChoice::Scope(scope)) },
            Decisions { decision: Some(task), scope: Some(ScopeChoice::NoScope) },
            Decisions { decision: None, scope: Some(ScopeChoice::All) },
            Graph { task: None, scope: None },
            Graph { task: Some(task), scope: None },
            Graph { task: None, scope: Some(ScopeChoice::Scope(scope)) },
            Graph { task: Some(task), scope: Some(ScopeChoice::Scope(scope)) },
            Graph { task: None, scope: Some(ScopeChoice::NoScope) },
            Graph { task: Some(task), scope: Some(ScopeChoice::All) },
            Scopes(None),
            Scopes(Some(ScopePanel::New)),
            Scopes(Some(ScopePanel::Edit(scope))),
            Anchor(task),
        ] {
            assert_eq!(Route::parse(&route.hash()), route, "{}", route.hash());
        }
    }

    #[test]
    fn the_four_pages_carry_one_scope_choice_after_the_rest_of_the_route() {
        let (item, scope) = (EntityId::from_bytes([1; 32]), EntityId::from_bytes([2; 32]));
        let (i, s) = (item.to_base64(), scope.to_base64());
        let chosen = ScopeChoice::Scope(scope);
        // The same words on every page: a scope's id, `none` or `all`.
        for (route, hash) in [
            (Tasks { task: None, scope: None }, "#/tasks".to_string()),
            (Route::task(item), format!("#/tasks/{i}")),
            (NewTask { scope: None }, "#/tasks/new".to_string()),
            (Route::decision(item), format!("#/decisions/{i}")),
            (Graph { task: Some(item), scope: None }, format!("#/graph/{i}")),
            (Route::question(item), format!("#/questions/{i}")),
        ] {
            assert!(route.filtered() && route.scope().is_none(), "{hash} names no scope");
            assert_eq!(route.hash(), hash);
            assert_eq!(route.with_scope(chosen).hash(), format!("{hash}?scope={s}"));
            assert_eq!(route.with_scope(ScopeChoice::NoScope).hash(), format!("{hash}?scope=none"));
            // As a link wrote it until 2026-10-03.
            assert_eq!(Route::parse(&format!("{hash}?scope=unassigned")), route.with_scope(ScopeChoice::NoScope));
            assert_eq!(route.with_scope(ScopeChoice::All).hash(), format!("{hash}?scope=all"));
            assert_eq!(Route::parse(&format!("{hash}?scope={s}")), route.with_scope(chosen));
            assert_eq!(route.with_scope(chosen).scope(), Some(chosen));
            // A word that is no choice names none, and the page fills in its own.
            assert_eq!(Route::parse(&format!("{hash}?scope=nonsense")), route);
        }
        // The Questions page carries it after whose questions show.
        let theirs = Questions { question: None, whose: Some("claude".into()), scope: Some(chosen) };
        assert_eq!(theirs.hash(), format!("#/questions?for=claude&scope={s}"));
        assert_eq!(Route::parse(&format!("#/questions?for=claude&scope={s}")), theirs);
        assert_eq!(Route::parse(&format!("#/questions?scope={s}&for=claude")), theirs, "in either order");
        // A scope's tasks, where its name and its lozenge lead, and a scope's route from before.
        assert_eq!(Route::scope_tasks(scope).hash(), format!("#/tasks?scope={s}"));
        assert_eq!(Route::parse(&format!("#/scopes/{s}")), Route::scope_tasks(scope));
        assert_eq!(Route::parse("#/nowhere"), Route::inbox());
        assert_eq!(NewTask { scope: Some(chosen) }.section(), "tasks");
        assert_eq!(Graph { task: Some(item), scope: Some(chosen) }.section(), "graph");
        assert_eq!(Route::decision(item).section(), "decisions");
    }

    #[test]
    fn the_inbox_the_scopes_page_and_a_highlights_page_list_nothing_by_scope() {
        let (anchor, scope) = (EntityId::from_bytes([4; 32]), EntityId::from_bytes([2; 32]));
        for route in [
            Route::inbox(),
            Inbox { whose: Some("claude".into()) },
            Scopes(None),
            Scopes(Some(ScopePanel::Edit(scope))),
            Anchor(anchor),
        ] {
            assert!(!route.filtered(), "{}", route.hash());
            assert_eq!(route.with_scope(ScopeChoice::All), route, "{}", route.hash());
            assert_eq!(route.scope(), None);
        }
        assert_eq!(Route::parse("#/inbox?scope=all"), Route::inbox());
    }

    #[test]
    fn a_route_given_its_scope_is_the_same_place_but_a_route_with_another_is_not() {
        let (task, a, b) =
            (EntityId::from_bytes([1; 32]), EntityId::from_bytes([2; 32]), EntityId::from_bytes([3; 32]));
        let bare = Route::task(task);
        let filled = bare.with_scope(ScopeChoice::Scope(a));
        assert!(filled.fills_in(&bare));
        assert!(!bare.fills_in(&filled), "leaving a scope out is no filling in");
        assert!(!filled.fills_in(&filled));
        assert!(!bare.with_scope(ScopeChoice::Scope(b)).fills_in(&filled), "another scope moves the page");
        assert!(!Route::task(a).with_scope(ScopeChoice::All).fills_in(&bare), "and so does another task");
        assert!(!Graph { task: Some(task), scope: Some(ScopeChoice::All) }.fills_in(&bare), "and another page");
        assert!(!Route::inbox().fills_in(&Route::inbox()));
    }

    #[test]
    fn the_scopes_page_opens_its_panel_for_a_new_scope_or_one_to_edit() {
        let scope = EntityId::from_bytes([2; 32]);
        let s = scope.to_base64();
        assert_eq!(Scopes(Some(ScopePanel::New)).hash(), "#/scopes/new");
        assert_eq!(Scopes(Some(ScopePanel::Edit(scope))).hash(), format!("#/scopes/{s}/edit"));
        assert_eq!(Route::parse("#/scopes"), Scopes(None));
        assert_eq!(Route::parse("#/scopes/nonsense/edit"), Scopes(None), "an id that does not parse opens none");
        // Without `/edit`, a scope's id still leads to the tasks worked in it, as a click on its
        // name does (Daniel's ruling of 2026-09-26).
        assert_eq!(Route::parse(&format!("#/scopes/{s}")), Route::scope_tasks(scope));
        assert_eq!(Scopes(Some(ScopePanel::Edit(scope))).section(), "scopes");
    }

    #[test]
    fn a_question_has_a_route_of_its_own_under_the_questions_page() {
        let question = EntityId::from_bytes([3; 32]);
        let q = question.to_base64();
        assert_eq!(Route::question(question).hash(), format!("#/questions/{q}"));
        assert_eq!(Route::parse(&format!("#/questions/{q}")), Route::question(question));
        let own = Questions { question: None, whose: None, scope: None };
        assert_eq!(Route::parse("#/questions"), own);
        assert_eq!(Route::parse("#/questions/nonsense"), own, "an id that does not parse selects none");
        assert_eq!(Route::question(question).section(), "questions");
    }

    #[test]
    fn another_principals_inbox_or_questions_are_named_by_handle_after_the_rest_of_the_route() {
        let question = EntityId::from_bytes([3; 32]);
        let q = question.to_base64();
        let claude = || Some("claude".to_string());
        assert_eq!(Inbox { whose: claude() }.hash(), "#/inbox?for=claude");
        assert_eq!(Route::parse("#/inbox?for=claude"), Inbox { whose: claude() });
        assert_eq!(Questions { question: None, whose: claude(), scope: None }.hash(), "#/questions?for=claude");
        assert_eq!(
            Questions { question: Some(question), whose: claude(), scope: None }.hash(),
            format!("#/questions/{q}?for=claude")
        );
        assert_eq!(
            Route::parse(&format!("#/questions/{q}?for=claude")),
            Questions { question: Some(question), whose: claude(), scope: None }
        );
        // The nav's links name no one, and neither does an empty `for`: the page shows the acting
        // principal's own, and follows the header's `as` when it changes.
        assert_eq!(Route::parse("#/inbox"), Route::inbox());
        assert_eq!(Route::inbox().hash(), "#/inbox");
        assert_eq!(Route::parse("#/inbox?for="), Route::inbox());
        assert_eq!(Route::parse("#/questions?for="), Questions { question: None, whose: None, scope: None });
        // `for` names whose Inbox or Questions page shows, and nothing on any other page.
        assert_eq!(Route::parse("#/tasks?for=claude"), Tasks { task: None, scope: None });
        assert_eq!(Route::parse("#/nowhere?for=claude"), Route::inbox());
        assert_eq!(Inbox { whose: claude() }.section(), "inbox");
        assert_eq!(Questions { question: None, whose: claude(), scope: None }.section(), "questions");
    }

    #[test]
    fn a_highlight_has_a_page_of_its_own_and_no_list() {
        let anchor = EntityId::from_bytes([4; 32]);
        let a = anchor.to_base64();
        assert_eq!(Anchor(anchor).hash(), format!("#/anchors/{a}"));
        assert_eq!(Route::parse(&format!("#/anchors/{a}")), Anchor(anchor));
        assert_eq!(Route::parse("#/anchors"), Route::inbox(), "no list of highlights");
        assert_eq!(Route::parse("#/anchors/nonsense"), Route::inbox());
        assert_eq!(Anchor(anchor).section(), "anchors");
    }
}
