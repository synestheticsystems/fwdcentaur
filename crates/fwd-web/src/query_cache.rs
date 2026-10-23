//! The live queries behind the threads beneath rows, shared and counted. Every row on the page holds the
//! queries that find its children; rows that show the same entity share one query, and a query
//! closes as soon as the last row holding it leaves the page (a branch closed, a card gone, the
//! page switched to another acting principal). The count of open queries is there to check
//! exactly that: closing a branch, or switching principals, brings it back to where it was.
//!
//! The cache knows nothing of Ankurah. It asks a [`QueryLayer`] to open a query and drops the
//! query to close it (dropping an Ankurah `LiveQuery` unsubscribes it at the daemon), so it is
//! tested natively against a fake layer.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Arc, Mutex, MutexGuard};

/// Opens the queries that find what refers to an entity: the part of a thread that talks to the
/// database.
pub trait QueryLayer: Send + Sync + 'static {
    type Id: Copy + Eq + Hash + Send + Sync + 'static;
    type Link: Copy + Eq + Hash + Send + Sync + 'static;
    /// An open query. Dropping it closes it.
    type Query: Send + Sync + 'static;

    /// Opens the query that finds what refers to `parent` through `link`, or answers `None` when
    /// that link is read from queries the page holds anyway, so following it opens nothing.
    fn open(&self, parent: Self::Id, link: Self::Link) -> Option<Self::Query>;
}

type Watcher = Box<dyn Fn(usize) + Send + Sync>;

struct Entry<T> {
    query: Arc<T>,
    holders: usize,
}

/// The open queries by parent and link.
type Entries<Q> = HashMap<(<Q as QueryLayer>::Id, <Q as QueryLayer>::Link), Entry<<Q as QueryLayer>::Query>>;

struct Shared<Q: QueryLayer> {
    entries: Mutex<Entries<Q>>,
    watcher: Mutex<Option<Watcher>>,
}

