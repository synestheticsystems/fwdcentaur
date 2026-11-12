//! `fwd daemon serve`: the daemon side of the contract. The CLI re-executes its own binary with
//! this hidden subcommand (tmux, watchman, sccache and Caddy do the same), so there is one
//! artifact to install and client and daemon are always the same build. One daemon per OS user
//! keeps every project in the index in one durable node and one store (docs/DECISIONS.md, D135),
//! where each record names its project, and hosts each project by its graph over that node
//! (`fwd_core::project_graph`). See docs/design/daemon.md.
//!
//! Besides each project's Ankurah WebSocket, the listener carries local-control routes (the
//! health check, the project list, each project's checkout registry, stop), the MCP endpoint at
//! `/mcp` (crates/fwd/src/mcp.rs), and the web UI's files. None of the control routes is a data
//! path: every read and write of a graph goes through Ankurah, the MCP endpoint's through the
//! project's graph over the daemon's own durable node, by the same operations the CLI's commands
//! run. Every project's socket joins the one node, and its path names the project its client
//! works in (`fwd_core::daemon_socket`); nothing at the socket keeps a client to that project yet,
//! which Ankurah's policy hooks are to do (docs/design/daemon.md, "Projects in one node"). A
//! client built for the daemons that kept a store per project, whose reads name no project, is
//! refused at the path those daemons served their sockets at.
//!
//! Who may talk to it: the daemon listens on loopback unless the person chose the LAN
//! (`daemon.toml`). Every request must carry one of the daemon's own `Host` names and, when a
//! browser sends one, one of its own `Origin`s or the origin of fwd's own VS Code webview, which
//! the extension registers before the page there loads; so a page from another site, a
//! DNS-rebinding page, or another extension's webview is refused. The control routes that change
//! what the daemon hosts, stop it, admit a webview, or read and write the checkout registry
//! answer only a process on this machine that is not a web page, and so does the MCP endpoint, which
//! asks for no login: a LAN bind does not expose it. No secret gates the sockets: on
//! a LAN bind every device on that network can read and write the graphs until the auth pass
//! (the JWT extension at the Ankurah layer).

