//! A page of cards worked from the keyboard, as the Inbox, Questions, Decisions and Scopes pages
//! are. One card is
//! selected at a time (on the Inbox, one stop: a section's heading, a row of Waiting on you or a
//! card, top to bottom: `accordion.rs`), and the reply box of its row (the message it is about) has
//! the focus:
//! Enter sends and Shift+Enter starts a new line; ArrowUp on the box's first line (or in an empty
//! box) selects the card above and ArrowDown on its last line the card below; Escape leaves the
//! box for the card's row, from which the arrows walk the rows beneath it (`thread.rs`) and go on
//! to the card above or below at either end. With no text field and no row focused the arrows
//! select cards too. Shift pressed twice reads the selected card's message aloud; Shift twice
//! again, or any other key, stops the reading.

use crate::accordion::Section;
use crate::session::{self, ctx};
use crate::store::{self, Arrivals};
use crate::thread::{Hooks, ThreadState};
use crate::waiting::Sorted;
use crate::{keys, notification_card, speech, whose};
use ankurah::signals::{Get as AnkurahGet, Peek, SubscriptionGuard};
use ankurah::{EntityId, LiveQuery};
use futures::future::{select, Either};
use fwd_core::model::*;
use fwd_core::ops;
use fwd_core::query::with_id;
use leptos::prelude::*;
use serde_json::Value;
use std::collections::HashSet;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;

/// Two taps of Shift at most this many milliseconds apart read the selected message aloud.
const DOUBLE_TAP_MS: f64 = 400.0;

/// A card on a page: the notifications about one conversation, task or decision (the Inbox,
/// keyed by what they point into: `notification_card::place`) or a question (the Questions page).
/// The Decisions and Scopes pages are lists with a panel, not cards (Daniel, 2026-09-26). The
/// Inbox's selection also stops on each row of Waiting on you and on each section's heading.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Item {
    Thread(EntityId),
    Question(EntityId),
    /// A row of the Inbox's Waiting on you, by the entity it is about (`waiting.rs`).
    Request(EntityId),
    /// A section's heading on the Inbox (`accordion.rs`).
    Section(Section),
    /// A line of the Inbox's Settled before you looked, by what it goes by (`waiting::Fold::key`).
    Fold(EntityId),
}

impl Item {
    /// The card's element id, by which the selection finds the card to focus it and scroll to it.
    pub fn element_id(self) -> String {
        match self {
            Item::Thread(id) => format!("inbox-card-{}", id.to_base64()),
            Item::Question(id) => format!("question-{}", id.to_base64()),
            Item::Request(id) => format!("inbox-row-{}", id.to_base64()),
            Item::Section(section) => format!("inbox-section-{}", section.key()),
            Item::Fold(key) => format!("inbox-fold-{}", key.to_base64()),
        }
    }

    /// Whether it heads what it holds, a section's heading or a line of Settled before you
    /// looked, which the page selects first only when nothing else is there to select.
    fn is_heading(self) -> bool {
        matches!(self, Item::Section(_) | Item::Fold(_))
    }

    /// Whether it takes the keyboard itself when selected, rather than a reply box or a row inside
    /// it: a row of Waiting on you, whose moves and open thread are reached from it, a section's
    /// heading and a line of Settled before you looked.
    fn takes_the_keyboard(self) -> bool {
        matches!(self, Item::Request(_) | Item::Section(_) | Item::Fold(_))
    }
}

/// What the cards and handlers of one page share: `Copy` handles only, so a closure takes it whole.
#[derive(Clone, Copy)]
pub struct Page {
    /// The notifications behind the cards; none on the Questions page.
    pub notifications: Notifications,
    /// Every card, in page order.
    pub items: Memo<Vec<Item>>,
    pub selected: RwSignal<Option<Item>>,
    /// The rows of every card on the page: which one has the focus, which are open, the drafts.
    /// Their replies, resolutions and moves are the acting principal's (`thread.me`).
    pub thread: ThreadState,
    /// The principal whose notifications or questions the cards are: the acting principal, or
    /// another, shown to read without acting as them (`whose.rs`).
    pub whose: EntityId,
    /// How the page's writes marking notifications read or dismissed go, which the page shows.
    pub marks: RwSignal<Marks>,
    /// The Inbox's notifications sorted into the rows of Waiting on you and the conversations
    /// (`waiting.rs`); none on the Questions page.
    pub sorted: Option<Memo<Sorted>>,
}

