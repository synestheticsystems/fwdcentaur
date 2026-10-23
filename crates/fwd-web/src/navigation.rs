//! The rows of a card as the keyboard meets them: where the arrows, Home, End and Enter take the
//! focus, where the focus goes when its row disappears, which rows open by themselves, which rows
//! stay in sight because someone is writing under them, and which row takes the focus once a
//! reply arrives. `thread.rs` draws the rows and hands every such decision to the functions here,
//! which need no browser and are tested natively.

use crate::referents::{Appearance, Appearances};
use std::collections::{HashMap, HashSet};
use std::hash::Hash;

/// How many levels below a card's own row open by themselves, and how many open below a row a
/// person opened: a long exchange stays readable, and the page never opens a query per message
/// all the way down on its own.
pub const OPEN_LEVELS: usize = 6;

/// Whether a row starts open: while it has levels left to open by itself, unless its entity
/// already shows open elsewhere on the page (the second appearance starts closed, saying how many
/// items wait below it, so one thread is not drawn twice).
pub fn starts_open(levels_left: usize, open_elsewhere: bool) -> bool {
    levels_left > 1 && !open_elsewhere
}

/// How many levels a row's children may still open by themselves: one fewer than the row had,
/// or all of them again below a row the person opened by hand.
pub fn levels_below(levels_left: usize, opened_by_hand: bool) -> usize {
    if opened_by_hand {
        OPEN_LEVELS
    } else {
        levels_left.saturating_sub(1)
    }
}

/// What heads a card's rows. Where a thread is shown on its own, the row of the entity it is about
/// says what the thread is about; where the page names that entity above the rows already, that
/// row would only say it again, so the rows beneath it head the card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Head {
    /// The card's own row: a card on the Inbox or the Questions page, a thing opened in place, and
    /// a page whose own row holds what the page shows nowhere else (a decision's moves, the box
    /// that opens a new thread on a highlight, a message's text).
    OwnRow,
    /// The rows beneath the card's own row, which is left out: a task's panel, which names the
    /// task at its top, a few lines above its Thread section (Daniel, 2026-09-30: "the Thread
    /// section of a task should not restate the name/id/status/tags of the task. that is
    /// redundant."; task IvTeCbVL).
    RowsBeneath,
}

impl Head {
    /// Whether `row` is the card's own row (`root`), left out by this head.
    pub fn leaves_out(self, root: Appearance, row: Appearance) -> bool {
        self == Head::RowsBeneath && row == root
    }

    /// The row that `parent`, the appearance a row sits under, is on the page: `parent` itself,
    /// unless it is the card's own row (`root`) left out, which no element stands for. Left climbs
    /// to it, and the focus goes to it when the rows beneath it leave.
    pub fn shown_parent(self, root: Appearance, parent: Option<Appearance>) -> Option<Appearance> {
        parent.filter(|parent| !self.leaves_out(root, *parent))
    }

    /// Where the Tab key enters a card none of whose rows has had the focus: the card's own row,
    /// or, where that row is left out, `first`, the first row beneath it. With no row beneath it,
    /// that is still the card's own appearance, which no element stands for, so the card offers
    /// the Tab key no stop until a row arrives.
    pub fn entry(self, root: Appearance, first: Option<Appearance>) -> Appearance {
        match self {
            Head::OwnRow => root,
            Head::RowsBeneath => first.unwrap_or(root),
        }
    }
}

/// A place the arrows stop at: a row, or the reply box a row has open. A reply box sits after the
/// rows beneath its row, where the reply it sends will appear.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    Row(Appearance),
    Editor(Appearance),
}

