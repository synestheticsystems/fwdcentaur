//! The inbox page: the Ask box, where questions are recorded, then the principal's notifications
//! in two sections stacked one above the other, each opening and closing on its own as an
//! accordion, which this browser remembers (D137; `accordion.rs`). First Waiting on you: one row
//! per thing that asks the reader for a move (a question asked of them, a ruling proposed, a
//! review, a proposed task, a task assigned to them or now ready), in a group per kind of move,
//! each group a section of its own (`waiting.rs`, `request_row.rs`); a row stays until its move is
//! made or it is dismissed, read or not, and its move is on the row. Then the conversations, as
//! cards, one card per conversation, task or decision they point into (`notification_card.rs`),
//! newest first by each card's newest notification: the unread ones by default, every one but the
//! dismissed with "unread only" off (dismissed ones are gone, as in `fwd inbox`). A request whose
//! subject stopped waiting is a conversation again, so nothing unread leaves without a word. Each
//! card says in a line per notification why it is here, over the row of what they point into, with
//! the conversation about that beneath it; a notification collecting what one author did on one
//! task lists its steps and comments beneath its line, and a card is as new as the last item that
//! joined it. One selection runs down the page, through each section's heading, the rows and the
//! cards, worked from the keyboard as `cards.rs` describes; the badge in the header counts the
//! rows waiting on the acting principal (`app.rs`).
//!
//! The selector beside the heading shows one other principal's inbox instead, which the route
//! names (`#/inbox?for=<handle>`), to read without acting as them (`whose.rs`): nothing on it then
//! marks their notifications read or dismisses them, the cards have no reply box of their own, and
//! a move made from a row is the acting principal's, where the rules let them make it.

use crate::accordion::{self, Closed, RowStep, Section};
use crate::cards::{self, Item, Marks, Notifications, Page};
use crate::empty::{self, Empty, Nothing};
use crate::lozenges::{self, Target};
use crate::notification_card::{self, Card, NotificationCard};
use crate::page_state;
use crate::referents::Models;
use crate::request_row::RequestRow;
use crate::route::{self, Route};
use crate::scope_filter::ScopeChoice;
use crate::session::{self, ctx};
use crate::store;
use crate::thread::{self, Hooks, Model, OpenQueries, ThreadState};
use crate::waiting::{self, Batch, Fold, Group, Keep, Kind, Sorted, Stood};
use crate::whose::{self, Found, WhosePicker};
use crate::{ask, decision_row, keys, message_row, task_row};
use ankurah::EntityId;
use fwd_core::model::*;
use fwd_core::query::with_id;
use leptos::prelude::*;
use serde_json::Value;
use std::collections::HashSet;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;

/// What the selector says on hover.
const HINT: &str = "Whose inbox this page shows: your own, or another principal's, to read without acting as them";

/// What Mark all read says on hover until the daemon has answered the inbox's query.
const LOADING_HINT: &str = "loading: Mark all read waits until the page holds the whole inbox";

/// The Inbox of the principal `whose` names, from the route (`whose.rs`), or of the acting
/// principal when it names none; `scope` is the header's scope choice, which the Ask box asks by
/// default (`ask.rs`).
#[component]
pub fn Inbox(whose: Memo<Option<String>>, scope: Memo<Option<ScopeChoice>>) -> impl IntoView {
    let found = whose::found(whose);
    let choose = Callback::new(|whose| route::go(Route::Inbox { whose }));
    move || {
        let me = session::acting().get()?.id();
        Some(match found.get() {
            Found::Principal(whose) => view! { <InboxFor me=me whose=whose scope=scope choose=choose/> }.into_any(),
            other => whose::no_one_to_show(other, "Inbox", me, HINT, choose),
        })
    }
}

/// Opens a section, or closes it: (the section, whether it is to be open).
type Toggle = Callback<(Section, bool)>;