/// The notifications behind a page's cards: one principal's inbox, as the page's live query of it
/// holds it, read whole or one notification at a time by its id; none on the Questions page.
#[derive(Clone, Copy)]
pub struct Notifications(Option<StoredValue<Inbox>>);

/// An inbox's live query, with the readers of each notification looked up in it by id.
struct Inbox {
    query: LiveQuery<NotificationView>,
    arrivals: Arrivals,
    _watching: SubscriptionGuard,
}

impl Notifications {
    pub fn none() -> Notifications {
        Notifications(None)
    }

    pub fn of(query: LiveQuery<NotificationView>) -> Notifications {
        let arrivals = Arrivals::default();
        let watching = arrivals.watch(&query);
        Notifications(Some(StoredValue::new(Inbox { query, arrivals, _watching: watching })))
    }

    /// Every one, read inside a closure, which then redraws when any of them arrives, leaves or
    /// changes.
    pub fn all(self) -> Vec<NotificationView> {
        self.0.and_then(|inbox| inbox.try_with_value(|inbox| inbox.query.get())).unwrap_or_default()
    }

    /// Every one, for what a key or a click does: reading them redraws nothing when they change.
    pub fn all_untracked(self) -> Vec<NotificationView> {
        self.0.and_then(|inbox| inbox.try_with_value(|inbox| inbox.query.peek())).unwrap_or_default()
    }

    /// The notification `id`, found by its id without copying the others: read inside a closure,
    /// which then redraws when it arrives or leaves, and, through the fields the closure reads,
    /// when it changes, but not when another notification does (`store::Arrivals`).
    pub fn get(self, id: EntityId) -> Option<NotificationView> {
        self.0.and_then(|inbox| inbox.try_with_value(|inbox| inbox.arrivals.member(&inbox.query, id))).flatten()
    }
}

/// How the page's writes marking notifications read or dismissed go: how many notifications Mark
/// all read's write marks while it is on its way, which its button says, and why the last write
/// failed, which shows beneath the heading until the next write starts. Until 2026-10-03 nothing
/// said that Mark all read was at work, and a failure went to the developer console alone (task
/// taRWSL2-).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Marks {
    /// How many Mark all read's write marks, while it is on its way.
    pub all: Option<usize>,
    /// Why the last write failed.
    pub failed: Option<String>,
}

impl Marks {
    /// A write of `n` notifications starts, Mark all read's when `all`.
    fn start(&mut self, n: usize, all: bool) {
        self.failed = None;
        if all {
            self.all = Some(n);
        }
    }

    /// The write of `n` notifications started with `all` ended, giving them `status` or failing.
    fn end(&mut self, n: usize, all: bool, status: NotificationStatus, outcome: Result<(), Failed>) {
        if all {
            self.all = None;
        }
        if let Err(failed) = outcome {
            self.failed = Some(failure(n, status, &failed));
        }
    }

    /// What Mark all read's button says.
    pub fn label(&self) -> String {
        match self.all {
            Some(n) => format!("Marking {n} read…"),
            None => "Mark all read".to_string(),
        }
    }
}

/// How long the page waits for a write marking notifications, and for the daemon's answer after
/// one that failed, before it gives Mark all read back and says that the daemon has not answered.
const MARK_BOUND_MS: i32 = 15_000;

/// How a write marking notifications fell short.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failed {
    /// It failed for `reason`, and the daemon then held `left` of its notifications short of the
    /// status written, or could not be asked (none) ([`settled`]).
    Erred { reason: String, left: Option<usize> },
    /// Neither the write nor the daemon's answer after it came within [`MARK_BOUND_MS`]: the
    /// daemon may still take it.
    Unanswered,
}

/// How a write that failed for `reason` ended, once the daemon has said how many of its
/// notifications it holds short of the status written (`left`, none when it could not be asked):
/// made after all when none, else failed.
fn settled(reason: String, left: Option<usize>) -> Result<(), Failed> {
    match left {
        Some(0) => Ok(()),
        left => Err(Failed::Erred { reason, left }),
    }
}

