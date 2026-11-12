//! The records the page names and does not list, wherever in the node they are: the far end of a
//! relation, the author of a message and the assignee of a task when they are another project's,
//! a notification's target in another project, a relation a notification is about that another
//! project made, and, on the wallboard, a prerequisite of a card that its selection leaves out. A
//! project is a grouping and a filter on what a list shows, and it does not stop a reference from
//! being followed (docs/DECISIONS.md, D176), so the page reads such a record as the command line
//! does (`fwd_core::project_graph::ProjectGraph::follow`): with its project, which the page names
//! beside it, or as one that cannot be followed here.
//!
//! The page follows what its readers name, for as long as they name it. A lookup that its standing
//! queries cannot answer (`store::task` and its siblings) takes a lease on the id for the closure
//! that asked, which the closure gives back when it runs again or leaves the page, so that each
//! kind's one live query, which names no project, reads the records of the ids some reader names now
//! and no others ([`Following`]). Following what is read, rather than every id the project's records
//! hold, follows a far message's author as readily as a relation's far end, without a list of the
//! fields that hold ids to keep in step with the model, and lets go of a wallboard card's
//! prerequisites once the card is gone. Until the daemon has answered for an id, the page says it
//! is loading; once it has, an id its query does not hold is one the node does not hold.
//!
//! Nothing that lists records reads through these queries: a list is its project's alone.

use ankurah::EntityId;
use fwd_core::project_graph::{InProjectView, LiveFollow};
use leptos::prelude::{on_cleanup, ArcTrigger, Notify, Owner, Track};
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, MutexGuard};
use wasm_bindgen_futures::spawn_local;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// How many readers name each id: an id is followed while any reader names it.
#[derive(Debug, Default)]
pub struct Leases {
    readers: HashMap<EntityId, usize>,
}

impl Leases {
    /// One more reader names `id`: true when no reader named it before.
    pub fn take(&mut self, id: EntityId) -> bool {
        let readers = self.readers.entry(id).or_default();
        *readers += 1;
        *readers == 1
    }

    /// A reader of `id` is gone, and with the last of them the id goes: true when it went.
    pub fn give_back(&mut self, id: EntityId) -> bool {
        let Some(readers) = self.readers.get_mut(&id) else { return false };
        *readers -= 1;
        let gone = *readers == 0;
        if gone {
            self.readers.remove(&id);
        }
        gone
    }

    /// The ids some reader names.
    pub fn ids(&self) -> BTreeSet<EntityId> {
        self.readers.keys().copied().collect()
    }
}

/// What the page can say of an id one of its live reads has not given it a record for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// The daemon has not answered a reading that names the id yet.
    Waiting,
    /// The daemon answered, and the node holds no such record: it cannot be followed here.
    NotHeld,
    /// The last reading failed, for this reason, so the page cannot tell.
    Failed(String),
}

/// What one live read asks the daemon for and what the daemon has answered, apart from the query
/// itself, so that what it says of an id is worked out as data.
#[derive(Debug, Default)]
pub struct Reading {
    leases: Leases,
    /// The ids the query reads for now.
    asked: BTreeSet<EntityId>,
    /// The ids the query read for when the daemon last answered it: an id among them that the
    /// query does not hold is one the node does not hold.
    answered: BTreeSet<EntityId>,
    /// Why the last reading failed, until one is answered.
    failed: Option<String>,
    /// Whether a change of the ids is on its way to the query.
    pending: bool,
}

impl Reading {
    /// A reader names `id`: true when the query must be told, now that it is to read for an id it
    /// was not given, and no change is on its way already.
    pub fn take(&mut self, id: EntityId) -> bool {
        let named = self.leases.take(id);
        self.wants_asking(named)
    }

    /// A reader of `id` is gone: true when the query must be told, as [`Reading::take`].
    pub fn give_back(&mut self, id: EntityId) -> bool {
        let gone = self.leases.give_back(id);
        self.wants_asking(gone)
    }

    /// Whether the query must be told of the ids its readers name, now that they `changed`: a
    /// change already on its way takes the ids as they are when it goes, so it is told once.
    fn wants_asking(&mut self, changed: bool) -> bool {
        let wanted = changed && !self.pending;
        self.pending |= wanted;
        wanted
    }

    /// The ids to read for, when they are not those the query reads for already; the change on its
    /// way arrives with this.
    pub fn next_ids(&mut self) -> Option<BTreeSet<EntityId>> {
        self.pending = false;
        let ids = self.leases.ids();
        (ids != self.asked).then(|| {
            self.asked = ids.clone();
            ids
        })
    }

    /// The daemon answered the query as it reads now.
    pub fn answered(&mut self) {
        self.answered = self.asked.clone();
        self.failed = None;
    }

    /// The query failed, for `reason`.
    pub fn failed(&mut self, reason: String) {
        self.failed = Some(reason);
    }