use ankurah::policy::DEFAULT_CONTEXT;
use ankurah::{Context as NodeContext, Node, PermissiveAgent};
use ankurah_storage_sqlite::SqliteStorageEngine;
use ankurah_websocket_server::WebsocketServer;
use anyhow::{anyhow, bail, Context, Result};
use axum::extract::{ConnectInfo, Path as RoutePath, Request, State};
use axum::handler::Handler;
use axum::http::{header, HeaderName, HeaderValue, Method, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::{routing::get, routing::post, Json, Router};
use fwd_core::daemon::{try_lock, write_port_file, Bind, CheckoutRecord, DaemonConfig, HostedProject, PortFile};
use fwd_core::daemon_socket;
use fwd_core::project::{self, Index, NamedProject, Paths};
use fwd_core::project_graph::ProjectGraph;
use include_dir::{include_dir, Dir};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tracing::{info, warn};
use uuid::Uuid;

use crate::dev_runner;
use crate::mcp;
use crate::services;
use crate::BUILD;

#[derive(Clone)]
struct AppState {
    paths: Arc<Paths>,
    shutdown: Arc<Notify>,
    info: Arc<PortFile>,
    /// The node that holds every project.
    node: Arc<DaemonNode>,
    /// Every project in the index, open or with the reason it could not open.
    projects: Arc<RwLock<BTreeMap<Uuid, Slot>>>,
    /// Held while a project opens after start, so two requests for one project never make its
    /// record twice.
    opening: Arc<tokio::sync::Mutex<()>>,
    allowed: Arc<RwLock<Allowed>>,
    /// The daemon's dev-runner entry: its status file counts the projects open, including those
    /// opened after start.
    dev_runner: Arc<dev_runner::Entry>,
}

impl AppState {
    fn new(
        paths: &Paths,
        info: Arc<PortFile>,
        node: Arc<DaemonNode>,
        projects: BTreeMap<Uuid, Slot>,
        dev_runner: Arc<dev_runner::Entry>,
    ) -> AppState {
        AppState {
            paths: Arc::new(paths.clone()),
            shutdown: Arc::new(Notify::new()),
            allowed: Arc::new(RwLock::new(Allowed::list(info.bind, info.port))),
            info,
            node,
            projects: Arc::new(RwLock::new(projects)),
            opening: Arc::new(tokio::sync::Mutex::new(())),
            dev_runner,
        }
    }
}

/// The `Host` values this listener answers to, and the `Origin`s a browser may present: the
/// daemon's own names (`own_hosts`), plus a development origin from `FWD_DEV_ORIGIN` (a trunk
/// dev server proxying to this daemon), plus the origins of fwd's own VS Code webviews, which the
/// extension registers (`POST /webview-origins`). A laptop that moves to another network gets new
/// addresses, so on a LAN bind `local_only` lists them again when a request names a host the
/// list lacks; the registered webviews stay.
struct Allowed {
    hosts: Vec<String>,
    origins: Vec<String>,
    /// The webview origins registered since the daemon started, oldest first, at most
    /// `WEBVIEW_ORIGINS` of them.
    webviews: VecDeque<String>,
    /// The earliest moment the interfaces may be listed again, so requests that name unknown
    /// hosts cannot keep the daemon listing them.
    relist_after: Instant,
}

/// How often, at most, a daemon on the LAN lists its interfaces again.
const RELIST_INTERVAL: Duration = Duration::from_secs(5);

/// How many webview origins the daemon keeps; registering one more drops the oldest. Every
/// window's panel registers one, again each time its page loads, so the ones in use stay recent.
const WEBVIEW_ORIGINS: usize = 64;

impl Allowed {
    fn list(bind: Bind, port: u16) -> Allowed {
        let (hosts, origins) = Allowed::own(bind, port);
        Allowed { hosts, origins, webviews: VecDeque::new(), relist_after: Instant::now() + RELIST_INTERVAL }
    }

    /// The daemon's own hosts and origins, as its interfaces are now.
    fn own(bind: Bind, port: u16) -> (Vec<String>, Vec<String>) {
        let mut hosts = own_hosts(bind, port);
        let mut origins = hosts.iter().map(|h| format!("http://{h}")).collect::<Vec<_>>();
        if let Ok(dev) = std::env::var("FWD_DEV_ORIGIN") {
            if let Some(host) = dev.split("://").nth(1) {
                hosts.push(host.trim_end_matches('/').to_string());
            }
            origins.push(dev.trim_end_matches('/').to_string());
        }
        (hosts, origins)
    }

    /// Lists the interfaces again, keeping the registered webviews.
    fn relist(&mut self, bind: Bind, port: u16) {
        (self.hosts, self.origins) = Allowed::own(bind, port);
        self.relist_after = Instant::now() + RELIST_INTERVAL;
    }

    /// A request names one of the daemon's own hosts and, when a browser sent an origin, one of
    /// its own origins or a registered webview's.
    fn admits(&self, host: Option<&str>, origin: Option<&str>) -> bool {
        let host_ok = host.is_some_and(|h| self.hosts.iter().any(|a| a.eq_ignore_ascii_case(h)));
        let origin_ok =
            origin.is_none_or(|o| self.origins.iter().chain(&self.webviews).any(|a| a.eq_ignore_ascii_case(o)));
        host_ok && origin_ok
    }

    /// Admits a webview's origin as the newest, dropping the oldest beyond `WEBVIEW_ORIGINS`.
    fn admit_webview(&mut self, origin: &str) {
        self.webviews.retain(|o| !o.eq_ignore_ascii_case(origin));
        self.webviews.push_back(origin.to_string());
        while self.webviews.len() > WEBVIEW_ORIGINS {
            self.webviews.pop_front();
        }
    }
}

/// True for a VS Code webview's origin and nothing more: `vscode-webview://` and the webview's
/// id, with no path, port or user. The extension runs the web UI in a webview from its own files,
/// so the page reaches the daemon from that origin (docs/design/vscode-extension.md). Any
/// extension's webview has such an origin, and a webview's page (a Markdown preview with scripts,
/// a notebook's output) runs whatever script it was given, so the daemon admits only the origins
/// the extension registers for its own panel.
fn webview_origin(origin: &str) -> bool {
    const SCHEME: &str = "vscode-webview://";
    let (Some(scheme), Some(id)) = (origin.get(..SCHEME.len()), origin.get(SCHEME.len()..)) else { return false };
    scheme.eq_ignore_ascii_case(SCHEME)
        && (1..=128).contains(&id.len())
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_'))
}

enum Slot {
    Open(Arc<Hosted>),
    Failed { project: NamedProject, error: String },
}

impl Slot {
    fn report(&self) -> HostedProject {
        match self {
            Slot::Open(hosted) => {
                HostedProject { id: hosted.project.id, name: hosted.project.name.clone(), error: None }
            }
            Slot::Failed { project, error } => {
                HostedProject { id: project.id, name: project.name.clone(), error: Some(error.clone()) }
            }
        }
    }
}

/// Upgrades a request to the daemon's Ankurah WebSocket.
type Socket = Box<dyn Fn(Request) -> Pin<Box<dyn Future<Output = Response> + Send>> + Send + Sync>;

/// The daemon's one durable node, over the store that holds every project's records, and the
/// socket every project's clients join it through.
struct DaemonNode {
    /// A context on the node, from which each hosted project's graph is made.
    ctx: NodeContext,
    socket: Socket,
    _node: Node<SqliteStorageEngine, PermissiveAgent>,
}

impl DaemonNode {
    /// Opens the node over `store` (made on first use), with its Ankurah system (created on first
    /// use), and readies the socket clients join it through. Runs under the lifetime lock.
    async fn open(store: SqliteStorageEngine) -> Result<DaemonNode> {
        let node = Node::new_durable(Arc::new(store), PermissiveAgent::new());
        node.system.wait_loaded().await?;
        if node.system.root().is_none() {
            info!("creating the Ankurah system");
            node.system.create().await?;
        }
        node.system.wait_system_ready().await.map_err(|e| anyhow!("the Ankurah system did not become ready: {e}"))?;
        let ctx = node.context_async(DEFAULT_CONTEXT).await?;
        let upgrade = WebsocketServer::new(node.clone()).route_handler();
        let socket: Socket = Box::new(move |req| {
            let upgrade = upgrade.clone();
            Box::pin(async move { upgrade.call(req, ()).await })
        });
        Ok(DaemonNode { ctx, socket, _node: node })
    }
}

/// One project the daemon hosts: its graph over the daemon's node, with the routing service over
/// it, and the checkouts of it seen on this machine.
struct Hosted {
    project: NamedProject,
    registry: Mutex<Registry>,
    registry_path: PathBuf,
    /// The project's graph, which the MCP endpoint runs its calls through.
    graph: ProjectGraph,
    _routing: services::Routing,
}

/// Why the daemon does not host a project whose records are still in a store of the project's
/// own, from before the daemon kept every project in one store: it would show the project empty
/// over records it cannot see, and the two would part from then on.
#[derive(Debug, thiserror::Error)]
#[error("project {name}'s records are still in a store of its own, {}, which this build of fwd does not read; {MOVE_THEM}", .store.display())]
struct Unmoved {
    name: String,
    store: PathBuf,
}

/// Refuses to start over a data folder whose every project still has its records in a store of its
/// own and which has no store of the daemon's yet: the store the daemon would make there could host
/// none of them, and the folder is left as the per-project build wrote it, for that build or for
/// fwd-store-merge.
fn refuse_a_folder_not_moved(paths: &Paths, index: &Index) -> Result<()> {
    if paths.store_path().exists() || index.projects.is_empty() {
        return Ok(());
    }
    let unmoved: Vec<PathBuf> =
        index.projects.iter().filter_map(|entry| paths.unmoved_store(&NamedProject::from(entry))).collect();
    if unmoved.len() < index.projects.len() {
        return Ok(());
    }
    bail!(
        "every project's records are still in a store of its own ({}), and this build of fwd does not read those; {MOVE_THEM}",
        unmoved.iter().map(|path| path.display().to_string()).collect::<Vec<_>>().join(", ")
    )
}

/// What brings a project's own store into the daemon's one store, for the refusals here to name.
const MOVE_THEM: &str = "it opens once fwd-store-merge has moved every project's store into the daemon's one store \
    (crates/fwd-store-merge/README.md says how to switch a machine over)";

/// The checkouts of a project seen on this machine: local-only, persisted beside the database so
/// it survives a restart, refreshed from git when a client reports.
#[derive(Default)]
struct Registry {
    checkouts: Vec<CheckoutRecord>,
    /// When each root's sibling worktrees were last scanned; a scan costs a few git processes
    /// per worktree, so it is not repeated for every command.
    expanded: HashMap<PathBuf, Instant>,
}

const EXPANSION_INTERVAL: Duration = Duration::from_secs(60);

/// The web UI's built files (crates/fwd-web/dist, produced by scripts/build-web.sh). Empty when
/// the UI was not built before this binary; `web` then serves the instructions instead.
static WEB: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../fwd-web/dist");

pub async fn serve(paths: Paths) -> Result<()> {
    paths.ensure()?;

    // The lifetime lock is held for the whole run; a second daemon exits quietly (its launcher
    // connects to the winner). Only under it may the store open.
    let Some(lifetime) = try_lock(&paths.lifetime_lock())? else {
        info!("another daemon holds the lifetime lock; exiting");
        return Ok(());
    };
    let config = DaemonConfig::load(&paths)?;
    let index = Index::load(&paths)?;
    refuse_a_folder_not_moved(&paths, &index)?;

    // The daemon's dev-runner entry, first cleared of what a crashed daemon for this data directory
    // left. Only under the lifetime lock: without it, those files could be a running daemon's.
    let dev_runner = Arc::new(dev_runner::Entry::new(dev_runner::registry(), &paths.data_dir));
    dev_runner.clear();
    dev_runner.starting();

    let store = SqliteStorageEngine::open(paths.store_path()).await.context("opening the daemon's store")?;
    let node = Arc::new(DaemonNode::open(store).await?);
    let mut projects = BTreeMap::new();
    for entry in &index.projects {
        let project = NamedProject::from(entry);
        projects.insert(project.id, open_slot(&paths, &node, project).await);
    }

    let listener = listen(config.bind, config.port).await?;
    let port = listener.local_addr()?.port();
    if config.port != Some(port) {
        if let Err(e) = DaemonConfig::update(&paths, |c| c.port = Some(port)) {
            warn!("could not record port {port} in {}: {e:#}", paths.daemon_config_path().display());
        }
    }
    let info = Arc::new(PortFile {
        port,
        pid: std::process::id(),
        instance_id: Uuid::new_v4().to_string(),
        started: fwd_core::now_ms(),
        bind: config.bind,
    });

    let hosted = projects.len();
    let open = open_projects(&projects);
    let state = AppState::new(&paths, info.clone(), node, projects, dev_runner.clone());
    let shutdown = state.shutdown.clone();
    let app = router(state);

    write_port_file(&paths.port_file(), &info)?;
    dev_runner.announce(port, open);
    let address = match config.bind {
        Bind::Loopback => "127.0.0.1",
        Bind::Lan => "every interface",
    };
    info!("daemon listening on {address}, port {port}, for {hosted} project(s)");

    let result = axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(async move {
            let ctrl_c = tokio::signal::ctrl_c();
            #[cfg(unix)]
            let term = async {
                let mut sig =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("signal");
                sig.recv().await;
            };
            #[cfg(not(unix))]
            let term = std::future::pending::<()>();
            tokio::select! {
                _ = shutdown.notified() => info!("stop requested"),
                _ = ctrl_c => info!("interrupted"),
                _ = term => info!("terminated"),
            }
        })
        .await;
    let _ = std::fs::remove_file(paths.port_file());
    dev_runner.clear();
    info!("daemon exiting");
    // Sockets that were upgraded before the stop keep the node, and the store under it, open until
    // the runtime shuts down, after this returns. The lifetime lock therefore stays held until the
    // process exits (the operating system releases it then), so a new daemon never opens the
    // store while this one still has it open.
    std::mem::forget(lifetime);
    result.map_err(Into::into)
}

/// The listener's routes (docs/design/daemon.md, "Runtime directory and listeners") behind the
/// checks every request passes; built apart from `serve` so the refusals are tested in process.
fn router(state: AppState) -> Router {
    let control = || middleware::from_fn(local_process);
    Router::new()
        .route("/health", get(health))
        .route("/projects", get(projects_list).merge(post(projects_open).route_layer(control())))
        // A project's Ankurah socket, at the base path every client builds with the project's id
        // (here the route's capture in its place) and the `/ws` Ankurah's connectors append.
        .route(&format!("{}/ws", daemon_socket::base_path("{project}")), get(project_socket))
        .route(PER_PROJECT_SOCKET, get(refuse_older_client))
        .route("/p/{project}/checkouts", get(checkouts_list).post(checkouts_report).route_layer(control()))
        .route("/webview-origins", post(admit_webview).route_layer(control()))
        .route("/stop", post(stop).route_layer(control()))
        .merge(mcp_routes(&state).route_layer(control()))
        // The web UI: the page at `/`, its script, wasm and stylesheet beside it. Every other
        // path is not found.
        .fallback(web)
        .layer(middleware::from_fn_with_state(state.clone(), local_only))
        .with_state(state)
}

/// The MCP endpoint (crates/fwd/src/mcp.rs) at `/mcp`, over Streamable HTTP: stateless, each POST
/// answered with JSON, so that a daemon restart costs a client nothing but the call in flight. The
/// `Host` and `Origin` checks are the daemon's own (`local_only`, and `local_process`, which the
/// caller layers over it so that only a process on this machine that is not a web page reaches it),
/// so rmcp's are not repeated.
fn mcp_routes(state: &AppState) -> Router<AppState> {
    let ops = Arc::new(mcp::Operations::new(Arc::new(state.clone()), Arc::new(mcp::NoWake)));
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_sse_keep_alive(None)
        .disable_allowed_hosts();
    let service = StreamableHttpService::new(
        move || Ok(mcp::Server::new(ops.clone())),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    Router::new().route_service("/mcp", service)
}

impl mcp::Hosting for AppState {
    fn paths(&self) -> &Paths {
        &self.paths
    }

    fn port(&self) -> u16 {
        self.info.port
    }

    fn projects(&self) -> Vec<(Uuid, String, Option<String>)> {
        // By the names the index has now, as `/projects` lists them.
        let names: HashMap<Uuid, String> = Index::load(&self.paths)
            .map(|index| index.projects.into_iter().map(|p| (p.id, p.name)).collect())
            .unwrap_or_default();
        let mut list: Vec<(Uuid, String, Option<String>)> = self
            .projects
            .read()
            .expect("projects lock")
            .values()
            .map(|slot| {
                let report = slot.report();
                (report.id, names.get(&report.id).cloned().unwrap_or(report.name), report.error)
            })
            .collect();
        list.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
        list
    }

    fn open(&self, id: Uuid) -> Option<(NamedProject, ProjectGraph)> {
        let hosted = hosted(self, id)?;
        let mut project = hosted.project.clone();
        if let Some(name) = mcp::Hosting::projects(self).into_iter().find(|p| p.0 == id).map(|p| p.1) {
            project.name = name;
        }
        Some((project, hosted.graph.clone()))
    }
}

/// Listens on the port the daemon had last time when it is free, so an open page or a phone's
/// bookmark keeps working across a restart; otherwise on any free port. Free includes that
/// nothing accepts connections to it on `127.0.0.1`, where every client on this machine
/// connects: tokio sets `SO_REUSEADDR`, and with it macOS lets `0.0.0.0:<port>` bind while
/// another program listens on `127.0.0.1:<port>`, so the bind succeeds and every client on this
/// machine reaches that program instead.
async fn listen(bind: Bind, recorded: Option<u16>) -> Result<TcpListener> {
    let ip: IpAddr = match bind {
        Bind::Loopback => Ipv4Addr::LOCALHOST.into(),
        Bind::Lan => Ipv4Addr::UNSPECIFIED.into(),
    };
    if let Some(port) = recorded.filter(|p| *p != 0) {
        if in_use_on_loopback(port).await {
            info!("port {port} is in use on 127.0.0.1; listening on another");
        } else {
            match TcpListener::bind((ip, port)).await {
                Ok(listener) => return Ok(listener),
                Err(e) => info!("port {port} is not available ({e}); listening on another"),
            }
        }
    }
    TcpListener::bind((ip, 0)).await.with_context(|| format!("listening on {ip}"))
}

/// True unless a connection to `127.0.0.1:<port>` is refused within a second: something accepts
/// it, or a listener too busy to answer holds the port.
async fn in_use_on_loopback(port: u16) -> bool {
    let attempt = tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port));
    !matches!(tokio::time::timeout(Duration::from_secs(1), attempt).await, Ok(Err(_)))
}

