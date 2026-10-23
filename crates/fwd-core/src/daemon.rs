//! The client side of the daemon contract: find or spawn this OS user's one daemon (the `fwd`
//! binary re-executed as `fwd daemon serve`), make sure it hosts the project, and connect to that
//! project's node as an ephemeral Ankurah node. See docs/design/daemon.md, "`ensure_daemon()`".
//!
//! The daemon owns `daemon.lock` (the lifetime lock) for as long as it runs and writes
//! `daemon.ws` (the port file) atomically once it is listening. A client that finds no daemon
//! answering takes `daemon.bootstrap.lock` so only one launcher spawns, re-checks, spawns
//! `fwd daemon serve` detached, and waits for the daemon to answer with a bounded deadline. A
//! port file alone proves nothing; only a daemon that answers on that port as the instance the
//! file names (its `/health`) and lists its projects does.
//!
//! A command never waits for good on a daemon that went away: once its socket to the daemon has
//! closed, it ends with [`Lost`] within [`LOST_AFTER`], or as soon as the daemon has come back on
//! another port ([`Client::while_connected`]). Of the CLI's commands, only `fwd watch` follows
//! the daemon to its new port, joining a new node there (`crates/fwd/src/watch.rs`).
//!
//! Nor does a command that writes report as made what the graph may never have got: a commit its
//! node makes while its socket is closed goes to the node's own store alone, which goes when the
//! command exits, so such a command ends with [`Lost`] whenever its socket closed while it ran
//! ([`Client::while_writing`]).

use crate::project::{Checkout, Paths, Project};
use ankurah::signals::{Subscribe, SubscriptionGuard};
use ankurah::{policy::DEFAULT_CONTEXT, Context, Node, PermissiveAgent};
use ankurah_storage_sqlite::{SqliteConnectionManager, SqliteStorageEngine};
use ankurah_websocket_client::{ConnectionState, WebsocketClient};
use anyhow::{anyhow, bail, Context as _, Result};
use fs4::fs_std::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

/// What the daemon writes when it is listening (`daemon.ws` in the runtime directory, owner-only).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortFile {
    pub port: u16,
    pub pid: u32,
    /// Fresh at every start, so a client can tell a restarted daemon from the one it knew.
    pub instance_id: String,
    pub started: i64,
    /// Where the daemon listens: loopback only, or also the LAN.
    pub bind: Bind,
}

/// Which interfaces the daemon listens on: loopback unless the person asked for the LAN (a
/// phone on the same Wi-Fi) with `fwd daemon start --lan` or `fwd ui --lan`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Bind {
    #[default]
    Loopback,
    Lan,
}

impl std::fmt::Display for Bind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Bind::Loopback => "loopback",
            Bind::Lan => "lan",
        })
    }
}

/// The daemon's settings, kept in `daemon.toml` in the data directory so a restart keeps them:
/// the bind the person chose, and the port the daemon last listened on, which it tries first
/// at its next start so an open page or a phone's bookmark usually keeps working.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonConfig {
    #[serde(default)]
    pub bind: Bind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

