// The badge a file with a highlight on the whole file gets on its name, in the explorer and on its
// tab. Such a highlight is about the file rather than lines of it, so nothing is drawn in its text
// (docs/DECISIONS.md, "A whole-file reference is an anchor with no line range"): its thread is on
// the file (src/comments.ts), and the badge says so wherever the file's name shows, open or not.
// A live query on every whole-file anchor in the project, and one on each one's messages, keep
// the badges current.

import * as vscode from 'vscode';
import { serially } from './fwd';
import { AnchorLines, HostNode, Live, MessageJson } from './node';

export class WholeFileBadges implements vscode.FileDecorationProvider {
  private changes = new vscode.EventEmitter<vscode.Uri[]>();
  readonly onDidChangeFileDecorations = this.changes.event;
  private anchors: Live<AnchorLines> | undefined;
  private messages = new Map<string, Live<MessageJson>>(); // by anchor id
  /** The threads not resolved on each file's whole-file anchors, by the file's absolute path. */
  private open = new Map<string, number>();
  private node: HostNode | undefined;
  private render = serially(async () => this.load());

  /** `absPath` turns a repository-relative path into the file's absolute path. */
  constructor(private absPath: (rel: string) => string) {}

  start(node: HostNode) {
    this.node = node;
    this.anchors = node.wholeFileAnchors(this.render);
  }

  /** Stops following the project and takes every badge away. */
  close() {
    this.node = undefined;
    this.anchors?.stop();
    this.anchors = undefined;
    for (const live of this.messages.values()) live.stop();
    this.messages.clear();
    this.update(new Map());
  }

  provideFileDecoration(uri: vscode.Uri): vscode.FileDecoration | undefined {
    const n = uri.scheme === 'file' ? this.open.get(uri.fsPath) : undefined;
    if (!n) return undefined;
    return new vscode.FileDecoration('◆', `${n} comment thread${n === 1 ? '' : 's'} on the whole file, not resolved yet (fwd)`);
  }

  private load() {
    const node = this.node;
    if (!node || !this.anchors) return;
    const anchors = this.anchors.items();
    const current = new Set(anchors.map((a) => a.id));
    for (const [id, live] of this.messages) {
      if (current.has(id)) continue;
      live.stop();
      this.messages.delete(id);
    }
    const open = new Map<string, number>();
    for (const anchor of anchors) {
      let live = this.messages.get(anchor.id);
      if (!live) this.messages.set(anchor.id, (live = node.messages(anchor.id, this.render)));
      const threads = live.items().filter((m) => !m.thread && m.status === 'active').length;
      const file = this.absPath(anchor.path);
      if (threads) open.set(file, (open.get(file) ?? 0) + threads);
    }
    this.update(open);
  }

  /** Keeps `open` and tells VS Code which files' badges changed. */
  private update(open: Map<string, number>) {
    const changed = [...new Set([...this.open.keys(), ...open.keys()])].filter((file) => this.open.get(file) !== open.get(file));
    this.open = open;
    if (changed.length) this.changes.fire(changed.map((file) => vscode.Uri.file(file)));
  }
}
