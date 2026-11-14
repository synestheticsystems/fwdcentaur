//! `fwd session view --follow`: the view printed at once, then again whenever it differs from the
//! last one printed, for a pane that redraws itself with no agent involved (Daniel, 2026-11-13: "I
//! do think it's really important to make it reactive, if that's possible, and not reliant on
//! polling"). A change reaches it through the node's live queries on what the view is made from
//! (`fwd_core::live_read`), never through a timer, and is only the signal to read the view again
//! with the one-shot read: changes that arrive while a read runs lead to one more read, not one per
//! change, and a document equal to the last one printed is not printed again.
//!
//! The one timer is the heartbeat, an empty line every 30 seconds while the connection to the
//! daemon is healthy, so that a reader can tell a quiet stream from a hung one. A daemon gone for
//! good (its socket closed for `daemon::LOST_AFTER`, or come back on another port), or a live query
//! Ankurah gave up on, prints the last document saying the daemon is out of reach, and the stream
//! joins it again, trying after 1 s, then 2 s, doubling up to 30 s, and never starting one, as `fwd
//! watch` does; the fresh document follows. A daemon that answers again running another build than
//! this process ends it, with exit 0, so that whatever runs it starts the build now installed.
//! Nothing else ends it but a reader gone.

use super::{join, line_of, page, write_line, Viewing};
use crate::BUILD;
use ankurah::signals::{Subscribe, SubscriptionGuard};
use ankurah_websocket_client::ConnectionState;
use anyhow::{anyhow, Context as _, Result};
use fwd_core::daemon::{self, Autostart, Client, Lost};
use fwd_core::live_read::{Listening, LiveRead};
use fwd_core::project::{NamedProject, Paths};
use fwd_core::session_view::Document;
use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::convert::Infallible;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use tokio::time::MissedTickBehavior;

/// How often an empty line says the stream is alive.
const HEARTBEAT: Duration = Duration::from_secs(30);
/// The wait before the next attempt to join a daemon out of reach, doubled after each failure up
/// to `MAX_RETRY`.
const FIRST_RETRY: Duration = Duration::from_secs(1);
const MAX_RETRY: Duration = Duration::from_secs(30);

/// Follows the view `viewing` reads until the daemon runs another build ([`ends_on_another_build`]),
/// joining the daemon at first as `autostart` says. It returns only an error: writing to a reader
/// that is gone, for one.
pub(super) async fn run(viewing: Viewing, paths: Paths, autostart: Autostart) -> Result<Infallible> {
    let mut following = Following { viewing, paths, changed: Arc::new(Notify::new()), printed: Printed::default() };
    let mut joined = match join(&following.paths, &autostart).await {
        Ok(client) => Some(following.joined(client)),
        Err(error) => {
            eprintln!("fwd: the daemon could not be joined ({error:#}); the view follows it once it answers");
            None
        }
    };
    loop {
        let session = match joined.take() {
            Some(session) => session,
            None => {
                following.printed.document(following.printed.last().out_of_reach())?;
                following.rejoin().await
            }
        };
        following.while_joined(session).await?;
    }
}

/// The stream across its connections.
struct Following {
    viewing: Viewing,
    paths: Paths,
    /// Told by every live read and by the socket coming back that the view may have changed. It
    /// holds one permit, so that the changes that arrive while a read runs lead to one more read.
    changed: Arc<Notify>,
    printed: Printed,
}

/// One connection to the daemon: the client, the live reads open through it, and what its socket
/// says.
struct Joined {
    // Declared first, so dropped first: they hold the client's node.
    listening: BTreeMap<LiveRead, Listening>,
    _socket: SubscriptionGuard,
    /// Set when the socket comes back after it closed, to the same port: the daemon may have
    /// restarted, running another build.
    came_back: Arc<AtomicBool>,
    client: Client,
}

impl Following {
    /// A connection through `client`, whose socket coming back after it closed is a change: the
    /// daemon may have restarted, and Ankurah registers the live reads there again.
    fn joined(&self, client: Client) -> Joined {
        let came_back = Arc::new(AtomicBool::new(false));
        let closed = AtomicBool::new(false);
        let (changed, told) = (self.changed.clone(), came_back.clone());
        let _socket = client.ws.state().subscribe(move |state: ConnectionState| match state {
            ConnectionState::Connected { .. } => {
                if closed.swap(false, Ordering::AcqRel) {
                    told.store(true, Ordering::Release);
                    changed.notify_one();
                }
            }
            _ => closed.store(true, Ordering::Release),
        });
        Joined { listening: BTreeMap::new(), _socket, came_back, client }
    }