impl DaemonConfig {
    /// The settings on disk; the defaults (loopback, any port) when there are none yet.
    pub fn load(paths: &Paths) -> Result<DaemonConfig> {
        let path = paths.daemon_config_path();
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).with_context(|| format!("parsing {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(DaemonConfig::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// Re-reads the file, applies `edit` and writes the result back atomically when it changed.
    /// The CLI edits `bind` before it starts or restarts the daemon, and the daemon edits `port`
    /// once it listens; each re-reads the file first, so neither writes back a stale copy of the
    /// other's field.
    pub fn update(paths: &Paths, edit: impl FnOnce(&mut DaemonConfig)) -> Result<DaemonConfig> {
        let before = DaemonConfig::load(paths)?;
        let mut config = before.clone();
        edit(&mut config);
        if config != before {
            let path = paths.daemon_config_path();
            let text = format!(
                "# fwd's daemon settings. `bind` is loopback or lan (`fwd daemon start --lan | --loopback`);\n# `port` is the port the daemon last listened on, tried first at its next start.\n{}",
                toml::to_string_pretty(&config)?
            );
            let tmp = path.with_extension(format!("toml.{}.tmp", std::process::id()));
            std::fs::write(&tmp, text)?;
            std::fs::rename(&tmp, &path).with_context(|| format!("replacing {}", path.display()))?;
        }
        Ok(config)
    }
}

/// A project as the daemon reports it (`GET /projects`); `error` says why the daemon could not
/// open it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostedProject {
    pub id: Uuid,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A daemon that answered its project list, and the projects it hosts.
#[derive(Debug, Clone)]
pub struct Running {
    pub pf: PortFile,
    pub projects: Vec<HostedProject>,
}

pub fn read_port_file(path: &Path) -> Result<Option<PortFile>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Writes the port file so only its owner can read it and no reader ever sees half of it: a
/// sibling temp file created with mode 0600, synced, then renamed into place.
pub fn write_port_file(path: &Path, pf: &PortFile) -> Result<()> {
    use std::io::Write;
    let bytes = serde_json::to_vec_pretty(pf)?;
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

/// Opens a lock file and tries to take it exclusively without blocking. `Ok(Some(file))` holds
/// the lock until dropped; `Ok(None)` means someone else holds it.
pub fn try_lock(path: &Path) -> Result<Option<File>> {
    let file = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(path)?;
    match file.try_lock_exclusive() {
        Ok(true) => Ok(Some(file)),
        Ok(false) => Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// True when a daemon holds the lifetime lock.
pub fn daemon_alive(paths: &Paths) -> Result<bool> {
    if !paths.lifetime_lock().exists() {
        return Ok(false);
    }
    Ok(try_lock(&paths.lifetime_lock())?.is_none())
}

/// An ephemeral node connected to one project's node in the daemon, plus a context for reads
/// and transactions.
pub struct Client {
    pub node: Node<SqliteStorageEngine, PermissiveAgent>,
    pub ws: WebsocketClient<SqliteStorageEngine, PermissiveAgent>,
    pub ctx: Context,
    pub port: u16,
    /// The port file this client connected through; its port also carries the control routes.
    pub pf: PortFile,
    /// The project whose node this client joined.
    pub project: Uuid,
    /// Where the port file lives, read again to find the daemon after the socket closes
    /// ([`Client::moved`]).
    port_file: PathBuf,
    /// Set when the socket to the daemon closes after the client joined, however soon it comes
    /// back, so that a command that writes knows a commit of its node may have reached no daemon
    /// ([`Client::while_writing`]).
    closed: Arc<AtomicBool>,
    /// Tells `closed` of every change of the socket's state for as long as the client lives.
    _closes: SubscriptionGuard,
}

/// How long a command waits for its socket to the daemon to come back once it has closed, before
/// it ends with [`Lost`]. Ankurah's socket client retries the port it joined 1, 3 and 7 seconds
/// after the socket closes, so a daemon that restarts on the same port within a few seconds lets
/// the command carry on.
pub const LOST_AFTER: Duration = Duration::from_secs(10);
/// How often a command waiting on the daemon looks at its socket.
const LOOK_EVERY: Duration = Duration::from_millis(250);

/// Why a command ended without the daemon it joined: its socket to the daemon closed while it
/// ran. A command never follows the daemon elsewhere (only `fwd watch` does, joining a new node),
/// so the way on is to run it again, which joins the daemon wherever it listens then.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Lost {
    /// The socket closed and has stayed closed for [`LOST_AFTER`].
    #[error(
        "the connection to the daemon on port {port} closed while this command ran and has not come back in {}s; run the command again",
        LOST_AFTER.as_secs()
    )]
    Gone { port: u16 },
    /// The socket closed and the port file names another daemon on another port, which Ankurah's
    /// socket client, retrying only the port it joined, never reaches.
    #[error(
        "the connection to the daemon on port {port} closed while this command ran, and the daemon came back on port {now}; run the command again"
    )]
    Moved { port: u16, now: u16 },
    /// The command failed while its socket was closed, which is what its failure comes from: a
    /// read with no daemon connected fails at once, and so does one the daemon was answering when
    /// the socket closed.
    #[error("the connection to the daemon on port {port} closed while this command ran; run the command again")]
    Closed { port: u16 },
    /// A command that writes found, before it began, that its socket had closed since it joined
    /// the daemon, and began nothing, since a commit made while the socket is closed reaches no
    /// daemon.
    #[error(
        "the connection to the daemon on port {port} closed before this command began to write, and it wrote nothing; run the command again"
    )]
    BeforeWriting { port: u16 },
    /// A command that writes lost its socket at some moment while it ran, whether it then
    /// succeeded or failed: a commit its node made while the socket was closed went to the node's
    /// own store alone, and one made before may have reached the daemon, so neither running it
    /// again nor leaving it is safe without a look at the graph.
    #[error(
        "the connection to the daemon on port {port} closed while this command ran, so what it wrote may not have reached the daemon; look (fwd task show, fwd message show) before writing it again"
    )]
    Unconfirmed { port: u16 },
}

