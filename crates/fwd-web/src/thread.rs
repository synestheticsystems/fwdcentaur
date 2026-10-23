//! What hangs beneath a row: the conversation about the row's entity, so it reads the way it
//! happened: a question, the answers under it, the follow-up under an answer; a task's comments
//! and the messages about it; a decision's comments. Only messages hang beneath a row (Daniel,
//! 2026-09-26: "Not referring to replies. Just things like relates to, and such"): an entity
//! joined to the row's by a relation (the task filed from an answer, a ruling a task cites, a
//! subtask) is a lozenge on the row's own line instead (`lozenges.rs`). A card mounts the row of
//! the entity it is about ([`card_row`]). That row's component (`message_row.rs`, `task_row.rs`,
//! `decision_row.rs`, `scope_row.rs`, `anchor_row.rs`, `person.rs`) draws its own fields, its
//! lozenges and its actions and hands them to [`frame`], which every row is drawn in and which
//! draws the messages beneath the row, choosing each child's component by the models the child
//! carries. A row with what hangs beneath it is the embedded mode of its entity's kind
//! (`modes.rs`), the one mode in which the page recurses. A page that names the entity above its
//! rows already, as a task's panel does, mounts only the rows beneath it ([`rows_beneath`]), so
//! that no row restates it.
//!
//! Which links each model follows is declared by the row modules (`FOLLOWS`) and gathered into
//! [`RULES`]; the order, the states and the cycle checks come from `referents.rs`; where the keys
//! take the focus comes from `navigation.rs`; the live queries are shared and counted by
//! `query_cache.rs`. Rows are keyed by their appearance with a keyed `<For>` and read what can
//! change about their entity inside closures, so a row keeps its focus, its reply box and its
//! queries while rows around it arrive, move and leave. What must outlive a row (the focus, which
//! rows are open, drafts, errors, the row to focus once it arrives) lives in [`ThreadState`],
//! keyed by appearance, so a live update never resets it.

use crate::empty::LOADING;
use crate::mentions::{MentionList, Mentions};
use crate::navigation::{self, Head, Mounted, PendingFocus, Press, RowFacts, Step, Stop};
use crate::query_cache::{Hold, QueryCache, QueryLayer};
use crate::referents::{self, Appearance, Appearances, Child, Level, Listing, Models, Row, Rules, State};
use crate::session::{self, ctx};
use crate::{anchor_row, decision_row, fmt, keys, lozenges, message_row, modes, person, scope_row, task_row};
use ankurah::signals::Get as AnkurahGet;
use ankurah::{EntityId, LiveQuery};
use futures::future::{FutureExt, RemoteHandle};
use fwd_core::model::MessageView;
use fwd_core::query::with_id;
use leptos::prelude::*;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;

/// How many children a row shows before "N more", and how many more each click shows.
const PAGE: usize = 50;

/// How long a sent reply may take to arrive and still take the focus.
const PENDING_FOCUS_MS: f64 = 5_000.0;

/// The models an entity can carry, in the page's names for them. Every fwdcentaur entity carries
/// exactly one of them today; Manifold's models are meant to sit beside them on the same entities.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Model {
    Message,
    Task,
    Decision,
    Scope,
    /// A code anchor: a highlight, a span of lines in a file (or a whole file) at a basis.
    Anchor,
    /// A person or an agent.
    Principal,
}

/// The links a thread follows from an entity to the messages that answer or discuss it. Every
/// link leads to messages: an entity joined by a relation is a lozenge on the row instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Link {
    /// Messages whose `reply_to` is the message: its replies, and the forwards of it.
    Replies,
    /// Messages whose `task` is the task and that answer no other message.
    Comments,
    /// Messages with a `concerns` relation to the task that is neither retracted nor rejected.
    Concerns,
    /// Messages whose `decision` is the decision and that answer no other message.
    DecisionComments,
    /// Messages whose `anchor` is the highlight and that answer no other message: the first
    /// message of each of its threads.
    AnchorComments,
}

/// Which links each model follows, as each row module declares them.
const RULES: Rules<Model, Link> = Rules::new(&[
    (Model::Message, message_row::FOLLOWS),
    (Model::Task, task_row::FOLLOWS),
    (Model::Decision, decision_row::FOLLOWS),
    (Model::Scope, scope_row::FOLLOWS),
    (Model::Anchor, anchor_row::FOLLOWS),
    (Model::Principal, person::FOLLOWS),
]);

/// Opens the live queries behind the links that need one (a message's replies, a task's or a
/// decision's comments, a highlight's threads). The messages about a task are joined to it by
/// relations, which the page's standing queries hold (`store.rs`), so following that link opens
/// nothing.
struct Queries;

/// One open child query.
pub enum Query {
    Live(Live),
    /// The query could not be opened, for this reason.
    Refused(String),
}

/// A live query for messages, and whether the daemon has answered it. The query first answers from
/// this load's own browser storage, which starts empty on every load, so until the daemon's answer
/// arrives an empty result means not yet rather than none, and a row says it is loading rather
/// than that nothing hangs beneath it (task tcUrcN1T).
pub struct Live {
    query: LiveQuery<MessageView>,
    /// Set once the daemon has answered the query, or the query has failed.
    answered: ArcRwSignal<bool>,
    /// The wait for the daemon's answer, which ends when this is dropped: once the last row
    /// holding the query has left the page, the wait no longer keeps the query open at the daemon.
    _waiting: RemoteHandle<()>,
}

impl Live {
    /// `query`, with a wait beside it for the daemon's answer. A failure ends the wait too; the
    /// query's own error says what failed.
    fn new(query: LiveQuery<MessageView>) -> Live {
        let answered = ArcRwSignal::new(false);
        let (wait, waiting) = {
            let (query, answered) = (query.clone(), answered.clone());
            async move {
                let _ = query.wait_durable_answered().await;
                answered.set(true);
            }
        }
        .remote_handle();
        spawn_local(wait);
        Live { query, answered, _waiting: waiting }
    }
}

impl QueryLayer for Queries {
    type Id = EntityId;
    type Link = Link;
    type Query = Query;

    fn open(&self, parent: EntityId, link: Link) -> Option<Query> {
        match link {
            Link::Replies => message_row::open(parent, link),
            Link::Comments | Link::Concerns => task_row::open(parent, link),
            Link::DecisionComments => decision_row::open(parent, link),
            Link::AnchorComments => anchor_row::open(parent, link),
        }
    }
}

/// The page's shared child queries, one per entity and link, whichever card's rows hold them.
fn cache() -> &'static QueryCache<Queries> {
    static CACHE: OnceLock<QueryCache<Queries>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let cache = QueryCache::new(Queries);
        let count = open_queries();
        // Told to the page once the rows now arriving or leaving have settled.
        cache.watch(move |n| {
            let count = count.clone();
            queue_microtask(move || count.set(n));
        });
        cache
    })
}

/// How many live queries the rows on the page hold.
fn open_queries() -> ArcRwSignal<usize> {
    static COUNT: OnceLock<ArcRwSignal<usize>> = OnceLock::new();
    COUNT.get_or_init(|| ArcRwSignal::new(0)).clone()
}