/// What the page says of a write that fell short of giving `n` notifications `status`.
fn failure(n: usize, status: NotificationStatus, failed: &Failed) -> String {
    let what = if n == 1 { "1 notification".to_string() } else { format!("{n} notifications") };
    let (reason, left) = match failed {
        Failed::Erred { reason, left } => (reason, *left),
        Failed::Unanswered => {
            let those = if n == 1 { "the notification".to_string() } else { format!("the {n} notifications") };
            let marked = match status {
                NotificationStatus::Dismissed => "dismissed".to_string(),
                status => format!("marked {status}"),
            };
            return format!(
                "The daemon has not answered in {} s; {those} may still be {marked}.",
                MARK_BOUND_MS / 1000
            );
        }
    };
    let said = match status {
        NotificationStatus::Dismissed => format!("Dismissing {what} failed: {reason}"),
        status => format!("Marking {what} {status} failed: {reason}"),
    };
    match (left, status) {
        (None, _) => said,
        (Some(left), NotificationStatus::Read) => format!("{said}; {left} still unread"),
        (Some(left), status) => format!("{said}; {left} still not {status}"),
    }
}

impl Page {
    /// Whether the page's cards act: they do on the acting principal's own cards, and on another
    /// principal's, shown to read, they neither reply nor mark anything read (`whose::acts`).
    pub fn acts(self) -> bool {
        whose::acts(self.thread.me, self.whose)
    }

    /// Every notification on the inbox card `key`, read or not, newest first: a batch that grew
    /// counts from when its last item joined. A notification in a row of Waiting on you is the
    /// row's, not the card's, so a card about the same task marks and dismisses only its own.
    /// Reading them redraws nothing when they change.
    pub fn on_card(self, key: EntityId) -> Vec<NotificationView> {
        let conversations: Option<HashSet<EntityId>> =
            self.sorted.map(|sorted| sorted.with_untracked(|s| s.conversations.iter().copied().collect()));
        let mut found: Vec<NotificationView> = untrack(|| {
            self.notifications
                .all_untracked()
                .into_iter()
                .filter(|n| conversations.as_ref().is_none_or(|ids| ids.contains(&n.id())))
                .filter(|n| notification_card::place(n).key == key)
                .collect()
        });
        found.sort_by_key(|n| std::cmp::Reverse(ops::notification_time(n)));
        found
    }

    /// Every notification of the row of Waiting on you about `subject`, newest first. Reading
    /// them redraws nothing when they change.
    pub fn on_row(self, subject: EntityId) -> Vec<NotificationView> {
        let members = self
            .sorted
            .and_then(|sorted| sorted.with_untracked(|s| s.row(subject).map(|r| r.members.clone())))
            .unwrap_or_default();
        untrack(|| members.into_iter().filter_map(|id| self.notifications.get(id)).collect())
    }

    /// Gives the notifications `ids` the status `status` in one write, each as the page holds it,
    /// fetched first only where the page holds none (no card names a notification outside its
    /// inbox today), so that marking an inbox of 212 notifications makes no round trip to the
    /// daemon per notification (`ops::set_status_of`; task taRWSL2-). `all` names Mark all read's
    /// write, whose button says how many it marks while it is on its way; a write that fails says
    /// why beneath the heading ([`Marks`]).
    pub fn mark(self, ids: Vec<EntityId>, status: NotificationStatus, all: bool) {
        if ids.is_empty() {
            return;
        }
        let n = ids.len();
        let (mut held, mut missing) = (Vec::with_capacity(n), Vec::new());
        for id in &ids {
            match self.notifications.get(*id) {
                Some(notification) => held.push(notification),
                None => missing.push(*id),
            }
        }
        self.marks.update(|marks| marks.start(n, all));
        spawn_local(async move {
            let marked = async move {
                match write(held, &missing, status).await {
                    Ok(()) => Ok(()),
                    // The daemon may have taken the write all the same. Ankurah's commit (at
                    // f8e85d2, core/src/transaction/commit.rs) sends the transaction to the daemon,
                    // then writes it to the page's own store, retrying on a conflict; the daemon's
                    // echo of the same changes to the page's live queries lands there meanwhile, and
                    // the retries run out: marking 220 notifications read failed with "TOCTOU
                    // attempts exhausted" every time while the daemon held all 220 read (task
                    // kH7MFKfH). So the page asks the daemon once whether it holds them marked, and
                    // says the write failed only when it does not.
                    Err(e) => {
                        let left = unmarked(self.whose, &ids, status).await;
                        if left == Some(0) {
                            tracing::warn!(
                                "marking {n} notifications {status} failed here ({e}), but the daemon holds them so"
                            );
                        }
                        settled(e.to_string(), left)
                    }
                }
            };
            // A daemon that does not answer leaves the write waiting for as long as the page is
            // open, so the page stops waiting after a while and says so.
            let outcome = match select(Box::pin(marked), Box::pin(session::sleep_ms(MARK_BOUND_MS))).await {
                Either::Left((outcome, _)) => outcome,
                Either::Right(((), _)) => Err(Failed::Unanswered),
            };
            if let Err(failed) = &outcome {
                tracing::error!("notification status: {failed:?}");
            }
            self.marks.try_update(|marks| marks.end(n, all, status, outcome));
        });
    }