/// The stops of one card's rows top to bottom: the card's own row, the rows it shows beneath it
/// (each followed by the rows beneath that, depth first), then its open reply box. Where `head`
/// leaves the card's own row out, the stops start at the rows beneath it, and the row's box goes
/// with the row. `shown` answers the rows an appearance shows beneath itself (none for a closed
/// row), and `editing` whether its reply box is open.
pub fn stops(
    root: Appearance,
    head: Head,
    shown: impl Fn(Appearance) -> Vec<Appearance>,
    editing: impl Fn(Appearance) -> bool,
) -> Vec<Stop> {
    let mut order = Vec::new();
    // Rows still to visit, and each open reply box waiting for the rows beneath its row.
    let mut stack = match head {
        Head::OwnRow => vec![Stop::Row(root)],
        Head::RowsBeneath => shown(root).into_iter().rev().map(Stop::Row).collect(),
    };
    while let Some(next) = stack.pop() {
        order.push(next);
        if let Stop::Row(row) = next {
            if editing(row) {
                stack.push(Stop::Editor(row));
            }
            stack.extend(shown(row).into_iter().rev().map(Stop::Row));
        }
    }
    order
}

/// A key the rows answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    Enter,
}

/// What the keyboard needs to know about the row that has the focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowFacts {
    pub row: Appearance,
    /// The row it sits under on the page ([`Head::shown_parent`]); none for a card's own row, and
    /// for a row beneath a card's own row that the card leaves out.
    pub parent: Option<Appearance>,
    pub open: bool,
    /// Whether opening it could show anything (see `Level::may_have_children`).
    pub openable: bool,
    /// The first row it shows beneath it.
    pub first_child: Option<Appearance>,
    /// Where its entity already shows on its path, for a row that stands for a cycle.
    pub cycle: Option<Appearance>,
    /// Whether it takes a reply box.
    pub editor: bool,
    /// Whether choosing it does something (a task opens its page).
    pub activate: bool,
}

/// What a key press on a row, or at an edge of a reply box, comes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Focus(Appearance),
    /// Put the caret in this row's reply box.
    FocusEditor(Appearance),
    Open(Appearance),
    Close(Appearance),
    /// Open the row's reply box and put the caret in it.
    Edit(Appearance),
    /// Do what choosing the row does.
    Activate(Appearance),
    /// The key was the rows', and there is nothing to do (Right on a leaf).
    Stay,
    /// The key is not the rows': the page around them may use it (Up on the card's own row).
    Pass,
}

fn step_to(stop: Stop) -> Step {
    match stop {
        Stop::Row(row) => Step::Focus(row),
        Stop::Editor(row) => Step::FocusEditor(row),
    }
}

fn neighbour(stops: &[Stop], from: Stop, delta: isize) -> Step {
    let at = stops.iter().position(|stop| *stop == from);
    at.and_then(|i| i.checked_add_signed(delta)).and_then(|i| stops.get(i)).map_or(Step::Pass, |stop| step_to(*stop))
}

/// Where a key pressed on a row leads, following the WAI-ARIA tree pattern: Up and Down move to
/// the stop before or after it; Right opens a closed row, then enters its first child; Left closes
/// an open row, then climbs to its parent; Home and End go to the first and last stops; Enter
/// follows a reference back to where its entity already shows, else opens the row's reply box,
/// else does what choosing the row does.
pub fn on_row(press: Press, stops: &[Stop], facts: &RowFacts) -> Step {
    let here = Stop::Row(facts.row);
    match press {
        Press::Up => neighbour(stops, here, -1),
        Press::Down => neighbour(stops, here, 1),
        Press::Right if facts.cycle.is_some() => Step::Stay,
        Press::Right if facts.openable && !facts.open => Step::Open(facts.row),
        Press::Right => facts.first_child.filter(|_| facts.open).map_or(Step::Stay, Step::Focus),
        Press::Left if facts.open && facts.cycle.is_none() => Step::Close(facts.row),
        Press::Left => facts.parent.map_or(Step::Pass, Step::Focus),
        Press::Home => stops.first().map_or(Step::Pass, |stop| step_to(*stop)),
        Press::End => stops.last().map_or(Step::Pass, |stop| step_to(*stop)),
        Press::Enter => match facts.cycle {
            Some(earlier) => Step::Focus(earlier),
            None if facts.editor => Step::Edit(facts.row),
            None if facts.activate => Step::Activate(facts.row),
            None => Step::Stay,
        },
    }
}

