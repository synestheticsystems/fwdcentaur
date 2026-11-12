//! The page's standing live queries and the lookups over them.
//!
//! A lookup is made inside a reactive closure (a memo, an effect, a closure a `view!` draws): it
//! makes that closure a reader of what it reads, and a lookup of a record the standing queries do
//! not hold names the record's id for the closure, which changes what a live read asks the daemon
//! for, and gives it back when the closure runs again or leaves the page (`followed.rs`). Made
//! where no reactive owner is, as in an event handler, a lookup names nothing, so a record of
//! another project that no closure names is not found there.
//!
//! One query per collection of the project's graph for the whole page (a project holds dozens to
//! hundreds of tasks, not millions), opened once after boot. Every read below is tracked by the reactive_graph
//! bridge, which turns it into a Leptos dependency, so any closure in a `view!` that calls these
//! re-renders when the daemon sends a change. A whole collection (`tasks()`) is copied whole, and
//! its reader runs again at any change to any member. One entity looked up by its id (`task(id)`),
//! or the relations at one entity (`relations_of`), is copied alone: its reader runs again when
//! that entity arrives or leaves ([`Arrivals`]), and, through the fields it reads, when it changes.
//! A row looks up what it names by id, so that the page's work at each change grows with the rows
//! that name what changed rather than with its rows times everything it holds (tasks GEQEKfUd,
//! unFIWd5c).
//!
//! The standing queries hold the project's records, which every list shows. A lookup by id answers
//! for a record of another project too, which the page names and does not list (the far end of a
//! relation, a message's author there, a dependency there, a relation a notification is about),
//! from the live reads `followed.rs` keeps; the record's project says whose it is ([`whose`]), and
//! the page names a record of another project with its project's name ([`named`]). The relations at
//! a record ([`relations_of`]) are those its project made and those another project made that
//! touch it, each kept while either read holds it ([`Ends`]).

use ankurah::changes::{ChangeSet, ItemChange};
use ankurah::signals::{Get as AnkurahGet, Subscribe, SubscriptionGuard};
use ankurah::{EntityId, LiveQuery, View};
use fwd_core::dormancy;
use fwd_core::former_ids::FormerIds;
use fwd_core::model::*;
use fwd_core::naming::{self, Named, Work};
use fwd_core::ops;
use fwd_core::project_graph::{Followed, InProjectView, ProjectGraph, ProjectRecord, Whose};
use fwd_core::rules::{self, Readiness, TaskSnapshot};
use leptos::prelude::{untrack, ArcRwSignal, ArcTrigger, Get, Notify, Set, Track};
use send_wrapper::SendWrapper;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use wasm_bindgen_futures::spawn_local;

use crate::clock;
use crate::followed::{Answer, Following};
use crate::session::graph;
use crate::thread::Model;
use std::fmt;

/// One kind of record the page reads: its project's, which its lists show, and those it names and
/// does not list, wherever in the node they are (`followed.rs`).
struct Records<V: InProjectView> {
    listed: LiveQuery<V>,
    named: Following<V>,
}

impl<V: InProjectView + Clone + 'static> Records<V> {
    fn open(graph: &ProjectGraph, listed: LiveQuery<V>) -> Result<Records<V>, ankurah::error::RetrievalError> {
        Ok(Records { listed, named: Following::new(graph.follow_live(&BTreeSet::new())?) })
    }
}

struct Store {
    /// The record of the project the page shows, which every record of its lists names.
    here: EntityId,
    tasks: Records<TaskView>,
    /// The project's relations, and those the page names by their ids wherever they were made (a
    /// notification's relation).
    relations: Records<RelationView>,
    /// The relations other projects made from or to the records whose relations a reader reads.
    relations_elsewhere: Following<RelationView>,
    /// Every project of the node, by whose names the page names a record of another project.
    projects: LiveQuery<ProjectView>,
    /// The collections the page reads beside its tasks and their relations, each opened only when
    /// the page names it.
    scopes: Option<Records<ScopeView>>,
    principals: Option<Records<PrincipalView>>,
    messages: Option<Records<MessageView>>,
    anchors: Option<Records<CodeAnchorView>>,
    decisions: Option<Records<DecisionView>>,
    former_ids: Option<LiveQuery<FormerIdView>>,
    /// Whether the daemon has answered every standing query above (`standing_loaded`).
    answered: ArcRwSignal<bool>,
    /// The readers of each entity looked up by its id in the queries above.
    arrivals: Arrivals,
    /// The relations at each entity (`relations_of`).
    ends: Ends,
    /// What keeps the two above current, for as long as the page.
    _watching: Vec<SubscriptionGuard>,
    /// What the first open asked for, which holds for as long as the page.
    opened_with: OpenedWith,
}

/// What the store was opened with: what the task query selects and the collections opened beside
/// it. The collections are a set, so that a later open naming them in another order is admitted.
struct OpenedWith {
    tasks: Tasks,
    reads: BTreeSet<Collection>,
}

/// The tasks of the project the store's task query holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tasks {
    /// Every task, as the app reads them.
    Every,
    /// The tasks a predicate selects, as the wallboard reads them.
    #[cfg(feature = "wallboard")]
    Selected(String),
}

impl fmt::Display for Tasks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Tasks::Every => f.write_str("every task"),
            #[cfg(feature = "wallboard")]
            Tasks::Selected(predicate) => write!(f, "`{predicate}`"),
        }
    }
}

impl OpenedWith {
    fn new(tasks: Tasks, reads: &[Collection]) -> Self {
        OpenedWith { tasks, reads: reads.iter().copied().collect() }
    }

    /// A later open asking for `asked`: other tasks or other collections are refused, since the
    /// store cannot give them without opening again, and are never ignored.
    fn admit(&self, asked: &OpenedWith) -> Result<(), String> {
        if self.tasks != asked.tasks {
            Err(format!("the task query already selects {}, not {}", self.tasks, asked.tasks))
        } else if self.reads != asked.reads {
            Err(format!("the store already reads {:?}, not {:?}", self.reads, asked.reads))
        } else {
            Ok(())
        }
    }
}

/// A collection the store keeps a standing query on beside the tasks and their relations, which
/// every page reads. A page names the collections it reads, and the store opens those alone, so
/// that a page loads, caches and waits for nothing it never shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Collection {
    Scopes,
    Principals,
    Messages,
    Anchors,
    Decisions,
    /// The ids the project's records had before they were renumbered, which a route, a typed name
    /// and a text written before then may name (`fwd_core::former_ids`).
    FormerIds,
}

impl Collection {
    /// Every collection: the app's pages read them all between them.
    pub const ALL: [Collection; 6] = [
        Collection::Scopes,
        Collection::Principals,
        Collection::Messages,
        Collection::Anchors,
        Collection::Decisions,
        Collection::FormerIds,
    ];
}

static STORE: OnceLock<SendWrapper<Store>> = OnceLock::new();

/// Opens the app's standing queries: every task, and every collection.
pub fn open() -> Result<(), String> {
    open_with(Tasks::Every, &Collection::ALL)
}