    fn is_unread(self, item: Item) -> bool {
        let unread = |n: &NotificationView| n.status().ok() == Some(NotificationStatus::Unread);
        match item {
            Item::Thread(key) => self.on_card(key).iter().any(unread),
            Item::Request(subject) => self.on_row(subject).iter().any(unread),
            Item::Question(_) | Item::Section(_) | Item::Fold(_) => false,
        }
    }

    /// Selects `item`, or gives its reply box the focus again when it is already selected.
    pub fn select(self, item: Item) {
        if self.selected.get_untracked() == Some(item) {
            request_animation_frame(move || focus(item, true));
        } else {
            self.selected.set(Some(item));
        }
    }

    /// Selects the card `delta` places from the selected one (the first card going down, or the
    /// last going up, when none is selected). False when there is no such card, so the key keeps
    /// its usual meaning.
    fn step(self, delta: isize) -> bool {
        self.step_from(self.selected.get_untracked(), delta)
    }

    /// Selects the card `delta` places from `from`, as `step` does from the selected card.
    pub fn step_from(self, from: Option<Item>, delta: isize) -> bool {
        let items = self.items.get_untracked();
        let at = from.and_then(|from| items.iter().position(|i| *i == from));
        let next = match at {
            Some(at) => at.checked_add_signed(delta),
            None if delta > 0 => Some(0),
            None => items.len().checked_sub(1),
        };
        match next.and_then(|i| items.get(i)) {
            Some(item) => {
                self.selected.set(Some(*item));
                true
            }
            None => false,
        }
    }

    /// What the selected card's message says, read aloud: its author and priority, then the
    /// text. An inbox card says why its newest notification is there, then the message behind it.
    fn spoken(self) -> Option<String> {
        let message = match self.selected.get_untracked()? {
            Item::Question(id) => store::message(id)?,
            Item::Thread(key) => {
                let newest = self.on_card(key).into_iter().next()?;
                let why = notification_card::sentence(self.whose, &newest);
                let originator: Value = newest.originator().ok()?.into();
                let text = id_at(&originator["message_id"])
                    .and_then(store::message)
                    .map(|m| ops::text(m.content()).unwrap_or_default())
                    .unwrap_or_default();
                return Some(format!("{why}. {text}"));
            }
            // A row says why its newest request is there, as its hover does, and a question's row
            // the question too.
            Item::Request(subject) => {
                let sorted = self.sorted?;
                let newest = sorted.with_untracked(|s| s.row(subject).and_then(|r| r.newest_request()))?;
                let newest = untrack(|| self.notifications.get(newest))?;
                let why = notification_card::sentence(self.whose, &newest);
                let text = store::message(subject).map(|m| ops::text(m.content()).unwrap_or_default());
                return Some(match text {
                    Some(text) => format!("{why}. {text}"),
                    None => why,
                });
            }
            Item::Section(_) | Item::Fold(_) => return None,
        };
        let author = message.author().ok().map(|a| store::handle_of(EntityId::from(a))).unwrap_or_default();
        let priority = message.priority().map(|p| p.to_string()).unwrap_or_default();
        Some(format!("{author}, priority {priority}. {}", ops::text(message.content()).unwrap_or_default()))
    }

    /// What a card's rows and the page tell each other. A reply sent from an inbox card, or a move
    /// made on its own row, marks every unread notification of the card read, as `fwd reply` marks
    /// the one it answers; `notification`, when given, is marked read too. On another principal's
    /// cards, shown to read, the card has no reply box of its own and nothing marks anything read
    /// ([`offers`]); the reply button and Enter on the card's own row then open a box beneath that
    /// row, as on any other row, whose reply is the acting principal's.
    pub fn hooks(self, item: Item, notification: Option<EntityId>) -> Hooks {
        let selected = Memo::new(move |_| self.selected.get() == Some(item));
        let offered = offers(self.acts(), item, notification);
        let card = match item {
            Item::Thread(key) => Some(key),
            _ => None,
        };
        let sent = offered.marks_read.then(|| {
            Callback::new(move |()| {
                let mut ids: Vec<EntityId> = card
                    .map(|key| self.on_card(key))
                    .unwrap_or_default()
                    .iter()
                    .filter(|n| n.status().ok() == Some(NotificationStatus::Unread))
                    .map(|n| n.id())
                    .collect();
                ids.extend(notification.filter(|n| !ids.contains(n)));
                self.mark(ids, NotificationStatus::Read, false);
            })
        });
        Hooks {
            // A row taking the focus selects its card, and the card leaves the focus on the row.
            entered: Some(Callback::new(move |()| {
                if self.selected.get_untracked() != Some(item) {
                    self.selected.set(Some(item));
                }
            })),
            reply_box: offered.reply_box.then(|| selected.into()),
            reply: offered.reply_box.then(|| Callback::new(move |()| self.select(item))),
            step: Some(Callback::new(move |delta| self.step_from(Some(item), delta))),
            sent,
            task_fields: None,
            strip_above: false,
            beneath: None,
        }
    }
}

