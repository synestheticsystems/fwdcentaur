// The extension host's own Ankurah node (crates/fwd-host: fwd-core compiled to WebAssembly for
// Node), joined to the project's daemon. It exists so the inbox and the comment threads change
// when the project changes, through live queries, rather than when a timer runs the CLI again.
// Everything that touches the package is here: loading it, the queries the extension keeps open,
// turning the views they return into plain objects, watching the node's socket to the daemon,
// moving the queries onto a new node once the daemon is back after a restart, and freeing what
// would otherwise keep a node alive, once no call into the package still uses it.

import type * as FwdHost from '../../../crates/fwd-host/pkg/fwd_host';

type Lib = typeof FwdHost;

/** A value wasm-bindgen handed to JavaScript; `free()` drops the Rust value behind it. */
interface Handle {
  free(): void;
}

/** What the live query classes the Model derive generates have in common. */
interface LiveQuery<V extends Handle> extends Handle {
  readonly loaded: boolean;
  readonly peek: V[];
  subscribe(callback: (changes: Handle) => void, immediate?: boolean): Handle;
}

/** A notification, spelled as fwd-core's `json_of_notification` (and so `fwd inbox`) spells it. */
export interface NotificationJson {
  id: string;
  type: string;
  status: string;
  priority: string;
  created: number;
  target: any;
  originator: any;
}

/**
 * An anchor's path and the lines it was made on: 0-based, the end exclusive. `whole` marks an
 * anchor on the whole file, which has no lines (its start and end are both 0). `onDisk` marks one
 * made on the file on disk, whose lines are the working tree's of that moment, where one made on a
 * revision's side of a diff (`fwd comment --against`) counts that revision's lines.
 */
export interface AnchorLines {
  id: string;
  path: string;
  start_line: number;
  end_line: number;
  whole: boolean;
  onDisk: boolean;
}

/**
 * The fields of fwd-core's `json_of_message` a comment thread shows. `thread` is the root message's
 * id, and is set on replies only.
 */
export interface MessageJson {
  id: string;
  author: string;
  content: string;
  status: string;
  created: number;
  thread?: string;
  /** When its text was last edited, by its author or an agent that succeeded its author; none for a message never edited. */
  edited?: number | null;
}

/**
 * A step a highlight is: its `step_of` relation's id, the task whose checklist it is on, and whether
 * it is checked (the relation resolved).
 */
export interface StepJson {
  id: string;
  task: string;
  checked: boolean;
}

/**
 * A relation that joins a highlight to something besides the steps it is, pointing at the
 * highlight: from a task whose code the highlight is (`anchored_at`), or from a message whose text
 * names it (`mention`).
 */
export interface JoinJson {
  id: string;
  kind: 'anchored_at' | 'mention';
  /** The task whose code the highlight is, or the message that names it. */
  from: string;
}

/**
 * A principal the mention search offers: its id and name, what it is (person, its role, or agent),
 * the titles of the scopes its work is in beside a name two or more principals of the project share
 * (else null), how a message's text writes its mention (`@name`, or `@` and the start of its id for
 * a shared name), which posting stores as its id, and whether it is an agent that has written
 * nothing for twelve hours, which the search offers after the others (docs/DECISIONS.md, D123).
 */
export interface MentionOffer {
  id: string;
  handle: string;
  what: string;
  scopes: string[] | null;
  written: string;
  dormant: boolean;
}

/**
 * What a reference in a message's text names, and what it is called: a person by the handle, a
 * task and a scope by the title, a decision by its number and title, lines of code by the path
 * and lines (`path:lines`, or the path alone for a whole file, as fwd-core names a highlight on
 * the page and in the CLI: the package's `anchor_place`), a message by its first words.
 */
export type Referent =
  | { kind: 'person'; handle: string }
  | { kind: 'task' | 'scope'; title: string }
  | { kind: 'decision'; number: number; title: string }
  | { kind: 'code'; place: string }
  | { kind: 'message'; words: string };

/** A principal of the project, a person or an agent, and its name (its handle). */
export interface PrincipalName {
  id: string;
  handle: string;
}

/**
 * The window's node, and every live query the views hold through it. After the daemon restarts,
 * `rejoin` puts a new node in the old one's place and moves each live query onto it, so a view
 * keeps the `Live` it was given and never learns that the node changed.
 */
