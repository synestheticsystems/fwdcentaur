//! `fwd watch`: a principal's notifications as JSON lines on standard output, until it has printed
//! `--count` of them. It is how an agent waits for an answer, run in the background so that its
//! exit wakes the agent; it has no timeout unless given `--timeout`, so the agent is woken only when
//! there is something to read (Daniel, 2026-09-29: "ideally fwd watch wouldn't have any timeout at
//! all. it should only exit if the daemon has been unreachable for an extended period of time").
//! The harness that runs this project's agents ends a background command within two hours all the
//! same, so an agent there gives `--timeout 7100` and arms the watch again after its `timeout`
//! event, the new watch printing first what arrived meanwhile
//! (docs/design/agents-and-inbox.md, "The triage agent's loop").
//!
//! Every agent loop waits in a watch, so a watch has to keep hearing across a daemon restart, and
//! says nothing about one (Daniel, 2026-09-26: "A daemon restart should not terminate a watcher,
//! but rather it should silently reconnect"). When the daemon comes back on the same port,
//! Ankurah reconnects the socket and registers the query again by itself. The watch covers what
//! Ankurah does not:
//!
//! - a query Ankurah has given up on, which it reports as the query's error. That is how every
//!   query of a watcher ended after a restart in the failure filed as "Make fwd watch fail loudly
//!   or resubscribe when its subscriptions die after a daemon restart"; its cause, the node's store
//!   emptied while the watch waited, is fixed where the store is made (`fwd_core::daemon`,
//!   `ephemeral_store`), and the watch recovers from any other cause the same way;
//! - a daemon that came back on another port, where Ankurah does not look; the port file names it;
//! - a query that stops delivering with neither sign: once a minute the watch asks the daemon for
//!   the unread notifications itself, and prints any the query missed.
//!
//! On a failed query or a moved daemon, the watch drops its ephemeral node, with the node's store
//! and query, and connects a new one, trying after 1 s, then 2 s, doubling up to 30 s, for as long
//! as it takes; whatever arrived meanwhile is printed once it is connected again. Besides `--count`
//! and `--timeout`, only a daemon out of reach for ten minutes ends the watch: one `unreachable`
//! line with the reason, then exit 7, the exit of a command that finds no daemon
//! (docs/design/daemon.md), so whatever runs the watch can start it again. Ankurah's own log lines
//! about a restart are silenced by the CLI's log filter (`run_cli`).

use crate::{notification_with_message, App};
use ankurah::changes::ChangeSet;
use ankurah::signals::{Subscribe, SubscriptionGuard};
use ankurah::{EntityId, LiveQuery};
use anyhow::{anyhow, Result};
use fwd_core::daemon::{self, Autostart, Client};
use fwd_core::model::NotificationView;
use fwd_core::ops;
use fwd_core::project::{Paths, Project};
use fwd_core::query::with_id;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::convert::Infallible;
use std::io::Write;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::{Instant, MissedTickBehavior};

/// How long the watch goes on without reaching the daemon before it gives up.
const OUTAGE_LIMIT: Duration = Duration::from_secs(10 * 60);
/// The wait before the next attempt to connect: it doubles after each failure up to
/// `MAX_BACKOFF`, and returns to `FIRST_BACKOFF` once a connection has lasted `STEADY`.
const FIRST_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
const STEADY: Duration = Duration::from_secs(60);
/// How often the watch looks at its connection.
const CHECK_EVERY: Duration = Duration::from_secs(1);
/// How often the watch asks the daemon for the unread notifications itself.
const RECONCILE_EVERY: Duration = Duration::from_secs(60);
/// The longest the watch waits for the daemon to answer its query, or its request for the unread.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);

/// Prints `who`'s notifications until it has printed `count` of them, or `timeout` has passed when
/// one is given, or the daemon has been out of reach for `OUTAGE_LIMIT`, and ends the process when
/// it is over: after its last line ([`end`]), or after an error (`fail` in main.rs, with exit 1, at
/// once for the reason [`end`] gives). It never returns.
pub async fn run(app: App, who: EntityId, timeout: Option<Duration>, count: Option<u64>) -> Infallible {
    let Err(error) = stream(app, who, timeout, count).await;
    crate::fail(error, 1)
}