/// The daemon the port file names now, when the socket joined through `joined` is closed and that
/// daemon is another one on another port: Ankurah's socket client retries only the port it joined,
/// so it would never reach it. A daemon that came back on the same port is not one: the socket
/// client finds it there.
fn moved(
    ws: &WebsocketClient<SqliteStorageEngine, PermissiveAgent>,
    joined: &PortFile,
    port_file: &Path,
) -> Option<PortFile> {
    if ws.is_connected() {
        return None;
    }
    read_port_file(port_file).ok().flatten().filter(|pf| pf.instance_id != joined.instance_id && pf.port != joined.port)
}

/// Resolves once the socket joined through `joined` has been closed for [`LOST_AFTER`], or as soon
/// as it is closed and the daemon has [`moved`]; never while the socket stays open.
async fn lost(ws: &WebsocketClient<SqliteStorageEngine, PermissiveAgent>, joined: &PortFile, port_file: &Path) -> Lost {
    let mut look = tokio::time::interval(LOOK_EVERY);
    look.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut closed_since = None;
    loop {
        look.tick().await;
        if ws.is_connected() {
            closed_since = None;
            continue;
        }
        if let Some(now) = moved(ws, joined, port_file) {
            return Lost::Moved { port: joined.port, now: now.port };
        }
        if closed_since.get_or_insert_with(Instant::now).elapsed() >= LOST_AFTER {
            return Lost::Gone { port: joined.port };
        }
    }
}

impl Client {
    /// The daemon the port file names now, when this client's socket is closed and that daemon is
    /// another one on another port, where Ankurah's socket client does not look for it.
    pub fn moved(&self) -> Option<PortFile> {
        moved(&self.ws, &self.pf, &self.port_file)
    }

    /// Runs `work`, a command's use of this client, and ends it with [`Lost`] when the daemon goes
    /// away meanwhile: once the socket has been closed for [`LOST_AFTER`], or as soon as it is
    /// closed and the daemon came back on another port. A daemon that comes back on the same port
    /// sooner lets `work` carry on. An error `work` ends with while the socket is closed is that
    /// loss's doing, and ends it as [`Lost`] too, with the error as its cause.
    pub async fn while_connected<T>(&self, work: impl Future<Output = Result<T>>) -> Result<T> {
        match self.watching(work).await {
            Ok(Err(error)) if !self.ws.is_connected() => {
                let lost = match self.moved() {
                    Some(now) => Lost::Moved { port: self.pf.port, now: now.port },
                    None => Lost::Closed { port: self.pf.port },
                };
                Err(error.context(lost))
            }
            Ok(done) => done,
            Err(lost) => Err(lost.into()),
        }
    }

