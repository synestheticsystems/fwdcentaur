//! What refers to an entity, worked out as data, so the page can draw beneath each row the rows
//! of the conversation about its entity: under a message its replies, under a task its comments
//! and the messages about it, under a decision its comments, and so on down. This module decides
//! which entities show under which, in what order, and what a thread must say when it does not
//! know yet; `thread.rs` draws the rows, and each row's module (`message_row.rs`, `task_row.rs`,
//! `decision_row.rs`) declares the links its model follows. Only messages hang beneath a row: an
//! entity joined by a relation is a lozenge on the row instead (`lozenges.rs`).
//!
//! It is written for an entity-component graph, where one entity id carries several models at
//! once: Ankurah 0.10 records the set of models each entity carries, and Manifold's generic
//! models (an edge, an annotation with an author and a thread, an anchor, an actor, a titled
//! item) are meant to sit on the same entities as fwdcentaur's own. So nothing here asks what
//! kind of thing an entity is. A child is its id plus the set of models it carries; the links to
//! follow beneath an entity are declared per model, and an entity that carries two models follows
//! the links of both; a row's component is chosen by matching the models its entity carries.
//! Every fwdcentaur entity carries one model today, and nothing here relies on that.
//!
//! Every kind of entity is drawn in four modes ([`Mode`]: a lozenge, a list item, embedded and
//! full; Daniel, 2026-09-28, "Every kind of entity renders in four modes"), and the component
//! that draws an entity in each of them is chosen the same way, by the models it carries
//! ([`Registry`]; `modes.rs` holds the page's registry).
//!
//! Nothing in this module touches Leptos, Ankurah or the browser, so its rules are tested natively
//! (`cargo test --manifest-path crates/fwd-web/Cargo.toml`).

use std::collections::{HashMap, HashSet};
use std::hash::Hash;

/// The models an entity carries, in the page's own names for them: a small set, kept sorted so
/// that two sets holding the same models are equal.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Models<M>(Vec<M>);

impl<M> Default for Models<M> {
    /// No models: an entity the page knows nothing about yet.
    fn default() -> Self {
        Models(Vec::new())
    }
}

impl<M: Copy + Ord> Models<M> {
    pub fn new(models: impl IntoIterator<Item = M>) -> Models<M> {
        let mut models: Vec<M> = models.into_iter().collect();
        models.sort();
        models.dedup();
        Models(models)
    }

    pub fn one(model: M) -> Models<M> {
        Models::new([model])
    }

    pub fn contains(&self, model: M) -> bool {
        self.0.binary_search(&model).is_ok()
    }

    /// Whether every model in `other` is among these.
    pub fn contains_all(&self, other: &Models<M>) -> bool {
        other.0.iter().all(|model| self.contains(*model))
    }

    pub fn iter(&self) -> impl Iterator<Item = M> + '_ {
        self.0.iter().copied()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Which links lead from an entity to its children, declared for each model separately, as data:
/// under an entity that carries a message, its replies; under one that carries a task, its
/// comments and the messages about it. An entity that carries several models follows the links of
/// every one of them, each link once, so a child reached through a link two models share is not
/// listed twice.
pub struct Rules<M: 'static, L: 'static> {
    table: &'static [(M, &'static [L])],
}

impl<M: Copy + Ord, L: Copy + Eq> Rules<M, L> {
    pub const fn new(table: &'static [(M, &'static [L])]) -> Rules<M, L> {
        Rules { table }
    }

    /// The links to follow beneath an entity that carries `models`, in the order the table
    /// declares them.
    pub fn links(&self, models: &Models<M>) -> Vec<L> {
        let mut links: Vec<L> = Vec::new();
        for (model, declared) in self.table {
            if models.contains(*model) {
                for link in *declared {
                    if !links.contains(link) {
                        links.push(*link);
                    }
                }
            }
        }
        links
    }
}