/// The `Host` values the listener answers to: its loopback names and, when it listens on the
/// LAN, the machine's `.local` name and every IPv4 address its interfaces have now (`Allowed`
/// lists them again after the machine moves to another network). A browser on another device
/// sends one of those; a DNS-rebinding page sends its own name.
fn own_hosts(bind: Bind, port: u16) -> Vec<String> {
    let mut hosts = vec![format!("127.0.0.1:{port}"), format!("localhost:{port}")];
    if bind == Bind::Lan {
        if let Some(name) = local_name() {
            hosts.push(format!("{name}.local:{port}"));
        }
        match local_ip_address::list_afinet_netifas() {
            Ok(interfaces) => {
                for (_, ip) in interfaces {
                    if let IpAddr::V4(ip) = ip {
                        let host = format!("{ip}:{port}");
                        if !ip.is_loopback() && !hosts.contains(&host) {
                            hosts.push(host);
                        }
                    }
                }
            }
            Err(e) => warn!("could not list the network interfaces ({e}); only this machine's names are accepted"),
        }
    }
    hosts
}

/// The machine's name as multicast DNS announces it on the LAN, lowercased as browsers send it:
/// the first label of the host name, so `Daniels-MacBook-Pro.local` gives `daniels-macbook-pro`.
#[cfg(unix)]
fn local_name() -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: the buffer is valid for writes of its whole length, and gethostname writes at most
    // that many bytes.
    if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } != 0 {
        return None;
    }
    let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    let name = std::str::from_utf8(&buf[..end]).ok()?.split('.').next()?.trim().to_ascii_lowercase();
    (!name.is_empty()).then_some(name)
}