    /// Runs `work`, the use of this client by a command that writes, so that the command never
    /// reports as made a write the daemon may not have got: Ankurah relays a commit to the durable
    /// peers the node has, and with the socket closed the node has none, so the commit goes to the
    /// node's own store alone and succeeds. `work` does not begin when the socket has closed since
    /// the client joined ([`Lost::BeforeWriting`]). Once it has begun, a socket that closed at any
    /// moment while it ran ends it with [`Lost::Unconfirmed`]: when it succeeded, when it failed
    /// (its error then the cause), and when the daemon went away before it ended, within the bound
    /// of [`Client::while_connected`]. What became of the daemon, which [`Lost::Gone`] and
    /// [`Lost::Moved`] tell a command that reads, is left out on purpose: the next command finds
    /// the daemon wherever it is, so it changes nothing the writer does next.
    pub async fn while_writing<T>(&self, work: impl Future<Output = Result<T>>) -> Result<T> {
        let port = self.pf.port;
        if self.has_closed() {
            return Err(Lost::BeforeWriting { port }.into());
        }
        match self.watching(work).await {
            Ok(done) if !self.has_closed() => done,
            Ok(Err(error)) => Err(error.context(Lost::Unconfirmed { port })),
            Ok(Ok(_)) | Err(_) => Err(Lost::Unconfirmed { port }.into()),
        }
    }

    /// Runs `work` until it ends, or until the daemon has gone away ([`lost`]), whichever comes
    /// first.
    async fn watching<T>(&self, work: impl Future<Output = Result<T>>) -> Result<Result<T>, Lost> {
        tokio::select! {
            biased;
            done = work => Ok(done),
            lost = lost(&self.ws, &self.pf, &self.port_file) => Err(lost),
        }
    }

    /// Whether the socket to the daemon has closed at some moment since the client joined, come
    /// back or not. Ankurah's socket client marks the socket closed (`is_connected`) before it
    /// takes the daemon off the node's durable peers, and announces the close on its state signal,
    /// which sets `closed`, after that but before it opens the socket again: so once a commit has
    /// found no durable peer, one of the two says so from then on.
    fn has_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire) || !self.ws.is_connected()
    }
}

/// The base URL a client hands Ankurah's connector for one project (the connector appends
/// `/ws` itself).
pub fn socket_base(pf: &PortFile, project: Uuid) -> String {
    format!("ws://127.0.0.1:{}/p/{project}", pf.port)
}

/// The ephemeral node's store: SQLite in memory, whose database lives only as long as the one
/// connection that opened it, so this pool never closes that connection. The node keeps the
/// entities its queries hold in memory beside the store, and the two have to agree for as long as
/// the node runs. `SqliteStorageEngine::open_in_memory` builds the same one-connection pool with
/// bb8's defaults, which close a connection idle for ten minutes or open for thirty; the next use
/// then opens an empty database. A `fwd watch` that had waited ten minutes lost its store that way:
/// with the daemon up, the next notification failed to apply ("no such table") and never reached
/// it; after a daemon restart, the daemon resent every row of its queries, every row failed to
/// apply (a write conflict against the row the store no longer had, retried until Ankurah gave up
/// with "TOCTOU attempts exhausted"), and the queries stopped for good. Once Ankurah's own
/// constructor keeps its connection, this can call it again.
async fn ephemeral_store() -> Result<SqliteStorageEngine> {
    let pool = bb8::Pool::builder()
        .max_size(1)
        .idle_timeout(None)
        .max_lifetime(None)
        // A connection that failed the check would be replaced, and the database with it.
        .test_on_check_out(false)
        .build(SqliteConnectionManager::memory())
        .await?;
    SqliteStorageEngine::new(pool).await
}

