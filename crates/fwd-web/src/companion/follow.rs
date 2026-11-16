//! The view of one session, read and kept current as `fwd session view --follow` keeps the pane's:
//! read with the CLI's one-shot read (`fwd_core::session_view::read`), for the person the page acts
//! as, then again whenever a live query on what it was made from changes (`fwd_core::live_read`),
//! never on a timer (Daniel, 2026-11-13: "I do think it's really important to make it reactive, if
//! that's possible, and not reliant on polling"). A change is only the signal to read again:
//! changes that arrive while a read runs lead to one more read, not one per change, and after each
//! read the live queries it needs are opened and those it no longer needs dropped. A read that
//! fails keeps what the view last showed, saying why, and is tried again after 1 s, then 2 s,
//! doubling up to 30 s, as the CLI tries it ([`Reads`]). While the connection to the daemon is
//! lost nothing is read, and the view keeps what it showed; once it is back, the live queries
//! Ankurah gave up on are opened again and the view is read at once, for what changed meanwhile.

use crate::session::{self, graph};
use fwd_core::live_read::{Listening, LiveRead};
use fwd_core::session_view::{self, Document, Held, Reader};
use leptos::prelude::*;
use send_wrapper::SendWrapper;
use std::cell::RefCell;
use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use wasm_bindgen_futures::spawn_local;

/// The wait before a read that failed is tried again, doubled after each failure up to
/// [`MAX_RETRY_MS`].
const FIRST_RETRY_MS: i32 = 1_000;
const MAX_RETRY_MS: i32 = 30_000;

/// The view of one session as it is followed: what it shows, and what keeps it current, until
/// [`Follow::stop`].
#[derive(Clone)]
pub struct Follow(Rc<State>);

struct State {
    session: String,
    /// The principal the page acts as, by its id, which names the person the view is read for.
    reader: String,
    /// The page's address, which the document's links follow.
    page: String,
    /// What the view shows: none until the first read has answered.
    shown: ArcRwSignal<Option<Document>>,
    inner: RefCell<Inner>,
}

struct Inner {
    reads: Reads,
    listening: BTreeMap<LiveRead, Listening>,
    stopped: bool,
}

impl Follow {
    /// Starts following the view of the Claude Code session `session` for the person the page acts
    /// as: the first read starts at once.
    pub fn start(session: String) -> Follow {
        let reader = session::me().map(|me| me.to_base64()).unwrap_or_default();
        let state = State {
            session,
            reader,
            page: session::page_address(),
            shown: ArcRwSignal::new(None),
            inner: RefCell::new(Inner { reads: Reads::default(), listening: BTreeMap::new(), stopped: false }),
        };
        let follow = Follow(Rc::new(state));
        follow.changed();
        follow
    }

    /// What the view shows, as it changes: none until the first read has answered.
    pub fn shown(&self) -> ArcRwSignal<Option<Document>> {
        self.0.shown.clone()
    }

    /// Drops every live query, and reads no more.
    pub fn stop(&self) {
        let mut inner = self.0.inner.borrow_mut();
        inner.stopped = true;
        inner.listening.clear();
    }

    /// The connection to the daemon is lost: nothing is read until it is back.
    pub fn lost(&self) {
        self.0.inner.borrow_mut().reads.lost();
    }

    /// The connection to the daemon is back: the live queries Ankurah gave up on are opened again
    /// by the read that starts now, which reads what changed meanwhile.
    pub fn back(&self) {
        let start = {
            let mut inner = self.0.inner.borrow_mut();
            inner.listening.retain(|_, listening| listening.failure().is_none());
            inner.reads.back()
        };
        if start {
            spawn_local(self.clone().reading());
        }
    }

    /// What it reads may have changed: reads it now, or once the read that runs is done.
    fn changed(&self) {
        if self.0.inner.borrow_mut().reads.changed() {
            spawn_local(self.clone().reading());
        }
    }