export class HostNode {
  // Every live query a view holds, each with its subscription. Each one holds its node, which
  // keeps a timer running while it lives, so `close` frees them all.
  private lives = new Set<Live<unknown>>();
  private closed = false;
  /** Whether the node's socket to the daemon is connected, as the node last said. */
  private up = true;
  /** Told each time the node's socket stops being connected (`onLost`). */
  private lost: (() => void) | undefined;
  /** The node in place, to which every call and every new live query goes. */
  private joined: Joined;
  /** The subscription to the state of the node's socket. */
  private watch: Handle;
  /** The nodes `rejoin` replaced that have not gone yet: each goes once its calls have settled. */
  private leaving = new Set<Joined>();

  private constructor(
    private lib: Lib,
    host: FwdHost.Host,
  ) {
    this.joined = new Joined(host, host.context());
    this.watch = this.follow(host);
  }

  /**
   * Joins a new in-memory node to the project's socket, `ws://127.0.0.1:<port>/p/<project>` with
   * both from `fwd daemon status`. Rejects with the package's sentence when the node has not
   * joined within `timeoutMs`.
   */
  static async connect(url: string, timeoutMs: number): Promise<HostNode> {
    // Required here rather than at the top: instantiating the 2.8 MB module is work a window
    // without a project never needs. The build copies the glue and its .wasm beside
    // dist/extension.js and leaves this require to run then (esbuild marks it external).
    const lib: Lib = require('./fwd_host.js');
    const host = await lib.Host.connect(url, timeoutMs);
    return new HostNode(lib, host);
  }

  /** Whether `close` has run, and how many live queries and subscription guards are held. */
  state(): { closed: boolean; held: number } {
    let held = 0;
    for (const live of this.lives) held += live.handles;
    return { closed: this.closed, held };
  }

  /** Whether the node's socket to the daemon is connected. */
  get connected(): boolean {
    return this.up && !this.closed;
  }

  /**
   * Calls `listener` each time the node's socket to the daemon stops being connected: the daemon
   * stopped or restarted, or the node halted. Ankurah's client then retries the address the node
   * was joined at, and nothing else, so it is `listener`'s part to find the daemon and `rejoin`.
   */
  onLost(listener: () => void): void {
    this.lost = listener;
    if (!this.up) setImmediate(() => this.tellLost(this.joined.host));
  }

  /**
   * Joins a new node at `url` in the old one's place: each live query opens again on the new node,
   * and each view goes on reading its query's last results until the new query has answered,
   * then reads the new one. Every call from then on goes to the new node, and the old node goes in
   * two steps: once the calls under way on it have settled (`close` says why nothing is freed
   * sooner), `Joined.leave` frees its context and closes its socket; once the last of its queries
   * has been replaced, nothing holds it any more. Rejects, changing nothing, when the new node has
   * not joined within `timeoutMs`.
   */
  async rejoin(url: string, timeoutMs: number): Promise<void> {
    if (this.closed) return;
    const host = await this.lib.Host.connect(url, timeoutMs);
    if (this.closed) {
      host.close();
      host.free();
      return;
    }
    const old = this.joined;
    this.joined = new Joined(host, host.context());
    const watch = this.watch;
    this.watch = this.follow(host);
    watch.free();
    for (const live of this.lives) live.reopen(this.joined.ctx);
    this.leaving.add(old);
    old.leave().then(
      () => this.leaving.delete(old),
      // Kept in `leaving`, so that `close` fails with the same error.
      (e) => console.error('fwd: the node a new one replaced could not be let go:', e),
    );
  }

  /**
   * The id of the principal `name` names, read as the CLI's `--as` reads it (its name, or its id),
   * or of the project's person (the first active human by handle, as the web UI chooses) when
   * `name` is empty; undefined when no principal has that name. A name two or more principals of
   * the project share is refused, with a sentence listing each by its id, which `fwd.as` then takes.
   */
  async principal(name: string): Promise<string | undefined> {
    const found = await this.lend((ctx) => (name ? this.lib.principal_named(ctx, name) : this.lib.default_principal(ctx)));
    return found?.id;
  }