/// Opens the standing queries on the project's graph: the tasks `tasks` names, their relations,
/// and the collections `reads` names, in any order. The store lives as long as the page, so its
/// first open binds both: other tasks or other collections are refused, never ignored
/// ([`OpenedWith`]).
pub fn open_with(tasks: Tasks, reads: &[Collection]) -> Result<(), String> {
    let asked = OpenedWith::new(tasks, reads);
    if let Some(store) = STORE.get() {
        return store.opened_with.admit(&asked);
    }
    let graph = graph();
    let refused =
        |what: &'static str| move |e: ankurah::error::RetrievalError| format!("the {what} query was refused: {e}");
    // The query of each collection `reads` names, and none for the others.
    let named = |collection| asked.reads.contains(&collection);
    // Each collection's records of the project, and the reads of those of that kind the page names.
    fn records<V: InProjectView + Clone + 'static>(
        graph: &ProjectGraph,
        listed: Result<LiveQuery<V>, ankurah::error::RetrievalError>,
    ) -> Result<Records<V>, ankurah::error::RetrievalError> {
        Records::open(graph, listed?)
    }
    let tasks = records(
        &graph,
        match &asked.tasks {
            Tasks::Every => graph.query_all::<TaskView>(),
            #[cfg(feature = "wallboard")]
            Tasks::Selected(predicate) => graph.query::<TaskView>(predicate.as_str()),
        },
    )
    .map_err(refused("tasks"))?;
    let relations = records(&graph, graph.query_all::<RelationView>()).map_err(refused("relations"))?;
    let relations_elsewhere = graph
        .relations_made_elsewhere_live(&BTreeSet::new())
        .map(Following::new)
        .map_err(refused("relations made elsewhere"))?;
    let projects = graph.query_projects().map_err(refused("projects"))?;
    let scopes = named(Collection::Scopes)
        .then(|| records(&graph, graph.query_all::<ScopeView>()).map_err(refused("scopes")))
        .transpose()?;
    let principals = named(Collection::Principals)
        .then(|| records(&graph, graph.query_all::<PrincipalView>()).map_err(refused("principals")))
        .transpose()?;
    let messages = named(Collection::Messages)
        .then(|| records(&graph, graph.query_all::<MessageView>()).map_err(refused("messages")))
        .transpose()?;
    let anchors = named(Collection::Anchors)
        .then(|| records(&graph, graph.query_all::<CodeAnchorView>()).map_err(refused("anchors")))
        .transpose()?;
    let decisions = named(Collection::Decisions)
        .then(|| records(&graph, graph.query_all::<DecisionView>()).map_err(refused("decisions")))
        .transpose()?;
    let former_ids = named(Collection::FormerIds)
        .then(|| graph.query_all::<FormerIdView>().map_err(refused("former ids")))
        .transpose()?;
    // Watched from before any of them answers, so that every arrival is heard.
    let (arrivals, ends) = (Arrivals::default(), Ends::default());
    let mut watching = vec![
        ends.watch(&relations.listed, RelationRead::Project),
        arrivals.watch(relations_elsewhere.live().live()),
        ends.watch(relations_elsewhere.live().live(), RelationRead::MadeElsewhere),
        arrivals.watch(&projects),
    ];
    for records in [
        Some(arrivals.watch_records(&tasks)),
        Some(arrivals.watch_records(&relations)),
        scopes.as_ref().map(|r| arrivals.watch_records(r)),
        principals.as_ref().map(|r| arrivals.watch_records(r)),
        messages.as_ref().map(|r| arrivals.watch_records(r)),
        anchors.as_ref().map(|r| arrivals.watch_records(r)),
        decisions.as_ref().map(|r| arrivals.watch_records(r)),
    ]
    .into_iter()
    .flatten()
    {
        watching.extend(records);
    }
    let store = Store {
        here: graph.record(),
        tasks,
        relations,
        relations_elsewhere,
        projects,
        scopes,
        principals,
        messages,
        anchors,
        decisions,
        former_ids,
        answered: ArcRwSignal::new(false),
        arrivals,
        ends,
        _watching: watching,
        opened_with: asked,
    };
    STORE.set(SendWrapper::new(store)).ok();
    spawn_local(record_the_daemons_answer());
    Ok(())
}

fn store() -> &'static Store {
    STORE.get().expect("store opened by App")
}

/// The standing query of `collection`, which the page must have named when it opened the store:
/// reading one it did not name is the page's mistake, and stops it rather than show nothing.
fn opened<T>(query: &Option<T>, collection: Collection) -> &T {
    query.as_ref().unwrap_or_else(|| panic!("the page reads {collection:?}, which it did not open the store with"))
}

// Each kind's records, by the store they are read from: what a lookup that names an id of that
// kind reads, and what gives the id back once its reader has gone.
fn task_records(store: &Store) -> &Records<TaskView> {
    &store.tasks
}
fn relation_records(store: &Store) -> &Records<RelationView> {
    &store.relations
}
fn scope_records(store: &Store) -> &Records<ScopeView> {
    opened(&store.scopes, Collection::Scopes)
}
fn principal_records(store: &Store) -> &Records<PrincipalView> {
    opened(&store.principals, Collection::Principals)
}
fn message_records(store: &Store) -> &Records<MessageView> {
    opened(&store.messages, Collection::Messages)
}
fn anchor_records(store: &Store) -> &Records<CodeAnchorView> {
    opened(&store.anchors, Collection::Anchors)
}
fn decision_records(store: &Store) -> &Records<DecisionView> {
    opened(&store.decisions, Collection::Decisions)
}

pub fn tasks() -> Vec<TaskView> {
    store().tasks.listed.get()
}
pub fn relations() -> Vec<RelationView> {
    store().relations.listed.get()
}
pub fn scopes() -> Vec<ScopeView> {
    scope_records(store()).listed.get()
}
pub fn principals() -> Vec<PrincipalView> {
    principal_records(store()).listed.get()
}
pub fn messages() -> Vec<MessageView> {
    message_records(store()).listed.get()
}
/// Every decision, by number (then by creation, should two share a number).
pub fn decisions() -> Vec<DecisionView> {
    let mut found = decision_records(store()).listed.get();
    found.sort_by_key(|d| (d.number().unwrap_or(0), d.created().unwrap_or(0)));
    found
}

/// The project's former ids, each with the record it names now, copied whole: read inside a
/// closure, which then runs again when one arrives.
pub fn former_ids() -> FormerIds {
    FormerIds::from_views(&opened(&store().former_ids, Collection::FormerIds).get())
}

/// The record `id` names now: the one it was renumbered to when it is a former id, and itself
/// otherwise.
pub fn current(id: EntityId) -> EntityId {
    former_ids().named(&id.to_base64()).unwrap_or(id)
}

/// Whether the daemon has answered every standing query, so that an empty result means none
/// rather than not yet, and a choice made once from them, such as the card the Inbox or the
/// Questions page selects first or the scope the Graph and Decisions pages open on, is made from
/// everything there is. Each query first answers from this load's own browser storage, which
/// starts empty on every load, and then receives the daemon's answer in small batches (some 70 ms
/// after a fresh load, longer over a LAN): a page that took the first answer for the daemon's said
/// it had nothing while what it shows was on its way (the Inbox's "Nothing waiting.": task
/// tcUrcN1T), and a first card chosen then was whichever arrived first (the oldest, in the check
/// that found it: task PVZ3Ovza). Every page waits on this one signal, so no caller has to
/// remember that the first answer is not the daemon's; what a page says while it waits is
/// `empty.rs`'s. Asked inside a closure, it makes the closure a dependent of the answer, so the
/// closure asks again once the daemon has answered.
pub fn standing_loaded() -> bool {
    store().answered.get()
}

