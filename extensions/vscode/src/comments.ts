// Comment threads and highlights on code. A highlight is an anchor with at least one thread whose
// first message is not resolved (docs/design/reviews-and-highlights.md). Every open document that
// shows a version of a file in the workspace, the file itself or a side of a git diff
// (src/sides.ts), gets the threads of the anchors in that file, each placed where its anchor
// sits in that version, and every highlight drawn as a background over its lines with a gutter
// mark and an overview-ruler mark. A highlight on the whole file is drawn nowhere in the text: its
// thread is on the file (and the explorer badges the file, src/badges.ts). A highlight's thread
// opens and closes from VS Code's comment icon in the gutter, and the cursor entering or leaving
// its lines leaves it as it was (docs/DECISIONS.md, D98, "In VS Code a highlight's thread opens
// from the gutter's comment icon, not when the cursor enters its lines"); a hover on its lines
// shows the thread's first lines. A comment of one's own offers VS Code's Edit, which saves the
// new text with `fwd message edit`, keeping the message's thread. The first thread on a highlight
// that is a step moves with the step (docs/DECISIONS.md, D99, "Checking a step resolves its
// highlight's first thread, and resolving that thread checks the step; each control says it does
// both"), so its title bar's actions say they check or uncheck the step too, and its label says
// whether the step is checked.
//
// VS Code draws a thread's box itself, its margins, its title bar and its reply box, and gives an
// extension no way to restyle it; fwd decides only what goes in the box, and keeps that short
// (docs/design/vscode-extension.md, The thread box): every thread carries its reply box from the
// moment it is made, VS Code's one line saying "Reply..." that a click opens to write in, the
// title bar rather than the first comment says that the thread is resolved, and a reference in a
// comment is a link that names it in a few words. So that a person reading a highlight's thread
// sees which tasks its lines belong to and gets to them in one click, the first comment of a
// highlight's first thread ends with a line of fwd's saying what the highlight is joined to beyond
// its threads, each a link that opens it on the page (`joinedLine`), and every thread's title bar
// opens the highlight's own page, which lists all of it.

import * as vscode from 'vscode';
import * as path from 'path';
import { CurrentTask } from './current';
import { actingAs, fwd, Json, serially, short, workspaceRoot } from './fwd';
import { AnchorLines, HostNode, JoinJson, Live, MessageJson, StepJson } from './node';
import { Principals } from './principals';
import { OPEN_REFERENCE, openLink, opensMention, References, referencesIn, taskLink } from './references';
import { Referents } from './referents';
import { filePathOf, revisionArgs, Side, sideOf } from './sides';

/**
 * Whom Ask asks: no one in particular, which the triage agent of the scope the question concerns
 * answers, as the CLI finds it (the current task's scope, of a task worked in several the one this
 * checkout works in when it is one of them, else the first; else the scope this checkout works in),
 * else the triage agent that serves the whole project; while neither exists the question waits on
 * the Questions page, listed under its scope (docs/DECISIONS.md, D122). The web page's Ask box
 * offers the scope its header chooses instead (crates/fwd-web/src/ask.rs), since the page has no
 * checkout.
 */
const ASKED_OF_NO_ONE = 'no one in particular, which the triage agent of its scope answers';

/** Lines of a document: 0-based, the end exclusive. */
interface Span {
  start: number;
  end: number;
}

/** A thread shown on a document, with what the hover needs to know of it. */
interface Shown {
  thread: vscode.CommentThread;
  root: MessageJson;
  replies: number;
  /**
   * The anchor whose lines are the highlight the thread is on, read in `FileThreads.lines` when
   * they are needed; none when the thread is resolved or on the whole file.
   */
  highlight?: string;
}

