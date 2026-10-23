//! What a page says while it has nothing to show (no card, no line, no drawing, nothing for the id
//! its route names), so that it never says it has nothing while the daemon's answer is still on
//! its way (task tcUrcN1T, "Say loading rather than nothing while the daemon's answer is on its
//! way, on every page, from the one signal that says it has answered"). Each standing query first
//! answers from this load's own browser storage, which starts empty on every load, and the daemon's
//! answer arrives after it, some 70 ms after a fresh load and longer over a LAN, so a page that
//! judged by the first answer said "Nothing waiting." while its cards were on their way. A page
//! says "loading…" until the daemon has answered the standing queries (`store::standing_loaded`),
//! and only then its words for having none. A row, a line in a list or a lozenge's preview naming
//! an entity the page does not hold says the same in the entity's place, "loading…" and then that
//! there is no such entity ("no such task"), where it went on saying "a task not loaded yet" after
//! the daemon had answered (task 7dq1vEd_). Every page's words are here, so that they read alike
//! and are tested together.

use crate::scope_filter::ScopeChoice;
use crate::store;
use crate::thread::Model;
use leptos::prelude::*;

/// What a page, or a thread's level beneath a row, says while what it shows is on its way.
pub const LOADING: &str = "loading…";

/// What a page has none of to show.
#[derive(Clone, Debug, PartialEq)]
pub enum Nothing {
    /// Nothing the page can name yet, whatever the standing queries say: it still waits for
    /// something of its own, as the Inbox waits for the daemon's answer to its own query of
    /// notifications, and the Questions, Tasks, Decisions and Graph pages for the scope they open
    /// on, which the page picks once the daemon has answered.
    Yet,
    /// The Inbox's Waiting on you: no row, nothing waiting on `reader` ("you", or the handle of
    /// the principal whose inbox shows).
    Requests { reader: String },
    /// The Inbox's conversations: no card, or with `unread_only`, no unread one.
    Conversations { unread_only: bool },
    /// The Questions page: no question asked of the principal with `handle`, or no open one with
    /// `open_only`; with `filtered_out`, none about the scope the page lists by, though some about
    /// other work.
    Questions { handle: String, open_only: bool, filtered_out: bool },
    /// The Tasks page's list: no task, or with `filtered_out`, none of the project's tasks kept by
    /// the filters.
    Tasks { filtered_out: bool },
    /// The Decisions page's list: no ruling, or with `filtered_out`, none of the project's rulings
    /// kept by the filters.
    Decisions { filtered_out: bool },
    /// The Graph page: no task the scope filter's `choice` keeps, or without `show_closed`, no open
    /// one.
    ToDraw { choice: ScopeChoice, show_closed: bool },
    /// The Scopes page: no scope.
    Scopes,
    /// A task's panel, for an id no task has.
    Task,
    /// A decision's panel, for an id no decision has.
    Decision,
    /// A scope's panel on the Scopes page, for an id no scope has.
    Scope,
    /// A highlight's page, for an id no highlight has.
    Highlight,
}

impl Nothing {
    /// What the page says once the daemon has answered.
    fn words(&self) -> String {
        match self {
            Nothing::Yet => LOADING.into(),
            Nothing::Requests { reader } => format!("Nothing waits on {reader}."),
            Nothing::Conversations { unread_only: true } => "No unread conversations.".into(),
            Nothing::Conversations { unread_only: false } => "No conversations.".into(),
            Nothing::Questions { handle, open_only: true, filtered_out: false } => {
                format!("No open questions to {handle}.")
            }
            Nothing::Questions { handle, open_only: false, filtered_out: false } => {
                format!("No questions to {handle}.")
            }
            Nothing::Questions { handle, open_only: true, filtered_out: true } => {
                format!("No open question to {handle} matches the scope.")
            }
            Nothing::Questions { handle, open_only: false, filtered_out: true } => {
                format!("No question to {handle} matches the scope.")
            }
            Nothing::Tasks { filtered_out: false } => "No tasks.".into(),
            Nothing::Tasks { filtered_out: true } => "No task matches the filters.".into(),
            Nothing::Decisions { filtered_out: false } => "No decisions.".into(),
            Nothing::Decisions { filtered_out: true } => "No decision matches the filters.".into(),
            Nothing::ToDraw { choice, show_closed } => match (choice, show_closed) {
                (ScopeChoice::Scope(_), false) => "No open tasks in this scope.",
                (ScopeChoice::Scope(_), true) => "No tasks in this scope.",
                (ScopeChoice::NoScope, false) => "No open tasks without a scope.",
                (ScopeChoice::NoScope, true) => "No tasks without a scope.",
                (ScopeChoice::All, false) => "No open tasks.",
                (ScopeChoice::All, true) => "No tasks.",
            }
            .into(),
            Nothing::Scopes => "No scopes.".into(),
            Nothing::Task => "No such task.".into(),
            Nothing::Decision => "No such decision.".into(),
            Nothing::Scope => "No such scope.".into(),
            Nothing::Highlight => "No such highlight.".into(),
        }
    }
}