  /**
   * What the mention search in a comment box offers for `typed`, what follows the `@`: at most
   * `most` principals that belong to the project whose name starts with it, the dormant agents
   * after the others (crates/fwd-host, `mention_offers`).
   */
  async mentionOffers(typed: string, most = 8): Promise<MentionOffer[]> {
    return this.lend((ctx) => this.lib.mention_offers(ctx, typed, most));
  }

  /** The handle of principal `id`, or undefined when the node cannot find it. */
  async handleOf(id: string): Promise<string | undefined> {
    try {
      return await this.lend(async (ctx) => {
        // `get` takes the EntityId it is given, so there is none left to free.
        const view = await this.lib.Principal.get(ctx, this.lib.EntityId.from_base64(id));
        try {
          return view.handle;
        } finally {
          view.free();
        }
      });
    } catch {
      return undefined;
    }
  }

  /** The text of message `id`, or undefined when the node cannot find it. */
  async contentOf(id: string): Promise<string | undefined> {
    try {
      return await this.lend(async (ctx) => {
        const view = await this.lib.Message.get(ctx, this.lib.EntityId.from_base64(id));
        try {
          return view.content;
        } finally {
          view.free();
        }
      });
    } catch {
      return undefined;
    }
  }

  /**
   * What the entity `id` is, for what names it in a view, a reference in a message's text among
   * them (src/referents.ts): undefined when the node finds nothing by that id. Each kind is asked
   * for in turn, people first, since mentions of people are the most common.
   */
  async describe(id: string): Promise<Referent | undefined> {
    return this.lend(async (ctx) => {
      const read = async <V extends Handle>(model: { get(ctx: FwdHost.Context, id: FwdHost.EntityId): Promise<V> }, name: (lib: Lib, view: V) => Referent) => {
        let view: V;
        try {
          view = await model.get(ctx, this.lib.EntityId.from_base64(id));
        } catch {
          return undefined;
        }
        try {
          return name(this.lib, view);
        } finally {
          view.free();
        }
      };
      return (
        (await read(this.lib.Principal, namePerson)) ??
        (await read(this.lib.Task, nameTask)) ??
        (await read(this.lib.Decision, nameDecision)) ??
        (await read(this.lib.Scope, nameScope)) ??
        (await read(this.lib.CodeAnchor, nameCode)) ??
        (await read(this.lib.Message, nameMessage))
      );
    });
  }

  /**
   * How a writer names the entity `id` in a message's text: `@handle` for a person or an agent
   * whose handle no other principal of the project has, `D12` for a decision; undefined for
   * anything else, a principal whose handle is shared included, which is named by the start of its
   * id, since posting refuses a handle two or more principals share.
   */
  async writtenAs(id: string): Promise<string | undefined> {
    return this.lend(async (ctx) => {
      let handle: string | undefined;
      try {
        const person = await this.lib.Principal.get(ctx, this.lib.EntityId.from_base64(id));
        try {
          handle = person.handle;
        } finally {
          person.free();
        }
      } catch {}
      if (handle !== undefined) {
        // The handle names this principal alone, or it is written by the start of the id.
        const named = await this.lib.principal_named(ctx, handle).catch(() => undefined);
        return named?.id === id ? `@${handle}` : undefined;
      }
      try {
        const decision = await this.lib.Decision.get(ctx, this.lib.EntityId.from_base64(id));
        try {
          return `D${decision.number}`;
        } finally {
          decision.free();
        }
      } catch {
        return undefined;
      }
    });
  }

  /**
   * The version of a file a message names, in the words its name shows after the file's path (the
   * commit's first seven characters, `index`, or `on disk`), from the version the stored mention
   * writes after the highlight's id (`worktree`, `index` or a commit's id), as the page and the CLI
   * word it (crates/fwd-host, `file_version`); undefined for anything else.
   */
  fileVersion(version: string): string | undefined {
    return this.lib.file_version(version) ?? undefined;
  }

  /** The title of task `id`, or undefined when the node cannot find it. */
  async titleOf(id: string): Promise<string | undefined> {
    try {
      return await this.lend(async (ctx) => {
        const view = await this.lib.Task.get(ctx, this.lib.EntityId.from_base64(id));
        try {
          return view.title;
        } finally {
          view.free();
        }
      });
    } catch {
      return undefined;
    }
  }