impl<Q: QueryLayer> Shared<Q> {
    fn announce(&self) {
        let count = lock(&self.entries).len();
        if let Some(watcher) = lock(&self.watcher).as_ref() {
            watcher(count);
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The page's open child queries, one per entity and link, each with the number of rows that
/// hold it.
pub struct QueryCache<Q: QueryLayer> {
    layer: Q,
    shared: Arc<Shared<Q>>,
}

impl<Q: QueryLayer> QueryCache<Q> {
    pub fn new(layer: Q) -> Self {
        QueryCache {
            layer,
            shared: Arc::new(Shared { entries: Mutex::new(HashMap::new()), watcher: Mutex::new(None) }),
        }
    }

    /// A hold on the query for what refers to `parent` through `link`, which opens the query when
    /// nothing holds it yet. The query stays open while any hold on it lives.
    pub fn hold(&self, parent: Q::Id, link: Q::Link) -> Hold<Q> {
        let key = (parent, link);
        let joined = lock(&self.shared.entries).get_mut(&key).map(|entry| {
            entry.holders += 1;
            entry.query.clone()
        });
        if let Some(query) = joined {
            return Hold { shared: Some(self.shared.clone()), key, query: Some(query) };
        }
        // Opened outside the lock: opening a query may run other code that holds rows.
        let Some(opened) = self.layer.open(parent, link) else {
            return Hold { shared: None, key, query: None };
        };
        let query = {
            let mut entries = lock(&self.shared.entries);
            let entry = entries.entry(key).or_insert_with(|| Entry { query: Arc::new(opened), holders: 0 });
            entry.holders += 1;
            entry.query.clone()
        };
        self.shared.announce();
        Hold { shared: Some(self.shared.clone()), key, query: Some(query) }
    }

    /// How many queries are open. The page learns it through `watch`.
    #[cfg(test)]
    pub fn open_count(&self) -> usize {
        lock(&self.shared.entries).len()
    }

    /// How many holds `parent`'s query for `link` has; 0 when it is not open.
    #[cfg(test)]
    pub fn holders(&self, parent: Q::Id, link: Q::Link) -> usize {
        lock(&self.shared.entries).get(&(parent, link)).map_or(0, |entry| entry.holders)
    }

    /// Calls `watcher` with the number of open queries each time a query opens or closes.
    pub fn watch(&self, watcher: impl Fn(usize) + Send + Sync + 'static) {
        *lock(&self.shared.watcher) = Some(Box::new(watcher));
    }
}

/// One row's hold on one query. Dropping it lets the query go once no other row holds it.
pub struct Hold<Q: QueryLayer> {
    /// None for a link that opened no query.
    shared: Option<Arc<Shared<Q>>>,
    key: (Q::Id, Q::Link),
    query: Option<Arc<Q::Query>>,
}

impl<Q: QueryLayer> Hold<Q> {
    /// The open query, or None for a link read from the page's own queries.
    pub fn query(&self) -> Option<&Q::Query> {
        self.query.as_deref()
    }

    pub fn link(&self) -> Q::Link {
        self.key.1
    }
}

impl<Q: QueryLayer> Drop for Hold<Q> {
    fn drop(&mut self) {
        let Some(shared) = self.shared.take() else { return };
        let removed = {
            let mut entries = lock(&shared.entries);
            match entries.get_mut(&self.key) {
                Some(entry) if entry.holders > 1 => {
                    entry.holders -= 1;
                    None
                }
                Some(_) => entries.remove(&self.key),
                None => None,
            }
        };
        let closed = removed.is_some();
        // The query closes here, outside the lock, as the last handles on it go.
        drop(removed);
        self.query.take();
        if closed {
            shared.announce();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::referents::fake::{Graph, Link, Model, RULES};
    use crate::referents::{self, Appearance, Appearances};
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Counts the queries it opens that have not been dropped yet. The notes joined to a todo by
    /// an edge come from the page's own queries in this fake, as the messages joined to a task by
    /// a relation do in fwdcentaur.
    #[derive(Default)]
    struct FakeLayer {
        live: Arc<AtomicUsize>,
    }

    struct FakeQuery {
        live: Arc<AtomicUsize>,
    }

    impl Drop for FakeQuery {
        fn drop(&mut self) {
            self.live.fetch_sub(1, Ordering::SeqCst);
        }
    }

    impl QueryLayer for FakeLayer {
        type Id = u32;
        type Link = Link;
        type Query = FakeQuery;

        fn open(&self, _parent: u32, link: Link) -> Option<FakeQuery> {
            // A link through relations reads the page's standing queries, as in `thread.rs`.
            if link == Link::About {
                return None;
            }
            self.live.fetch_add(1, Ordering::SeqCst);
            Some(FakeQuery { live: self.live.clone() })
        }
    }

    type Holds = HashMap<Appearance, Vec<Hold<FakeLayer>>>;

    /// Puts the row at `at`, and the rows beneath it, on the page the way `thread.rs` does: every
    /// row on the page holds the queries of the links its models follow (a closed row too, so it
    /// can say how many wait below it); an open row's children are on the page, a closed row's
    /// are not, and a row that stands for a cycle holds nothing.
    fn mount(
        cache: &QueryCache<FakeLayer>,
        graph: &Graph,
        appearances: &mut Appearances<u32, Link>,
        at: Appearance,
        closed: &HashSet<u32>,
        holds: &mut Holds,
    ) {
        let id = appearances.entity(at);
        let links = RULES.links(&graph.models(id));
        holds.insert(at, links.into_iter().map(|link| cache.hold(id, link)).collect());
        if closed.contains(&id) {
            return;
        }
        let level = referents::level(appearances, at, graph.listings(id), 50);
        for row in level.rows.into_iter().filter(|row| row.cycle.is_none()) {
            mount(cache, graph, appearances, row.appearance, closed, holds);
        }
    }

    /// Note 1 has replies 2 and 3; reply 2 has replies 4 and 5, and reply 4 has reply 6. Todo 7
    /// is a card of its own: note 8 comments on it and note 9 is joined to it by an edge.
    fn graph() -> Graph {
        let mut graph = Graph::default();
        for id in 1..=6 {
            graph.add(id, &[Model::Note], i64::from(id));
        }
        graph.add(7, &[Model::Todo], 7).add(8, &[Model::Note], 8).add(9, &[Model::Note], 9);
        graph.link(2, Link::Replies, 1).link(3, Link::Replies, 1).link(4, Link::Replies, 2).link(5, Link::Replies, 2);
        graph.link(6, Link::Replies, 4).link(8, Link::Comments, 7).link(9, Link::About, 7);
        graph
    }

    #[test]
    fn rows_showing_the_same_entity_share_one_query() {
        let layer = FakeLayer::default();
        let live = layer.live.clone();
        let cache = QueryCache::new(layer);
        let first = cache.hold(1, Link::Replies);
        let second = cache.hold(1, Link::Replies);
        assert_eq!((cache.open_count(), cache.holders(1, Link::Replies), live.load(Ordering::SeqCst)), (1, 2, 1));
        drop(first);
        assert_eq!((cache.open_count(), live.load(Ordering::SeqCst)), (1, 1), "still held by the second row");
        drop(second);
        assert_eq!((cache.open_count(), live.load(Ordering::SeqCst)), (0, 0), "the last hold closed it");
    }

    #[test]
    fn a_link_read_from_the_pages_own_queries_opens_nothing() {
        let cache = QueryCache::new(FakeLayer::default());
        let hold = cache.hold(7, Link::About);
        assert!(hold.query().is_none());
        assert_eq!(cache.open_count(), 0);
    }

    #[test]
    fn closing_a_branch_brings_the_open_query_count_back() {
        let graph = graph();
        let layer = FakeLayer::default();
        let live = layer.live.clone();
        let cache = QueryCache::new(layer);
        let mut appearances = Appearances::new();
        let root = appearances.root(100, 1);
        let todo = appearances.root(200, 7);
        let mut holds = Holds::new();
        let closed: HashSet<u32> = [2].into_iter().collect();
        mount(&cache, &graph, &mut appearances, root, &closed, &mut holds);
        mount(&cache, &graph, &mut appearances, todo, &closed, &mut holds);
        // Open now: the replies of 1, 2 (closed, but counting what waits below it) and 3, the
        // comments of todo 7, and the replies of notes 8 and 9; the note joined to the todo by an
        // edge comes from the page's own queries.
        let baseline = cache.open_count();
        assert_eq!(baseline, 6);
        assert_eq!(baseline, live.load(Ordering::SeqCst));

        // The person opens reply 2: its branch comes onto the page and holds its queries.
        let two = appearances.child(root, Link::Replies, 2);
        let level = referents::level(&mut appearances, two, graph.listings(2), 50);
        for row in level.rows {
            mount(&cache, &graph, &mut appearances, row.appearance, &HashSet::new(), &mut holds);
        }
        assert_eq!(cache.open_count(), baseline + 3, "the replies of 4, 5 and 6");

        // And closes it again: the rows beneath it leave the page, dropping their holds.
        let beneath: Vec<Appearance> =
            holds.keys().copied().filter(|a| *a != two && appearances.is_within(*a, two)).collect();
        for appearance in beneath {
            holds.remove(&appearance);
        }
        assert_eq!(cache.open_count(), baseline);
        assert_eq!(live.load(Ordering::SeqCst), baseline, "the queries were dropped, not only uncounted");

        // Switching principals takes every card off the page.
        holds.clear();
        assert_eq!((cache.open_count(), live.load(Ordering::SeqCst)), (0, 0));
    }

    #[test]
    fn two_cards_showing_one_message_hold_its_queries_once() {
        let graph = graph();
        let cache = QueryCache::new(FakeLayer::default());
        let mut appearances = Appearances::new();
        let mut holds = Holds::new();
        let card = appearances.root(100, 2);
        mount(&cache, &graph, &mut appearances, card, &HashSet::new(), &mut holds);
        let alone = cache.open_count();
        let other_card = appearances.root(200, 2);
        mount(&cache, &graph, &mut appearances, other_card, &HashSet::new(), &mut holds);
        assert_eq!(cache.open_count(), alone);
        assert_eq!(cache.holders(2, Link::Replies), 2);
    }

    #[test]
    fn the_watcher_hears_each_query_open_and_close() {
        let cache = QueryCache::new(FakeLayer::default());
        let heard = Arc::new(Mutex::new(Vec::new()));
        let record = heard.clone();
        cache.watch(move |count| record.lock().unwrap().push(count));
        let a = cache.hold(1, Link::Replies);
        let b = cache.hold(1, Link::Replies);
        let c = cache.hold(2, Link::Replies);
        drop(a);
        drop(b);
        drop(c);
        assert_eq!(*heard.lock().unwrap(), vec![1, 2, 1, 0]);
    }
}
