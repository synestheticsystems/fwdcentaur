// fwdcentaur VS Code extension. The inbox view, the comment threads and the highlights on code
// read the project through the extension host's own Ankurah node (src/node.ts), so they change as
// the project changes. The task graph and the inbox page are the web UI's page (crates/fwd-web),
// which the build copies into dist/web and a webview runs as a node of its own; the task the page
// shows is the window's current task, which comments and questions made in the editor go to, with
// their code as the context (src/current.ts), and a highlight clicked on the page opens in the
// editor (src/reveal.ts).
// Starting the daemon, everything that needs git (a new comment's anchor, where an anchor sits in
// a version of its file) and replies still run the `fwd` binary (`fwd --format json`, child
// processes; src/fwd.ts).

import * as vscode from 'vscode';
import { randomBytes } from 'crypto';
import * as fs from 'fs';
import * as http from 'http';
import * as path from 'path';
import { WholeFileBadges } from './badges';
import { Comments } from './comments';
import { CurrentTask } from './current';
import { actingAs, config, findBinary, fwd, Json, repositoryPrefix, serially, short, workspaceRoot } from './fwd';
import { HostNode, Live, NotificationJson } from './node';
import { Principals } from './principals';
import { FILE_VERSION, References, referencesIn } from './references';
import { Referents } from './referents';
import { reveal } from './reveal';

// ---------------------------------------------------------------------------------------------
// Inbox view

class InboxItem extends vscode.TreeItem {
  constructor(public n: Json) {
    super(`${n.type}: ${n.message?.content ?? n.target?.type + ' ' + short(n.target?.id ?? '')}`, vscode.TreeItemCollapsibleState.None);
    this.description = `${n.priority}${n.status === 'unread' ? ' · unread' : ''}`;
    this.contextValue = n.status;
    this.tooltip = JSON.stringify(n, null, 2);
    this.iconPath = new vscode.ThemeIcon(n.status === 'unread' ? 'mail' : 'mail-read');
    this.command = { command: 'fwd.openNotification', title: 'Open', arguments: [this] };
  }
}

/**
 * The inbox of the principal the extension acts as, from a live query on its notifications, each
 * with the text of the message behind it, where everything it mentions is named as it is now.
 */
class Inbox implements vscode.TreeDataProvider<InboxItem> {
  private emitter = new vscode.EventEmitter<void>();
  onDidChangeTreeData = this.emitter.event;
  private node: HostNode | undefined;
  private live: Live<NotificationJson> | undefined;
  private starts = 0; // counts `start` calls, so an older one that finishes late gives way
  private items: Json[] = [];
  private texts = new Map<string, string>(); // the messages behind notifications, by id
  private references: References;
  private render = serially(() => this.load());

  constructor(
    principals: Principals,
    private referents: Referents,
  ) {
    this.references = new References(() => this.node, principals, referents);
    // A person, an agent or anything else named anew: when a message behind the inbox mentions
    // one, the tree is drawn again, naming it as it is now.
    const renamed = (ids: Set<string>) => {
      if ([...this.texts.values()].some((text) => referencesIn(text).some((id) => ids.has(id)))) this.render();
    };
    principals.onRenamed(renamed);
    referents.onRenamed(renamed);
  }

  /**
   * Follows the notifications of the principal the extension acts as (`fwd.as`, else the
   * project's person, found by its id once and kept: `actingAs`), replacing the query any earlier
   * call started: at connect, after `fwd.as` changes, and when the refresh command asks again.
   */
  async start(node: HostNode) {
    const start = ++this.starts;
    this.node = node;
    this.live?.stop();
    this.live = undefined;
    const handle = config().as;
    // Nothing makes the principal here: an agent names itself with `fwd principal add`, and `--as`
    // makes none (docs/design/agents-and-inbox.md, "Who belongs to a project").
    const me = await actingAs(node);
    if (start !== this.starts) return;
    if (!me) throw new Error(`no principal of this project is named ${handle}: an agent names itself with \`fwd principal add ${handle} --role <role>\``);
    this.live = node.notifications(me, this.render);
  }

  refresh(): Promise<void> {
    return this.node && !this.node.isClosed ? this.start(this.node) : Promise.resolve();
  }

  /** True once the query's first answer has arrived. */
  get loaded(): boolean {
    return this.live?.loaded ?? false;
  }

  async setStatus(item: InboxItem, status: 'read' | 'dismissed') {
    if (!this.node) throw new Error('this window is not connected to a fwd project');
    await this.node.setNotificationStatus([item.n.id], status); // the live query redraws the tree
  }

  getTreeItem(e: InboxItem) {
    return e;
  }

  getChildren(): InboxItem[] {
    return this.items.map((n) => new InboxItem(n));
  }

  /**
   * Reads the query's results in `fwd inbox`'s order, with the text of the message behind each,
   * its references read as names.
   */
  private async load() {
    const live = this.live;
    if (!live || !this.node) return;
    const notices = live.items().sort(inboxOrder);
    for (const n of notices) {
      const id = n.originator?.message_id;
      if (typeof id !== 'string' || this.texts.has(id)) continue;
      const text = await this.node.contentOf(id);
      if (text !== undefined) this.texts.set(id, text);
    }
    // What the tree shows: the things the messages behind its notifications mention.
    await this.referents.show(this, notices.flatMap((n) => referencesIn(this.texts.get(n.originator?.message_id) ?? '')));
    if (live !== this.live) return; // `start` replaced the query while the texts loaded
    this.items = notices.map((n) => {
      const text = this.texts.get(n.originator?.message_id);
      return text === undefined ? n : { ...n, message: { content: this.references.render(text) } };
    });
    this.emitter.fire();
  }
}