/// What a card's hooks offer besides selecting the card and moving between cards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Offers {
    /// The card's own reply box, shown while the card is selected, which then has the keyboard.
    reply_box: bool,
    /// A reply sent from the card, or a move made on its own row, marks read what the card was
    /// notified of: its unread notifications, and the notification its hooks name.
    marks_read: bool,
}

/// What `item`'s hooks offer, on a page whose cards act or not (`Page::acts`): on the acting
/// principal's own cards, the reply box, and marking read on a card that holds notifications (an
/// inbox card, or one whose hooks name a notification); on another principal's, shown to read,
/// neither, so the page, which acts as its own principal, never replies from their cards or marks
/// their notifications read. A question's card holds no notifications, so a reply from it marks
/// nothing read on either.
fn offers(acts: bool, item: Item, notification: Option<EntityId>) -> Offers {
    let notified = matches!(item, Item::Thread(_)) || notification.is_some();
    Offers { reply_box: acts, marks_read: acts && notified }
}

/// Gives the notifications `held`, and those `missing` once fetched, `status` in one transaction
/// ([`Page::mark`]).
async fn write(
    mut held: Vec<NotificationView>,
    missing: &[EntityId],
    status: NotificationStatus,
) -> anyhow::Result<()> {
    let ctx = ctx();
    for id in missing {
        held.push(ctx.get(*id).await?);
    }
    ops::set_status_of(&ctx, &held, status).await
}

/// How many of the notifications `ids`, in `whose`'s inbox, the daemon holds short of `status`
/// ([`short_of`]), asked once; none when it cannot be asked.
async fn unmarked(whose: EntityId, ids: &[EntityId], status: NotificationStatus) -> Option<usize> {
    let selection = with_id(short_of(status), whose).ok()?;
    let short: Vec<NotificationView> = ctx().fetch(selection).await.ok()?;
    Some(short.iter().filter(|n| ids.contains(&n.id())).count())
}

/// The daemon's selection of a principal's notifications that a write giving them `status` has
/// not reached. Marking read, it selects the unread alone: a notification dismissed meanwhile,
/// from another client, is not still unread, and the dismissed ones, every one the principal has
/// dismissed, would come with each answer. Dismissing, it selects every one not dismissed.
fn short_of(status: NotificationStatus) -> &'static str {
    match status {
        NotificationStatus::Read => "recipient = ? AND status = 'unread'",
        NotificationStatus::Dismissed => "recipient = ? AND status != 'dismissed'",
        NotificationStatus::Unread => "recipient = ? AND status != 'unread'",
    }
}

/// The entity id a JSON value holds as text (a notification's `target.id`, say).
pub fn id_at(value: &Value) -> Option<EntityId> {
    value.as_str().and_then(|s| s.parse::<EntityId>().ok())
}

/// Keeps one card selected: at first the first unread card (else the first card), once the daemon
/// has `answered` the queries the cards are made from; and when the selected card leaves the page
/// (dismissed, resolved, or changed through another client), the card that took its place, else
/// the last (`replacement`).
pub fn keep_one_selected(page: Page, answered: Signal<bool>) {
    let last_index = StoredValue::new(0usize);
    Effect::new(move |_| {
        let items = page.items.get();
        let current = page.selected.get();
        if let Some(at) = current.and_then(|c| items.iter().position(|i| *i == c)) {
            last_index.set_value(at);
            return;
        }
        let next = replacement(
            &items,
            current,
            last_index.get_value(),
            answered.get(),
            |i| page.is_unread(i),
            Item::is_heading,
        );
        if next != current {
            page.selected.set(next);
        }
    });
}