export class Comments {
  readonly controller = vscode.comments.createCommentController('fwd', 'fwdcentaur');
  /** The root message of each thread shown, which a reply answers and Resolve resolves. */
  readonly roots = new WeakMap<vscode.CommentThread, string>();
  /** The anchor each thread shown is on: the highlight Add as a Step adds. */
  readonly anchors = new WeakMap<vscode.CommentThread, string>();
  /** How a highlight is drawn: the lines' background, a gutter mark and a mark in the overview ruler. */
  readonly highlight: vscode.TextEditorDecorationType;
  /** The highlights last drawn on each editor. */
  readonly painted = new WeakMap<vscode.TextEditor, vscode.Range[]>();
  /** The references in comments, read as names. */
  readonly references: References;
  private files = new Map<string, FileThreads>(); // by document URI
  /**
   * The line of what its highlight is joined to that closes the first comment of each thread that
   * has one (`joinedLine`), by thread: the first thread of a highlight joined to anything.
   */
  private joined = new WeakMap<vscode.CommentThread, string>();
  /** The message each comment shown stands for, and the thread showing it: what Edit, Save and Cancel act on. */
  private shownAs = new WeakMap<vscode.Comment, { message: MessageJson; thread: vscode.CommentThread }>();
  /** The messages being edited in place, by id: the thread whose comment is in its editor, and the text the editor holds. */
  private editing = new Map<string, { thread: vscode.CommentThread; text: string }>();
  /** The principal this window acts as, whose comments offer Edit; undefined until the node says. */
  private me: string | undefined;
  /** How many times the window has asked whom it acts as, so that only the latest answer is kept. */
  private lookups = 0;
  private node: HostNode | undefined;
  // Where the workspace folder sits in its repository ("" at the root), for repository-relative paths.
  private prefix = '';
  /** How many highlights the active editor's document has, and the text it shows while it shows. */
  private count = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Right, 100);
  private countText: string | undefined;

  constructor(
    context: vscode.ExtensionContext,
    private current: CurrentTask,
    readonly principals: Principals,
    readonly referents: Referents,
  ) {
    this.references = new References(() => this.node, principals, referents);
    this.controller.commentingRangeProvider = {
      // New comments start on lines of every version of a file a document shows: the file on
      // disk, and a revision on a side of a diff (the index, HEAD, a commit), whose lines the CLI
      // anchors as that revision has them (`--against`). A comment on the whole file starts on the
      // file on disk only, since the whole file's threads show there and on no revision.
      provideCommentingRanges: (document) => {
        const side = sideOf(document.uri);
        if (!side || this.relPath(document.uri) === undefined) return [];
        return { ranges: [new vscode.Range(0, 0, Math.max(0, document.lineCount - 1), 0)], enableFileComments: side.kind === 'worktree' };
      },
    };
    this.highlight = vscode.window.createTextEditorDecorationType({
      isWholeLine: true,
      backgroundColor: new vscode.ThemeColor('fwd.highlightBackground'),
      overviewRulerColor: new vscode.ThemeColor('fwd.highlightRuler'),
      overviewRulerLane: vscode.OverviewRulerLane.Left,
      gutterIconPath: context.asAbsolutePath('media/highlight.svg'),
      gutterIconSize: 'contain',
    });
    this.count.command = 'editor.action.nextCommentThreadAction';
    context.subscriptions.push(
      this.controller,
      this.highlight,
      this.count,
      vscode.workspace.onDidOpenTextDocument((d) => this.watch(d)),
      vscode.workspace.onDidCloseTextDocument((d) => this.unwatch(d)),
      vscode.workspace.onDidSaveTextDocument((d) => this.files.get(d.uri.toString())?.saved()),
      vscode.workspace.onDidChangeTextDocument((e) => this.files.get(e.document.uri.toString())?.edited(e)),
      vscode.window.onDidChangeVisibleTextEditors((editors) => {
        for (const editor of editors) this.files.get(editor.document.uri.toString())?.paint(editor);
        this.showCount();
      }),
      vscode.window.onDidChangeActiveTextEditor(() => this.showCount()),
      vscode.languages.registerHoverProvider([{ scheme: 'file' }, { scheme: 'git' }], {
        provideHover: (document, position) => this.hover(document, position),
      }),
      // The box VS Code opens for a comment or a reply holds a document of the `comment` scheme.
      vscode.languages.registerCompletionItemProvider(
        { scheme: 'comment' },
        { provideCompletionItems: (document, position) => this.mentions(document.lineAt(position.line).text.slice(0, position.character), position) },
        '@',
      ),
      principals.onRenamed((ids) => this.renamed(ids)),
      referents.onRenamed((ids) => this.renamed(ids)),
    );
  }

  /**
   * The mention search in a comment box: while the cursor ends an `@name` being typed (`before` is
   * its line up to the cursor), the principals that belong to the project whose name starts with
   * what is typed, each with the scopes its work is in beside a name two or more principals of the
   * project share (docs/DECISIONS.md, D107), shown as the item's description, since a completion
   * list draws text. The agents that have written nothing for twelve hours come after all the
   * others, each saying `dormant` after what it is, since a completion list has no headings (D123).
   * Choosing one writes `@name`, or `@` and the start of the id for a shared name, which posting
   * would refuse, in place of the `@` and what follows it up to the cursor, at `cursor`; posting
   * stores either as the chosen principal's id.
   */
  async mentions(before: string, cursor?: vscode.Position): Promise<vscode.CompletionItem[]> {
    const node = this.node;
    const typing = /@([A-Za-z0-9_.-]*)$/.exec(before);
    if (!node || !typing || !opensMention(before[typing.index - 1])) return [];
    const offers = await node.mentionOffers(typing[1]).catch((e) => {
      console.warn('fwd: the mention search could not ask the node:', e);
      return [];
    });
    return offers.map((offer, i) => {
      const scopes = offer.scopes && (offer.scopes.length ? offer.scopes.join(' · ') : 'no scope');
      const what = offer.dormant ? `${offer.what}, dormant` : offer.what;
      const label = { label: `@${offer.handle}`, detail: ` ${what}`, description: scopes ?? undefined };
      const item = new vscode.CompletionItem(label, vscode.CompletionItemKind.User);
      item.insertText = `${offer.written} `;
      item.filterText = `@${offer.handle}`;
      item.sortText = String(i).padStart(3, '0');
      if (cursor) item.range = new vscode.Range(cursor.translate(0, -typing[0].length), cursor);
      return item;
    });
  }

  /** Shows threads and highlights on every open document of a file in the workspace folder, through `node`, from now on. */
  start(node: HostNode, prefix: string) {
    this.node = node;
    this.prefix = prefix;
    for (const doc of vscode.workspace.textDocuments) this.watch(doc);
    this.identify().catch((e) => console.warn('fwd: finding whom this window acts as:', e));
  }

  /**
   * Learns whom this window acts as (`fwd.as`, else the project's person, found by its id once and
   * kept, as the inbox finds it: `actingAs`), since a message's author edits it and so their
   * comments offer Edit, and draws every thread again with it: when the node joins, and when
   * `fwd.as` changes. An agent that succeeded a comment's author edits it too, from the CLI or the
   * page; the window does not offer it Edit, since it does not read who succeeded whom (task
   * 0v1yNbNb).
   */
  async identify() {
    const node = this.node;
    if (!node) return;
    // `fwd.as` may change again while this answer is awaited; an earlier answer arriving after a
    // later one would otherwise offer Edit on the comments of whom the window no longer acts as.
    const lookup = ++this.lookups;
    const me = await actingAs(node);
    if (node !== this.node || lookup !== this.lookups) return;
    this.me = me;
    for (const file of this.files.values()) file.redraw();
  }

  /** Stops watching every document and removes its threads and highlights. */
  close() {
    this.node = undefined;
    for (const file of this.files.values()) file.dispose();
    this.files.clear();
    this.editing.clear();
    this.showCount();
  }

  threadsOn(uri: vscode.Uri): vscode.CommentThread[] {
    return [...(this.files.get(uri.toString())?.threads.values() ?? [])];
  }

  /**
   * Each document the threads are shown on, by URI, with its threads, one with none yet too: the
   * documents VS Code's Comments panel lists the threads under.
   */
  threadsByDocument(): Map<string, vscode.CommentThread[]> {
    return new Map([...this.files].map(([uri, file]) => [uri, [...file.threads.values()]]));
  }

  /** The highlights drawn on the visible editors of `uri`, or undefined when none shows it. */
  paintedOn(uri: vscode.Uri): vscode.Range[] | undefined {
    const editor = vscode.window.visibleTextEditors.find((e) => e.document.uri.toString() === uri.toString());
    return editor && (this.painted.get(editor) ?? []);
  }

  /** What the status bar says about the highlights in the active editor's document, when it says anything. */
  countShown(): string | undefined {
    return this.countText;
  }

  /** The repository-relative path of the file a document inside the workspace folder shows (itself, or a version of it). */
  relPath(uri: vscode.Uri): string | undefined {
    const root = workspaceRoot();
    const file = filePathOf(uri);
    if (!root || !file) return undefined;
    const rel = path.relative(root, file);
    return rel.startsWith('..') || path.isAbsolute(rel) ? undefined : this.prefix + rel.split(path.sep).join('/');
  }

  /** The file at a repository-relative path. */
  absPath(rel: string): string {
    const up = this.prefix.split('/').filter(Boolean).map(() => '..');
    return path.join(workspaceRoot() ?? '', ...up, rel);
  }

  author(id: string): string {
    return this.principals.handle(id) ?? short(id);
  }

  /**
   * The things in `ids` were named anew: principals renamed or named for the first time
   * (`Principals`), or anything else renamed (`Referents`). The threads of each document that
   * show one, as a comment's author, in a label or a line of what a highlight is joined to, or
   * mentioned in a comment's text, are drawn again.
   */
  private renamed(ids: Set<string>) {
    for (const file of this.files.values()) if ([...file.authors, ...file.tasks, ...file.referenced].some((id) => ids.has(id))) file.redraw();
  }

  /** The title of task `id` as it is now (`Referents`), or the start of its id while none has been read. */
  title(id: string): string {
    const named = this.referents.named(id);
    return named?.kind === 'task' ? named.title : short(id);
  }

  /**
   * A new thread on `uri` (on `range`, or on the whole file when there is none) whose comments are
   * `messages`, a first message and its replies: closed, with its reply box at its foot. It is
   * made with its comments rather than given them after, since VS Code builds a thread's box as
   * the thread arrives, and a box built empty says "Start discussion" in its title bar and holds
   * its reply box open, five lines high; built with comments, the reply box is VS Code's one line
   * saying "Reply...", which a click opens to write in.
   *
   * The thread keeps `canReply` as VS Code makes it, true, and fwd never changes it, since a
   * change is what drew the reply box twice (docs/design/vscode-extension.md, The thread box):
   * VS Code builds a thread's reply box when `canReply` turns true and, in the same update, tells
   * the new box that the thread changed, which reaches it while it is still being built; the box
   * makes its one-line button both then and once built, and draws the two side by side.
   */
  makeThread(uri: vscode.Uri, range: vscode.Range | undefined, messages: MessageJson[], joined?: string): vscode.CommentThread {
    const drawn = messages.map((m, i) => this.draw(m, undefined, i === 0 ? joined : undefined));
    const thread = this.controller.createCommentThread(uri, range ?? new vscode.Range(0, 0, 0, 0), drawn);
    drawn.forEach((c, i) => this.shownAs.set(c, { message: messages[i], thread }));
    this.setJoined(thread, joined);
    thread.collapsibleState = vscode.CommentThreadCollapsibleState.Collapsed;
    return thread;
  }

  /**
   * Makes `joined` the line of what its highlight is joined to that closes `thread`'s first comment
   * from the thread's next drawing on, or takes the line away when there is none.
   */
  setJoined(thread: vscode.CommentThread, joined: string | undefined) {
    if (joined) this.joined.set(thread, joined);
    else this.joined.delete(thread);
  }

  /**
   * Message `m` as a comment of `thread`: its text with each reference drawn as a link that names
   * it in a few words (`References.markdown`), labelled edited when it was, or, while it is being
   * edited there, in VS Code's editor holding the text being edited. One of the window's own
   * messages offers Edit. Nothing on a comment says that it is resolved: the thread's title bar
   * says that the thread is (FileThreads.show). The thread's first comment ends with the line of
   * what its highlight is joined to, when the thread has one, and not while it is being edited:
   * the line is fwd's, not the message's.
   */
  comment(m: MessageJson, thread: vscode.CommentThread): vscode.Comment {
    const c = this.draw(m, thread, this.roots.get(thread) === m.id ? this.joined.get(thread) : undefined);
    this.shownAs.set(c, { message: m, thread });
    return c;
  }

  /**
   * Message `m` as `comment` draws it, in its editor while it is being edited in `thread`, and
   * otherwise followed by `joined`, the line of what its highlight is joined to, when one is given.
   */
  private draw(m: MessageJson, thread: vscode.CommentThread | undefined, joined: string | undefined): vscode.Comment {
    const editing = this.editing.get(m.id);
    const opened = thread && editing?.thread === thread ? editing.text : undefined;
    const text = new vscode.MarkdownString(joined ? this.references.markdownThen(m.content, joined) : this.references.markdown(m.content));
    // Trusted for the one command a reference's link runs, so a command link in a message runs no other.
    text.isTrusted = { enabledCommands: [OPEN_REFERENCE] };
    return {
      body: opened ?? text,
      mode: opened === undefined ? vscode.CommentMode.Preview : vscode.CommentMode.Editing,
      author: { name: this.author(m.author) },
      label: m.edited ? 'edited' : undefined,
      timestamp: new Date(m.created),
      // Chooses whether the comment's title bar offers Edit (package.json, comments/comment/title).
      contextValue: this.me !== undefined && m.author === this.me ? 'own' : undefined,
    };
  }

  /**
   * The comment `thread` shows for message `m` when the thread is drawn again. While `m` is in its
   * editor there it is the comment already shown, so VS Code keeps what is written in the editor;
   * otherwise a new one.
   */
  commentIn(thread: vscode.CommentThread, m: MessageJson): vscode.Comment {
    if (this.editing.get(m.id)?.thread === thread) {
      const shown = thread.comments.find((c) => this.shownAs.get(c)?.message.id === m.id);
      if (shown) {
        this.shownAs.set(shown, { message: m, thread });
        return shown;
      }
    }
    return this.comment(m, thread);
  }

  /**
   * Opens one of the window's own comments in VS Code's editor (the comment's Edit), holding its
   * text as its author would type it again: people by `@handle`, decisions by `[[D12]]`, anything
   * else by the start of its id (`References.editable`).
   */
  async edit(comment: vscode.Comment) {
    const shown = this.shownAs.get(comment);
    if (!shown) return;
    const { message, thread } = shown;
    this.editing.set(message.id, { thread, text: await this.references.editable(message.content) });
    this.redraw(thread, message);
  }

  /**
   * Saves what the comment's editor holds as the message's new text with `fwd message edit` (the
   * editor's Save, which VS Code hands the comment with the editor's text as its body): the message
   * keeps its id, its thread and its place, and whoever the new text names and the old one did not
   * hears of it. A refusal (a name that fits nothing, a blank text) is shown in the CLI's words,
   * and the comment stays in its editor, holding what was written, to correct.
   */
  async save(comment: vscode.Comment) {
    const shown = this.shownAs.get(comment);
    if (!shown) return;
    const { message, thread } = shown;
    const text = typeof comment.body === 'string' ? comment.body : comment.body.value;
    // What was written, for a comment VS Code has not shown before, should the thread need one.
    this.editing.set(message.id, { thread, text });
    const saved: MessageJson = await fwd(['message', 'edit', message.id, '--', text]);
    this.editing.delete(message.id);
    await this.referents.load(referencesIn(saved.content));
    this.redraw(thread, { ...message, content: saved.content, edited: saved.edited });
  }

  /** Closes the comment's editor and shows the message's text as it was (the editor's Cancel). */
  cancelEdit(comment: vscode.Comment) {
    const shown = this.shownAs.get(comment);
    if (!shown) return;
    this.editing.delete(shown.message.id);
    this.redraw(shown.thread, shown.message);
  }

  /** Draws message `m` in `thread` again, in its editor or not, as a comment VS Code has not shown before. */
  private redraw(thread: vscode.CommentThread, m: MessageJson) {
    thread.comments = thread.comments.map((c) => (this.shownAs.get(c)?.message.id === m.id ? this.comment(m, thread) : c));
  }

  /**
   * Posts what was written in a new thread's comment box, as a comment (the box's Send to agents,
   * or Comment on the task while a task is current) or as a question asked of no one in particular,
   * which the triage agent of its scope answers (Ask). The thread's lines become a highlight, and a
   * thread with no lines is on the whole file.
   */
  async create(reply: vscode.CommentReply, as: 'comment' | 'question') {
    const { uri, range } = reply.thread;
    const rel = this.anchorablePath(uri, range ? 'lines' : 'file');
    await this.post(as, rel, range && `${range.start.line + 1}-${range.end.line + 1}`, reply.text, revisionArgs(uri));
    // This thread was VS Code's draft; the node hears the new anchor and message, and the file's
    // live queries show the comment in a thread of its own.
    reply.thread.dispose();
  }

  /**
   * Posts `text` about `lines` of the file `rel` (a-b, 1-based; none for the whole file), which
   * become a highlight, as a comment or as a question; `revision` names the revision the lines are
   * of when a revision's side of a diff showed them (`revisionArgs`). With no task current, a
   * comment goes to the agents working here (`fwd comment --path --lines`), whom routing tells, and
   * a question is asked of no one in particular (`fwd ask --path --lines`, with no `--to`), which
   * reaches the triage agent of the scope this checkout works in alone, else the one that serves the
   * whole project, or waits on the Questions page while neither exists (a question reaches only whom
   * it is addressed to, docs/DECISIONS.md D111; per scope, D122). With a task current, either is
   * on that task, with the highlight as its context (`fwd comment <task> ...`, `fwd ask --about
   * <task> ...`), and never a step: docs/DECISIONS.md, D92, "A new highlight made in the editor
   * with a task current is a comment or a question on that task, with the highlight as its
   * context; adding a step is a separate, explicit action".
   */
  private async post(as: 'comment' | 'question', rel: string, lines: string | undefined, text: string, revision: string[]) {
    const task = this.current.id;
    const where = ['--path', rel, ...(lines ? ['--lines', lines] : []), ...revision];
    if (as === 'comment') await fwd(['comment', ...(task ? [task] : []), ...where, '--', text]);
    else await fwd(['ask', ...(task ? ['--about', task] : []), ...where, '--', text]);
  }

  /**
   * Sends what was written in a thread's reply box as a reply to its first message (the box's
   * Reply); VS Code then empties the box and closes it to its one line again.
   */
  async replyTo(reply: vscode.CommentReply) {
    const root = this.roots.get(reply.thread);
    if (!root) return this.create(reply, 'comment');
    await fwd(['reply', root, reply.text]); // the node hears the reply, which joins the thread
  }

  /**
   * Adds the highlight `thread` is on as the next step of the current task (`fwd step add <task>
   * <anchor>`): the thread's Add as a Step, one of the explicit actions that make a step now that
   * a comment made in the editor never does (docs/DECISIONS.md, D92). The page shows the step
   * through its own live queries; the status bar says which step it is.
   */
  async addAsStep(thread: vscode.CommentThread) {
    const anchor = this.anchors.get(thread);
    const task = this.current.id;
    if (!anchor) return;
    if (!task) throw new Error('No task is current: open one on the page to add steps to it.');
    const step = await fwd(['step', 'add', task, anchor]);
    vscode.window.setStatusBarMessage(`$(tasklist) Step ${step?.record?.n ?? ''} of ${this.current.title ?? short(task)}`, 5000);
  }

  /**
   * Resolves the thread's first message (`fwd message resolve`); the thread follows it. On the first
   * thread of a highlight that is a step, which offers this as Resolve and Check the Step, fwd checks
   * the step too (D99, above), and the status bar says which.
   */
  async resolve(thread: vscode.CommentThread) {
    const root = this.roots.get(thread);
    if (!root) return;
    const done = await fwd(['message', 'resolve', root]);
    this.sayMoved(done?.checked_steps, 'Resolved the thread and checked');
  }

  /**
   * Sets the thread's first message back to active (`fwd message unresolve`); the thread follows it.
   * On the first thread of a highlight that is a step, which offers this as Unresolve and Uncheck the
   * Step, fwd unchecks the step too, and the status bar says which.
   */
  async unresolve(thread: vscode.CommentThread) {
    const root = this.roots.get(thread);
    if (!root) return;
    const done = await fwd(['message', 'unresolve', root]);
    this.sayMoved(done?.unchecked_steps, 'Set the thread back to active and unchecked');
  }

  /** Says in the status bar which tasks' steps moved with a thread, when any did. */
  private sayMoved(steps: Json[] | undefined, what: string) {
    if (!steps?.length) return;
    const on = steps.map((s) => `“${s.task_title ?? short(s.task ?? '')}”`).join(', ');
    vscode.window.setStatusBarMessage(`$(tasklist) ${what} its step of ${on}`, 5000);
  }

  /** Opens the thread of `root` on the document `uri` and shows its lines: the hover's "open". */
  async openThread(uri: string, root: string) {
    const shown = this.files.get(uri)?.shownThread(root);
    if (!shown) return;
    shown.thread.collapsibleState = vscode.CommentThreadCollapsibleState.Expanded;
    const editor = vscode.window.visibleTextEditors.find((e) => e.document.uri.toString() === uri);
    if (editor && shown.span) editor.revealRange(new vscode.Range(shown.span.start, 0, shown.span.end - 1, 0), vscode.TextEditorRevealType.InCenterIfOutsideViewport);
  }

  /**
   * Opens the comment box in the editor showing `uri` (the active editor when there is none), with
   * VS Code's own "Add Comment on Current Selection": on the whole lines of the selection, or on the
   * whole file. The box says where a new comment goes (`newCommentWording`), and its buttons say
   * whether it is a comment or a question (package.json, comments/commentThread/context). Like
   * VS Code's command, it opens or closes the threads already there instead when the selection's
   * last line ends one, or, for the whole file, when the file has threads on the whole file.
   */
  async openCommentBox(uri: vscode.Uri | undefined, on: 'lines' | 'file') {
    const editor = editorShowing(uri);
    if (!editor) return;
    this.anchorablePath(editor.document.uri, on);
    let where: object = { fileComment: true };
    if (on === 'lines') {
      const { start, end } = wholeLines(editor.selection);
      where = { range: { startLineNumber: start + 1, endLineNumber: end + 1, endColumn: 1 } };
    }
    // VS Code's command acts on the editor that has the focus: the one the menu was opened on.
    if (editor !== vscode.window.activeTextEditor) await vscode.window.showTextDocument(editor.document, editor.viewColumn);
    // VS Code gives each box it builds, a new comment's or a thread's reply box, the controller's
    // wording as it is when the box is built, and keeps it for as long as the box lasts. So the
    // controller carries the wording that says where a new comment goes only while VS Code builds
    // this box: the new box's thread reaches VS Code before the wording is taken back, since the
    // extension host and VS Code each handle the other's messages in order. At all other times the
    // controller carries none, and VS Code's own words show: "Reply..." on a thread's reply box,
    // "Type a new comment" in a box opened with VS Code's own + in the gutter.
    this.controller.options = this.newCommentWording();
    try {
      await vscode.commands.executeCommand('workbench.action.addComment', where);
    } finally {
      this.controller.options = undefined;
    }
  }

  /**
   * Asks no one in particular about the whole lines of the selection in the editor showing `uri`
   * (the active editor when there is none), in the words the input box takes, and about the current
   * task when there is one, as the box's Ask does: the lines become a highlight whose thread the
   * question starts, and routing tells the triage agent of the scope it concerns, and no one else.
   * The node hears the new anchor and message, so the file's live queries show the thread, where
   * the answers arrive as replies.
   */
  async askAbout(uri?: vscode.Uri) {
    const target = this.selectedLines(uri);
    if (!target) return;
    const on = this.current.title ? `, on ${this.current.title}` : '';
    const text = await vscode.window.showInputBox({
      title: `Ask about ${target.rel}, lines ${target.lines}${on}`,
      prompt: `Asked of ${ASKED_OF_NO_ONE}. Your question starts a thread on these lines, where the answer comes; @handle asks someone else too. Enter sends it.`,
      ignoreFocusOut: true,
    });
    if (text?.trim()) await this.post('question', target.rel, target.lines, text, target.revision);
  }

  /**
   * Adds the whole lines of the selection in the editor showing `uri` (the active editor when there
   * is none), or the whole file (`on`), as the next step of the current task, with the words the
   * input box takes as the step's first comment (`fwd step add <task> --path <file> --lines a-b`,
   * without `--lines` for the whole file): the explicit way to make a new highlight a step, since a
   * comment or a question made in the editor never makes one (docs/DECISIONS.md, D92). fwd makes
   * the step before it posts the comment, so the comment reaches the task's creator and assignee.
   */
  async stepAbout(uri: vscode.Uri | undefined, on: 'lines' | 'file') {
    const target = this.selectedLines(uri, on);
    const task = this.current.id;
    if (!target) return;
    if (!task) throw new Error('No task is current: open one on the page to add steps to it.');
    const lines = on === 'lines' ? target.lines : undefined;
    const text = await vscode.window.showInputBox({
      title: `A step of ${this.current.title ?? short(task)}: ${target.rel}, ${lines ? `lines ${lines}` : 'the whole file'}`,
      prompt: "The step's first comment: what to look at here, and why. Enter adds the step.",
      ignoreFocusOut: true,
    });
    if (text?.trim()) await fwd(['step', 'add', task, '--path', target.rel, ...(lines ? ['--lines', lines] : []), ...target.revision, '--', text]);
  }

  /**
   * The file and the whole lines (a-b, 1-based) of the selection in the editor showing `uri`, when
   * fwd can anchor them, or the whole file (`on`), with the revision they are of when that editor
   * shows one (`revisionArgs`).
   */
  private selectedLines(uri: vscode.Uri | undefined, on: 'lines' | 'file' = 'lines'): { rel: string; lines: string; revision: string[] } | undefined {
    const editor = editorShowing(uri);
    if (!editor) return undefined;
    const { start, end } = wholeLines(editor.selection);
    return { rel: this.anchorablePath(editor.document.uri, on), lines: `${start + 1}-${end + 1}`, revision: revisionArgs(editor.document.uri) };
  }

  /**
   * The repository-relative path of the file `uri` shows, when fwd can anchor it: lines of a file
   * in the working tree of this workspace, in an editor of its own or on either side of a diff,
   * where a revision's lines are anchored as that revision has them; and the whole file on the
   * file on disk only, where the whole file's threads show. Anything else is refused with a
   * sentence saying why.
   */
  anchorablePath(uri: vscode.Uri, on: 'lines' | 'file' = 'lines'): string {
    const side = sideOf(uri);
    const rel = side && this.relPath(uri);
    if (!rel) throw new Error('Only files in the working tree of this workspace can be commented on.');
    if (on === 'file' && side?.kind === 'revision') {
      throw new Error('A comment or a step on the whole file is made on the file itself, where its threads show: use the working-tree side, or the file.');
    }
    return rel;
  }

  /** After a document's threads were built again: the count of highlights may differ. */
  changed() {
    this.showCount();
  }

  /**
   * What a comment box that fwd opens says (`openCommentBox`): where a new comment or question
   * goes, on the current task, with the code as its context (docs/DECISIONS.md, D92), or to the
   * agents working here. VS Code shows `prompt` on the box while it is closed and `placeHolder` in
   * it while it is empty.
   */
  newCommentWording(): vscode.CommentOptions {
    const title = this.current.title;
    return title
      ? { prompt: `A comment or a question on ${title}`, placeHolder: `A comment or a question on ${title}, about this code` }
      : { prompt: 'Comment for the agents working here', placeHolder: 'For the agents working here: is this related to a task? what should they watch for?' };
  }

  /** The first lines of the threads of the highlights on the hovered line, each with a link that opens its thread. */
  private hover(document: vscode.TextDocument, position: vscode.Position): vscode.Hover | undefined {
    const file = this.files.get(document.uri.toString());
    const here = (file?.highlighted() ?? []).filter(({ span }) => position.line >= span.start && position.line < span.end);
    if (!here.length) return undefined;
    const text = new vscode.MarkdownString();
    text.isTrusted = { enabledCommands: ['fwd.openThread'] };
    here.forEach(({ root, replies }, i) => {
      if (i) text.appendMarkdown('\n\n---\n\n');
      text.appendMarkdown(`**${plain(this.author(root.author))}**: `);
      text.appendMarkdown(plain(firstLines(this.references.render(root.content))));
      const args = encodeURIComponent(JSON.stringify([document.uri.toString(), root.id]));
      const more = replies ? ` (${replies} ${replies === 1 ? 'reply' : 'replies'})` : '';
      text.appendMarkdown(`\n\n[open](command:fwd.openThread?${args} "Open this thread")${more}`);
    });
    return new vscode.Hover(text);
  }

  private showCount() {
    const editor = vscode.window.activeTextEditor;
    const n = (editor && this.files.get(editor.document.uri.toString())?.highlights) ?? 0;
    this.countText = n ? `$(comment-discussion) ${n}` : undefined;
    if (!this.countText) return this.count.hide();
    this.count.text = this.countText;
    this.count.tooltip = `${n} highlight${n === 1 ? '' : 's'} in this file: comment threads not resolved yet. Click for the next thread.`;
    this.count.show();
  }

  private watch(doc: vscode.TextDocument) {
    const key = doc.uri.toString();
    const side = sideOf(doc.uri);
    const rel = side && this.relPath(doc.uri);
    if (!this.node || !side || !rel || this.files.has(key)) return;
    this.files.set(key, new FileThreads(this, this.node, doc, rel, side));
  }

  private unwatch(doc: vscode.TextDocument) {
    const key = doc.uri.toString();
    this.files.get(key)?.dispose();
    this.files.delete(key);
    this.showCount();
  }
}

