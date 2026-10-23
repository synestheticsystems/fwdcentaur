//! What the Graph page, the Inbox and the lists of the Tasks and Decisions pages keep of themselves
//! while the page lives, beside the route, so that Back in the VS Code webview brings each back as
//! the person left it (task toDjwytu, "Carry the Graph's scope filter and a page's state through
//! Back in the webview"; the lists, task lccNMCh_, "Give Back to a decision opened from the Inbox,
//! the Graph or the Questions page, and keep the Tasks and Decisions lists' own filters through a
//! Back that crosses pages, in the VS Code webview"). The route carries what a link and a reload
//! keep, such as the scope the Questions, Tasks, Decisions and Graph pages list by (so Back brings
//! a page back with the scope it had) and whose inbox shows; the rest of what a person does on a
//! page, a visit starts over: how far the Graph is zoomed and
//! scrolled and whether it draws closed tasks, which inbox card is selected and whether the Inbox
//! lists unread cards only, and the lists' other filters (the Tasks list's title, label, "mine",
//! "ready only" and "show closed", the Decisions list's status and area). So each of these pages
//! writes that state here as it changes, into a signal of its own, outside the route as a panel's
//! dragged width is (`panel.rs`), and reads it back as it opens when Back brought the page there
//! (`back::brought_back`; for the lists `back::returning_to`, which leaves where Back led for the
//! panel the Tasks or Decisions page then shows); opened any other way, the page starts over, as it
//! always did, and keeps its new state here from then on. A list comes back so when Back crosses
//! pages, from a decision to the task whose lozenge opened it, say: a Back within its own page
//! leaves the list where it was, filters and all. Nothing here outlasts a reload.

use crate::back;
use crate::cards::Item;
use ankurah::EntityId;
use fwd_core::model::DecisionStatus;
use leptos::prelude::*;
use std::collections::HashSet;
use std::sync::OnceLock;

/// What the Graph page keeps of itself.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GraphState {
    /// How much the drawing is scaled: 1 at its own size.
    pub zoom: f64,
    /// Whether the drawing shows closed tasks.
    pub show_closed: bool,
    /// How far the drawing's canvas is scrolled across and down, in CSS pixels.
    pub scroll: (i32, i32),
}

impl Default for GraphState {
    /// How a visit opens the Graph: the drawing at its own size, without closed tasks, scrolled to
    /// its start.
    fn default() -> Self {
        GraphState { zoom: 1.0, show_closed: false, scroll: (0, 0) }
    }
}

/// What the Inbox keeps of itself, for one principal's inbox.
#[derive(Clone, Debug, PartialEq)]
pub struct InboxState {
    /// The principal whose inbox the page showed.
    pub whose: EntityId,
    pub unread_only: bool,
    /// The card selected, once the page had chosen one.
    pub selected: Option<Item>,
    /// The notifications the selected card showed, which it goes on showing while it stays
    /// selected, read or not (`inbox.rs`).
    pub kept: HashSet<EntityId>,
}

/// What the Tasks page's list keeps of itself: its filters other than the scope, which the route
/// carries. A visit opens the list with none of them set.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TaskListState {
    /// What the box that filters by title holds.
    pub title: String,
    /// The label whose tasks the list keeps; none keeps every task.
    pub label: Option<String>,
    /// Whether the list keeps only the tasks assigned to the acting principal.
    pub mine: bool,
    /// Whether it keeps only the tasks ready to start.
    pub ready_only: bool,
    /// Whether it lists closed tasks too.
    pub show_closed: bool,
}

/// What the Decisions page's list keeps of itself: its filters other than the scope, which the
/// route carries. A visit opens the list on every status and every area.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DecisionListState {
    /// Only this status; every status when none.
    pub status: Option<DecisionStatus>,
    /// Only this area; every area when none, and `Some("")` the decisions with no area.
    pub area: Option<String>,
}

fn graph() -> ArcRwSignal<GraphState> {
    static GRAPH: OnceLock<ArcRwSignal<GraphState>> = OnceLock::new();
    GRAPH.get_or_init(|| ArcRwSignal::new(GraphState::default())).clone()
}

fn inbox() -> ArcRwSignal<Option<InboxState>> {
    static INBOX: OnceLock<ArcRwSignal<Option<InboxState>>> = OnceLock::new();
    INBOX.get_or_init(|| ArcRwSignal::new(None)).clone()
}

fn task_list() -> ArcRwSignal<TaskListState> {
    static TASK_LIST: OnceLock<ArcRwSignal<TaskListState>> = OnceLock::new();
    TASK_LIST.get_or_init(|| ArcRwSignal::new(TaskListState::default())).clone()
}