/// Joins a new ephemeral node to `project`'s node in the daemon `pf` names, and waits until it is
/// ready. The node is ready once the daemon has answered its first queries (Ankurah's catalog of
/// the project's models), for as long as that takes while the socket is open; a daemon that stops
/// before answering them ends the wait with [`Lost`], within [`LOST_AFTER`]. Until 2026-09-30 that
/// wait had no end: the socket client retried the old port and the queries waited for a daemon
/// that had moved elsewhere, and a command caught there never ended (task tXSgMMXC). From the
/// moment the node is ready, the client records every close of its socket, for a command that
/// writes ([`Client::while_writing`]).
pub async fn connect(paths: &Paths, pf: &PortFile, project: &Project, timeout: Duration) -> Result<Client> {
    let port = pf.port;
    let storage = ephemeral_store().await?;
    let node = Node::new(Arc::new(storage), PermissiveAgent::new());
    let url = socket_base(pf, project.id);
    let ws = WebsocketClient::new(node.clone(), &url)
        .await
        .with_context(|| format!("connecting to project {} on port {port}", project.name))?;
    tokio::time::timeout(timeout, ws.wait_connected())
        .await
        .map_err(|_| anyhow!("timed out connecting to the daemon on port {port}"))?
        .map_err(|e| anyhow!("daemon connection failed: {e}"))?;
    tokio::time::timeout(timeout, node.system.wait_system_ready())
        .await
        .map_err(|_| anyhow!("timed out waiting for project {}'s system on port {port}", project.name))?
        .map_err(|e| anyhow!("project {}'s system is not ready: {e}", project.name))?;
    let port_file = paths.port_file();
    let ctx = tokio::select! {
        biased;
        ctx = node.context_async(DEFAULT_CONTEXT) => ctx?,
        lost = lost(&ws, pf, &port_file) => return Err(lost.into()),
    };
    // From here the command may write, so every close of the socket from now on is recorded,
    // including one that came before the subscription did.
    let closed = Arc::new(AtomicBool::new(false));
    let told = closed.clone();
    let _closes = ws.state().subscribe(move |state: ConnectionState| {
        if !matches!(state, ConnectionState::Connected { .. }) {
            told.store(true, Ordering::Release);
        }
    });
    if !ws.is_connected() {
        closed.store(true, Ordering::Release);
    }
    Ok(Client { node, ws, ctx, port, pf: pf.clone(), project: project.id, port_file, closed, _closes })
}

#[derive(Debug, Clone)]
pub enum Autostart {
    Never,
    Spawn { daemon_bin: PathBuf },
}

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const START_DEADLINE: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(100);

/// The shared startup routine: the daemon running (spawned when needed), the project hosted in
/// it (opened on request when the daemon started before the project existed), and a client
/// connected to the project's node. Never searches PATH: the caller passes the binary to
/// re-execute (normally its own).
pub async fn ensure_daemon(paths: &Paths, project: &Project, autostart: Autostart) -> Result<Client> {
    let running = ensure_running(paths, &autostart).await?;
    if !running.projects.iter().any(|p| p.id == project.id && p.error.is_none()) {
        host(&running.pf, project).await?;
    }
    connect(paths, &running.pf, project, CONNECT_TIMEOUT).await
}

/// The daemon answering, spawned first when none is. Serializes launchers on the bootstrap
/// lock; whoever wins spawns, everyone else waits for the same daemon.
pub async fn ensure_running(paths: &Paths, autostart: &Autostart) -> Result<Running> {
    paths.ensure()?;
    if let Some(running) = answering(paths).await? {
        return Ok(running);
    }
    let Autostart::Spawn { daemon_bin } = autostart else {
        bail!("no daemon is running and autostart is off");
    };
    let _bootstrap = wait_for_lock(&paths.bootstrap_lock(), START_DEADLINE).await?;
    if let Some(running) = answering(paths).await? {
        return Ok(running);
    }
    // Holding the lifetime lock is the proof that no daemon runs: only then may the stale port
    // file go, and only then is spawning right. A daemon that holds the lock but does not answer
    // is starting (it opens every project before it listens), exiting (it holds the lock until
    // its process ends), or wedged; spawning another would only make the newcomer exit, so this
    // waits, and spawns once the lock comes free without an answer. It spawns at most once, so a
    // daemon that fails at start is reported rather than retried. Pids are never killed on the
    // strength of a file.
    let mut spawned = false;
    let deadline = Instant::now() + START_DEADLINE;
    loop {
        if let Some(running) = answering(paths).await? {
            return Ok(running);
        }
        if !spawned {
            if let Some(lock) = try_lock(&paths.lifetime_lock())? {
                let _ = std::fs::remove_file(paths.port_file());
                drop(lock);
                spawn_daemon(paths, daemon_bin)?;
                spawned = true;
            }
        }
        if Instant::now() >= deadline {
            bail!(
                "no daemon answered within {}s (one may still be opening its projects, or be stuck before it listens); see {}",
                START_DEADLINE.as_secs(),
                paths.log_path().display()
            );
        }
        tokio::time::sleep(POLL).await;
    }
}