/// `whose`'s inbox, on a page acting as `me`, under the Ask box, which asks `scope` by default.
#[component]
fn InboxFor(
    me: EntityId,
    whose: EntityId,
    scope: Memo<Option<ScopeChoice>>,
    choose: Callback<Option<String>>,
) -> impl IntoView {
    // A dismissed notification is gone from the page, so the query leaves it out rather than the
    // page holding and grouping every notification the principal was ever sent.
    let selection = with_id("recipient = ? AND status != 'dismissed'", whose).expect("static selection");
    let query = match ctx().query::<NotificationView>(selection) {
        Ok(q) => q,
        Err(e) => return view! { <p class="error">{format!("the inbox query was refused: {e}")}</p> }.into_any(),
    };
    // The first selection waits for the daemon's answer to the notifications, and to the messages
    // that say which conversation each points into (`store::standing_loaded`), so it is the first
    // unread card rather than whichever card happened to arrive first: each query first answers
    // from this load's own browser storage, which starts empty, and the daemon's answer then
    // arrives in small batches. Until then the page says it is loading rather than that nothing
    // waits.
    let answered = RwSignal::new(false);
    spawn_local({
        let query = query.clone();
        async move {
            if let Err(e) = query.wait_durable_answered().await {
                tracing::warn!("the inbox query did not load: {e}");
            }
            answered.set(true);
        }
    });
    let notifications = Notifications::of(query);
    // The selection and "unread only" as the person left them, when Back brought the page back to
    // this inbox (`page_state.rs`); else the page starts over, with unread cards only and nothing
    // selected until the daemon has answered. The page keeps them there as they change.
    let left = page_state::open_inbox(whose);
    let back_to = left.as_ref().and_then(|left| left.selected);
    let unread_only = RwSignal::new(left.as_ref().is_none_or(|left| left.unread_only));
    let selected = RwSignal::new(back_to);
    // The notifications the selected card showed, kept on it while it stays selected: with
    // "unread only", reading or replying to it neither pulls it away nor changes its lines. It
    // leaves when the selection moves on.
    let kept = StoredValue::new(left.map_or_else(|| (None, HashSet::new()), |left| (left.selected, left.kept)));
    // The messages the reader has answered, which a question asked of them leaves Waiting on you by,
    // from the messages they wrote. The first selection waits for them too: before they arrive, a
    // question the reader answered still waits, and a row the selection lands on stays, as though a
    // move had been made from it, until the selection moves on.
    let (answered_by, answers_loaded) = waiting::answers(whose);
    // The selected row, kept after its move until the selection moves on, where it stood while it
    // waited, which the page notes each time it sorts (`waiting::Keep`).
    let keep = Memo::new(move |_| match selected.get() {
        Some(Item::Request(subject)) => Some(subject),
        _ => None,
    });
    let stood = StoredValue::new(None::<(EntityId, Stood)>);
    let sorted = Memo::new(move |_| {
        let subject = keep.get();
        let keep = subject.map(|subject| Keep {
            subject,
            stood: stood.with_value(|s| s.filter(|(id, _)| *id == subject).map(|(_, stood)| stood)),
        });
        let sorted = answered_by.with(|answered| waiting::sorted_now(&notifications.all(), whose, answered, keep));
        if let Some(row) = subject.and_then(|subject| sorted.row(subject)).filter(|row| row.waits) {
            stood.set_value(Some((row.subject, row.stood())));
        }
        sorted
    });
    let cards = Memo::new(move |_| {
        let (only_unread, selected) = (unread_only.get(), selected.get());
        let keep: HashSet<EntityId> =
            kept.with_value(|(item, ids)| if *item == selected { ids.clone() } else { HashSet::new() });
        let conversations: HashSet<EntityId> = sorted.with(|s| s.conversations.iter().copied().collect());
        let cards = notification_card::cards(&notifications.all(), |n| {
            let status = n.status().ok();
            conversations.contains(&n.id())
                && status != Some(NotificationStatus::Dismissed)
                && (!only_unread || status == Some(NotificationStatus::Unread) || keep.contains(&n.id()))
        });
        if let Some(Item::Thread(key)) = selected {
            if let Some(card) = cards.iter().find(|c| c.key == key) {
                kept.set_value((selected, card.members.iter().copied().collect()));
            }
        }
        cards
    });
    // Which sections are closed, as this browser remembers them; which groups show every row past
    // the first eight; which rows are open.
    let closed = RwSignal::new(accordion::remembered());
    let expanded = RwSignal::new(HashSet::<Kind>::new());
    let open_rows = RwSignal::new(HashSet::<EntityId>::new());
    // Which lines of Settled before you looked show their notifications.
    let open_folds = RwSignal::new(HashSet::<EntityId>::new());
    // The stops of the one selection, top to bottom (`accordion::stops`).
    let items = Memo::new(move |_| {
        let keys: Vec<EntityId> = cards.with(|cards| cards.iter().map(|c| c.key).collect());
        sorted.with(|sorted| {
            closed.with(|closed| expanded.with(|expanded| accordion::stops(sorted, closed, expanded, &keys)))
        })
    });
    let marks = RwSignal::new(Marks::default());
    let page =
        Page { notifications, items, selected, thread: ThreadState::new(me), whose, marks, sorted: Some(sorted) };
    let loaded = Signal::derive(move || answered.get() && store::standing_loaded() && answers_loaded.get());
    cards::keep_one_selected(page, loaded);
    cards::follow_selection(page);
    cards::listen_for_keys(page);
    // The selection, the filter and what the selected card shows, kept for Back to bring back.
    Effect::new(move |_| {
        let (unread_only, selected) = (unread_only.get(), selected.get());
        // After the cards, which note what the selected card shows.
        cards.track();
        let kept = kept.with_value(|(item, ids)| if *item == selected { ids.clone() } else { HashSet::new() });
        page_state::keep_inbox(page_state::InboxState { whose, unread_only, selected, kept });
    });
    // Brought back by Back, the page gives the stop that was selected the keyboard, scrolled into
    // view, once the daemon has answered, as it does the first one it selects: until then it may
    // not be on the page to take it. One that has left the page meanwhile (dismissed, say) gives
    // way to the stop a visit would select first.
    if let Some(card) = back_to {
        Effect::new(move |answered_now: Option<bool>| {
            if answered_now == Some(true) {
                return true;
            }
            if !loaded.get() {
                return false;
            }
            if !items.with(|items| items.contains(&card)) {
                selected.set(None);
            } else if selected.get_untracked() == Some(card) {
                page.select(card);
            }
            true
        });
    }
    // Opening or closing a section, which this browser remembers. Closing the section the
    // selection stands in moves the selection to its heading, so it never rests on what is hidden.
    let toggle: Toggle = Callback::new(move |(section, open): (Section, bool)| {
        closed.update(|closed| closed.set(section, open));
        closed.with_untracked(accordion::remember);
        let hidden = selected
            .get_untracked()
            .is_some_and(|item| sorted.with_untracked(|sorted| accordion::within(item, section, sorted)));
        if !open && hidden {
            selected.set(Some(Item::Section(section)));
        }
    });

    // Mark all read marks the unread notifications the page holds, rows and cards alike, in one
    // write, so it waits until the daemon has answered the inbox's query, and while its write is on
    // its way it is disabled and says how many it marks; a write that fails says why beneath the
    // heading (task taRWSL2-). The rows stay: reading one only clears its new mark.
    let mark_all_read = move |_| {
        let ids: Vec<EntityId> = notifications
            .all_untracked()
            .iter()
            .filter(|n| n.status().ok() == Some(NotificationStatus::Unread))
            .map(|n| n.id())
            .collect();
        page.mark(ids, NotificationStatus::Read, true);
    };
    let marking = move || marks.with(|marks| marks.all.is_some());
    // Another principal's inbox, shown to read, says so beneath the heading, and Mark all read
    // stays disabled and says why on hover.
    let acts = page.acts();
    let reading = move || (!acts).then(|| whose::reading_inbox(&store::handle_of(whose), &store::handle_of(me)));
    let mark_hint = move || reading().or_else(|| (!answered.get()).then(|| LOADING_HINT.to_string()));
    view! {
        <section class="page inbox">
            <ask::AskBox me=me scope=scope/>
            <header class="page-head">
                <h2>"Inbox"</h2>
                <WhosePicker me=me shown=Some(whose) hint=HINT choose=choose/>
                <button class="btn" disabled=move || !acts || !answered.get() || marking() title=mark_hint
                    on:click=mark_all_read>
                    {move || marks.with(Marks::label)}
                </button>
                <OpenQueries/>
            </header>
            {move || reading().map(|sentence| view! { <p class="reading-note">{sentence}</p> })}
            {move || marks.with(|marks| marks.failed.clone()).map(|failed| view! { <p class="error">{failed}</p> })}
            <Waiting page=page sorted=sorted closed=closed expanded=expanded open_rows=open_rows toggle=toggle
                loaded=loaded/>
            <Conversations page=page cards=cards sorted=sorted closed=closed toggle=toggle unread_only=unread_only
                answered=answered open_folds=open_folds/>
        </section>
    }
    .into_any()
}