fn decision_list() -> ArcRwSignal<DecisionListState> {
    static DECISION_LIST: OnceLock<ArcRwSignal<DecisionListState>> = OnceLock::new();
    DECISION_LIST.get_or_init(|| ArcRwSignal::new(DecisionListState::default())).clone()
}

/// What a page whose state is `kept` opens with: the state it had when it was left, when Back
/// `brought_back` the page; else a visit's first, which it keeps from then on, so that a later Back
/// brings back this visit rather than an earlier one.
fn open<T: Clone + Default + Send + Sync + 'static>(kept: &ArcRwSignal<T>, brought_back: bool) -> T {
    let state = if brought_back { kept.get_untracked() } else { T::default() };
    kept.set(state.clone());
    state
}

/// The state the Graph page opens with, asked once as it opens: the state it had when it was
/// left, when Back brought the page there; else a visit's first, which it keeps from then on.
pub fn open_graph() -> GraphState {
    open(&graph(), back::brought_back("graph"))
}

/// Keeps a change to the Graph page's state.
pub fn keep_graph(change: impl FnOnce(&mut GraphState)) {
    graph().update(change);
}

/// The state `whose`'s inbox opens with, asked once as it opens: the state the Inbox had when it
/// was left, when Back brought the page back to that principal's inbox; else none, and the page
/// starts over.
pub fn open_inbox(whose: EntityId) -> Option<InboxState> {
    let brought_back = back::brought_back("inbox");
    inbox().with_untracked(|kept| reopened(kept.as_ref(), whose, brought_back))
}

/// Keeps the Inbox's state as it is now.
pub fn keep_inbox(state: InboxState) {
    inbox().set(Some(state));
}

/// The filters the Tasks page's list opens with, asked once as the list is built: those it had
/// when it was left, when Back is bringing the page back; else a visit's first, which it keeps
/// from then on.
pub fn open_task_list() -> TaskListState {
    open(&task_list(), back::returning_to("tasks"))
}

/// Keeps the Tasks page's list's filters as they are now.
pub fn keep_task_list(state: TaskListState) {
    task_list().set(state);
}

/// The filters the Decisions page's list opens with, asked once as the list is built: those it
/// had when it was left, when Back is bringing the page back; else a visit's first, which it keeps
/// from then on.
pub fn open_decision_list() -> DecisionListState {
    open(&decision_list(), back::returning_to("decisions"))
}

/// Keeps the Decisions page's list's filters as they are now.
pub fn keep_decision_list(state: DecisionListState) {
    decision_list().set(state);
}

/// What the Inbox showing `whose`'s notifications reopens with, `kept` being its state when it was
/// last left: that state when Back `brought_back` the page and it was the same principal's inbox.
/// Another principal's inbox, shown since the page acts as someone else or the route names another,
/// starts over.
fn reopened(kept: Option<&InboxState>, whose: EntityId, brought_back: bool) -> Option<InboxState> {
    kept.filter(|kept| brought_back && kept.whose == whose).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> EntityId {
        EntityId::from_bytes([n; 32])
    }

    #[test]
    fn the_inbox_comes_back_to_its_card_and_filter_only_when_back_brings_it_back_to_the_same_inbox() {
        let (me, claude) = (id(1), id(2));
        let left =
            InboxState { whose: me, unread_only: false, selected: Some(Item::Thread(id(7))), kept: [id(8)].into() };
        assert_eq!(reopened(Some(&left), me, true), Some(left.clone()));
        assert_eq!(reopened(Some(&left), me, false), None, "a visit any other way starts over");
        assert_eq!(reopened(Some(&left), claude, true), None, "and so does another principal's inbox");
        assert_eq!(reopened(None, me, true), None, "and a page that has not shown the Inbox yet");
    }

    #[test]
    fn a_list_comes_back_with_its_filters_only_when_back_brings_it_back_and_a_visit_starts_it_over() {
        let left = DecisionListState { status: Some(DecisionStatus::Accepted), area: Some("ux".into()) };
        let kept = ArcRwSignal::new(left.clone());
        assert_eq!(open(&kept, true), left, "Back brought it back");
        assert_eq!(open(&kept, true), left, "and again, as it was left again");
        assert_eq!(open(&kept, false), DecisionListState::default(), "a visit any other way starts over");
        assert_eq!(open(&kept, true), DecisionListState::default(), "and a Back after it brings back that visit");
        let left = TaskListState {
            title: "back".into(),
            label: Some("bug".into()),
            mine: true,
            ready_only: true,
            show_closed: true,
        };
        let kept = ArcRwSignal::new(left.clone());
        assert_eq!(open(&kept, true), left);
        assert_eq!(open(&kept, false), TaskListState::default());
    }
}