/// The daemon the port file names, when the process on the file's port is that daemon (its
/// `/health` carries the file's instance id and pid) and lists its projects. Anything short of
/// that counts as no daemon: no file, nothing listening, or, after a crash left the file behind,
/// another program or another daemon (a scratch one, another user's) on that port now.
async fn answering(paths: &Paths) -> Result<Option<Running>> {
    let Some(pf) = read_port_file(&paths.port_file())? else { return Ok(None) };
    let answer = async {
        identify(&pf).await?;
        hosted_projects(&pf).await
    };
    match answer.await {
        Ok(projects) => Ok(Some(Running { pf, projects })),
        Err(e) => {
            tracing::debug!(port = pf.port, error = %e, "port file present but its daemon did not answer");
            Ok(None)
        }
    }
}

/// What a daemon says about itself on `/health`: enough to tell the daemon a port file names
/// from anything else listening on that port.
#[derive(Deserialize)]
struct Health {
    pid: u32,
    instance_id: String,
}

/// Succeeds when the process listening on the port file's port is the daemon the file names.
async fn identify(pf: &PortFile) -> Result<()> {
    let health: Health = match http(pf, "GET", "/health", None, CONNECT_TIMEOUT).await? {
        Some((200, body)) => serde_json::from_str(&body).context("parsing the daemon's health check")?,
        Some((status, body)) => bail!("the health check was refused ({status}): {}", first_line(&body)),
        None => bail!("nothing is listening on port {}", pf.port),
    };
    if health.instance_id != pf.instance_id || health.pid != pf.pid {
        bail!(
            "port {} answers as daemon {} (pid {}), not as {} (pid {}), which the port file names",
            pf.port,
            health.instance_id,
            health.pid,
            pf.instance_id,
            pf.pid
        );
    }
    Ok(())
}

async fn hosted_projects(pf: &PortFile) -> Result<Vec<HostedProject>> {
    match http(pf, "GET", "/projects", None, CONNECT_TIMEOUT).await? {
        Some((200, body)) => Ok(serde_json::from_str(&body).context("parsing the daemon's project list")?),
        Some((status, body)) => bail!("the daemon refused the project list ({status}): {}", first_line(&body)),
        None => bail!("nothing is listening on port {}", pf.port),
    }
}

/// Asks the daemon to open a project from the index that it does not host: one created after
/// the daemon started (`fwd init`), or one that failed to open. The daemon answers once the
/// project's node is ready, or with the reason it could not open it.
pub async fn host(pf: &PortFile, project: &Project) -> Result<()> {
    let body = serde_json::json!({ "id": project.id }).to_string();
    let answer = match http(pf, "POST", "/projects", Some(&body), START_DEADLINE).await {
        Err(e) if e.is::<NoAnswer>() => {
            bail!("the daemon is still opening project {} ({}); try again", project.name, project.id)
        }
        answer => answer?,
    };
    match answer {
        Some((200, _)) => Ok(()),
        Some((_, body)) => {
            let reason = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v["error"].as_str().map(str::to_string))
                .unwrap_or_else(|| first_line(&body).to_string());
            bail!("the daemon could not open project {} ({}): {reason}", project.name, project.id)
        }
        None => bail!("no daemon answered on port {}", pf.port),
    }
}