/// One entity that refers to another, as the query that found it reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Child<I, M, L> {
    pub id: I,
    /// The models the child carries, which decide the links followed beneath it and how its row
    /// is drawn.
    pub models: Models<M>,
    /// The link that placed it under its parent. It is part of the child's appearance, so the
    /// same entity reached through two different links shows as two rows.
    pub link: L,
    /// How that link reads beside the row ("about it"); empty when it goes without saying.
    pub label: String,
    /// When the child was created, in milliseconds. Siblings list oldest first, and by id between
    /// equal times, so every client lists them in the same order.
    pub created: i64,
}

/// What one link's query reports about the children of one entity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Listing<I, M, L> {
    /// The query has not answered yet: the children are unknown, which is not the same as none.
    Loading,
    /// The query answered with these children.
    Ready(Vec<Child<I, M, L>>),
    /// The query failed, for this reason (a sentence).
    Failed(String),
}

/// One place an entity shows on the page: as the entity a card is about (the root of that card's
/// rows), or as a child reached from a parent appearance through one link. A row keeps its focus, its draft and its
/// expansion for as long as its appearance stays, whatever arrives above or below it, so rows are
/// keyed by appearance, never by their position or by the bare entity id (one entity can show in
/// several places at once).
///
/// Appearances are handed out by [`Appearances`], which answers the same appearance for the same
/// place every time. An appearance means something only to the `Appearances` that made it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Appearance(u32);

impl Appearance {
    /// A number that no other appearance of the same `Appearances` has, for element ids.
    pub fn index(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Place<I, L> {
    Root { scope: I, id: I },
    Child { parent: Appearance, link: L, id: I },
}

/// Hands out appearances and remembers where each one is: its entity, its parent and its path.
pub struct Appearances<I, L> {
    places: Vec<Place<I, L>>,
    index: HashMap<Place<I, L>, Appearance>,
}

impl<I: Copy + Eq + Hash, L: Copy + Eq + Hash> Default for Appearances<I, L> {
    fn default() -> Self {
        Appearances { places: Vec::new(), index: HashMap::new() }
    }
}

impl<I: Copy + Eq + Hash, L: Copy + Eq + Hash> Appearances<I, L> {
    pub fn new() -> Self {
        Self::default()
    }

    fn intern(&mut self, place: Place<I, L>) -> Appearance {
        if let Some(appearance) = self.index.get(&place) {
            return *appearance;
        }
        let appearance = Appearance(u32::try_from(self.places.len()).expect("fewer than 2^32 appearances"));
        self.places.push(place);
        self.index.insert(place, appearance);
        appearance
    }

    /// The appearance of `id` as the entity a card is about, drawn inside `scope` (the card's own
    /// id), so the same entity heading two cards has two roots.
    pub fn root(&mut self, scope: I, id: I) -> Appearance {
        self.intern(Place::Root { scope, id })
    }

    /// The appearance of `id` under `parent`, placed there by `link`.
    pub fn child(&mut self, parent: Appearance, link: L, id: I) -> Appearance {
        self.intern(Place::Child { parent, link, id })
    }

    /// The appearance with this index, when this `Appearances` made one (an element id read back).
    pub fn get(&self, index: u32) -> Option<Appearance> {
        ((index as usize) < self.places.len()).then_some(Appearance(index))
    }

    fn place(&self, appearance: Appearance) -> Place<I, L> {
        self.places[appearance.0 as usize]
    }

    /// The entity that shows at `appearance`.
    pub fn entity(&self, appearance: Appearance) -> I {
        match self.place(appearance) {
            Place::Root { id, .. } | Place::Child { id, .. } => id,
        }
    }

    /// The appearance `appearance` sits under; none for the entity a card is about.
    pub fn parent(&self, appearance: Appearance) -> Option<Appearance> {
        match self.place(appearance) {
            Place::Root { .. } => None,
            Place::Child { parent, .. } => Some(parent),
        }
    }

    /// The appearances from the root of `appearance`'s card down to `appearance`, the root first.
    pub fn path(&self, appearance: Appearance) -> Vec<Appearance> {
        let mut path = vec![appearance];
        let mut at = appearance;
        while let Some(parent) = self.parent(at) {
            path.push(parent);
            at = parent;
        }
        path.reverse();
        path
    }

    /// The root of the card `appearance` belongs to.
    pub fn root_of(&self, appearance: Appearance) -> Appearance {
        self.path(appearance)[0]
    }

    /// How far below its card's root `appearance` sits: 0 for the root, 1 for what refers to the
    /// root directly.
    pub fn depth(&self, appearance: Appearance) -> usize {
        self.path(appearance).len() - 1
    }

    /// Whether `appearance` is `ancestor` or sits somewhere beneath it.
    pub fn is_within(&self, appearance: Appearance, ancestor: Appearance) -> bool {
        self.path(appearance).contains(&ancestor)
    }
}

/// One child in the form its thread shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row<I, M, L> {
    pub appearance: Appearance,
    pub child: Child<I, M, L>,
    /// Where the same entity already shows on this row's own path, when it does. The row then
    /// names the entity and leads back to that appearance instead of listing its children again,
    /// so a cycle in the graph ends in a reference rather than going round forever or passing for
    /// a leaf.
    pub cycle: Option<Appearance>,
}

/// What shows beneath one appearance: its rows, and what the thread has to say besides them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Level<I, M, L> {
    pub rows: Vec<Row<I, M, L>>,
    /// Children found but held back by the level's limit, behind "N more".
    pub hidden: usize,
    /// True while a link's query has not answered, so the rows so far may not be all there is.
    pub loading: bool,
    /// Why a link's children could not be listed, a sentence for each link whose query failed.
    pub failed: Vec<String>,
}