/** `fwd inbox`'s order (fwd-core's `ops::inbox`): unread first, then by priority, then oldest first. */
function inboxOrder(a: NotificationJson, b: NotificationJson): number {
  const unread = (n: NotificationJson) => (n.status === 'unread' ? 0 : 1);
  const priority = (n: NotificationJson) => ({ now: 0, next: 1 } as Record<string, number>)[n.priority] ?? 2;
  return unread(a) - unread(b) || priority(a) - priority(b) || a.created - b.created;
}

// ---------------------------------------------------------------------------------------------
// The page: the task graph, the inbox page and the rest of the web UI

/** Where the daemon listens, and the project the workspace folder belongs to (`fwd daemon status`). */
interface Daemon {
  port: number;
  project: string;
}

/** What the page's document shows, as the bridge in it reports when asked (the headless test asks). */
export interface PageReport {
  title: string;
  /** The document's origin, `vscode-webview://<id>`: the one the extension registered with the daemon. */
  origin: string;
  /** The query string of the document's URL: VS Code's own parameters and the page's. */
  search: string;
  /** The page's route. */
  hash: string;
  /** The text the page's body holds. */
  text: string;
  /** The stylesheets the document loaded, by address. */
  styles: string[];
  /** What the Content Security Policy refused since the document started: the directive and the address. */
  refused: string[];
  /** How the page last said it was doing (`failed`, or its connection's state); null until it says. */
  state: string | null;
  /**
   * The webview's state, where the bridge keeps the route: what VS Code saves with the panel, and
   * hands back when it brings the panel back.
   */
  saved: unknown;
}

/**
 * Registers `origin`, the origin of the page's webview, with the daemon at `port`
 * (`POST /webview-origins`), which then admits requests from that origin for as long as it runs.
 * The daemon admits no other webview's origin: any extension's webview, a Markdown preview with
 * scripts or a notebook's output can run script, so being a webview proves nothing. The call is a
 * control call like the CLI's: from this machine, with no `Origin`.
 */
function admitOrigin(port: number, origin: string): Promise<void> {
  const body = JSON.stringify({ origin });
  return new Promise((resolve, reject) => {
    const request = http.request(
      {
        host: '127.0.0.1',
        port,
        path: '/webview-origins',
        method: 'POST',
        timeout: 5000,
        headers: { 'Content-Type': 'application/json', 'Content-Length': Buffer.byteLength(body) },
      },
      (response) => {
        let text = '';
        response.setEncoding('utf8');
        response.on('data', (chunk) => (text += chunk));
        response.on('end', () =>
          response.statusCode === 200 ? resolve() : reject(new Error(`HTTP ${response.statusCode}: ${text.trim()}`)),
        );
      },
    );
    request.on('timeout', () => request.destroy(new Error('the daemon did not answer within 5 seconds')));
    request.on('error', reject);
    request.end(body);
  });
}

/** An event the bridge sends in the page for a probe (`PagePanel.probe`). */
export interface PageEvent {
  type: string;
  detail?: unknown;
  selector?: string;
}

/** What the extension does with what the page tells it, besides admitting it. */
interface PageHooks {
  /** The page went to `hash`, or booted on it. */
  route(hash: string): void;
  /** The page asks for the highlight whose id is `anchor` to be shown in the editor. */
  reveal(anchor: string): void;
  /** The panel was closed. */
  closed(): void;
}

/**
 * The view type of the page's panel, by which VS Code, bringing a saved panel back, finds the
 * serializer that builds the page in it again; the manifest's activation event names it too.
 */
const PAGE_VIEW = 'fwdPage';

/** The page's files: the page the build copied from crates/fwd-web/dist. */
function pageFiles(context: vscode.ExtensionContext): vscode.Uri {
  return vscode.Uri.joinPath(context.extensionUri, 'dist', 'web');
}

/** What the page's webview may do: run scripts, and load the page's files and nothing else. */
function pageOptions(files: vscode.Uri): vscode.WebviewOptions {
  return { enableScripts: true, localResourceRoots: [files] };
}

/**
 * The route the page was on, from the webview's state that VS Code saved with its panel, where the
 * bridge keeps it (`setState`); the graph's when the state holds none.
 */
function savedRoute(state: unknown): string {
  const hash = (state as { hash?: unknown } | null | undefined)?.hash;
  return typeof hash === 'string' && hash.startsWith('#/') ? hash : '#/graph';
}

/**
 * The web UI's page (crates/fwd-web, the Leptos app the daemon serves to browsers) in a webview
 * panel, pinned to the window's project. The page is an ephemeral node of its own with its own
 * connection to the daemon, so it reads and writes the graph itself; `postMessage` carries only
 * the route a command opens it at, the page's origin for the extension to register with the
 * daemon, how the page is doing, the route the page is on (the window's current task follows it),
 * the highlight the page asks to have shown in the editor (`reveal`), the page's request for the
 * window to reload (`reload`), and the probe the headless test uses. One panel serves every route.
 * VS Code saves an open panel with the window and brings it back after the window reloads or
 * restarts (`revive`).
 */
class PagePanel {
  static current: PagePanel | undefined;
  private waiting = new Map<number, (report?: PageReport) => void>(); // unanswered probes, by id
  private probes = 0;
  private disposed = false;
  /** The daemon the page's document was built for. */
  private daemon!: Daemon;
  /**
   * How the page last said it was doing, as the bridge relays it: `failed` when it could not
   * boot, else its connection's state (`Connected`, `Error`, ...); undefined until it says, which
   * each new document starts again.
   */
  private state: string | undefined;

