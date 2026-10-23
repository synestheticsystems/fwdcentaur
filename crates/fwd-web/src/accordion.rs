//! The inbox's sections, stacked one above another, each opening and closing on its own (D137,
//! Daniel, 2026-10-03, on the mockup that put Waiting on you beside the conversations: "my only
//! feedback thus far is to have each section be stacked vertically in an accordion style"): the
//! region Waiting on you, each of its groups (`waiting.rs`), and the conversations. Each section
//! starts open; a person closes one to put it out of the way, and this browser remembers which are
//! closed, so the inbox opens as it was left (the lead's ruling of 2026-10-03: independent
//! sections, each remembered).
//!
//! One selection runs down the page, as on the inbox's cards before (`cards.rs`): each section's
//! heading, then what the section shows when it is open, top to bottom: the rows of each open
//! group, then, among the conversations, Settled before you looked and its lines, then the cards.
//! A closed section is one stop, its heading. What the keys do on a
//! heading and on a row is worked out here as data and tested natively; the page carries it out
//! (`inbox.rs`, `request_row.rs`).

use crate::cards::Item;
use crate::waiting::{Kind, Sorted};
use ankurah::EntityId;
use std::collections::{BTreeSet, HashSet};

/// The localStorage key under which this browser remembers which of the inbox's sections are
/// closed.
pub const CLOSED_KEY: &str = "fwd.web.inbox-closed";

/// A section of the inbox, under a heading of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Section {
    /// Waiting on you: the requests, in their groups.
    Waiting,
    /// One group of Waiting on you: the rows asking one kind of move.
    Group(Kind),
    /// The conversations, as cards.
    Conversations,
    /// Settled before you looked, at the top of the conversations: a line per task, else per
    /// sender, of the unread notifications that settled before the reader looked (`waiting.rs`).
    Settled,
}

impl Section {
    /// The name this browser remembers the section by, and its heading's element id ends with.
    pub fn key(self) -> String {
        match self {
            Section::Waiting => "waiting".to_string(),
            Section::Group(kind) => format!("group-{}", kind.key()),
            Section::Conversations => "conversations".to_string(),
            Section::Settled => "settled".to_string(),
        }
    }

    /// The section [`Section::key`] names.
    pub fn keyed(key: &str) -> Option<Section> {
        match key {
            "waiting" => Some(Section::Waiting),
            "conversations" => Some(Section::Conversations),
            "settled" => Some(Section::Settled),
            _ => key.strip_prefix("group-").and_then(Kind::keyed).map(Section::Group),
        }
    }
}

/// Which of the inbox's sections are closed. Each starts open.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Closed(BTreeSet<Section>);

impl Closed {
    pub fn is_open(&self, section: Section) -> bool {
        !self.0.contains(&section)
    }

    /// Opens the section, or closes it.
    pub fn set(&mut self, section: Section, open: bool) {
        if open {
            self.0.remove(&section);
        } else {
            self.0.insert(section);
        }
    }

    /// What this browser keeps of it: the closed sections' names, one per line.
    pub fn text(&self) -> String {
        self.0.iter().map(|s| s.key()).collect::<Vec<_>>().join("\n")
    }

    /// The sections `text` names as closed ([`Closed::text`]); a name this build does not know, as
    /// one a later build wrote, is passed over.
    pub fn from_text(text: &str) -> Closed {
        Closed(text.lines().filter_map(|line| Section::keyed(line.trim())).collect())
    }
}

/// The sections this browser remembers closed, when it lets the page read them; else none.
pub fn remembered() -> Closed {
    let storage = web_sys::window().and_then(|w| w.local_storage().ok().flatten());
    storage.and_then(|s| s.get_item(CLOSED_KEY).ok().flatten()).map(|text| Closed::from_text(&text)).unwrap_or_default()
}

/// Remembers `closed` when the browser lets the page write there; it holds for the visit either way.
pub fn remember(closed: &Closed) {
    if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = storage.set_item(CLOSED_KEY, &closed.text());
    }
}

