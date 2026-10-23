// The name of each principal of the project, a person or an agent, as it is now. So that everything
// in the editor that names a principal shows the name it has now, and follows a renaming (`fwd
// principal rename`) with no reload, each reads the name here: a comment's author
// (src/comments.ts), and a mention in a message's text, in a comment thread and in the inbox tree
// (src/references.ts). One live query on the project's principals keeps the names, as the page's
// store keeps them (crates/fwd-web, `store`), and says which principals it named anew, so that
// each view draws again what shows them.

import * as vscode from 'vscode';
import { HostNode, Live, PrincipalName } from './node';

export class Principals {
  /**
   * Each principal's name (its handle), by id: read when a view first shows the principal (`load`),
   * and kept from then on by the live query, whose answer a read that ends after it does not undo.
   */
  private handles = new Map<string, string>();
  /** The live query on the project's principals that keeps `handles`, while the window has a node. */
  private live: Live<PrincipalName> | undefined;
  private node: HostNode | undefined;
  private renaming = new vscode.EventEmitter<Set<string>>();
  /**
   * Fires with the ids of the principals the live query named anew: each one renamed, and each one
   * it names first, before any view read its name.
   */
  readonly onRenamed = this.renaming.event;

  /** Follows the project's principals through `node`, from now on. */
  start(node: HostNode) {
    this.node = node;
    this.live = node.principals(() => this.answered());
  }

  /** Stops following them. The names known stay, until a new node's query names a principal anew. */
  close() {
    this.node = undefined;
    this.live?.stop();
    this.live = undefined;
  }

  /** The name of principal `id` as it is now, or undefined while neither a read nor the query has named it. */
  handle(id: string): string | undefined {
    return this.handles.get(id);
  }

  /**
   * Reads the names of the principals in `ids` not known yet, so that a view drawn before the live
   * query first answers names them at once.
   */
  async load(ids: Iterable<string>) {
    for (const id of new Set(ids)) {
      if (this.handles.has(id) || !this.node) continue;
      const handle = await this.node.handleOf(id);
      if (handle && !this.handles.has(id)) this.handles.set(id, handle); // unless the live query answered first
    }
  }

  /**
   * The live query answered: each principal whose name differs from the one known is named anew,
   * and the views hear which.
   */
  private answered() {
    const renamed = new Set<string>();
    for (const { id, handle } of this.live?.items() ?? []) {
      if (this.handles.get(id) === handle) continue;
      this.handles.set(id, handle);
      renamed.add(id);
    }
    if (renamed.size) this.renaming.fire(renamed);
  }
}