/// A level's state as a person reads it. A thread never shows an empty list for something it does
/// not know: a level that is still loading or has failed says so.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// A link's query has not answered; the rows found so far show, with "loading" below them.
    Loading,
    /// Every link answered and every child shows (possibly none, for a leaf).
    Complete,
    /// Every link answered and `hidden` more children exist than show.
    Truncated { hidden: usize },
    /// A link's query failed; the rows the other links found show, with the reason below them.
    Unavailable { reason: String },
}

impl<I, M, L> Level<I, M, L> {
    pub fn empty() -> Self {
        Level { rows: Vec::new(), hidden: 0, loading: false, failed: Vec::new() }
    }

    /// Every child found, shown or held back.
    pub fn total(&self) -> usize {
        self.rows.len() + self.hidden
    }

    /// A failure outweighs loading, and loading outweighs a limit: each hides more of the truth.
    pub fn state(&self) -> State {
        if !self.failed.is_empty() {
            State::Unavailable { reason: self.failed.join("; ") }
        } else if self.loading {
            State::Loading
        } else if self.hidden > 0 {
            State::Truncated { hidden: self.hidden }
        } else {
            State::Complete
        }
    }

    /// Whether opening this level could show anything: false only for a leaf, a level every link
    /// answered with no children at all.
    pub fn may_have_children(&self) -> bool {
        self.total() > 0 || self.loading || !self.failed.is_empty()
    }
}