  /**
   * Shows the page at `route` (`#/graph`, `#/inbox`, `#/tasks/<id>`, `#/decisions/<id>`), in a new
   * panel or in the open one. An open page that is booting or connected is moved to the route. One
   * that could not boot, or has lost the daemon, gets a new document instead, built for the daemon
   * as `locate` finds it now (started when none runs, and perhaps on another port since), so
   * opening the page again boots it again, as the page's failure sentence says.
   */
  static async show(
    context: vscode.ExtensionContext,
    daemon: Daemon,
    route: string,
    locate: () => Promise<Daemon | undefined>,
    hooks: PageHooks,
  ) {
    const open = PagePanel.current;
    if (open && (open.state === undefined || open.state === 'Connected')) {
      open.panel.reveal();
      // VS Code holds the message until the document has loaded, also when revealing the panel
      // made it build the document again.
      void open.panel.webview.postMessage({ type: 'route', hash: route });
      return;
    }
    if (open) {
      const now = await locate();
      if (open.disposed) return;
      open.load(now ?? open.daemon, route);
      open.panel.reveal();
      return;
    }
    const files = pageFiles(context);
    const panel = vscode.window.createWebviewPanel(PAGE_VIEW, 'fwd', vscode.ViewColumn.Beside, {
      ...pageOptions(files),
      // A hidden page is dropped; shown again, it boots again and rebuilds from live queries, at
      // the route it was on (the bridge keeps the route in the webview's state).
      retainContextWhenHidden: false,
    });
    PagePanel.current = new PagePanel(panel, files, daemon, route, hooks);
    context.subscriptions.push(panel);
  }

  /**
   * Shows the page at `route` in `panel`, the page's panel VS Code brought back after the window
   * reloaded or restarted. The panel gets the options a new one gets, since the ones VS Code saved
   * with it name the files of the build that saved it, which a reload after an update replaces.
   */
  static revive(context: vscode.ExtensionContext, panel: vscode.WebviewPanel, daemon: Daemon, route: string, hooks: PageHooks) {
    const files = pageFiles(context);
    panel.webview.options = pageOptions(files);
    PagePanel.current = new PagePanel(panel, files, daemon, route, hooks);
    context.subscriptions.push(panel);
  }

  private constructor(
    readonly panel: vscode.WebviewPanel,
    private files: vscode.Uri,
    daemon: Daemon,
    route: string,
    private hooks: PageHooks,
  ) {
    // The tab shows the page's favicon, the project's mark, as a browser's tab does: set here,
    // where a new panel and one VS Code brought back (`revive`) both pass, so that each shows this
    // build's file, as `revive` gives a panel this build's options.
    panel.iconPath = vscode.Uri.joinPath(files, 'mark.png');
    panel.webview.onDidReceiveMessage((message) => this.receive(message));
    panel.onDidDispose(() => {
      this.disposed = true;
      if (PagePanel.current === this) PagePanel.current = undefined;
      hooks.closed();
    });
    this.load(daemon, route);
  }

  /** Gives the panel a new document, for the page at `route` on `daemon`, which boots the page afresh. */
  private load(daemon: Daemon, route: string) {
    this.daemon = daemon;
    this.state = undefined;
    // Every document differs (its nonce), so VS Code loads it even where the last was the same.
    this.panel.webview.html = pageHtml(this.panel.webview, this.files, daemon, route);
  }

  private receive(message: any) {
    switch (message?.type) {
      case 'probe':
        this.waiting.get(message.id)?.(message.report);
        break;
      case 'state':
        if (typeof message.state === 'string') this.state = message.state;
        break;
      case 'origin':
        // Before the page boots, and again each time its connection drops.
        void this.admit(String(message.origin));
        break;
      case 'route':
        if (typeof message.hash === 'string') this.hooks.route(message.hash);
        break;
      case 'reveal':
        if (typeof message.anchor === 'string') this.hooks.reveal(message.anchor);
        break;
      case 'reload':
        // The page's bar says the daemon restarted on a newer build, and the person asked for its
        // page. The page's files are this extension's, which only a reload of the window replaces
        // with those of a newer build of the extension, when one is installed. VS Code then brings
        // the panel back, on the route the page was on, with the same current task (`revive`).
        void vscode.commands.executeCommand('workbench.action.reloadWindow');
        break;
    }
  }

  /**
   * Registers the page's origin with its daemon, then tells the bridge, which lets the page boot
   * either way: refused, the page says it cannot reach the daemon.
   */
  private async admit(origin: string) {
    try {
      await admitOrigin(this.daemon.port, origin);
    } catch (e: any) {
      console.warn(`fwd: the daemon did not admit the page's origin ${origin}: ${e?.message ?? e}`);
    }
    if (!this.disposed) void this.panel.webview.postMessage({ type: 'admitted' });
  }

  /**
   * What the page shows now, as its bridge reports it; undefined when no answer comes within `ms`.
   * With `dispatch`, the bridge first sends an event: a mouse event of that type (a click) on the
   * first element `selector` finds, else an event with `detail` on the page's window, as the page
   * itself sends one. It is how the headless test clicks on the page.
   */
  probe(ms = 1000, dispatch?: PageEvent): Promise<PageReport | undefined> {
    const id = ++this.probes;
    return new Promise((resolve) => {
      const answer = (report?: PageReport) => {
        clearTimeout(timer);
        this.waiting.delete(id);
        resolve(report);
      };
      const timer = setTimeout(answer, ms);
      this.waiting.set(id, answer);
      void this.panel.webview.postMessage({ type: 'probe', id, dispatch });
    });
  }
}