/// Records that the daemon has answered every standing query the page opened, or that one failed:
/// a failure is logged and counts as an answer, so nothing waits for it forever. The daemon answers
/// them side by side, so waiting for each in turn takes as long as the slowest.
async fn record_the_daemons_answer() {
    let store = store();
    let mut answers = vec![
        ("tasks", store.tasks.listed.wait_durable_answered().await),
        ("relations", store.relations.listed.wait_durable_answered().await),
        ("projects", store.projects.wait_durable_answered().await),
    ];
    if let Some(scopes) = &store.scopes {
        answers.push(("scopes", scopes.listed.wait_durable_answered().await));
    }
    if let Some(principals) = &store.principals {
        answers.push(("principals", principals.listed.wait_durable_answered().await));
    }
    if let Some(messages) = &store.messages {
        answers.push(("messages", messages.listed.wait_durable_answered().await));
    }
    if let Some(anchors) = &store.anchors {
        answers.push(("anchors", anchors.listed.wait_durable_answered().await));
    }
    if let Some(decisions) = &store.decisions {
        answers.push(("decisions", decisions.listed.wait_durable_answered().await));
    }
    if let Some(former_ids) = &store.former_ids {
        answers.push(("former ids", former_ids.wait_durable_answered().await));
    }
    for (what, answer) in answers {
        if let Err(e) = answer {
            tracing::warn!("the {what} query did not load: {e}");
        }
    }
    store.answered.set(true);
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// What one change in a query's change set says of the entity it names: that it came into the
/// query's results, or went out of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Moved {
    Arrived,
    Left,
}

/// What `change` says: an `Initial` (the first answer) or an `Add` that its entity arrived, a
/// `Remove` that it left, and an `Update`, a change to an entity that stays, neither.
fn moved<I>(change: &ItemChange<I>) -> Option<Moved> {
    match change {
        ItemChange::Initial { .. } | ItemChange::Add { .. } => Some(Moved::Arrived),
        ItemChange::Remove { .. } => Some(Moved::Left),
        ItemChange::Update { .. } => None,
    }
}

/// The readers of entities looked up by their ids, each run again when the entity it looked up
/// arrives in or leaves a query this watches, as the query's change sets name it. The result set's
/// own lookup (`by_id`) runs its reader again whenever any member arrives or leaves, so every row
/// that looked up a message ran again as each message of a load arrived (task unFIWd5c). An id
/// names one entity whatever its collection, so one `Arrivals` serves several queries.
///
/// It holds a trigger for each id looked up, which goes when its entity leaves (the readers run
/// again, and one that reads the id again makes a new trigger). reactive_graph does not say
/// whether a trigger has a reader left, so a trigger whose readers are all gone stays until then:
/// the map holds at most one small entry for each id the page has looked up, and the page looks up
/// the entities its standing queries hold in full, so the map grows as they do, by far less.
#[derive(Clone, Default)]
pub struct Arrivals(Arc<Mutex<HashMap<EntityId, ArcTrigger>>>);

impl Arrivals {
    /// Hears what `query` gains and loses, for as long as the guard is kept.
    pub fn watch<V: View + Clone + Send + Sync + 'static>(&self, query: &LiveQuery<V>) -> SubscriptionGuard {
        let arrivals = self.clone();
        query.subscribe(move |changes: ChangeSet<V>| {
            for change in &changes.changes {
                match moved(change) {
                    Some(Moved::Arrived) => arrivals.fire(change.entity().id()),
                    Some(Moved::Left) => arrivals.forget(change.entity().id()),
                    None => {}
                }
            }
        })
    }

    /// Hears what both reads of a kind of record gain and lose: the project's, and those the page
    /// names.
    fn watch_records<V: InProjectView + Clone + Send + Sync + 'static>(
        &self,
        records: &Records<V>,
    ) -> [SubscriptionGuard; 2] {
        [self.watch(&records.listed), self.watch(records.named.live().live())]
    }

    /// The member `id` of `query`, a query this watches, found by its id without copying the others:
    /// read inside a closure, which then runs again when it arrives or leaves, and, through the
    /// fields the closure reads, when it changes, but not when another member does.
    pub fn member<V: View + Clone + 'static>(&self, query: &LiveQuery<V>, id: EntityId) -> Option<V> {
        self.track(id);
        untrack(|| query.resultset().by_id(&id))
    }

    /// Makes the closure reading `id` run again when the entity arrives or leaves.
    fn track(&self, id: EntityId) {
        let trigger = lock(&self.0).entry(id).or_default().clone();
        trigger.track();
    }

    /// Runs again every closure that read `id`.
    fn fire(&self, id: EntityId) {
        let trigger = lock(&self.0).get(&id).cloned();
        if let Some(trigger) = trigger {
            trigger.notify();
        }
    }

    /// Runs again every closure that read `id`, whose entity left, and drops its trigger.
    fn forget(&self, id: EntityId) {
        let trigger = lock(&self.0).remove(&id);
        if let Some(trigger) = trigger {
            trigger.notify();
        }
    }
}

/// One of the two reads of relations [`Ends`] keeps the ends of: the project's own, and the read of
/// those other projects made that touch the records whose relations a reader reads. A relation whose
/// project changes, as one from a task that moves does, leaves one and arrives in the other, and the
/// two say so in either order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum RelationRead {
    Project,
    MadeElsewhere,
}

/// The ids of the relations at each entity, from or to it, in the order the relations queries hold
/// them (by id), kept from the queries' change sets, with the readers of each entity's. Each
/// relation is kept with the reads that hold it, and stays at its ends while either does, so that
/// one read's departure never takes away what the other holds.
#[derive(Clone, Default)]
struct Ends {
    by_entity: Arc<Mutex<HashMap<EntityId, RelationsAt>>>,
    readers: Arrivals,
}

/// The relations at one entity, by id, each with the reads that hold it.
type RelationsAt = BTreeMap<EntityId, BTreeSet<RelationRead>>;

impl Ends {
    /// Keeps the ends of what `relations`, the read `read`, gains and loses, for as long as the
    /// guard is kept. A relation's ends never change once it is written.
    fn watch(&self, relations: &LiveQuery<RelationView>, read: RelationRead) -> SubscriptionGuard {
        let ends = self.clone();
        relations.subscribe(move |changes: ChangeSet<RelationView>| {
            for change in &changes.changes {
                let Some(moved) = moved(change) else { continue };
                let arrived = moved == Moved::Arrived;
                let r = change.entity();
                // Untracked: this runs inside whatever the change interrupted.
                let Some((from, to)) = untrack(|| Some((r.from_id().ok()?, r.to_id().ok()?))) else { continue };
                ends.place(from, r.id(), read, arrived);
                if to != from {
                    ends.place(to, r.id(), read, arrived);
                }
            }
        })
    }

    /// Records that `read` holds the relation at the entity when it `arrived`, else that it no
    /// longer does, dropping a relation no read holds and an entity left with none, and runs the
    /// entity's readers again when the relations at it changed.
    fn place(&self, entity: EntityId, relation: EntityId, read: RelationRead, arrived: bool) {
        let changed = {
            let mut by_entity = lock(&self.by_entity);
            let at = by_entity.entry(entity).or_default();
            let held = at.contains_key(&relation);
            if arrived {
                at.entry(relation).or_default().insert(read);
            } else if let Some(reads) = at.get_mut(&relation) {
                reads.remove(&read);
                if reads.is_empty() {
                    at.remove(&relation);
                }
            }
            let changed = held != at.contains_key(&relation);
            if at.is_empty() {
                by_entity.remove(&entity);
            }
            changed
        };
        if changed {
            self.readers.fire(entity);
        }
    }

    /// The ids of the relations at `id`, read inside a closure, which then runs again when one of
    /// them arrives or leaves.
    fn of(&self, id: EntityId) -> Vec<EntityId> {
        self.readers.track(id);
        lock(&self.by_entity).get(&id).map(|at| at.keys().copied().collect()).unwrap_or_default()
    }
}

/// The member `id` of `query`, one of the standing queries ([`Arrivals::member`]).
fn member<V: View + Clone + 'static>(query: &LiveQuery<V>, id: EntityId) -> Option<V> {
    store().arrivals.member(query, id)
}