/// The number of live queries the rows on the page hold, shown in a page's header when the URL
/// has `?debug=1`: closing a branch, or switching principals, should bring it back where it was.
#[component]
pub fn OpenQueries() -> impl IntoView {
    let count = open_queries();
    session::debug().then(|| {
        view! {
            <span class="muted" title="live queries the rows on this page hold">
                "thread queries: " {move || count.get()}
            </span>
        }
    })
}

/// A live query for the messages that match `predicate`, its one `?` bound to `id`.
pub fn messages_where(predicate: &'static str, id: EntityId) -> Query {
    let opened = with_id(predicate, id)
        .map_err(|e| e.to_string())
        .and_then(|selection| ctx().query::<MessageView>(selection).map_err(|e| e.to_string()));
    match opened {
        Ok(query) => Query::Live(Live::new(query)),
        Err(e) => Query::Refused(format!("the thread's query was refused: {e}")),
    }
}

/// What a link answered by a live message query lists: a child for each member `child` accepts,
/// once the daemon has answered the query ([`Live`]), and until then that it is loading. Read
/// inside a reactive closure, so the level redraws when the query's results change and when the
/// daemon's answer arrives.
pub fn from_messages(
    query: Option<&Query>,
    child: impl Fn(&MessageView) -> Option<Child<EntityId, Model, Link>>,
) -> Listing<EntityId, Model, Link> {
    let live = match query {
        Some(Query::Live(live)) => live,
        Some(Query::Refused(reason)) => return Listing::Failed(reason.clone()),
        None => return Listing::Failed("the thread opened no query for this link".into()),
    };
    // Read before anything else, so the level depends on the results even while they load.
    let members = live.query.get();
    if let Some(error) = live.query.error().get() {
        return Listing::Failed(format!("the thread's query failed: {error}"));
    }
    if !live.answered.get() {
        return Listing::Loading;
    }
    Listing::Ready(members.iter().filter_map(child).collect())
}

/// The messages beneath `parent` through `link`, from the row module whose model follows that
/// link.
fn listing(parent: EntityId, link: Link, query: Option<&Query>) -> Listing<EntityId, Model, Link> {
    match link {
        Link::Replies => message_row::listing(parent, link, query),
        Link::Comments | Link::Concerns => task_row::listing(parent, link, query),
        Link::DecisionComments => decision_row::listing(parent, link, query),
        Link::AnchorComments => anchor_row::listing(parent, link, query),
    }
}

/// How a row module names an entity of its model in a few words, when the page holds it.
type Namer = fn(EntityId) -> Option<String>;

/// A few words that name an entity: a row that stands for a cycle shows them, and so do the note
/// on a row whose entity also shows elsewhere on the page and a reference in a message's text
/// where no lozenge can go (`references::plain`).
pub fn name(id: EntityId, models: &Models<Model>) -> String {
    let namers: [(Model, Namer); 6] = [
        (Model::Task, task_row::name),
        (Model::Decision, decision_row::name),
        (Model::Scope, scope_row::name),
        (Model::Message, message_row::name),
        (Model::Anchor, anchor_row::name),
        (Model::Principal, person::name),
    ];
    // By the models the entity carries first, then by any other, for one not loaded yet.
    let (own, other): (Vec<_>, Vec<_>) = namers.into_iter().partition(|(model, _)| models.contains(*model));
    own.into_iter().chain(other).find_map(|(_, namer)| namer(id)).unwrap_or_else(|| fmt::short(id))
}

/// What a card's rows and the page around them tell each other.
#[derive(Clone, Copy, Default)]
pub struct Hooks {
    /// The keyboard came into the card's rows: a row, or a box to write in beneath one, took the
    /// focus.
    pub entered: Option<Callback<()>>,
    /// Whether the card's own row shows its reply box (while the card is selected).
    pub reply_box: Option<Signal<bool>>,
    /// Shows the card's own reply box with the caret in it (the page selects the card), for the
    /// reply button and Enter on the card's own row.
    pub reply: Option<Callback<()>>,
    /// Up (-1) or Down (1) past the card's rows (off an edge of its reply box, or above its own
    /// row): the page selects the card above or below this one, and answers whether it did.
    pub step: Option<Callback<isize, bool>>,
    /// A reply was sent from one of the card's reply boxes, or a move was made from its own row
    /// (a proposal accepted): the card's notifications are dealt with.
    pub sent: Option<Callback<()>>,
    /// The task's prose the card's own row shows beneath its line, labelled, when the card's own
    /// row is a task: what a reviewer or an approver needs without opening the task.
    pub task_fields: Option<Signal<Vec<task_row::Field>>>,
    /// Whether the page draws the lozenges of the card's own entity itself, above the card's own
    /// row (a decision's panel, under its own heading), so the row leaves them out rather than
    /// showing them twice.
    pub strip_above: bool,
    /// The entity the page shows the card's own row beneath, as a step opened in place shows
    /// beneath its task: the row leaves that entity's lozenge out, as a row does the entity of
    /// the row it hangs under.
    pub beneath: Option<EntityId>,
}

/// A reply box's text, sent: the id of the entity it became, or why it could not be sent.
pub type Sending = Pin<Box<dyn Future<Output = Result<EntityId, String>>>>;

/// The reply box a row of some models takes, or the box that edits a row's own text in place
/// ([`edit_box`]).
#[derive(Clone, Copy)]
pub struct Editor {
    pub placeholder: &'static str,
    /// Sends the text written under an entity, as the acting principal (first); answers the id
    /// of the entity the text became, whose row takes the focus once it arrives. For an edit box,
    /// saves the text as the entity's own and answers the entity's id.
    pub send: fn(EntityId, EntityId, String) -> Sending,
}

/// What a row does besides showing itself: the reply box it takes, or what choosing it does.
#[derive(Clone, Copy, Default)]
pub struct Actions {
    pub editor: Option<Editor>,
    pub activate: Option<fn(EntityId)>,
}

/// One card's rows: its own row, whether that row heads them or is left out, the row that takes
/// the Tab key, and what the card asked to hear.
#[derive(Clone, Copy)]
struct Tree {
    root: Appearance,
    head: Head,
    tabbable: Memo<Appearance>,
    hooks: Hooks,
    /// The first row beneath the card's own row, as the page shows it now, which that row's frame
    /// keeps where the row is left out: where the Tab key enters the card then.
    first: RwSignal<Option<Appearance>>,
}

/// Where a row sits: what its component needs to draw it and the thread beneath it.
#[derive(Clone)]
pub struct RowAt {
    pub appearance: Appearance,
    pub id: EntityId,
    pub models: Models<Model>,
    /// The entity of the row this one hangs under; none for a card's own row.
    parent: Option<EntityId>,
    levels_left: usize,
    /// The level that lists this row, for its current label; none for a card's own row.
    listed_in: Option<Memo<Level<EntityId, Model, Link>>>,
    tree: Tree,
}

impl RowAt {
    /// Whether this is a card's own row, the one the card is about, rather than a row beneath it.
    pub fn is_card_row(&self) -> bool {
        self.listed_in.is_none()
    }