  /**
   * Of `messages`, the messages on one highlight, the id of the first message of its first thread,
   * or undefined while no one has started a thread on it: fwd-core's own rule
   * (`rules::first_thread`), the one resolving and checking follow, so that a thread's actions say
   * what they do by the same rule that does it.
   */
  firstThread(messages: MessageJson[]): string | undefined {
    return this.lib.first_thread(messages) ?? undefined;
  }

  /** Sets `status` on the notifications in `ids`, in one transaction. */
  setNotificationStatus(ids: string[], status: 'read' | 'dismissed'): Promise<void> {
    return this.lend((ctx) => this.lib.set_notification_status(ctx, ids, status));
  }

  /** `recipient`'s notifications that are not dismissed. */
  notifications(recipient: string, onChange: () => void): Live<NotificationJson> {
    const open = (ctx: FwdHost.Context) =>
      this.withId(recipient, (id) => this.lib.Notification.query(ctx, "recipient = ? AND status != 'dismissed'", id));
    return this.live(open, readNotification, onChange);
  }

  /** The anchors in the repository-relative `path`. */
  anchors(path: string, onChange: () => void): Live<AnchorLines> {
    return this.live((ctx) => this.lib.CodeAnchor.query(ctx, 'path = ?', path), readAnchor, onChange);
  }

  /**
   * Every anchor on a whole file in the project. A whole-file anchor has no lines, so its end is
   * 0, which the end of an anchor on lines never is; the predicate compares that number rather
   * than the `whole_file` flag because a boolean literal in a predicate has missed entities that
   * arrived over the wire (crates/fwd-host, `default_principal`).
   */
  wholeFileAnchors(onChange: () => void): Live<AnchorLines> {
    return this.live((ctx) => this.lib.CodeAnchor.query(ctx, 'end_line = 0'), readAnchor, onChange);
  }

  /**
   * The steps the highlight `anchorId` is: its `step_of` relations that stand, unchecked (accepted)
   * or checked (resolved), as fwd-core's `rules::holds` counts them.
   */
  steps(anchorId: string, onChange: () => void): Live<StepJson> {
    const standing = "from_id = ? AND kind = 'step_of' AND (status = 'accepted' OR status = 'resolved')";
    const open = (ctx: FwdHost.Context) => this.withId(anchorId, (id) => this.lib.Relation.query(ctx, standing, id));
    return this.live(open, readStep, onChange);
  }

  /** The messages on anchor `anchorId`: the roots of its threads and their replies. */
  messages(anchorId: string, onChange: () => void): Live<MessageJson> {
    const open = (ctx: FwdHost.Context) => this.withId(anchorId, (id) => this.lib.Message.query(ctx, 'anchor = ?', id));
    return this.live(open, readMessage, onChange);
  }

  /**
   * The relations that stand and point at the highlight `anchorId`: from each task whose code it
   * is (`anchored_at`), and from each message whose text names it (`mention`, which an edit that
   * takes the name out of the text retracts).
   */
  joinsTo(anchorId: string, onChange: () => void): Live<JoinJson> {
    const standing = "to_id = ? AND (kind = 'anchored_at' OR kind = 'mention') AND status = 'accepted'";
    const open = (ctx: FwdHost.Context) => this.withId(anchorId, (id) => this.lib.Relation.query(ctx, standing, id));
    return this.live(open, readJoin, onChange);
  }

  /**
   * What the thing `id` is called as it changes, from a live query on that one thing, so that what
   * names it follows it when it is renamed: a task's title, a decision's number and title, a
   * scope's title, where a highlight's lines are, a message's first words. `kind` is what the
   * thing is, as `describe` found it, a person aside, whose name the query on every principal
   * follows (`principals`).
   */
  referent(id: string, kind: Exclude<Referent['kind'], 'person'>, onChange: () => void): Live<Referent> {
    const byId =
      <V extends Handle>(model: { query(ctx: FwdHost.Context, selection: string, ...values: any[]): LiveQuery<V> }) =>
      (ctx: FwdHost.Context) =>
        this.withId(id, (entity) => model.query(ctx, 'id = ?', entity));
    switch (kind) {
      case 'task':
        return this.live(byId(this.lib.Task), nameTask, onChange);
      case 'decision':
        return this.live(byId(this.lib.Decision), nameDecision, onChange);
      case 'scope':
        return this.live(byId(this.lib.Scope), nameScope, onChange);
      case 'code':
        return this.live(byId(this.lib.CodeAnchor), nameCode, onChange);
      case 'message':
        return this.live(byId(this.lib.Message), nameMessage, onChange);
    }
  }