/// A section's heading: its caret and what it says, `children`, opening and closing the section by
/// a click on it or by the keys (`accordion::on_heading`), and one stop of the page's selection. A
/// click on a control in it (a button, the "unread only" box) is the control's alone.
#[component]
fn SectionHead(
    section: Section,
    page: Page,
    closed: RwSignal<Closed>,
    toggle: Toggle,
    class: &'static str,
    children: Children,
) -> impl IntoView {
    let item = Item::Section(section);
    let element = item.element_id();
    let open = Memo::new(move |_| closed.with(|closed| closed.is_open(section)));
    let selected = Memo::new(move |_| page.selected.get() == Some(item));
    let on_key = {
        let element = element.clone();
        move |ev: web_sys::KeyboardEvent| {
            if ev.default_prevented() || !keys::is_plain(&ev) {
                return;
            }
            let on_heading =
                ev.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()).is_some_and(|t| t.id() == element);
            if let Some(now) = on_heading.then(|| accordion::on_heading(&ev.key(), open.get_untracked())).flatten() {
                ev.prevent_default();
                toggle.run((section, now));
            }
        }
    };
    let on_click = move |ev: web_sys::MouseEvent| {
        let on_control = ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .is_some_and(|t| t.closest("a, button, input, select, label").ok().flatten().is_some());
        if on_control {
            return;
        }
        if page.selected.get_untracked() != Some(item) {
            page.selected.set(Some(item));
        }
        toggle.run((section, !open.get_untracked()));
    };
    view! {
        <div class=format!("section-head {class}") id=element tabindex="-1" class:selected=move || selected.get()
            class:closed=move || !open.get() on:keydown=on_key on:click=on_click>
            <span class="caret" title="open or close this section">{move || if open.get() { "▾" } else { "▸" }}</span>
            {children()}
        </div>
    }
}