/**
 * The page's document in the webview. The page reads its parameters from its URL and its route
 * from the URL's fragment, but VS Code gives a webview's document a URL of its own, so the bridge
 * (below) adds them before the page starts: `daemon` (the daemon's address, since the page's
 * origin is the webview's and not the daemon's), `p` (the window's project) and `lock=1` (no
 * project selector). The page's module loads meanwhile, and the page boots once the extension
 * has registered the webview's origin with the daemon (`window.fwdAdmitted`, which the bridge
 * settles). The Content Security Policy lets the page load only its script, wasm, stylesheet and
 * images from dist/web (compiling the wasm needs 'wasm-unsafe-eval'; the stylesheet shows the mark
 * beside the name from mark.png), and connect only to those files (the page fetches its wasm) and
 * to the daemon: `/health` and `/projects` over HTTP, then the project's socket.
 */
function pageHtml(webview: vscode.Webview, files: vscode.Uri, daemon: Daemon, route: string): string {
  const nonce = randomBytes(16).toString('base64');
  // Tells this document from the ones built before it: a document VS Code loads again after
  // hiding it has the same, and goes back to the route it was on.
  const load = randomBytes(8).toString('hex');
  const file = (name: string) => webview.asWebviewUri(vscode.Uri.joinPath(files, name)).toString();
  const address = `http://127.0.0.1:${daemon.port}`;
  const policy = [
    "default-src 'none'",
    `script-src 'nonce-${nonce}' ${webview.cspSource} 'wasm-unsafe-eval'`,
    `style-src 'nonce-${nonce}' ${webview.cspSource}`,
    `img-src ${webview.cspSource}`,
    `connect-src ${webview.cspSource} ${address} ws://127.0.0.1:${daemon.port}`,
  ].join('; ');
  const params = { daemon: address, p: daemon.project, lock: '1' };
  // VS Code's default styles pad the body; the page lays itself out from the edges, as in a browser.
  return `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="${attribute(policy)}">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>fwd</title>
<script nonce="${nonce}">
${BRIDGE}
bridge(${literal(params)}, ${literal(route)}, ${literal(load)});
</script>
<link rel="stylesheet" href="${attribute(file('app.css'))}">
<style nonce="${nonce}">body { padding: 0; }</style>
<script type="module" nonce="${nonce}">
try {
  const [{ default: start }] = await Promise.all([import(${literal(file('fwd-web.js'))}), window.fwdAdmitted]);
  await start({ module_or_path: ${literal(file('fwd-web_bg.wasm'))} });
} catch (e) {
  document.body.textContent = 'The page could not start in this view: ' + e;
  dispatchEvent(new CustomEvent('fwd:state', { detail: 'failed' }));
}
</script>
</head>
<body></body>
</html>
`;
}

/**
 * The script that runs in the webview before the page starts. It adds the page's parameters to
 * the document's URL and sets the route as its fragment, keeping VS Code's own parameters (VS Code
 * finds the webview's files by the `id` one). It keeps the route in the webview's state, so a page
 * dropped while hidden comes back where it was; VS Code saves that state with the panel, and the
 * extension reads the route there when VS Code brings the panel back after the window reloads
 * (`PagePanel.revive`). A new document the extension builds (`load` tells them apart) opens on
 * the route it was built for. It tells the extension each route the page is on (`route`), which
 * the window's current task follows. VS Code cancels a click on a link to a fragment in a webview
 * (it only scrolls to the element of that id), so the bridge follows the page's links to its own
 * routes, except a link whose click the page cancelled.
 *
 * The daemon lets the page in only from an origin the extension registered with it, so the
 * bridge asks the extension to register the document's origin (`origin`) and settles
 * `window.fwdAdmitted`, on which the page's boot waits, when the extension answers (`admitted`),
 * or after five seconds, when the page boots anyway and says what it cannot reach. It relays how
 * the page is doing (the page's `fwd:state` events: `failed`, or its connection's state) to the
 * extension, and asks for the registration again each time the connection drops, since a
 * restarted daemon has forgotten it; the page's own retries then get in. When the page asks for a
 * highlight to be shown in the editor (its `fwd:reveal` event, sent during a click or a key press
 * on a step or a highlight, whose detail is the anchor's id), the bridge takes the request by
 * cancelling the event, so the page stays where it is, and hands the extension a `reveal` message
 * with the anchor. When the button on the page's bar that offers a newer page (shown once the
 * daemon has restarted on a newer build) is clicked, the page asks to be loaded again (its
 * `fwd:reload` event). Its files are the extension's, so a reload of the page would load the same
 * files again: the bridge takes the request by cancelling the event, so the page does not reload
 * itself, and hands the extension a `reload` message, on which the extension has VS Code reload
 * the window. It answers the extension: `route` moves the page, `probe` reports what the page
 * shows, with what the Content Security Policy refused (the bridge runs before anything else the
 * document loads, so it hears every refusal) and the webview's state, after sending the event the
 * probe carries, if any (`PagePanel.probe`).
 */