/// What a page with `nothing` to show says in its place: "loading…" until the daemon has
/// `answered` the standing queries, then its words for `nothing`.
pub fn says(nothing: &Nothing, answered: bool) -> String {
    if answered {
        nothing.words()
    } else {
        LOADING.to_string()
    }
}

/// The sentence a page shows in place of what it has none of ([`says`]), following the daemon's
/// answer to the standing queries.
#[component]
pub fn Empty(#[prop(into)] nothing: Signal<Nothing>) -> impl IntoView {
    view! { <p class="empty">{move || says(&nothing.get(), store::standing_loaded())}</p> }
}

/// What a row, a line in a list or a lozenge's preview says in place of an entity the page does
/// not hold, of the kind `model` names, or of a kind the page cannot tell (`None`), having nothing
/// under the id to tell it by: [`says_absent`], following the daemon's answer to the standing
/// queries. Asked inside a closure, it makes the closure a dependent of the answer, so the closure
/// asks again once the daemon has answered.
pub fn absent(model: Option<Model>) -> String {
    says_absent(model, store::standing_loaded())
}

/// "loading…" until the daemon has `answered` the standing queries, which hold every task,
/// decision, message, scope, highlight and principal of the project, then that the project has no
/// such entity of the kind `model` names, or for a kind the page cannot tell, that the page holds
/// nothing under the id.
fn says_absent(model: Option<Model>, answered: bool) -> String {
    if !answered {
        return LOADING.into();
    }
    let kind = match model {
        Some(Model::Task) => "task",
        Some(Model::Decision) => "decision",
        Some(Model::Message) => "message",
        Some(Model::Scope) => "scope",
        Some(Model::Anchor) => "highlight",
        Some(Model::Principal) => "principal",
        None => return "not something this page holds".into(),
    };
    format!("no such {kind}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ankurah::EntityId;

    /// Every page's words for having nothing, as it says them once the daemon has answered.
    fn every_page() -> Vec<(Nothing, &'static str)> {
        let scope = ScopeChoice::Scope(EntityId::from_bytes([1; 32]));
        let questions =
            |open_only, filtered_out| Nothing::Questions { handle: "dnorman".into(), open_only, filtered_out };
        let drawing = |choice, show_closed| Nothing::ToDraw { choice, show_closed };
        vec![
            (Nothing::Requests { reader: "you".into() }, "Nothing waits on you."),
            (Nothing::Requests { reader: "lead-fwd".into() }, "Nothing waits on lead-fwd."),
            (Nothing::Conversations { unread_only: true }, "No unread conversations."),
            (Nothing::Conversations { unread_only: false }, "No conversations."),
            (questions(true, false), "No open questions to dnorman."),
            (questions(false, false), "No questions to dnorman."),
            (questions(true, true), "No open question to dnorman matches the scope."),
            (questions(false, true), "No question to dnorman matches the scope."),
            (Nothing::Tasks { filtered_out: false }, "No tasks."),
            (Nothing::Tasks { filtered_out: true }, "No task matches the filters."),
            (Nothing::Decisions { filtered_out: false }, "No decisions."),
            (Nothing::Decisions { filtered_out: true }, "No decision matches the filters."),
            (drawing(scope, false), "No open tasks in this scope."),
            (drawing(scope, true), "No tasks in this scope."),
            (drawing(ScopeChoice::NoScope, false), "No open tasks without a scope."),
            (drawing(ScopeChoice::NoScope, true), "No tasks without a scope."),
            (drawing(ScopeChoice::All, false), "No open tasks."),
            (drawing(ScopeChoice::All, true), "No tasks."),
            (Nothing::Scopes, "No scopes."),
            (Nothing::Task, "No such task."),
            (Nothing::Decision, "No such decision."),
            (Nothing::Scope, "No such scope."),
            (Nothing::Highlight, "No such highlight."),
        ]
    }

    #[test]
    fn every_page_says_loading_until_the_daemon_has_answered_and_its_own_words_after() {
        for (nothing, words) in every_page() {
            assert_eq!(says(&nothing, false), LOADING, "{nothing:?}, before the daemon has answered");
            assert_eq!(says(&nothing, true), words, "{nothing:?}, once it has");
        }
    }

    #[test]
    fn a_page_still_waiting_for_something_of_its_own_says_loading_once_the_daemon_has_answered_too() {
        assert_eq!(says(&Nothing::Yet, false), LOADING);
        assert_eq!(says(&Nothing::Yet, true), LOADING);
    }

    #[test]
    fn an_entity_the_page_does_not_hold_is_loading_until_the_daemon_has_answered_and_then_no_such_entity() {
        let every_kind = [
            (Some(Model::Task), "no such task"),
            (Some(Model::Decision), "no such decision"),
            (Some(Model::Message), "no such message"),
            (Some(Model::Scope), "no such scope"),
            (Some(Model::Anchor), "no such highlight"),
            (Some(Model::Principal), "no such principal"),
            (None, "not something this page holds"),
        ];
        for (model, words) in every_kind {
            assert_eq!(says_absent(model, false), LOADING, "{model:?}, before the daemon has answered");
            assert_eq!(says_absent(model, true), words, "{model:?}, once it has");
        }
    }
}