    /// Reads and prints the view whenever it may have changed, and an empty line at each
    /// heartbeat while the connection is healthy, until the daemon is out of reach.
    async fn while_joined(&mut self, mut session: Joined) -> Result<()> {
        let mut heartbeat = tokio::time::interval_at(tokio::time::Instant::now() + HEARTBEAT, HEARTBEAT);
        heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            // A daemon that does not answer here is read below, which finds whether it is lost.
            if session.came_back.swap(false, Ordering::AcqRel) {
                if let Ok(Some(running)) = daemon::status(&self.paths).await {
                    ends_on_another_build(running.build.as_deref());
                }
            }
            match session.client.while_connected(self.read(&session.client, &mut session.listening)).await {
                Ok(document) => self.printed.document(document)?,
                Err(error) if error.is::<Lost>() => return Ok(()),
                Err(error) => self.printed.document(self.printed.last().unreadable(&format!("{error:#}")))?,
            }
            loop {
                let changed = async {
                    self.changed.notified().await;
                    Ok(())
                };
                tokio::select! {
                    // Waiting fails only when the daemon is out of reach.
                    waited = session.client.while_connected(changed) => match waited {
                        Ok(()) => break,
                        Err(_) => return Ok(()),
                    },
                    _ = heartbeat.tick() => {
                        if session.listening.values().any(|l| l.failure().is_some()) {
                            return Ok(());
                        }
                        if session.client.ws.is_connected() {
                            self.printed.heartbeat()?;
                        }
                    }
                }
            }
        }
    }

    /// The view as the daemon has it now, with a live read open on each part of what it was read
    /// from, and none left open on what it no longer reads.
    async fn read(&self, client: &Client, listening: &mut BTreeMap<LiveRead, Listening>) -> Result<Document> {
        let held = self.viewing.read(client).await?;
        let wanted: BTreeSet<LiveRead> = held.live_reads();
        listening.retain(|read, _| wanted.contains(read));
        for read in wanted {
            if let Entry::Vacant(vacant) = listening.entry(read) {
                let changed = self.changed.clone();
                let opened = vacant.key().listen(&client.graph, Arc::new(move |()| changed.notify_one()))?;
                vacant.insert(opened);
            }
        }
        Ok(held.document(&page(client)))
    }

    /// Joins the daemon again once it answers, never starting one: one stopped on purpose, to
    /// install a new build say, stays stopped until something else starts it.
    async fn rejoin(&self) -> Joined {
        let mut wait = FIRST_RETRY;
        loop {
            tokio::time::sleep(wait).await;
            match self.answering().await {
                Ok(client) => return self.joined(client),
                Err(error) => tracing::debug!(error = format!("{error:#}"), "the view could not join the daemon"),
            }
            wait = (wait * 2).min(MAX_RETRY);
        }
    }

    /// A client of the daemon, when one answers, through the first project it hosts; it ends the
    /// process when that daemon runs another build ([`ends_on_another_build`]).
    async fn answering(&self) -> Result<Client> {
        let running = daemon::status(&self.paths).await?.ok_or_else(|| anyhow!("no daemon answers"))?;
        ends_on_another_build(running.build.as_deref());
        let hosted = running.projects.into_iter().find(|p| p.error.is_none()).context("the daemon hosts no project")?;
        let project = NamedProject { id: hosted.id, name: hosted.name };
        daemon::ensure_daemon(&self.paths, &project, Autostart::Never).await
    }
}

/// Ends the process, with exit 0 and a line on standard error saying why, when the daemon runs
/// another build than this process (`build`, none from a daemon older than builds on `/health`):
/// the fwd on the machine is no longer the build that started, and the document may have changed
/// shape, so whatever runs the stream starts it again.
fn ends_on_another_build(build: Option<&str>) {
    if build == Some(BUILD) {
        return;
    }
    let theirs = build.unwrap_or("one that does not name its build");
    eprintln!(
        "fwd: the daemon now runs build {theirs}, not this view's {BUILD}; start `fwd session view --follow` again"
    );
    std::process::exit(0)
}

/// What the stream has printed.
#[derive(Default)]
struct Printed {
    /// The last document printed, and its line.
    last: Option<(Document, String)>,
}

impl Printed {
    /// The last document printed; before any, the one that says the daemon is out of reach.
    fn last(&self) -> Document {
        self.last.as_ref().map_or_else(Document::never_read, |(document, _)| document.clone())
    }

    /// Prints `document` unless it is the one printed last.
    fn document(&mut self, document: Document) -> Result<()> {
        let line = line_of(&document);
        if self.last.as_ref().is_some_and(|(_, last)| *last == line) {
            return Ok(());
        }
        write_line(&line)?;
        self.last = Some((document, line));
        Ok(())
    }

    /// Prints the heartbeat's empty line.
    fn heartbeat(&self) -> Result<()> {
        write_line("")
    }
}