/// What the keyboard steps through on the inbox, top to bottom, as the page shows it from `sorted`:
/// the heading of Waiting on you; while it is open, each group's heading and, while the group is
/// open, the rows it shows (its first eight, or every one once `expanded` holds its kind); then the
/// heading of the conversations and, while it is open, the heading of Settled before you looked
/// when anything settled, and while that is open each of its lines, then `cards`, the cards' keys
/// in their order.
pub fn stops(sorted: &Sorted, closed: &Closed, expanded: &HashSet<Kind>, cards: &[EntityId]) -> Vec<Item> {
    let mut stops = vec![Item::Section(Section::Waiting)];
    if closed.is_open(Section::Waiting) {
        for group in &sorted.groups {
            let section = Section::Group(group.kind);
            stops.push(Item::Section(section));
            if closed.is_open(section) {
                let shown = group.shown(expanded.contains(&group.kind));
                stops.extend(shown.iter().flat_map(|b| b.rows.iter()).map(|r| Item::Request(r.subject)));
            }
        }
    }
    stops.push(Item::Section(Section::Conversations));
    if closed.is_open(Section::Conversations) {
        if !sorted.folds.is_empty() {
            stops.push(Item::Section(Section::Settled));
            if closed.is_open(Section::Settled) {
                stops.extend(sorted.folds.iter().map(|f| Item::Fold(f.key())));
            }
        }
        stops.extend(cards.iter().copied().map(Item::Thread));
    }
    stops
}

/// Whether `item` stands within `section` as the page shows `sorted`: a row within its group and
/// within Waiting on you, a group's heading within Waiting on you, and a card within the
/// conversations. So that closing a section does not leave the selection on something it hid, the
/// selection moves to the section's heading when it stood within it.
pub fn within(item: Item, section: Section, sorted: &Sorted) -> bool {
    match (item, section) {
        (Item::Request(subject), Section::Waiting) => sorted.row(subject).is_some(),
        (Item::Request(subject), Section::Group(kind)) => sorted.row(subject).is_some_and(|r| r.kind == kind),
        (Item::Section(Section::Group(_)), Section::Waiting) => true,
        (Item::Thread(_) | Item::Section(Section::Settled), Section::Conversations) => true,
        (Item::Fold(_), Section::Settled | Section::Conversations) => true,
        _ => false,
    }
}

/// What a key pressed on a section's heading does, the section being `open` or not: the state to
/// give it, or none for a key it leaves to the page. ArrowRight opens a closed section and
/// ArrowLeft closes an open one, as on the rows of a thread; Enter and Space open or close it.
pub fn on_heading(key: &str, open: bool) -> Option<bool> {
    match key {
        "ArrowRight" if !open => Some(true),
        "ArrowLeft" if open => Some(false),
        "Enter" | " " => Some(!open),
        _ => None,
    }
}

/// What a key pressed on a row of Waiting on you does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowStep {
    /// Opens what the row holds beneath it: a task's or a ruling's record with its comments, a
    /// question's thread.
    Open,
    /// Closes it again.
    Close,
    /// Gives the keyboard to the first row of the thread beneath the open row.
    IntoThread,
    /// Selects the heading of the row's group.
    ToGroup,
    /// Opens the subject's own page, or on a question's row, its reply box.
    Activate,
}