async fn wait_for_lock(path: &Path, deadline: Duration) -> Result<File> {
    let until = Instant::now() + deadline;
    loop {
        if let Some(file) = try_lock(path)? {
            return Ok(file);
        }
        if Instant::now() >= until {
            bail!("timed out waiting for the bootstrap lock at {}", path.display());
        }
        tokio::time::sleep(POLL).await;
    }
}

fn spawn_daemon(paths: &Paths, daemon_bin: &Path) -> Result<()> {
    let log = OpenOptions::new().create(true).append(true).open(paths.log_path())?;
    let mut cmd = std::process::Command::new(daemon_bin);
    cmd.arg("daemon")
        .arg("serve")
        .arg("--runtime-dir")
        .arg(&paths.runtime_dir)
        .arg("--data-dir")
        .arg(&paths.data_dir)
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own process group: a Ctrl-C in the launching terminal must not take the daemon.
        cmd.process_group(0);
    }
    let mut child = cmd.spawn().with_context(|| format!("spawning {}", daemon_bin.display()))?;
    // A launcher that outlives the daemon (a test, or a command that restarts it) reaps it, so
    // its pid is gone once it exits, which is what `stop` waits for. A launcher that exits first
    // leaves that to the system.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// The daemon and the projects it hosts, when it answers; never autostarts.
pub async fn status(paths: &Paths) -> Result<Option<Running>> {
    answering(paths).await
}

/// Asks the running daemon to drain and exit; returns false when none was running. Waits for
/// its process to end, not for the lifetime lock to come free: the moment the lock is free,
/// another command may start a successor that takes it, and probing the lock could take it
/// from a daemon that is starting.
pub async fn stop(paths: &Paths) -> Result<bool> {
    let Some(Running { pf, .. }) = answering(paths).await? else { return Ok(false) };
    let Some((status, body)) = http(&pf, "POST", "/stop", None, CONNECT_TIMEOUT).await? else { return Ok(false) };
    if status != 200 {
        bail!("the daemon refused to stop: {}", first_line(&body));
    }
    let deadline = Instant::now() + START_DEADLINE;
    while process_exists(pf.pid) {
        if Instant::now() >= deadline {
            bail!(
                "the daemon (pid {}) acknowledged the stop but has not exited within {}s",
                pf.pid,
                START_DEADLINE.as_secs()
            );
        }
        tokio::time::sleep(POLL).await;
    }
    Ok(true)
}