/// The watch, which ends the process after its last line ([`end`]): it returns only an error.
async fn stream(app: App, who: EntityId, timeout: Option<Duration>, count: Option<u64>) -> Result<Infallible> {
    // Whom the watch prints for: the principal it acts as, whose own notifications alone end with
    // how to answer them, `--for` naming another or not (`reason::reply_id`).
    let App { paths, project, client, acting: reader, .. } = app;
    // The CLI's watch has the client it connected with; the daemon runs no watch (`mcp`).
    let client = client.ok_or_else(|| anyhow::anyhow!("a watch needs the CLI's connection to the daemon"))?;
    let deadline = timeout.map(|timeout| Instant::now() + timeout);
    let (tx, mut rx) = mpsc::unbounded_channel::<NotificationView>();
    let first = Session::open(client, who, tx.clone()).await?;
    // Every notification printed, the snapshot's included: each counts toward `--count` once, and
    // one a query sends again, or a later session finds again, is not printed twice.
    let mut printed = HashSet::new();
    // What is unread when the watch starts is printed first, as the snapshot. The query was
    // subscribed before the snapshot is taken, so a notification that arrives between the two is
    // printed by one or the other.
    for n in unread(&first, who).await? {
        let (nj, _) = notification_with_message(&first.client.ctx, &n, reader).await?;
        emit(json!({"event": "snapshot", "notification": nj}))?;
        printed.insert(n.id());
    }
    let mut watch = Watch {
        paths,
        project,
        who,
        reader,
        tx,
        session: Some(first),
        printed,
        backoff: FIRST_BACKOFF,
        retry_at: Instant::now(),
        reconcile_at: Instant::now() + RECONCILE_EVERY,
        last_contact: Instant::now(),
        last_failure: String::new(),
    };
    let mut check = tokio::time::interval(CHECK_EVERY);
    check.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        // Checked before each wait, so a snapshot that already reaches the count ends the watch at
        // once.
        if count.is_some_and(|n| watch.printed.len() as u64 >= n) {
            end(json!({"event": "done", "count": watch.printed.len()}), 0);
        }
        tokio::select! {
            // A notification already received is printed before the timeout is.
            biased;
            // Held while there is no session: a notification's message is read through one.
            Some(n) = rx.recv(), if watch.session.is_some() => {
                if watch.printed.insert(n.id()) {
                    let session = watch.session.as_ref().expect("the branch requires a session");
                    print_notification(session, &n, watch.reader).await?;
                }
            }
            () = expiry(deadline) => end(json!({"event": "timeout"}), 0),
            _ = check.tick() => match deadline {
                // A check cut short by the deadline is abandoned; the next turn prints the timeout.
                Some(deadline) => {
                    if let Ok(checked) = tokio::time::timeout_at(deadline, watch.check()).await {
                        checked?;
                    }
                }
                None => watch.check().await?,
            },
        }
    }
}