    /// Whether the page draws what the row's entity is joined to above the row, under a heading of
    /// its own (a card's own row in a decision's or a highlight's panel), so the row leaves it out
    /// rather than showing it twice.
    pub fn joined_drawn_above(&self) -> bool {
        self.is_card_row() && self.tree.hooks.strip_above
    }

    /// The row's lozenges, for its line: every entity joined to the row's by a relation, less the
    /// entity of the row it hangs under, which shows just above it. A card's own row in a panel
    /// that draws them under its own heading has none.
    pub fn strip(&self) -> AnyView {
        if self.joined_drawn_above() {
            return ().into_any();
        }
        let above = if self.is_card_row() { self.tree.hooks.beneath } else { self.parent };
        lozenges::strip(self.id, above)
    }

    /// The task prose the card asks its own row to show; none for any other row.
    pub fn card_task_fields(&self) -> Option<Signal<Vec<task_row::Field>>> {
        self.tree.hooks.task_fields.filter(|_| self.is_card_row())
    }

    /// What tells the card that its row dealt with what the card is about (a move made on its
    /// own row), as a sent reply does.
    pub fn card_sent(&self) -> Option<Callback<()>> {
        self.tree.hooks.sent
    }
}

/// What a row on the page last told the keyboard about itself.
#[derive(Clone, Default)]
struct Facts {
    /// The rows it shows beneath it, in order.
    shown: Vec<Appearance>,
    openable: bool,
    cycle: Option<Appearance>,
    editor: Option<Editor>,
    activate: Option<fn(EntityId)>,
    models: Models<Model>,
}

/// Everything about the rows of one page that must outlive a row: which row has the focus, which
/// rows are open, the drafts in their reply boxes, the errors under them, and the row waiting to
/// take the focus. All of it is keyed by appearance. `Copy`, so a closure takes it whole.
#[derive(Clone, Copy)]
pub struct ThreadState {
    /// The acting principal: replies are theirs, and so are resolutions.
    pub me: EntityId,
    /// Tells this page's element ids from another's.
    serial: u32,
    appearances: StoredValue<Appearances<EntityId, Link>>,
    focused: RwSignal<Option<Appearance>>,
    open: RwSignal<HashMap<Appearance, bool>>,
    /// Rows a person opened, below which six more levels open by themselves.
    by_hand: StoredValue<HashSet<Appearance>>,
    limits: RwSignal<HashMap<Appearance, usize>>,
    editors: RwSignal<HashSet<Appearance>>,
    drafts: RwSignal<HashMap<Appearance, String>>,
    /// The rows whose own text is being edited in place, with what their edit box holds.
    edits: RwSignal<HashMap<Appearance, String>>,
    errors: RwSignal<HashMap<Appearance, String>>,
    pending: StoredValue<Option<PendingFocus<EntityId>>>,
    mounted: StoredValue<Mounted<EntityId>>,
    /// Changes whenever rows arrive on the page or leave it, for the notes on repeated rows.
    arrivals: RwSignal<u64>,
    /// Whether a change to `arrivals` is already on its way.
    announcing: StoredValue<bool>,
    facts: StoredValue<HashMap<Appearance, Facts>>,
}

fn document() -> Option<web_sys::Document> {
    web_sys::window().and_then(|w| w.document())
}

/// Whether the browser's focus is nowhere in particular, as when the element that had it left the
/// page.
fn focus_lost() -> bool {
    document().and_then(|d| d.active_element()).is_none_or(|active| active.tag_name() == "BODY")
}

/// Puts the caret at the end of the text box whose element id is `id`, once it is drawn.
fn caret_to_end(id: String) {
    request_animation_frame(move || {
        let element = document().and_then(|d| d.get_element_by_id(&id));
        if let Some(text_box) = element.and_then(|e| e.dyn_into::<web_sys::HtmlTextAreaElement>().ok()) {
            let _ = text_box.focus();
            let end = text_box.text_length();
            let _ = text_box.set_selection_range(end, end);
        }
    });
}

/// Whether moving the focus takes no one's typing away: it is nowhere, or on a row.
fn focus_is_free() -> bool {
    document()
        .and_then(|d| d.active_element())
        .is_none_or(|active| active.tag_name() == "BODY" || active.get_attribute("role").as_deref() == Some("treeitem"))
}

impl ThreadState {
    pub fn new(me: EntityId) -> Self {
        static SERIAL: AtomicU32 = AtomicU32::new(0);
        ThreadState {
            me,
            serial: SERIAL.fetch_add(1, Ordering::Relaxed),
            appearances: StoredValue::new(Appearances::new()),
            focused: RwSignal::new(None),
            open: RwSignal::new(HashMap::new()),
            by_hand: StoredValue::new(HashSet::new()),
            limits: RwSignal::new(HashMap::new()),
            editors: RwSignal::new(HashSet::new()),
            drafts: RwSignal::new(HashMap::new()),
            edits: RwSignal::new(HashMap::new()),
            errors: RwSignal::new(HashMap::new()),
            pending: StoredValue::new(None),
            mounted: StoredValue::new(Mounted::default()),
            arrivals: RwSignal::new(0),
            announcing: StoredValue::new(false),
            facts: StoredValue::new(HashMap::new()),
        }
    }

    /// Opens a row's reply box (or, for a card's own row, the card's), for its reply button.
    pub fn edit(self, at: &RowAt) -> Callback<()> {
        let (tree, row) = (at.tree, at.appearance);
        Callback::new(move |()| self.start_editing(tree, row))
    }

    /// Shows a sentence under a row, for an action on it that failed.
    pub fn report(self, row: Appearance) -> Callback<String> {
        Callback::new(move |reason: String| {
            self.errors.update(|errors| {
                errors.insert(row, reason);
            });
        })
    }

    /// Opens the box that edits the row's own text in place ([`edit_box`]) holding `text`, or keeps
    /// the draft already in it, and puts the caret at its end: the edit button of a message the
    /// acting principal wrote, or whose author it succeeded.
    pub fn start_edit(self, row: Appearance, text: String) {
        self.edits.update(|edits| {
            edits.entry(row).or_insert(text);
        });
        self.focus_edit_box(row);
    }

    /// Whether the row's edit box is open. It changes when the box opens or closes, never as its
    /// text does, so the row's line is not drawn again at each key.
    pub fn editing(self, row: Appearance) -> Memo<bool> {
        Memo::new(move |_| self.edits.with(|edits| edits.contains_key(&row)))
    }

    /// Closes the row's edit box, dropping what was written in it and the reason under the row, if
    /// any; the row takes the focus when the box had it.
    fn close_edit(self, row: Appearance) {
        self.edits.update(|edits| {
            edits.remove(&row);
        });
        self.errors.update(|errors| {
            errors.remove(&row);
        });
        self.refocus_after_box(row);
    }