/**
 * The threads and highlights on one open document. A live query on the anchors in the file's path,
 * and on each anchor one on its messages, one on the steps it is and one on the relations that
 * point at it (`HostNode.joinsTo`), rebuild them on every change set: one thread per root message,
 * its replies as the thread's comments, and a highlight for each anchor with a thread not
 * resolved. Where each anchor sits in the version the document shows
 * comes from the CLI (`fwd anchor list --against`, which resolves with git): when the document
 * opens, when an anchor arrives, and in the working tree after each save, while edits in between
 * move the lines along (`edited`). Until the CLI answers, a thread in the working tree sits on the lines its anchor was
 * made on, when it was made on the file on disk; a revision shows only the anchors the CLI found in it, and so does the
 * working tree an anchor made on a revision.
 */
class FileThreads {
  readonly threads = new Map<string, vscode.CommentThread>(); // by root message id
  readonly uri: vscode.Uri;
  /** How many highlights the document shows, on lines or on the whole file. */
  highlights = 0;
  private shown = new Map<string, Shown>(); // by root message id
  private anchors: Live<AnchorLines>;
  private messages = new Map<string, Live<MessageJson>>(); // by anchor id
  private steps = new Map<string, Live<StepJson>>(); // by anchor id
  private joins = new Map<string, Live<JoinJson>>(); // by anchor id
  /** The tasks the threads name, in a label or a line of what a highlight is joined to, as last built. */
  tasks = new Set<string>();
  /** The principals the threads show as the authors of their comments, as last built. */
  authors = new Set<string>();
  /** The ids the threads' comments refer to in their texts, the people and agents they mention among them, as last built. */
  referenced = new Set<string>();
  /** Each anchor's resolution in this version of the file, from the CLI. */
  private placed = new Map<string, Json>();
  /**
   * Each anchor's lines in this document now, as the CLI placed them and the edits since have
   * moved them: null when the CLI found none in this version. The drawn highlights, the hover and
   * the hover's "open" all read them here.
   */
  private lines = new Map<string, Span | null>();
  /** The anchors the CLI has been asked to place, so one that arrives later is asked about too. */
  private asked = new Set<string>();
  /** The anchors drawn as highlights on lines: those with a thread not resolved and lines here. */
  private drawn: string[] = [];
  /** How many times the document has been saved, so that a placing can tell that a save came while the CLI ran (`placeAll`). */
  private saves = 0;
  private disposed = false;
  /**
   * Kills the placing under way when the threads are disposed, as the document closes or the
   * window deactivates: its answer is of no use then, and a placing whose daemon went away runs on
   * until the CLI gives up on the daemon, up to ten seconds later (`LOST_AFTER` in
   * crates/fwd-core/src/daemon.rs).
   */
  private cancel = new AbortController();
  private render = serially(() => this.rebuild());
  private place = serially(() => this.placeAll());