#[cfg(not(unix))]
fn local_name() -> Option<String> {
    None
}

async fn open_slot(paths: &Paths, node: &DaemonNode, project: NamedProject) -> Slot {
    match host_project(paths, node, &project).await {
        Ok(hosted) => {
            info!("hosting project {} ({})", project.name, project.id);
            Slot::Open(Arc::new(hosted))
        }
        Err(e) => {
            let error = format!("{e:#}");
            warn!("could not open project {} ({}): {error}", project.name, project.id);
            Slot::Failed { project, error }
        }
    }
}

/// The number the daemon's dev-runner status file reports: the projects open, leaving out those
/// that could not open (the log and `/projects` name them).
fn open_projects(projects: &BTreeMap<Uuid, Slot>) -> usize {
    projects.values().filter(|slot| matches!(slot, Slot::Open(_))).count()
}

/// Hosts one project over the daemon's node: its record found, or made on first use, its graph,
/// the routing service over that graph, and its checkout registry. A project whose records are
/// still in a store of its own is refused ([`Unmoved`]).
async fn host_project(paths: &Paths, node: &DaemonNode, project: &NamedProject) -> Result<Hosted> {
    if let Some(store) = paths.unmoved_store(project) {
        return Err(Unmoved { name: project.name.clone(), store }.into());
    }
    std::fs::create_dir_all(paths.storage_dir(project))?;
    let graph = ProjectGraph::ensure(node.ctx.clone(), project.id, &project.name).await?;
    let routing = services::start(graph.clone()).await.context("starting the routing service")?;
    let registry_path = paths.checkouts_path(project);
    let registry = Registry { checkouts: load_registry(&registry_path)?, expanded: HashMap::new() };
    Ok(Hosted { project: project.clone(), registry: Mutex::new(registry), registry_path, graph, _routing: routing })
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "not found\n").into_response()
}

/// Refuses requests that are not addressed to one of this daemon's own names, and browser
/// requests from any origin but the daemon's own page and the webviews the extension registered.
/// The CLI's sockets and control calls carry no `Origin`. On a LAN bind a miss lists the
/// interfaces again first (at most every `RELIST_INTERVAL`), so after the laptop moves to another
/// network the phone there is answered without a restart. A webview's page is on another origin
/// than the daemon's, and a browser shows such a page an answer only when the answer names its
/// origin, so the answers to a registered webview do.
async fn local_only(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let host = req.headers().get(header::HOST).and_then(|h| h.to_str().ok());
    let origin = req.headers().get(header::ORIGIN).and_then(|o| o.to_str().ok());
    let mut admitted = state.allowed.read().expect("allowlist lock").admits(host, origin);
    if !admitted && state.info.bind == Bind::Lan {
        let mut allowed = state.allowed.write().expect("allowlist lock");
        if Instant::now() >= allowed.relist_after {
            allowed.relist(Bind::Lan, state.info.port);
        }
        admitted = allowed.admits(host, origin);
    }
    if !admitted {
        return (StatusCode::FORBIDDEN, "forbidden\n").into_response();
    }
    // Admitted, so a webview's origin here is a registered one.
    let webview = req.headers().get(header::ORIGIN).filter(|o| o.to_str().is_ok_and(webview_origin)).cloned();
    let mut response = next.run(req).await;
    if let Some(origin) = webview {
        let headers = response.headers_mut();
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
        headers.append(header::VARY, HeaderValue::from_static("origin"));
    }
    response
}

/// The control routes (open a project, stop the daemon, admit a webview's origin, the checkout
/// registry) answer only a process on this machine that is not a web page. The peer must be on
/// loopback, so a daemon that listens on the LAN cannot be stopped or reconfigured from another
/// device, and the paths of this machine's checkouts do not leave it. And the request must carry
/// no `Origin`, which the CLI and the extension host never send and a browser sends with every
/// POST and with every request a page makes to another origin (where a webview's page is): no
/// page calls these routes, and a page that could would stop the daemon with a POST that needs
/// no preflight.
async fn local_process(ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request, next: Next) -> Response {
    if !peer.ip().to_canonical().is_loopback() {
        return (StatusCode::FORBIDDEN, "forbidden: only a process on the daemon's machine may call this\n")
            .into_response();
    }
    if req.headers().contains_key(header::ORIGIN) {
        return (StatusCode::FORBIDDEN, "forbidden: a web page may not call this\n").into_response();
    }
    next.run(req).await
}

async fn health(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "ok": true,
        "pid": state.info.pid,
        "instance_id": state.info.instance_id,
        "started": state.info.started,
        "bind": state.info.bind,
        "build": BUILD,
    }))
}

async fn stop(State(state): State<AppState>) -> Response {
    state.shutdown.notify_one();
    "stopping\n".into_response()
}

#[derive(Deserialize)]
struct WebviewOrigin {
    origin: String,
}

/// Admits a VS Code webview's origin for as long as the daemon runs: the extension registers the
/// origin of its own panel before the page there loads, and again whenever that page loses its
/// connection (a restarted daemon has forgotten it), so fwd's page reaches the daemon from its
/// webview and another extension's webview does not.
async fn admit_webview(State(state): State<AppState>, Json(request): Json<WebviewOrigin>) -> Response {
    if !webview_origin(&request.origin) {
        let refusal = format!("`{}` is not a VS Code webview's origin (vscode-webview://<id>)\n", request.origin);
        return (StatusCode::BAD_REQUEST, refusal).into_response();
    }
    state.allowed.write().expect("allowlist lock").admit_webview(&request.origin);
    info!("admitted the webview origin {}", request.origin);
    Json(serde_json::json!({ "ok": true })).into_response()
}

/// Every project the daemon hosts, by name; `error` on those it could not open. The names are
/// read from the index at each request, so a project renamed with `fwd project rename` is listed
/// under its new name without a restart; an index that cannot be read leaves the names the
/// projects had when they opened.
async fn projects_list(State(state): State<AppState>) -> Json<Vec<HostedProject>> {
    let names: HashMap<Uuid, String> = match Index::load(&state.paths) {
        Ok(index) => index.projects.into_iter().map(|p| (p.id, p.name)).collect(),
        Err(e) => {
            warn!("listing projects under the names they opened with: {e:#}");
            HashMap::new()
        }
    };
    let mut list: Vec<HostedProject> = state
        .projects
        .read()
        .expect("projects lock")
        .values()
        .map(|slot| {
            let mut report = slot.report();
            if let Some(name) = names.get(&report.id) {
                report.name = name.clone();
            }
            report
        })
        .collect();
    list.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    Json(list)
}