/// The card to select when the selected card, if any, is not among `items`, the cards on the page
/// in order. Until the daemon has `answered`, the cards so far are whichever arrived first, so
/// none is chosen from them, and a selected card stays: it may not have arrived yet, as when the
/// route names it. Once the daemon has answered, the first unread card is chosen (else the first
/// card that is no `heading`, else the first) when none is selected, and a selected card that left
/// gives way to the card that took its place, at `at`, where it was (else the last).
fn replacement<T: Copy>(
    items: &[T],
    selected: Option<T>,
    at: usize,
    answered: bool,
    unread: impl Fn(T) -> bool,
    heading: impl Fn(T) -> bool,
) -> Option<T> {
    if !answered {
        return selected;
    }
    match selected {
        Some(_) => items.get(at.min(items.len().saturating_sub(1))).copied(),
        None => items
            .iter()
            .copied()
            .find(|i| unread(*i))
            .or_else(|| items.iter().copied().find(|i| !heading(*i)))
            .or(items.first().copied()),
    }
}

/// Whenever the selection changes, the selected card takes the keyboard once it is drawn.
pub fn follow_selection(page: Page) {
    Effect::new(move |_| {
        if let Some(item) = page.selected.get() {
            request_animation_frame(move || focus(item, false));
        }
    });
}

/// Gives the keyboard to a card: to its reply box (caret at the end) when it has one, else to its
/// row (a card about a task), else to the card itself, and scrolls the card into view. A row of
/// Waiting on you and a section's heading take it themselves, unless it is already inside them (on
/// a move, in a box for a reason or a reply). The focus stays where it is while the person types
/// in another text field (the Ask box, a picker), so a card selected because something arrived
/// never takes the keyboard from them, and, unless `insist` (the card's reply button), while it is
/// already among the card's own rows.
fn focus(item: Item, insist: bool) {
    let Some(document) = web_sys::window().and_then(|w| w.document()) else { return };
    let Some(card) = document.get_element_by_id(&item.element_id()) else { return };
    let active = document.active_element();
    let in_rows = active
        .as_ref()
        .is_some_and(|a| card.contains(Some(a.as_ref())) && a.closest("[role=tree]").ok().flatten().is_some());
    if in_rows && !insist {
        return;
    }
    let options = web_sys::ScrollIntoViewOptions::new();
    options.set_block(web_sys::ScrollLogicalPosition::Nearest);
    card.scroll_into_view_with_scroll_into_view_options(&options);
    if active.as_ref().is_some_and(|a| keys::is_text_field(a) && !card.contains(Some(a.as_ref()))) {
        return;
    }
    if item.takes_the_keyboard() {
        let inside = active.is_some_and(|a| card.contains(Some(a.as_ref())) && !a.is_same_node(Some(card.as_ref())));
        if !inside || insist {
            if let Ok(card) = card.dyn_into::<web_sys::HtmlElement>() {
                let _ = card.focus();
            }
        }
        return;
    }
    let reply = card
        .query_selector("[data-card-reply]")
        .ok()
        .flatten()
        .and_then(|e| e.dyn_into::<web_sys::HtmlTextAreaElement>().ok());
    match reply {
        Some(reply) => {
            let _ = reply.focus();
            let end = reply.text_length();
            let _ = reply.set_selection_range(end, end);
        }
        None => {
            let row = card.query_selector("[role=treeitem]").ok().flatten().unwrap_or(card);
            if let Ok(row) = row.dyn_into::<web_sys::HtmlElement>() {
                let _ = row.focus();
            }
        }
    }
}