  constructor(
    private comments: Comments,
    private node: HostNode,
    private document: vscode.TextDocument,
    readonly rel: string,
    readonly side: Side,
  ) {
    this.uri = document.uri;
    this.anchors = node.anchors(rel, this.render);
    this.place();
  }

  /**
   * The threads of the highlights on lines of this document, which the hover lists, each with its
   * highlight's lines as they are when the hover asks. The lines are read in `lines`, which each
   * edit moves as it is made (`edited`) and from which the highlights are drawn (`paint`), so that
   * right after an unsaved edit a hover finds a thread on the lines drawn with its highlight and
   * gutter mark. The thread's `range` would not do: VS Code moves a thread along with the text in
   * its own copy of the thread only, and the extension's `range` stays where the extension last
   * put it.
   */
  highlighted(): (Shown & { span: Span })[] {
    return [...this.shown.values()].flatMap((s) => {
      const span = this.spanOf(s);
      return span ? [{ ...s, span }] : [];
    });
  }

  /** The thread of `root`, with its highlight's lines as they are now when it is a highlight on lines. */
  shownThread(root: string): (Shown & { span?: Span }) | undefined {
    const shown = this.shown.get(root);
    return shown && { ...shown, span: this.spanOf(shown) };
  }

  /** The file was saved: the CLI places the anchors in the working tree again. */
  saved() {
    this.saves++;
    if (this.side.kind === 'worktree') this.place();
  }