    /// Saves what the row's edit box holds as the row's own text, through `editor`, as the acting
    /// principal. Once the write succeeds the box closes (unless something new was written in it
    /// meanwhile) and the row, showing the new text, takes the focus when the box had it; a refusal
    /// shows under the row, and the box stays open with the text as written, to correct.
    fn save_edit(self, row: Appearance, editor: Editor) {
        let Some(text) = self.edits.with_untracked(|edits| edits.get(&row).cloned()) else { return };
        let (me, id) = (self.me, self.entity(row));
        self.errors.update(|errors| {
            errors.remove(&row);
        });
        spawn_local(async move {
            match (editor.send)(me, id, text.clone()).await {
                Ok(_) => {
                    session::wrote(me);
                    let unchanged = self.edits.try_with_untracked(|edits| edits.get(&row) == Some(&text));
                    if unchanged == Some(true) {
                        self.edits.try_update(|edits| {
                            edits.remove(&row);
                        });
                        self.refocus_after_box(row);
                    }
                }
                Err(reason) => {
                    self.errors.try_update(|errors| {
                        errors.insert(row, reason);
                    });
                }
            }
        });
    }

    /// Once a box inside the row has gone, gives the row the focus if the box took it along.
    fn refocus_after_box(self, row: Appearance) {
        request_animation_frame(move || {
            if focus_lost() {
                self.focus(row, true);
            }
        });
    }

    /// The entities that show above the row at `at`: every row it hangs beneath, up to the card's
    /// own, and what the card itself shows beneath (a step's task). What a row shows beneath its
    /// text (the highlights a message names) leaves them out, as a row's lozenges leave out the
    /// entity of the row it hangs under.
    pub fn shown_above(self, at: &RowAt) -> Vec<EntityId> {
        let mut above: Vec<EntityId> = self.appearances.with_value(|ap| {
            let path = ap.path(at.appearance);
            path[..path.len() - 1].iter().map(|row| ap.entity(*row)).collect()
        });
        above.extend(at.tree.hooks.beneath);
        above
    }

    fn element_id(self, row: Appearance) -> String {
        format!("thread-{}-{}", self.serial, row.index())
    }

    fn editor_id(self, row: Appearance) -> String {
        format!("thread-{}-{}-reply", self.serial, row.index())
    }

    fn edit_box_id(self, row: Appearance) -> String {
        format!("thread-{}-{}-edit", self.serial, row.index())
    }

    /// The appearance a row element of this page stands for.
    fn appearance_of(self, element: &web_sys::Element) -> Option<Appearance> {
        if !element.id().starts_with(&format!("thread-{}-", self.serial)) {
            return None;
        }
        let index = element.get_attribute("data-appearance")?.parse::<u32>().ok()?;
        self.appearances.with_value(|ap| ap.get(index))
    }

    fn entity(self, row: Appearance) -> EntityId {
        self.appearances.with_value(|ap| ap.entity(row))
    }

    /// A row's `aria-level`: 1 for a card's own row, or where the card leaves that row out, for
    /// the rows beneath it.
    fn level_of(self, tree: Tree, row: Appearance) -> String {
        let depth = self.appearances.with_value(|ap| ap.depth(row));
        match tree.head {
            Head::OwnRow => depth + 1,
            Head::RowsBeneath => depth,
        }
        .to_string()
    }

    fn models_of(self, row: Appearance) -> Models<Model> {
        self.facts.with_value(|facts| facts.get(&row).map(|f| f.models.clone())).unwrap_or_default()
    }

    fn is_on_page(self, row: Appearance) -> bool {
        let id = self.entity(row);
        self.mounted.with_value(|mounted| mounted.contains(id, row))
    }

    /// Whether another appearance of `id` on the page is open, so this one starts closed.
    fn open_elsewhere(self, id: EntityId, row: Appearance) -> bool {
        let others = self.mounted.with_value(|mounted| mounted.others(id, row));
        self.open.with_untracked(|open| others.iter().any(|other| open.get(other) == Some(&true)))
    }