/// Resolves once the watch's timeout has passed; never, for a watch without one.
async fn expiry(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

/// One connection's worth of watching: an ephemeral node joined to the daemon's, the live query
/// over the principal's unread notifications, and the subscription that feeds the watch. Dropping
/// it closes the socket and lets the node go.
struct Session {
    // Declared first, so dropped first: the subscription and the query hold the client's node.
    _subscription: SubscriptionGuard,
    query: LiveQuery<NotificationView>,
    client: Client,
    opened: Instant,
}

impl Session {
    /// Registers the query with the daemon and subscribes to it. It waits for the daemon's answer,
    /// not only the local store's (empty on a new node), so that what the caller's snapshot does
    /// not hold arrives through the subscription.
    async fn open(client: Client, who: EntityId, tx: mpsc::UnboundedSender<NotificationView>) -> Result<Session> {
        let query = client.ctx.query::<NotificationView>(with_id("recipient = ? AND status = 'unread'", who)?)?;
        tokio::time::timeout(ANSWER_TIMEOUT, query.wait_durable_answered()).await.map_err(|_| {
            anyhow!("the daemon did not answer the watch's query within {}s", ANSWER_TIMEOUT.as_secs())
        })??;
        // A fresh entity arrives as both `added` and `appeared` (docs/design/data-model.md, the
        // Ankurah quirks list); the watch prints each notification once, by id. A batch that grows
        // while it stays unread arrives as `updated`, which is left: its later items wake no one
        // again, and whoever reads it reads them all (docs/design/agents-and-inbox.md, rule 10).
        let subscription = query.subscribe(move |cs: ChangeSet<NotificationView>| {
            for n in cs.added().into_iter().chain(cs.appeared()) {
                let _ = tx.send(n);
            }
        });
        Ok(Session { _subscription: subscription, query, client, opened: Instant::now() })
    }

    /// Why this session can no longer be relied on to deliver, when it cannot: Ankurah gave up on
    /// its query, or the daemon came back on another port, where Ankurah does not look for it.
    fn failure(&self) -> Option<String> {
        if let Some(error) = self.query.error().value() {
            return Some(format!("the watch's query failed: {error}"));
        }
        let now = self.client.moved()?;
        Some(format!("the daemon moved from port {} to port {}", self.client.pf.port, now.port))
    }
}

/// The watch between its snapshot and its end.
struct Watch {
    paths: Paths,
    project: Project,
    who: EntityId,
    /// The principal the watch acts as ([`stream`]).
    reader: Option<EntityId>,
    tx: mpsc::UnboundedSender<NotificationView>,
    session: Option<Session>,
    printed: HashSet<EntityId>,
    backoff: Duration,
    /// When the next attempt to connect may start, while there is no session.
    retry_at: Instant,
    reconcile_at: Instant,
    /// The last moment the watch knew the daemon answered.
    last_contact: Instant,
    /// Why the last session was dropped, or the last attempt to connect failed.
    last_failure: String,
}

impl Watch {
    /// Looks at the connection once: drops a session that can no longer be relied on, connects a
    /// new one once the backoff has passed, asks the daemon for the unread notifications once a
    /// minute, and ends the watch when the daemon has been out of reach for `OUTAGE_LIMIT`.
    async fn check(&mut self) -> Result<()> {
        let now = Instant::now();
        match &self.session {
            None if now >= self.retry_at => self.reconnect().await?,
            None => {}
            Some(session) => match session.failure() {
                Some(reason) => self.drop_session(reason),
                None if session.client.ws.is_connected() => {
                    if session.opened.elapsed() >= STEADY {
                        self.backoff = FIRST_BACKOFF;
                    }
                    self.last_contact = now;
                    if now >= self.reconcile_at {
                        self.reconcile().await?;
                    }
                }
                // Closed, and Ankurah is reconnecting to the same port.
                None => {}
            },
        }
        if self.last_contact.elapsed() >= OUTAGE_LIMIT {
            self.give_up();
        }
        Ok(())
    }

    /// Asks the daemon for the unread notifications and prints any the subscription has not
    /// delivered: a query can stop delivering with no error and the socket open, and this bounds
    /// what that costs to a minute. A daemon that does not answer fails the session.
    async fn reconcile(&mut self) -> Result<()> {
        self.reconcile_at = Instant::now() + RECONCILE_EVERY;
        let Some(session) = &self.session else { return Ok(()) };
        match unread(session, self.who).await {
            Ok(list) => {
                for n in list {
                    if self.printed.insert(n.id()) {
                        tracing::debug!(notification = %n.id(), "the watch's query had not delivered a notification the daemon has");
                        print_notification(session, &n, self.reader).await?;
                    }
                }
            }
            Err(e) => self.drop_session(format!("{e:#}")),
        }
        Ok(())
    }

    /// Connects a new session and prints what arrived while the watch had none.
    async fn reconnect(&mut self) -> Result<()> {
        match reopen(&self.paths, &self.project, self.who, self.tx.clone()).await {
            Ok((session, unread)) => {
                tracing::debug!(port = session.client.port, "the watch is connected again");
                self.last_contact = Instant::now();
                self.reconcile_at = Instant::now() + RECONCILE_EVERY;
                for n in unread {
                    if self.printed.insert(n.id()) {
                        print_notification(&session, &n, self.reader).await?;
                    }
                }
                self.session = Some(session);
            }
            Err(e) => {
                tracing::debug!(error = format!("{e:#}"), "the watch could not connect");
                self.last_failure = format!("{e:#}");
                self.retry_at = Instant::now() + self.backoff;
                self.backoff = (self.backoff * 2).min(MAX_BACKOFF);
            }
        }
        Ok(())
    }

    /// Closes the session (its socket, node and query) and schedules the next attempt.
    fn drop_session(&mut self, reason: String) {
        tracing::debug!(%reason, "the watch drops its connection");
        self.session = None;
        self.last_failure = reason;
        self.retry_at = Instant::now() + self.backoff;
        self.backoff = (self.backoff * 2).min(MAX_BACKOFF);
    }

    /// Ends the watch after `OUTAGE_LIMIT` without the daemon: one line saying so, and exit 7.
    fn give_up(&self) -> ! {
        let reason = match &self.session {
            Some(session) => {
                format!("the connection to the daemon on port {} closed and has not come back", session.client.pf.port)
            }
            None => self.last_failure.clone(),
        };
        let since = fwd_core::now_ms() - self.last_contact.elapsed().as_millis() as i64;
        end(json!({"event": "unreachable", "since": since, "reason": reason}), 7)
    }
}

/// Prints the watch's last line (`done`, `timeout` or `unreachable`) and ends the process with
/// `code` at once, so that the watch exits as soon as it has said it is over.
///
/// Returning through `main` instead would shut the runtime down, which waits for each of its
/// threads, and one can be stuck for good inside Ankurah. When a node the watch has let go of stops
/// (the one it left for the daemon's new port), Ankurah's reactor closes the node's subscriptions
/// on a runtime thread (`SubscriptionState::close`, core/src/reactor/subscription_state.rs at the
/// pinned rev f8e85d2): it holds their state lock while it marks each result set unloaded, which
/// can drop a live query whose unsubscribing asks for the same lock, so the thread waits on
/// itself. A watch that had printed `done` never exited that way twice in thirty runs under load
/// (2026-09-29), and in another run the shutdown cancelled a task of Ankurah's, which logged an
/// error on standard error. Ending the process waits for no thread and cancels no task. The fix
/// inside Ankurah is Daniel's, in his repository.
fn end(event: Value, code: i32) -> ! {
    let mut out = std::io::stdout().lock();
    // A last line that cannot be written has no reader left; the exit status still tells the end.
    let _ = writeln!(out, "{event}").and_then(|()| out.flush());
    std::process::exit(code)
}

/// A session on a new ephemeral node, and the notifications unread as it opened. It never starts
/// a daemon: one stopped on purpose, to install a new build say, stays stopped until something
/// else starts it, and the watch waits for that. A daemon that goes away again before the new node
/// is ready fails the attempt ([`daemon::Lost`]), and the watch tries again after its backoff.
async fn reopen(
    paths: &Paths,
    project: &Project,
    who: EntityId,
    tx: mpsc::UnboundedSender<NotificationView>,
) -> Result<(Session, Vec<NotificationView>)> {
    let client = daemon::ensure_daemon(paths, project, Autostart::Never).await?;
    let session = Session::open(client, who, tx).await?;
    let unread = unread(&session, who).await?;
    Ok((session, unread))
}

/// The principal's unread notifications, as the daemon has them.
async fn unread(session: &Session, who: EntityId) -> Result<Vec<NotificationView>> {
    tokio::time::timeout(ANSWER_TIMEOUT, ops::inbox(&session.client.ctx, who, true))
        .await
        .map_err(|_| anyhow!("the daemon did not list the unread notifications within {}s", ANSWER_TIMEOUT.as_secs()))?
}

async fn print_notification(session: &Session, n: &NotificationView, reader: Option<EntityId>) -> Result<()> {
    let (nj, message) = notification_with_message(&session.client.ctx, n, reader).await?;
    let mut event = json!({"event": "notification", "notification": nj});
    if let Some(message) = message {
        event["message"] = message;
    }
    emit(event)
}

/// One line of the watch's output.
fn emit(event: Value) -> Result<()> {
    println!("{}", serde_json::to_string(&event)?);
    Ok(())
}