  /** Builds the threads again from what the live queries hold: once the window knows whom it acts as. */
  redraw() {
    this.render();
  }

  /**
   * Follows an edit, so that the drawn highlights and the hover are on the lines an unsaved edit
   * moved them to from the moment it is made. In the working tree each anchor's lines move with
   * each change (`moved`), and the highlights are drawn on them again at once in every editor
   * showing the document. VS Code moves drawn highlights along with the text by itself, but by its
   * own rule for decorations, which can differ: Enter typed at the start of a highlight's first
   * line puts the new empty line inside the drawn highlight, where the extension's lines, which
   * the hover reads, move down past it. VS Code moves the threads along by itself too, and the
   * extension puts them on its lines when it next builds them. A change that leaves the document
   * as the file on disk (VS Code reading the file again after something else wrote it, an undo
   * back to the saved text) is placed by the CLI again, and so is a revision the git extension
   * serves again (the index, after a stage). The first edit after a save asks the CLI too, since
   * VS Code hands it over with the document not dirty yet (the dirty flag follows in an event of
   * its own); the CLI's answer, which comes after the edit, leaves the moved lines as they are.
   */
  edited(e: vscode.TextDocumentChangeEvent) {
    let moves = false;
    for (const change of this.side.kind === 'worktree' ? e.contentChanges : []) {
      for (const [id, span] of this.lines) {
        const now = span && moved(span, change);
        if (!span || !now || (now.start === span.start && now.end === span.end)) continue;
        this.lines.set(id, now);
        moves = true;
      }
    }
    if (moves) for (const editor of this.editors()) this.paint(editor);
    if (!e.document.isDirty && e.contentChanges.length) this.place();
  }