/// The record `id` of the kind `records` reads: the project's, else one the page names wherever it
/// is, which the reader asking names until it runs again or leaves the page (`followed.rs`). Before
/// the daemon has answered the standing queries, a record of the project may still be on its way,
/// so nothing is named yet, and the reader runs again once they have answered.
fn look_up<V: InProjectView + Clone + 'static>(records: fn(&Store) -> &Records<V>, id: EntityId) -> Option<V> {
    let store = store();
    if let Some(found) = member(&records(store).listed, id) {
        return Some(found);
    }
    if !standing_loaded() {
        return None;
    }
    name(move |store| &records(store).named, id);
    member(records(store).named.live().live(), id)
}

/// Names `id` in the read `following` gives, for the reader running now, and tells the read when
/// that changes the ids it reads for.
fn name<V, F>(following: F, id: EntityId)
where
    V: InProjectView + Clone + 'static,
    F: Fn(&'static Store) -> &'static Following<V> + Copy + Send + Sync + 'static,
{
    let ask = move || Following::ask(move || following(store()));
    let let_go = move || {
        if following(store()).give_back(id) {
            ask();
        }
    };
    if following(store()).name(id, let_go) {
        ask();
    }
}

/// What the page can say of `id`, a record of the kind `records` reads that a lookup did not find:
/// read inside a closure, which then runs again when the daemon answers.
fn answer_for<V: InProjectView + Clone + 'static>(records: fn(&Store) -> &Records<V>, id: EntityId) -> Answer {
    if !standing_loaded() {
        return Answer::Waiting;
    }
    name(move |store| &records(store).named, id);
    records(store()).named.answer_for(id)
}

// A lookup by id answers for the project's records and for those the page names in other projects;
// [`whose`] tells them apart.
pub fn task(id: EntityId) -> Option<TaskView> {
    look_up(task_records, id)
}
pub fn scope(id: EntityId) -> Option<ScopeView> {
    look_up(scope_records, id)
}
pub fn message(id: EntityId) -> Option<MessageView> {
    look_up(message_records, id)
}
pub fn anchor(id: EntityId) -> Option<CodeAnchorView> {
    look_up(anchor_records, id)
}
pub fn decision(id: EntityId) -> Option<DecisionView> {
    look_up(decision_records, id)
}
pub fn principal(id: EntityId) -> Option<PrincipalView> {
    look_up(principal_records, id)
}

/// The kinds of record the page reads, in the order an id of a kind it does not know is looked up.
fn kinds_read(store: &Store) -> Vec<Model> {
    [
        (Model::Task, true),
        (Model::Decision, store.decisions.is_some()),
        (Model::Message, store.messages.is_some()),
        (Model::Scope, store.scopes.is_some()),
        (Model::Anchor, store.anchors.is_some()),
        (Model::Principal, store.principals.is_some()),
    ]
    .into_iter()
    .filter_map(|(model, read)| read.then_some(model))
    .collect()
}

/// Whether the project's records of the kind `model` hold `id`.
fn listed(store: &Store, model: Model, id: EntityId) -> bool {
    match model {
        Model::Task => member(&task_records(store).listed, id).is_some(),
        Model::Decision => member(&decision_records(store).listed, id).is_some(),
        Model::Message => member(&message_records(store).listed, id).is_some(),
        Model::Scope => member(&scope_records(store).listed, id).is_some(),
        Model::Anchor => member(&anchor_records(store).listed, id).is_some(),
        Model::Principal => member(&principal_records(store).listed, id).is_some(),
    }
}

/// Whether the page holds `id` as a record of the kind `model`, the project's or one it names.
fn held(model: Model, id: EntityId) -> bool {
    match model {
        Model::Task => task(id).is_some(),
        Model::Decision => decision(id).is_some(),
        Model::Message => message(id).is_some(),
        Model::Scope => scope(id).is_some(),
        Model::Anchor => anchor(id).is_some(),
        Model::Principal => principal(id).is_some(),
    }
}

/// The kind of record `id` is, as the page holds it: the project's records of every kind are asked
/// first, so that a record of the project names nothing in the reads of other projects' records;
/// then the reads of each kind in turn, up to the one holding it. None while nothing holds it.
pub fn kind_of(id: EntityId) -> Option<Model> {
    let kinds = kinds_read(store());
    let here = kinds.iter().copied().find(|model| listed(store(), *model, id));
    here.or_else(|| kinds.into_iter().find(|model| held(*model, id)))
}

/// The relation `id`, whichever project made it: the project's, or one the page names wherever it
/// was made, as a notification about a relation of another project names it, which the reader
/// asking names as a lookup of a record of any kind does ([`look_up`]); meanwhile, one another
/// project made that touches a record whose relations a reader reads ([`relations_of`]).
pub fn relation(id: EntityId) -> Option<RelationView> {
    look_up(relation_records, id).or_else(|| member(store().relations_elsewhere.live().live(), id))
}

/// The task `id` as the page follows it, as the command line follows a dependency
/// (`fwd_core::project_graph::ProjectGraph::follow`): held, with its project, or not held by the
/// node; none while the page cannot tell yet.
pub fn followed_task(id: EntityId) -> Option<Followed<TaskView>> {
    match task(id) {
        Some(found) => Some(Followed::Held { record: found.id(), project: whose(&found)?, found }),
        None => (answer_for(task_records, id) == Answer::NotHeld).then_some(Followed::NotHeld),
    }
}

/// What the page can say of `id`, which it looked up as a record of the kind `model` names and did
/// not find, or of every kind for none: loading, until the daemon has answered for it, then that
/// the node holds no such record. Read inside a closure, which then runs again when that changes.
pub fn answer(id: EntityId, model: Option<Model>) -> Answer {
    let of = |model| match model {
        Model::Task => answer_for(task_records, id),
        Model::Decision => answer_for(decision_records, id),
        Model::Message => answer_for(message_records, id),
        Model::Scope => answer_for(scope_records, id),
        Model::Anchor => answer_for(anchor_records, id),
        Model::Principal => answer_for(principal_records, id),
    };
    match model {
        Some(model) => of(model),
        // Not held only when no read of any kind holds it; every kind is asked at once.
        None => {
            let answers: Vec<Answer> = kinds_read(store()).into_iter().map(of).collect();
            answers.into_iter().find(|a| *a != Answer::NotHeld).unwrap_or(Answer::NotHeld)
        }
    }
}

/// Whose record `view` is: the project's, or another project's, which the page names beside it;
/// none while the page has not read that project yet. Read inside a closure, which then runs again
/// when it has.
pub fn whose<V: InProjectView>(view: &V) -> Option<Whose> {
    let store = store();
    let named = |project| member(&store.projects, project).and_then(|p| ProjectRecord::of(&p).ok());
    project_whose(view.project_id().ok()?, store.here, named)
}

/// Whose record one naming `project` is, on a page showing the project whose record is `here`: the
/// page's, or the project `named` gives, none while it gives none.
fn project_whose(
    project: EntityId,
    here: EntityId,
    named: impl FnOnce(EntityId) -> Option<ProjectRecord>,
) -> Option<Whose> {
    if project == here {
        Some(Whose::This)
    } else {
        named(project).map(Whose::Other)
    }
}

/// The project `view`'s record is in, when that is another than the page's: where a link to it
/// leads.
pub fn elsewhere<V: InProjectView>(view: &V) -> Option<ProjectRecord> {
    match whose(view)? {
        Whose::This => None,
        Whose::Other(project) => Some(project),
    }
}