/// Where Up on the first line, or Down on the last line, of `owner`'s reply box leads: to the
/// stop before it (the last row beneath `owner`, or `owner` itself) or the stop after it.
pub fn off_editor(up: bool, stops: &[Stop], owner: Appearance) -> Step {
    neighbour(stops, Stop::Editor(owner), if up { -1 } else { 1 })
}

/// Where the focus goes when the row `gone` leaves a list of rows that was `before` and is now
/// `after`: to its next sibling that stayed, else its previous sibling that stayed, else its
/// parent. Never to another row that merely looks alike.
pub fn refocus(
    before: &[Appearance],
    after: &[Appearance],
    gone: Appearance,
    parent: Option<Appearance>,
) -> Option<Appearance> {
    let Some(at) = before.iter().position(|a| *a == gone) else { return parent };
    let stayed = |a: &&Appearance| after.contains(a);
    before[at + 1..].iter().find(stayed).or_else(|| before[..at].iter().rev().find(stayed)).copied().or(parent)
}

/// Whether `row` stays in sight beneath a closed parent: while it, or a row beneath it, has a box
/// open that someone writes in (its reply box, or the box that edits its own text), so nothing a
/// person is writing disappears when a branch above it closes.
pub fn pinned<I, L>(row: Appearance, open_editors: &HashSet<Appearance>, appearances: &Appearances<I, L>) -> bool
where
    I: Copy + Eq + Hash,
    L: Copy + Eq + Hash,
{
    open_editors.iter().any(|editor| appearances.is_within(*editor, row))
}

/// The row that should take the focus once it appears: the reply just sent, whose id the page
/// learns only after the write has committed, by which time its row may already be there. The
/// thread checks the rows already there when it sets this, and each row as it arrives, until
/// `until_ms`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PendingFocus<I> {
    pub parent: Appearance,
    pub id: I,
    pub until_ms: f64,
}

impl<I: PartialEq> PendingFocus<I> {
    /// Whether the row of `id` under `parent`, arriving at `now_ms`, is the one waited for.
    pub fn claims(&self, parent: Appearance, id: &I, now_ms: f64) -> bool {
        now_ms <= self.until_ms && self.parent == parent && self.id == *id
    }
}

/// Which appearances are on the page, by entity: a row says where else its entity shows, and a
/// row whose entity already shows open elsewhere starts closed.
pub struct Mounted<I> {
    by_entity: HashMap<I, Vec<Appearance>>,
}

impl<I: Copy + Eq + Hash> Default for Mounted<I> {
    fn default() -> Self {
        Mounted { by_entity: HashMap::new() }
    }
}

impl<I: Copy + Eq + Hash> Mounted<I> {
    pub fn add(&mut self, id: I, appearance: Appearance) {
        let list = self.by_entity.entry(id).or_default();
        if !list.contains(&appearance) {
            list.push(appearance);
        }
    }

    pub fn remove(&mut self, id: I, appearance: Appearance) {
        if let Some(list) = self.by_entity.get_mut(&id) {
            list.retain(|a| *a != appearance);
            if list.is_empty() {
                self.by_entity.remove(&id);
            }
        }
    }

    /// Every appearance of `id` on the page, in the order they arrived.
    pub fn of(&self, id: I) -> Vec<Appearance> {
        self.by_entity.get(&id).cloned().unwrap_or_default()
    }

    /// The other appearances of `id` on the page, in the order they arrived.
    pub fn others(&self, id: I, except: Appearance) -> Vec<Appearance> {
        self.of(id).into_iter().filter(|a| *a != except).collect()
    }