    /// What the page can say of `id`, which the query does not hold.
    pub fn answer_for(&self, id: EntityId) -> Answer {
        match &self.failed {
            Some(reason) => Answer::Failed(reason.clone()),
            None if self.answered.contains(&id) => Answer::NotHeld,
            None => Answer::Waiting,
        }
    }
}

/// One live read the page follows what its readers name through, and what it has asked and been
/// answered.
pub struct Following<V: InProjectView> {
    live: LiveFollow<V>,
    reading: Arc<Mutex<Reading>>,
    /// Runs again each reader that asked about an id, once the daemon answers the query.
    answers: ArcTrigger,
}

impl<V: InProjectView + Clone + 'static> Following<V> {
    pub fn new(live: LiveFollow<V>) -> Following<V> {
        Following { live, reading: Arc::default(), answers: ArcTrigger::new() }
    }

    /// The live query, to read the records it holds.
    pub fn live(&self) -> &LiveFollow<V> {
        &self.live
    }

    /// The reader running now names `id`, until it runs again or leaves the page, when `let_go`
    /// gives the id back ([`Following::give_back`]): true when the query must be told
    /// ([`Following::ask`]). A lookup made outside any reader, as in an event handler, names
    /// nothing, since nothing would give the id back.
    pub fn name(&self, id: EntityId, let_go: impl FnOnce() + Send + Sync + 'static) -> bool {
        if Owner::current().is_none() {
            return false;
        }
        on_cleanup(let_go);
        lock(&self.reading).take(id)
    }

    /// A reader of `id` is gone: true when the query must be told.
    pub fn give_back(&self, id: EntityId) -> bool {
        lock(&self.reading).give_back(id)
    }

    /// What the page can say of `id`, which the query does not hold: read inside a closure, which
    /// then runs again when the daemon answers.
    pub fn answer_for(&self, id: EntityId) -> Answer {
        self.answers.track();
        lock(&self.reading).answer_for(id)
    }

    /// Tells the query the ids its readers name now, and records the daemon's answer, once the
    /// readers running now have named what they name.
    pub fn ask(following: impl FnOnce() -> &'static Following<V> + 'static) {
        spawn_local(async move {
            let this = following();
            let Some(ids) = lock(&this.reading).next_ids() else { return };
            let answered = match this.live.follow_ids(&ids) {
                Ok(()) => this.live.live().wait_durable_answered().await,
                Err(e) => Err(e),
            };
            match answered {
                Ok(()) => lock(&this.reading).answered(),
                Err(e) => {
                    tracing::warn!("a read of the records the page names failed: {e}");
                    lock(&this.reading).failed(e.to_string());
                }
            }
            this.answers.notify();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> EntityId {
        EntityId::from_bytes([n; 32])
    }

    #[test]
    fn an_id_is_followed_while_any_reader_names_it() {
        let mut leases = Leases::default();
        assert!(leases.take(id(1)), "named for the first time");
        assert!(!leases.take(id(1)));
        leases.take(id(2));
        assert_eq!(leases.ids(), [id(1), id(2)].into());
        assert!(!leases.give_back(id(1)));
        assert_eq!(leases.ids(), [id(1), id(2)].into(), "a second reader still names it");
        assert!(leases.give_back(id(1)), "the last reader let it go");
        assert!(!leases.give_back(id(3)), "one never named changes nothing");
        assert_eq!(leases.ids(), [id(2)].into());
    }

    #[test]
    fn a_reader_running_again_asks_nothing_new_and_a_change_is_asked_once() {
        let mut reading = Reading::default();
        assert!(reading.take(id(1)), "a new id: the query must be told");
        assert!(!reading.take(id(2)), "a change is on its way already");
        assert_eq!(reading.next_ids(), Some([id(1), id(2)].into()));
        // A reader runs again: it gives its id back and names it again before the query is told.
        assert!(reading.give_back(id(1)));
        assert!(!reading.take(id(1)));
        assert_eq!(reading.next_ids(), None, "the same ids: nothing to tell the query");
        assert!(reading.give_back(id(2)));
        assert_eq!(reading.next_ids(), Some([id(1)].into()));
    }

    #[test]
    fn an_id_is_loading_until_a_reading_naming_it_is_answered_and_then_cannot_be_followed_here() {
        let mut reading = Reading::default();
        reading.take(id(1));
        assert_eq!(reading.answer_for(id(1)), Answer::Waiting, "not asked yet");
        reading.next_ids();
        assert_eq!(reading.answer_for(id(1)), Answer::Waiting, "asked, not answered");
        reading.answered();
        assert_eq!(reading.answer_for(id(1)), Answer::NotHeld);
        reading.take(id(2));
        reading.next_ids();
        assert_eq!(reading.answer_for(id(2)), Answer::Waiting, "a reading that names it is on its way");
        reading.failed("the daemon refused".into());
        assert_eq!(reading.answer_for(id(1)), Answer::Failed("the daemon refused".into()), "it cannot tell");
        reading.answered();
        assert_eq!((reading.answer_for(id(1)), reading.answer_for(id(2))), (Answer::NotHeld, Answer::NotHeld));
    }
}
