// The window's current task: the task the page in the panel shows, kept so that a comment or a
// question made in the editor goes to that task, with its new highlight as the context, a comment
// reaching the task's assignee and the triage agent of each of the task's scopes, and a question,
// asked of no one in particular, the triage agent of the one scope it asks alone
// (docs/DECISIONS.md, D111, "A message reaches only whom it is addressed to"; D122, triage per
// scope; D92, "A new highlight made in the editor with a task current is a comment or a question on
// that task, with the highlight as its context; adding a step is a separate, explicit action"),
// and so that a highlight can be added as a step of it with Add as a Step. The page's bridge
// reports each route the page goes to; a route that shows a task makes it current, and one that
// shows none leaves the current task as it was, so reading the inbox on the page does not end a
// review. A status-bar item names the current task, and clicking it clears it; closing the panel
// clears it too. VS Code brings the panel back after the window reloads (its serializer, in
// src/extension.ts), and the current task comes back with it: the task and the route the page last
// reported are kept in the workspace's state as they change, and put back when the panel is
// (`restore`), so that a task current from an earlier route, while the page shows the inbox, is
// current again, and one the reader cleared stays cleared.

import * as vscode from 'vscode';
import { short } from './fwd';
import { HostNode } from './node';

/** The key of the workspace's state that keeps the current task and the page's route. */
const KEPT = 'fwd.currentTask';

/** What the workspace's state keeps: the current task, and the route the page last reported. */
interface Kept {
  id?: string;
  route?: string;
}

export class CurrentTask {
  id: string | undefined;
  title: string | undefined;
  private item = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 100);
  /** The route the page last reported, which it reports again each time it boots. */
  private route: string | undefined;
  private node: HostNode | undefined;
  private kept: vscode.Memento;

  constructor(context: vscode.ExtensionContext) {
    this.kept = context.workspaceState;
    this.item.command = 'fwd.clearCurrentTask';
    context.subscriptions.push(this.item);
  }

  /**
   * Reads task titles through `node` from now on (none while the window has no node), first the
   * title of a task made current before the node joined (`restore`).
   */
  useNode(node: HostNode | undefined) {
    this.node = node;
    if (node && this.id) void this.name(this.id);
  }

  /**
   * Follows the page to `route`. A route that shows a task makes that task current. The same
   * route again changes nothing: the page boots again each time the panel is shown after being
   * hidden, and a task the reader cleared stays cleared.
   */
  async follow(route: string) {
    if (route === this.route) return;
    this.route = route;
    this.keep();
    const id = taskOf(route);
    if (id && id !== this.id) await this.set(id);
  }

  /** Makes task `id` current, named by its title once the node has read it. */
  async set(id: string) {
    this.id = id;
    this.title = short(id);
    this.keep();
    this.show();
    await this.name(id);
  }

  /**
   * No task is current any more. Closing the panel also forgets the page's route, so a page
   * opened again on the same task makes it current again.
   */
  clear(forgetRoute = false) {
    if (forgetRoute) this.route = undefined;
    this.id = undefined;
    this.title = undefined;
    this.keep();
    this.show();
  }

  /**
   * Puts back the task and the route kept from before the window reloaded, as VS Code brings the
   * page's panel back: the task, which the page's route alone may not show, and the route, so that
   * the page reporting it again as it boots changes nothing.
   */
  restore() {
    const kept = this.kept.get<Kept>(KEPT) ?? {};
    this.route = kept.route;
    this.id = kept.id;
    this.title = kept.id && short(kept.id);
    this.show();
    if (kept.id) void this.name(kept.id);
  }

  /**
   * Forgets the current task and the route as the extension stops (`deactivate`), and leaves them
   * kept, for the page's panel VS Code brings back after a reload (`restore`).
   */
  forget() {
    this.route = undefined;
    this.id = undefined;
    this.title = undefined;
    this.show();
  }

  private keep() {
    const kept: Kept = { id: this.id, route: this.route };
    void this.kept.update(KEPT, kept);
  }

  /** Names task `id` by its title once the node has read it, unless it is no longer current. */
  private async name(id: string) {
    const title = await this.node?.titleOf(id);
    if (this.id !== id || !title) return; // cleared or replaced meanwhile, or no title to read
    this.title = title;
    this.show();
  }

  /** What the status-bar item shows, while it shows. */
  get status(): string | undefined {
    return this.id ? this.item.text : undefined;
  }

  private show() {
    void vscode.commands.executeCommand('setContext', 'fwd.hasCurrentTask', this.id !== undefined);
    if (this.id && this.title) {
      const title = this.title.length > 40 ? `${this.title.slice(0, 39)}…` : this.title;
      this.item.text = `$(tasklist) ${title}`;
      this.item.tooltip = `Comments and questions made in the editor go to "${this.title}". Click to stop.`;
      this.item.show();
    } else {
      this.item.hide();
    }
  }
}

/**
 * The task a page route shows: the Tasks page with a task in its panel (`#/tasks/<id>`, also when
 * filtered to a scope, `?scope=<id>`), or the Graph page with one (`#/graph/<id>`).
 */
export function taskOf(route: string): string | undefined {
  return /^#\/(?:tasks|graph)\/([A-Za-z0-9_-]+)/.exec(route)?.[1];
}