/// `name`, which names `view`'s record, as the page shows it: with its project's name after it when
/// it is another project's (`fwd_core::ops::in_project`: "Ship the connector in beta"), since a
/// title, a handle or a number names something else there.
pub fn named<V: InProjectView>(name: String, view: &V) -> String {
    let here = view.project_id().is_ok_and(|project| project == store().here);
    in_project(name, here, whose(view))
}

/// `name` as [`named`] shows it: the project's name after it for a record of another project, and
/// while that project has not been read, that it is another's.
fn in_project(name: String, here: bool, whose: Option<Whose>) -> String {
    match whose {
        Some(whose) => ops::in_project(name, &whose),
        None if here => name,
        None => format!("{name} in another project"),
    }
}

/// How the page names `id`, a record of the kind `model` names (of any kind for none) that it does
/// not hold: the start of its id while the daemon has not answered for it, then the start of its id
/// and that it cannot be followed here (`fwd_core::ops::not_followed`), as the command line names it.
pub fn unheld(id: EntityId, model: Option<Model>) -> String {
    match answer(id, model) {
        Answer::NotHeld => ops::not_followed(&id.to_base64()),
        Answer::Waiting | Answer::Failed(_) => crate::fmt::short(id),
    }
}

/// The relations from or to `id`, those its project made and those another project made, in the
/// order each query holds them: read inside a closure, which then runs again when one of them
/// arrives or leaves, and, through the fields the closure reads, when one of them changes. The
/// relations made elsewhere are read for as long as the reader reads them, once the daemon has
/// answered the standing queries.
pub fn relations_of(id: EntityId) -> Vec<RelationView> {
    if standing_loaded() {
        name(|store| &store.relations_elsewhere, id);
    }
    held_relations(id)
}

/// The relations from the task `id`: those its project made. The standing query holds them for a
/// task of the project, whose relations no other project makes (a command on a record runs in the
/// record's project); for a task of another project they are read as [`relations_of`] reads them.
pub fn relations_from(task: EntityId) -> Vec<RelationView> {
    let ours = member(&store().tasks.listed, task).is_some();
    let at = if ours { held_relations(task) } else { relations_of(task) };
    at.into_iter().filter(|r| r.from_id().ok() == Some(task)).collect()
}

/// The relations at `id` the store holds now, from either query.
fn held_relations(id: EntityId) -> Vec<RelationView> {
    let store = store();
    let ids = store.ends.of(id);
    let elsewhere = store.relations_elsewhere.live().live();
    untrack(|| {
        ids.into_iter()
            .filter_map(|r| store.relations.listed.resultset().by_id(&r).or_else(|| elsewhere.resultset().by_id(&r)))
            .collect()
    })
}
/// The principal `name` names among the page's principals, active or not, read as every typed name
/// is (`fwd_core::naming::resolve`: its name, or its id): none while no principal has it, and a
/// sentence listing them when two or more share it, each with the scopes its work is in
/// (docs/design/agents-and-inbox.md, "A principal's name").
pub fn principal_named(name: &str) -> Result<Option<PrincipalView>, String> {
    let principals = principals();
    match naming::resolve(&principals, name, None, &former_ids()) {
        Named::One(p) => Ok(Some(p.clone())),
        Named::NoOne => Ok(None),
        Named::Shared(several) => {
            let held = Held::now();
            Err(naming::refusal(name, &several, false, |id| held.scope_titles_of(id)))
        }
        Named::Started(several) => {
            let held = Held::now();
            Err(naming::refusal(name, &several, true, |id| held.scope_titles_of(id)))
        }
    }
}

/// The handle of principal `id` while no other principal of the project has it, which is when the
/// handle, typed, names that principal; none for a handle two or more share, or a principal the
/// page does not hold.
pub fn unique_handle(id: EntityId) -> Option<String> {
    let principals = principals();
    let handle = principals.iter().find(|p| p.id() == id)?.handle().ok()?;
    (naming::by_name(&principals, &handle, None).len() == 1).then_some(handle)
}

/// The titles of the scopes principal `id`'s work is in, the most recent first, from the page's
/// standing queries (`fwd_core::naming::scopes_of`): what tells apart principals sharing a name.
pub fn scope_titles_of(id: EntityId) -> Vec<String> {
    Held::now().scope_titles_of(id)
}

/// The standing queries a principal's scopes are read from, taken once for as many principals as
/// a list asks about.
struct Held {
    tasks: Vec<TaskView>,
    relations: Vec<RelationView>,
    scopes: Vec<ScopeView>,
    messages: Vec<MessageView>,
}

impl Held {
    fn now() -> Held {
        Held { tasks: tasks(), relations: relations(), scopes: scopes(), messages: messages() }
    }

    fn scope_titles_of(&self, id: EntityId) -> Vec<String> {
        let work =
            Work { tasks: &self.tasks, relations: &self.relations, scopes: &self.scopes, messages: &self.messages };
        naming::scope_titles(&naming::scopes_of(id, &work), &self.scopes)
    }
}

/// One heading of a principal picker (the header's `as`, the Ask box's recipient, a task's
/// assignee, the Inbox's and the Questions page's selector of whose they show).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub label: &'static str,
    /// Each member's id and what its option says (its handle, with the scopes its work is in beside
    /// a handle two or more principals share: `fwd_core::naming::option_texts`), by handle.
    pub members: Vec<(EntityId, String)>,
}

/// The headings, in the order the pickers list them: people, then agents by the role each selected
/// (the leads, one per session that coordinates the project's work; the triage agents, one per
/// scope and one that serves a small project whole; the implementers; the reviewers), then agents
/// with no role, which a graph from before the fixed roles can hold, and last the dormant agents,
/// whatever their role (docs/design/web-ui.md, "Who acts").
const GROUPS: [&str; 7] = ["people", "leads", "triage", "implementers", "reviewers", "agents", DORMANT];

/// The heading of the agents that have written nothing for twelve hours, below every other, so
/// that the users left behind by finished agents do not crowd out the agents at work, while each
/// stays choosable (docs/DECISIONS.md, D123; `fwd_core::dormancy`).
const DORMANT: &str = "dormant";

/// The heading of a principal's role. An agent's role is a default, not a rule
/// (docs/design/data-model.md, "Rights"), so this sorts the pickers and decides nothing else.
fn role_heading(p: &PrincipalView) -> &'static str {
    match (p.kind().ok(), fwd_core::ops::role_of(p)) {
        (Some(PrincipalKind::Human), _) => "people",
        (_, Some(AgentRole::Lead)) => "leads",
        (_, Some(AgentRole::Triage)) => "triage",
        (_, Some(AgentRole::Implementer)) => "implementers",
        (_, Some(AgentRole::Reviewer)) => "reviewers",
        (_, None) => "agents",
    }
}

/// Where a heading comes among [`GROUPS`], which orders the dormant agents by their roles as the
/// headings order the others.
fn rank(heading: &str) -> usize {
    GROUPS.iter().position(|g| *g == heading).unwrap_or(GROUPS.len())
}

/// One principal as a picker lists it: its id, the heading it goes under, the place of its role's
/// heading ([`rank`]), its handle and what its option says.
struct Listed {
    id: EntityId,
    heading: &'static str,
    role: usize,
    handle: String,
    text: String,
}

/// `listed` under their headings, in the order of [`GROUPS`], with the empty headings left out;
/// under each, by role, which orders the dormant agents as the headings above order the others,
/// then by handle, then by what the options say.
fn grouped(mut listed: Vec<Listed>) -> Vec<Group> {
    listed.sort_by(|a, b| (a.role, &a.handle, &a.text).cmp(&(b.role, &b.handle, &b.text)));
    let mut groups: Vec<Group> = GROUPS.iter().map(|label| Group { label, members: Vec::new() }).collect();
    for one in listed {
        if let Some(group) = groups.iter_mut().find(|g| g.label == one.heading) {
            group.members.push((one.id, one.text));
        }
    }
    groups.retain(|g| !g.members.is_empty());
    groups
}