  /**
   * The name of each principal of the project, people and agents, as it changes, from one live
   * query on them all, as the page keeps them (crates/fwd-web, `store`), so that what names a
   * principal follows it when it is renamed (`fwd principal rename`).
   */
  principals(onChange: () => void): Live<PrincipalName> {
    return this.live((ctx) => this.lib.Principal.query(ctx, "kind = 'human' OR kind = 'agent'"), readPrincipal, onChange);
  }

  /**
   * Closes the node without freeing anything still in use, so that closing never throws. In
   * order: it refuses new calls and new live queries (each fails with "the fwd node is closed"),
   * so that no call starts while it waits; it stops hearing the node's socket, which it is about
   * to close; it frees every live query and subscription guard, so that nothing more reaches the
   * views; it waits a turn, in which the queries' unsubscribes go out over the socket, and then
   * until every call under way has settled, because a call borrows the context until its promise
   * settles and freeing a borrowed value throws ("attempted to take ownership of Rust value while
   * it was borrowed"); and only then does it free the context, close the node's socket and drop
   * the node. It resolves once every node `rejoin` replaced has gone as well. Running it again
   * does nothing.
   */
  async close(): Promise<void> {
    if (this.closed) return;
    this.closed = true;
    this.watch.free();
    for (const live of this.lives) live.free();
    this.lives.clear();
    await Promise.all([this.joined.leave(), ...[...this.leaving].map((old) => old.leave())]);
  }

  /**
   * The context of the node in place, for a call into the package that returns before anything
   * else runs (opening a live query); a call that awaits takes it through `lend` instead. Refused
   * once `close` has begun.
   */
  private context(): FwdHost.Context {
    if (this.closed) throw new Error('the fwd node is closed');
    return this.joined.ctx;
  }

  /**
   * Runs `call` on the context of the node in place; that node does not go until the call has
   * settled (`Joined.lend`). Refused once `close` has begun.
   */
  private lend<T>(call: (ctx: FwdHost.Context) => Promise<T>): Promise<T> {
    if (this.closed) return Promise.reject(new Error('the fwd node is closed'));
    return this.joined.lend(call);
  }

  /** Stops holding `live`, whose handles `Live.stop` frees. */
  forget(live: Live<unknown>): void {
    this.lives.delete(live);
  }

  get isClosed(): boolean {
    return this.closed;
  }

  /** A live query that `open` opens on the node, held until the view stops it or the node closes. */
  private live<V extends Handle, T>(
    open: (ctx: FwdHost.Context) => LiveQuery<V>,
    read: (lib: Lib, view: V) => T,
    onChange: () => void,
  ): Live<T> {
    const live = new Live<T>(this, this.context(), open, (view) => read(this.lib, view as V), onChange);
    this.lives.add(live);
    return live;
  }

  /**
   * Follows the socket of `host`, the node now in place: `up` says whether it is connected, and
   * `lost` hears each time it stops being connected. Ankurah calls back while it is changing the
   * state, so the callback only notes the state and leaves the rest to the next turn.
   */
  private follow(host: FwdHost.Host): Handle {
    const signal = host.connectionState();
    try {
      const now = signal.peek;
      this.up = now.value() === 'Connected';
      now.free();
      return signal.subscribe((state) => {
        const up = state.value() === 'Connected';
        state.free();
        if (host !== this.joined.host) return; // a node already replaced
        const was = this.up;
        this.up = up;
        if (was && !up) setImmediate(() => this.tellLost(host));
      });
    } finally {
      signal.free(); // the subscription keeps what it needs
    }
  }

  private tellLost(host: FwdHost.Host) {
    if (!this.closed && host === this.joined.host && !this.up) this.lost?.();
  }

  /**
   * Runs `use` with `id` as an EntityId and frees it after. A query binds an EntityId as a typed
   * literal (as fwd-core's `with_id` does), and borrows it rather than taking it.
   */
  private withId<T>(id: string, use: (id: FwdHost.EntityId) => T): T {
    const entity = this.lib.EntityId.from_base64(id);
    try {
      return use(entity);
    } finally {
      entity.free();
    }
  }
}