    /// Reads the view until no change is left unread, then waits for the next, or, after a read
    /// that failed, for the wait before trying it again.
    async fn reading(self) {
        loop {
            let read = session_view::read(&graph(), &self.0.session, Reader::Named(&self.0.reader)).await;
            if self.0.inner.borrow().stopped {
                return;
            }
            let read = read.and_then(|held| self.listen(held));
            let next = self.0.inner.borrow_mut().reads.finished(read.is_ok());
            match (read, &next) {
                (Ok(document), _) => self.show(document),
                // The page says the daemon is out of reach, and reads again once it is back.
                (Err(_), Next::Lost) => {}
                (Err(error), _) => {
                    tracing::warn!("the view of the session could not be read: {error:#}");
                    let last = self.0.shown.get_untracked().unwrap_or_else(Document::never_read);
                    self.show(last.unreadable(&error));
                }
            }
            match next {
                Next::Now => continue,
                Next::Idle | Next::Lost => return,
                Next::After(wait) => {
                    session::sleep_ms(wait.ms).await;
                    let mut inner = self.0.inner.borrow_mut();
                    if inner.stopped || !inner.reads.retry(wait) {
                        return;
                    }
                }
            }
        }
    }

    /// Opens a live query on each part of what `held` was read from, drops those on what it no
    /// longer reads, and gives the document.
    fn listen(&self, held: Held) -> anyhow::Result<Document> {
        let wanted: BTreeSet<LiveRead> = held.live_reads();
        let mut inner = self.0.inner.borrow_mut();
        inner.listening.retain(|read, _| wanted.contains(read));
        for read in wanted {
            if let Entry::Vacant(vacant) = inner.listening.entry(read) {
                let opened = vacant.key().listen(&graph(), self.listener())?;
                vacant.insert(opened);
            }
        }
        Ok(held.document(&self.0.page))
    }

    /// What a live query calls at each change: a read, after the call returns, since a live query
    /// may call it while it is being opened.
    fn listener(&self) -> ankurah::signals::Listener {
        let follow = SendWrapper::new(Rc::downgrade(&self.0));
        Arc::new(move |()| {
            let follow: &Weak<State> = &follow;
            if let Some(state) = follow.upgrade() {
                spawn_local(async move { Follow(state).changed() });
            }
        })
    }

    /// Shows `document`, unless it is what the view shows already.
    fn show(&self, document: Document) {
        if self.0.shown.with_untracked(|shown| shown.as_ref() != Some(&document)) {
            self.0.shown.set(Some(document));
        }
    }
}

/// Where the reads stand: whether one runs, whether a change came that it may not have read, after
/// a read that failed the wait before it is tried again, and whether the connection is lost.
#[derive(Debug, Default)]
struct Reads {
    running: bool,
    again: bool,
    retry: Option<Wait>,
    lost: bool,
    /// How many waits have begun, which numbers the next one.
    waits: u64,
}

/// A wait before a read that failed is tried again: how long it lasts, and which wait it is, so
/// that one the connection's return replaced starts nothing when it ends ([`Reads::retry`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Wait {
    ms: i32,
    number: u64,
}

/// What follows a read.
#[derive(Debug, PartialEq, Eq)]
enum Next {
    /// Another read, at once, for a change it may not have read.
    Now,
    /// Nothing, until something changes.
    Idle,
    /// The read tried again once this wait is over.
    After(Wait),
    /// Nothing until the connection is back: the read failed while it was lost.
    Lost,
}

impl Reads {
    /// Something changed: whether a read starts now. While one runs, waits to be tried again, or
    /// the connection is lost, the change is read once that is over.
    fn changed(&mut self) -> bool {
        if self.running || self.retry.is_some() || self.lost {
            self.again = true;
            return false;
        }
        self.running = true;
        true
    }

    /// The read that ran is done, and `succeeded` or not: what follows it.
    fn finished(&mut self, succeeded: bool) -> Next {
        self.running = false;
        if !succeeded && self.lost {
            self.again = true;
            return Next::Lost;
        }
        if !succeeded {
            let ms = self.retry.map_or(FIRST_RETRY_MS, |wait| (wait.ms * 2).min(MAX_RETRY_MS));
            self.waits += 1;
            let wait = Wait { ms, number: self.waits };
            self.retry = Some(wait);
            return Next::After(wait);
        }
        self.retry = None;
        if self.again && !self.lost {
            self.again = false;
            self.running = true;
            return Next::Now;
        }
        Next::Idle
    }

    /// The wait `wait` before a read that failed is tried again is over: whether the read starts
    /// now, which it does unless the connection is lost, or came back since the wait began and so
    /// replaced it with a read of its own. It reads every change made until it starts.
    fn retry(&mut self, wait: Wait) -> bool {
        if self.lost || self.retry != Some(wait) {
            return false;
        }
        self.running = true;
        self.again = false;
        true
    }

