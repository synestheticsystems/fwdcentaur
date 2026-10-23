//! The page's standing live queries and the pure lookups over them.
//!
//! One query per collection for the whole page (a project holds dozens to hundreds of tasks,
//! not millions), opened once after boot. Every read below is tracked by the reactive_graph
//! bridge, which turns it into a Leptos dependency, so any closure in a `view!` that calls these
//! re-renders when the daemon sends a change. A whole collection (`tasks()`) is copied whole, and
//! its reader runs again at any change to any member. One entity looked up by its id (`task(id)`),
//! or the relations at one entity (`relations_of`), is copied alone: its reader runs again when
//! that entity arrives or leaves ([`Arrivals`]), and, through the fields it reads, when it changes.
//! A row looks up what it names by id, so that the page's work at each change grows with the rows
//! that name what changed rather than with its rows times everything it holds (tasks GEQEKfUd,
//! unFIWd5c).

use ankurah::changes::{ChangeSet, ItemChange};
use ankurah::signals::{Get as AnkurahGet, Subscribe, SubscriptionGuard};
use ankurah::{EntityId, LiveQuery, View};
use fwd_core::dormancy;
use fwd_core::model::*;
use fwd_core::naming::{self, Named, Work};
use fwd_core::rules::{self, Readiness, TaskSnapshot};
use leptos::prelude::{untrack, ArcRwSignal, ArcTrigger, Get, Notify, Set, Track};
use send_wrapper::SendWrapper;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use wasm_bindgen_futures::spawn_local;

use crate::session::ctx;

pub struct Store {
    pub tasks: LiveQuery<TaskView>,
    pub relations: LiveQuery<RelationView>,
    pub scopes: LiveQuery<ScopeView>,
    pub principals: LiveQuery<PrincipalView>,
    pub messages: LiveQuery<MessageView>,
    pub anchors: LiveQuery<CodeAnchorView>,
    pub decisions: LiveQuery<DecisionView>,
    /// Whether the daemon has answered every standing query above (`standing_loaded`).
    answered: ArcRwSignal<bool>,
    /// The readers of each entity looked up by its id in the queries above.
    arrivals: Arrivals,
    /// The relations at each entity (`relations_of`).
    ends: Ends,
    /// What keeps the two above current, for as long as the page.
    _watching: Vec<SubscriptionGuard>,
}

static STORE: OnceLock<SendWrapper<Store>> = OnceLock::new();

pub fn open() -> Result<(), String> {
    if STORE.get().is_some() {
        return Ok(());
    }
    let ctx = ctx();
    let refused =
        |what: &'static str| move |e: ankurah::error::RetrievalError| format!("the {what} query was refused: {e}");
    // `created > 0` rather than `>= 0`: a live query on `>= 0` never fires (Ankurah 0.9.3).
    let tasks = ctx.query::<TaskView>("created > 0").map_err(refused("tasks"))?;
    let relations = ctx.query::<RelationView>("created > 0").map_err(refused("relations"))?;
    let scopes = ctx.query::<ScopeView>("created > 0").map_err(refused("scopes"))?;
    let principals = ctx.query::<PrincipalView>("kind = 'human' OR kind = 'agent'").map_err(refused("principals"))?;
    let messages = ctx.query::<MessageView>("created > 0").map_err(refused("messages"))?;
    let anchors = ctx.query::<CodeAnchorView>("created > 0").map_err(refused("anchors"))?;
    let decisions = ctx.query::<DecisionView>("created > 0").map_err(refused("decisions"))?;
    // Watched from before any of them answers, so that every arrival is heard.
    let (arrivals, ends) = (Arrivals::default(), Ends::default());
    let watching = vec![
        arrivals.watch(&tasks),
        arrivals.watch(&relations),
        arrivals.watch(&scopes),
        arrivals.watch(&principals),
        arrivals.watch(&messages),
        arrivals.watch(&anchors),
        arrivals.watch(&decisions),
        ends.watch(&relations),
    ];
    let store = Store {
        tasks,
        relations,
        scopes,
        principals,
        messages,
        anchors,
        decisions,
        answered: ArcRwSignal::new(false),
        arrivals,
        ends,
        _watching: watching,
    };
    STORE.set(SendWrapper::new(store)).ok();
    spawn_local(record_the_daemons_answer());
    Ok(())
}

fn store() -> &'static Store {
    STORE.get().expect("store opened by App")
}