#[derive(Deserialize)]
struct OpenRequest {
    id: Uuid,
}

/// Opens a project from the index that the daemon does not host yet: one created since the
/// daemon started (`fwd init` asks for it), or one that failed to open before. Asking for a
/// project the daemon already hosts changes nothing.
async fn projects_open(State(state): State<AppState>, Json(request): Json<OpenRequest>) -> Response {
    let _opening = state.opening.lock().await;
    if let Some(hosted) = hosted(&state, request.id) {
        return Json(Slot::Open(hosted).report()).into_response();
    }
    let entry = match Index::load(&state.paths) {
        Ok(index) => index.projects.into_iter().find(|p| p.id == request.id),
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": format!("{e:#}")})))
                .into_response()
        }
    };
    let Some(entry) = entry else {
        let error = format!("no project has the id {} in {}", request.id, state.paths.index_path().display());
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": error }))).into_response();
    };
    let slot = open_slot(&state.paths, &state.node, NamedProject::from(&entry)).await;
    let report = slot.report();
    state.projects.write().expect("projects lock").insert(request.id, slot);
    if report.error.is_none() {
        state.dev_runner.ready(open_projects(&state.projects.read().expect("projects lock")));
    }
    let status = if report.error.is_some() { StatusCode::INTERNAL_SERVER_ERROR } else { StatusCode::OK };
    (status, Json(report)).into_response()
}

fn hosted(state: &AppState, id: Uuid) -> Option<Arc<Hosted>> {
    match state.projects.read().expect("projects lock").get(&id) {
        Some(Slot::Open(hosted)) => Some(hosted.clone()),
        _ => None,
    }
}

/// The project a `/p/<project>/...` route names.
#[derive(Deserialize)]
struct ProjectPath {
    project: Uuid,
}

async fn project_socket(
    State(state): State<AppState>,
    RoutePath(path): RoutePath<ProjectPath>,
    req: Request,
) -> Response {
    match hosted(&state, path.project) {
        Some(_) => (state.node.socket)(req).await,
        None => not_found(),
    }
}

/// The path at which the daemons that kept a store per project served each project's socket, and
/// at which a client built for them still asks for it.
const PER_PROJECT_SOCKET: &str = "/p/{project}/ws";

/// What such a client is told, whatever project it names.
const OLDER_CLIENT_REFUSAL: &str =
    "This fwd daemon keeps every project in one store, and this client's build of fwd is older than the daemon's.\n";

/// Refuses a client built for the daemons that kept a store per project, which would read every
/// project's records here (`fwd_core::daemon_socket` says how). `410 Gone`, since that socket is
/// gone from this path for good, unlike the socket's `404` for a project the daemon does not host.
/// No status stops those clients sooner, since none of them reads it: a command's Ankurah client
/// fails its first attempt on any answer but `101` and the command ends with it, a `fwd watch`
/// that was running keeps trying, at most every 30 s, until its ten-minute outage limit ends it,
/// and a page's browser hides the answer from it while its connector tries again, at most every
/// 10 s.
async fn refuse_older_client() -> Response {
    (StatusCode::GONE, OLDER_CLIENT_REFUSAL).into_response()
}

/// The project's registry, minus checkouts whose directory is gone (a deleted worktree simply
/// stops being listed; nothing in the graph referred to it).
async fn checkouts_list(State(state): State<AppState>, RoutePath(path): RoutePath<ProjectPath>) -> Response {
    let Some(hosted) = hosted(&state, path.project) else { return not_found() };
    let mut registry = hosted.registry.lock().expect("registry lock");
    let before = registry.checkouts.len();
    registry.checkouts.retain(|c| c.root.is_dir());
    if registry.checkouts.len() != before {
        if let Err(e) = persist(&hosted.registry_path, &registry.checkouts) {
            warn!("could not write the checkout registry: {e}");
        }
    }
    Json(registry.checkouts.clone()).into_response()
}

/// A client reports the checkout it runs in (or binds a scope to one, or clears that binding).
/// The daemon re-reads the checkout with git and answers; the sibling worktrees are scanned in
/// the background, at most once a minute per root, so one report from any worktree teaches the
/// daemon about all of them without every command paying for the scan.
async fn checkouts_report(
    State(state): State<AppState>,
    RoutePath(path): RoutePath<ProjectPath>,
    Json(reported): Json<CheckoutRecord>,
) -> Response {
    let Some(hosted) = hosted(&state, path.project) else { return not_found() };
    let root = reported.root.clone();
    let checked = tokio::task::spawn_blocking({
        let root = root.clone();
        move || project::checkout_at(&root)
    })
    .await;
    let fresh = match checked {
        Ok(Ok(Some(c))) => CheckoutRecord::from(&c),
        Ok(Ok(None)) => return (StatusCode::BAD_REQUEST, "not inside a git checkout\n").into_response(),
        Ok(Err(e)) => return (StatusCode::BAD_REQUEST, format!("{e}\n")).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}\n")).into_response(),
    };
    let scan_due;
    let count;
    {
        let mut registry = hosted.registry.lock().expect("registry lock");
        let binding =
            if reported.clear_scope { Binding::Clear } else { reported.scope.map_or(Binding::Keep, Binding::Set) };
        upsert(&mut registry.checkouts, fresh, binding);
        if let Err(e) = persist(&hosted.registry_path, &registry.checkouts) {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("could not write the checkout registry: {e}\n"))
                .into_response();
        }
        count = registry.checkouts.len();
        scan_due = registry.expanded.get(&root).is_none_or(|t| t.elapsed() > EXPANSION_INTERVAL);
        if scan_due {
            registry.expanded.insert(root.clone(), Instant::now());
        }
    }
    if scan_due {
        tokio::spawn(scan_siblings(hosted, root));
    }
    Json(serde_json::json!({"ok": true, "checkouts": count})).into_response()
}

enum Binding {
    Keep,
    Set(String),
    Clear,
}

fn upsert(checkouts: &mut Vec<CheckoutRecord>, record: CheckoutRecord, binding: Binding) {
    match checkouts.iter_mut().find(|c| c.root == record.root) {
        Some(existing) => {
            let scope = match binding {
                Binding::Keep => existing.scope.take(),
                Binding::Set(s) => Some(s),
                Binding::Clear => None,
            };
            *existing = CheckoutRecord { scope, clear_scope: false, ..record };
        }
        None => {
            let scope = match binding {
                Binding::Set(s) => Some(s),
                _ => None,
            };
            checkouts.push(CheckoutRecord { scope, clear_scope: false, ..record });
        }
    }
}