/// An option's `text` with its words in brackets after it: `role` for a dormant agent, which is
/// listed under no heading of its role, then `serving` for a triage agent, what it serves
/// ("impl-alice (implementer)", "triage-repl (triage, serving Replication)", "triage-repl (serving
/// Replication)").
fn with_words(text: String, role: Option<String>, serving: Option<String>) -> String {
    let words: Vec<String> = role.into_iter().chain(serving).collect();
    if words.is_empty() {
        text
    } else {
        format!("{text} ({})", words.join(", "))
    }
}

/// The principals that belong to the project the page shows, grouped for a picker; empty headings
/// are left out. A graph holds every principal that ever ran a command in the project, a session
/// that only read it included, and a picker offers only those that belong (docs/DECISIONS.md,
/// D106; `fwd_core::ops::belongs`). `keep` is listed whether or not it belongs, so a picker never
/// shows another principal than the one it holds: the one the page acts as, and the one whose
/// questions the page shows. A handle two or more principals of the project share, whether or not
/// the others are offered, carries the scopes each one's work is in, so that the options can be
/// told apart (docs/DECISIONS.md, D107; `fwd_core::naming::shows_scopes`). An agent dormant
/// by the page's clock as it draws the picker goes under the last heading, "dormant", with its role
/// beside its name (docs/DECISIONS.md, D123).
pub fn principal_groups(keep: Option<EntityId>) -> Vec<Group> {
    let all = principals();
    let listed: Vec<&PrincipalView> =
        all.iter().filter(|p| fwd_core::ops::belongs(p) || Some(p.id()) == keep).collect();
    let shown: Vec<(&PrincipalView, String, bool)> = listed
        .iter()
        .map(|p| {
            let handle = p.handle().unwrap_or_default();
            let shows = naming::shows_scopes(&all, &handle);
            (*p, handle, shows)
        })
        .collect();
    // The work behind the scopes, read once, and only when some name is shared.
    let held = shown.iter().any(|(_, _, shows)| *shows).then(Held::now);
    let offered: Vec<naming::Offered> = shown
        .into_iter()
        .map(|(p, handle, shows)| {
            let scopes = held.as_ref().filter(|_| shows).map(|held| held.scope_titles_of(p.id()));
            (p.id(), handle, scopes)
        })
        .collect();
    let texts = naming::option_texts(&offered);
    let now = fwd_core::now_ms();
    let entries: Vec<Listed> = texts
        .into_iter()
        .zip(&listed)
        .zip(offered)
        .map(|((text, p), (id, handle, _))| {
            let role = role_heading(p);
            let dormant = dormancy::is_dormant(p, now);
            // A dormant agent's option names its role, which no heading says, and a triage agent's
            // says what it serves, so that whoever asks one by name, or reads its questions, sees
            // which scopes it answers first for (docs/DECISIONS.md, D122).
            let text = with_words(text, dormant.then(|| naming::what(p)), serving_words(p));
            Listed { id, heading: if dormant { DORMANT } else { role }, role: rank(role), handle, text }
        })
        .collect();
    grouped(entries)
}

/// What a triage agent's option in a picker says it serves, as `fwd principal list` says it:
/// "serving Replication", "serving Replication and the whole project", "serving nothing"; none for
/// any other principal.
fn serving_words(p: &PrincipalView) -> Option<String> {
    if fwd_core::ops::role_of(p) != Some(AgentRole::Triage) {
        return None;
    }
    let serving = fwd_core::ops::Serving::of(p);
    let mut served: Vec<String> = serving.scopes.iter().map(|s| scope_label(*s)).collect();
    if serving.project_wide {
        served.push("the whole project".to_string());
    }
    Some(match served.split_last() {
        None => "serving nothing".to_string(),
        Some((last, [])) => format!("serving {last}"),
        Some((last, rest)) => format!("serving {} and {last}", rest.join(", ")),
    })
}

/// The triage agent that answers first for the questions asked of `scope`, or of no scope when it
/// is none, chosen as the routing service chooses it (`fwd_core::ops::front_line`): the scope's own,
/// else the one that serves the whole project; none while neither exists, when such a question
/// waits on every Questions page (docs/DECISIONS.md, D122).
pub fn front_line(scope: Option<EntityId>) -> Option<EntityId> {
    fwd_core::ops::front_line(&principals(), scope).map(|f| f.agent)
}

/// A principal's handle, with its project's name after it when the principal is another project's
/// ("alice in beta"); a short id while the principal has not arrived, and that it cannot be
/// followed here once the daemon has answered that the node holds no such principal.
pub fn handle_of(id: EntityId) -> String {
    match principal(id) {
        Some(p) => named(p.handle().unwrap_or_default(), &p),
        None => unheld(id, Some(Model::Principal)),
    }
}

/// The scope a task is worked in, named for a list: its name, with its project's name after it when
/// it is another project's, or the start of its id while the page does not hold it. Until 2026-09-29
/// its owner's handle followed the name; a scope has no owner now (docs/DECISIONS.md, D108, "A scope
/// has no owner: scopes are not assigned to principals").
pub fn scope_label(scope: EntityId) -> String {
    match self::scope(scope) {
        Some(s) => named(s.title().unwrap_or_default(), &s),
        None => unheld(scope, Some(Model::Scope)),
    }
}

/// A task's title as the page names it where it shows again: with its project's name after it
/// when it is another project's; the start of its id while the page does not hold it, or with
/// the words that it cannot be followed here.
pub fn task_title(id: EntityId) -> String {
    match task(id) {
        Some(t) => named(t.title().unwrap_or_default(), &t),
        None => unheld(id, Some(Model::Task)),
    }
}

/// The scopes a task is worked in, any number of them (docs/DECISIONS.md, D136, "A task may be
/// worked in several scopes", 2026-10-03), read from `relations`, the page's or the task's own, in
/// the order the command line lists them (`fwd_core::ops::scopes_in`): the targets of its accepted
/// `worked_in` relations, or of an `assigned_to`, the name that relation had until 2026-09-28,
/// which `fwd admin migrate` has not renamed yet. Until 2026-10-03 the page read the first of them
/// alone, a task having one scope.
pub fn task_scopes(task: EntityId, relations: &[RelationView]) -> Vec<EntityId> {
    fwd_core::ops::scopes_in(task, relations)
}

/// The scopes one task is worked in ([`task_scopes`]), its relations looked up by its id rather than
/// among a copy of every relation: read inside a closure, which runs again when one arrives, leaves
/// or changes.
pub fn scopes_of_task(task: EntityId) -> Vec<EntityId> {
    task_scopes(task, &relations_from(task))
}

/// The same three-valued answer the CLI gives (`fwd_core::ops::readiness`), each dependency
/// followed wherever it leads and looked up by id: a finished task of another project counts as
/// finished, an unfinished one blocks and is named with its project, and one the node does not hold
/// blocks as one that cannot be followed here. Read inside a closure, which runs again when a
/// dependency or its status changes.
pub fn readiness(task: &TaskView) -> Readiness {
    let snapshot = TaskSnapshot {
        id: task.id(),
        status: task.status().unwrap_or(TaskStatus::Proposed),
        hold: task.hold().unwrap_or(None),
        not_before: task.not_before().unwrap_or(None),
    };
    rules::readiness(&snapshot, &dependencies(task.id()), clock::present())
}