/**
 * One node joined to the daemon: the package's `Host`, the context every call and live query on the
 * node goes through, and the calls under way on that context. `HostNode` puts a new one in place
 * at each `rejoin`, and lets each one go with `leave`.
 */
class Joined {
  /** Each call under way on `ctx`, until it settles. */
  private underWay = new Set<Promise<unknown>>();
  /** Settles once the node has gone (`leave`). */
  private gone: Promise<void> | undefined;

  constructor(
    readonly host: FwdHost.Host,
    readonly ctx: FwdHost.Context,
  ) {}

  /**
   * Runs `call` on the context, keeping its promise until it settles, so that `leave` can wait
   * for it: wasm-bindgen lends the context to each function of the package that takes it until the
   * function's promise settles, and freeing a lent value throws. A call that reads several times
   * keeps the context until the last read has settled.
   */
  lend<T>(call: (ctx: FwdHost.Context) => Promise<T>): Promise<T> {
    const running = (async () => call(this.ctx))();
    this.underWay.add(running);
    const settled = () => void this.underWay.delete(running);
    running.then(settled, settled);
    return running;
  }

  /**
   * Lets the node go: waits a turn, in which live queries freed just before send their
   * unsubscribes from tasks of their own (each fails with "Node has been dropped" once the node is
   * gone), then until every call under way has settled; then frees the context, closes the node's
   * socket and drops the node. Asked again, it returns the same promise.
   */
  leave(): Promise<void> {
    this.gone ??= (async () => {
      await new Promise((resolve) => setImmediate(resolve));
      while (this.underWay.size) await Promise.allSettled(this.underWay);
      this.ctx.free();
      this.host.close();
      this.host.free();
    })();
    return this.gone;
  }
}

/**
 * A live query a view holds, read as plain objects. The view keeps the same object while the node
 * is replaced under it (`HostNode.rejoin`): the query opens again on the new node, and `items`
 * reads the old query's last results until the new query has answered.
 */
export class Live<T> {
  /** The query `items` reads, and its subscription. */
  private query: LiveQuery<Handle>;
  private guard: Handle;
  /** The same query on a new node, with its subscription, until its first answer. */
  private next: { query: LiveQuery<Handle>; guard: Handle } | undefined;
  private stopped = false;
  /** A call to `onChange` is waiting for the next turn. */
  private pending = false;

  constructor(
    private node: HostNode,
    ctx: FwdHost.Context,
    private open: (ctx: FwdHost.Context) => LiveQuery<Handle>,
    private read: (view: Handle) => T,
    private onChange: () => void,
  ) {
    this.query = open(ctx);
    this.guard = this.subscribe(this.query, true);
  }

  /** True once the query's first answer has arrived. */
  get loaded(): boolean {
    return !this.node.isClosed && !this.stopped && this.query.loaded;
  }

  /** The query's current results; none once the node is closed or the query stopped. */
  items(): T[] {
    if (this.node.isClosed || this.stopped) return [];
    const views = this.query.peek;
    try {
      return views.map(this.read);
    } finally {
      for (const view of views) view.free();
    }
  }

  /** Ends the subscription and frees the query. */
  stop(): void {
    if (this.stopped) return;
    this.node.forget(this);
    this.free();
  }

  /**
   * How many live queries and subscription guards this holds: two, and two more while a new
   * node's query has not answered yet.
   */
  get handles(): number {
    return this.next ? 4 : 2;
  }

  /** Opens the query again on `ctx`, a new node's context; `items` moves to it once it has answered. */
  reopen(ctx: FwdHost.Context): void {
    this.dropNext();
    const query = this.open(ctx);
    // Not called back at once with the empty results a new query starts with: the view goes on
    // reading the old query until this one has its first answer.
    this.next = { query, guard: this.subscribe(query, false) };
  }

  /** Frees the subscriptions and the queries, a new node's included. */
  free(): void {
    this.stopped = true;
    this.dropNext();
    this.guard.free();
    this.query.free();
  }

  private dropNext() {
    if (!this.next) return;
    this.next.guard.free();
    this.next.query.free();
    this.next = undefined;
  }