  /** Draws the highlights on `editor`, one of the editors showing this document, on their lines as they are now. */
  paint(editor: vscode.TextEditor) {
    const ranges = this.drawn.flatMap((id) => {
      const span = this.lines.get(id);
      return span ? [new vscode.Range(span.start, 0, span.end - 1, 0)] : [];
    });
    editor.setDecorations(this.comments.highlight, ranges);
    this.comments.painted.set(editor, ranges);
  }

  dispose() {
    this.disposed = true;
    this.cancel.abort();
    this.anchors.stop();
    for (const live of this.messages.values()) live.stop();
    this.messages.clear();
    for (const live of this.steps.values()) live.stop();
    this.steps.clear();
    for (const live of this.joins.values()) live.stop();
    this.joins.clear();
    this.tasks.clear();
    this.comments.referents.hide(this);
    for (const thread of this.threads.values()) thread.dispose();
    this.threads.clear();
    this.shown.clear();
    this.drawn = [];
    for (const editor of this.editors()) this.paint(editor);
  }

  /**
   * Asks the CLI where each anchor in the file sits in the version this document shows. The window
   * asks this by itself (a document opened or saved, a highlight arrived), never at a person's
   * command, so the call never starts a daemon (`--no-autostart`), as the node never starts one
   * while it waits for the daemon to come back (`rejoinWhenBack`, src/extension.ts): with none
   * running, the CLI refuses it and the threads stay where they are. Nor does the call outlive the
   * threads: disposing them kills it (`cancel`), and they ask nothing more, though `serially` runs
   * this once more when a placing was asked for while it ran. So a daemon stopped to install a new
   * build stays stopped, and a closing window leaves no placing behind to start one after the
   * daemon was stopped, as one did on CI once the headless test had stopped its scratch daemon
   * (task lK-_4TEE).
   */
  private async placeAll() {
    if (this.disposed) return;
    for (const anchor of this.anchors.items()) this.asked.add(anchor.id);
    const against = this.side.kind === 'worktree' ? 'worktree' : this.side.against;
    const saves = this.saves;
    let listed: Json[];
    try {
      const args = ['--no-autostart', 'anchor', 'list', '--path', this.rel, '--against', against];
      listed = (await fwd(args, { signal: this.cancel.signal })) ?? [];
    } catch (e) {
      // Killed as the threads were disposed, or failed after: no one reads the answer.
      if (this.disposed) return;
      // No daemon, no git: working-tree threads stay on the lines their anchors were made on
      // there, and an anchor made on a revision shows nowhere.
      console.warn(`fwd: placing the anchors of ${this.rel} against ${against}:`, e);
      return;
    }
    if (this.disposed) return;
    // The CLI read the file on disk; while the editor holds unsaved edits, the lines the edits
    // moved stand. They stand too when the file was saved while the CLI ran, since the CLI may
    // have read the file before the save wrote it; the placing the save asked for comes next.
    const unsaved = this.side.kind === 'worktree' && (this.document.isDirty || this.saves !== saves);
    for (const a of listed) {
      this.asked.add(a.id);
      this.placed.set(a.id, a.resolved);
      if (unsaved && this.lines.has(a.id)) continue;
      const range = a.resolved?.range;
      this.lines.set(a.id, range ? { start: range.start_line, end: range.end_line } : null);
    }
    this.render();
  }

