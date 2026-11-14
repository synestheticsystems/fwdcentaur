//! `fwd session view --follow`: the view printed at once, then again whenever it differs from the
//! last one printed, for a pane that redraws itself with no agent involved (Daniel, 2026-11-13: "I
//! do think it's really important to make it reactive, if that's possible, and not reliant on
//! polling"). A change reaches it through the node's live queries on what the view is made from
//! (`fwd_core::live_read`), never through a timer, and is only the signal to read the view again
//! with the one-shot read: changes that arrive while a read runs lead to one more read, not one per
//! change, and a document equal to the last one printed is not printed again.
//!
//! Refreshes are driven by those changes alone; timers serve the rest. An empty line every 30
//! seconds while the stream is healthy (connected, its last read good) lets a reader tell a quiet
//! stream from a hung one, and goes on while a read runs. One observer of the daemon's loss lives
//! as long as the connection: a socket closed for ten seconds, or a daemon come back on another
//! port, or a live query Ankurah gave up on, prints the last document saying the daemon is out of
//! reach, and the stream joins it again after 1 s, then 2 s, doubling up to 30 s, never starting
//! one, as `fwd watch` does; the fresh document follows. A read that fails with the daemon in reach
//! prints the last document saying so, and is tried again after the same waits.
//!
//! It ends by itself, with exit 0, in two cases ([`Ended`]): the daemon answers running another
//! build than this process (at the first join, at each join again, and when the socket comes back
//! on the same port, a check that could not be made being made again until it is), so that
//! whatever runs it starts the build now installed; and its reader is gone (a write that finds its
//! standard output closed, or, at each tick of its timers, the process that started it gone).

use super::{join, line_of, page, Viewing};
use crate::BUILD;
use ankurah::signals::{Subscribe, SubscriptionGuard};
use ankurah_websocket_client::ConnectionState;
use anyhow::{anyhow, Context as _, Result};
use fwd_core::daemon::{self, Autostart, Client, Lost};
use fwd_core::live_read::{Listening, LiveRead};
use fwd_core::project::{NamedProject, Paths};
use fwd_core::session_view::{Document, Held};
use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::convert::Infallible;
use std::future::Future;
use std::io::Write;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use tokio::time::{Instant, MissedTickBehavior};

/// How often an empty line says the stream is alive.
const HEARTBEAT: Duration = Duration::from_secs(30);
/// The wait before the next attempt to join a daemon out of reach, or to read again after a read
/// that failed, doubled after each failure up to `MAX_RETRY`.
const FIRST_RETRY: Duration = Duration::from_secs(1);
const MAX_RETRY: Duration = Duration::from_secs(30);

/// How the stream ended by itself, which `main` turns into exit 0.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Ended {
    /// The daemon answers running another build, whose name it gives, or none for a daemon too old
    /// to give one: the fwd on the machine is no longer this process's build, and the document may
    /// have changed shape, so whatever runs the stream starts it again.
    #[error("fwd: the daemon now runs build {}, not this view's {BUILD}; start `fwd session view --follow` again", .theirs.as_deref().unwrap_or("of its own"))]
    AnotherBuild { theirs: Option<String> },
    /// No one reads the stream any more.
    #[error("the view's reader is gone")]
    ReaderGone,
}

/// Follows the view `viewing` reads, joining the daemon at first as `autostart` says, until it
/// ends by itself ([`Ended`]) or fails writing; it returns only how it ended.
pub(super) async fn run(viewing: Viewing, paths: Paths, autostart: Autostart) -> Result<Infallible> {
    let mut stream = Stream {
        viewing,
        paths,
        changed: Arc::new(Notify::new()),
        printed: Printed::default(),
        parent: std::os::unix::process::parent_id(),
    };
    let mut joined = match join(&stream.paths, &autostart).await {
        Ok((client, build)) => {
            another_build(build.as_deref())?;
            Some(client)
        }
        Err(error) => {
            say(&format!("fwd: the daemon could not be joined ({error:#}); the view follows it once it answers"));
            None
        }
    };
    loop {
        let client = match joined.take() {
            Some(client) => client,
            None => {
                stream.printed.document(stream.printed.last().out_of_reach())?;
                stream.rejoin().await?
            }
        };
        stream.while_joined(client).await?;
    }
}

/// The stream across its connections.
struct Stream {
    viewing: Viewing,
    paths: Paths,
    /// Told by every live read and by the socket coming back that the view may have changed. It
    /// holds one permit, so that the changes that arrive while a read runs lead to one more read.
    changed: Arc<Notify>,
    printed: Printed,
    /// The process that started this one: once it is gone, so is the stream's reader.
    parent: u32,
}

/// A read of the view under way, which the stream polls beside its timers.
type Reading<'a> = Pin<Box<dyn Future<Output = Result<Held>> + 'a>>;