    /// Moves the page's idea of the focused row to `row`, and the browser's focus with it when
    /// `take` is set (once the row is drawn).
    fn focus(self, row: Appearance, take: bool) {
        self.focused.set(Some(row));
        if take {
            let id = self.element_id(row);
            request_animation_frame(move || {
                let element = document().and_then(|d| d.get_element_by_id(&id));
                if let Some(element) = element.and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok()) {
                    let _ = element.focus();
                }
            });
        }
    }

    /// Puts the caret at the end of `row`'s reply box, once it is drawn.
    fn focus_editor(self, row: Appearance) {
        caret_to_end(self.editor_id(row));
    }

    /// Puts the caret at the end of `row`'s edit box, once it is drawn.
    fn focus_edit_box(self, row: Appearance) {
        caret_to_end(self.edit_box_id(row));
    }

    fn set_open(self, row: Appearance, open: bool, by_hand: bool) {
        if open && by_hand {
            self.by_hand.update_value(|rows| {
                rows.insert(row);
            });
        }
        self.open.update(|rows| {
            rows.insert(row, open);
        });
    }

    fn show_more(self, row: Appearance) {
        self.limits.update(|limits| {
            *limits.entry(row).or_insert(PAGE) += PAGE;
        });
    }

    fn start_editing(self, tree: Tree, row: Appearance) {
        // The card's own row replies through the card's reply box, which shows while the card is
        // selected.
        if row == tree.root {
            if let Some(reply) = tree.hooks.reply {
                reply.run(());
                return;
            }
        }
        self.editors.update(|editors| {
            editors.insert(row);
        });
        self.focus_editor(row);
    }

    fn is_card_box(tree: Tree, row: Appearance) -> bool {
        row == tree.root && tree.hooks.reply_box.is_some()
    }

    /// Escape in a reply box: the caret leaves it for its row. A box with nothing written in it
    /// closes; one with a draft stays open, so the draft stays in sight, and the card's own box
    /// stays while the card is selected.
    fn leave_editor(self, tree: Tree, row: Appearance) {
        let empty = self.drafts.with_untracked(|drafts| drafts.get(&row).is_none_or(|text| text.trim().is_empty()));
        if empty && !Self::is_card_box(tree, row) {
            self.editors.update(|editors| {
                editors.remove(&row);
            });
        }
        self.focus(row, true);
    }

    /// Closes a row's reply box and drops what was written in it.
    fn cancel(self, row: Appearance) {
        self.drafts.update(|drafts| {
            drafts.remove(&row);
        });
        self.errors.update(|errors| {
            errors.remove(&row);
        });
        self.editors.update(|editors| {
            editors.remove(&row);
        });
        self.focus(row, true);
    }

    /// Sends what is written in `row`'s reply box. The card's own box stays open with the caret in
    /// it, ready for the next reply; any other box closes, its row opens so the reply shows
    /// beneath it, and the reply's row takes the focus when it arrives. A failed send puts the
    /// text back (unless something new was written meanwhile) with the reason under the row.
    fn send(self, tree: Tree, row: Appearance) {
        let text =
            self.drafts.with_untracked(|drafts| drafts.get(&row).map(|t| t.trim().to_string()).unwrap_or_default());
        let editor = self.facts.with_value(|facts| facts.get(&row).and_then(|f| f.editor));
        let Some(editor) = editor.filter(|_| !text.is_empty()) else { return };
        let card_box = Self::is_card_box(tree, row);
        let under = self.entity(row);
        self.drafts.update(|drafts| {
            drafts.remove(&row);
        });
        self.errors.update(|errors| {
            errors.remove(&row);
        });
        if !card_box {
            self.editors.update(|editors| {
                editors.remove(&row);
            });
            self.focus(row, true);
        }
        self.set_open(row, true, true);
        let (me, sent) = (self.me, tree.hooks.sent);
        spawn_local(async move {
            match (editor.send)(me, under, text.clone()).await {
                Ok(reply) => {
                    session::wrote(me);
                    if !card_box {
                        self.expect(row, reply);
                    }
                    if let Some(sent) = sent {
                        sent.try_run(());
                    }
                }
                Err(reason) => {
                    self.errors.try_update(|errors| {
                        errors.insert(row, reason);
                    });
                    self.drafts.try_update(|drafts| {
                        drafts.entry(row).or_insert(text);
                    });
                    if !card_box {
                        self.editors.try_update(|editors| {
                            editors.insert(row);
                        });
                    }
                }
            }
        });
    }

    /// The reply `id` was sent under `parent`: its row takes the focus now if it is already on the
    /// page, else when it arrives (for a few seconds).
    fn expect(self, parent: Appearance, id: EntityId) {
        let here = self.mounted.try_with_value(|mounted| mounted.of(id)).unwrap_or_default();
        let arrived = here
            .into_iter()
            .find(|row| self.appearances.try_with_value(|ap| ap.parent(*row)).flatten() == Some(parent));
        match arrived {
            Some(row) => self.focus(row, focus_is_free()),
            None => {
                let until_ms = js_sys::Date::now() + PENDING_FOCUS_MS;
                self.pending.try_update_value(|pending| *pending = Some(PendingFocus { parent, id, until_ms }));
            }
        }
    }

    /// A row arriving under `parent` takes the focus when it is the reply the page waits for.
    fn claim(self, row: Appearance, parent: Appearance, id: EntityId) {
        let Some(pending) = self.pending.get_value() else { return };
        let now = js_sys::Date::now();
        if pending.claims(parent, &id, now) {
            self.pending.set_value(None);
            self.focus(row, focus_is_free());
        } else if now > pending.until_ms {
            self.pending.set_value(None);
        }
    }

    /// Notes a row arriving on the page, and its leaving when it goes.
    fn arrive(self, row: Appearance, id: EntityId) {
        self.mounted.update_value(|mounted| mounted.add(id, row));
        self.announce_arrivals();
        on_cleanup(move || {
            self.mounted.try_update_value(|mounted| mounted.remove(id, row));
            self.facts.try_update_value(|facts| {
                facts.remove(&row);
            });
            self.announce_arrivals();
        });
        if let Some(parent) = self.appearances.with_value(|ap| ap.parent(row)) {
            self.claim(row, parent, id);
        }
    }

    /// Tells the notes on repeated rows to look again, once the rows now arriving or leaving have
    /// settled: once for all of them, however many there are.
    fn announce_arrivals(self) {
        if self.announcing.try_get_value() != Some(false) {
            return;
        }
        self.announcing.set_value(true);
        let (arrivals, announcing) = (self.arrivals, self.announcing);
        queue_microtask(move || {
            announcing.try_update_value(|on_its_way| *on_its_way = false);
            arrivals.try_update(|n| *n += 1);
        });
    }

    /// Records what `row`, one of `tree`'s rows, shows and can do, for the keyboard; when a row
    /// that held the focus (or had it beneath it) is gone from what `row` shows, the focus goes to
    /// a sibling that stayed, else to `row`, unless `row` is the card's own row left out.
    fn publish(self, tree: Tree, row: Appearance, facts: Facts, before: Option<&[Appearance]>) {
        let after = facts.shown.clone();
        self.facts.update_value(|all| {
            all.insert(row, facts);
        });
        let (Some(before), Some(focused)) = (before, self.focused.get_untracked()) else { return };
        let gone = before
            .iter()
            .copied()
            .find(|gone| !after.contains(gone) && self.appearances.with_value(|ap| ap.is_within(focused, *gone)));
        if let Some(gone) = gone {
            match navigation::refocus(before, &after, gone, tree.head.shown_parent(tree.root, Some(row))) {
                Some(to) => self.focus(to, focus_lost()),
                None => self.focused.set(None),
            }
        }
    }

    fn activate(self, row: Appearance) {
        let activate = self.facts.with_value(|facts| facts.get(&row).and_then(|f| f.activate));
        if let Some(activate) = activate {
            activate(self.entity(row));
        }
    }

    /// Holds the queries beneath `id` for as long as the row at `row` is on the page, and answers
    /// the level they make.
    fn follow(self, row: Appearance, id: EntityId, models: &Models<Model>) -> Memo<Level<EntityId, Model, Link>> {
        let holds: Vec<Hold<Queries>> = RULES.links(models).into_iter().map(|link| cache().hold(id, link)).collect();
        // Dropped with the row, which lets each query go once no other row holds it.
        let holds = StoredValue::new(holds);
        let (limits, appearances) = (self.limits, self.appearances);
        Memo::new(move |_| {
            let limit = limits.with(|limits| limits.get(&row).copied().unwrap_or(PAGE));
            let listings: Vec<_> =
                holds.with_value(|holds| holds.iter().map(|hold| listing(id, hold.link(), hold.query())).collect());
            appearances.try_update_value(|ap| referents::level(ap, row, listings, limit)).unwrap_or_else(Level::empty)
        })
    }

    /// The stops of a card's rows, top to bottom, as the page shows them now. The card's own box
    /// is a stop only when its own row takes one (a message or a decision, not a task or a scope):
    /// a stop for a box that is not there would swallow the key that leaves the card.
    fn stops(self, tree: Tree) -> Vec<Stop> {
        let root_edits = self.facts.with_value(|facts| facts.get(&tree.root).is_some_and(|f| f.editor.is_some()));
        let card_box = root_edits && tree.hooks.reply_box.is_some_and(|shown| shown.get_untracked());
        let editors = self.editors.get_untracked();
        let editing = |row: Appearance| editors.contains(&row) || (row == tree.root && card_box);
        self.facts.with_value(|facts| {
            let shown = |row| facts.get(&row).map(|f| f.shown.clone()).unwrap_or_default();
            navigation::stops(tree.root, tree.head, shown, editing)
        })
    }

    fn row_facts(self, tree: Tree, row: Appearance) -> RowFacts {
        let facts = self.facts.with_value(|facts| facts.get(&row).cloned()).unwrap_or_default();
        RowFacts {
            row,
            parent: tree.head.shown_parent(tree.root, self.appearances.with_value(|ap| ap.parent(row))),
            open: self.open.with_untracked(|open| open.get(&row).copied().unwrap_or(false)),
            openable: facts.openable,
            first_child: facts.shown.first().copied(),
            cycle: facts.cycle,
            editor: facts.editor.is_some(),
            activate: facts.activate.is_some(),
        }
    }

    /// Carries out a step; false when the key was not the rows' to use.
    fn take(self, tree: Tree, step: Step) -> bool {
        match step {
            Step::Focus(to) => self.focus(to, true),
            Step::FocusEditor(row) => self.focus_editor(row),
            Step::Open(row) => self.set_open(row, true, true),
            Step::Close(row) => self.set_open(row, false, false),
            Step::Edit(row) => self.start_editing(tree, row),
            Step::Activate(row) => self.activate(row),
            Step::Stay => {}
            Step::Pass => return false,
        }
        true
    }

    /// The keys of a card's rows (see `navigation::on_row`) and of the reply boxes beneath them.
    /// A key the rows do not use goes on to the page, which selects the card above or below. A
    /// key something inside a row already used (a highlight's line beneath a message's text, or
    /// the rows of a highlight opened there, `attachments.rs`) is left alone.
    fn on_key(self, tree: Tree, ev: web_sys::KeyboardEvent) {
        if ev.default_prevented() {
            return;
        }
        let Some(target) = ev.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) else { return };
        let Some(row_element) = target.closest("[role=treeitem]").ok().flatten() else { return };
        let Some(row) = self.appearance_of(&row_element) else { return };
        if target.has_attribute("data-reply") {
            self.on_reply_key(tree, row, &target, &ev);
            return;
        }
        if keys::is_text_field(&target) || !keys::is_plain(&ev) {
            return;
        }
        let press = match ev.key().as_str() {
            "ArrowUp" => Press::Up,
            "ArrowDown" => Press::Down,
            "ArrowLeft" => Press::Left,
            "ArrowRight" => Press::Right,
            "Home" => Press::Home,
            "End" => Press::End,
            // Enter on a button or a link inside the row presses that instead.
            "Enter" if target.is_same_node(Some(row_element.as_ref())) => Press::Enter,
            _ => return,
        };
        let step = navigation::on_row(press, &self.stops(tree), &self.row_facts(tree, row));
        // Up above the card's own row, or Down past its last row and box, goes on to the card
        // above or below this one.
        if step == Step::Pass && matches!(press, Press::Up | Press::Down) {
            if let Some(next_card) = tree.hooks.step {
                if next_card.run(if press == Press::Up { -1 } else { 1 }) {
                    ev.prevent_default();
                }
            }
            return;
        }
        if self.take(tree, step) {
            ev.prevent_default();
        }
    }

    /// Enter sends and Shift+Enter starts a new line; Escape leaves the box for its row; Up on the
    /// first line and Down on the last (a line being what the text breaks into) leave the box for
    /// the stop above or below it, except that the card's own box hands them to the page, which
    /// selects the card above or below.
    fn on_reply_key(self, tree: Tree, row: Appearance, target: &web_sys::Element, ev: &web_sys::KeyboardEvent) {
        if keys::is_send(ev) {
            ev.prevent_default();
            self.send(tree, row);
            return;
        }
        let up = match ev.key().as_str() {
            "Escape" => {
                ev.prevent_default();
                self.leave_editor(tree, row);
                return;
            }
            "ArrowUp" => true,
            "ArrowDown" => false,
            _ => return,
        };
        let Some(reply) = target.dyn_ref::<web_sys::HtmlTextAreaElement>() else { return };
        let (first, last) = keys::caret_lines(reply);
        if !keys::is_plain(ev) || !(if up { first } else { last }) {
            return;
        }
        if Self::is_card_box(tree, row) {
            if let Some(step) = tree.hooks.step {
                if step.run(if up { -1 } else { 1 }) {
                    ev.prevent_default();
                }
                return;
            }
        }
        if self.take(tree, navigation::off_editor(up, &self.stops(tree), row)) {
            ev.prevent_default();
        }
    }

    /// Keeps the page's idea of the focused row with the browser's focus, and tells the card when
    /// a row or a box to write in took the focus, but not when a control that acts on its click
    /// did: a link (every lozenge is one), a button (a reply, a proposal's Accept) or the summary
    /// that opens a decision's record. A press on a control in a card below the selected one
    /// focuses the control before its release; selecting the card then would close the reply box
    /// of the card selected before, which moves this card up under the pointer, and the release
    /// would land beside the control, so its click would be lost (a proposal's Accept on such a
    /// card did nothing, 2026-09-28). What the control does selects the card when it should: a
    /// reply button opens a box, and the box taking the focus selects it.
    fn on_focus_in(self, tree: Tree, ev: web_sys::FocusEvent) {
        let target = ev.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok());
        let control = target.as_ref().is_some_and(|t| t.closest("a, button, summary").ok().flatten().is_some());
        let row = target.and_then(|t| t.closest("[role=treeitem]").ok().flatten()).and_then(|r| self.appearance_of(&r));
        if let Some(row) = row {
            if self.focused.get_untracked() != Some(row) {
                self.focused.set(Some(row));
            }
        }
        if let Some(entered) = tree.hooks.entered.filter(|_| !control) {
            entered.run(());
        }
    }
}