    pub fn contains(&self, id: I, appearance: Appearance) -> bool {
        self.by_entity.get(&id).is_some_and(|list| list.contains(&appearance))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::referents::fake::Link;

    /// A card's row 1 shows rows 2 and 5; row 2 shows 3 and 4; row 3 shows 6.
    fn appearances() -> (Appearances<u32, Link>, [Appearance; 6]) {
        let mut ap = Appearances::new();
        let one = ap.root(100, 1);
        let two = ap.child(one, Link::Replies, 2);
        let three = ap.child(two, Link::Replies, 3);
        let four = ap.child(two, Link::Replies, 4);
        let five = ap.child(one, Link::Replies, 5);
        let six = ap.child(three, Link::Replies, 6);
        (ap, [one, two, three, four, five, six])
    }

    fn shown(rows: [Appearance; 6]) -> HashMap<Appearance, Vec<Appearance>> {
        let [one, two, three, _, five, six] = rows;
        [(one, vec![two, five]), (two, vec![three, rows[3]]), (three, vec![six])].into_iter().collect()
    }

    fn facts(row: Appearance) -> RowFacts {
        RowFacts {
            row,
            parent: None,
            open: false,
            openable: false,
            first_child: None,
            cycle: None,
            editor: false,
            activate: false,
        }
    }

    #[test]
    fn six_levels_open_by_themselves_and_a_row_opened_by_hand_gives_six_more() {
        // The card's own row starts with one more, so six levels open beneath it.
        let mut levels = OPEN_LEVELS + 1;
        let mut open = Vec::new();
        for _ in 0..9 {
            open.push(starts_open(levels, false));
            levels = levels_below(levels, false);
        }
        assert_eq!(
            open,
            vec![true, true, true, true, true, true, false, false, false],
            "the sixth row down waits for a click"
        );
        assert_eq!(levels_below(1, true), OPEN_LEVELS);
        assert!(!starts_open(OPEN_LEVELS, true), "a repeat starts closed while its entity shows open elsewhere");
    }

    #[test]
    fn the_stops_walk_open_rows_depth_first_with_each_reply_box_after_its_rows() {
        let (_, rows) = appearances();
        let [one, two, three, four, five, six] = rows;
        let shown = shown(rows);
        let shown = |a: Appearance| shown.get(&a).cloned().unwrap_or_default();
        let none = |_: Appearance| false;
        let expected: Vec<Stop> = [one, two, three, six, four, five].into_iter().map(Stop::Row).collect();
        assert_eq!(stops(one, Head::OwnRow, shown, none), expected);
        let editing = |a: Appearance| a == one || a == two;
        let expected = vec![
            Stop::Row(one),
            Stop::Row(two),
            Stop::Row(three),
            Stop::Row(six),
            Stop::Row(four),
            Stop::Editor(two),
            Stop::Row(five),
            Stop::Editor(one),
        ];
        assert_eq!(stops(one, Head::OwnRow, shown, editing), expected);
        let closed_two = |a: Appearance| if a == two { Vec::new() } else { shown(a) };
        assert_eq!(stops(one, Head::OwnRow, closed_two, none), vec![Stop::Row(one), Stop::Row(two), Stop::Row(five)]);
    }

    #[test]
    fn a_task_panel_starts_its_thread_at_a_message_and_a_card_at_what_the_thread_is_about() {
        use crate::referents::fake::{Graph, Model};
        use crate::referents::{self, Models};
        // A task (a todo, 1) with two comments (notes 2 and 3), the second answered by note 4.
        let mut graph = Graph::default();
        graph.add(1, &[Model::Todo], 0).add(2, &[Model::Note], 10).add(3, &[Model::Note], 20);
        graph.add(4, &[Model::Note], 30);
        graph.link(2, Link::Comments, 1).link(3, Link::Comments, 1).link(4, Link::Replies, 3);
        // The rows each appearance shows beneath itself, every row open, as the page lists them.
        let mut ap = Appearances::new();
        let task = ap.root(100, 1);
        let mut beneath: HashMap<Appearance, Vec<Appearance>> = HashMap::new();
        let mut unvisited = vec![task];
        while let Some(parent) = unvisited.pop() {
            let id = ap.entity(parent);
            let rows: Vec<Appearance> =
                referents::level(&mut ap, parent, graph.listings(id), 50).rows.iter().map(|r| r.appearance).collect();
            unvisited.extend(rows.iter().copied());
            beneath.insert(parent, rows);
        }
        let (first, second) = (beneath[&task][0], beneath[&task][1]);
        let reply = beneath[&second][0];
        let shown = |a: Appearance| beneath.get(&a).cloned().unwrap_or_default();
        let none = |_: Appearance| false;
        let row_of = |stop: Stop| match stop {
            Stop::Row(row) | Stop::Editor(row) => row,
        };
        let entities = |stops: &[Stop]| stops.iter().map(|stop| ap.entity(row_of(*stop))).collect::<Vec<_>>();

        // The task's panel names the task at its top, so its Thread section starts at the first
        // comment and no row in it is the task's own; even an open box of that row goes with it.
        let panel = stops(task, Head::RowsBeneath, shown, |a| a == task);
        assert_eq!(entities(&panel), vec![2, 3, 4]);
        assert_eq!(graph.models(ap.entity(row_of(panel[0]))), Models::one(Model::Note), "the first row is a message");
        // The keyboard meets the same rows: Home goes to the first comment, Up and Left there leave
        // the key to the page, a reply climbs to its comment, and the Tab key enters at the first
        // comment, or nowhere while the thread has none.
        let head = Head::RowsBeneath;
        let at = |row: Appearance| RowFacts { parent: head.shown_parent(task, ap.parent(row)), ..facts(row) };
        assert_eq!(on_row(Press::Home, &panel, &at(reply)), Step::Focus(first));
        assert_eq!(on_row(Press::Up, &panel, &at(first)), Step::Pass);
        assert_eq!(on_row(Press::Left, &panel, &at(first)), Step::Pass, "no row above the first comment to climb to");
        assert_eq!(on_row(Press::Left, &panel, &at(reply)), Step::Focus(second));
        assert_eq!(head.entry(task, Some(first)), first);
        assert_eq!(head.entry(task, None), task, "the card's own appearance, which no element stands for");
        assert_eq!(refocus(&[first], &[], first, head.shown_parent(task, Some(task))), None, "the last comment left");

        // An Inbox card about the task heads the same rows with the task's own row, which says there
        // what the thread is about (as a card on the Questions page heads its rows with the
        // question), and its keys climb to it.
        let card = stops(task, Head::OwnRow, shown, none);
        assert_eq!(entities(&card), vec![1, 2, 3, 4]);
        assert_eq!(graph.models(ap.entity(row_of(card[0]))), Models::one(Model::Todo));
        let own = |row: Appearance| RowFacts { parent: Head::OwnRow.shown_parent(task, ap.parent(row)), ..facts(row) };
        assert_eq!(on_row(Press::Left, &card, &own(first)), Step::Focus(task));
        assert_eq!(on_row(Press::Home, &card, &own(reply)), Step::Focus(task));
        assert_eq!(Head::OwnRow.entry(task, Some(first)), task);
    }

    #[test]
    fn arrows_move_through_the_stops_and_pass_at_the_ends() {
        let (_, [one, two, three, four, five, _]) = appearances();
        let stops =
            [Stop::Row(one), Stop::Row(two), Stop::Row(three), Stop::Row(four), Stop::Editor(two), Stop::Row(five)];
        assert_eq!(on_row(Press::Down, &stops, &facts(three)), Step::Focus(four));
        assert_eq!(
            on_row(Press::Down, &stops, &facts(four)),
            Step::FocusEditor(two),
            "into the reply box after the rows"
        );
        assert_eq!(on_row(Press::Up, &stops, &facts(five)), Step::FocusEditor(two));
        assert_eq!(on_row(Press::Up, &stops, &facts(one)), Step::Pass, "the card's own row leaves Up to the page");
        assert_eq!(on_row(Press::Down, &stops, &facts(five)), Step::Pass);
        assert_eq!(on_row(Press::Home, &stops, &facts(four)), Step::Focus(one));
        assert_eq!(on_row(Press::End, &stops, &facts(two)), Step::Focus(five));
    }

    #[test]
    fn right_opens_then_enters_and_left_closes_then_climbs() {
        let (_, [one, two, three, ..]) = appearances();
        let stops = [Stop::Row(one), Stop::Row(two), Stop::Row(three)];
        let closed = RowFacts { openable: true, parent: Some(one), ..facts(two) };
        assert_eq!(on_row(Press::Right, &stops, &closed), Step::Open(two));
        let open = RowFacts { open: true, first_child: Some(three), ..closed };
        assert_eq!(on_row(Press::Right, &stops, &open), Step::Focus(three));
        assert_eq!(on_row(Press::Left, &stops, &open), Step::Close(two));
        assert_eq!(on_row(Press::Left, &stops, &closed), Step::Focus(one));
        assert_eq!(on_row(Press::Left, &stops, &facts(one)), Step::Pass, "a closed card row leaves Left to the page");
        assert_eq!(on_row(Press::Right, &stops, &facts(three)), Step::Stay, "a leaf keeps Right");
    }

    #[test]
    fn enter_follows_a_reference_else_edits_else_activates() {
        let (_, [one, two, ..]) = appearances();
        let stops = [Stop::Row(one), Stop::Row(two)];
        let reference = RowFacts { cycle: Some(one), editor: true, ..facts(two) };
        assert_eq!(on_row(Press::Enter, &stops, &reference), Step::Focus(one));
        let both = RowFacts { editor: true, activate: true, ..facts(two) };
        assert_eq!(on_row(Press::Enter, &stops, &both), Step::Edit(two));
        assert_eq!(on_row(Press::Enter, &stops, &RowFacts { activate: true, ..facts(two) }), Step::Activate(two));
        assert_eq!(on_row(Press::Enter, &stops, &facts(two)), Step::Stay);
    }

    #[test]
    fn a_reply_box_hands_its_edges_to_the_stops_around_it() {
        let (_, [one, two, three, four, five, _]) = appearances();
        let stops =
            [Stop::Row(one), Stop::Row(two), Stop::Row(three), Stop::Row(four), Stop::Editor(two), Stop::Row(five)];
        assert_eq!(off_editor(true, &stops, two), Step::Focus(four), "the last row beneath its row");
        assert_eq!(off_editor(false, &stops, two), Step::Focus(five));
        let alone = [Stop::Row(one), Stop::Editor(one)];
        assert_eq!(off_editor(true, &alone, one), Step::Focus(one), "its own row when nothing shows beneath it");
        assert_eq!(off_editor(false, &alone, one), Step::Pass);
    }

    #[test]
    fn a_row_that_leaves_hands_the_focus_to_a_sibling_else_its_parent() {
        let (_, [_, two, three, four, five, six]) = appearances();
        assert_eq!(refocus(&[three, four, six], &[three, six], four, Some(two)), Some(six), "the next sibling");
        assert_eq!(refocus(&[three, four, six], &[three], six, Some(two)), Some(three), "else the previous one");
        assert_eq!(refocus(&[three, four], &[five], three, Some(two)), Some(two), "else the parent");
        assert_eq!(refocus(&[three, four], &[], four, None), None);
    }

    #[test]
    fn a_row_with_a_reply_box_open_below_it_stays_in_sight() {
        let (ap, [_, two, three, four, _, six]) = appearances();
        let open: HashSet<Appearance> = [six].into_iter().collect();
        assert!(pinned(three, &open, &ap), "the reply box is under it");
        assert!(pinned(six, &open, &ap), "the reply box is its own");
        assert!(pinned(two, &open, &ap));
        assert!(!pinned(four, &open, &ap));
    }

    #[test]
    fn a_pending_focus_claims_its_row_until_it_expires() {
        let (_, [one, two, ..]) = appearances();
        let pending = PendingFocus { parent: one, id: 7u32, until_ms: 5_000.0 };
        assert!(pending.claims(one, &7, 1_000.0));
        assert!(!pending.claims(two, &7, 1_000.0), "the same entity under another parent");
        assert!(!pending.claims(one, &8, 1_000.0));
        assert!(!pending.claims(one, &7, 5_001.0), "too late");
    }

    #[test]
    fn mounted_rows_know_where_else_their_entity_shows() {
        let (_, [one, two, three, ..]) = appearances();
        let mut mounted = Mounted::default();
        mounted.add(9u32, two);
        mounted.add(9, three);
        mounted.add(9, three);
        mounted.add(1, one);
        assert_eq!(mounted.of(9), vec![two, three]);
        assert_eq!(mounted.others(9, two), vec![three]);
        mounted.remove(9, three);
        assert!(mounted.others(9, two).is_empty());
        assert!(mounted.contains(9, two) && !mounted.contains(9, three));
    }
}