pub fn tasks() -> Vec<TaskView> {
    store().tasks.get()
}
pub fn relations() -> Vec<RelationView> {
    store().relations.get()
}
pub fn scopes() -> Vec<ScopeView> {
    store().scopes.get()
}
pub fn principals() -> Vec<PrincipalView> {
    store().principals.get()
}
pub fn messages() -> Vec<MessageView> {
    store().messages.get()
}
/// Every decision, by number (then by creation, should two share a number).
pub fn decisions() -> Vec<DecisionView> {
    let mut found = store().decisions.get();
    found.sort_by_key(|d| (d.number().unwrap_or(0), d.created().unwrap_or(0)));
    found
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

/// Records that the daemon has answered every standing query, or that one failed: a failure is
/// logged and counts as an answer, so nothing waits for it forever. The daemon answers them side
/// by side, so waiting for each in turn takes as long as the slowest.
async fn record_the_daemons_answer() {
    let store = store();
    let answers = [
        ("tasks", store.tasks.wait_durable_answered().await),
        ("relations", store.relations.wait_durable_answered().await),
        ("scopes", store.scopes.wait_durable_answered().await),
        ("principals", store.principals.wait_durable_answered().await),
        ("messages", store.messages.wait_durable_answered().await),
        ("anchors", store.anchors.wait_durable_answered().await),
        ("decisions", store.decisions.wait_durable_answered().await),
    ];
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

/// The ids of the relations at each entity, from or to it, in the order the relations query holds
/// them (by id), kept from the query's change sets, with the readers of each entity's.
#[derive(Clone, Default)]
struct Ends {
    by_entity: Arc<Mutex<HashMap<EntityId, Vec<EntityId>>>>,
    readers: Arrivals,
}

impl Ends {
    /// Keeps the ends of what `relations` gains and loses, for as long as the guard is kept. A
    /// relation's ends never change once it is written.
    fn watch(&self, relations: &LiveQuery<RelationView>) -> SubscriptionGuard {
        let ends = self.clone();
        relations.subscribe(move |changes: ChangeSet<RelationView>| {
            for change in &changes.changes {
                let Some(moved) = moved(change) else { continue };
                let arrived = moved == Moved::Arrived;
                let r = change.entity();
                // Untracked: this runs inside whatever the change interrupted.
                let Some((from, to)) = untrack(|| Some((r.from_id().ok()?, r.to_id().ok()?))) else { continue };
                ends.place(from, r.id(), arrived);
                if to != from {
                    ends.place(to, r.id(), arrived);
                }
            }
        })
    }

    /// Adds the relation to the entity's when it `arrived`, else takes it away, dropping an entity
    /// left with none, and runs the entity's readers again when that changed anything.
    fn place(&self, entity: EntityId, relation: EntityId, arrived: bool) {
        let changed = {
            let mut by_entity = lock(&self.by_entity);
            if arrived {
                let at = by_entity.entry(entity).or_default();
                match at.binary_search(&relation) {
                    Ok(_) => false,
                    Err(i) => {
                        at.insert(i, relation);
                        true
                    }
                }
            } else {
                let Some(at) = by_entity.get_mut(&entity) else { return };
                let Ok(i) = at.binary_search(&relation) else { return };
                at.remove(i);
                if at.is_empty() {
                    by_entity.remove(&entity);
                }
                true
            }
        };
        if changed {
            self.readers.fire(entity);
        }
    }

    /// The ids of the relations at `id`, read inside a closure, which then runs again when one of
    /// them arrives or leaves.
    fn of(&self, id: EntityId) -> Vec<EntityId> {
        self.readers.track(id);
        lock(&self.by_entity).get(&id).cloned().unwrap_or_default()
    }
}

/// The member `id` of `query`, one of the standing queries ([`Arrivals::member`]).
fn member<V: View + Clone + 'static>(query: &LiveQuery<V>, id: EntityId) -> Option<V> {
    store().arrivals.member(query, id)
}

pub fn task(id: EntityId) -> Option<TaskView> {
    member(&store().tasks, id)
}
pub fn scope(id: EntityId) -> Option<ScopeView> {
    member(&store().scopes, id)
}
pub fn message(id: EntityId) -> Option<MessageView> {
    member(&store().messages, id)
}
pub fn anchor(id: EntityId) -> Option<CodeAnchorView> {
    member(&store().anchors, id)
}
pub fn decision(id: EntityId) -> Option<DecisionView> {
    member(&store().decisions, id)
}
pub fn principal(id: EntityId) -> Option<PrincipalView> {
    member(&store().principals, id)
}
pub fn relation(id: EntityId) -> Option<RelationView> {
    member(&store().relations, id)
}

/// The relations from or to `id`, in the order the relations query holds them: read inside a
/// closure, which then runs again when one of them arrives or leaves, and, through the fields the
/// closure reads, when one of them changes.
pub fn relations_of(id: EntityId) -> Vec<RelationView> {
    let store = store();
    let ids = store.ends.of(id);
    untrack(|| ids.into_iter().filter_map(|r| store.relations.resultset().by_id(&r)).collect())
}
/// The principal `name` names among the page's principals, active or not, read as every typed name
/// is (`fwd_core::naming::resolve`: its name, or its id): none while no principal has it, and a
/// sentence listing them when two or more share it, each with the scopes its work is in
/// (docs/design/agents-and-inbox.md, "A principal's name").
pub fn principal_named(name: &str) -> Result<Option<PrincipalView>, String> {
    let principals = principals();
    match naming::resolve(&principals, name, None) {
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

/// A principal's handle, or a short id while the principal has not arrived.
pub fn handle_of(id: EntityId) -> String {
    principal(id).and_then(|p| p.handle().ok()).unwrap_or_else(|| crate::fmt::short(id))
}

/// The scope a task is worked in, named for a list: its name, or the start of its id while the page
/// does not hold it. Until 2026-09-29 its owner's handle followed the name; a scope has no owner
/// now (docs/DECISIONS.md, D108, "A scope has no owner: scopes are not assigned to principals").
pub fn scope_label(scope: EntityId) -> String {
    match self::scope(scope) {
        Some(s) => s.title().unwrap_or_default(),
        None => crate::fmt::short(scope),
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
    task_scopes(task, &relations_of(task))
}

/// The same three-valued answer the CLI gives, from the snapshots already in hand.
pub fn readiness_of(task: &TaskView, tasks: &[TaskView], relations: &[RelationView]) -> Readiness {
    let status_by_id: HashMap<EntityId, TaskStatus> =
        tasks.iter().filter_map(|t| Some((t.id(), t.status().ok()?))).collect();
    readiness_from(task, relations, |to| status_by_id.get(&to).copied())
}

/// [`readiness_of`] for one task, its dependencies looked up by id rather than among copies of
/// every task and relation.
pub fn readiness(task: &TaskView) -> Readiness {
    readiness_from(task, &relations_of(task.id()), |to| self::task(to).and_then(|t| t.status().ok()))
}

/// The readiness of `task` given `relations`, among them its own, and the status of each task by
/// id.
fn readiness_from(
    task: &TaskView,
    relations: &[RelationView],
    status: impl Fn(EntityId) -> Option<TaskStatus>,
) -> Readiness {
    let deps: Vec<(EntityId, Option<TaskStatus>)> = relations
        .iter()
        .filter(|r| {
            r.from_id().ok() == Some(task.id())
                && r.kind().ok() == Some(RelationKind::DependsOn)
                && r.status().ok() == Some(RelationStatus::Accepted)
        })
        .filter_map(|r| r.to_id().ok())
        .map(|to| (to, status(to)))
        .collect();
    let snapshot = TaskSnapshot {
        id: task.id(),
        status: task.status().unwrap_or(TaskStatus::Proposed),
        hold: task.hold().unwrap_or(None),
    };
    rules::readiness(&snapshot, &deps)
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
        let ends = Ends::default();
        let (task, other) = (id(1), id(9));
        let (at_task, runs) = reader({
            let ends = ends.clone();
            move || ends.of(task)
        });
        assert_eq!(at_task.get(), Vec::<EntityId>::new());
        ends.place(task, id(4), true);
        ends.place(task, id(2), true);
        ends.place(task, id(2), true);
        assert_eq!(at_task.get(), vec![id(2), id(4)], "by id, as the query holds them, each once");
        let before = runs.load(Ordering::SeqCst);
        ends.place(other, id(3), true);
        ends.place(task, id(5), false);
        at_task.get();
        assert_eq!(runs.load(Ordering::SeqCst), before, "another entity's relation, or one it never had, runs nothing");
        ends.place(task, id(4), false);
        assert_eq!(at_task.get(), vec![id(2)]);
        assert_eq!(ends.of(other), vec![id(3)]);
        // An entity left with no relation leaves no entry, and taking away one it never had makes
        // none.
        ends.place(task, id(2), false);
        assert_eq!(at_task.get(), Vec::<EntityId>::new());
        ends.place(id(7), id(8), false);
        let held: Vec<EntityId> = lock(&ends.by_entity).keys().copied().collect();
        assert_eq!(held, vec![other]);
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