/// A card's own row, for the entity the card is about (`id`, carrying `models`), with every row
/// beneath it: one tree the keyboard moves through, a WAI-ARIA tree with one row in the Tab
/// order. `card` is the card's own id, so the same entity heading two cards has two appearances.
pub fn card_row(state: ThreadState, card: EntityId, id: EntityId, models: Models<Model>, hooks: Hooks) -> AnyView {
    outline(state, card, id, models, hooks, Head::OwnRow)
}

/// The rows beneath the entity `id` (carrying `models`) as [`card_row`] draws them, without the
/// entity's own row, for a page that names the entity above them already: a task's panel, whose
/// Thread section starts at the task's messages (Daniel, 2026-09-30: "the Thread section of a
/// task should not restate the name/id/status/tags of the task. that is redundant."). They are
/// the tree's top rows, always shown, since no row is there to close them with. `card` is as for
/// [`card_row`].
pub fn rows_beneath(state: ThreadState, card: EntityId, id: EntityId, models: Models<Model>, hooks: Hooks) -> AnyView {
    outline(state, card, id, models, hooks, Head::RowsBeneath)
}

/// One card's rows, headed as `head` says: the tree of [`card_row`] and [`rows_beneath`].
fn outline(
    state: ThreadState,
    card: EntityId,
    id: EntityId,
    models: Models<Model>,
    hooks: Hooks,
    head: Head,
) -> AnyView {
    let Some(root) = state.appearances.try_update_value(|ap| ap.root(card, id)) else { return ().into_any() };
    let first = RwSignal::new(None);
    // The row the Tab key reaches: the one last focused among this card's rows, else where the
    // Tab key enters the card. Never the card's own row left out, which no element stands for.
    let tabbable = Memo::new(move |_| {
        let focused = state.focused.get();
        focused
            .filter(|row| {
                state.appearances.with_value(|ap| ap.root_of(*row)) == root
                    && state.is_on_page(*row)
                    && !head.leaves_out(root, *row)
            })
            .unwrap_or_else(|| head.entry(root, first.get()))
    });
    let tree = Tree { root, head, tabbable, hooks, first };
    let at = RowAt {
        appearance: root,
        id,
        models,
        parent: None,
        // So that six levels beneath the card's own row open by themselves.
        levels_left: navigation::OPEN_LEVELS + 1,
        listed_in: None,
        tree,
    };
    let (class, top) = match head {
        Head::OwnRow => ("thread-rows", row(state, at)),
        // The frame of the card's own row left out draws only the rows beneath it, so no component
        // of its entity's kind draws a line or offers an action for it; the rows keep the look of
        // rows beneath a row (app.css).
        Head::RowsBeneath => ("thread-rows rows-beneath", frame(state, at, ().into_any(), Actions::default())),
    };
    view! {
        <ul class=class role="tree" aria-label="thread"
            on:keydown=move |ev| state.on_key(tree, ev)
            on:focusin=move |ev| state.on_focus_in(tree, ev)>
            {top}
        </ul>
    }
    .into_any()
}