/// Waiting on you: its heading, how many rows wait and how many are new, and while it is open, a
/// note on what a row is, then each group that has rows, in the order the groups stand.
#[component]
fn Waiting(
    page: Page,
    sorted: Memo<Sorted>,
    closed: RwSignal<Closed>,
    expanded: RwSignal<HashSet<Kind>>,
    open_rows: RwSignal<HashSet<EntityId>>,
    toggle: Toggle,
    loaded: Signal<bool>,
) -> impl IntoView {
    // Whom it waits on: "you" on one's own inbox, the principal's handle on another's.
    let reader = Memo::new(move |_| if page.acts() { "you".to_string() } else { store::handle_of(page.whose) });
    let count = Memo::new(move |_| sorted.with(Sorted::waiting));
    let new = Memo::new(move |_| sorted.with(Sorted::unread));
    let open = Memo::new(move |_| closed.with(|closed| closed.is_open(Section::Waiting)));
    let kinds = Memo::new(move |_| sorted.with(|s| s.groups.iter().map(|g| g.kind).collect::<Vec<_>>()));
    let nothing = move || {
        kinds.with(Vec::is_empty).then(|| {
            let says = empty::says(&Nothing::Requests { reader: reader.get() }, loaded.get());
            view! { <p class="empty">{says}</p> }
        })
    };
    view! {
        <section class="inbox-region waiting" aria-label="Waiting on you">
            <SectionHead section=Section::Waiting page=page closed=closed toggle=toggle class="region-head">
                <h3>{move || format!("Waiting on {}", reader.get())}</h3>
                <span class="pill" title="rows waiting">{count}</span>
                {move || (new.get() > 0).then(|| view! { <span class="pill new">{format!("{} new", new.get())}</span> })}
            </SectionHead>
            {move || {
                open.get().then(|| {
                    view! {
                        <p class="region-note">
                            "Each row asks a move of " {reader} ". It stays until the move is made or the row is \
                             dismissed; reading it only clears its new mark."
                        </p>
                        {nothing}
                        <For each=move || kinds.get() key=|kind| *kind children=move |kind| {
                            view! {
                                <GroupSection kind=kind page=page sorted=sorted closed=closed expanded=expanded
                                    open_rows=open_rows toggle=toggle reader=reader/>
                            }
                        }/>
                    }
                })
            }}
        </section>
    }
}

