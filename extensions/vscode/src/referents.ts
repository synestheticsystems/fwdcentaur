// What each thing a view in the editor names is called now, a person or an agent aside, whose
// name src/principals.ts keeps: a task and a scope by its title, a decision by its number and its
// title, a highlight by where its lines are, a message by its first words. So that everything in
// the editor that names one shows the name it has now, and follows a renaming (`fwd task amend
// --title`, `fwd scope rename`, a message edited) with no reload, each reads the name here: a
// thread's label and its line of what its highlight is joined to (src/comments.ts), and a
// reference in a message's text, in a comment thread and in the inbox tree (src/references.ts).
// A thing is read when a view first shows it, then kept by a live query on that one thing for as
// long as a view shows it, and the store says which things it named anew, so that each view draws
// again what shows them.

import * as vscode from 'vscode';
import { HostNode, Live, Referent } from './node';

/** What the store knows of one thing a view shows. */
interface Known {
  /**
   * What the thing is called: as the first read found it, then as the live query on it has it;
   * null when the node found nothing by its id, and undefined until the first read has answered.
   */
  named?: Referent | null;
  /** The first read, which settles once `named` is set. */
  read: Promise<void>;
  /**
   * The live query on the thing, which keeps `named`; none for a person or an agent, whose name
   * src/principals.ts keeps, and none for an id the node found nothing by.
   */
  live?: Live<Referent>;
}

export class Referents {
  /** What each thing a view shows is called, by id. */
  private known = new Map<string, Known>();
  /** The ids of the things each view shows, by view: the threads of an open document, the inbox. */
  private shown = new Map<object, Set<string>>();
  private node: HostNode | undefined;
  private renaming = new vscode.EventEmitter<Set<string>>();
  /** Fires with the id of a thing the live query on it named anew: one renamed since it was read. */
  readonly onRenamed = this.renaming.event;

  /** Reads through `node` from now on. */
  start(node: HostNode) {
    this.node = node;
  }

  /**
   * Stops following the things: each live query ends, and what was known goes, to be read again
   * through the next node for each view that shows it then.
   */
  close() {
    this.node = undefined;
    for (const known of this.known.values()) known.live?.stop();
    this.known.clear();
  }

  /** What `id` names as it is now, or undefined while no read has named it. */
  named(id: string): Referent | undefined {
    return this.known.get(id)?.named ?? undefined;
  }

  /**
   * `view` shows the things in `ids` from now on, and none it showed before besides: each one not
   * known yet is read, and followed from then on, and each one no view shows any more stops being
   * followed. Settles once each has been read.
   */
  show(view: object, ids: Iterable<string>): Promise<void> {
    const now = new Set(ids);
    this.shown.set(view, now);
    this.forget();
    return this.load(now);
  }

  /** `view` shows nothing any more: what no other view shows stops being followed. */
  hide(view: object) {
    if (this.shown.delete(view)) this.forget();
  }

  /**
   * Reads what each id in `ids` names, when it is not known yet, and follows it from then on while
   * a view shows it: for things a view is about to show, as a comment's new text is shown before
   * its document's threads are built again.
   */
  async load(ids: Iterable<string>) {
    for (const id of new Set(ids)) await this.read(id);
  }

  /** Reads what `id` names, once, and follows it from then on through a live query on it. */
  private read(id: string): Promise<void> {
    const had = this.known.get(id);
    const node = this.node;
    if (had || !node) return had?.read ?? Promise.resolve();
    const known: Known = { read: Promise.resolve() }; // until the read below starts
    this.known.set(id, known);
    known.read = (async () => {
      const named = (await node.describe(id).catch(() => undefined)) ?? null;
      if (this.known.get(id) !== known || node.isClosed) return; // no view shows it any more, or the node went
      known.named = named;
      if (named && named.kind !== 'person') known.live = node.referent(id, named.kind, () => this.answered(id));
    })();
    return known.read;
  }

  /** The live query on thing `id` answered: when it names the thing anew, the views hear so. */
  private answered(id: string) {
    const known = this.known.get(id);
    const now = known?.live?.items()[0];
    // A name is a small plain object (`Referent`), the same whichever read made it.
    if (!known || !now || JSON.stringify(now) === JSON.stringify(known.named)) return;
    known.named = now;
    this.renaming.fire(new Set([id]));
  }

  /** Stops following, and forgets, each thing no view shows any more. */
  private forget() {
    const shown = new Set([...this.shown.values()].flatMap((ids) => [...ids]));
    for (const [id, known] of this.known) {
      if (shown.has(id)) continue;
      known.live?.stop();
      this.known.delete(id);
    }
  }
}