/// Every worktree git lists beside `root`, read with git off the async runtime, then merged
/// (their existing bindings kept).
async fn scan_siblings(hosted: Arc<Hosted>, root: PathBuf) {
    let siblings = tokio::task::spawn_blocking(move || {
        project::worktrees_of(&root)
            .into_iter()
            .filter_map(|wt| project::checkout_at(&wt).ok().flatten())
            .map(|c| CheckoutRecord::from(&c))
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();
    if siblings.is_empty() {
        return;
    }
    let mut registry = hosted.registry.lock().expect("registry lock");
    for record in siblings {
        upsert(&mut registry.checkouts, record, Binding::Keep);
    }
    if let Err(e) = persist(&hosted.registry_path, &registry.checkouts) {
        warn!("could not write the checkout registry: {e}");
    }
}

/// Reads the registry at start. A file that does not parse is kept beside the new one as
/// `checkouts.json.corrupt` rather than silently discarded.
fn load_registry(path: &Path) -> Result<Vec<CheckoutRecord>> {
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(list) => Ok(list),
            Err(e) => {
                let kept = path.with_extension("json.corrupt");
                warn!(
                    "the checkout registry {} does not parse ({e}); keeping it as {}",
                    path.display(),
                    kept.display()
                );
                let _ = std::fs::rename(path, &kept);
                Ok(Vec::new())
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Written while the caller holds the registry lock: a temp file synced and renamed into place.
fn persist(path: &Path, checkouts: &[CheckoutRecord]) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        use std::io::Write;
        f.write_all(&serde_json::to_vec_pretty(checkouts)?)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Sent with everything `web` serves, so no other site can show the page in a frame: the page
/// needs no key, so a framed copy would work, and the site around it could lead a click onto
/// it. The VS Code webview bundles the app rather than framing this page (docs/design/web-ui.md).
const NO_FRAMING: [(HeaderName, &str); 2] =
    [(header::X_FRAME_OPTIONS, "DENY"), (header::CONTENT_SECURITY_POLICY, "frame-ancestors 'none'")];

/// Serves the embedded web UI. `/` is the page; any other path is a file beside it, or not
/// found. Files are sent with `no-cache` so a rebuilt UI shows up on the next reload without
/// renamed assets.
async fn web(method: Method, uri: Uri) -> Response {
    (NO_FRAMING, web_file(&method, &uri)).into_response()
}

fn web_file(method: &Method, uri: &Uri) -> Response {
    if method != Method::GET && method != Method::HEAD {
        return not_found();
    }
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    match WEB.get_file(path) {
        Some(file) => {
            let mime = match path.rsplit('.').next() {
                Some("html") => "text/html; charset=utf-8",
                Some("js") => "text/javascript",
                Some("wasm") => "application/wasm",
                Some("css") => "text/css",
                Some("svg") => "image/svg+xml",
                Some("png") => "image/png",
                Some("json") => "application/json",
                Some("woff2") => "font/woff2",
                _ => "application/octet-stream",
            };
            ([(header::CONTENT_TYPE, mime), (header::CACHE_CONTROL, "no-cache")], file.contents()).into_response()
        }
        None if path == "index.html" => {
            ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], NOT_BUILT).into_response()
        }
        None => not_found(),
    }
}

const NOT_BUILT: &str = "<!doctype html><meta charset='utf-8'><title>fwd</title>\
<body style='font-family:system-ui;max-width:42em;margin:4em auto;line-height:1.5'>\
<h1>fwd</h1><p>This build of <code>fwd</code> carries no web UI. Build it, rebuild the binary and restart the daemon:</p>\
<pre>scripts/build-web.sh\ncargo build --workspace\nfwd daemon stop\nfwd ui</pre></body>";

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::extract::connect_info::MockConnectInfo;
    use axum::http::request::Builder;
    use tower::ServiceExt;

    /// The port the daemon under test believes it listens on; nothing listens anywhere.
    const PORT: u16 = 4242;
    const HERE: &str = "127.0.0.1:4242";
    /// A device on the LAN.
    const PHONE: [u8; 4] = [192, 168, 1, 20];
    const THIS_MACHINE: [u8; 4] = [127, 0, 0, 1];

    /// The state of a daemon that hosts no project, over a store in memory.
    async fn state(dir: &Path, bind: Bind) -> AppState {
        let paths = Paths { runtime_dir: dir.join("run"), data_dir: dir.join("data") };
        let info = PortFile { port: PORT, pid: std::process::id(), instance_id: "test".into(), started: 0, bind };
        let dev_runner = dev_runner::Entry::new(Some(dir.join("dev-runner")), &paths.data_dir);
        let store = SqliteStorageEngine::open_in_memory().await.expect("a store in memory");
        let node = DaemonNode::open(store).await.expect("the daemon's node");
        AppState::new(&paths, Arc::new(info), Arc::new(node), BTreeMap::new(), Arc::new(dev_runner))
    }

    /// The daemon's router, with every request arriving from `peer`.
    fn app(state: AppState, peer: [u8; 4]) -> Router {
        router(state).layer(MockConnectInfo(SocketAddr::from((peer, 50_000))))
    }

    fn request(method: Method, path: &str, host: &str) -> Builder {
        axum::http::Request::builder().method(method).uri(path).header(header::HOST, host)
    }

    async fn send(app: &Router, request: Builder) -> Response {
        app.clone().oneshot(request.body(Body::empty()).expect("a request")).await.expect("the router answers")
    }

    /// `POST /webview-origins` for `origin`, as the extension host sends it: JSON, no `Origin`.
    async fn register(app: &Router, origin: &str) -> StatusCode {
        let body = serde_json::json!({ "origin": origin }).to_string();
        let request = request(Method::POST, "/webview-origins", HERE)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .expect("a request");
        app.clone().oneshot(request).await.expect("the router answers").status()
    }

    /// Whether `/stop` has asked the daemon to stop: the request stores a permit that a waiter
    /// takes at once.
    async fn asked_to_stop(state: &AppState) -> bool {
        tokio::time::timeout(Duration::from_millis(20), state.shutdown.notified()).await.is_ok()
    }

    #[tokio::test]
    async fn stop_answers_only_a_peer_on_this_machine() {
        let dir = tempfile::tempdir().expect("a temp dir");
        // Naming 127.0.0.1 as the host passes the host check; the peer's address decides.
        let from_the_phone = app(state(dir.path(), Bind::Lan).await, PHONE);
        assert_eq!(send(&from_the_phone, request(Method::POST, "/stop", HERE)).await.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            send(&from_the_phone, request(Method::POST, "/projects", HERE)).await.status(),
            StatusCode::FORBIDDEN
        );
        let checkouts = format!("/p/{}/checkouts", Uuid::nil());
        assert_eq!(send(&from_the_phone, request(Method::GET, &checkouts, HERE)).await.status(), StatusCode::FORBIDDEN);
        assert_eq!(register(&from_the_phone, "vscode-webview://abc").await, StatusCode::FORBIDDEN);

        let from_here = app(state(dir.path(), Bind::Lan).await, THIS_MACHINE);
        assert_eq!(send(&from_here, request(Method::POST, "/stop", HERE)).await.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn the_mcp_endpoint_answers_only_a_process_on_this_machine_that_is_not_a_web_page() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let list = || {
            request(Method::POST, "/mcp", HERE)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ACCEPT, "application/json, text/event-stream")
        };
        let body = || Body::from(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#);
        let answer = |app: Router, request: Builder| async move {
            app.oneshot(request.body(body()).expect("a request")).await.expect("the router answers").status()
        };
        // The endpoint asks for no login, so a LAN bind must not expose it.
        let from_the_phone = app(state(dir.path(), Bind::Lan).await, PHONE);
        assert_eq!(answer(from_the_phone, list()).await, StatusCode::FORBIDDEN);
        let from_here = app(state(dir.path(), Bind::Loopback).await, THIS_MACHINE);
        for origin in ["http://127.0.0.1:4242", "https://evil.example", "null"] {
            assert_eq!(
                answer(from_here.clone(), list().header(header::ORIGIN, origin)).await,
                StatusCode::FORBIDDEN,
                "{origin}"
            );
        }
        assert_eq!(answer(from_here, list()).await, StatusCode::OK);
    }

    #[tokio::test]
    async fn the_control_routes_refuse_every_web_page_the_daemons_own_and_a_webviews_included() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let state = state(dir.path(), Bind::Loopback).await;
        let app = app(state.clone(), THIS_MACHINE);
        assert_eq!(register(&app, "vscode-webview://ours").await, StatusCode::OK);
        let checkouts = format!("/p/{}/checkouts", Uuid::nil());
        for origin in ["http://127.0.0.1:4242", "vscode-webview://ours", "vscode-webview://theirs", "null"] {
            for (method, path) in [
                (Method::POST, "/stop"),
                (Method::POST, "/projects"),
                (Method::GET, checkouts.as_str()),
                (Method::POST, checkouts.as_str()),
                (Method::POST, "/webview-origins"),
            ] {
                // A page's POST with `Content-Type: text/plain` is a simple request: the browser
                // sends it without asking first, so the refusal has to come from the daemon.
                let request = request(method.clone(), path, HERE)
                    .header(header::ORIGIN, origin)
                    .header(header::CONTENT_TYPE, "text/plain");
                assert_eq!(send(&app, request).await.status(), StatusCode::FORBIDDEN, "{method} {path} from {origin}");
            }
        }
        assert!(!asked_to_stop(&state).await, "no page stopped the daemon");
        // The CLI sends no origin.
        assert_eq!(send(&app, request(Method::POST, "/stop", HERE)).await.status(), StatusCode::OK);
        assert!(asked_to_stop(&state).await);
    }

    #[tokio::test]
    async fn only_a_webview_origin_is_registered_and_the_oldest_goes_first() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let state = state(dir.path(), Bind::Loopback).await;
        let app = app(state.clone(), THIS_MACHINE);
        for not_one in [
            "https://evil.example",
            "http://127.0.0.1:4242",
            "vscode-webview://",
            "vscode-webview://abc/path",
            "vscode-webview://abc:80",
            "vscode-webview://me@abc",
            "null",
        ] {
            assert_eq!(register(&app, not_one).await, StatusCode::BAD_REQUEST, "{not_one}");
        }
        let admitted = |origin: &str| state.allowed.read().expect("allowlist lock").admits(Some(HERE), Some(origin));
        for n in 0..=WEBVIEW_ORIGINS {
            assert_eq!(register(&app, &format!("vscode-webview://w{n}")).await, StatusCode::OK);
        }
        assert!(!admitted("vscode-webview://w0"), "one past the bound drops the oldest");
        assert!(admitted("vscode-webview://w1") && admitted(&format!("vscode-webview://w{WEBVIEW_ORIGINS}")));
        // Registering one again makes it the newest, so the next to go is the one after it.
        assert_eq!(register(&app, "vscode-webview://w1").await, StatusCode::OK);
        assert_eq!(register(&app, "vscode-webview://another").await, StatusCode::OK);
        assert!(admitted("vscode-webview://w1") && !admitted("vscode-webview://w2"));
    }

    #[tokio::test]
    async fn a_socket_upgrade_from_another_sites_page_is_refused() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let app = app(state(dir.path(), Bind::Loopback).await, THIS_MACHINE);
        let upgrade = |origin: Option<&str>| {
            let request = request(Method::GET, &format!("{}/ws", daemon_socket::base_path(Uuid::nil())), HERE)
                .header(header::CONNECTION, "Upgrade")
                .header(header::UPGRADE, "websocket")
                .header(header::SEC_WEBSOCKET_VERSION, "13")
                .header(header::SEC_WEBSOCKET_KEY, "dGhlIHNhbXBsZSBub25jZQ==");
            match origin {
                Some(origin) => request.header(header::ORIGIN, origin),
                None => request,
            }
        };
        assert_eq!(send(&app, upgrade(Some("http://evil.example"))).await.status(), StatusCode::FORBIDDEN);
        assert_eq!(send(&app, upgrade(Some("vscode-webview://abc"))).await.status(), StatusCode::FORBIDDEN);
        // The daemon's own page, the webview the extension registered, and a client that sends no
        // origin, reach the socket route (which has no project by that id).
        assert_eq!(register(&app, "vscode-webview://abc").await, StatusCode::OK);
        assert_eq!(send(&app, upgrade(Some("http://127.0.0.1:4242"))).await.status(), StatusCode::NOT_FOUND);
        assert_eq!(send(&app, upgrade(Some("vscode-webview://abc"))).await.status(), StatusCode::NOT_FOUND);
        assert_eq!(send(&app, upgrade(None)).await.status(), StatusCode::NOT_FOUND);
        assert_eq!(send(&app, upgrade(Some("vscode-webview://other"))).await.status(), StatusCode::FORBIDDEN);
    }

    /// One socket upgrade to `path`, sent as Ankurah's clients send it over a real connection to
    /// the listener on `port`, so that an upgrade can succeed: the answer's status line, and its
    /// body (none when the socket opened).
    async fn upgrade_over_tcp(port: u16, path: &str) -> (String, String) {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
        let connection = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.expect("a connection");
        let mut answer = BufReader::new(connection);
        let head = format!(
            "GET {path} HTTP/1.1\r\nHost: {HERE}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\
             Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
        );
        answer.get_mut().write_all(head.as_bytes()).await.expect("the request is sent");
        let mut status = String::new();
        answer.read_line(&mut status).await.expect("a status line");
        let mut length = 0;
        loop {
            let mut line = String::new();
            answer.read_line(&mut line).await.expect("a header");
            match line.split_once(':') {
                _ if line == "\r\n" => break,
                Some((name, value)) if name.eq_ignore_ascii_case("content-length") => {
                    length = value.trim().parse().expect("a length");
                }
                _ => {}
            }
        }
        let mut body = vec![0; length];
        answer.read_exact(&mut body).await.expect("the body");
        (status.trim_end().to_string(), String::from_utf8(body).expect("a text body"))
    }

    /// A client built for the daemons that kept a store per project asks for its socket where
    /// those daemons served it, and is refused with `410 Gone` and the sentence saying why,
    /// whatever project it names, so that it never reads the one store; a client of this build
    /// asks where `fwd_core::daemon_socket` says, and its socket opens.
    #[tokio::test]
    async fn an_older_client_is_refused_at_the_per_project_socket_and_this_builds_socket_opens() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let state = state(dir.path(), Bind::Loopback).await;
        let project = NamedProject { id: Uuid::new_v4(), name: "test".into() };
        let hosted = host_project(&state.paths, &state.node, &project).await.expect("the project is hosted");
        state.projects.write().expect("projects lock").insert(project.id, Slot::Open(Arc::new(hosted)));
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("a listener");
        let port = listener.local_addr().expect("the listener's address").port();
        // The peer's address as `serve` gives it, which Ankurah's upgrade reads from the request
        // itself, where `MockConnectInfo` puts nothing.
        let app = router(state).into_make_service_with_connect_info::<SocketAddr>();
        let server = tokio::spawn(async move { axum::serve(listener, app).await });

        for named in [project.id, Uuid::nil()] {
            assert_eq!(
                upgrade_over_tcp(port, &format!("/p/{named}/ws")).await,
                (
                    "HTTP/1.1 410 Gone".to_string(),
                    "This fwd daemon keeps every project in one store, and this client's build of fwd is older than the daemon's.\n"
                        .to_string()
                )
            );
        }
        assert_eq!(
            upgrade_over_tcp(port, &format!("{}/ws", daemon_socket::base_path(project.id))).await,
            ("HTTP/1.1 101 Switching Protocols".to_string(), String::new())
        );
        server.abort();
    }

    #[tokio::test]
    async fn only_the_webview_the_extension_registered_may_read_the_daemon() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let app = app(state(dir.path(), Bind::Loopback).await, THIS_MACHINE);
        let get =
            |path: &str, origin: &str, host: &str| request(Method::GET, path, host).header(header::ORIGIN, origin);
        let allowed_origin = |response: &Response| response.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN).cloned();
        for path in ["/health", "/projects"] {
            assert_eq!(
                send(&app, get(path, "vscode-webview://abc", HERE)).await.status(),
                StatusCode::FORBIDDEN,
                "{path}"
            );
        }
        assert_eq!(register(&app, "vscode-webview://abc").await, StatusCode::OK);
        for path in ["/health", "/projects"] {
            let webview = send(&app, get(path, "vscode-webview://abc", HERE)).await;
            assert_eq!(webview.status(), StatusCode::OK, "{path}");
            // The webview's page is on another origin, so the browser shows it the answer only
            // because the answer names that origin.
            assert_eq!(allowed_origin(&webview), Some(HeaderValue::from_static("vscode-webview://abc")), "{path}");
            assert_eq!(send(&app, get(path, "vscode-webview://other", HERE)).await.status(), StatusCode::FORBIDDEN);
        }
        assert_eq!(send(&app, get("/health", "https://evil.example", HERE)).await.status(), StatusCode::FORBIDDEN);
        // The host check still holds for a registered webview.
        assert_eq!(
            send(&app, get("/health", "vscode-webview://abc", "evil.example:4242")).await.status(),
            StatusCode::FORBIDDEN
        );
        // The daemon's own page is on the daemon's origin and needs no such header.
        let own = send(&app, get("/health", "http://127.0.0.1:4242", HERE)).await;
        assert_eq!((own.status(), allowed_origin(&own)), (StatusCode::OK, None));
    }

    #[tokio::test]
    async fn the_health_check_names_the_build() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let app = app(state(dir.path(), Bind::Loopback).await, THIS_MACHINE);
        let response = send(&app, request(Method::GET, "/health", HERE)).await;
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.expect("the answer's body");
        let health: serde_json::Value = serde_json::from_slice(&body).expect("the answer is JSON");
        assert!(!BUILD.is_empty());
        assert_eq!(health["build"], BUILD);
    }

    #[tokio::test]
    async fn a_request_for_another_host_is_refused() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let app = app(state(dir.path(), Bind::Loopback).await, THIS_MACHINE);
        assert_eq!(
            send(&app, request(Method::GET, "/health", "evil.example:4242")).await.status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(send(&app, request(Method::GET, "/", "evil.example:4242")).await.status(), StatusCode::FORBIDDEN);
        assert_eq!(send(&app, request(Method::GET, "/health", HERE)).await.status(), StatusCode::OK);
        assert_eq!(send(&app, request(Method::GET, "/health", "localhost:4242")).await.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn no_other_site_may_frame_the_page() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let app = app(state(dir.path(), Bind::Loopback).await, THIS_MACHINE);
        for path in ["/", "/index.html", "/no-such-file.js"] {
            let response = send(&app, request(Method::GET, path, HERE)).await;
            let headers = response.headers();
            assert_eq!(headers.get(header::X_FRAME_OPTIONS).and_then(|v| v.to_str().ok()), Some("DENY"), "{path}");
            assert_eq!(
                headers.get(header::CONTENT_SECURITY_POLICY).and_then(|v| v.to_str().ok()),
                Some("frame-ancestors 'none'"),
                "{path}"
            );
        }
    }

    #[tokio::test]
    async fn the_project_list_names_projects_as_the_index_does_now() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let state = state(dir.path(), Bind::Loopback).await;
        let project = NamedProject { id: Uuid::new_v4(), name: "before".into() };
        state
            .projects
            .write()
            .expect("projects lock")
            .insert(project.id, Slot::Failed { project: project.clone(), error: "not opened in this test".into() });
        let list = |app: Router| async move {
            let response = send(&app, request(Method::GET, "/projects", HERE)).await;
            let body = axum::body::to_bytes(response.into_body(), 1 << 16).await.expect("the body");
            serde_json::from_slice::<Vec<HostedProject>>(&body).expect("the project list")
        };
        let app = app(state.clone(), THIS_MACHINE);
        assert_eq!(
            list(app.clone()).await[0].name,
            "before",
            "a project the index lacks keeps the name it opened with"
        );
        // As `fwd project rename` leaves it: the index has the new name, the daemon was not restarted.
        Index::update(&state.paths, |index| {
            index.projects.push(project::IndexEntry { id: project.id, name: "after".into(), upstreams: Vec::new() })
        })
        .expect("the index");
        assert_eq!(list(app).await[0].name, "after");
    }

    #[tokio::test]
    async fn a_port_anything_accepts_on_loopback_is_in_use() {
        let other = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("a port");
        let port = other.local_addr().expect("its address").port();
        assert!(in_use_on_loopback(port).await);
        drop(other);
        assert!(!in_use_on_loopback(port).await);
    }

    #[tokio::test]
    async fn a_lan_daemon_answers_at_the_addresses_of_the_network_it_moved_to() {
        let current = local_ip_address::list_afinet_netifas().ok().and_then(|interfaces| {
            interfaces.into_iter().find_map(|(_, ip)| match ip {
                IpAddr::V4(ip) if !ip.is_loopback() => Some(ip),
                _ => None,
            })
        });
        let Some(ip) = current else {
            eprintln!("skipped: this machine has no network address besides loopback");
            return;
        };
        let dir = tempfile::tempdir().expect("a temp dir");
        let state = state(dir.path(), Bind::Lan).await;
        let allowed = state.allowed.clone();
        let app = app(state, PHONE);
        let host = format!("{ip}:{PORT}");
        // As if the daemon had listed its interfaces on another network: today's address is
        // missing, and the next listing is not due yet. A webview was registered meanwhile.
        *allowed.write().expect("allowlist lock") = Allowed {
            hosts: vec![HERE.into()],
            origins: vec![format!("http://{HERE}")],
            webviews: VecDeque::from(["vscode-webview://abc".to_string()]),
            relist_after: Instant::now() + Duration::from_secs(60),
        };
        assert_eq!(send(&app, request(Method::GET, "/health", &host)).await.status(), StatusCode::FORBIDDEN);
        // Once a listing is due, a request for today's address has the daemon list them again.
        allowed.write().expect("allowlist lock").relist_after = Instant::now();
        assert_eq!(send(&app, request(Method::GET, "/health", &host)).await.status(), StatusCode::OK);
        let origin = request(Method::GET, "/health", &host).header(header::ORIGIN, format!("http://{host}"));
        assert_eq!(send(&app, origin).await.status(), StatusCode::OK);
        assert_eq!(
            send(&app, request(Method::GET, "/health", "evil.example:4242")).await.status(),
            StatusCode::FORBIDDEN
        );
        assert!(
            allowed.read().expect("allowlist lock").admits(Some(HERE), Some("vscode-webview://abc")),
            "listing the interfaces again keeps the registered webviews"
        );
    }
}