/// Who sent a group's rows, for its heading: "from lead-fwd", "from lead-fwd and impl-x", or how
/// many principals past two.
fn senders(handles: &[String]) -> Option<String> {
    match handles {
        [] => None,
        [one] => Some(format!("from {one}")),
        [one, two] => Some(format!("from {one} and {two}")),
        several => Some(format!("from {} principals", several.len())),
    }
}

/// One group of Waiting on you, a section of its own: its heading says what it asks of the reader,
/// how many rows it holds and how many are new, and whom they are from, and offers Mark read, which
/// clears the rows' new marks, and Dismiss, which names its count (there is no Accept all: each
/// ruling keeps its own reason). Beneath it, while it is open, its batches, each under a line
/// naming its sender and the scope its work is in, the first eight rows of the group and then "N
/// more".
#[component]
fn GroupSection(
    kind: Kind,
    page: Page,
    sorted: Memo<Sorted>,
    closed: RwSignal<Closed>,
    expanded: RwSignal<HashSet<Kind>>,
    open_rows: RwSignal<HashSet<EntityId>>,
    toggle: Toggle,
    reader: Memo<String>,
) -> impl IntoView {
    let section = Section::Group(kind);
    let group = Memo::new(move |_| sorted.with(|s| s.group(kind).cloned()));
    let open = Memo::new(move |_| closed.with(|closed| closed.is_open(section)));
    let more = Memo::new(move |_| expanded.with(|expanded| expanded.contains(&kind)));
    let shown = Memo::new(move |_| group.with(|g| g.as_ref().map(|g| g.shown(more.get())).unwrap_or_default()));
    let hidden = Memo::new(move |_| group.with(|g| g.as_ref().map_or(0, |g| g.hidden(more.get()))));
    let len = Memo::new(move |_| group.with(|g| g.as_ref().map_or(0, Group::len)));
    let unread = Memo::new(move |_| group.with(|g| g.as_ref().map_or(0, Group::unread)));
    let from = move || {
        let handles: Vec<String> = group
            .with(|g| g.as_ref().map(Group::senders).unwrap_or_default())
            .into_iter()
            .map(store::handle_of)
            .collect();
        senders(&handles)
    };
    let acts = page.acts();
    let reading =
        move || (!acts).then(|| whose::reading_inbox(&store::handle_of(page.whose), &store::handle_of(page.thread.me)));
    let members = move || {
        group.with_untracked(|g| g.as_ref().map(|g| g.rows().flat_map(|r| r.members.clone()).collect::<Vec<_>>()))
    };
    let mark_read = move |_| {
        let unread: Vec<EntityId> = members()
            .unwrap_or_default()
            .into_iter()
            .filter(|id| {
                page.notifications.get(*id).is_some_and(|n| n.status().ok() == Some(NotificationStatus::Unread))
            })
            .collect();
        page.mark(unread, NotificationStatus::Read, false);
    };
    let dismiss = move |_| page.mark(members().unwrap_or_default(), NotificationStatus::Dismissed, false);
    let batches = move || shown.with(|shown| shown.iter().map(|b| (b.sender, b.scope)).collect::<Vec<_>>());
    view! {
        <section class="request-group">
            <SectionHead section=section page=page closed=closed toggle=toggle class="group-head">
                <h4>{move || kind.heading(&reader.get())}</h4>
                <span class="pill">{len}</span>
                {move || (unread.get() > 0).then(|| view! { <span class="pill new">{format!("{} new", unread.get())}</span> })}
                <span class="muted group-from">{from}</span>
                {move || {
                    let paired = sorted.with(|s| s.paired);
                    (kind == Kind::Assignment && paired > 0).then(|| {
                        let tasks = if paired == 1 { "1 more shows".to_string() } else { format!("{paired} more show") };
                        view! { <span class="muted group-from">{format!("· {tasks} beside the rulings they ask for, above")}</span> }
                    })
                }}
                <span class="group-acts">
                    <button class="btn subtle tiny" disabled=move || !acts || unread.get() == 0 title=reading
                        on:click=mark_read>"Mark read"</button>
                    <button class="btn subtle tiny" disabled=!acts title=reading on:click=dismiss>
                        {move || format!("Dismiss {}", len.get())}
                    </button>
                </span>
            </SectionHead>
            {move || {
                open.get().then(|| {
                    view! {
                        <For each=batches key=|key| *key children=move |key| {
                            view! { <BatchRows kind=kind batch=key group=group shown=shown page=page sorted=sorted open_rows=open_rows/> }
                        }/>
                        {move || {
                            (hidden.get() > 0).then(|| {
                                view! {
                                    <div class="group-more">
                                        <button class="btn subtle tiny" on:click=move |_| expanded.update(|e| {
                                            e.insert(kind);
                                        })>{format!("{} more", hidden.get())}</button>
                                    </div>
                                }
                            })
                        }}
                    }
                })
            }}
        </section>
    }
}