/// What a key pressed on a row does, the row being `open` or not, as on the rows of a thread
/// (`navigation::on_row`): ArrowRight opens a closed row, then goes into what it holds; ArrowLeft
/// closes an open row, then goes up to its group's heading; Enter opens the subject's page. None
/// for a key the row leaves to the page (ArrowUp and ArrowDown select the stop above or below).
pub fn on_row(key: &str, open: bool) -> Option<RowStep> {
    match key {
        "ArrowRight" if open => Some(RowStep::IntoThread),
        "ArrowRight" => Some(RowStep::Open),
        "ArrowLeft" if open => Some(RowStep::Close),
        "ArrowLeft" => Some(RowStep::ToGroup),
        "Enter" => Some(RowStep::Activate),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::waiting::{Batch, Fold, Group, Row};

    fn id(n: u8) -> EntityId {
        EntityId::from_bytes([n; 32])
    }

    fn row(kind: Kind, subject: u8) -> Row {
        Row {
            kind,
            subject: id(subject),
            members: vec![id(subject + 100)],
            comments: Vec::new(),
            comments_unread: 0,
            task: None,
            unread: true,
            waits: true,
            sender: None,
            scope: None,
            ready: true,
            priority: None,
            first: 0,
            newest: 0,
        }
    }

    /// Two questions and ten rulings waiting; two cards.
    fn inbox() -> (Sorted, Vec<EntityId>) {
        let questions = Group {
            kind: Kind::Question,
            batches: vec![Batch {
                sender: None,
                scope: None,
                rows: vec![row(Kind::Question, 1), row(Kind::Question, 2)],
            }],
        };
        let rulings = Group {
            kind: Kind::Ruling,
            batches: vec![Batch { sender: None, scope: None, rows: (10..20).map(|n| row(Kind::Ruling, n)).collect() }],
        };
        (Sorted { groups: vec![questions, rulings], ..Sorted::default() }, vec![id(50), id(51)])
    }

    #[test]
    fn the_keyboard_steps_through_each_heading_and_what_each_open_section_shows() {
        let (sorted, cards) = inbox();
        let all = stops(&sorted, &Closed::default(), &HashSet::new(), &cards);
        let mut expected = vec![
            Item::Section(Section::Waiting),
            Item::Section(Section::Group(Kind::Question)),
            Item::Request(id(1)),
            Item::Request(id(2)),
            Item::Section(Section::Group(Kind::Ruling)),
        ];
        expected.extend((10..18).map(|n| Item::Request(id(n))));
        expected.extend([Item::Section(Section::Conversations), Item::Thread(id(50)), Item::Thread(id(51))]);
        assert_eq!(all, expected, "eight rulings until the rest are asked for");
        let expanded: HashSet<Kind> = [Kind::Ruling].into();
        assert_eq!(stops(&sorted, &Closed::default(), &expanded, &cards).len(), expected.len() + 2);
    }

    #[test]
    fn a_closed_section_is_its_heading_alone_and_each_closes_on_its_own() {
        let (sorted, cards) = inbox();
        let mut closed = Closed::default();
        closed.set(Section::Group(Kind::Ruling), false);
        closed.set(Section::Conversations, false);
        assert_eq!(
            stops(&sorted, &closed, &HashSet::new(), &cards),
            vec![
                Item::Section(Section::Waiting),
                Item::Section(Section::Group(Kind::Question)),
                Item::Request(id(1)),
                Item::Request(id(2)),
                Item::Section(Section::Group(Kind::Ruling)),
                Item::Section(Section::Conversations),
            ]
        );
        closed.set(Section::Waiting, false);
        assert_eq!(
            stops(&sorted, &closed, &HashSet::new(), &cards),
            vec![Item::Section(Section::Waiting), Item::Section(Section::Conversations)],
            "the region closed hides its groups, whatever each of them is"
        );
        closed.set(Section::Waiting, true);
        assert!(!closed.is_open(Section::Group(Kind::Ruling)), "opening the region leaves its groups as they were");
    }

    /// A line of Settled before you looked about task `task`.
    fn fold(task: u8) -> Fold {
        Fold { task: Some(id(task)), sender: None, members: vec![id(task + 100)], words: "1 comment".into(), newest: 0 }
    }

    #[test]
    fn settled_before_you_looked_stands_first_among_the_conversations_and_closes_on_its_own() {
        let (mut sorted, cards) = inbox();
        sorted.folds = vec![fold(60), fold(61)];
        let all = stops(&sorted, &Closed::default(), &HashSet::new(), &cards);
        let at = all.iter().position(|s| *s == Item::Section(Section::Conversations)).expect("the conversations");
        assert_eq!(
            all[at..].to_vec(),
            vec![
                Item::Section(Section::Conversations),
                Item::Section(Section::Settled),
                Item::Fold(id(60)),
                Item::Fold(id(61)),
                Item::Thread(id(50)),
                Item::Thread(id(51)),
            ]
        );
        let mut closed = Closed::default();
        closed.set(Section::Settled, false);
        let all = stops(&sorted, &closed, &HashSet::new(), &cards);
        assert!(all.contains(&Item::Section(Section::Settled)) && !all.contains(&Item::Fold(id(60))));
        assert!(all.contains(&Item::Thread(id(50))), "the cards stay");
        assert!(within(Item::Fold(id(60)), Section::Settled, &sorted));
        assert!(within(Item::Fold(id(60)), Section::Conversations, &sorted));
        assert!(within(Item::Section(Section::Settled), Section::Conversations, &sorted));
        // Nothing settled, no heading for it.
        sorted.folds.clear();
        assert!(!stops(&sorted, &Closed::default(), &HashSet::new(), &cards).contains(&Item::Section(Section::Settled)));
    }

    #[test]
    fn this_browser_remembers_the_closed_sections_by_name() {
        let mut closed = Closed::default();
        for section in [Section::Waiting, Section::Group(Kind::Assignment), Section::Conversations] {
            closed.set(section, false);
        }
        assert_eq!(Closed::from_text(&closed.text()), closed);
        assert_eq!(closed.text(), "waiting\ngroup-assigned\nconversations");
        let later = Closed::from_text("group-rulings\ngroup-something-new\n\nconversations");
        assert!(!later.is_open(Section::Group(Kind::Ruling)) && !later.is_open(Section::Conversations));
        assert!(later.is_open(Section::Waiting), "a name this build does not know is passed over");
        for kind in Kind::ALL {
            assert_eq!(Section::keyed(&Section::Group(kind).key()), Some(Section::Group(kind)));
        }
        assert_eq!(Section::keyed("settled"), Some(Section::Settled));
    }

    #[test]
    fn closing_a_section_takes_the_selection_from_what_it_hides() {
        let (sorted, _) = inbox();
        assert!(within(Item::Request(id(11)), Section::Group(Kind::Ruling), &sorted));
        assert!(within(Item::Request(id(11)), Section::Waiting, &sorted));
        assert!(!within(Item::Request(id(11)), Section::Group(Kind::Question), &sorted));
        assert!(within(Item::Section(Section::Group(Kind::Ruling)), Section::Waiting, &sorted));
        assert!(!within(Item::Section(Section::Waiting), Section::Waiting, &sorted), "its own heading stays");
        assert!(within(Item::Thread(id(50)), Section::Conversations, &sorted));
        assert!(!within(Item::Thread(id(50)), Section::Waiting, &sorted));
    }

    #[test]
    fn the_arrows_open_and_close_a_heading_as_they_do_a_row() {
        assert_eq!(on_heading("ArrowRight", false), Some(true));
        assert_eq!(on_heading("ArrowRight", true), None, "already open");
        assert_eq!(on_heading("ArrowLeft", true), Some(false));
        assert_eq!(on_heading("ArrowLeft", false), None);
        assert_eq!(on_heading("Enter", true), Some(false));
        assert_eq!(on_heading(" ", false), Some(true));
        assert_eq!(on_heading("ArrowDown", true), None, "the page's: the stop below");
    }

    #[test]
    fn on_a_row_right_opens_then_enters_and_left_closes_then_climbs() {
        assert_eq!(on_row("ArrowRight", false), Some(RowStep::Open));
        assert_eq!(on_row("ArrowRight", true), Some(RowStep::IntoThread));
        assert_eq!(on_row("ArrowLeft", true), Some(RowStep::Close));
        assert_eq!(on_row("ArrowLeft", false), Some(RowStep::ToGroup));
        assert_eq!(on_row("Enter", false), Some(RowStep::Activate));
        assert_eq!(on_row("ArrowUp", false), None);
        assert_eq!(on_row("Tab", false), None, "Tab walks the row's moves");
    }
}