/// The accepted `depends_on` targets of the task `id`, each as readiness weighs it.
pub fn dependencies(id: EntityId) -> Vec<rules::Dependency> {
    relations_from(id)
        .iter()
        .filter(|r| r.kind().ok() == Some(RelationKind::DependsOn) && r.status().ok() == Some(RelationStatus::Accepted))
        .filter_map(|r| r.to_id().ok())
        .map(|to| dependency(to, followed_task(to).map(status_of)))
        .collect()
}

/// What readiness weighs of a followed task: its status, which a fault can leave unread.
fn status_of(followed: Followed<TaskView>) -> Followed<Option<TaskStatus>> {
    match followed {
        Followed::Held { record, project, found } => Followed::Held { record, project, found: found.status().ok() },
        Followed::NotHeld => Followed::NotHeld,
    }
}

/// The dependency on `to` as readiness weighs it, from what the page holds of it (`found`, none
/// while it cannot tell yet): a task of the project with its status, a task of another project with
/// that project's name and its status, and one the node does not hold, which cannot be followed
/// here. Until the page can tell, or while a status cannot be read, it is unknown, which a known
/// blocker outweighs (`fwd_core::rules::readiness`).
fn dependency(to: EntityId, found: Option<Followed<Option<TaskStatus>>>) -> rules::Dependency {
    match found {
        None => rules::Dependency::Here(to, None),
        Some(Followed::NotHeld) => rules::Dependency::NotHeld(to),
        Some(Followed::Held { record, project: Whose::This, found }) => rules::Dependency::Here(record, found),
        Some(Followed::Held { record, project: Whose::Other(p), found: Some(status) }) => {
            rules::Dependency::Elsewhere { task: record, project: p.name, status }
        }
        Some(Followed::Held { record, found: None, .. }) => rules::Dependency::Here(record, None),
    }
}