/// The rows of one batch, the rows its group shows of it (`shown`), under a line naming who sent
/// them and the scope their work is in, and how many of the whole batch are new, else how many
/// rows it has.
#[component]
fn BatchRows(
    kind: Kind,
    batch: (Option<EntityId>, Option<EntityId>),
    group: Memo<Option<Group>>,
    shown: Memo<Vec<Batch>>,
    page: Page,
    sorted: Memo<Sorted>,
    open_rows: RwSignal<HashSet<EntityId>>,
) -> impl IntoView {
    let (sender, scope) = batch;
    let whole = Memo::new(move |_| {
        group.with(|g| {
            g.as_ref()
                .and_then(|g| g.batches.iter().find(|b| (b.sender, b.scope) == batch))
                .map(|b| (b.rows.len(), b.unread()))
                .unwrap_or_default()
        })
    });
    let subjects = Memo::new(move |_| {
        shown.with(|shown| {
            shown
                .iter()
                .find(|b| (b.sender, b.scope) == batch)
                .map(|b| b.rows.iter().map(|r| r.subject).collect::<Vec<_>>())
                .unwrap_or_default()
        })
    });
    let counted = move || {
        let (rows, new) = whole.get();
        if new > 0 {
            format!("{new} new")
        } else {
            rows.to_string()
        }
    };
    view! {
        <div class="batch-head">
            {sender.map(|s| view! { <b>{move || store::handle_of(s)}</b> " · " })}
            {scope.map(|s| view! { {lozenges::single(Target::Scope(s))} " · " })}
            {counted}
        </div>
        <For each=move || subjects.get() key=|subject| *subject children=move |subject| {
            view! { <RequestRow subject=subject kind=kind sorted=sorted page=page open_rows=open_rows/> }
        }/>
    }
}

/// The conversations, as cards: the section's heading with how many cards it holds and "unread
/// only", which applies to the conversations alone (a row of Waiting on you stays read or not), and
/// while it is open Settled before you looked, then the cards, newest first.
#[component]
fn Conversations(
    page: Page,
    cards: Memo<Vec<Card>>,
    sorted: Memo<Sorted>,
    closed: RwSignal<Closed>,
    toggle: Toggle,
    unread_only: RwSignal<bool>,
    answered: RwSignal<bool>,
    open_folds: RwSignal<HashSet<EntityId>>,
) -> impl IntoView {
    let open = Memo::new(move |_| closed.with(|closed| closed.is_open(Section::Conversations)));
    let count = Memo::new(move |_| cards.with(Vec::len));
    let nothing = Signal::derive(move || {
        if answered.get() {
            Nothing::Conversations { unread_only: unread_only.get() }
        } else {
            Nothing::Yet
        }
    });
    view! {
        <section class="inbox-region conversations" aria-label="Conversations">
            <SectionHead section=Section::Conversations page=page closed=closed toggle=toggle class="region-head">
                <h3>"Conversations"</h3>
                <span class="pill" title="cards">{count}</span>
                <label class="toggle">
                    <input type="checkbox" prop:checked=move || unread_only.get()
                        on:change=move |ev| unread_only.set(event_target_checked(&ev))/>
                    " unread only"
                </label>
            </SectionHead>
            {move || {
                open.get().then(|| {
                    view! {
                        <Settled page=page sorted=sorted closed=closed toggle=toggle open_folds=open_folds/>
                        {move || cards.with(Vec::is_empty).then(|| view! { <Empty nothing=nothing/> })}
                        <For each=move || cards.get() key=|card| card.key children=move |card| {
                            view! { <NotificationCard key=card.key root=card.root cards=cards page=page/> }
                        }/>
                    }
                })
            }}
        </section>
    }
}