  private async rebuild() {
    if (this.disposed) return;
    const anchors = this.anchors.items();
    const current = new Set(anchors.map((a) => a.id));
    for (const lives of [this.messages, this.steps, this.joins] as Map<string, Live<unknown>>[]) {
      for (const [id, live] of lives) {
        if (current.has(id)) continue;
        live.stop();
        lives.delete(id);
      }
    }
    for (const id of current) {
      if (!this.messages.has(id)) this.messages.set(id, this.node.messages(id, this.render));
      if (!this.steps.has(id)) this.steps.set(id, this.node.steps(id, this.render));
      if (!this.joins.has(id)) this.joins.set(id, this.node.joinsTo(id, this.render));
    }
    if (anchors.some((a) => !this.asked.has(a.id))) this.place();
    // Until the CLI answers, an anchor made on the file on disk sits in the working tree on the
    // lines it was made on, which edits then move; one made on a revision's side of a diff counts
    // that revision's lines, which the working tree may have elsewhere, so it waits (`placeOf`).
    for (const a of anchors) {
      if (this.side.kind === 'worktree' && a.onDisk && !a.whole && !this.lines.has(a.id)) this.lines.set(a.id, { start: a.start_line, end: a.end_line });
    }
    const held = new Map(anchors.map((a) => [a.id, this.messages.get(a.id)!.items()]));
    const notes = new Map([...held].map(([id, all]) => [id, all.filter((m) => m.status !== 'deleted')]));
    const messages = [...notes.values()].flat();
    const steps = new Map(anchors.map((a) => [a.id, this.steps.get(a.id)!.items()]));
    const joins = new Map(anchors.map((a) => [a.id, this.joins.get(a.id)!.items()]));
    const codeOf = [...joins.values()].flat().filter((j) => j.kind === 'anchored_at');
    this.tasks = new Set([...[...steps.values()].flat().map((s) => s.task), ...codeOf.map((j) => j.from)]);
    this.authors = new Set(messages.map((m) => m.author));
    this.referenced = new Set(messages.flatMap((m) => referencesIn(m.content)));
    await this.comments.referents.show(this, [...this.tasks, ...this.referenced]);
    await this.comments.principals.load(this.authors);
    if (this.disposed) return;
    const shown = new Set<string>();
    const drawn: string[] = [];
    let highlights = 0;
    for (const anchor of anchors) {
      const where = this.placeOf(anchor);
      if (!where) continue;
      const all = notes.get(anchor.id)!;
      const roots = all.filter((m) => !m.thread);
      if (roots.some((m) => m.status !== 'resolved')) {
        highlights++;
        if (where.span) drawn.push(anchor.id);
      }
      // The steps the anchor is move with its first thread, and with no other; that thread's first
      // comment ends with the line of what the highlight is joined to.
      const isStep = steps.get(anchor.id)!;
      const own = new Set(held.get(anchor.id)!.map((m) => m.id));
      const joined = joinedLine(anchor.id, isStep, joins.get(anchor.id)!, own, (id) => this.comments.title(id));
      const first = isStep.length || joined ? this.node.firstThread(all) : undefined;
      for (const root of roots) {
        const replies = all.filter((m) => m.thread === root.id).sort((a, b) => a.created - b.created);
        const isFirst = root.id === first;
        this.show(anchor, root, replies, where.span, isFirst ? isStep : [], isFirst ? joined : undefined);
        shown.add(root.id);
      }
    }
    for (const [id, thread] of this.threads) {
      if (shown.has(id)) continue;
      thread.dispose();
      this.threads.delete(id);
      this.shown.delete(id);
    }
    this.highlights = highlights;
    this.drawn = drawn;
    for (const editor of this.editors()) this.paint(editor);
    this.comments.changed();
  }

  /**
   * Where an anchor's threads go in this document: on lines (`span`), on the whole file (no
   * span), or nowhere (undefined). The working tree shows every anchor of its file: on its lines,
   * and on the whole file when the anchor is on the whole file or its lines are gone. A revision
   * shows the anchors whose lines the CLI found in it, and no whole-file anchor, whose thread the
   * working tree has. An anchor on lines the CLI has not placed yet shows nowhere, unless it was
   * made on the file on disk (above, `rebuild`).
   */
  private placeOf(anchor: AnchorLines): { span?: Span } | undefined {
    const span = this.lines.get(anchor.id);
    if (span === undefined && !anchor.whole) return undefined;
    if (this.side.kind === 'revision') return anchor.whole || !span ? undefined : { span };
    return anchor.whole || !span ? {} : { span };
  }

  /**
   * Shows `root` and its replies as one thread, on `span` or on the whole file, its label saying
   * whether it is resolved and where its anchor sits; `steps` are the steps that move with it, when
   * it is the first thread on a highlight that is a step, which its label names too, and `joined`
   * the line of what the highlight is joined to that closes its first comment, when it is the
   * first thread on a highlight joined to anything.
   */
  private show(anchor: AnchorLines, root: MessageJson, replies: MessageJson[], span: Span | undefined, steps: StepJson[], joined: string | undefined) {
    const range = span && new vscode.Range(span.start, 0, span.end - 1, 0);
    const messages = [root, ...replies];
    const made = this.threads.get(root.id);
    const thread = made ?? this.comments.makeThread(this.uri, range, messages, joined);
    if (made) {
      this.comments.setJoined(thread, joined);
      thread.comments = messages.map((m) => this.comments.commentIn(thread, m));
    } else {
      this.comments.roots.set(thread, root.id);
      this.comments.anchors.set(thread, anchor.id);
      this.threads.set(root.id, thread);
    }
    // Set only when it changed: VS Code moves a thread with the edits by itself.
    if (range === undefined ? thread.range !== undefined : !thread.range?.isEqual(range)) thread.range = range;
    const resolved = root.status === 'resolved';
    const said = steps.map((s) => `Step of “${this.comments.title(s.task)}”: ${s.checked ? 'checked' : 'not checked'}`);
    // A resolved thread says so here, once, rather than on its first comment; the first thread of a
    // highlight whose steps are all checked says it already, since it moves with them (D99, above).
    const settled = resolved && !(steps.length && steps.every((s) => s.checked)) ? ['resolved'] : [];
    const placed = labelOf(this.placed.get(anchor.id), anchor);
    // VS Code leaves a title bar's words as they were when the label is set to nothing (or to an
    // empty string), so a label with nothing to say is a space.
    thread.label = [...said, ...settled, ...(placed ? [placed] : [])].join(' · ') || ' ';
    thread.state = resolved ? vscode.CommentThreadState.Resolved : vscode.CommentThreadState.Unresolved;
    // Chooses the title bar's action (package.json, comments/commentThread/title): Resolve or
    // Unresolve, which on the first thread of a highlight that is a step (`unresolvedStep`,
    // `resolvedStep`) read as also checking or unchecking the step, since fwd does both there.
    thread.contextValue = `${resolved ? 'resolved' : 'unresolved'}${steps.length ? 'Step' : ''}`;
    this.shown.set(root.id, { thread, root, replies: replies.length, highlight: resolved || !span ? undefined : anchor.id });
  }