/// The row for the entity at `at`, drawn in the embedded mode of the kind its models call for (a
/// message row, a task row, a highlight's row) or, for an entity carrying no model this page
/// draws, a row with its id and models.
fn row(state: ThreadState, at: RowAt) -> AnyView {
    match modes::kind(&at.models) {
        Some(kind) => (kind.embedded)(state, at),
        None => fallback_row(state, at),
    }
}

fn fallback_row(state: ThreadState, at: RowAt) -> AnyView {
    let carried = if at.models.is_empty() {
        "no model this page draws".to_string()
    } else {
        at.models.iter().map(|model| format!("{model:?}").to_lowercase()).collect::<Vec<_>>().join(", ")
    };
    let line = view! {
        <span class="mono">{fmt::short(at.id)}</span>
        " "
        <span class="muted">{carried}</span>
    }
    .into_any();
    frame(state, at, line, Actions::default())
}

/// A row whose entity already shows on its own path: it names the entity and leads back to that
/// appearance, so a cycle in the graph ends here rather than going round forever.
fn reference_row(state: ThreadState, at: RowAt, earlier: Appearance) -> AnyView {
    let RowAt { appearance: row, id, models, tree, .. } = at;
    state.arrive(row, id);
    let facts = Facts { cycle: Some(earlier), models: models.clone(), ..Facts::default() };
    state.facts.update_value(|all| {
        all.insert(row, facts);
    });
    // `aria-level` is set outside `view!`, which only accepts the ARIA attributes it knows.
    view! {
        <li role="treeitem" class="row reference" id=state.element_id(row) data-appearance=row.index().to_string()
            tabindex=move || if tree.tabbable.get() == row { "0" } else { "-1" }>
            <div class="row-line">
                <span class="caret"></span>
                <span class="muted">"see above: "</span>
                <button class="btn subtle tiny" title="go to where it shows above" on:click=move |_| state.focus(earlier, true)>
                    {move || name(id, &models)}
                </button>
            </div>
        </li>
    }
    .attr("aria-level", state.level_of(tree, row))
    .into_any()
}

/// The frame every row is drawn in, whatever its models: the element the keyboard moves between
/// (a WAI-ARIA treeitem), a caret when the row has or may have children, the row's own `line`,
/// the thread beneath it, then its reply box, where the reply it sends will appear. The row holds
/// the queries of the links its models follow for as long as it is on the page, open or closed,
/// so a closed row can say how many wait below it. A card's own row that the card leaves out
/// ([`rows_beneath`]) is drawn as the thread beneath it alone, open for good.
pub fn frame(state: ThreadState, at: RowAt, line: AnyView, actions: Actions) -> AnyView {
    let RowAt { appearance: row, id, models, levels_left, listed_in, tree, .. } = at;
    let left_out = tree.head.leaves_out(tree.root, row);
    state.arrive(row, id);
    let level = state.follow(row, id, &models);
    let starts_open = left_out || navigation::starts_open(levels_left, state.open_elsewhere(id, row));
    state.open.update_untracked(|open| {
        open.entry(row).or_insert(starts_open);
    });
    let open = Memo::new(move |_| state.open.with(|open| open.get(&row).copied().unwrap_or(false)));
    let openable = Memo::new(move |_| level.with(Level::may_have_children));
    // An open row shows its children; a closed one only those with a box open beneath them: a
    // reply box, or the box that edits a row's own text.
    let shown = Memo::new(move |_| {
        let is_open = open.get();
        level.with(|level| {
            if is_open {
                return level.rows.clone();
            }
            let boxes: HashSet<Appearance> = state
                .editors
                .with(|editors| state.edits.with(|edits| editors.iter().chain(edits.keys()).copied().collect()));
            state.appearances.with_value(|ap| {
                level.rows.iter().filter(|r| navigation::pinned(r.appearance, &boxes, ap)).cloned().collect()
            })
        })
    });
    let card_box = tree.hooks.reply_box.filter(|_| listed_in.is_none());
    let editing = Memo::new(move |_| {
        state.editors.with(|editors| editors.contains(&row)) || card_box.is_some_and(|shown| shown.get())
    });
    Effect::new(move |before: Option<Vec<Appearance>>| {
        let after: Vec<Appearance> = shown.with(|rows| rows.iter().map(|r| r.appearance).collect());
        if left_out {
            tree.first.set(after.first().copied());
        }
        let facts = Facts {
            shown: after.clone(),
            openable: openable.get(),
            cycle: None,
            editor: actions.editor,
            activate: actions.activate,
            models: models.clone(),
        };
        state.publish(tree, row, facts, before.as_deref());
        after
    });

    let caret = move || {
        if !openable.get() {
            return view! { <span class="caret"></span> }.into_any();
        }
        let toggle = move |_| {
            let now = open.get_untracked();
            state.set_open(row, !now, !now);
        };
        view! {
            <span class="caret" title="open or close the replies and comments beneath it" on:click=toggle>
                {move || if open.get() { "▾" } else { "▸" }}
            </span>
        }
        .into_any()
    };
    let label = move || {
        let label =
            listed_in?.with(|level| level.rows.iter().find(|r| r.appearance == row).map(|r| r.child.label.clone()));
        label.filter(|label| !label.is_empty()).map(|label| view! { <span class="rel-label">{label}</span> })
    };
    // A row whose entity shows elsewhere on the page too says where, and leads there: a small
    // mark at the row's end, the words on hover, so the note does not compete with the row's own
    // content (2026-09-26, after Daniel found rows carrying such notes too noisy to read). A "see
    // above" row beneath it, which closes a cycle back to it (a chain of replies that arrived
    // looping back on itself), is not somewhere else.
    let also = move || {
        state.arrivals.track();
        let others = state.mounted.with_value(|mounted| mounted.others(id, row));
        let other = others.into_iter().find(|other| !state.appearances.with_value(|ap| ap.is_within(*other, row)))?;
        let text = match state.appearances.with_value(|ap| ap.parent(other)) {
            Some(parent) => format!("also under {}", name(state.entity(parent), &state.models_of(parent))),
            None => "also a card of its own".to_string(),
        };
        Some(view! {
            <button class="also-mark" title=format!("{text}: click to go there") aria-label=text
                on:click=move |_| state.focus(other, true)>
                "↗"
            </button>
        })
    };
    let error =
        move || state.errors.with(|errors| errors.get(&row).cloned()).map(|e| view! { <p class="error">{e}</p> });
    let child = move |listed: Row<EntityId, Model, Link>| {
        let opened_by_hand = state.by_hand.with_value(|rows| rows.contains(&row));
        let at = RowAt {
            appearance: listed.appearance,
            id: listed.child.id,
            models: listed.child.models,
            parent: Some(id),
            levels_left: navigation::levels_below(levels_left, opened_by_hand),
            listed_in: Some(level),
            tree,
        };
        match listed.cycle {
            Some(earlier) => reference_row(state, at, earlier),
            None => self::row(state, at),
        }
    };
    // What the thread says besides its rows: never an empty list for something it does not know.
    let more = move || {
        let is_open = open.get();
        level.with(|level| {
            let mut notes: Vec<AnyView> = Vec::new();
            if is_open {
                if level.loading {
                    notes.push(view! { <li class="more" role="none"><span class="muted">{LOADING}</span></li> }.into_any());
                }
                for reason in level.failed.clone() {
                    notes.push(view! { <li class="more error" role="none">{reason}</li> }.into_any());
                }
                if level.hidden > 0 {
                    let label = format!("{} more", level.hidden);
                    notes.push(
                        view! {
                            <li class="more" role="none">
                                <button class="btn subtle tiny" on:click=move |_| state.show_more(row)>{label}</button>
                            </li>
                        }
                        .into_any(),
                    );
                }
            } else if level.total() > 0 {
                let label = format!("{} more below", level.total());
                notes.push(
                    view! {
                        <li class="more" role="none">
                            <button class="btn subtle tiny" on:click=move |_| state.set_open(row, true, true)>{label}</button>
                        </li>
                    }
                    .into_any(),
                );
            }
            notes
        })
    };
    let editor = actions.editor;
    let replying =
        move || editor.filter(|_| editing.get()).map(|editor| reply_box(state, tree, row, editor, card_box.is_some()));
    if left_out {
        return view! {
            <For each=move || shown.get() key=|listed| listed.appearance children=child/>
            {more}
        }
        .into_any();
    }
    // `aria-level` is set outside `view!`, which only accepts the ARIA attributes it knows.
    view! {
        <li role="treeitem" class="row" id=state.element_id(row) data-appearance=row.index().to_string()
            aria-expanded=move || openable.get().then(|| open.get().to_string())
            tabindex=move || if tree.tabbable.get() == row { "0" } else { "-1" }>
            <div class="row-line">{caret}{label}{line}{also}</div>
            {error}
            <ul role="group" class="thread-rows" aria-busy=move || level.with(|level| level.state() == State::Loading).to_string()>
                <For each=move || shown.get() key=|listed| listed.appearance children=child/>
                {more}
            </ul>
            {replying}
        </li>
    }
    .attr("aria-level", state.level_of(tree, row))
    .into_any()
}