  /**
   * Calls `onChange` after every change set `query` delivers (and at once, with `immediate`, for
   * the results it has), first moving `items` to a new node's query once that has answered. The
   * call waits for the next turn: the subscription callback runs inside the node's delivery of
   * the change, where reading the query would re-enter the node mid-delivery, and waiting also
   * turns a burst of change sets into one call.
   */
  private subscribe(query: LiveQuery<Handle>, immediate: boolean): Handle {
    return query.subscribe((changes) => {
      setImmediate(() => changes.free());
      if (this.pending) return;
      this.pending = true;
      setImmediate(() => {
        this.pending = false;
        if (this.stopped || this.node.isClosed) return;
        if (this.next?.query.loaded) {
          this.guard.free();
          this.query.free();
          ({ query: this.query, guard: this.guard } = this.next);
          this.next = undefined;
        }
        this.onChange();
      });
    }, immediate);
  }
}

// ---------------------------------------------------------------------------------------------
// Views as plain objects. Every id, reference and view read here is a wasm-bindgen handle of its
// own, freed as soon as its value is out.

function idText(id: FwdHost.EntityId): string {
  try {
    return id.to_base64();
  } finally {
    id.free();
  }
}

function refText(ref: { readonly id: FwdHost.EntityId; free(): void } | undefined): string | undefined {
  if (!ref) return undefined;
  try {
    return idText(ref.id);
  } finally {
    ref.free();
  }
}

/**
 * `TaskAssigned` as `task_assigned`: the models' enums print in snake case (strum), and each enum
 * object in the package maps a value back to its variant's name.
 */
function snake(name: string | undefined): string {
  return (name ?? '').replace(/[A-Z]/g, (letter, at) => (at ? '_' : '') + letter.toLowerCase());
}

function readNotification(lib: Lib, view: FwdHost.NotificationView): NotificationJson {
  return {
    id: idText(view.id),
    type: snake(lib.NotificationType[view.notification_type]),
    status: snake(lib.NotificationStatus[view.status]),
    priority: snake(lib.Priority[view.priority]),
    created: Number(view.created),
    target: view.target,
    originator: view.originator,
  };
}

function readAnchor(lib: Lib, view: FwdHost.CodeAnchorView): AnchorLines {
  const start_line = Number(view.start_line);
  const end_line = Number(view.end_line);
  // An anchor made before whole-file anchors existed has no flag, and names lines.
  const whole = view.whole_file === true || end_line <= start_line;
  return { id: idText(view.id), path: view.path, start_line, end_line, whole, onDisk: view.side === lib.DiffSide.Worktree };
}

function readStep(lib: Lib, view: FwdHost.RelationView): StepJson {
  return { id: idText(view.id), task: idText(view.to_id), checked: snake(lib.RelationStatus[view.status]) === 'resolved' };
}

function readJoin(lib: Lib, view: FwdHost.RelationView): JoinJson {
  return { id: idText(view.id), kind: snake(lib.RelationKind[view.kind]) as JoinJson['kind'], from: idText(view.from_id) };
}

// What a reference in a message's text names, and what it is called (`Referent`), from the view
// of each kind of thing: read once (`HostNode.describe`) and as it changes (`HostNode.referent`)
// alike.

function namePerson(_lib: Lib, view: FwdHost.PrincipalView): Referent {
  return { kind: 'person', handle: view.handle };
}

function nameTask(_lib: Lib, view: FwdHost.TaskView): Referent {
  return { kind: 'task', title: view.title };
}

function nameDecision(_lib: Lib, view: FwdHost.DecisionView): Referent {
  return { kind: 'decision', number: Number(view.number), title: view.title };
}

function nameScope(_lib: Lib, view: FwdHost.ScopeView): Referent {
  return { kind: 'scope', title: view.title };
}

function nameCode(lib: Lib, view: FwdHost.CodeAnchorView): Referent {
  return { kind: 'code', place: lib.anchor_place(view) };
}

function nameMessage(_lib: Lib, view: FwdHost.MessageView): Referent {
  return { kind: 'message', words: view.content.split(/\s+/).filter(Boolean).slice(0, 8).join(' ') };
}

function readPrincipal(_lib: Lib, view: FwdHost.PrincipalView): PrincipalName {
  return { id: idText(view.id), handle: view.handle };
}

function readMessage(lib: Lib, view: FwdHost.MessageView): MessageJson {
  return {
    id: idText(view.id),
    author: refText(view.author) ?? '',
    content: view.content,
    status: snake(lib.MessageStatus[view.status]),
    created: Number(view.created),
    thread: refText(view.thread),
  };
}