/// True while a process with this pid exists. Signal 0 sends nothing: `kill` only reports
/// whether the pid could be signalled, and only "no such process" (ESRCH) means it is gone
/// (EPERM means it exists under another user).
fn process_exists(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else { return false };
    // SAFETY: kill with signal 0 has no preconditions and no effect on the target process.
    if unsafe { libc::kill(pid, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

/// What `set_bind` changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BindSwitch {
    /// `daemon.toml` named the other bind before.
    pub changed: bool,
    /// A daemon was running with the other bind and was stopped; the next start listens as
    /// asked.
    pub stopped: bool,
}

/// Records the bind the person chose in `daemon.toml` and returns the daemon listening that
/// way: a running daemon that listens the other way is stopped and the next one started. A
/// daemon that was starting when the setting changed has read the old bind and answers only
/// afterwards, so the answering daemon's bind is checked again and it is restarted once more
/// when it differs. No data moves: the new daemon opens the same projects.
pub async fn set_bind(paths: &Paths, bind: Bind, autostart: &Autostart) -> Result<(BindSwitch, Running)> {
    paths.ensure()?;
    let changed = DaemonConfig::load(paths)?.bind != bind;
    if changed {
        DaemonConfig::update(paths, |config| config.bind = bind)?;
    }
    let mut stopped = match status(paths).await? {
        Some(running) if running.pf.bind != bind => stop(paths).await?,
        _ => false,
    };
    let mut running = ensure_running(paths, autostart).await?;
    if running.pf.bind != bind {
        stopped |= stop(paths).await?;
        running = ensure_running(paths, autostart).await?;
        if running.pf.bind != bind {
            bail!(
                "the daemon still listens on {} after a restart: another command set the bind back meanwhile ({})",
                running.pf.bind,
                paths.daemon_config_path().display()
            );
        }
    }
    Ok((BindSwitch { changed, stopped }, running))
}

/// A checkout as the daemon's registry records it: local-only knowledge, never in the graph.
/// `scope` is the scope a session bound to this checkout with `fwd scope attach-dir`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckoutRecord {
    pub root: PathBuf,
    pub upstream: Option<String>,
    pub branch: Option<String>,
    pub head: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    /// True to drop the scope binding (`scope attach-dir --clear`); a report without a scope
    /// otherwise keeps whatever binding the daemon has.
    #[serde(default)]
    pub clear_scope: bool,
    #[serde(default)]
    pub seen: i64,
}

impl From<&Checkout> for CheckoutRecord {
    fn from(c: &Checkout) -> Self {
        CheckoutRecord {
            root: c.root.clone(),
            upstream: c.upstream.clone(),
            branch: c.branch.clone(),
            head: c.head.clone(),
            scope: None,
            clear_scope: false,
            seen: crate::now_ms(),
        }
    }
}

fn checkouts_route(client: &Client) -> String {
    format!("/p/{}/checkouts", client.project)
}

/// Tells the daemon about a checkout of the client's project (the caller's, or one being bound
/// to a scope). The registry is a convenience, so callers reporting their own checkout may
/// ignore a failure; `scope attach-dir` must not.
pub async fn report_checkout(client: &Client, record: &CheckoutRecord) -> Result<()> {
    let body = serde_json::to_string(record)?;
    match http(&client.pf, "POST", &checkouts_route(client), Some(&body), CONNECT_TIMEOUT).await? {
        Some((200, _)) => Ok(()),
        Some((status, body)) => {
            bail!("the daemon refused the checkout report ({status}): {}", first_line(&body))
        }
        None => bail!("no daemon answered on port {}", client.pf.port),
    }
}

/// The daemon's registry of checkouts for the client's project.
pub async fn checkouts(client: &Client) -> Result<Vec<CheckoutRecord>> {
    match http(&client.pf, "GET", &checkouts_route(client), None, CONNECT_TIMEOUT).await? {
        Some((200, body)) => Ok(serde_json::from_str(&body)?),
        Some((status, body)) => {
            bail!("the daemon refused the checkout list ({status}): {}", first_line(&body))
        }
        None => bail!("no daemon answered on port {}", client.pf.port),
    }
}

fn first_line(body: &str) -> &str {
    body.lines().next().unwrap_or("")
}

/// A request the daemon accepted but did not answer before the caller's deadline.
#[derive(Debug, thiserror::Error)]
#[error("the daemon did not answer {method} {path} on port {port} within {}s", .timeout.as_secs())]
struct NoAnswer {
    method: String,
    path: String,
    port: u16,
    timeout: Duration,
}

/// One HTTP/1.1 request on the daemon's listener over loopback, without an HTTP client crate:
/// the routes besides the Ankurah sockets are local control, not data. The Host carries the
/// port (the daemon checks it), and the request comes from loopback, which the control routes
/// require. `None` when nothing is listening; a `NoAnswer` error when the answer does not
/// arrive within `timeout`.
async fn http(
    pf: &PortFile,
    method: &str,
    path: &str,
    body: Option<&str>,
    timeout: Duration,
) -> Result<Option<(u16, String)>> {
    let port = pf.port;
    let mut stream = match tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
        Ok(s) => s,
        Err(_) => return Ok(None),
    };
    let body = body.unwrap_or("");
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await?;
    let mut response = Vec::new();
    if tokio::time::timeout(timeout, stream.read_to_end(&mut response)).await.is_err() {
        return Err(NoAnswer { method: method.into(), path: path.into(), port, timeout }.into());
    }
    let text = String::from_utf8_lossy(&response).into_owned();
    let status: u16 = text.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    Ok(Some((status, body)))
}