/// The level beneath `parent`, from what each link's query reports: every child, each appearance
/// once, oldest first (by `created`, then by id), the first `limit` of them as rows. A child whose
/// entity already shows on `parent`'s path becomes a reference to that earlier appearance.
pub fn level<I, M, L>(
    appearances: &mut Appearances<I, L>,
    parent: Appearance,
    listings: impl IntoIterator<Item = Listing<I, M, L>>,
    limit: usize,
) -> Level<I, M, L>
where
    I: Copy + Ord + Hash,
    M: Copy + Ord,
    L: Copy + Eq + Hash,
{
    let mut found: Vec<Child<I, M, L>> = Vec::new();
    let mut loading = false;
    let mut failed = Vec::new();
    for listing in listings {
        match listing {
            Listing::Loading => loading = true,
            Listing::Ready(children) => found.extend(children),
            Listing::Failed(reason) => failed.push(reason),
        }
    }
    found.sort_by_key(|child| (child.created, child.id));
    let path: Vec<(Appearance, I)> =
        appearances.path(parent).into_iter().map(|appearance| (appearance, appearances.entity(appearance))).collect();
    let mut seen = HashSet::new();
    let mut rows = Vec::new();
    for child in found {
        let appearance = appearances.child(parent, child.link, child.id);
        if !seen.insert(appearance) {
            continue;
        }
        let cycle = path.iter().find(|(_, id)| *id == child.id).map(|(earlier, _)| *earlier);
        rows.push(Row { appearance, child, cycle });
    }
    let hidden = rows.len().saturating_sub(limit);
    rows.truncate(limit);
    Level { rows, hidden, loading, failed }
}

/// The four ways an entity is drawn, from the most compact to the whole thing (Daniel,
/// 2026-09-28: "1. lozenge - the most compact form ... 2. list item - pretty compact form,
/// intended to be presented in lists (like a steps/subtasks list) 3. embedded - medium compact
/// form - shows more contents and does some recursion, but not full form 4. full - the full monty
/// version - renders everything that would be on the \"this thing\" page").
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Mode {
    /// The fewest words that name it, in an outline: on a row's line, in a message's text, beside
    /// a field. A preview on hover, and a click goes to it.
    Lozenge,
    /// One line in a list: a task's steps, the lines of the Tasks, Decisions and Scopes pages.
    ListItem,
    /// More of it, with the conversation about it beneath (the rows of `thread.rs`, recursively),
    /// inside something else: a card's row, a step opened in place.
    Embedded,
    /// Everything its own page shows: a task's or a decision's panel, a highlight's page.
    Full,
}

impl Mode {
    /// The richest mode anything drawn inside a thing drawn in this mode may take. A full page
    /// embeds (its thread, its steps opened in place); an embedded thing embeds again, which is
    /// where the recursion happens, but never another thing in full; a list item names what it
    /// shows as lozenges; a lozenge holds nothing but words.
    pub fn inner(self) -> Mode {
        match self {
            Mode::Full | Mode::Embedded => Mode::Embedded,
            Mode::ListItem | Mode::Lozenge => Mode::Lozenge,
        }
    }

    /// The mode a thing asked for in `wanted` takes inside a thing drawn in this mode: `wanted`,
    /// or the richest this mode allows inside it when `wanted` is richer.
    pub fn nest(self, wanted: Mode) -> Mode {
        wanted.min(self.inner())
    }
}

/// Chooses how an entity is drawn from the models it carries, so an entity that carries a model
/// the page does not know beside ones it does still draws as what it knows, and an entity that
/// carries only unknown models gets the fallback (a row with its id and its models, a lozenge with
/// its short id) rather than nothing.
pub struct Registry<M, R> {
    entries: Vec<(Models<M>, R)>,
}

impl<M: Copy + Ord, R> Default for Registry<M, R> {
    fn default() -> Self {
        Registry { entries: Vec::new() }
    }
}

impl<M: Copy + Ord, R> Registry<M, R> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Draws with `renderer` an entity that carries every one of `models`. Between two entries
    /// that match an entity equally specifically, the one added first wins.
    pub fn add(mut self, models: Models<M>, renderer: R) -> Self {
        self.entries.push((models, renderer));
        self
    }

    /// The renderer for an entity that carries `carried`: of the entries whose models it carries
    /// all of, the most specific (the one naming the most models), the earliest added between
    /// equals. None when no entry matches, and the caller draws its fallback row.
    pub fn choose(&self, carried: &Models<M>) -> Option<&R> {
        let mut best: Option<&(Models<M>, R)> = None;
        for entry in &self.entries {
            if carried.contains_all(&entry.0) && best.is_none_or(|b| entry.0.len() > b.0.len()) {
                best = Some(entry);
            }
        }
        best.map(|(_, renderer)| renderer)
    }
}