/// The page's keys outside its rows and reply boxes, which handle their own: with no text field
/// focused the arrows select cards, Escape leaves a text field, and Shift tapped twice reads the
/// selected card aloud. Any other key stops the reading. The listeners go when the page does.
pub fn listen_for_keys(page: Page) {
    // A press of Shift is a tap when no other key went down while it was held.
    let tapping = StoredValue::new(false);
    let last_tap = StoredValue::new(f64::NEG_INFINITY);
    let keydown = window_event_listener(leptos::ev::keydown, move |ev| {
        if ev.key() == "Shift" {
            tapping.set_value(!ev.repeat());
            return;
        }
        tapping.set_value(false);
        last_tap.set_value(f64::NEG_INFINITY);
        if speech::speaking() {
            speech::stop();
        }
        // A row or a reply box beneath one already used the key.
        if ev.default_prevented() {
            return;
        }
        let field = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.active_element())
            .filter(keys::is_text_field)
            .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok());
        let delta = match ev.key().as_str() {
            "Escape" => {
                if let Some(field) = field {
                    let _ = field.blur();
                }
                return;
            }
            "ArrowUp" => -1,
            "ArrowDown" => 1,
            _ => return,
        };
        if field.is_none() && keys::is_plain(&ev) && page.step(delta) {
            ev.prevent_default();
        }
    });
    let keyup = window_event_listener(leptos::ev::keyup, move |ev| {
        if ev.key() != "Shift" || !tapping.get_value() {
            return;
        }
        tapping.set_value(false);
        let now = js_sys::Date::now();
        if now - last_tap.get_value() > DOUBLE_TAP_MS {
            last_tap.set_value(now);
            return;
        }
        last_tap.set_value(f64::NEG_INFINITY);
        if speech::speaking() {
            speech::stop();
        } else if let Some(text) = page.spoken() {
            speech::speak(&text);
        }
    });
    on_cleanup(move || {
        keydown.remove();
        keyup.remove();
        speech::stop();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use ankurah::ankql::ast::{Parsed, PathExpr};
    use ankurah::core::selection::filter::{evaluate_predicate, ValueLookup};

    #[test]
    fn mark_all_read_says_how_many_it_marks_while_it_writes_and_a_failed_write_says_why() {
        let failed = |reason: &str, left| Failed::Erred { reason: reason.into(), left };
        let mut marks = Marks::default();
        assert_eq!(marks.label(), "Mark all read");
        // Mark all read's write of 212 starts: its button says how many, and is disabled while
        // the write is on its way.
        marks.start(212, true);
        assert_eq!((marks.label().as_str(), marks.all), ("Marking 212 read…", Some(212)));
        // The write fails, and the daemon cannot be asked how it holds them: the button is back,
        // and the page says why.
        marks.end(212, true, NotificationStatus::Read, Err(failed("the daemon is out of reach", None)));
        assert_eq!((marks.label().as_str(), marks.all), ("Mark all read", None));
        assert_eq!(marks.failed.as_deref(), Some("Marking 212 notifications read failed: the daemon is out of reach"));
        // Trying again takes the failure away; this time the daemon holds 30 of them unread.
        marks.start(212, true);
        assert_eq!(marks.failed, None);
        marks.end(212, true, NotificationStatus::Read, Err(failed("TOCTOU attempts exhausted", Some(30))));
        assert_eq!(
            marks.failed.as_deref(),
            Some("Marking 212 notifications read failed: TOCTOU attempts exhausted; 30 still unread")
        );
        // A write that succeeds leaves nothing to say.
        marks.start(212, true);
        marks.end(212, true, NotificationStatus::Read, Ok(()));
        assert_eq!(marks, Marks::default());
        // A card's write leaves Mark all read's button as it was, and says why it failed too.
        marks.start(1, false);
        assert_eq!(marks.all, None);
        marks.end(1, false, NotificationStatus::Dismissed, Err(failed("refused", Some(1))));
        assert_eq!(marks.failed.as_deref(), Some("Dismissing 1 notification failed: refused; 1 still not dismissed"));
    }

    #[test]
    fn a_write_the_daemon_has_not_answered_in_time_gives_the_button_back_and_may_still_land() {
        let mut marks = Marks::default();
        marks.start(218, true);
        marks.end(218, true, NotificationStatus::Read, Err(Failed::Unanswered));
        assert_eq!((marks.label().as_str(), marks.all), ("Mark all read", None), "the button is back");
        assert_eq!(
            marks.failed.as_deref(),
            Some("The daemon has not answered in 15 s; the 218 notifications may still be marked read.")
        );
        marks.start(1, false);
        marks.end(1, false, NotificationStatus::Dismissed, Err(Failed::Unanswered));
        assert_eq!(
            marks.failed.as_deref(),
            Some("The daemon has not answered in 15 s; the notification may still be dismissed.")
        );
    }

    #[test]
    fn a_failed_write_the_daemon_holds_made_is_made() {
        // Ankurah's commit can fail on the page after the daemon took the write (task kH7MFKfH):
        // the daemon holding none of them at another status, the write was made.
        assert_eq!(settled("TOCTOU attempts exhausted".into(), Some(0)), Ok(()));
        let failed = |left| Failed::Erred { reason: "TOCTOU attempts exhausted".into(), left };
        assert_eq!(settled("TOCTOU attempts exhausted".into(), Some(3)), Err(failed(Some(3))));
        assert_eq!(settled("TOCTOU attempts exhausted".into(), None), Err(failed(None)), "the daemon could not say");
    }

    /// A notification as the daemon's selection reads it: whose it is, and its status.
    struct Stored {
        recipient: EntityId,
        status: NotificationStatus,
    }

    impl ValueLookup<Parsed> for Stored {
        fn value_at(&self, path: &PathExpr) -> Option<ankurah::Value> {
            match path.property() {
                "recipient" => Some(ankurah::Value::EntityId(self.recipient)),
                "status" => Some(ankurah::Value::String(self.status.to_string())),
                _ => None,
            }
        }
    }

    #[test]
    fn after_a_failed_write_the_daemon_is_asked_for_the_notifications_it_has_not_reached() {
        use NotificationStatus::{Dismissed, Read, Unread};
        let (me, other) = (EntityId::from_bytes([1; 32]), EntityId::from_bytes([2; 32]));
        let selects = |writing: NotificationStatus, recipient: EntityId, status: NotificationStatus| {
            let selection = with_id(short_of(writing), me).expect("the selection parses");
            evaluate_predicate(&Stored { recipient, status }, &selection.predicate).expect("it evaluates")
        };
        // Marking read: the unread alone, so neither a notification dismissed meanwhile from
        // another client nor every one dismissed before comes back as still unread.
        assert!(selects(Read, me, Unread));
        assert!(!selects(Read, me, Read));
        assert!(!selects(Read, me, Dismissed));
        assert!(!selects(Read, other, Unread), "another principal's");
        // Dismissing: every one not dismissed.
        assert!(selects(Dismissed, me, Unread));
        assert!(selects(Dismissed, me, Read));
        assert!(!selects(Dismissed, me, Dismissed));
    }

    #[test]
    fn another_principals_cards_offer_no_reply_box_and_mark_nothing_read() {
        let id = |n: u8| EntityId::from_bytes([n; 32]);
        let (card, question) = (Item::Thread(id(1)), Item::Question(id(2)));
        // One's own: an inbox card has its reply box, and a reply from it marks the card read; a
        // question's card has its reply box and nothing to mark read, unless its hooks name a
        // notification.
        assert_eq!(offers(true, card, None), Offers { reply_box: true, marks_read: true });
        assert_eq!(offers(true, question, None), Offers { reply_box: true, marks_read: false });
        assert_eq!(offers(true, question, Some(id(3))), Offers { reply_box: true, marks_read: true });
        // Another principal's, shown to read: neither, whatever the card.
        for (item, notification) in [(card, None), (card, Some(id(3))), (question, None), (question, Some(id(3)))] {
            assert_eq!(offers(false, item, notification), Offers { reply_box: false, marks_read: false }, "{item:?}");
        }
    }

    #[test]
    fn no_card_is_chosen_before_the_daemon_has_answered_and_then_the_first_unread_one() {
        // Cards 1 to 4 on the page, in order, 3 and 4 unread.
        let (items, unread) = ([1, 2, 3, 4], |card: u8| card >= 3);
        let none = |_: u8| false;
        assert_eq!(
            replacement(&items, None, 0, false, unread, none),
            None,
            "the cards so far are whichever arrived first"
        );
        assert_eq!(replacement(&items, None, 0, true, unread, none), Some(3), "the first unread card");
        assert_eq!(replacement(&items, None, 0, true, |_| false, none), Some(1), "else the first card");
        assert_eq!(replacement(&[], None, 0, true, unread, none), None, "no card to choose");
        // On the Inbox, 1 and 2 are headings: with nothing unread, the first stop that is not one.
        let headings = |card: u8| card <= 2;
        assert_eq!(replacement(&items, None, 0, true, |_| false, headings), Some(3), "the first row or card");
        assert_eq!(replacement(&[1, 2], None, 0, true, |_| false, headings), Some(1), "else the first heading");
    }

    #[test]
    fn a_selected_card_missing_from_the_page_stays_until_the_daemon_has_answered() {
        // The route named card 9, which has not arrived.
        let none = |_: u8| false;
        assert_eq!(replacement(&[1, 2], Some(9), 0, false, |_| true, none), Some(9));
        // Once the daemon has answered, a card that left gives way to the card that took its
        // place, else the last, else none.
        assert_eq!(replacement(&[1, 2, 3], Some(9), 1, true, |_| true, none), Some(2));
        assert_eq!(replacement(&[1, 2, 3], Some(9), 5, true, |_| true, none), Some(3));
        assert_eq!(replacement(&[], Some(9), 0, true, |_| true, none), None);
    }
}