const BRIDGE = `function bridge(params, route, load) {
  const vscode = acquireVsCodeApi();
  const refused = [];
  addEventListener('securitypolicyviolation', (event) => refused.push(event.effectiveDirective + ' ' + event.blockedURI));
  const url = new URL(location.href);
  for (const [name, value] of Object.entries(params)) url.searchParams.set(name, value);
  const saved = vscode.getState() || {};
  url.hash = (saved.load === load && saved.hash) || route;
  history.replaceState(null, '', url);
  const remember = () => {
    vscode.setState({ load, hash: location.hash });
    vscode.postMessage({ type: 'route', hash: location.hash });
  };
  remember();
  addEventListener('hashchange', remember);
  addEventListener('click', (event) => {
    const link = event.composedPath().find((node) => node instanceof HTMLAnchorElement);
    const href = link ? link.getAttribute('href') : null;
    // A link the page cancelled stays where it is: a highlight the extension shows instead.
    if (href && href.startsWith('#/') && !event.defaultPrevented) location.hash = href;
  });
  let admitting = null;
  const admit = () => {
    if (!admitting) {
      admitting = new Promise((resolve) => {
        const done = () => {
          clearTimeout(timer);
          removeEventListener('message', answer);
          admitting = null;
          resolve();
        };
        const answer = (event) => {
          if ((event.data || {}).type === 'admitted') done();
        };
        const timer = setTimeout(done, 5000);
        addEventListener('message', answer);
        vscode.postMessage({ type: 'origin', origin: location.origin });
      });
    }
    return admitting;
  };
  window.fwdAdmitted = admit();
  let state = null;
  addEventListener('fwd:state', (event) => {
    state = String(event.detail);
    vscode.postMessage({ type: 'state', state });
    if (state !== 'Connected' && state !== 'failed') admit();
  });
  addEventListener('fwd:reveal', (event) => {
    event.preventDefault();
    vscode.postMessage({ type: 'reveal', anchor: String(event.detail) });
  });
  addEventListener('fwd:reload', (event) => {
    event.preventDefault();
    vscode.postMessage({ type: 'reload' });
  });
  addEventListener('message', (event) => {
    const message = event.data || {};
    if (message.type === 'route' && typeof message.hash === 'string' && message.hash.startsWith('#/')) {
      location.hash = message.hash;
    } else if (message.type === 'probe') {
      const sent = message.dispatch;
      if (sent && sent.selector) {
        const on = document.querySelector(sent.selector);
        if (on) on.dispatchEvent(new MouseEvent(sent.type, { bubbles: true, cancelable: true }));
      } else if (sent) {
        dispatchEvent(new CustomEvent(sent.type, { detail: sent.detail, cancelable: true }));
      }
      const text = document.body ? document.body.textContent : '';
      const styles = Array.from(document.styleSheets, (sheet) => sheet.href).filter(Boolean);
      const report = { title: document.title, origin: location.origin, search: location.search, hash: location.hash, text, styles, refused, state, saved: vscode.getState() };
      vscode.postMessage({ type: 'probe', id: message.id, report });
    }
  });
}`;

/** `value` as a JavaScript literal inside a <script> element. */
function literal(value: unknown): string {
  return JSON.stringify(value).replace(/</g, '\\u003c');
}

