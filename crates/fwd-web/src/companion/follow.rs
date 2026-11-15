//! The view of one session, read and kept current as `fwd session view --follow` keeps the pane's:
//! read with the CLI's one-shot read (`fwd_core::session_view::read`), for the person the page acts
//! as, then again whenever a live query on what it was made from changes (`fwd_core::live_read`),
//! never on a timer (Daniel, 2026-11-13: "I do think it's really important to make it reactive, if
//! that's possible, and not reliant on polling"). A change is only the signal to read again:
//! changes that arrive while a read runs lead to one more read, not one per change, and after each
//! read the live queries it needs are opened and those it no longer needs dropped. A read that
//! fails keeps what the view last showed, saying why, and is tried again after 1 s, then 2 s,
//! doubling up to 30 s, as the CLI tries it ([`Reads`]).

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
            match read {
                Ok(document) => self.show(document),
                Err(error) => {
                    tracing::warn!("the view of the session could not be read: {error:#}");
                    let last = self.0.shown.get_untracked().unwrap_or_else(Document::never_read);
                    self.show(last.unreadable(&error));
                }
            }
            match next {
                Next::Now => continue,
                Next::Idle => return,
                Next::After(wait) => {
                    session::sleep_ms(wait).await;
                    let mut inner = self.0.inner.borrow_mut();
                    if inner.stopped || !inner.reads.retry() {
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

/// Where the reads stand: whether one runs, whether a change came that it may not have read, and
/// after a read that failed, the wait before it is tried again.
#[derive(Debug, Default)]
struct Reads {
    running: bool,
    again: bool,
    retry: Option<i32>,
}

/// What follows a read.
#[derive(Debug, PartialEq, Eq)]
enum Next {
    /// Another read, at once, for a change it may not have read.
    Now,
    /// Nothing, until something changes.
    Idle,
    /// The read tried again after this many milliseconds.
    After(i32),
}

impl Reads {
    /// Something changed: whether a read starts now. While one runs, or waits to be tried again,
    /// the change is read once that one is done.
    fn changed(&mut self) -> bool {
        if self.running || self.retry.is_some() {
            self.again = true;
            return false;
        }
        self.running = true;
        true
    }

    /// The read that ran is done, and `succeeded` or not: what follows it.
    fn finished(&mut self, succeeded: bool) -> Next {
        if !succeeded {
            let wait = self.retry.map_or(FIRST_RETRY_MS, |wait| (wait * 2).min(MAX_RETRY_MS));
            self.retry = Some(wait);
            self.running = false;
            return Next::After(wait);
        }
        self.retry = None;
        if std::mem::take(&mut self.again) {
            Next::Now
        } else {
            self.running = false;
            Next::Idle
        }
    }

    /// The wait before a read that failed is tried again is over: whether it starts now, which it
    /// does unless a read already runs. It reads every change made until it starts.
    fn retry(&mut self) -> bool {
        if self.running {
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
        let waits: Vec<Next> = (0..7)
            .map(|_| {
                let next = reads.finished(false);
                assert!(reads.retry(), "the wait over, the read is tried again");
                next
            })
            .collect();
        let after = |ms| Next::After(ms);
        assert_eq!(
            waits,
            [after(1_000), after(2_000), after(4_000), after(8_000), after(16_000), after(30_000), after(30_000)]
        );
        assert_eq!(reads.finished(false), Next::After(30_000));
        assert!(!reads.changed(), "a change while it waits starts no read of its own");
        assert!(reads.retry());
        assert_eq!(reads.finished(true), Next::Idle, "the try read it");
        assert!(reads.changed());
        assert_eq!(reads.finished(false), Next::After(1_000), "a read that succeeded starts the waits over");
    }
}