/// Settled before you looked, first among the conversations, a section of its own (task w4g0qBJY):
/// a line per task, else per sender, of the unread notifications that settled before the reader
/// looked (`waiting::Fold`), each saying what they are and what they are about, with Mark read,
/// which marks the line's notifications read in one write; the heading offers to mark them all
/// read. Nothing in it is read until the reader marks it. Shown only while anything settled.
#[component]
fn Settled(
    page: Page,
    sorted: Memo<Sorted>,
    closed: RwSignal<Closed>,
    toggle: Toggle,
    open_folds: RwSignal<HashSet<EntityId>>,
) -> impl IntoView {
    let count = Memo::new(move |_| sorted.with(Sorted::settled));
    let keys = Memo::new(move |_| sorted.with(|s| s.folds.iter().map(Fold::key).collect::<Vec<_>>()));
    let open = Memo::new(move |_| closed.with(|closed| closed.is_open(Section::Settled)));
    let acts = page.acts();
    let reading =
        move || (!acts).then(|| whose::reading_inbox(&store::handle_of(page.whose), &store::handle_of(page.thread.me)));
    let mark_all = move |_| {
        let ids: Vec<EntityId> = sorted.with_untracked(|s| s.folds.iter().flat_map(|f| f.members.clone()).collect());
        page.mark(ids, NotificationStatus::Read, false);
    };
    let any = Memo::new(move |_| count.get() > 0);
    move || {
        any.get().then(|| {
            view! {
                <section class="settled" aria-label="Settled before you looked">
                    <SectionHead section=Section::Settled page=page closed=closed toggle=toggle class="settled-head">
                        <h4>"Settled before you looked"</h4>
                        <span class="pill">{count}</span>
                        <span class="muted group-from">"resolved, or moved on, before you read them"</span>
                        <span class="group-acts">
                            <button class="btn subtle tiny" disabled=!acts title=reading on:click=mark_all>
                                {move || format!("Mark all {} read", count.get())}
                            </button>
                        </span>
                    </SectionHead>
                    {move || {
                        open.get().then(|| {
                            view! {
                                <For each=move || keys.get() key=|key| *key children=move |key| {
                                    view! { <FoldLine key=key page=page sorted=sorted open_folds=open_folds/> }
                                }/>
                            }
                        })
                    }}
                </section>
            }
        })
    }
}

/// One line of Settled before you looked: what its notifications are and what they are about ("169
/// comments and 18 replies about Review: the rename walk"), Mark read, and beneath it, while it is
/// open (ArrowRight, or its caret), each notification on a line of its own, mounting no thread
/// until one is opened. One stop of the page's selection: ArrowLeft on a closed line selects the
/// section's heading.
#[component]
fn FoldLine(key: EntityId, page: Page, sorted: Memo<Sorted>, open_folds: RwSignal<HashSet<EntityId>>) -> impl IntoView {
    let item = Item::Fold(key);
    let element = item.element_id();
    let fold = Memo::new(move |_| sorted.with(|s| s.fold(key).cloned()));
    let opened = Memo::new(move |_| open_folds.with(|open| open.contains(&key)));
    let selected = Memo::new(move |_| page.selected.get() == Some(item));
    let set_open = move |open: bool| {
        open_folds.update(|folds| {
            if open {
                folds.insert(key);
            } else {
                folds.remove(&key);
            }
        })
    };
    let acts = page.acts();
    let reading =
        move || (!acts).then(|| whose::reading_inbox(&store::handle_of(page.whose), &store::handle_of(page.thread.me)));
    let words = move || fold.with(|f| f.as_ref().map(|f| f.words.clone()));
    let about = move || {
        let (task, sender) = fold.with(|f| f.as_ref().map(|f| (f.task, f.sender)))?;
        Some(match (task, sender) {
            (Some(task), _) => view! { " about " {lozenges::single(Target::Task(task))} }.into_any(),
            (None, Some(sender)) => view! { " from " <b>{move || store::handle_of(sender)}</b> }.into_any(),
            (None, None) => ().into_any(),
        })
    };
    let members = Memo::new(move |_| fold.with(|f| f.as_ref().map(|f| f.members.clone()).unwrap_or_default()));
    let mark_read = move |_| page.mark(members.get_untracked(), NotificationStatus::Read, false);
    let on_key = {
        let element = element.clone();
        move |ev: web_sys::KeyboardEvent| {
            if ev.default_prevented() || !keys::is_plain(&ev) {
                return;
            }
            let on_line =
                ev.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()).is_some_and(|t| t.id() == element);
            let Some(step) = on_line.then(|| accordion::on_row(&ev.key(), opened.get_untracked())).flatten() else {
                return;
            };
            ev.prevent_default();
            match step {
                RowStep::Open => set_open(true),
                RowStep::Close => set_open(false),
                RowStep::ToGroup => page.select(Item::Section(Section::Settled)),
                RowStep::Activate => set_open(!opened.get_untracked()),
                RowStep::IntoThread => {}
            }
        }
    };
    let on_click = move |ev: web_sys::MouseEvent| {
        let on_control = ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .is_some_and(|t| t.closest("a, button, .fold-items").ok().flatten().is_some());
        if !on_control && page.selected.get_untracked() != Some(item) {
            page.selected.set(Some(item));
        }
    };
    view! {
        <div class="fold-line" id=element tabindex="-1" class:selected=move || selected.get()
            aria-expanded=move || opened.get().to_string() on:keydown=on_key on:click=on_click>
            <span class="caret" title="show or hide them" on:click=move |_| set_open(!opened.get_untracked())>
                {move || if opened.get() { "▾" } else { "▸" }}
            </span>
            <span class="fold-words">{words}{about}</span>
            <span class="group-acts">
                <button class="btn subtle tiny" disabled=!acts title=reading on:click=mark_read>"Mark read"</button>
            </span>
        </div>
        {move || {
            opened.get().then(|| {
                view! {
                    <ul class="fold-items">
                        <For each=move || members.get() key=|id| *id children=move |id| view! { <FoldItem id=id page=page/> }/>
                    </ul>
                }
            })
        }}
    }
}