/** `value` inside a double-quoted HTML attribute. */
function attribute(value: string): string {
  return value.replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

// ---------------------------------------------------------------------------------------------
// Activation

/** How long a node may take to join the daemon. */
const JOIN_TIMEOUT_MS = 8000;
/**
 * After the node loses the daemon, the wait before the first `fwd daemon status`, and the
 * longest wait between two; each wait doubles the one before, as `fwd watch` waits.
 */
const FIRST_RETRY_MS = 1000;
const LONGEST_RETRY_MS = 30_000;

/** The project's socket on the daemon; Ankurah's WebSocket client appends `/ws`. */
function socketOf(daemon: Daemon): string {
  return `ws://127.0.0.1:${daemon.port}/p/${daemon.project}`;
}

/**
 * The window's connection to its project: the node, and the views that read through it. When the
 * node's socket to the daemon closes (the daemon stopped, or restarted to install a new build),
 * the connection waits for the daemon to answer again and joins a new node to it, onto which the
 * views' live queries move (`rejoinWhenBack`).
 */
class Connection {
  node: HostNode | undefined;
  /**
   * Where the daemon listens and the window's project, once `fwd daemon status` has answered:
   * at activation, and again each time the node joins the daemon anew; undefined for a folder
   * that belongs to no project, or a window where `fwd` cannot run.
   */
  daemon: Promise<Daemon | undefined>;
  readonly ready: Promise<void>;
  private closed = false;
  /** Whether `rejoinWhenBack` is waiting for the daemon, or joining it. */
  private rejoining = false;
  /** Ends the wait `rejoinWhenBack` is in: `close` cuts it short. */
  private endPause: (() => void) | undefined;

  constructor(
    private principals: Principals,
    private referents: Referents,
    private comments: Comments,
    private badges: WholeFileBadges,
    private current: CurrentTask,
    private inbox: Inbox,
  ) {
    this.daemon = this.locate();
    this.ready = this.daemon.then((daemon) => (daemon ? this.join(daemon) : undefined));
  }

  /**
   * Starts the daemon when none runs (`fwd daemon start` spawns it from the same binary) and asks
   * it which project the workspace folder belongs to (`fwd daemon status`): at activation, and
   * again when a command opens a page that could not reach the daemon. A folder that belongs to
   * no project, or a window where `fwd` cannot run, gets no node and says nothing, as the proof of
   * concept stayed quiet there.
   */
  async locate(): Promise<Daemon | undefined> {
    if (!workspaceRoot()) return undefined;
    try {
      await fwd(['daemon', 'start']);
      return await this.status();
    } catch (e) {
      console.warn('fwd: no daemon for this window:', e);
      return undefined;
    }
  }

  /**
   * Where the daemon listens and the window's project, from `fwd daemon status`, which never
   * starts a daemon and fails when none answers.
   */
  private async status(): Promise<Daemon | undefined> {
    const status = await fwd(['daemon', 'status']);
    return status?.project ? { port: status.port, project: status.project } : undefined;
  }

  /**
   * Joins the node to the project's socket, and starts on it the names of the project's principals
   * and of the other things the threads and the inbox name, the open files' threads and
   * highlights, the whole-file badges, the current task's title and the inbox.
   */
  private async join(daemon: Daemon) {
    const root = workspaceRoot();
    const prefix = (root && (await repositoryPrefix(root))) ?? '';
    const node = await HostNode.connect(socketOf(daemon), JOIN_TIMEOUT_MS);
    if (this.closed) return node.close();
    this.node = node;
    node.onLost(() => void this.rejoinWhenBack());
    this.principals.start(node);
    this.referents.start(node);
    this.comments.start(node, prefix);
    this.badges.start(node);
    this.current.useNode(node);
    await this.inbox.start(node);
  }

  /**
   * After the node's socket to the daemon closes: asks `fwd daemon status` after a second, then
   * after two, four and so on up to thirty, until the daemon answers (it never starts one: a
   * daemon stopped to install a new build stays stopped until something else starts it); then
   * joins a new node at the port the daemon answers on, and the views' live queries move onto it
   * (`HostNode.rejoin`). A new node rather than the old one's socket back, because Ankurah's
   * client retries only the address the old node joined at: it reconnects by itself when the
   * daemon comes back on the same port, but never finds one that came back on another (its old
   * port taken meanwhile), and a node that halted does not retry at all.
   */
  private async rejoinWhenBack() {
    const node = this.node;
    if (!node || this.rejoining || this.closed) return;
    this.rejoining = true;
    try {
      for (let wait = FIRST_RETRY_MS; ; wait = Math.min(wait * 2, LONGEST_RETRY_MS)) {
        await this.sleep(wait);
        if (this.closed) return;
        const daemon = await this.status().catch(() => undefined);
        if (!daemon || this.closed) continue;
        try {
          await node.rejoin(socketOf(daemon), JOIN_TIMEOUT_MS);
        } catch (e) {
          console.warn(`fwd: the node could not join the daemon on port ${daemon.port} again:`, e);
          continue;
        }
        this.daemon = Promise.resolve(daemon);
        console.info(`fwd: the node joined the daemon again, on port ${daemon.port}`);
        // A daemon that went away again meanwhile is waited for again.
        if (node.connected || this.closed) return;
      }
    } finally {
      this.rejoining = false;
    }
  }

  /** Waits `ms`, or less when `close` runs meanwhile. */
  private sleep(ms: number): Promise<void> {
    return new Promise((resolve) => {
      const end = () => {
        clearTimeout(timer);
        this.endPause = undefined;
        resolve();
      };
      const timer = setTimeout(end, ms);
      this.endPause = end;
    });
  }

  async close() {
    this.closed = true;
    this.endPause?.();
    this.principals.close();
    this.comments.close();
    this.referents.close();
    this.badges.close();
    this.current.useNode(undefined);
    // The window stops following the project: its current task goes, kept for the page's panel,
    // which VS Code brings back after a reload.
    this.current.forget();
    await this.node?.close();
  }
}

/** What `activate` returns: the parts the headless test (src/test) looks at. */
export interface Api {
  /**
   * Settles once the node has joined and the inbox and the open files follow it (at once in a
   * folder that belongs to no project); rejects with the reason the node could not join.
   */
  ready: Promise<void>;
  inbox: Inbox;
  /** The comment threads shown on `uri`. */
  threads(uri: vscode.Uri): vscode.CommentThread[];
  /** Each document threads are shown on, by URI, with its threads: what the Comments panel lists them under. */
  threadsByDocument(): Map<string, vscode.CommentThread[]>;
  /** The highlights drawn on an editor showing `uri`, or undefined when none shows it. */
  highlights(uri: vscode.Uri): vscode.Range[] | undefined;
  /** The badge on the name of the file `uri`, in the explorer and on its tab. */
  badge(uri: vscode.Uri): vscode.FileDecoration | undefined;
  /** What the status bar says of the highlights in the active editor's file, while it says anything. */
  count(): string | undefined;
  /** The window's current task, what the status bar says of it, and what a comment box fwd opens says. */
  current(): { id?: string; title?: string; status?: string; placeHolder?: string };
  /**
   * The wording the comment controller carries now, which VS Code gives each comment box it
   * builds: none but while fwd opens a new comment's box (src/comments.ts, openCommentBox).
   */
  commentOptions(): vscode.CommentOptions | undefined;
  /**
   * What the mention search in a comment box offers while the box's line ends with `before`
   * (src/comments.ts, `mentions`).
   */
  mentions(before: string): Promise<vscode.CompletionItem[]>;
  /** The version the build stamped (version.mjs). */
  version: string;
  /** Once the node has joined: whether it is closed, and how many live queries and guards it holds. */
  node(): { closed: boolean; held: number } | undefined;
  /**
   * Joins a new node in the node's place now, at the port the daemon last answered on, as the
   * window does once the daemon is back after a restart (`HostNode.rejoin`).
   */
  rejoin(): Promise<void>;
  /**
   * After `deactivate`, joins the window to its project again as activation does, the same views
   * following the new node; settles as `ready` does.
   */
  reconnect(): Promise<void>;
  /**
   * The page's panel while one is open: the document it was given, what it shows now, and a way
   * to move it to a route as a command does.
   */
  page():
    | {
        html: string;
        probe(ms?: number, dispatch?: PageEvent): Promise<PageReport | undefined>;
        go(hash: string): void;
      }
    | undefined;
  /**
   * The serializer the extension registers for the page's panel, which VS Code hands the panel it
   * brings back after the window reloads, with the state it saved; the test hands it one as
   * VS Code would, since a reload would end the run.
   */
  pageSerializer: vscode.WebviewPanelSerializer;
}

let connection: Connection | undefined;

export function activate(context: vscode.ExtensionContext): Api {
  findBinary();
  const current = new CurrentTask(context);
  const principals = new Principals();
  const referents = new Referents();
  const comments = new Comments(context, current, principals, referents);
  const badges = new WholeFileBadges((rel) => comments.absPath(rel));
  const inbox = new Inbox(principals, referents);
  context.subscriptions.push(
    vscode.window.registerTreeDataProvider('fwdInbox', inbox),
    vscode.window.registerFileDecorationProvider(badges),
  );
  /** Joins the window to its project: at activation, and again when the headless test reconnects. */
  const connect = () => {
    const joining = (connection = new Connection(principals, referents, comments, badges, current, inbox));
    // Once deactivated, a startup still under way fails on the closed node; that is no news.
    joining.ready.catch((e) => {
      if (connection === joining) vscode.window.showErrorMessage(`fwd: ${e?.message ?? e}`);
    });
    return joining;
  };
  let c = connect();
  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration((e) => {
      if (!e.affectsConfiguration('fwd.as')) return;
      inbox.refresh().catch((err) => vscode.window.showErrorMessage(`fwd: ${err?.message ?? err}`));
      // Whose comments offer Edit follows whom the window acts as.
      comments.identify().catch((err) => vscode.window.showErrorMessage(`fwd: ${err?.message ?? err}`));
    })
  );

  const cmd = (name: string, f: (...a: any[]) => Promise<void> | void) =>
    context.subscriptions.push(
      vscode.commands.registerCommand(name, async (...a: any[]) => {
        try {
          await f(...a);
        } catch (e: any) {
          vscode.window.showErrorMessage(e.message ?? String(e));
        }
      })
    );

  /**
   * Opens a highlight in an editor group other than the page's: the active one when it can; with
   * `version`, a file a message names, at that version (src/reveal.ts).
   */
  const showHighlight = (anchor: string, version?: string) => {
    const page = PagePanel.current?.panel.viewColumn;
    const groups = [vscode.window.tabGroups.activeTabGroup, ...vscode.window.tabGroups.all];
    const column = groups.find((g) => g.viewColumn !== page)?.viewColumn ?? vscode.ViewColumn.Beside;
    return reveal(anchor, (rel) => comments.absPath(rel), column, version);
  };
  const hooks: PageHooks = {
    route: (hash) => void current.follow(hash),
    // The page asks for a highlight by its id, and for a file a message names by the id, an
    // at-sign and the version, as the message's text stores it (crates/fwd-web, `session::reveal`).
    reveal: (asked) => {
      const [anchor, version] = asked.split('@');
      void showHighlight(anchor, version !== undefined && FILE_VERSION.test(version) ? version : undefined).catch((e) =>
        vscode.window.showErrorMessage(`fwd: ${e?.message ?? e}`)
      );
    },
    // The person closed the panel. A panel closed as the extension stops, after `deactivate` (VS Code
    // disposes the extension's subscriptions, the panel among them), is one VS Code saved with the
    // window to bring back after a reload, and the kept current task comes back with it.
    closed: () => {
      if (connection) current.clear(true);
    },
  };

  /** Opens the page at `route`, pinned to the window's project. */
  const openPage = async (route: string) => {
    const daemon = await c.daemon;
    if (!daemon) throw new Error("This window's folder belongs to no fwd project, or fwd could not start its daemon here.");
    await PagePanel.show(context, daemon, route, () => c.locate(), hooks);
  };

  // VS Code saves the page's open panel with the window, and when the window reloads or restarts
  // (the page's Reload window among the reasons) it brings the panel back and hands it here, with
  // the webview's state, where the bridge kept the route the page was on. The page is built again
  // there, and the current task kept with the workspace's state is current again. A folder with no
  // project has no page, and a page opened meanwhile by a command keeps its place: one panel
  // serves every route.
  const pageSerializer: vscode.WebviewPanelSerializer = {
    deserializeWebviewPanel: async (panel, state) => {
      const daemon = await c.daemon;
      if (!daemon || PagePanel.current) return panel.dispose();
      current.restore();
      PagePanel.revive(context, panel, daemon, savedRoute(state), hooks);
    },
  };
  context.subscriptions.push(vscode.window.registerWebviewPanelSerializer(PAGE_VIEW, pageSerializer));

  cmd('fwd.showGraph', () => openPage('#/graph'));
  cmd('fwd.showInbox', () => openPage('#/inbox'));
  cmd('fwd.refreshInbox', () => inbox.refresh());
  // A new thread's box: Send to agents, or Comment on the task while one is current, and Ask, which
  // asks the project.
  cmd('fwd.createComment', (reply: vscode.CommentReply) => comments.create(reply, 'comment'));
  cmd('fwd.createTaskComment', (reply: vscode.CommentReply) => comments.create(reply, 'comment'));
  cmd('fwd.createQuestion', (reply: vscode.CommentReply) => comments.create(reply, 'question'));
  // A thread's reply box, which every thread carries: its Reply sends what was written.
  cmd('fwd.replyComment', (reply: vscode.CommentReply) => comments.replyTo(reply));
  // A thread's title bar: Resolve or Unresolve, and Add as a Step while a task is current. On the
  // first thread of a highlight that is a step, fwd moves the step with the thread, and the same
  // two actions show under titles that say so.
  cmd('fwd.resolveThread', (thread: vscode.CommentThread) => comments.resolve(thread));
  cmd('fwd.resolveThreadAndCheckStep', (thread: vscode.CommentThread) => comments.resolve(thread));
  cmd('fwd.unresolveThread', (thread: vscode.CommentThread) => comments.unresolve(thread));
  cmd('fwd.unresolveThreadAndUncheckStep', (thread: vscode.CommentThread) => comments.unresolve(thread));
  cmd('fwd.createStep', (thread: vscode.CommentThread) => comments.addAsStep(thread));
  // A comment of one's own: Edit on its title bar, then Cancel and Save beneath its editor, which
  // VS Code hands the comment with the editor's text as its body.
  cmd('fwd.editComment', (comment: vscode.Comment) => comments.edit(comment));
  cmd('fwd.saveComment', (comment: vscode.Comment) => comments.save(comment));
  cmd('fwd.cancelEditComment', (comment: vscode.Comment) => comments.cancelEdit(comment));
  cmd('fwd.openThread', (uri: string, root: string) => comments.openThread(uri, root));
  // A reference in a comment, drawn as a link (src/references.ts, markdown): a task or a decision
  // opens on the page, a scope opens the page's tasks worked in it, and lines of code open in the
  // editor as a step clicked on the page opens them. The line of what a highlight is joined to,
  // closing its first thread's first comment, links the same way, and its count of the messages
  // naming the highlight opens the highlight's own page (`highlight`), which lists them. A file a
  // message names at a version (`file`) opens in the editor at that version (src/reveal.ts). A
  // message's author can write such a link by hand, so the id must be one, and the version one a
  // stored mention writes, before it reaches the CLI or the page.
  cmd('fwd.openReference', async (id: string, kind: string, version?: string) => {
    if (!/^[A-Za-z0-9_-]{43}$/.test(String(id))) return;
    if (kind === 'code') return showHighlight(id);
    if (kind === 'file') return FILE_VERSION.test(String(version)) ? showHighlight(id, String(version)) : undefined;
    const route = ({ task: `#/tasks/${id}`, decision: `#/decisions/${id}`, scope: `#/tasks?scope=${id}`, highlight: `#/anchors/${id}` } as Record<string, string>)[kind];
    if (route) await openPage(route);
  });
  // A thread's title bar: the highlight the thread is on, on its own page, where what it is joined
  // to, its lines and all its threads show.
  cmd('fwd.openHighlightPage', async (thread: vscode.CommentThread) => {
    const anchor = comments.anchors.get(thread);
    if (anchor) await openPage(`#/anchors/${anchor}`);
  });
  // From the editor's menus VS Code hands these the document the menu was opened on; from the
  // palette, nothing, and they act on the active editor.
  cmd('fwd.commentOnSelection', (uri?: vscode.Uri) => comments.openCommentBox(uri, 'lines'));
  cmd('fwd.askOnSelection', (uri?: vscode.Uri) => comments.askAbout(uri));
  cmd('fwd.stepOnSelection', (uri?: vscode.Uri) => comments.stepAbout(uri, 'lines'));
  cmd('fwd.commentOnFile', (uri?: vscode.Uri) => comments.openCommentBox(uri, 'file'));
  cmd('fwd.stepOnFile', (uri?: vscode.Uri) => comments.stepAbout(uri, 'file'));
  cmd('fwd.clearCurrentTask', () => current.clear());
  cmd('fwd.readNotification', (item: InboxItem) => inbox.setStatus(item, 'read'));
  cmd('fwd.dismissNotification', (item: InboxItem) => inbox.setStatus(item, 'dismissed'));
  cmd('fwd.replyNotification', async (item: InboxItem) => {
    const text = await vscode.window.showInputBox({ prompt: 'Reply', placeHolder: 'what you did, or your answer' });
    if (!text) return;
    await fwd(['reply', item.n.id, text]); // it marks the notification read, which the inbox hears
  });
  cmd('fwd.openNotification', async (item: InboxItem) => {
    const t = item.n.target ?? {};
    if (t.type === 'CodeAnchor') {
      // The highlight, where a step of it on the page would open it.
      if (workspaceRoot()) await showHighlight(t.id);
    } else {
      // The page, at the task or the decision the notification is about, else at the inbox.
      const section = ({ Task: 'tasks', Decision: 'decisions' } as Record<string, string>)[t.type];
      await openPage(section && typeof t.id === 'string' ? `#/${section}/${t.id}` : '#/inbox');
    }
    if (item.n.status === 'unread') await inbox.setStatus(item, 'read');
  });

  return {
    ready: c.ready,
    inbox,
    threads: (uri) => comments.threadsOn(uri),
    threadsByDocument: () => comments.threadsByDocument(),
    highlights: (uri) => comments.paintedOn(uri),
    badge: (uri) => badges.provideFileDecoration(uri),
    count: () => comments.countShown(),
    current: () => ({ id: current.id, title: current.title, status: current.status, placeHolder: comments.newCommentWording().placeHolder }),
    commentOptions: () => comments.controller.options,
    mentions: (before) => comments.mentions(before),
    version: stampedVersion(context),
    node: () => c.node?.state(),
    rejoin: async () => {
      const daemon = await c.daemon;
      if (!daemon || !c.node) throw new Error('this window has no node');
      await c.node.rejoin(socketOf(daemon), JOIN_TIMEOUT_MS);
    },
    reconnect: () => {
      if (connection) return Promise.reject(new Error('this window is still connected: deactivate it first'));
      c = connect();
      return c.ready;
    },
    page: () => {
      const open = PagePanel.current;
      return (
        open && {
          html: open.panel.webview.html,
          probe: (ms, dispatch) => open.probe(ms, dispatch),
          go: (hash) => void open.panel.webview.postMessage({ type: 'route', hash }),
        }
      );
    },
    pageSerializer,
  };
}

/**
 * The version the build stamped with the commit it was made from (version.mjs writes it to
 * dist/version.json, and `npm run package` gives the VSIX the same), else package.json's.
 */
function stampedVersion(context: vscode.ExtensionContext): string {
  try {
    return JSON.parse(fs.readFileSync(path.join(context.extensionPath, 'dist', 'version.json'), 'utf8')).version;
  } catch {
    return context.extension.packageJSON.version;
  }
}

/**
 * Frees the node and every handle holding it, and removes the threads, the highlights and the
 * badges. VS Code calls this when the extension stops; the headless test calls it too. Running it
 * again does nothing.
 */
export async function deactivate() {
  const c = connection;
  connection = undefined;
  await c?.close();
}