pub fn message_count(task: EntityId, messages: &[MessageView]) -> usize {
    messages.iter().filter(|m| m.task().ok().flatten().map(EntityId::from) == Some(task)).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use leptos::prelude::ArcMemo;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn id(n: u8) -> EntityId {
        EntityId::from_bytes([n; 32])
    }

    /// A reader of what `read` returns, counting how many times it has run.
    fn reader<T: PartialEq + Send + Sync + 'static>(
        read: impl Fn() -> T + Send + Sync + 'static,
    ) -> (ArcMemo<T>, Arc<AtomicUsize>) {
        let runs = Arc::new(AtomicUsize::new(0));
        let counted = runs.clone();
        let memo = ArcMemo::new(move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
            read()
        });
        (memo, runs)
    }

    fn beta() -> ProjectRecord {
        ProjectRecord { record: id(20), uuid: uuid::Uuid::nil(), name: "beta".into() }
    }

    #[test]
    fn a_record_is_the_pages_own_or_another_projects_by_the_project_it_names() {
        let (here, there) = (id(10), id(20));
        assert_eq!(project_whose(here, here, |_| panic!("the page's own project is not looked up")), Some(Whose::This));
        assert_eq!(project_whose(there, here, |p| (p == there).then(beta)), Some(Whose::Other(beta())));
        assert_eq!(project_whose(there, here, |_| None), None, "not yet: the project has not been read");
    }

    #[test]
    fn a_record_of_another_project_is_named_with_that_projects_name() {
        let title = || "Ship the connector".to_string();
        assert_eq!(in_project(title(), true, Some(Whose::This)), "Ship the connector");
        assert_eq!(in_project(title(), false, Some(Whose::Other(beta()))), "Ship the connector in beta");
        assert_eq!(in_project(title(), false, None), "Ship the connector in another project", "never as its own");
        assert_eq!(in_project(title(), true, None), "Ship the connector");
    }

    /// The readiness of an open task with these dependencies, as the page weighs each one it found.
    fn ready_with(found: Vec<(EntityId, Option<Followed<Option<TaskStatus>>>)>) -> Readiness {
        let task = TaskSnapshot { id: id(1), status: TaskStatus::Open, hold: None, not_before: None };
        let deps: Vec<rules::Dependency> = found.into_iter().map(|(to, found)| dependency(to, found)).collect();
        rules::readiness(&task, &deps, 0)
    }

    #[test]
    fn a_dependency_of_another_project_gates_readiness_as_the_command_line_weighs_it() {
        use rules::{NotReadyReason, UnknownReason};
        let far = id(2);
        let there = |status| Some(Followed::Held { record: far, project: Whose::Other(beta()), found: Some(status) });
        assert_eq!(ready_with(vec![(far, there(TaskStatus::Done))]), Readiness::Ready, "done there counts as done");
        assert_eq!(
            ready_with(vec![(far, there(TaskStatus::InProgress))]),
            Readiness::NotReady(vec![NotReadyReason::BlockedByTaskIn { task: far, project: "beta".into() }]),
            "unfinished there blocks, named with its project"
        );
        assert_eq!(
            ready_with(vec![(far, Some(Followed::NotHeld))]),
            Readiness::NotReady(vec![NotReadyReason::DependencyNotHeld(far)]),
            "one the node does not hold blocks, as one that cannot be followed here"
        );
        let here = |status| Some(Followed::Held { record: id(3), project: Whose::This, found: Some(status) });
        assert_eq!(ready_with(vec![(id(3), here(TaskStatus::Done)), (far, there(TaskStatus::Done))]), Readiness::Ready);
        assert_eq!(
            ready_with(vec![(far, None)]),
            Readiness::Unknown(vec![UnknownReason::DependencyUnavailable(far)]),
            "until the daemon has answered for it, the page cannot tell"
        );
        assert_eq!(
            ready_with(vec![(far, None), (id(3), here(TaskStatus::Open))]),
            Readiness::NotReady(vec![NotReadyReason::BlockedBy(id(3))]),
            "a known blocker outweighs one the page cannot tell yet"
        );
    }

    #[test]
    fn a_later_open_naming_the_same_collections_in_another_order_is_admitted() {
        use Collection::{Messages, Principals, Scopes};
        let opened = OpenedWith::new(Tasks::Every, &[Scopes, Principals, Messages]);
        assert_eq!(opened.admit(&OpenedWith::new(Tasks::Every, &[Messages, Scopes, Principals])), Ok(()));
        assert_eq!(opened.admit(&OpenedWith::new(Tasks::Every, &[Scopes, Principals, Messages])), Ok(()));
    }

    #[test]
    fn a_later_open_naming_other_collections_is_refused() {
        use Collection::{Anchors, Messages, Principals, Scopes};
        let opened = OpenedWith::new(Tasks::Every, &[Scopes, Principals, Messages]);
        assert_eq!(
            opened.admit(&OpenedWith::new(Tasks::Every, &[Principals, Scopes])),
            Err("the store already reads {Scopes, Principals, Messages}, not {Scopes, Principals}".to_owned()),
            "one fewer"
        );
        assert!(
            opened.admit(&OpenedWith::new(Tasks::Every, &[Messages, Anchors, Scopes, Principals])).is_err(),
            "one more"
        );
    }

    #[cfg(feature = "wallboard")]
    #[test]
    fn a_later_open_selecting_other_tasks_is_refused() {
        use Collection::{Messages, Principals, Scopes};
        let opened = OpenedWith::new(Tasks::Every, &[Scopes, Principals, Messages]);
        assert_eq!(
            opened.admit(&OpenedWith::new(Tasks::Selected("status = 'open'".into()), &[Scopes, Principals, Messages])),
            Err("the task query already selects every task, not `status = 'open'`".to_owned()),
        );
    }

    #[test]
    fn a_change_set_says_which_entities_arrived_and_which_left() {
        assert_eq!(moved(&ItemChange::Initial { item: 1u8 }), Some(Moved::Arrived), "in the first answer");
        assert_eq!(moved(&ItemChange::Add { item: 2u8, events: vec![] }), Some(Moved::Arrived));
        assert_eq!(moved(&ItemChange::Remove { item: 3u8, events: vec![] }), Some(Moved::Left));
        assert_eq!(moved(&ItemChange::Update { item: 4u8, events: vec![] }), None, "a change to one that stays");
    }

    #[test]
    fn an_arrival_runs_again_only_the_readers_of_what_arrived() {
        let arrivals = Arrivals::default();
        let (one, runs) = reader({
            let arrivals = arrivals.clone();
            move || arrivals.track(id(1))
        });
        one.get();
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        arrivals.fire(id(2));
        one.get();
        assert_eq!(runs.load(Ordering::SeqCst), 1, "another entity arrived: the reader of 1 does not run again");
        arrivals.fire(id(1));
        one.get();
        assert_eq!(runs.load(Ordering::SeqCst), 2, "1 arrived or left: its reader runs again");
    }

    #[test]
    fn the_relations_at_an_entity_follow_their_arrivals_in_the_querys_order() {
        use RelationRead::Project;
        let ends = Ends::default();
        let (task, other) = (id(1), id(9));
        let (at_task, runs) = reader({
            let ends = ends.clone();
            move || ends.of(task)
        });
        assert_eq!(at_task.get(), Vec::<EntityId>::new());
        ends.place(task, id(4), Project, true);
        ends.place(task, id(2), Project, true);
        ends.place(task, id(2), Project, true);
        assert_eq!(at_task.get(), vec![id(2), id(4)], "by id, as the query holds them, each once");
        let before = runs.load(Ordering::SeqCst);
        ends.place(other, id(3), Project, true);
        ends.place(task, id(5), Project, false);
        at_task.get();
        assert_eq!(runs.load(Ordering::SeqCst), before, "another entity's relation, or one it never had, runs nothing");
        ends.place(task, id(4), Project, false);
        assert_eq!(at_task.get(), vec![id(2)]);
        assert_eq!(ends.of(other), vec![id(3)]);
        // An entity left with no relation leaves no entry, and taking away one it never had makes
        // none.
        ends.place(task, id(2), Project, false);
        assert_eq!(at_task.get(), Vec::<EntityId>::new());
        ends.place(id(7), id(8), Project, false);
        let held: Vec<EntityId> = lock(&ends.by_entity).keys().copied().collect();
        assert_eq!(held, vec![other]);
    }

    /// A relation whose project changes leaves the project's read and arrives in the read of those
    /// made elsewhere, or the other way round, and the two say so in either order. When the read
    /// it arrives in says so first, it stays at its ends throughout and their readers do not run
    /// again; when the read it leaves says so first, no read holds it until the other says it
    /// arrived, and it is away from its ends meanwhile. One that both reads hold stays while either
    /// does, and goes with the last.
    #[test]
    fn a_relation_stays_at_its_ends_while_either_read_holds_it_whichever_says_so_first() {
        use RelationRead::{MadeElsewhere, Project};
        let ends = Ends::default();
        let (task, r) = (id(1), id(4));
        let (at_task, runs) = reader({
            let ends = ends.clone();
            move || ends.of(task)
        });
        ends.place(task, r, Project, true);
        assert_eq!(at_task.get(), vec![r]);
        // It arrives in the other read before it leaves the project's.
        let before = runs.load(Ordering::SeqCst);
        ends.place(task, r, MadeElsewhere, true);
        ends.place(task, r, Project, false);
        assert_eq!(at_task.get(), vec![r], "arrival, then departure");
        assert_eq!(runs.load(Ordering::SeqCst), before, "nothing at the task changed, so its reader did not run again");
        // It leaves the other read before it arrives back in the project's.
        ends.place(task, r, MadeElsewhere, false);
        assert_eq!(at_task.get(), Vec::<EntityId>::new(), "between the departure and the arrival no read holds it");
        ends.place(task, r, Project, true);
        assert_eq!(at_task.get(), vec![r], "departure, then arrival");
        // Held by both, it stays while either does.
        ends.place(task, r, MadeElsewhere, true);
        ends.place(task, r, Project, false);
        assert_eq!(at_task.get(), vec![r], "dropped by one of the two that held it");
        ends.place(task, r, MadeElsewhere, false);
        assert_eq!(at_task.get(), Vec::<EntityId>::new(), "dropped by both");
        assert!(lock(&ends.by_entity).is_empty(), "an entity left with no relation leaves no entry");
    }

    #[test]
    fn a_trigger_goes_when_its_entity_leaves_and_comes_back_with_a_reader_that_reads_it_again() {
        let arrivals = Arrivals::default();
        let held = |arrivals: &Arrivals| lock(&arrivals.0).len();
        let (one, runs) = reader({
            let arrivals = arrivals.clone();
            move || arrivals.track(id(1))
        });
        one.get();
        assert_eq!(held(&arrivals), 1);
        arrivals.forget(id(1));
        assert_eq!(held(&arrivals), 0, "its entity left");
        one.get();
        assert_eq!(runs.load(Ordering::SeqCst), 2, "its reader ran again");
        assert_eq!(held(&arrivals), 1, "and read it again");
        arrivals.forget(id(2));
        assert_eq!(held(&arrivals), 1, "an id no one read has nothing to drop");
    }

    /// Principal `n` as a picker lists it, under the heading of `role`, or under "dormant".
    fn listed(n: u8, role: &'static str, dormant: bool, handle: &str) -> Listed {
        Listed {
            id: EntityId::from_bytes([n; 32]),
            heading: if dormant { DORMANT } else { role },
            role: rank(role),
            handle: handle.into(),
            text: handle.into(),
        }
    }

    #[test]
    fn a_picker_lists_the_dormant_agents_last_under_a_heading_of_their_own_in_the_order_of_their_roles() {
        let groups = grouped(vec![
            listed(1, "implementers", true, "impl-alpha"),
            listed(2, "implementers", false, "impl-zed"),
            listed(3, "people", false, "dnorman"),
            listed(4, "leads", true, "lead-old"),
            listed(5, "implementers", false, "impl-beta"),
            listed(6, "reviewers", true, "a-reviewer"),
            listed(7, "triage", false, "triage-bot"),
        ]);
        let shown: Vec<(&str, Vec<&str>)> =
            groups.iter().map(|g| (g.label, g.members.iter().map(|(_, text)| text.as_str()).collect())).collect();
        assert_eq!(
            shown,
            vec![
                ("people", vec!["dnorman"]),
                ("triage", vec!["triage-bot"]),
                ("implementers", vec!["impl-beta", "impl-zed"]),
                ("dormant", vec!["lead-old", "impl-alpha", "a-reviewer"]),
            ],
            "the others under their roles by name, the dormant ones last, by role and then by name"
        );
        let awake = grouped(vec![listed(1, "implementers", false, "impl-a")]);
        assert!(awake.iter().all(|g| g.label != DORMANT), "no dormant heading while no agent is dormant");
    }

    #[test]
    fn a_dormant_agents_option_names_its_role_and_a_triage_agents_what_it_serves() {
        let words = |role: Option<&str>, serving: Option<&str>| {
            with_words("triage-repl".into(), role.map(String::from), serving.map(String::from))
        };
        assert_eq!(words(None, None), "triage-repl");
        assert_eq!(words(None, Some("serving Replication")), "triage-repl (serving Replication)");
        assert_eq!(words(Some("triage"), Some("serving Replication")), "triage-repl (triage, serving Replication)");
        assert_eq!(words(Some("implementer"), None), "triage-repl (implementer)");
    }
}