/// A small graph for the tests of the threads' pure parts: notes that answer notes, todos and
/// rulings with comments (notes), notes joined to a todo by an edge of their own, and entities
/// that carry two models at once. Every link leads to notes, as every link beneath a row leads to
/// messages in the page: an entity joined by a relation is a lozenge, not a row.
#[cfg(test)]
pub(crate) mod fake {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub enum Model {
        Note,
        Todo,
        /// A ruling, with comments of its own (a decision, in the page).
        Ruling,
        /// A model the page has no renderer for.
        Unknown,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub enum Link {
        /// A note that answers the entity (a field on the note).
        Replies,
        /// A note about the todo or the ruling (a field on the note).
        Comments,
        /// A note joined to the todo by an edge of its own (a `concerns` relation, in the page),
        /// read from the page's own queries rather than a query of its own.
        About,
    }

    pub const RULES: Rules<Model, Link> = Rules::new(&[
        (Model::Note, &[Link::Replies]),
        (Model::Todo, &[Link::Comments, Link::About]),
        (Model::Ruling, &[Link::Comments]),
    ]);

    #[derive(Default)]
    pub struct Graph {
        entities: HashMap<u32, (Models<Model>, i64)>,
        /// (child, link, parent)
        links: Vec<(u32, Link, u32)>,
        /// Links whose query has not answered, by parent.
        pub loading: HashSet<(u32, Link)>,
        /// Links whose query failed, by parent.
        pub failing: HashSet<(u32, Link)>,
    }

    impl Graph {
        pub fn add(&mut self, id: u32, models: &[Model], created: i64) -> &mut Self {
            self.entities.insert(id, (Models::new(models.iter().copied()), created));
            self
        }

        pub fn link(&mut self, child: u32, link: Link, parent: u32) -> &mut Self {
            self.links.push((child, link, parent));
            self
        }

        pub fn models(&self, id: u32) -> Models<Model> {
            self.entities.get(&id).map(|(models, _)| models.clone()).unwrap_or_default()
        }

        pub fn listing(&self, parent: u32, link: Link) -> Listing<u32, Model, Link> {
            if self.failing.contains(&(parent, link)) {
                return Listing::Failed(format!("{link:?} of {parent} could not be listed"));
            }
            if self.loading.contains(&(parent, link)) {
                return Listing::Loading;
            }
            let children = self
                .links
                .iter()
                .filter(|(_, l, p)| *l == link && *p == parent)
                .map(|(child, link, _)| {
                    let (models, created) = self.entities[child].clone();
                    Child { id: *child, models, link: *link, label: String::new(), created }
                })
                .collect();
            Listing::Ready(children)
        }