/// The box that edits a row's own text in place, drawn by the row where its text shows while
/// [`ThreadState::editing`] says so, looking as a reply box does. What it holds is saved through
/// `editor` by Save or Enter, Shift+Enter starts a new line, and Cancel or Escape closes it and
/// leaves the text as it was. Its keys are its own: the rows leave a text box's keys alone, and it
/// marks the ones it uses as used, so the page's own do nothing with them either.
pub fn edit_box(state: ThreadState, row: Appearance, editor: Editor) -> AnyView {
    let text = Memo::new(move |_| state.edits.with(|edits| edits.get(&row).cloned().unwrap_or_default()));
    let write = move |written: String| {
        state.edits.update(|edits| {
            if let Some(text) = edits.get_mut(&row) {
                *text = written;
            }
        })
    };
    let field = NodeRef::<leptos::html::Textarea>::new();
    let mentions = Mentions::new(field);
    let on_key = move |ev: web_sys::KeyboardEvent| {
        if mentions.key(&ev, &text.get_untracked(), write) {
            return;
        }
        if keys::is_send(&ev) {
            ev.prevent_default();
            state.save_edit(row, editor);
        } else if ev.key() == "Escape" {
            ev.prevent_default();
            state.close_edit(row);
        }
    };
    view! {
        <div class="reply-box edit-box">
            <textarea id=state.edit_box_id(row) node_ref=field
                placeholder=editor.placeholder
                prop:value=move || text.get()
                on:input=move |ev| {
                    write(event_target_value(&ev));
                    mentions.typed();
                }
                on:keyup=move |_| mentions.moved()
                on:click=move |_| mentions.moved()
                on:blur=move |_| mentions.left()
                on:keydown=on_key></textarea>
            <MentionList mentions=mentions text=text.into() write=Callback::new(write)/>
            <div class="actions">
                <button class="btn primary" on:click=move |_| state.save_edit(row, editor)>"Save"</button>
                <button class="btn subtle" on:click=move |_| state.close_edit(row)>"Cancel"</button>
                <span class="keys">"Enter saves · Shift+Enter new line · Esc cancels"</span>
            </div>
        </div>
    }
    .into_any()
}

/// A row's reply box. The card's own box (`card_box`) is the one the page gives the focus to
/// when the card is selected; its keys move between cards rather than rows.
fn reply_box(state: ThreadState, tree: Tree, row: Appearance, editor: Editor, card_box: bool) -> AnyView {
    let draft = Memo::new(move |_| state.drafts.with(|drafts| drafts.get(&row).cloned().unwrap_or_default()));
    let write = move |text: String| {
        state.drafts.update(|drafts| {
            drafts.insert(row, text);
        })
    };
    let field = NodeRef::<leptos::html::Textarea>::new();
    let mentions = Mentions::new(field);
    // The arrows move between cards, and Shift twice reads aloud, on a page of cards only; a box
    // that is always there on another page (a highlight's) leaves itself as any other box does.
    let hint = if card_box && tree.hooks.step.is_some() {
        "Enter sends · Shift+Enter new line · ↑ ↓ at the first or last line move between cards · Esc leaves the box · Shift twice reads aloud"
    } else {
        "Enter sends · Shift+Enter new line · ↑ ↓ at the first or last line leave the box · Esc leaves it"
    };
    view! {
        <div class="reply-box">
            <textarea id=state.editor_id(row) data-reply="true" data-card-reply=card_box.then_some("true")
                node_ref=field
                placeholder=editor.placeholder
                prop:value=move || draft.get()
                on:input=move |ev| {
                    write(event_target_value(&ev));
                    mentions.typed();
                }
                on:keyup=move |_| mentions.moved()
                on:click=move |_| mentions.moved()
                on:blur=move |_| mentions.left()
                // The mention search takes its keys first, marking them used, so the rows leave
                // them (`on_key`).
                on:keydown=move |ev| {
                    mentions.key(&ev, &draft.get_untracked(), write);
                }></textarea>
            <MentionList mentions=mentions text=draft.into() write=Callback::new(write)/>
            <div class="actions">
                <button class="btn primary" on:click=move |_| state.send(tree, row)>"Send"</button>
                {(!card_box).then(|| view! { <button class="btn subtle" on:click=move |_| state.cancel(row)>"Cancel"</button> })}
                <span class="keys">{hint}</span>
            </div>
        </div>
    }
    .into_any()
}