/// One notification of a line of Settled before you looked, on a line of its own: a message's
/// author and first line, with "resolved" once it is, and "open", which mounts its thread beneath
/// it; a request's subject as its list item; else why it is in the inbox.
#[component]
fn FoldItem(id: EntityId, page: Page) -> impl IntoView {
    let shown = RwSignal::new(false);
    let message = Memo::new(move |_| {
        let n = page.notifications.get(id)?;
        let originator: Value = n.originator().ok()?.into();
        cards::id_at(&originator["message_id"])
    });
    let line = move || {
        if let Some(message) = message.get() {
            return message_row::list_item(message);
        }
        let Some(n) = page.notifications.get(id) else { return ().into_any() };
        let target: Value = n.target().map(Into::into).unwrap_or(Value::Null);
        match (target["type"].as_str(), cards::id_at(&target["id"])) {
            (Some("Task"), Some(task)) => task_row::list_item(task),
            (Some("Decision"), Some(decision)) => decision_row::list_item(decision),
            _ => view! { <span>{notification_card::sentence(page.whose, &n)}</span> }.into_any(),
        }
    };
    // The thread a message opens, or is in, mounted only once asked for.
    let thread = move || {
        let message = message.get().filter(|_| shown.get())?;
        let first =
            store::message(message).and_then(|m| m.thread().ok().flatten()).map(EntityId::from).unwrap_or(message);
        Some(thread::card_row(page.thread, id, first, Models::one(Model::Message), Hooks::default()))
    };
    view! {
        <li class="fold-item">
            {line}
            {move || {
                message.get().map(|_| {
                    view! {
                        " "
                        <button class="btn subtle tiny" on:click=move |_| shown.update(|s| *s = !*s)>
                            {move || if shown.get() { "close" } else { "open" }}
                        </button>
                    }
                })
            }}
            {thread}
        </li>
    }
}

#[cfg(test)]
mod tests {
    use super::senders;

    #[test]
    fn a_groups_heading_names_whom_its_rows_are_from() {
        let handles = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        assert_eq!(senders(&[]), None);
        assert_eq!(senders(&handles(&["lead-fwd"])).as_deref(), Some("from lead-fwd"));
        assert_eq!(senders(&handles(&["lead-fwd", "impl-x"])).as_deref(), Some("from lead-fwd and impl-x"));
        assert_eq!(senders(&handles(&["a", "b", "c"])).as_deref(), Some("from 3 principals"));
    }
}