impl Stream {
    /// Reads and prints the view whenever it may have changed, and an empty line at each heartbeat
    /// while it is healthy, until the daemon is out of reach (it returns) or the stream ends.
    async fn while_joined(&mut self, client: Client) -> Result<()> {
        let came_back = Arc::new(AtomicBool::new(false));
        let _socket = self.watch_socket(&client, &came_back);
        let mut listening: BTreeMap<LiveRead, Listening> = BTreeMap::new();
        let lost = client.while_connected(std::future::pending::<Result<()>>());
        tokio::pin!(lost);
        let mut heartbeat = tokio::time::interval_at(Instant::now() + HEARTBEAT, HEARTBEAT);
        heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let viewing = &self.viewing;
        let mut reading: Option<Reading<'_>> = Some(Box::pin(viewing.read(&client)));
        // After a read that failed: the wait before it was tried again, and when it is tried.
        let mut failing: Option<(Duration, Instant)> = None;
        let mut build_pending = false;
        loop {
            build_pending |= came_back.swap(false, Ordering::AcqRel);
            if build_pending {
                build_pending = !self.build_checked().await?;
            }
            tokio::select! {
                biased;
                _ = &mut lost => return Ok(()),
                read = reading_of(&mut reading), if reading.is_some() => {
                    reading = None;
                    match read.and_then(|held| self.listen(&client, &mut listening, held)) {
                        Ok(document) => {
                            self.printed.document(document)?;
                            failing = None;
                        }
                        Err(error) if error.is::<Lost>() => return Ok(()),
                        Err(error) => {
                            say(&format!("fwd: the view could not be read: {error:#}"));
                            self.printed.document(self.printed.last().unreadable(&error))?;
                            let wait = failing.map_or(FIRST_RETRY, |(wait, _)| (wait * 2).min(MAX_RETRY));
                            failing = Some((wait, Instant::now() + wait));
                        }
                    }
                }
                () = self.changed.notified(), if reading.is_none() && failing.is_none() => {
                    reading = Some(Box::pin(viewing.read(&client)));
                }
                () = sleep_until(failing.map(|(_, at)| at)), if reading.is_none() && failing.is_some() => {
                    self.parent_here()?;
                    reading = Some(Box::pin(viewing.read(&client)));
                }
                _ = heartbeat.tick() => {
                    self.parent_here()?;
                    if listening.values().any(|l| l.failure().is_some()) {
                        return Ok(());
                    }
                    if failing.is_none() && client.ws.is_connected() {
                        self.printed.heartbeat()?;
                    }
                }
            }
        }
    }

    /// Opens a live read on each part of what `held` was read from, drops those on what it no
    /// longer reads, and gives the document.
    fn listen(&self, client: &Client, listening: &mut BTreeMap<LiveRead, Listening>, held: Held) -> Result<Document> {
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

    /// Hears the socket to the daemon come back after it closed, to the same port, which is a
    /// change, since Ankurah registers the live reads there again, and calls for the build check
    /// (`came_back`), since the daemon may have restarted as another build.
    fn watch_socket(&self, client: &Client, came_back: &Arc<AtomicBool>) -> SubscriptionGuard {
        let closed = AtomicBool::new(false);
        let (changed, told) = (self.changed.clone(), came_back.clone());
        client.ws.state().subscribe(move |state: ConnectionState| match state {
            ConnectionState::Connected { .. } => {
                if closed.swap(false, Ordering::AcqRel) {
                    told.store(true, Ordering::Release);
                    changed.notify_one();
                }
            }
            _ => closed.store(true, Ordering::Release),
        })
    }

    /// Whether the daemon's build could be read, ending the stream when it is another than this
    /// process's.
    async fn build_checked(&self) -> Result<bool> {
        match daemon::status(&self.paths).await {
            Ok(Some(running)) => another_build(running.build.as_deref()).map(|()| true),
            _ => Ok(false),
        }
    }

    /// Joins the daemon again once it answers, never starting one, after a wait that doubles with
    /// each attempt: one stopped on purpose, to install a new build say, stays stopped until
    /// something else starts it.
    async fn rejoin(&self) -> Result<Client> {
        let mut wait = FIRST_RETRY;
        loop {
            tokio::time::sleep(wait).await;
            self.parent_here()?;
            match self.answering().await {
                Ok(client) => return Ok(client),
                Err(error) if error.is::<Ended>() => return Err(error),
                Err(error) => tracing::debug!(error = format!("{error:#}"), "the view could not join the daemon"),
            }
            wait = (wait * 2).min(MAX_RETRY);
        }
    }

    /// A client of the daemon, when one answers running this process's build, through the first
    /// project it hosts.
    async fn answering(&self) -> Result<Client> {
        let running = daemon::status(&self.paths).await?.ok_or_else(|| anyhow!("no daemon answers"))?;
        another_build(running.build.as_deref())?;
        let hosted = running.projects.into_iter().find(|p| p.error.is_none()).context("the daemon hosts no project")?;
        let project = NamedProject { id: hosted.id, name: hosted.name };
        daemon::ensure_daemon(&self.paths, &project, Autostart::Never).await
    }

    /// Ends the stream once the process that started it is gone, as its reader then is.
    fn parent_here(&self) -> Result<()> {
        if std::os::unix::process::parent_id() == self.parent {
            Ok(())
        } else {
            Err(Ended::ReaderGone.into())
        }
    }
}

/// The read under way, once it is done; never, while there is none.
async fn reading_of(reading: &mut Option<Reading<'_>>) -> Result<Held> {
    match reading {
        Some(read) => read.await,
        None => std::future::pending().await,
    }
}

/// Resolves at `at`; never, without one.
async fn sleep_until(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// Ends the stream when `build`, the build a daemon answers with, is not this process's.
fn another_build(build: Option<&str>) -> Result<()> {
    if build == Some(BUILD) {
        Ok(())
    } else {
        Err(Ended::AnotherBuild { theirs: build.map(str::to_string) }.into())
    }
}

/// Says `line` on standard error; a line that cannot be written has no reader left.
fn say(line: &str) {
    let _ = writeln!(std::io::stderr(), "{line}");
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
        written(&line)?;
        self.last = Some((document, line));
        Ok(())
    }

    /// Prints the heartbeat's empty line.
    fn heartbeat(&self) -> Result<()> {
        written("")
    }
}

/// Writes `line` and a newline on standard output, flushed; a reader gone ends the stream quietly.
fn written(line: &str) -> Result<()> {
    let mut out = std::io::stdout().lock();
    match writeln!(out, "{line}").and_then(|()| out.flush()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Err(Ended::ReaderGone.into()),
        Err(error) => Err(anyhow::Error::new(error).context("writing the view on standard output")),
    }
}