  /** The lines of the highlight `shown` is on as they are now; none when it is on no highlight on lines. */
  private spanOf(shown: Shown): Span | undefined {
    return (shown.highlight !== undefined && this.lines.get(shown.highlight)) || undefined;
  }

  private editors(): vscode.TextEditor[] {
    return vscode.window.visibleTextEditors.filter((e) => e.document.uri.toString() === this.uri.toString());
  }
}

/**
 * The editor a command from the editor's menus acts on: the one showing `uri`, the document the
 * menu was opened on, preferring the active editor when several show it; with no `uri` (the
 * command palette), the active editor.
 */
function editorShowing(uri: vscode.Uri | undefined): vscode.TextEditor | undefined {
  const active = vscode.window.activeTextEditor;
  if (!(uri instanceof vscode.Uri)) return active;
  const shows = (e: vscode.TextEditor) => e.document.uri.toString() === uri.toString();
  return active && shows(active) ? active : vscode.window.visibleTextEditors.find(shows);
}

/**
 * The lines a selection takes, whole, 0-based and inclusive: a selection that ends at the start of
 * a line does not take that line in.
 */
function wholeLines(selection: vscode.Selection): { start: number; end: number } {
  const { start, end } = selection;
  return { start: start.line, end: end.line > start.line && end.character === 0 ? end.line - 1 : end.line };
}

/**
 * Where `span` is after `change` (the text from the change's range start to its end replaced with
 * the change's text), so that a highlight keeps to its text until the CLI places it again. A span
 * runs from the start of its first line to the start of the line after its last, and each of
 * those two places goes where the text beside it goes: a place before the change stays, a place
 * after it moves with the text after it, and a place inside the text the change replaced goes to
 * the change's start (the span's start) or to the end of the new text (the span's end), so that
 * the span takes in all of the change. Text put in exactly at either place goes outside the span:
 * lines added right above a highlight move it down, where the CLI finds it once the file is
 * saved, and lines added right below it leave it as it was.
 */
function moved(span: Span, change: vscode.TextDocumentContentChangeEvent): Span {
  const { start: from, end: to } = change.range;
  const added = change.text.split('\n');
  // The text that followed the change goes on from the end of the new text.
  const end = new vscode.Position(from.line + added.length - 1, (added.length > 1 ? 0 : from.character) + added[added.length - 1].length);
  // Where the start of line `line` goes. Text put in exactly there goes before it when it is the
  // span's start (`first`), and after it when it is the span's end: outside the span either way.
  const place = (line: number, first: boolean): vscode.Position => {
    const at = new vscode.Position(line, 0);
    if (at.isBefore(from) || (at.isEqual(from) && !(first && to.isEqual(from)))) return at;
    if (at.isBefore(to)) return first ? from : end;
    return at.line === to.line ? end : at.translate(end.line - to.line);
  };
  const start = place(span.start, true).line;
  const after = place(span.end, false);
  // A span whose end comes to the middle of a line takes that line in.
  return { start, end: Math.max(after.character ? after.line + 1 : after.line, start + 1) };
}

/**
 * The line of what the highlight `anchor` is joined to beyond its threads, which closes the first
 * comment of its first thread, in the words the highlight's own page gives those groups: the
 * tasks it is a step of (its `steps`; whether each is checked, the label says) and the tasks whose
 * code it is (its `anchored_at` relations), each a link that opens the task on the page, and how
 * many messages name it, not counting its own threads' (`own`), a link that opens the
 * highlight's own page, which lists them. None when it is joined to nothing, and its first
 * comment ends with its own text.
 */
function joinedLine(anchor: string, steps: StepJson[], joins: JoinJson[], own: Set<string>, title: (task: string) => string): string | undefined {
  const tasks = (ids: string[]) => [...new Set(ids)].map((id) => taskLink(id, title(id))).join(', ');
  const stepOf = steps.map((s) => s.task);
  const codeOf = joins.filter((j) => j.kind === 'anchored_at').map((j) => j.from);
  const naming = new Set(joins.filter((j) => j.kind === 'mention' && !own.has(j.from)).map((j) => j.from)).size;
  const groups = [
    ...(stepOf.length ? [`*step of* ${tasks(stepOf)}`] : []),
    ...(codeOf.length ? [`*code of* ${tasks(codeOf)}`] : []),
    ...(naming ? [`*mentioned in* ${openLink(anchor, 'highlight', `${naming} message${naming === 1 ? '' : 's'}`, 'The messages that name this highlight, listed on its page')}`] : []),
  ];
  return groups.length ? groups.join(' · ') : undefined;
}

/**
 * What a thread's label says of where its anchor sits in this version: nothing when its lines
 * are there as they were, at the same place or another, and otherwise the resolution's word, with
 * the lines the anchor was made on when its lines are gone.
 */
function labelOf(resolved: Json, anchor: AnchorLines): string | undefined {
  const word = resolved?.label ?? resolved?.state;
  if (!word || word === 'exact' || word === 'shifted') return undefined;
  const gone = !anchor.whole && !resolved.range;
  return `anchor ${word}${gone ? ` (original lines ${anchor.start_line + 1}-${anchor.end_line})` : ''}`;
}

/**
 * `text` as Markdown that shows the text as it is: the first lines of a comment are cut from
 * their Markdown, where an opened code block or emphasis would run on into what follows them.
 */
function plain(text: string): string {
  return text.replace(/[\\`*_{}[\]()#+\-.!|<>~]/g, '\\$&').replace(/\n/g, '  \n');
}

/** The first three lines of a comment, at most 300 characters. */
function firstLines(text: string): string {
  const lines = text.split('\n');
  let head = lines.slice(0, 3).join('\n');
  if (head.length > 300) head = `${head.slice(0, 299)}…`;
  else if (lines.length > 3) head += ' …';
  return head;
}