    /// The connection to the daemon is lost.
    fn lost(&mut self) {
        self.lost = true;
    }

    /// The connection to the daemon is back: whether a read starts now, which reads what changed
    /// meanwhile, with no wait left over from a read that failed. One that runs is read again.
    fn back(&mut self) -> bool {
        self.lost = false;
        self.retry = None;
        if self.running {
            self.again = true;
            return false;
        }
        self.running = true;
        self.again = false;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wait that follows a read that failed.
    fn waiting(next: Next) -> Wait {
        match next {
            Next::After(wait) => wait,
            other => panic!("a read that failed is followed by {other:?}, not a wait"),
        }
    }

    /// Changes that arrive while a read runs lead to one more read, and none after it.
    #[test]
    fn changes_during_a_read_lead_to_one_more_read() {
        let mut reads = Reads::default();
        assert!(reads.changed(), "the first change starts a read");
        assert!(!reads.changed() && !reads.changed(), "two more while it runs start none");
        assert_eq!(reads.finished(true), Next::Now, "they are read once");
        assert_eq!(reads.finished(true), Next::Idle);
        assert!(reads.changed(), "a change after that starts a read again");
    }

    /// A read that fails is tried again after a wait that doubles up to 30 s, and the change that
    /// came meanwhile is read by that try, not by one more after it.
    #[test]
    fn a_read_that_fails_is_tried_again_after_a_wait_that_doubles() {
        let mut reads = Reads::default();
        assert!(reads.changed());
        let waits: Vec<i32> = (0..7)
            .map(|_| {
                let wait = waiting(reads.finished(false));
                assert!(reads.retry(wait), "the wait over, the read is tried again");
                wait.ms
            })
            .collect();
        assert_eq!(waits, [1_000, 2_000, 4_000, 8_000, 16_000, 30_000, 30_000]);
        let wait = waiting(reads.finished(false));
        assert_eq!(wait.ms, 30_000);
        assert!(!reads.changed(), "a change while it waits starts no read of its own");
        assert!(reads.retry(wait));
        assert_eq!(reads.finished(true), Next::Idle, "the try read it");
        assert!(reads.changed());
        assert_eq!(waiting(reads.finished(false)).ms, 1_000, "a read that succeeded starts the waits over");
    }

    /// While the connection is lost nothing is read, a read that fails is neither shown nor tried
    /// again, and the wait of one that failed before is dropped; once it is back one read starts,
    /// for every change meanwhile, and the wait it dropped starts nothing when it ends, even after a
    /// read that succeeded or failed since.
    #[test]
    fn nothing_is_read_while_the_connection_is_lost_and_one_read_starts_once_it_is_back() {
        let mut reads = Reads::default();
        assert!(reads.changed());
        reads.lost();
        assert_eq!(reads.finished(false), Next::Lost, "the read under way fails with the connection");
        assert!(!reads.changed() && !reads.changed(), "no change is read meanwhile");
        assert!(reads.back(), "once it is back, one read starts");
        assert!(!reads.changed());
        assert_eq!(reads.finished(true), Next::Now, "and a change during it is read after it");
        assert_eq!(reads.finished(true), Next::Idle);

        assert!(reads.changed());
        let ends_while_lost = waiting(reads.finished(false));
        reads.lost();
        assert!(!reads.retry(ends_while_lost), "the try waits for the connection");
        assert!(reads.back(), "which starts a read at once");
        let ends_once_back = waiting(reads.finished(false));
        assert_eq!(ends_once_back.ms, 1_000, "with the waits started over");

        reads.lost();
        assert!(reads.back());
        assert_eq!(reads.finished(true), Next::Idle);
        assert!(!reads.retry(ends_once_back), "a wait the return dropped starts nothing once it ends");
        assert!(reads.changed());
        let dropped = waiting(reads.finished(false));
        reads.lost();
        assert!(reads.back());
        let since = waiting(reads.finished(false));
        assert!(!reads.retry(dropped), "nor cuts short the wait of a read that failed since");

        assert!(reads.retry(since));
        reads.lost();
        assert!(!reads.back(), "back while a read runs");
        assert_eq!(reads.finished(true), Next::Now, "it is read again, for what changed while it was lost");
    }
}