        /// What each link `parent`'s models follow reports, as a row's level reads it.
        pub fn listings(&self, parent: u32) -> Vec<Listing<u32, Model, Link>> {
            RULES.links(&self.models(parent)).into_iter().map(|link| self.listing(parent, link)).collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{Graph, Link, Model, RULES};
    use super::*;

    fn ids(level: &Level<u32, Model, Link>) -> Vec<u32> {
        level.rows.iter().map(|row| row.child.id).collect()
    }

    #[test]
    fn an_entity_follows_the_links_of_every_model_it_carries_each_once() {
        assert_eq!(RULES.links(&Models::one(Model::Note)), vec![Link::Replies]);
        assert_eq!(RULES.links(&Models::one(Model::Todo)), vec![Link::Comments, Link::About]);
        assert_eq!(RULES.links(&Models::one(Model::Ruling)), vec![Link::Comments]);
        assert_eq!(
            RULES.links(&Models::new([Model::Todo, Model::Note])),
            vec![Link::Replies, Link::Comments, Link::About]
        );
        assert_eq!(
            RULES.links(&Models::new([Model::Ruling, Model::Todo])),
            vec![Link::Comments, Link::About],
            "the comment link the two share is followed once"
        );
        assert!(RULES.links(&Models::one(Model::Unknown)).is_empty());
        // Two models declaring the same link: it is followed once.
        const SHARED: Rules<Model, Link> =
            Rules::new(&[(Model::Note, &[Link::Replies]), (Model::Todo, &[Link::Replies, Link::About])]);
        assert_eq!(SHARED.links(&Models::new([Model::Note, Model::Todo])), vec![Link::Replies, Link::About]);
    }

    #[test]
    fn siblings_list_oldest_first_and_by_id_between_equal_times() {
        let mut graph = Graph::default();
        graph.add(1, &[Model::Note], 10).add(7, &[Model::Note], 30).add(5, &[Model::Note], 20).add(
            3,
            &[Model::Note],
            20,
        );
        graph.link(7, Link::Replies, 1).link(5, Link::Replies, 1).link(3, Link::Replies, 1);
        let mut appearances = Appearances::new();
        let root = appearances.root(100, 1);
        let level = level(&mut appearances, root, graph.listings(1), 50);
        assert_eq!(ids(&level), vec![3, 5, 7]);
        assert_eq!(level.state(), State::Complete);
    }

    #[test]
    fn a_row_keeps_its_appearance_when_siblings_arrive_before_it() {
        let mut graph = Graph::default();
        graph.add(1, &[Model::Note], 0).add(2, &[Model::Note], 20);
        graph.link(2, Link::Replies, 1);
        let mut appearances = Appearances::new();
        let root = appearances.root(100, 1);
        let before = level(&mut appearances, root, graph.listings(1), 50);
        graph.add(3, &[Model::Note], 10).link(3, Link::Replies, 1);
        let after = level(&mut appearances, root, graph.listings(1), 50);
        assert_eq!(ids(&after), vec![3, 2]);
        assert_eq!(
            before.rows[0].appearance, after.rows[1].appearance,
            "the older row moved down but kept its appearance"
        );
    }

    #[test]
    fn an_appearance_is_the_parent_the_link_and_the_entity() {
        let mut appearances: Appearances<u32, Link> = Appearances::new();
        let card = appearances.root(100, 1);
        let other_card = appearances.root(200, 1);
        assert_ne!(card, other_card, "one entity heading two cards has two roots");
        let reply = appearances.child(card, Link::Replies, 2);
        assert_eq!(appearances.child(card, Link::Replies, 2), reply);
        assert_ne!(appearances.child(card, Link::Comments, 2), reply);
        assert_ne!(appearances.child(other_card, Link::Replies, 2), reply);
        assert_eq!(appearances.entity(reply), 2);
        assert_eq!(appearances.parent(reply), Some(card));
        assert_eq!(appearances.parent(card), None);
        let deeper = appearances.child(reply, Link::Replies, 3);
        assert_eq!(appearances.path(deeper), vec![card, reply, deeper]);
        assert_eq!(appearances.root_of(deeper), card);
        assert_eq!(appearances.depth(deeper), 2);
        assert!(appearances.is_within(deeper, card) && !appearances.is_within(card, deeper));
        assert_eq!(appearances.get(deeper.index()), Some(deeper));
        assert_eq!(appearances.get(deeper.index() + 1), None);
    }

    #[test]
    fn an_entity_carrying_two_models_lists_what_refers_to_each_once_per_link() {
        // Entity 1 is a note and a todo at once. Note 2 answers it; note 3 comments on it; note 4
        // is joined to it by an edge; note 5 both answers it and comments on it, so it shows once
        // per link; note 2 reported twice by its query still shows once.
        let mut graph = Graph::default();
        graph.add(1, &[Model::Note, Model::Todo], 0);
        graph.add(2, &[Model::Note], 1).add(3, &[Model::Note], 2).add(4, &[Model::Note], 3).add(5, &[Model::Note], 4);
        graph.link(2, Link::Replies, 1).link(2, Link::Replies, 1).link(3, Link::Comments, 1).link(4, Link::About, 1);
        graph.link(5, Link::Replies, 1).link(5, Link::Comments, 1);
        let mut appearances = Appearances::new();
        let root = appearances.root(100, 1);
        let level = level(&mut appearances, root, graph.listings(1), 50);
        let rows: Vec<(u32, Link)> = level.rows.iter().map(|row| (row.child.id, row.child.link)).collect();
        assert_eq!(
            rows,
            vec![(2, Link::Replies), (3, Link::Comments), (4, Link::About), (5, Link::Replies), (5, Link::Comments)]
        );
    }

    #[test]
    fn a_level_tells_loading_complete_truncated_and_unavailable_apart() {
        let mut graph = Graph::default();
        graph.add(1, &[Model::Todo], 0).add(2, &[Model::Note], 1).add(3, &[Model::Note], 2).add(4, &[Model::Note], 3);
        graph.link(2, Link::Comments, 1).link(3, Link::Comments, 1).link(4, Link::About, 1);
        let mut appearances = Appearances::new();
        let root = appearances.root(100, 1);

        let complete = level(&mut appearances, root, graph.listings(1), 50);
        assert_eq!((complete.state(), complete.total()), (State::Complete, 3));

        let truncated = level(&mut appearances, root, graph.listings(1), 2);
        assert_eq!(truncated.state(), State::Truncated { hidden: 1 });
        assert_eq!((ids(&truncated), truncated.total()), (vec![2, 3], 3));

        graph.loading.insert((1, Link::About));
        let loading = level(&mut appearances, root, graph.listings(1), 50);
        assert_eq!(loading.state(), State::Loading);
        assert_eq!(ids(&loading), vec![2, 3], "the comments already found still show");
        assert!(loading.may_have_children());

        graph.failing.insert((1, Link::Comments));
        let failed = level(&mut appearances, root, graph.listings(1), 50);
        assert!(matches!(failed.state(), State::Unavailable { reason } if reason.contains("Comments of 1")));
        assert!(failed.rows.is_empty() && failed.may_have_children(), "a failure is never a leaf");

        let leaf = level(&mut appearances, root, graph.listings(4), 50);
        assert_eq!(leaf.state(), State::Complete);
        assert!(!leaf.may_have_children());
    }

    #[test]
    fn an_entity_already_on_the_path_becomes_a_reference_to_its_earlier_appearance() {
        // Note 2 answers note 1, and note 1 answers note 2 again (a loop that arrived by sync).
        let mut graph = Graph::default();
        graph.add(1, &[Model::Note], 0).add(2, &[Model::Note], 1);
        graph.link(2, Link::Replies, 1).link(1, Link::Replies, 2);
        let mut appearances = Appearances::new();
        let root = appearances.root(100, 1);
        let first = level(&mut appearances, root, graph.listings(1), 50);
        assert_eq!(first.rows[0].cycle, None);
        let under_two = level(&mut appearances, first.rows[0].appearance, graph.listings(2), 50);
        assert_eq!(ids(&under_two), vec![1]);
        assert_eq!(under_two.rows[0].cycle, Some(root), "the row leads back to the root rather than repeating it");
    }

    #[test]
    fn one_link_lists_children_carrying_different_models_each_drawn_as_its_own() {
        // Note 1's replies: note 2, entity 3 (a note that is also a todo) and entity 4 (a note
        // carrying a model the page does not know too), oldest first whatever their models.
        let mut graph = Graph::default();
        graph.add(1, &[Model::Note], 0).add(2, &[Model::Note], 30).add(3, &[Model::Note, Model::Todo], 10).add(
            4,
            &[Model::Note, Model::Unknown],
            20,
        );
        graph.link(2, Link::Replies, 1).link(3, Link::Replies, 1).link(4, Link::Replies, 1);
        let mut appearances = Appearances::new();
        let root = appearances.root(100, 1);
        let level = level(&mut appearances, root, graph.listings(1), 50);
        assert_eq!(ids(&level), vec![3, 4, 2]);
        let registry = Registry::new()
            .add(Models::one(Model::Note), "note")
            .add(Models::one(Model::Todo), "todo")
            .add(Models::new([Model::Note, Model::Todo]), "note and todo");
        let drawn: Vec<&str> = level.rows.iter().map(|row| *registry.choose(&row.child.models).unwrap()).collect();
        assert_eq!(drawn, vec!["note and todo", "note", "note"]);
    }

    #[test]
    fn an_embedded_thing_never_embeds_another_in_full() {
        use Mode::*;
        // Inside a full page: anything but another full page.
        assert_eq!(Full.nest(Full), Embedded);
        assert_eq!(Full.nest(Embedded), Embedded);
        assert_eq!(Full.nest(ListItem), ListItem);
        // Inside an embedded thing: the recursion goes on, never in full.
        assert_eq!(Embedded.nest(Full), Embedded);
        assert_eq!(Embedded.nest(Embedded), Embedded);
        assert_eq!(Embedded.nest(Lozenge), Lozenge);
        // Inside a list item or a lozenge: lozenges only.
        assert_eq!(ListItem.nest(Embedded), Lozenge);
        assert_eq!(ListItem.nest(Lozenge), Lozenge);
        assert_eq!(Lozenge.nest(Full), Lozenge);
        for outer in [Lozenge, ListItem, Embedded, Full] {
            assert!(outer.nest(Full) < Full, "{outer:?} never holds a thing in full");
        }
    }

    #[test]
    fn each_kind_answers_every_mode_by_the_models_its_entity_carries() {
        // One component per kind, each answering all four modes; the entity's models choose it.
        struct Kind(&'static str);
        impl Kind {
            fn draw(&self, mode: Mode) -> String {
                format!("{} as {mode:?}", self.0)
            }
        }
        let registry = Registry::new()
            .add(Models::one(Model::Note), Kind("note"))
            .add(Models::one(Model::Todo), Kind("todo"))
            .add(Models::new([Model::Note, Model::Todo]), Kind("note and todo"));
        let drawn = |models: Models<Model>, mode| registry.choose(&models).map(|kind| kind.draw(mode));
        for mode in [Mode::Lozenge, Mode::ListItem, Mode::Embedded, Mode::Full] {
            assert_eq!(drawn(Models::one(Model::Todo), mode), Some(format!("todo as {mode:?}")));
            assert_eq!(
                drawn(Models::new([Model::Todo, Model::Note, Model::Unknown]), mode),
                Some(format!("note and todo as {mode:?}")),
                "the most specific kind draws every mode"
            );
            assert_eq!(drawn(Models::one(Model::Unknown), mode), None, "the page's fallback draws it");
        }
    }

    #[test]
    fn the_most_specific_renderer_wins_and_unknown_models_fall_back() {
        let registry = Registry::new()
            .add(Models::one(Model::Note), "note")
            .add(Models::one(Model::Todo), "todo")
            .add(Models::new([Model::Note, Model::Todo]), "note and todo")
            .add(Models::one(Model::Todo), "a second todo renderer");
        assert_eq!(registry.choose(&Models::one(Model::Note)), Some(&"note"));
        assert_eq!(registry.choose(&Models::new([Model::Todo, Model::Note])), Some(&"note and todo"));
        assert_eq!(
            registry.choose(&Models::new([Model::Todo, Model::Unknown])),
            Some(&"todo"),
            "unknown models beside a known one"
        );
        assert_eq!(registry.choose(&Models::one(Model::Todo)), Some(&"todo"), "the earlier of two equal entries");
        assert_eq!(registry.choose(&Models::one(Model::Unknown)), None, "only unknown models: the fallback row");
    }
}
