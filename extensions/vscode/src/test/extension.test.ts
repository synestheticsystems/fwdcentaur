// The headless test: the extension inside a real VS Code, against the scratch project and daemon
// that .vscode-test.mjs made. It checks that the build stamped its version with its commit; that
// the editor's right-click menu offers fwd's items, titled as fwd's, wherever fwd can anchor what
// is selected and nowhere else, that the palette lists only commands titled as fwd's, and that a
// new thread's box offers a comment and a question, and a thread Reply, and Add as a Step while a
// task is current; that the inbox and the comment threads follow the project through the extension
// host's node with no poll and no refresh, and that the inbox's commands write through the node;
// that a thread not resolved is a highlight drawn on its lines, whose thread the cursor or a
// selection entering and leaving them leaves as it was (the gutter's comment icon opens and closes
// it) and whose hover shows it, that Resolve and Unresolve change its first message, the thread's
// label rather than its first comment saying that it is resolved, that an edit moves it at once,
// hover and all, to where the CLI finds it once saved, and both sides of a diff show it, while the
// copy of the file the gutter's change markers compare against shows no threads, so that VS Code's
// Comments panel lists each thread once, and that a highlight on the whole file badges the file's
// name instead;
// that every thread carries its reply box from the moment it is made, with no fwd: Reply to show
// it and no Cancel to hide it, and a reply sent from it joins the thread; that
// the menu's comment items open VS Code's comment box on the selection's lines, on a revision's
// side of a diff too, or on the whole file, which a revision's side refuses, saying where a new
// comment goes while VS Code builds the box and nothing at other times (so a thread's reply box
// says VS Code's own Reply...), that its question item asks no one in particular about the
// selected lines, which become a highlight, and that a
// comment on the whole file sent from its box becomes one; that one's own comment offers VS Code's
// Edit, whose Save edits the message with `fwd message edit` (a name that fits nothing refused, the
// comment kept in its editor), telling whom the new text names, and whose Cancel leaves it, while
// another's comment offers no Edit; that the task-graph command shows the web UI's page in a
// webview that boots, connects to the daemon (which admits the webview's origin because the
// extension registered it, and no other webview's) and follows the project, a task made and an
// agent renamed from the CLI showing on it without a reload; that the task the page
// shows becomes the window's current task, which the box fwd opens names and a comment or a
// question made in the editor goes to without making a step, while Add as a Step, from the menu on
// lines or on the whole file or on a thread, makes one, and a whole file's step tells the task's
// assignee of its first comment; that a reference in a comment is a link that names it in a few
// words and opens it on the page, and a file a comment names at a version a link naming its path
// and the version, which opens the file at that version, a commit's in the git extension's
// document of it, from the thread and from the page; that the first comment of a highlight's first thread ends with
// what the highlight is joined to, the tasks it is a step of, the tasks whose code it is and how
// many messages name it, each a link that opens it on the page, following the graph with no
// refresh, and that every thread's title bar opens the highlight's own page; that the
// first thread of a highlight that is a step offers Resolve and Check the Step, which checks the step
// as `fwd step list` shows, and Unresolve and Uncheck the Step, its label saying whether the step is
// checked, that checking the step with `fwd step check` or the page's box resolves or unresolves
// that thread, and that a later thread's plain Resolve leaves the step as it was; and that
// a step clicked on the page, or another highlight the page asks to reveal, opens the file in the
// normal editor, never a diff, with the cursor at its lines as the working tree has them and the
// step's thread left closed: lines as committed, lines in a change not staged, and a line in a
// staged change, where the working tree has it rather than the index; that a comment made on the
// index's side of the working tree's changes, and a comment and a question made on the two sides
// of the staged changes, are each a highlight on that side, with its thread there, which the page
// opens in the file at the lines the working tree has; that a highlight whose lines are gone from
// the working tree opens its file and says they are gone, and one whose file is gone says that;
// that the page gets back in after the daemon restarts, offering no newer page since the same build
// came back, and the threads and the inbox follow the project again, also when the daemon comes
// back on another port; that the page's request to reload the window, which the button of its bar
// offering a newer page sends, has VS Code reload the window, after which VS Code brings the panel
// back on the route the page was on, with the same current task; that a command reopens a page that
// lost the daemon; that closing the page clears the current task; that an agent that renames
// itself from the CLI shows its new name as its comment's author and where a comment mentions it,
// and a task renamed its new title where a comment mentions it, in the thread and in the inbox,
// with no reload; that a window acting as an agent goes on acting as it once the agent renames
// itself, its inbox following it through a refresh and a new connection, and its comment offering
// Edit, whose Save edits it; that with the daemon stopped, a document opened asks the CLI where its
// highlights are without starting a daemon; that deactivate, and a new node put in the node's
// place, free nothing still in use while reads are under way on the node and its live queries
// answer, twenty times over; and that deactivate frees the node; then it checks that no `fwd` call
// the extension started still runs, and that one scratch daemon does, which the runner stops once
// VS Code has exited (.vscode-test.mjs).

import * as assert from 'assert';
import { execFileSync } from 'child_process';
import * as fs from 'fs';
import * as http from 'http';
import * as net from 'net';
import * as path from 'path';
import * as vscode from 'vscode';
import type { Api, PageReport } from '../extension';

const EXTENSION = 'synestheticsystems.fwdcentaur';

/**
 * A request to the daemon at `port` as a page from `origin` would send it (none: as the CLI does),
 * with `Content-Type: text/plain`, which a page's POST may carry without asking first: its status,
 * and the origin the answer names.
 */
function ask(port: number, method: string, path: string, origin?: string): Promise<{ status: number; allowed?: string }> {
  const headers: Record<string, string> = { 'Content-Type': 'text/plain', ...(origin ? { Origin: origin } : {}) };
  return new Promise((resolve, reject) => {
    const request = http.request({ host: '127.0.0.1', port, method, path, headers, timeout: 5000 }, (response) => {
      response.resume();
      const allowed = response.headers['access-control-allow-origin'];
      response.on('end', () => resolve({ status: response.statusCode ?? 0, ...(allowed ? { allowed: String(allowed) } : {}) }));
    });
    request.on('timeout', () => request.destroy(new Error(`${method} ${path}: no answer within 5 seconds`)));
    request.on('error', reject);
    request.end();
  });
}

/** The first truthy answer of `probe`, asked every 100 ms for up to `ms`; undefined if none. */
async function until<T>(probe: () => T | PromiseLike<T>, ms = 10_000): Promise<T | undefined> {
  for (const deadline = Date.now() + ms; ; await new Promise((resolve) => setTimeout(resolve, 100))) {
    const answer = await probe();
    if (answer || Date.now() >= deadline) return answer || undefined;
  }
}

/**
 * The first report of what `page` shows that `ok` accepts, asked every 200 ms until `deadline` (a
 * time in ms since the epoch); else the last report the page gave, if any.
 */
async function untilPage(page: NonNullable<ReturnType<Api['page']>>, ok: (report: PageReport) => boolean, deadline: number) {
  let report: PageReport | undefined;
  for (;;) {
    report = (await page.probe(1000)) ?? report;
    if ((report && ok(report)) || Date.now() >= deadline) return report;
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
}

/** What every hover at `position` in `uri` says, their Markdown joined. */
async function hoverText(uri: vscode.Uri, position: vscode.Position): Promise<string> {
  const hovers = await vscode.commands.executeCommand<vscode.Hover[]>('vscode.executeHoverProvider', uri, position);
  return hovers.flatMap((h) => h.contents.map((c) => (typeof c === 'string' ? c : c.value))).join('\n');
}

/** The `git:` document of `file` at `ref`, as the git extension names the sides of its diffs. */
function gitUri(file: vscode.Uri, ref: string): vscode.Uri {
  return file.with({ scheme: 'git', path: file.path, query: JSON.stringify({ path: file.fsPath, ref }) });
}

/**
 * Whether the when-clause `when` holds where the context keys have the values in `keys` (a key not
 * there has none). It reads the part of VS Code's grammar the manifest uses: a key alone, `true`,
 * `false`, `!`, `&&`, `||`, parentheses, `key == value` and `key =~ /regex/`, with `!` binding
 * tightest, then `&&`, then `||`, as VS Code's reference on when-clause contexts orders them.
 * Anything else fails the test, so the manifest cannot outgrow this reading unnoticed.
 */
function holds(when: string, keys: Record<string, string | boolean>): boolean {
  const tokens = when.match(/\(|\)|!(?!=)|&&|\|\||==|=~|\/(?:\\.|[^/\\])*\/[a-z]*|[\w.:-]+/g) ?? [];
  assert.strictEqual(tokens.join(''), when.replace(/\s+/g, ''), `a when-clause this test cannot read: ${when}`);
  let at = 0;
  const take = (expected?: string) => {
    const token = tokens[at++];
    if (expected !== undefined) assert.strictEqual(token, expected, `in ${when}`);
    return token;
  };
  const either = (): boolean => {
    let value = both();
    while (tokens[at] === '||') {
      take();
      value = both() || value;
    }
    return value;
  };
  const both = (): boolean => {
    let value = one();
    while (tokens[at] === '&&') {
      take();
      value = one() && value;
    }
    return value;
  };
  const one = (): boolean => {
    const token = take();
    if (token === '!') return !one();
    if (token === '(') {
      const value = either();
      take(')');
      return value;
    }
    if (token === 'true' || token === 'false') return token === 'true';
    const value = keys[token];
    if (tokens[at] === '==') {
      take();
      return String(value) === take();
    }
    if (tokens[at] === '=~') {
      take();
      const [, source, flags] = /^\/(.*)\/([a-z]*)$/.exec(take()) ?? [];
      return typeof value === 'string' && new RegExp(source, flags).test(value);
    }
    return Boolean(value);
  };
  const value = either();
  assert.strictEqual(at, tokens.length, `in ${when}`);
  return value;
}

/** An item of a menu the manifest contributes. */
interface MenuItem {
  command: string;
  when: string;
  group?: string;
}

/**
 * The commands `menu` shows where the context keys have the values in `keys`, in the order VS Code
 * sorts them: by group, then by the order after the group's `@`.
 */
function offered(menu: MenuItem[], keys: Record<string, string | boolean>): string[] {
  const place = (item: MenuItem) => {
    const [group, order] = (item.group ?? '').split('@');
    return { group, order: Number(order ?? 0) };
  };
  return menu
    .filter((item) => holds(item.when, keys))
    .sort((a, b) => place(a).group.localeCompare(place(b).group) || place(a).order - place(b).order)
    .map((item) => item.command);
}

/**
 * Runs `run` with `object[name]` replaced by `standIn`, then puts the original back. The test and
 * the extension are files of one extension, which VS Code gives one API object, so the extension
 * calls the stand-in too: how the test answers an input box, and sees what the extension hands
 * VS Code where the extension API shows no result (the comment box VS Code opens).
 */
async function standingIn<T extends object, K extends keyof T>(object: T, name: K, standIn: T[K], run: () => Thenable<unknown>) {
  const original = object[name];
  object[name] = standIn;
  try {
    await run();
  } finally {
    object[name] = original;
  }
}

const icon = (item: vscode.TreeItem) => (item.iconPath as vscode.ThemeIcon).id;
/**
 * The line of what a highlight is joined to, which closes the first comment of its first thread
 * (src/comments.ts, `joinedLine`), and a comment's own text without it.
 */
const JOINED = /\n\n(\*(?:step of|code of|mentioned in)\* [^\n]*)$/;
const joinedOf = (c: vscode.Comment | undefined) => (c && JOINED.exec((c.body as vscode.MarkdownString).value ?? '')?.[1]) || undefined;
const textOf = (c: vscode.Comment) => (c.body as vscode.MarkdownString).value?.replace(JOINED, '');
const shown = (c: vscode.Comment) => `${c.author.name}: ${textOf(c)}`;
/** Each link in `markdown` that runs fwd.openReference: its words, the command's arguments, and its tooltip. */
const linksOf = (markdown: string | undefined) =>
  [...(markdown ?? '').matchAll(/\[((?:\\.|[^\]\\])*)\]\(command:fwd\.openReference\?([^ )]+) "((?:\\.|[^"\\])*)"\)/g)].map(([, words, args, tooltip]) => ({
    words: words.replace(/\\(.)/g, '$1'),
    args: JSON.parse(decodeURIComponent(args)),
    tooltip,
  }));
/** The first and last line of each range. */
const lines = (ranges: readonly (vscode.Range | undefined)[]) => ranges.map((r) => [r?.start.line, r?.end.line]);
const visible = (uri: vscode.Uri) => vscode.window.visibleTextEditors.find((e) => e.document.uri.toString() === uri.toString());
const activeTab = () => vscode.window.tabGroups.activeTabGroup.activeTab?.input;
/** The page is on the graph: the page adds the scope its filter opens on to the route. */
const onGraph = (hash: string) => /^#\/graph(\?|$)/.test(hash);
/** The page is on the task `id`: the page adds the scope the list behind it keeps to the route. */
const onTaskPage = (hash: string | undefined, id: string) => new RegExp(`^#/tasks/${id}(\\?scope=[A-Za-z0-9_-]+)?$`).test(hash ?? '');
const { Collapsed, Expanded } = vscode.CommentThreadCollapsibleState;

suite('fwdcentaur against a scratch daemon', () => {
  const runtimeDir = process.env.FWD_RUNTIME_DIR ?? '';
  const root = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ?? '';
  const bin = vscode.workspace.getConfiguration('fwd').get<string>('path') ?? '';
  const file = vscode.Uri.file(path.join(root, 'src', 'lib.rs'));
  const fwd = (...args: string[]) => execFileSync(bin, args, { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], timeout: 30_000 });
  /** The `ps` lines of daemons serving the scratch runtime directory. */
  const daemons = () =>
    execFileSync('ps', ['-Ao', 'pid,command'], { encoding: 'utf8' })
      .split('\n')
      .filter((line) => line.includes(`--runtime-dir ${runtimeDir}`));
  /** The `ps` lines of the `fwd` calls that the extension host, this process, started and that still run. */
  const calls = () =>
    execFileSync('ps', ['-Ao', 'pid,ppid,command'], { encoding: 'utf8' })
      .split('\n')
      .filter((line) => {
        const fields = /^\s*\d+\s+(\d+)\s+(.*)$/.exec(line);
        return fields !== null && Number(fields[1]) === process.pid && fields[2].startsWith(`${bin} `);
      });
  let api: Api;
  let redraws = 0;
  let comment: { id: string; anchor: string };
  let daemon: { port: number; project: string };
  let opened = 0; // when the graph command opened the page
  let review: string; // the task the page shows, which comments and questions made in the editor go to
  /** The page's header names the project and its connection state once the page has booted. */
  const booted = (report: PageReport) => report.text.includes('scratch') && report.text.includes('Connected');
  /**
   * Has the page ask for the highlight `anchor` to be shown in the editor, as a click on it does,
   * and waits for where a click on a highlight leads (docs/DECISIONS.md, D115): the file `uri` in
   * the normal editor, not a diff, as the active tab, with the cursor on `line`. The cursor of an
   * editor already showing the file is put on another line first, so that the line found is the
   * one the page's request put it on.
   */
  const revealsAt = async (anchor: string, uri: vscode.Uri, line: number) => {
    const elsewhere = new vscode.Position(line === 0 ? 1 : 0, 0);
    for (const e of vscode.window.visibleTextEditors.filter((e) => e.document.uri.toString() === uri.toString())) {
      e.selection = new vscode.Selection(elsewhere, elsewhere);
    }
    await api.page()!.probe(1000, { type: 'fwd:reveal', detail: anchor });
    const opened = await until(() => {
      const e = vscode.window.activeTextEditor;
      return activeTab() instanceof vscode.TabInputText && e?.document.uri.toString() === uri.toString() && e.selection.active.line === line;
    });
    const e = vscode.window.activeTextEditor;
    assert.ok(opened, `the active tab is ${JSON.stringify(activeTab())}, the cursor in ${e?.document.uri.toString()} on line ${e?.selection.active.line}, not ${uri.toString()} on ${line}`);
  };

  suiteSetup(async () => {
    // Never a person's daemon or data: .vscode-test.mjs set both directories for this run.
    assert.ok(runtimeDir && process.env.FWD_DATA_DIR, 'FWD_RUNTIME_DIR and FWD_DATA_DIR must name the scratch directories');
    assert.ok(path.isAbsolute(bin), `fwd.path must name the scratch run's binary, not ${bin}`);
    api = await vscode.extensions.getExtension<Api>(EXTENSION)!.activate();
    await api.ready;
    assert.ok(api.node(), 'the node joined the project');
    api.inbox.onDidChangeTreeData(() => redraws++);
  });

  test('the build stamped its version with the commit it was made from', () => {
    const extension = vscode.extensions.getExtension(EXTENSION)!;
    const git = (...args: string[]) => execFileSync('git', ['--no-optional-locks', ...args], { cwd: extension.extensionPath, encoding: 'utf8' }).trim();
    const [count, head] = [git('rev-list', '--count', 'HEAD'), git('rev-parse', '--short', 'HEAD')];
    const base = extension.packageJSON.version.replace(/\./g, '\\.');
    assert.match(api.version, new RegExp(`^${base}-${count}\\.g${head}(\\.dirty\\.\\d{14})?$`));
  });

  test("the editor's right-click menu offers fwd's items, marked as fwd's, wherever fwd can anchor what is selected", () => {
    const { commands, menus } = vscode.extensions.getExtension(EXTENSION)!.packageJSON.contributes;
    const titles = new Map<string, string>(commands.map((c: any) => [c.command, c.title]));
    const context: MenuItem[] = menus['editor/context'];
    // In a group of their own, sorted right after VS Code's navigation group, so separators set
    // them apart near the top of the menu; each title says it is fwd's.
    for (const { command, group } of context) {
      assert.match(group ?? '', /^0_fwd@\d$/, command);
      assert.match(titles.get(command) ?? '', /^fwd: /, command);
    }
    const onDisk = 'file:///repo/src/lib.rs';
    const revision = 'git:/repo/src/lib.rs?%7B%22path%22%3A%22%2Frepo%2Fsrc%2Flib.rs%22%2C%22ref%22%3A%22~%22%7D';
    // The context keys where the menu opens. `resourceScheme` is the scheme of the editor group's
    // active editor, which for a diff is its right-hand side on both of its sides, so a diff's
    // sides are told apart by `isInDiffLeftEditor`, `isInDiffRightEditor` and the sides' URIs.
    const diff = (side: 'Left' | 'Right', original: string, modified: string) => ({
      resourceScheme: modified.split(':')[0],
      isInDiffEditor: true,
      [`isInDiff${side}Editor`]: true,
      diffEditorOriginalUri: original,
      diffEditorModifiedUri: modified,
    });
    // Where fwd can anchor what is selected: lines of the file on disk, and of a revision on a side
    // of a diff (the index, HEAD), which fwd anchors as the revision has them; the whole file on the
    // file on disk only, where the whole file's threads show; nothing in a buffer never saved.
    const linesAndFile = { lines: true, file: true };
    const linesOnly = { lines: true, file: false };
    const where: [string, Record<string, string | boolean>, { lines: boolean; file: boolean }][] = [
      ['a file', { resourceScheme: 'file' }, linesAndFile],
      ["the working tree's side of its changes", diff('Right', revision, onDisk), linesAndFile],
      ["the index's side of the working tree's changes", diff('Left', revision, onDisk), linesOnly],
      ['the left side of two files compared', diff('Left', onDisk, onDisk), linesAndFile],
      ["the index's side of the staged changes", diff('Right', revision, revision), linesOnly],
      ["HEAD's side of the staged changes", diff('Left', revision, revision), linesOnly],
      ['an untitled buffer', { resourceScheme: 'untitled' }, { lines: false, file: false }],
    ];
    for (const [name, keys, anchorable] of where) {
      const selected = { ...keys, editorHasSelection: true };
      const expected = (items: string[]) => items.filter((c) => (c.endsWith('OnFile') ? anchorable.file : anchorable.lines));
      assert.deepStrictEqual(
        offered(context, selected),
        expected(['fwd.commentOnSelection', 'fwd.askOnSelection', 'fwd.commentOnFile']),
        `a selection in ${name}`,
      );
      // Adding the lines, or the whole file, as a step is an item of its own while a task is
      // current: a comment or a question made in the editor never makes a step (docs/DECISIONS.md,
      // D92).
      assert.deepStrictEqual(
        offered(context, { ...selected, 'fwd.hasCurrentTask': true }),
        expected(['fwd.commentOnSelection', 'fwd.askOnSelection', 'fwd.stepOnSelection', 'fwd.commentOnFile', 'fwd.stepOnFile']),
        `a selection in ${name}, a task current`,
      );
      assert.deepStrictEqual(offered(context, keys), expected(['fwd.commentOnFile']), `no selection in ${name}`);
      assert.deepStrictEqual(
        offered(context, { ...keys, 'fwd.hasCurrentTask': true }),
        expected(['fwd.commentOnFile', 'fwd.stepOnFile']),
        `no selection in ${name}, a task current`,
      );
    }
    // The editor's title bar has the whole-file items in its "..." menu, for a file's own editor.
    const title: MenuItem[] = menus['editor/title'];
    const fileEditor = { activeEditor: 'workbench.editors.files.textFileEditor', resourceScheme: 'file' };
    assert.deepStrictEqual(offered(title, fileEditor), ['fwd.commentOnFile']);
    assert.deepStrictEqual(offered(title, { ...fileEditor, 'fwd.hasCurrentTask': true }), ['fwd.commentOnFile', 'fwd.stepOnFile']);
    assert.deepStrictEqual(offered(title, { activeEditor: 'workbench.editors.textDiffEditor', resourceScheme: 'file', 'fwd.hasCurrentTask': true }), []);
    // The palette offers the lines' items on a revision's document too, as in the staged changes,
    // whose sides are both `git:` documents, and the whole file's items on the file on disk only.
    const anchoring = (menus.commandPalette as MenuItem[]).filter((item) => /^fwd\.(comment|ask|step)On/.test(item.command));
    const task = { editorHasSelection: true, 'fwd.hasCurrentTask': true };
    assert.deepStrictEqual(offered(anchoring, { ...task, resourceScheme: 'git' }), ['fwd.commentOnSelection', 'fwd.askOnSelection', 'fwd.stepOnSelection']);
    assert.deepStrictEqual(offered(anchoring, { ...task, resourceScheme: 'file' }), [
      'fwd.commentOnSelection',
      'fwd.askOnSelection',
      'fwd.stepOnSelection',
      'fwd.commentOnFile',
      'fwd.stepOnFile',
    ]);
    // The command palette lists only what says it is fwd's: a command titled for a button or a
    // row (Send to agents, Reply, Mark read) needs the thread or the inbox item it acts on.
    const palette = new Map<string, string>(menus.commandPalette.map((item: any) => [item.command, item.when]));
    for (const [command, name] of titles) {
      if (!name.startsWith('fwd: ')) assert.strictEqual(palette.get(command), 'false', `${command} ("${name}") is in the palette`);
    }
  });

  test("a new thread's box offers a comment and a question, on the task while one is current, and a thread's reply box offers Reply, its title bar the highlight's page, and Add as a Step then", () => {
    const { menus } = vscode.extensions.getExtension(EXTENSION)!.packageJSON.contributes;
    const box: MenuItem[] = menus['comments/commentThread/context'];
    const draft = { commentController: 'fwd', commentThreadIsEmpty: true };
    assert.deepStrictEqual(offered(box, draft), ['fwd.createComment', 'fwd.createQuestion']);
    // With a task current, the new highlight is the context of a comment or a question on that
    // task, and never a step (docs/DECISIONS.md, D92).
    assert.deepStrictEqual(offered(box, { ...draft, 'fwd.hasCurrentTask': true }), ['fwd.createTaskComment', 'fwd.createQuestion']);
    // A thread's reply box, which every thread carries: Reply alone, the button Ctrl+Enter presses;
    // a click elsewhere closes a box with nothing written in it.
    const thread = { commentController: 'fwd', commentThread: 'unresolved' };
    assert.deepStrictEqual(offered(box, thread), ['fwd.replyComment']);
    const titleBar: MenuItem[] = menus['comments/commentThread/title'];
    assert.deepStrictEqual(offered(titleBar, thread), ['fwd.resolveThread', 'fwd.openHighlightPage']);
    assert.deepStrictEqual(offered(titleBar, { ...thread, 'fwd.hasCurrentTask': true }), ['fwd.resolveThread', 'fwd.openHighlightPage', 'fwd.createStep']);
    assert.deepStrictEqual(offered(titleBar, { ...thread, commentThread: 'resolved' }), ['fwd.unresolveThread', 'fwd.openHighlightPage']);
    // The first thread of a highlight that is a step moves the step with it (docs/DECISIONS.md, D99),
    // and its actions say so.
    const stepThread = { commentController: 'fwd', commentThread: 'unresolvedStep' };
    assert.deepStrictEqual(offered(titleBar, stepThread), ['fwd.resolveThreadAndCheckStep', 'fwd.openHighlightPage']);
    assert.deepStrictEqual(offered(titleBar, { ...stepThread, 'fwd.hasCurrentTask': true }), ['fwd.resolveThreadAndCheckStep', 'fwd.openHighlightPage', 'fwd.createStep']);
    assert.deepStrictEqual(offered(titleBar, { ...stepThread, commentThread: 'resolvedStep' }), ['fwd.unresolveThreadAndUncheckStep', 'fwd.openHighlightPage']);
  });

  test('the inbox starts empty', async () => {
    assert.ok(await until(() => api.inbox.loaded), 'the inbox query answered');
    assert.deepStrictEqual(api.inbox.getChildren(), []);
  });

  test('the mention search in a comment box offers the principals whose name starts with what follows the at-sign', async () => {
    const shown = (items: vscode.CompletionItem[]) =>
      items.map((i) => {
        const label = i.label as vscode.CompletionItemLabel;
        return [label.label, label.detail, label.description, i.insertText];
      });
    // impl-alice's name is hers alone, so it shows bare and is written as it is.
    assert.deepStrictEqual(shown(await api.mentions('why, @impl')), [['@impl-alice', ' implementer', undefined, '@impl-alice ']]);
    assert.deepStrictEqual(
      shown(await api.mentions('@')).map((s) => s[0]),
      ['@impl-alice', '@tester', '@triage-bot'],
      'an at-sign alone offers everyone who belongs, by name'
    );
    assert.deepStrictEqual(await api.mentions('mail a@impl'), [], 'an at-sign inside a word opens no mention');
  });

  test('a mention by a second principal appears in the inbox without a refresh', async () => {
    const before = redraws;
    comment = JSON.parse(fwd('--format', 'json', '--as', 'impl-alice', 'comment', '--path', 'src/lib.rs', '--lines', '1-2', 'hello from the CLI, @tester')).record;
    const item = await until(() => api.inbox.getChildren().find((i) => i.n.type === 'mentioned'));
    assert.ok(item, 'a mentioned notification is in the tree');
    assert.ok(redraws > before, 'the inbox told the tree to redraw');
    // Stored as `[[<the person's id>]]`, read as the handle again.
    assert.strictEqual(item.n.message?.content, 'hello from the CLI, @tester');
    assert.strictEqual(icon(item), 'mail');
  });

  test('marking it read changes its icon', async () => {
    const item = api.inbox.getChildren().find((i) => i.n.type === 'mentioned')!;
    await vscode.commands.executeCommand('fwd.readNotification', item);
    const read = await until(() => api.inbox.getChildren().find((i) => i.n.id === item.n.id && icon(i) === 'mail-read'));
    assert.ok(read, `the icon is still ${api.inbox.getChildren().map(icon)}`);
    assert.strictEqual(read.n.status, 'read');
  });

  test("the comment's thread appears on the file when it is opened", async () => {
    await vscode.window.showTextDocument(await vscode.workspace.openTextDocument(file));
    const thread = await until(() => api.threads(file)[0]);
    assert.ok(thread, 'a thread is shown on src/lib.rs');
    assert.deepStrictEqual([thread.range?.start.line, thread.range?.end.line], [0, 1]);
    assert.deepStrictEqual(thread.comments.map(shown), ['impl-alice: hello from the CLI, @tester']);
  });

  test('a reply from the CLI joins the open thread', async () => {
    fwd('--format', 'json', '--as', 'impl-alice', 'reply', comment.id, 'and a reply from the CLI');
    const thread = await until(() => api.threads(file).find((t) => t.comments.length === 2));
    assert.ok(thread, `the thread holds ${api.threads(file).map((t) => t.comments.length)} comments`);
    assert.deepStrictEqual(thread.comments.map(shown), ['impl-alice: hello from the CLI, @tester', 'impl-alice: and a reply from the CLI']);
  });

  test('dismissing the notification removes it from the inbox', async () => {
    const item = api.inbox.getChildren().find((i) => i.n.type === 'mentioned')!;
    await vscode.commands.executeCommand('fwd.dismissNotification', item);
    assert.ok(await until(() => !api.inbox.getChildren().some((i) => i.n.id === item.n.id)), 'the notification left the tree');
  });

  test('a thread not resolved is a highlight: its lines are drawn, and the status bar counts it', async () => {
    // What the extension handed VS Code for the file's editor, with the decoration that paints the
    // lines' background, a gutter mark and a mark in the overview ruler.
    const drawn = await until(() => api.highlights(file)?.length && api.highlights(file));
    assert.ok(drawn, `drawn on the file: ${JSON.stringify(api.highlights(file))}`);
    assert.deepStrictEqual(lines(drawn), [[0, 1]]);
    assert.strictEqual(await until(() => api.count()), '$(comment-discussion) 1');
  });

  test('the cursor or a selection entering and leaving a highlight leaves its thread as it was, closed or open', async () => {
    // docs/DECISIONS.md, D98: VS Code's comment icon in the gutter opens and closes a thread, and
    // the cursor's moves never do.
    const editor = visible(file)!;
    const thread = api.threads(file)[0];
    const put = (line: number) => (editor.selection = new vscode.Selection(line, 0, line, 0));
    // Each check waits long enough for a thread to have opened on the cursor's entering, as one did
    // until D98 (a quarter of a second after the cursor stopped), and for VS Code to have answered
    // a change of a thread's open state.
    const pause = () => new Promise((resolve) => setTimeout(resolve, 600));
    put(2); // below the highlight on lines 0 and 1
    await pause();
    assert.strictEqual(thread.collapsibleState, Collapsed, 'closed while the cursor is outside');
    put(1);
    await pause();
    assert.strictEqual(thread.collapsibleState, Collapsed, 'still closed with the cursor in its lines');
    editor.selection = new vscode.Selection(0, 3, 2, 0);
    await pause();
    assert.strictEqual(thread.collapsibleState, Collapsed, 'still closed with its lines selected');
    thread.collapsibleState = Expanded; // as a click on the comment icon in the gutter does
    await pause();
    put(2);
    await pause();
    assert.strictEqual(thread.collapsibleState, Expanded, 'opened, it stays open when the cursor leaves');
    put(0);
    await pause();
    assert.strictEqual(thread.collapsibleState, Expanded, 'and when the cursor comes back');
    thread.collapsibleState = Collapsed;
    await pause();
  });

  test("a hover on a highlighted line shows the thread's first lines and a link that opens it", async () => {
    const text = await hoverText(file, new vscode.Position(1, 3));
    // The comment's words are escaped, to read as they were written.
    const words = text.replace(/\\(.)/g, '$1');
    assert.match(words, /\*\*impl-alice\*\*: hello from the CLI, @tester/);
    assert.match(text, /\[open\]\(command:fwd\.openThread\?/);
    assert.match(text, /1 reply/);
  });

  test('Resolve resolves the thread\'s first message and the highlight goes; Unresolve brings both back', async () => {
    const thread = api.threads(file)[0];
    const status = () => JSON.parse(fwd('--format', 'json', 'message', 'show', comment.id)).status;
    assert.strictEqual(thread.contextValue, 'unresolved', 'the thread offers Resolve on its title');
    await vscode.commands.executeCommand('fwd.resolveThread', thread);
    assert.strictEqual(status(), 'resolved');
    assert.ok(await until(() => thread.state === vscode.CommentThreadState.Resolved && api.highlights(file)?.length === 0), 'resolved, it is no highlight');
    assert.strictEqual(thread.contextValue, 'resolved', 'the thread offers Unresolve');
    // The thread's title bar says that it is resolved, once; its first comment says nothing of it.
    assert.strictEqual(thread.label, 'resolved');
    assert.deepStrictEqual(thread.comments.map((c) => c.label), [undefined, undefined]);
    assert.strictEqual(api.count(), undefined, 'no highlight to count');
    await vscode.commands.executeCommand('fwd.unresolveThread', thread);
    assert.ok(await until(() => thread.state === vscode.CommentThreadState.Unresolved && api.highlights(file)?.length === 1), 'unresolved, a highlight again');
    assert.strictEqual(status(), 'active');
    // A label with nothing to say is a space, since VS Code keeps a title bar's words when the
    // label is set to nothing.
    assert.strictEqual(thread.label, ' ');
  });

  test('an edit moves a highlight with its lines at once, to where the CLI finds them once saved, and both sides of a diff of the file show it where each has it', async () => {
    const editor = visible(file)!;
    await editor.edit((b) => b.insert(new vscode.Position(0, 0), 'fn zero() {}\n'));
    // Not saved yet, and with no wait: VS Code hands the extension the edit before it answers
    // the edit. The edit puts a line in right above the highlight on lines 0 and 1, which moves
    // the highlight down a line: it is drawn on lines 1 and 2, a hover on line 2 shows its
    // thread, and a hover on line 0, the line put in, shows none.
    const hovered = async (line: number) => (await hoverText(file, new vscode.Position(line, 0))).includes('command:fwd.openThread');
    assert.deepStrictEqual(lines(api.highlights(file)!), [[1, 2]], 'drawn a line down');
    assert.ok(await hovered(2), 'a hover on line 2, in the moved highlight, shows its thread');
    assert.ok(!(await hovered(0)), 'a hover on line 0, the line the edit put in, shows none');
    // Saved, the file on disk has the edit, and the CLI finds the anchor's lines where the edit
    // moved the highlight; the extension then puts the thread on them too.
    await editor.document.save();
    const anchors = JSON.parse(fwd('--format', 'json', 'anchor', 'list', '--path', 'src/lib.rs', '--against', 'worktree'));
    const found = anchors.find((a: any) => a.id === comment.anchor)?.resolved?.range;
    assert.deepStrictEqual(found, { start_line: 1, end_line: 3 }, 'the CLI finds lines 1 and 2');
    const thread = api.threads(file).find((t) => t.comments.some((c) => shown(c) === 'impl-alice: hello from the CLI, @tester'));
    assert.ok(await until(() => thread?.range?.start.line === 1), `the thread sits on ${JSON.stringify(lines([thread?.range]))}`);
    assert.deepStrictEqual(lines([thread?.range]), [[1, 2]]);
    assert.deepStrictEqual(lines(api.highlights(file)!), [[1, 2]], 'drawn there still');
    // The git extension serves the other sides of its diffs; it must have found the repository.
    const git = (await vscode.extensions.getExtension<any>('vscode.git')!.activate()).getAPI(1);
    assert.ok(await until(() => git.repositories.length > 0, 20_000), 'the git extension found the repository');
    for (const [ref, against] of [['~', 'the index'], ['HEAD', 'HEAD']]) {
      const base = gitUri(file, ref);
      await vscode.commands.executeCommand('vscode.diff', base, file, `lib.rs (${against})`);
      const left = await until(() => api.highlights(base)?.length && api.highlights(base), 15_000);
      assert.ok(left, `nothing drawn on ${against}'s side: ${JSON.stringify(api.highlights(base))}`);
      assert.deepStrictEqual(lines(left), [[0, 1]], `${against} has the lines where they were`);
      assert.deepStrictEqual(lines(api.threads(base).map((t) => t.range)), [[0, 1]], `the thread sits on them in ${against}`);
      assert.deepStrictEqual(lines(api.highlights(file)!), [[1, 2]], 'the working tree has them one line down');
      await vscode.commands.executeCommand('workbench.action.closeActiveEditor');
    }
  });

  test("the copy of the file the gutter's change markers compare against shows no threads, so VS Code's Comments panel lists each thread once", async () => {
    // The git extension opens that copy once an editor shows the file and it has found the
    // repository (the test above waited for it), naming it as the file with `.git` after its name
    // (src/sides.ts). The sides of a diff keep their threads, as the test above checks.
    await vscode.window.showTextDocument(file);
    const isCopy = (uri: vscode.Uri) => uri.scheme === 'git' && uri.path === `${file.path}.git`;
    const copy = await until(() => vscode.workspace.textDocuments.find((d) => isCopy(d.uri)), 20_000);
    const open = vscode.workspace.textDocuments.map((d) => d.uri.toString());
    assert.ok(copy, `VS Code opened no copy of src/lib.rs for the change markers: ${open.join(', ')}`);
    const shownOn = [...api.threadsByDocument().keys()];
    assert.ok(!shownOn.some((uri) => isCopy(vscode.Uri.parse(uri))), `threads are shown on ${shownOn.join(', ')}`);
    assert.ok(api.threads(file).length > 0, 'the file shows its threads');
  });

  test("a highlight on the whole file draws nothing in the text: its thread is on the file, and the file's name has a badge", async () => {
    const whole = JSON.parse(fwd('--format', 'json', '--as', 'impl-alice', 'comment', '--path', 'src/lib.rs', 'about the whole file')).record;
    const thread = await until(() => api.threads(file).find((t) => t.comments.some((c) => shown(c).endsWith('about the whole file'))));
    assert.ok(thread, 'a thread for it on the file');
    assert.strictEqual(thread.range, undefined, 'on the file, not on lines');
    const badge = await until(() => api.badge(file));
    assert.ok(badge, 'the file has a badge');
    assert.strictEqual(badge.badge, '◆');
    assert.match(badge.tooltip ?? '', /^1 comment thread on the whole file/);
    assert.deepStrictEqual(lines(api.highlights(file)!), [[1, 2]], 'no lines drawn for it');
    assert.ok(await until(() => api.count() === '$(comment-discussion) 2'), `the status bar says ${api.count()}`);
    fwd('message', 'resolve', whole.id);
    assert.ok(await until(() => !api.badge(file)), 'resolved, the badge goes');
  });

  test('every thread carries its reply box from the moment it is made, with nothing to show or hide it, and a reply sent from it joins the thread', async () => {
    const thread = api.threads(file).find((t) => t.comments.some((c) => shown(c) === 'impl-alice: about the whole file'));
    assert.ok(thread, 'the thread on the whole file');
    // VS Code makes a thread with `canReply` true and fwd never changes it: a change is what drew
    // the reply box twice, side by side (docs/design/vscode-extension.md, The thread box).
    assert.ok(api.threads(file).every((t) => t.canReply === true), 'every thread on the file carries its reply box');
    const commands = await vscode.commands.getCommands(true);
    assert.deepStrictEqual(['fwd.openReply', 'fwd.cancelReply'].filter((c) => commands.includes(c)), [], 'no command shows or hides it');
    // What VS Code hands the box's Reply: the thread, and what is written in the box.
    await vscode.commands.executeCommand('fwd.replyComment', { thread, text: 'the whole file, or its first half?' });
    const joined = await until(() => thread.comments.some((c) => shown(c) === 'tester: the whole file, or its first half?'));
    assert.ok(joined, `the thread holds ${JSON.stringify(thread.comments.map(shown))}`);
    assert.strictEqual(thread.canReply, true, 'the reply sent, the box is still there');
  });

  test("the comment items open VS Code's comment box on the selection's whole lines, on a revision's side too, or on the whole file on disk, saying where a comment goes", async () => {
    const execute = vscode.commands.executeCommand;
    const opened: unknown[][] = [];
    const worded: (string | undefined)[] = [];
    // VS Code's command that opens the box is recorded instead of run, with the wording the
    // controller carries meanwhile, which VS Code gives the box it builds: the box is not in the API.
    const recordBox = ((command: string, ...rest: unknown[]) =>
      command === 'workbench.action.addComment'
        ? (opened.push(rest), worded.push(api.commentOptions()?.placeHolder), Promise.resolve())
        : execute(command, ...rest)) as typeof execute;
    const editor = await vscode.window.showTextDocument(file, { preview: false });
    // From the middle of line 1 to the start of line 3, which the selection leaves out.
    editor.selection = new vscode.Selection(1, 3, 3, 0);
    await standingIn(vscode.commands, 'executeCommand', recordBox, async () => {
      // As the menu runs them, with the document it was opened on.
      await vscode.commands.executeCommand('fwd.commentOnSelection', file);
      await vscode.commands.executeCommand('fwd.commentOnFile', file);
    });
    assert.deepStrictEqual(opened, [[{ range: { startLineNumber: 2, endLineNumber: 3, endColumn: 1 } }], [{ fileComment: true }]]);
    const agents = 'For the agents working here: is this related to a task? what should they watch for?';
    assert.deepStrictEqual(worded, [agents, agents]);
    assert.strictEqual(api.commentOptions(), undefined, 'the wording goes once the box is built');
    // On the index's side of the working tree's changes, the lines' item opens the box on the
    // lines the index has there; the whole file's item says why it does nothing, since a thread on
    // the whole file shows on the file on disk.
    const base = gitUri(file, '~');
    await vscode.commands.executeCommand('vscode.diff', base, file, 'lib.rs (the index)');
    const left = await until(() => visible(base));
    assert.ok(left, "the diff shows the index's side");
    left.selection = new vscode.Selection(1, 0, 1, 2);
    const said: string[] = [];
    const recordError = ((message: string) => (said.push(message), Promise.resolve(undefined))) as typeof vscode.window.showErrorMessage;
    await standingIn(vscode.window, 'showErrorMessage', recordError, () =>
      standingIn(vscode.commands, 'executeCommand', recordBox, async () => {
        await vscode.commands.executeCommand('fwd.commentOnSelection', base);
        await vscode.commands.executeCommand('fwd.commentOnFile', base);
      }),
    );
    assert.deepStrictEqual(opened.slice(2), [[{ range: { startLineNumber: 2, endLineNumber: 2, endColumn: 1 } }]], "a box on the index's line");
    assert.deepStrictEqual(said, ['A comment or a step on the whole file is made on the file itself, where its threads show: use the working-tree side, or the file.']);
    // The index's side lacked the focus a right-click there gives it (the test cannot give it), so
    // the item opened the index in an editor of its own to put the box there, which closes with the
    // diff.
    const showsIndex = (t: vscode.Tab) =>
      (t.input instanceof vscode.TabInputText && t.input.uri.toString() === base.toString()) ||
      (t.input instanceof vscode.TabInputTextDiff && t.input.original.toString() === base.toString());
    await vscode.window.tabGroups.close(vscode.window.tabGroups.all.flatMap((g) => g.tabs).filter(showsIndex));
  });

  test("asking about lines posts the question on them: it is asked of no one in particular, the triage agent that answers for it is told, and the lines become a highlight whose thread it starts", async () => {
    const editor = await vscode.window.showTextDocument(file, { preview: false });
    editor.selection = new vscode.Selection(2, 0, 2, 6); // in `fn two() {}`, the third line since the edit above
    const question = 'why does two come after one?';
    const boxes: (vscode.InputBoxOptions | undefined)[] = [];
    const answer = (async (options?: vscode.InputBoxOptions) => (boxes.push(options), question)) as typeof vscode.window.showInputBox;
    await standingIn(vscode.window, 'showInputBox', answer, () => vscode.commands.executeCommand('fwd.askOnSelection', file));
    assert.deepStrictEqual(
      boxes.map((box) => [box?.title, box?.prompt?.split('. ')[0]]),
      [['Ask about src/lib.rs, lines 3-3', 'Asked of no one in particular, which the triage agent of its scope answers']],
      'saying whom it asks',
    );
    const anchors = JSON.parse(fwd('--format', 'json', 'anchor', 'list', '--path', 'src/lib.rs'));
    const on = anchors.find((a: any) => a.messages.some((m: any) => m.content === question));
    assert.ok(on, 'an anchor holds the question');
    assert.deepStrictEqual([on.start_line, on.end_line, on.anchored_text.trim()], [2, 3, 'fn two() {}'], 'the selected line');
    const asked = on.messages.find((m: any) => m.content === question);
    assert.deepStrictEqual(asked.context, { asks: 'project' }, 'asked of no one by name, and of no scope, the project having none');
    // Routing hears of the question because it is one transaction with its new highlight: posted
    // apart, it could be loaded by this extension's live query on the new highlight's messages before
    // the daemon's node published it, which then told no one (ops::post_on_highlight, task F-Z2QwJe).
    const inbox = () => JSON.parse(fwd('--format', 'json', '--as', 'triage-bot', 'inbox', '--unread'));
    assert.ok(await until(() => inbox().some((n: any) => n.type === 'question_asked' && n.target.id === asked.id)), 'the triage agent is asked it');
    const thread = await until(() => api.threads(file).find((t) => t.comments.some((c) => shown(c) === `tester: ${question}`)));
    assert.deepStrictEqual(lines([thread?.range]), [[2, 2]], 'its thread is on the line');
  });

  test("a comment sent from the whole file's box goes to the agents as a highlight on the whole file", async () => {
    let replaced = false;
    const draft = { uri: file, range: undefined, dispose: () => (replaced = true) };
    // What VS Code hands the box's button, Send to agents, once the whole-file item has opened it.
    await vscode.commands.executeCommand('fwd.createComment', { thread: draft, text: 'is this file getting long?' });
    assert.ok(replaced, "the draft gave way to the highlight's own thread");
    const thread = await until(() => api.threads(file).find((t) => t.comments.some((c) => shown(c) === 'tester: is this file getting long?')));
    assert.ok(thread, 'a thread for it on the file');
    assert.strictEqual(thread.range, undefined, 'on the whole file');
    assert.ok(await until(() => api.badge(file)), "the file's name has a badge");
    await vscode.commands.executeCommand('fwd.resolveThread', thread);
    assert.ok(await until(() => !api.badge(file)), 'resolved, the badge goes');
  });

  test("one's own comment offers VS Code's Edit, whose Save is fwd message edit: the text changes in its thread, the person it names newly hears of it; another's comment offers none", async () => {
    const { menus } = vscode.extensions.getExtension(EXTENSION)!.packageJSON.contributes;
    // Edit on the title bar of one's own comment only; Cancel and Save beneath a comment being edited.
    const titleBar: MenuItem[] = menus['comments/comment/title'];
    assert.deepStrictEqual(offered(titleBar, { commentController: 'fwd', comment: 'own' }), ['fwd.editComment']);
    assert.deepStrictEqual(offered(titleBar, { commentController: 'fwd' }), [], "another's comment offers no Edit");
    const beneath: MenuItem[] = menus['comments/comment/context'];
    assert.deepStrictEqual(offered(beneath, { commentController: 'fwd', comment: 'own' }), ['fwd.cancelEditComment', 'fwd.saveComment']);

    // The question tester asked about line 3 is tester's; impl-alice's comments are not.
    const question = 'why does two come after one?';
    const mine = api.threads(file).find((t) => t.comments.some((c) => shown(c) === `tester: ${question}`));
    const theirs = api.threads(file).find((t) => t.comments.some((c) => shown(c) === 'impl-alice: hello from the CLI, @tester'));
    assert.ok(mine && theirs, 'both threads are on the file');
    assert.strictEqual(mine.comments[0].contextValue, 'own', 'the question offers Edit');
    assert.ok(theirs.comments.every((c) => c.contextValue !== 'own'), "impl-alice's comments offer none");
    const anchors = JSON.parse(fwd('--format', 'json', 'anchor', 'list', '--path', 'src/lib.rs'));
    const asked = anchors.flatMap((a: any) => a.messages).find((m: any) => m.content === question);
    const editing = () => mine.comments.find((c) => c.mode === vscode.CommentMode.Editing);

    // Edit opens the comment in VS Code's editor, holding its text as its author typed it.
    await vscode.commands.executeCommand('fwd.editComment', mine.comments[0]);
    assert.strictEqual((await until(editing))?.body, question);

    // A name that fits nothing is refused in the CLI's words, and the comment stays in its editor,
    // holding what was written, to correct. VS Code hands Save the comment with the editor's text
    // as its body.
    const said: string[] = [];
    const recordError = ((message: string) => (said.push(message), Promise.resolve(undefined))) as typeof vscode.window.showErrorMessage;
    const typo = 'why does two come after one, @impl-alicee?';
    await standingIn(vscode.window, 'showErrorMessage', recordError, () => {
      editing()!.body = typo;
      return vscode.commands.executeCommand('fwd.saveComment', editing());
    });
    assert.strictEqual(said.length, 1, `said ${JSON.stringify(said)}`);
    assert.match(said[0], /`@impl-alicee` \(no one has the handle `impl-alicee`, and no id starts with it\) names nothing/);
    assert.strictEqual(editing()?.body, typo, 'still in its editor, with what was written');
    assert.strictEqual(JSON.parse(fwd('--format', 'json', 'message', 'show', asked.id)).edited, null, 'nothing was written');

    // Save: the question now names impl-alice, in the same thread and the same message, marked
    // edited; impl-alice, named newly, hears of it.
    const fixed = 'why does two come after one, @impl-alice?';
    editing()!.body = fixed;
    await vscode.commands.executeCommand('fwd.saveComment', editing());
    const saved = await until(() => mine.comments.find((c) => c.mode === vscode.CommentMode.Preview && shown(c) === `tester: ${fixed}`));
    assert.ok(saved, `the thread shows ${JSON.stringify(mine.comments.map((c) => [c.mode, c.body]))}`);
    assert.strictEqual(saved.label, 'edited');
    const message = JSON.parse(fwd('--format', 'json', 'message', 'show', asked.id));
    assert.strictEqual(typeof message.edited, 'number', 'the message says when it was edited');
    assert.deepStrictEqual(message.references.map((r: any) => r.name), ['@impl-alice']);
    const told = () => JSON.parse(fwd('--format', 'json', '--as', 'impl-alice', 'inbox', '--unread'));
    assert.ok(await until(() => told().some((n: any) => n.type === 'mentioned' && n.target.id === asked.id)), 'impl-alice is told she is named');

    // Cancel closes the editor and leaves the text as saved.
    await vscode.commands.executeCommand('fwd.editComment', saved);
    assert.strictEqual((await until(editing))?.body, fixed, 'the editor opens on the saved text');
    editing()!.body = 'something else';
    await vscode.commands.executeCommand('fwd.cancelEditComment', editing());
    assert.ok(await until(() => !editing() && shown(mine.comments[0]) === `tester: ${fixed}`), 'the text is as saved');
  });

  test('the graph command opens the bundled page in a webview, pinned to the project', async () => {
    daemon = JSON.parse(fwd('daemon', 'status'));
    await vscode.commands.executeCommand('fwd.showGraph');
    opened = Date.now();
    const page = api.page();
    assert.ok(page, 'the page panel is open');
    const policy = page.html.match(/http-equiv="Content-Security-Policy" content="([^"]*)"/)?.[1] ?? '';
    const directives = new Map(policy.split(';').map((d) => d.trim().split(/\s+/)).map(([name, ...sources]) => [name, sources]));
    assert.deepStrictEqual(directives.get('default-src'), ["'none'"], policy);
    assert.ok(directives.get('script-src')?.includes("'wasm-unsafe-eval'"), policy);
    const address = `http://127.0.0.1:${daemon.port}`;
    for (const source of [address, `ws://127.0.0.1:${daemon.port}`]) {
      assert.ok(directives.get('connect-src')?.includes(source), `${source} is not in ${policy}`);
    }
    // Every file the page loads is the build's copy of the page in dist/web, through a webview URI.
    const web = path.join(vscode.extensions.getExtension(EXTENSION)!.extensionPath, 'dist', 'web');
    const loaded = [...page.html.matchAll(/"(https:\/\/[^"]+)"/g)].map((m) => vscode.Uri.parse(m[1]).path);
    assert.deepStrictEqual(loaded.map((file) => path.basename(file)).sort(), ['app.css', 'fwd-web.js', 'fwd-web_bg.wasm']);
    for (const file of loaded) {
      assert.strictEqual(path.dirname(file), web);
      assert.ok(fs.existsSync(file), `${file} was copied`);
    }
    const params = JSON.stringify({ daemon: address, p: daemon.project, lock: '1' });
    // The third argument is the document's own id, by which a document loaded again after hiding
    // tells itself from a new one.
    const call = page.html.match(/bridge\((.*), "#\/graph", "[0-9a-f]{16}"\);/);
    assert.strictEqual(call?.[1], params, 'the bridge adds the daemon, the project, lock=1 and the route');
  });

  test('the page boots in the webview within ten seconds: titled fwd, showing the project, connected', async () => {
    const report = await untilPage(api.page()!, booted, opened + 10_000);
    const summary = report && { ...report, text: `${report.text.slice(0, 120)}…` };
    console.log(`    the page's report ${Date.now() - opened} ms after the command: ${JSON.stringify(summary)}`);
    assert.ok(report && booted(report), `the page shows ${JSON.stringify(report)}`);
    assert.strictEqual(report.title, 'fwd');
    assert.ok(onGraph(report.hash), report.hash);
    // The policy let in everything the page needed, and the page's stylesheet applies.
    assert.deepStrictEqual(report.refused, []);
    assert.deepStrictEqual(report.styles.map((href) => path.basename(vscode.Uri.parse(href).path)), ['app.css']);
    const params = new URLSearchParams(report.search);
    assert.deepStrictEqual(
      [params.get('daemon'), params.get('p'), params.get('lock')],
      [`http://127.0.0.1:${daemon.port}`, daemon.project, '1'],
    );
  });

  test("the daemon admits the page's origin, which the extension registered, and no other webview's", async () => {
    const report = await untilPage(api.page()!, booted, Date.now() + 10_000);
    assert.ok(report, 'the page answers');
    assert.match(report.origin, /^vscode-webview:\/\/[A-Za-z0-9._-]+$/);
    for (const route of ['/health', '/projects']) {
      assert.deepStrictEqual(await ask(daemon.port, 'GET', route, report.origin), { status: 200, allowed: report.origin }, route);
      assert.strictEqual((await ask(daemon.port, 'GET', route, 'vscode-webview://another-extension')).status, 403, route);
    }
    // No page calls the control routes, the page's own origin included, and the daemon still runs.
    for (const [method, route] of [['POST', '/stop'], ['POST', '/projects'], ['GET', `/p/${daemon.project}/checkouts`]]) {
      assert.strictEqual((await ask(daemon.port, method, route, report.origin)).status, 403, `${method} ${route}`);
    }
    assert.strictEqual((await ask(daemon.port, 'GET', '/health')).status, 200, 'the daemon still runs');
  });

  test('a task created from the CLI appears on the open page without a reload', async () => {
    fwd('task', 'create', 'Seen through the webview', '--why', 'the headless test');
    const report = await untilPage(api.page()!, (r) => r.text.includes('Seen through the webview'), Date.now() + 10_000);
    assert.ok(report?.text.includes('Seen through the webview'), `the page shows ${JSON.stringify(report)}`);
  });

  test("an agent that renames itself from the CLI shows its new name in the open page's pickers without a reload", async () => {
    fwd('--as', 'reviewer-before', 'principal', 'add', 'reviewer-before', '--role', 'reviewer');
    const listed = await untilPage(api.page()!, (r) => r.text.includes('reviewer-before'), Date.now() + 10_000);
    assert.ok(listed?.text.includes('reviewer-before'), `the header's picker lists the agent: ${JSON.stringify(listed)}`);
    fwd('--as', 'reviewer-before', 'principal', 'rename', 'reviewer-after');
    const renamed = (r: PageReport) => r.text.includes('reviewer-after') && !r.text.includes('reviewer-before');
    const report = await untilPage(api.page()!, renamed, Date.now() + 10_000);
    assert.ok(report && renamed(report), `the page shows ${JSON.stringify(report)}`);
  });

  test('the inbox command moves the same page to its inbox', async () => {
    const before = api.page()!.html;
    await vscode.commands.executeCommand('fwd.showInbox');
    assert.strictEqual(api.page()?.html, before, 'the same panel, with the same document');
    const report = await untilPage(api.page()!, (r) => r.hash === '#/inbox' && booted(r), Date.now() + 10_000);
    assert.ok(report && report.hash === '#/inbox' && booted(report), `the page shows ${JSON.stringify(report)}`);
    assert.deepStrictEqual(report.refused, [], 'nothing the page did on either route was refused');
  });

  test("the task the page shows becomes the window's current task, named in the status bar and the comment box", async () => {
    review = JSON.parse(fwd('--format', 'json', 'task', 'create', 'Review the lib', '--why', 'the headless test')).record.id;
    assert.strictEqual(api.current().id, undefined, 'no task is current yet');
    assert.match(api.current().placeHolder ?? '', /^For the agents working here/);
    api.page()!.go(`#/tasks/${review}`);
    const now = await until(() => api.current().title === 'Review the lib' && api.current());
    assert.ok(now, `the current task: ${JSON.stringify(api.current())}`);
    assert.strictEqual(now.id, review);
    assert.strictEqual(now.status, '$(tasklist) Review the lib');
    assert.match(now.placeHolder ?? '', /^A comment or a question on Review the lib/);
  });

  test("with a task current, the comment box fwd opens says it is a comment or a question on the task, and a thread's reply box says Reply", async () => {
    const execute = vscode.commands.executeCommand;
    const worded: (vscode.CommentOptions | undefined)[] = [];
    // VS Code's command that opens the box is recorded instead of run, with the wording the
    // controller carries meanwhile, which VS Code gives the box it builds.
    const recordBox = ((command: string, ...rest: unknown[]) =>
      command === 'workbench.action.addComment' ? (worded.push(api.commentOptions()), Promise.resolve()) : execute(command, ...rest)) as typeof execute;
    // In the first group: opened in the page's, the file would hide the page, which VS Code drops.
    const editor = await vscode.window.showTextDocument(file, { viewColumn: vscode.ViewColumn.One, preview: false });
    editor.selection = new vscode.Selection(2, 0, 2, 3);
    await standingIn(vscode.commands, 'executeCommand', recordBox, () => vscode.commands.executeCommand('fwd.commentOnSelection', file));
    assert.deepStrictEqual(worded, [{ prompt: 'A comment or a question on Review the lib', placeHolder: 'A comment or a question on Review the lib, about this code' }]);
    // At every other time the controller carries no wording, so VS Code gives the reply box of each
    // thread it shows its own: "Reply...", closed and open (its rule for a thread that has comments).
    assert.strictEqual(api.commentOptions(), undefined);
  });

  test('a comment or a question made in the editor while a task is current is on the task, with its new highlight as the context, and makes no step', async () => {
    let replaced = 0;
    const draft = () => ({ uri: file, range: new vscode.Range(0, 0, 0, 0), dispose: () => replaced++ });
    // What VS Code hands the box's buttons while a task is current: Comment on the task, and Ask.
    await vscode.commands.executeCommand('fwd.createTaskComment', { thread: draft(), text: 'is zero needed?' });
    await vscode.commands.executeCommand('fwd.createQuestion', { thread: draft(), text: 'who calls zero?' });
    assert.strictEqual(replaced, 2, "each draft gave way to the highlight's own thread");
    const anchors = JSON.parse(fwd('--format', 'json', 'anchor', 'list', '--path', 'src/lib.rs'));
    const on = anchors.find((a: any) => a.messages.some((m: any) => m.content === 'is zero needed?'));
    assert.deepStrictEqual([on?.start_line, on?.end_line, on?.anchored_text.trim()], [0, 1, 'fn zero() {}'], 'one highlight, on the line');
    const posted = on.messages.map((m: any) => [m.content, m.task, m.context]);
    assert.deepStrictEqual(posted.sort(), [
      ['is zero needed?', review, {}],
      ['who calls zero?', review, { asks: 'project' }],
    ]);
    assert.deepStrictEqual(JSON.parse(fwd('--format', 'json', 'step', 'list', review)), [], 'neither made a step');
    assert.ok(await until(() => api.threads(file).filter((t) => lines([t.range])[0][0] === 0).length === 2), 'two threads on the line');
  });

  test("Add as a Step makes a step of the current task: from the right-click menu on lines, and on a highlight's thread", async () => {
    // In the first group: opened in the page's, the file would hide the page, which VS Code drops.
    const editor = await vscode.window.showTextDocument(file, { viewColumn: vscode.ViewColumn.One, preview: false });
    editor.selection = new vscode.Selection(3, 0, 3, 4); // in `fn three() {}`
    const boxes: (vscode.InputBoxOptions | undefined)[] = [];
    const answer = (async (options?: vscode.InputBoxOptions) => (boxes.push(options), 'look at three')) as typeof vscode.window.showInputBox;
    await standingIn(vscode.window, 'showInputBox', answer, () => vscode.commands.executeCommand('fwd.stepOnSelection', file));
    assert.deepStrictEqual(boxes.map((box) => box?.title), ['A step of Review the lib: src/lib.rs, lines 4-4']);
    const steps = () => JSON.parse(fwd('--format', 'json', 'step', 'list', review)).map((s: any) => [s.kind, s.path, s.lines, s.excerpt]);
    assert.deepStrictEqual(steps(), [['anchor', 'src/lib.rs', '4-4', 'fn three() {}']]);
    const thread = await until(() => api.threads(file).find((t) => t.comments.some((c) => shown(c).endsWith('look at three'))));
    assert.deepStrictEqual(lines([thread?.range]), [[3, 3]], "the step's first comment is a thread on those lines");
    // The thread title bar's Add as a Step, on the comment on the task above, adds its highlight.
    const zero = api.threads(file).find((t) => t.comments.some((c) => shown(c).endsWith('is zero needed?')));
    await vscode.commands.executeCommand('fwd.createStep', zero);
    assert.deepStrictEqual(steps(), [
      ['anchor', 'src/lib.rs', '4-4', 'fn three() {}'],
      ['anchor', 'src/lib.rs', '1-1', 'fn zero() {}'],
    ]);
  });

  test("the whole file's step item makes the file a step, then posts its first comment, which reaches the task's assignee", async () => {
    fwd('task', 'assign', review, '@impl-alice');
    const boxes: (vscode.InputBoxOptions | undefined)[] = [];
    const first = 'read the whole file last';
    const answer = (async (options?: vscode.InputBoxOptions) => (boxes.push(options), first)) as typeof vscode.window.showInputBox;
    await standingIn(vscode.window, 'showInputBox', answer, () => vscode.commands.executeCommand('fwd.stepOnFile', file));
    assert.deepStrictEqual(boxes.map((box) => box?.title), ['A step of Review the lib: src/lib.rs, the whole file']);
    const last = JSON.parse(fwd('--format', 'json', 'step', 'list', review)).pop();
    assert.deepStrictEqual([last?.kind, last?.path, last?.whole_file, last?.lines], ['anchor', 'src/lib.rs', true, null], 'the next step is the whole file');
    // fwd made the step first, so routing found it when the comment came: the assignee hears of
    // both in one notification, whose message is the comment.
    const told = () => JSON.parse(fwd('--format', 'json', '--as', 'impl-alice', 'inbox', '--unread')) as any[];
    const heard = await until(() => told().find((n) => n.reason === 'tester added a step and commented on your task Review the lib'));
    assert.ok(heard, `impl-alice's inbox says ${JSON.stringify(told().map((n) => n.reason))}`);
    assert.strictEqual(heard.message?.content, first);
    const thread = await until(() => api.threads(file).find((t) => t.comments.some((c) => shown(c) === `tester: ${first}`)));
    assert.ok(thread, "the step's first comment is a thread");
    assert.strictEqual(thread.range, undefined, 'on the whole file');
    fwd('task', 'unassign', review);
  });

  test("a step's first thread resolves and checks the step, saying so; checking the step resolves that thread; a later thread's Resolve is plain", async () => {
    const { commands, menus } = vscode.extensions.getExtension(EXTENSION)!.packageJSON.contributes;
    const titles = new Map<string, string>(commands.map((c: any) => [c.command, c.title]));
    const titleBar: MenuItem[] = menus['comments/commentThread/title'];
    /** The titles of the actions a thread's title bar offers, no task current. */
    const actions = (t: vscode.CommentThread) => offered(titleBar, { commentController: 'fwd', commentThread: t.contextValue ?? '' }).map((c) => titles.get(c));
    const listed = () => JSON.parse(fwd('--format', 'json', 'step', 'list', review)) as any[];
    const stepOf = (excerpt: string) => listed().find((s) => s.excerpt === excerpt);
    const facts = (excerpt: string) => [stepOf(excerpt).checked, stepOf(excerpt).thread?.status];
    const { Resolved, Unresolved } = vscode.CommentThreadState;
    const page = api.page()!;
    const onThree = () => api.threads(file).find((t) => t.comments.some((c) => shown(c).endsWith('look at three')));
    // The step's first thread says what its Resolve does, and its label that the step is not checked.
    const three = await until(() => onThree()?.contextValue === 'unresolvedStep' && onThree());
    assert.ok(three, `the step's thread offers ${onThree()?.contextValue}`);
    assert.deepStrictEqual(actions(three), ['fwd: Resolve and Check the Step', 'fwd: Open the Highlight on the Page']);
    assert.strictEqual(three.label, 'Step of “Review the lib”: not checked');
    await vscode.commands.executeCommand('fwd.resolveThreadAndCheckStep', three);
    assert.deepStrictEqual(facts('fn three() {}'), [true, 'resolved'], 'fwd step list: the step is checked, its thread resolved');
    assert.ok(await until(() => three.state === Resolved && three.contextValue === 'resolvedStep'), `the thread is ${three.state}, ${three.contextValue}`);
    assert.ok(await until(() => three.label === 'Step of “Review the lib”: checked'), `the label says ${three.label}`);
    assert.deepStrictEqual(actions(three), ['fwd: Unresolve and Uncheck the Step', 'fwd: Open the Highlight on the Page']);
    // The page's step line shows the thread's state beside the box.
    const resolvedThere = await untilPage(page, (r) => r.text.includes('thread resolved'), Date.now() + 10_000);
    assert.ok(resolvedThere?.text.includes('thread resolved'), `the page shows ${JSON.stringify(resolvedThere?.text.slice(0, 400))}`);
    await vscode.commands.executeCommand('fwd.unresolveThreadAndUncheckStep', three);
    assert.deepStrictEqual(facts('fn three() {}'), [false, 'active'], 'unchecked, the thread active again');
    assert.ok(await until(() => three.state === Unresolved && three.label === 'Step of “Review the lib”: not checked'), `the label says ${three.label}`);
    // Checking the step from the CLI resolves its thread in the editor.
    fwd('step', 'check', review, stepOf('fn three() {}').edge);
    assert.ok(await until(() => three.state === Resolved && three.contextValue === 'resolvedStep'), 'fwd step check resolved the thread');
    // Unchecking it with the page's box, the first step's, sets the thread back to active.
    assert.ok(await untilPage(page, (r) => r.text.includes('thread resolved'), Date.now() + 10_000), 'the page saw the check');
    await page.probe(1000, { type: 'click', selector: '.step input[type=checkbox]' });
    assert.ok(await until(() => three.state === Unresolved), `the page's box left the thread ${three.state}`);
    assert.deepStrictEqual(facts('fn three() {}'), [false, 'active']);
    // A later thread on a step's highlight has the plain Resolve, which leaves the step and the
    // first thread as they were.
    const later = api.threads(file).find((t) => t.comments.some((c) => shown(c).endsWith('who calls zero?')));
    assert.ok(later, 'the question is a second thread on the highlight that became a step');
    assert.strictEqual(later.contextValue, 'unresolved');
    assert.deepStrictEqual(actions(later), ['fwd: Resolve', 'fwd: Open the Highlight on the Page']);
    await vscode.commands.executeCommand('fwd.resolveThread', later);
    assert.ok(await until(() => later.state === Resolved), 'the later thread is resolved');
    assert.deepStrictEqual(facts('fn zero() {}'), [false, 'active'], 'its step and first thread are as they were');
    await vscode.commands.executeCommand('fwd.unresolveThread', later);
    assert.ok(await until(() => later.state === Unresolved), 'and open again');
  });

  test('a step clicked on the page opens the file at its lines when they are as committed, and the page stays', async () => {
    const page = api.page()!;
    const shown = await untilPage(page, (r) => r.text.includes('fn three() {}'), Date.now() + 10_000);
    assert.ok(shown?.text.includes('fn three() {}'), `the page does not show the step: ${JSON.stringify(shown?.text.slice(0, 400))}`);
    // A click on the step's line, which the page answers with its fwd:reveal.
    await page.probe(1000, { type: 'click', selector: '.step-body' });
    const editor = await until(() => {
      const e = vscode.window.activeTextEditor;
      return activeTab() instanceof vscode.TabInputText && e?.document.uri.fsPath === file.fsPath && e.selection.active.line === 3 && e;
    });
    assert.ok(editor, `the active tab is ${JSON.stringify(activeTab())}, the cursor on line ${vscode.window.activeTextEditor?.selection.active.line}`);
    const stays = (await page.probe(1000))?.hash;
    assert.ok(onTaskPage(stays, review), `the page stays on the task: ${stays}`);
    // The cursor at the lines leaves the step's thread closed, as it was (docs/DECISIONS.md, D98).
    const step = api.threads(file).find((t) => t.comments.some((c) => textOf(c) === 'look at three'));
    await new Promise((resolve) => setTimeout(resolve, 600));
    assert.strictEqual(step?.collapsibleState, Collapsed, "the step's thread stays closed");
  });

  test("a highlight's link on the page opens it in the editor, and the page does not follow the link", async () => {
    const page = api.page()!;
    const editor = await vscode.window.showTextDocument(file, { viewColumn: vscode.ViewColumn.One, preview: false });
    editor.selection = new vscode.Selection(0, 0, 0, 0);
    // The step opened in place shows its highlight's row, whose caption links to the highlight's page.
    await page.probe(1000, { type: 'click', selector: '.step .caret' });
    let hash: string | undefined;
    for (const deadline = Date.now() + 10_000; Date.now() < deadline; await new Promise((resolve) => setTimeout(resolve, 300))) {
      hash = (await page.probe(1000, { type: 'click', selector: '.step-embedded a.anchor-caption' }))?.hash;
      if (vscode.window.activeTextEditor?.selection.active.line === 3) break;
    }
    const e = vscode.window.activeTextEditor;
    assert.ok(e?.document.uri.fsPath === file.fsPath && e.selection.active.line === 3, `the editor did not go to the highlight: ${e?.document.uri.fsPath}:${e?.selection.active.line}`);
    assert.ok(onTaskPage(hash, review), `the page stays on the task: ${hash}`);
  });

  test("a highlight in a change not staged opens the file at its lines, not the working tree's changes", async () => {
    // Lines 1 and 2: the line the edit above added and did not stage, and one as committed.
    const made = JSON.parse(fwd('--format', 'json', '--as', 'impl-alice', 'comment', '--path', 'src/lib.rs', '--lines', '1-2', 'is this new?')).record;
    await revealsAt(made.anchor, file, 0);
  });

  test('a highlight in a staged change opens the file, with the cursor where the working tree has its lines', async () => {
    // A file committed, then an edit to its second line staged, then a line added above and not
    // staged: the edited line is the working tree's third and the index's second, so the cursor
    // lands on the working tree's line (VS Code's line 2, counted from 0), not the index's (1).
    const staged = vscode.Uri.file(path.join(root, 'src', 'staged.rs'));
    const git = (...args: string[]) => execFileSync('git', ['-c', 'commit.gpgsign=false', ...args], { cwd: root, stdio: 'ignore' });
    fs.writeFileSync(staged.fsPath, 'fn a() {}\nfn b() {}\nfn c() {}\n');
    git('add', 'src/staged.rs');
    git('commit', '-q', '-m', 'staged.rs');
    fs.writeFileSync(staged.fsPath, 'fn a() {}\nfn b2() {}\nfn c() {}\n');
    git('add', 'src/staged.rs');
    fs.writeFileSync(staged.fsPath, 'fn top() {}\nfn a() {}\nfn b2() {}\nfn c() {}\n');
    const made = JSON.parse(fwd('--format', 'json', '--as', 'impl-alice', 'comment', '--path', 'src/staged.rs', '--lines', '3-3', 'is this staged?')).record;
    await revealsAt(made.anchor, staged, 2);
    await vscode.commands.executeCommand('workbench.action.closeActiveEditor');
  });

  test("a comment made on the index's side of the working tree's changes is a highlight there: its thread shows there, and the page opens it in the file at the working tree's line", async () => {
    // src/staged.rs as the test above left it: `fn b2() {}` staged in place of `fn b() {}`, the
    // index's second line, and `fn top() {}` added above everything and not staged, so that the
    // working tree has that line third.
    const staged = vscode.Uri.file(path.join(root, 'src', 'staged.rs'));
    const index = gitUri(staged, '~');
    await vscode.commands.executeCommand('vscode.diff', index, staged, 'staged.rs (Working Tree)');
    assert.ok(await until(() => visible(index)), "the diff shows the index's side");
    // What VS Code hands the box's Comment on the task (the review is current), once a box opened
    // on the index's second line.
    let replaced = false;
    const draft = { uri: index, range: new vscode.Range(1, 0, 1, 0), dispose: () => (replaced = true) };
    await vscode.commands.executeCommand('fwd.createTaskComment', { thread: draft, text: 'why b2?' });
    assert.ok(replaced, "the draft gave way to the highlight's own thread");
    const anchors = JSON.parse(fwd('--format', 'json', 'anchor', 'list', '--path', 'src/staged.rs', '--against', 'index'));
    const made = anchors.find((a: any) => a.messages.some((m: any) => m.content === 'why b2?'));
    assert.ok(made, 'an anchor holds the comment');
    // The index's line, made on the diff's left, with the index's content for its basis.
    assert.deepStrictEqual([made.side, made.start_line, made.end_line, made.anchored_text], ['base', 1, 2, 'fn b2() {}']);
    assert.strictEqual(made.basis_blob, made.index_blob, "the index's blob is its basis");
    // Its thread and its highlight are on the index's side, at the index's line, and on the
    // working tree's side a line down.
    const on = (uri: vscode.Uri) => api.threads(uri).find((t) => t.comments.some((c) => shown(c).endsWith('why b2?')));
    assert.deepStrictEqual(lines([(await until(() => on(index)))?.range]), [[1, 1]], "its thread is on the index's line");
    const drawn = await until(() => api.highlights(index)?.some((r) => r.start.line === 1));
    assert.ok(drawn, `drawn on the index's side: ${JSON.stringify(api.highlights(index))}`);
    assert.deepStrictEqual(lines([(await until(() => on(staged)))?.range]), [[2, 2]], "and on the working tree's side, a line down");
    // The page opens it, as it opens any highlight, in the file at the lines the working tree has:
    // `fn b2() {}` is the working tree's third line, a line below where it was made.
    await vscode.commands.executeCommand('workbench.action.closeActiveEditor');
    await revealsAt(made.id, staged, 2);
    await vscode.commands.executeCommand('workbench.action.closeActiveEditor');
  });

  test("a comment on the index's side of the staged changes and a question on HEAD's side are highlights there, which the page opens in the file at the working tree's lines", async () => {
    const staged = vscode.Uri.file(path.join(root, 'src', 'staged.rs'));
    const [head, index] = [gitUri(staged, 'HEAD'), gitUri(staged, '')];
    await vscode.commands.executeCommand('vscode.diff', head, index, 'staged.rs (Index)');
    assert.ok(await until(() => visible(head) && visible(index)), 'the diff shows both sides');
    // The index's third line, `fn c() {}`, on the right; HEAD's second, `fn b() {}`, which the
    // staged change replaced, on the left. The review is current.
    const draft = (uri: vscode.Uri, line: number) => ({ uri, range: new vscode.Range(line, 0, line, 0), dispose: () => undefined });
    await vscode.commands.executeCommand('fwd.createTaskComment', { thread: draft(index, 2), text: 'is c staged?' });
    await vscode.commands.executeCommand('fwd.createQuestion', { thread: draft(head, 1), text: 'why was b renamed?' });
    const anchors = JSON.parse(fwd('--format', 'json', 'anchor', 'list', '--path', 'src/staged.rs'));
    const holding = (text: string) => anchors.find((a: any) => a.messages.some((m: any) => m.content === text));
    const [onIndex, onHead] = [holding('is c staged?'), holding('why was b renamed?')];
    assert.deepStrictEqual([onIndex?.side, onIndex?.anchored_text, onIndex?.basis_blob === onIndex?.index_blob], ['modified', 'fn c() {}', true]);
    assert.deepStrictEqual([onHead?.side, onHead?.anchored_text, onHead?.basis_blob === onHead?.head_blob], ['base', 'fn b() {}', true]);
    const on = (uri: vscode.Uri, text: string) => api.threads(uri).find((t) => t.comments.some((c) => shown(c).endsWith(text)));
    assert.deepStrictEqual(lines([(await until(() => on(index, 'is c staged?')))?.range]), [[2, 2]], "the comment's thread is on the index's line");
    assert.deepStrictEqual(lines([(await until(() => on(head, 'why was b renamed?')))?.range]), [[1, 1]], "the question's is on HEAD's");
    await vscode.commands.executeCommand('workbench.action.closeActiveEditor');
    // The page opens each in the file, with the cursor at the line the working tree has: `fn c()
    // {}` its fourth, and HEAD's `fn b() {}` its third, where the staged change made it `fn b2()
    // {}` (the CLI finds it there with its text changed, `recovered`).
    for (const [anchor, at] of [[onIndex.id, 3], [onHead.id, 2]] as const) {
      await revealsAt(anchor, staged, at);
      await vscode.commands.executeCommand('workbench.action.closeActiveEditor');
    }
  });

  test('a highlight whose lines are gone from the working tree opens its file, saying they are gone; one whose file is gone says that', async () => {
    // A line with nothing like it left once it is deleted, so that no search finds it again.
    const gone = vscode.Uri.file(path.join(root, 'src', 'gone.rs'));
    fs.writeFileSync(gone.fsPath, 'const KEEP: u8 = 1;\nfn doomed_and_deleted_soon(x: u32) -> u32 { x * 2 }\nconst AFTER: u8 = 2;\n');
    const made = JSON.parse(fwd('--format', 'json', '--as', 'impl-alice', 'comment', '--path', 'src/gone.rs', '--lines', '2-2', 'soon gone')).record;
    fs.writeFileSync(gone.fsPath, 'const KEEP: u8 = 1;\nconst AFTER: u8 = 2;\n');
    /** What the extension says while the page asks for the highlight to be shown. */
    const saidOnReveal = async () => {
      const said: string[] = [];
      const record = ((message: string) => (said.push(message), Promise.resolve(undefined))) as typeof vscode.window.showInformationMessage;
      await standingIn(vscode.window, 'showInformationMessage', record, async () => {
        await api.page()!.probe(1000, { type: 'fwd:reveal', detail: made.anchor });
        await until(() => said.length);
      });
      return said;
    };
    assert.deepStrictEqual(await saidOnReveal(), ['The lines of this highlight are no longer in the working tree; its threads are on the whole file.']);
    const e = vscode.window.activeTextEditor;
    assert.ok(activeTab() instanceof vscode.TabInputText && e?.document.uri.toString() === gone.toString(), `the active tab is ${JSON.stringify(activeTab())}`);
    await vscode.commands.executeCommand('workbench.action.closeActiveEditor');
    fs.rmSync(gone.fsPath);
    assert.deepStrictEqual(await saidOnReveal(), ['The file of this highlight, src/gone.rs, is no longer in the working tree.']);
  });

  test('a reference in a comment is a link that names it in a few words, whose tooltip names it in full and whose click opens it', async () => {
    const decision = JSON.parse(fwd('--format', 'json', 'decision', 'create', 'Replies in the editor stay short, and say whom they reach', '--area', 'ux')).record;
    fwd('--as', 'impl-alice', 'reply', comment.id, `see [[${decision.ref}]] and [[${review.slice(0, 8)}]], at [[${comment.anchor.slice(0, 8)}]]`);
    const drawn = await until(() => api.threads(file).flatMap((t) => t.comments).find((c) => (c.body as vscode.MarkdownString).value.startsWith('see ')));
    assert.ok(drawn, 'the reply is in its thread');
    const text = drawn.body as vscode.MarkdownString;
    const links = linksOf(text.value);
    assert.deepStrictEqual(links, [
      // A decision by its number and its title cut at a word, as its lozenge on the page shows it.
      { words: `${decision.ref} Replies in the editor stay short…`, args: [decision.id, 'decision'], tooltip: `${decision.ref}: Replies in the editor stay short, and say whom they reach` },
      { words: 'Review the lib', args: [review, 'task'], tooltip: 'Review the lib' },
      // Lines of code by where they are, `path:lines`, as the page and the CLI name a highlight
      // (fwd-core's `anchor_name`, which the package gives the extension as `anchor_place`).
      { words: 'src/lib.rs:1-2', args: [comment.anchor, 'code'], tooltip: 'src/lib.rs:1-2' },
    ]);
    assert.deepStrictEqual(text.isTrusted, { enabledCommands: ['fwd.openReference'] }, 'the links run that command, and a link in the text no other');
    // A click on the decision's link opens it on the page.
    await vscode.commands.executeCommand('fwd.openReference', ...links[0].args);
    const report = await untilPage(api.page()!, (r) => r.hash.startsWith(`#/decisions/${decision.id}`), Date.now() + 10_000);
    assert.ok(report?.hash.startsWith(`#/decisions/${decision.id}`), `the page is on ${report?.hash}`);
  });

  test('a file a comment names at a version is a link naming its path and the version, whose click opens the file at that version', async () => {
    const head = execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).trim();
    const said = JSON.parse(fwd('--format', 'json', '--as', 'impl-alice', 'reply', comment.id, 'the file [[src/lib.rs]], and as committed [[src/lib.rs@HEAD]]')).record;
    // The text stores each as its whole-file highlight's id and the version it names.
    const [onDisk, atHead] = JSON.parse(fwd('--format', 'json', 'message', 'show', said.id)).references;
    assert.deepStrictEqual([onDisk.version, atHead.version], ['worktree', head]);
    const drawn = await until(() => api.threads(file).flatMap((t) => t.comments).find((c) => (c.body as vscode.MarkdownString).value.startsWith('the file ')));
    assert.ok(drawn, 'the reply is in its thread');
    const links = linksOf((drawn.body as vscode.MarkdownString).value);
    // The path, then the version in the words the page and the CLI use (fwd-core's `anchor_name`).
    assert.deepStrictEqual(links, [
      { words: 'src/lib.rs on disk', args: [onDisk.id, 'file', 'worktree'], tooltip: 'src/lib.rs on disk' },
      { words: `src/lib.rs ${head.slice(0, 7)}`, args: [atHead.id, 'file', head], tooltip: `src/lib.rs ${head.slice(0, 7)}` },
    ]);
    // At a commit, the git extension's document of the file there; on disk, the file itself.
    const committed = gitUri(file, head);
    await vscode.commands.executeCommand('fwd.openReference', ...links[1].args);
    const atCommit = await until(() => vscode.window.activeTextEditor?.document.uri.toString() === committed.toString());
    assert.ok(atCommit, `the active editor shows ${vscode.window.activeTextEditor?.document.uri.toString()}`);
    const asCommitted = execFileSync('git', ['show', `${head}:src/lib.rs`], { cwd: root, encoding: 'utf8' });
    assert.strictEqual(vscode.window.activeTextEditor?.document.getText(), asCommitted);
    await vscode.commands.executeCommand('fwd.openReference', ...links[0].args);
    const onDiskOpened = await until(() => vscode.window.activeTextEditor?.document.uri.toString() === file.toString());
    assert.ok(onDiskOpened, `the active editor shows ${vscode.window.activeTextEditor?.document.uri.toString()}`);
    // The page asks for the file at its version as the text stores it, the id, an at-sign and the version.
    await api.page()!.probe(1000, { type: 'fwd:reveal', detail: `${atHead.id}@${head}` });
    const fromPage = await until(() => vscode.window.activeTextEditor?.document.uri.toString() === committed.toString());
    assert.ok(fromPage, `the active editor shows ${vscode.window.activeTextEditor?.document.uri.toString()}`);
    await vscode.commands.executeCommand('workbench.action.closeActiveEditor');
  });

  test("the first thread on a highlight ends its first comment with what the highlight is joined to, each a link that opens it on the page, and every thread's title bar opens the highlight's own page", async () => {
    const { menus } = vscode.extensions.getExtension(EXTENSION)!.packageJSON.contributes;
    const three = (JSON.parse(fwd('--format', 'json', 'step', 'list', review)) as any[]).find((s) => s.excerpt === 'fn three() {}');
    const onThree = () => api.threads(file).find((t) => t.comments.some((c) => textOf(c) === 'look at three'));
    const thread = await until(() => joinedOf(onThree()?.comments[0]) && onThree());
    assert.ok(thread, `the step's thread says ${JSON.stringify(onThree()?.comments.map((c) => (c.body as vscode.MarkdownString).value))}`);
    // The highlight on `fn three() {}` is a step of the review: the line names the review in the
    // words the highlight's own page gives the group, as a link that opens it.
    assert.match(joinedOf(thread.comments[0])!, /^\*step of\* \[Review the lib\]\(/);
    assert.deepStrictEqual(linksOf(joinedOf(thread.comments[0])), [{ words: 'Review the lib', args: [review, 'task'], tooltip: 'Review the lib' }]);
    // A comment on the review that names the highlight: the line counts it at once, with no refresh.
    fwd('--as', 'impl-alice', 'comment', review, '--', `compare [[${three.id.slice(0, 8)}]] with one`);
    const named = await until(() => joinedOf(thread.comments[0])?.includes('*mentioned in*') && joinedOf(thread.comments[0]));
    assert.ok(named, `the line says ${joinedOf(thread.comments[0])}`);
    assert.deepStrictEqual(linksOf(named), [
      { words: 'Review the lib', args: [review, 'task'], tooltip: 'Review the lib' },
      { words: '1 message', args: [three.id, 'highlight'], tooltip: 'The messages that name this highlight, listed on its page' },
    ]);
    // A click on the review's link opens the page at the review, in the extension's panel.
    await vscode.commands.executeCommand('fwd.openReference', ...linksOf(named)[0].args);
    const onTask = await untilPage(api.page()!, (r) => onTaskPage(r.hash, review), Date.now() + 10_000);
    assert.ok(onTaskPage(onTask?.hash, review), `the page is on ${onTask?.hash}`);
    // The count's link opens the highlight's own page, which lists the message.
    await vscode.commands.executeCommand('fwd.openReference', ...linksOf(named)[1].args);
    const listed = await untilPage(api.page()!, (r) => r.hash === `#/anchors/${three.id}` && r.text.includes('compare'), Date.now() + 10_000);
    assert.ok(listed?.hash === `#/anchors/${three.id}` && listed.text.includes('compare'), `the page is on ${listed?.hash}: ${listed?.text.slice(0, 400)}`);
    // Every thread's title bar opens that page too: fwd: Open the Highlight on the Page.
    const titleBar: MenuItem[] = menus['comments/commentThread/title'];
    for (const commentThread of ['unresolved', 'resolved', 'unresolvedStep', 'resolvedStep']) {
      assert.ok(offered(titleBar, { commentController: 'fwd', commentThread }).includes('fwd.openHighlightPage'), `a thread that is ${commentThread}`);
    }
    await vscode.commands.executeCommand('fwd.openReference', review, 'task');
    assert.ok(await untilPage(api.page()!, (r) => onTaskPage(r.hash, review), Date.now() + 10_000), 'the page went back to the review');
    await vscode.commands.executeCommand('fwd.openHighlightPage', thread);
    const opened = await untilPage(api.page()!, (r) => r.hash === `#/anchors/${three.id}`, Date.now() + 10_000);
    assert.strictEqual(opened?.hash, `#/anchors/${three.id}`);
    // A highlight joined to nothing beyond its threads: its first comment ends with its own text.
    const question = api.threads(file).find((t) => t.comments.some((c) => textOf(c) === 'why does two come after one, @impl-alice?'));
    assert.ok(question, 'the question on the third line');
    assert.strictEqual((question.comments[0].body as vscode.MarkdownString).value, 'why does two come after one, @impl-alice?', 'nothing after its text');
  });

  test('the line of what a highlight is joined to follows the graph with no refresh: a task renamed, a task whose code it is, a step added and taken off', async () => {
    const three = (JSON.parse(fwd('--format', 'json', 'step', 'list', review)) as any[]).find((s) => s.excerpt === 'fn three() {}');
    const thread = api.threads(file).find((t) => t.comments.some((c) => textOf(c) === 'look at three'));
    assert.ok(thread, "the step's thread");
    const joined = () => joinedOf(thread.comments[0]);
    // Renamed, the review is named anew in the line and in the thread's label.
    fwd('task', 'amend', review, '--title', 'Review the whole lib');
    const renamed = await until(() => joined()?.startsWith('*step of* [Review the whole lib]') && thread.label === 'Step of “Review the whole lib”: not checked');
    assert.ok(renamed, `the line says ${joined()}, the label ${thread.label}`);
    fwd('task', 'amend', review, '--title', 'Review the lib');
    assert.ok(await until(() => joined()?.startsWith('*step of* [Review the lib]')), `the line says ${joined()}`);
    // A task whose code the highlight is joins the line, after the tasks it is a step of.
    const naming = JSON.parse(fwd('--format', 'json', 'task', 'create', 'Name the functions', '--why', 'the headless test')).record.id;
    fwd('link', naming, 'anchored_at', three.id);
    const coded = await until(() => joined()?.includes('*code of*') && joined());
    assert.ok(coded, `the line says ${joined()}`);
    assert.match(coded, /^\*step of\* \[Review the lib\]\([^)]*\) · \*code of\* \[Name the functions\]\([^)]*\) · \*mentioned in\* \[1 message\]\(/);
    assert.deepStrictEqual(linksOf(coded).map((l) => l.args), [[review, 'task'], [naming, 'task'], [three.id, 'highlight']]);
    // The question's highlight, joined to nothing, gets the line as a step of the review, and
    // loses it when the step comes off the checklist.
    const question = api.threads(file).find((t) => t.comments.some((c) => textOf(c) === 'why does two come after one, @impl-alice?'));
    assert.ok(question, 'the question on the third line');
    const anchors = JSON.parse(fwd('--format', 'json', 'anchor', 'list', '--path', 'src/lib.rs'));
    const asked = anchors.find((a: any) => a.messages.some((m: any) => String(m.content).startsWith('why does two come after one')));
    const step = JSON.parse(fwd('--format', 'json', 'step', 'add', review, asked.id)).record;
    const body = () => (question.comments[0].body as vscode.MarkdownString).value;
    assert.ok(await until(() => joinedOf(question.comments[0])?.startsWith('*step of* [Review the lib]')), `the question's first comment is ${body()}`);
    fwd('step', 'remove', review, String(step.n), '--reason', 'the headless test takes it off again');
    assert.ok(await until(() => joinedOf(question.comments[0]) === undefined), `the question's first comment is ${body()}`);
  });

  test('clearing the current task from the status bar sends comments to the agents again', async () => {
    await vscode.commands.executeCommand('fwd.clearCurrentTask');
    const now = api.current();
    assert.deepStrictEqual([now.id, now.title, now.status], [undefined, undefined, undefined]);
    assert.match(now.placeHolder ?? '', /^For the agents working here/);
    // A route that shows no task leaves no task current; the page goes back to its inbox, in
    // front, where the next check expects it.
    await vscode.commands.executeCommand('fwd.showInbox');
    const report = await untilPage(api.page()!, (r) => r.hash === '#/inbox', Date.now() + 10_000);
    assert.strictEqual(report?.hash, '#/inbox');
    assert.strictEqual(api.current().id, undefined);
  });

  test('hidden, the page is dropped; shown again, it boots again on the route it was on', async () => {
    const page = api.page()!;
    const group = vscode.window.tabGroups.activeTabGroup;
    assert.ok(group.activeTab?.input instanceof vscode.TabInputWebview, 'the page is the active tab of its group');
    // Another editor in the page's group hides the page, and with retainContextWhenHidden off
    // VS Code drops the page's document, so nothing answers the probe.
    const other = await vscode.workspace.openTextDocument({ content: 'hides the page' });
    await vscode.window.showTextDocument(other, { viewColumn: group.viewColumn, preview: false });
    assert.strictEqual(await page.probe(1000), undefined, 'the hidden page answers nothing');
    // Closing that editor shows the page again, which boots again.
    await vscode.commands.executeCommand('workbench.action.revertAndCloseActiveEditor');
    const report = await untilPage(page, booted, Date.now() + 10_000);
    assert.ok(report && booted(report), `the page shows ${JSON.stringify(report)}`);
    assert.strictEqual(report.hash, '#/inbox', 'it came back on the route it was on');
    assert.deepStrictEqual(report.refused, []);
  });

  /** Stops the scratch daemon and waits until the open page says it lost its connection. */
  const loseTheDaemon = async () => {
    fwd('daemon', 'stop');
    const lost = await untilPage(api.page()!, (r) => r.state !== null && r.state !== 'Connected', Date.now() + 10_000);
    assert.ok(lost && lost.state !== 'Connected', `the page did not notice: ${JSON.stringify(lost?.state)}`);
  };

  test('restarted, the daemon has forgotten the page; the page has its origin registered again and gets back in', async () => {
    await loseTheDaemon();
    fwd('daemon', 'start');
    daemon = JSON.parse(fwd('daemon', 'status'));
    const report = await untilPage(api.page()!, (r) => r.state === 'Connected' && booted(r), Date.now() + 30_000);
    assert.ok(report && report.state === 'Connected' && booted(report), `the page shows ${JSON.stringify(report)}`);
    assert.strictEqual(report.hash, '#/inbox', 'the same page, where it was');
    assert.strictEqual((await ask(daemon.port, 'GET', '/health', report.origin)).status, 200);
    // Back in, the page asks /health which build the daemon runs; the same binary came back, so
    // its bar offering a newer page does not show.
    const later = await untilPage(api.page()!, () => false, Date.now() + 2000);
    assert.ok(later && !later.text.includes('restarted on a newer build'), `the page shows ${JSON.stringify(later?.text)}`);
    // The extension host's node follows the project again: a reply from the CLI joins its thread
    // on the open file, and its mention of the person is in the inbox tree, with no reload.
    const text = 'after the restart, @tester';
    fwd('--format', 'json', '--as', 'impl-alice', 'reply', comment.id, text);
    const thread = await until(() => api.threads(file).find((t) => t.comments.some((c) => shown(c) === `impl-alice: ${text}`)), 30_000);
    assert.ok(thread, `the threads on the file hold ${JSON.stringify(api.threads(file).map((t) => t.comments.map(shown)))}`);
    assert.strictEqual(shown(thread.comments[0]), 'impl-alice: hello from the CLI, @tester', "the reply joined the comment's thread");
    const item = await until(() => api.inbox.getChildren().find((i) => i.n.type === 'mentioned' && i.n.message?.content === text), 30_000);
    assert.ok(item, `the inbox holds ${JSON.stringify(api.inbox.getChildren().map((i) => i.label))}`);
  });

  test("the page's Reload window, on the bar it shows once the daemon runs a newer build, has VS Code reload the window", async () => {
    // The scratch daemon always comes back on the same build, so the page never shows the bar
    // here; the probe sends the event the bar's button sends (crates/fwd-web/src/session.rs,
    // `reload`), and VS Code's command is recorded instead of run, since it would reload this
    // window and end the test.
    const execute = vscode.commands.executeCommand;
    const reloads: string[] = [];
    const record = ((command: string, ...rest: unknown[]) =>
      command === 'workbench.action.reloadWindow' ? (reloads.push(command), Promise.resolve()) : execute(command, ...rest)) as typeof execute;
    await standingIn(vscode.commands, 'executeCommand', record, async () => {
      await api.page()!.probe(1000, { type: 'fwd:reload' });
      await until(() => reloads.length > 0, 5000);
    });
    assert.deepStrictEqual(reloads, ['workbench.action.reloadWindow']);
    const report = await untilPage(api.page()!, booted, Date.now() + 5000);
    assert.ok(report && booted(report) && report.hash === '#/inbox', `the page stays as it was: ${JSON.stringify(report)}`);
  });

  test('after the window reloads, VS Code brings the panel back on the route the page was on, with the same current task', async () => {
    // A task the page showed stays current while the page shows the inbox, whose route cannot
    // make it current again: only what the extension kept can.
    api.page()!.go(`#/tasks/${review}`);
    assert.ok(await until(() => api.current().id === review), 'the page shows the task, which is current');
    api.page()!.go('#/inbox');
    const before = await untilPage(api.page()!, (r) => r.hash === '#/inbox' && booted(r), Date.now() + 10_000);
    assert.ok(before?.hash === '#/inbox' && api.current().id === review, `the page is on ${before?.hash}, ${api.current().id} current`);
    // A reload would end this run, so the test does what VS Code does. VS Code saves the panel with
    // the window, and the webview's state with it, where the bridge keeps the route; it calls the
    // extension's deactivate, then disposes the extension's subscriptions, the panel among them.
    // In the reloaded window the extension joins the project again, and VS Code builds the panel
    // again and hands it to the serializer the extension registered for the panel's view type,
    // which the manifest's activation event names, with the state it saved.
    const main = path.join(vscode.extensions.getExtension(EXTENSION)!.extensionPath, 'dist', 'extension.js');
    await require(main).deactivate();
    const tab = vscode.window.tabGroups.all.flatMap((g) => g.tabs).find((t) => t.input instanceof vscode.TabInputWebview);
    assert.ok(tab, "the page's tab");
    await vscode.window.tabGroups.close(tab);
    assert.strictEqual(api.current().id, undefined, 'the stopped extension holds no current task');
    await api.reconnect();
    const events: string[] = vscode.extensions.getExtension(EXTENSION)!.packageJSON.activationEvents;
    const view = events.map((e) => /^onWebviewPanel:(.+)$/.exec(e)?.[1]).find(Boolean);
    assert.ok(view, `the manifest's activation events name no panel: ${events}`);
    const panel = vscode.window.createWebviewPanel(view, 'fwd', vscode.ViewColumn.Beside, {});
    await api.pageSerializer.deserializeWebviewPanel(panel, before.saved);
    const report = await untilPage(api.page()!, (r) => r.hash === '#/inbox' && booted(r), Date.now() + 20_000);
    assert.ok(report && booted(report) && report.hash === '#/inbox', `the page shows ${JSON.stringify(report)}`);
    assert.deepStrictEqual(report.refused, []);
    const now = await until(() => api.current().title === 'Review the lib' && api.current());
    assert.ok(now, `the current task: ${JSON.stringify(api.current())}`);
    assert.deepStrictEqual([now.id, now.status], [review, '$(tasklist) Review the lib']);
  });

  test('restarted on another port, the daemon is found again: the threads and the inbox follow it on a new node', async () => {
    // Something else takes the daemon's port while it is stopped, so it starts on another one.
    // Ankurah's client retries only the old address, so only a new node can follow the daemon.
    const old = daemon.port;
    await loseTheDaemon();
    const squatter = net.createServer((socket) => socket.destroy());
    await new Promise<void>((resolve, reject) => squatter.once('error', reject).listen(old, '127.0.0.1', resolve));
    try {
      fwd('daemon', 'start');
      daemon = JSON.parse(fwd('daemon', 'status'));
      assert.notStrictEqual(daemon.port, old, 'the daemon listens on another port');
      const text = 'after the move, @tester';
      fwd('--format', 'json', '--as', 'impl-alice', 'reply', comment.id, text);
      const thread = await until(() => api.threads(file).find((t) => t.comments.some((c) => shown(c) === `impl-alice: ${text}`)), 30_000);
      assert.ok(thread, `the threads on the file hold ${JSON.stringify(api.threads(file).map((t) => t.comments.map(shown)))}`);
      assert.strictEqual(shown(thread.comments[0]), 'impl-alice: hello from the CLI, @tester', "the reply joined the comment's thread");
      const item = await until(() => api.inbox.getChildren().find((i) => i.n.type === 'mentioned' && i.n.message?.content === text), 30_000);
      assert.ok(item, `the inbox holds ${JSON.stringify(api.inbox.getChildren().map((i) => i.label))}`);
    } finally {
      squatter.close();
    }
  });

  test('a command opens a page that lost the daemon afresh: a new document, on its route, the daemon started', async () => {
    const before = api.page()!.html;
    await loseTheDaemon();
    await vscode.commands.executeCommand('fwd.showGraph');
    const page = api.page()!;
    assert.notStrictEqual(page.html, before, 'the panel has a new document');
    const report = await untilPage(page, (r) => onGraph(r.hash) && r.state === 'Connected' && booted(r), Date.now() + 20_000);
    assert.ok(report && onGraph(report.hash) && booted(report), `the page shows ${JSON.stringify(report)}`);
    assert.deepStrictEqual(report.refused, []);
  });

  test('closing the panel clears the current task', async () => {
    api.page()!.go(`#/tasks/${review}`);
    assert.ok(await until(() => api.current().id === review), 'the page shows the task, which is current again');
    const tab = vscode.window.tabGroups.all.flatMap((g) => g.tabs).find((t) => t.input instanceof vscode.TabInputWebview);
    assert.ok(tab, "the page's tab");
    await vscode.window.tabGroups.close(tab);
    assert.ok(await until(() => api.current().id === undefined), `still current: ${JSON.stringify(api.current())}`);
    assert.strictEqual(api.page(), undefined);
  });

  test("an agent that renames itself from the CLI, and a task renamed, show their new names, as a comment's author and where a comment mentions them, on the open file's thread and in the inbox, without a reload", async () => {
    fwd('--as', 'author-before', 'principal', 'add', 'author-before', '--role', 'implementer');
    const task = JSON.parse(fwd('--format', 'json', 'task', 'create', 'Name the first line', '--why', 'the headless test')).record.id;
    const said = JSON.parse(fwd('--format', 'json', '--as', 'author-before', 'comment', '--path', 'src/lib.rs', '--lines', '1-1', 'said before a rename')).record;
    // A reply that mentions the agent and a task, and the window's person, so that it reaches the inbox too.
    fwd('--as', 'impl-alice', 'reply', said.id, `@tester, ask @author-before about this line, for [[${task.slice(0, 8)}]]`);
    const comments = () => api.threads(file).find((t) => t.comments.some((c) => textOf(c) === 'said before a rename'))?.comments ?? [];
    const inbox = () => api.inbox.getChildren().map((i) => i.label);
    const asked = (name: string) => `@tester, ask @${name} about this line, for `;
    /** The thread and the inbox name the agent `name` and the task `title`, which the thread's reply names by a link. */
    const reads = (name: string, title: string) => {
      const [first, reply] = comments();
      return (
        first && shown(first) === `${name}: said before a rename` &&
        reply && shown(reply).startsWith(`impl-alice: ${asked(name)}[`) &&
        JSON.stringify(linksOf(textOf(reply))) === JSON.stringify([{ words: title, args: [task, 'task'], tooltip: title }]) &&
        inbox().includes(`mentioned: ${asked(name)}[${title}]`)
      );
    };
    const state = () => `the thread reads ${JSON.stringify(comments().map(shown))}, the inbox ${JSON.stringify(inbox())}`;
    assert.ok(await until(() => reads('author-before', 'Name the first line')), state());
    fwd('--as', 'author-before', 'principal', 'rename', 'author-after');
    assert.ok(await until(() => reads('author-after', 'Name the first line')), state());
    fwd('task', 'amend', task, '--title', 'Name the first line anew');
    assert.ok(await until(() => reads('author-after', 'Name the first line anew')), state());
  });

  test('a window acting as an agent goes on acting as it once the agent renames itself: its inbox follows it through a refresh and a new connection, and its comment offers Edit, whose Save edits it', async () => {
    // The module VS Code loaded, as in the checks below: this is the deactivate VS Code calls.
    const main = path.join(vscode.extensions.getExtension(EXTENSION)!.extensionPath, 'dist', 'extension.js');
    const deactivate: () => Promise<void> = require(main).deactivate;
    fwd('--as', 'window-before', 'principal', 'add', 'window-before', '--role', 'implementer');
    const said = JSON.parse(fwd('--format', 'json', '--as', 'window-before', 'comment', '--path', 'src/lib.rs', '--lines', '2-2', 'said by the window')).record;
    const inbox = () => api.inbox.getChildren().map((i) => i.label);
    const comments = () => api.threads(file).flatMap((t) => t.comments);
    const own = () => comments().find((c) => textOf(c)?.startsWith('said by the window'));
    const settings = () => vscode.workspace.getConfiguration('fwd');
    // The window acts as the agent from now on, as the setting names it: a reply in the agent's
    // thread reaches the window's inbox, as `replied`, which outranks the reply's mention of the
    // agent (fwd-core's `routing::precedence`).
    await settings().update('as', 'window-before', vscode.ConfigurationTarget.Workspace);
    try {
      fwd('--as', 'impl-alice', 'reply', said.id, '@window-before, before the rename');
      assert.ok(await until(() => inbox().includes('replied: @window-before, before the rename')), `the inbox holds ${JSON.stringify(inbox())}`);
      assert.ok(await until(() => own()?.contextValue === 'own'), `the agent's comment is ${JSON.stringify(own())}`);

      // Renamed, the agent is the one the window acts as still: a refresh runs the inbox's query
      // again for it, and a reply after the rename reaches the inbox.
      fwd('--as', 'window-before', 'principal', 'rename', 'window-after');
      await api.inbox.refresh();
      fwd('--as', 'impl-alice', 'reply', said.id, '@window-after, after the rename');
      const renamed = ['replied: @window-after, before the rename', 'replied: @window-after, after the rename'];
      assert.ok(await until(() => renamed.every((label) => inbox().includes(label))), `the inbox holds ${JSON.stringify(inbox())}`);

      // Joined to the project anew, the window acts as the agent still.
      await deactivate();
      await api.reconnect();
      await vscode.window.showTextDocument(await vscode.workspace.openTextDocument(file));
      fwd('--as', 'impl-alice', 'reply', said.id, '@window-after, after a new connection');
      assert.ok(await until(() => inbox().includes('replied: @window-after, after a new connection')), `the inbox holds ${JSON.stringify(inbox())}`);

      // The agent's comment, under its new name, offers Edit, and Save edits it: `fwd message
      // edit`, run as the agent, named by its id.
      const mine = await until(() => own()?.contextValue === 'own' && own());
      assert.ok(mine, `the agent's comment is ${JSON.stringify(own())}`);
      assert.strictEqual(mine.author.name, 'window-after');
      await vscode.commands.executeCommand('fwd.editComment', mine);
      const editing = () => comments().find((c) => c.mode === vscode.CommentMode.Editing);
      assert.strictEqual((await until(editing))?.body, 'said by the window');
      const errors: string[] = [];
      const recordError = ((message: string) => (errors.push(message), Promise.resolve(undefined))) as typeof vscode.window.showErrorMessage;
      const edited = 'said by the window, and edited once it was renamed';
      await standingIn(vscode.window, 'showErrorMessage', recordError, () => {
        editing()!.body = edited;
        return vscode.commands.executeCommand('fwd.saveComment', editing());
      });
      assert.deepStrictEqual(errors, []);
      const saved = await until(() => comments().find((c) => c.mode === vscode.CommentMode.Preview && shown(c) === `window-after: ${edited}`));
      assert.ok(saved, `the thread shows ${JSON.stringify(comments().map(shown))}`);
      assert.strictEqual(saved.label, 'edited');
    } finally {
      await settings().update('as', undefined, vscode.ConfigurationTarget.Workspace);
    }
  });

  test('with the daemon stopped, a document opened asks the CLI where its highlights are without starting a daemon', async () => {
    // The window places a document's highlights by itself, not at a person's command, so its call
    // tells the CLI not to start a daemon (src/comments.ts, `placeAll`), and with none running the
    // CLI refuses it. A closing window's last calls are such placings, and on CI one started a
    // daemon after the scratch daemon was stopped (task lK-_4TEE). The extension runs the CLI with
    // child_process's execFile, looked up on the module at each call (src/fwd.ts), so the test
    // stands in for it to see the placing's call and the CLI's answer; every call still runs.
    fwd('daemon', 'stop');
    assert.ok(await until(() => daemons().length === 0), `ps still finds ${daemons().join('; ')}`);
    const late = vscode.Uri.file(path.join(root, 'src', 'late.rs'));
    fs.writeFileSync(late.fsPath, 'fn late() {}\n');
    const childProcess: typeof import('child_process') = require('child_process');
    const execFile = childProcess.execFile;
    let placing: { args: string[]; stderr: string } | undefined;
    const record = ((file: string, args: unknown, ...rest: unknown[]) => {
      const done = rest.pop();
      if (Array.isArray(args) && args.includes('src/late.rs') && typeof done === 'function') {
        rest.push((error: Error | null, stdout: string, stderr: string) => {
          placing = { args, stderr: String(stderr) };
          done(error, stdout, stderr);
        });
      } else {
        rest.push(done);
      }
      return (execFile as unknown as (...call: unknown[]) => unknown)(file, args, ...rest);
    }) as unknown as typeof execFile;
    try {
      await standingIn(childProcess, 'execFile', record, async () => {
        await vscode.window.showTextDocument(await vscode.workspace.openTextDocument(late));
        await until(() => placing, 30_000);
      });
      assert.ok(placing, 'the window asked the CLI nothing about src/late.rs');
      assert.deepStrictEqual(daemons(), [], `fwd ${placing.args.join(' ')} started a daemon`);
      assert.match(placing.stderr, /no daemon is running and autostart is off/, `fwd ${placing.args.join(' ')}: ${placing.stderr}`);
    } finally {
      fwd('daemon', 'start');
    }
  });

  test("with reads under way on the node and its live queries answering, deactivate and a new node in the node's place free nothing still in use, twenty times over", async function () {
    // About twenty seconds in the container; GitHub's runner takes several times as long.
    this.timeout(300_000);
    // The module VS Code loaded, as in the next check: this is the deactivate VS Code calls.
    const main = path.join(vscode.extensions.getExtension(EXTENSION)!.extensionPath, 'dist', 'extension.js');
    const deactivate: () => Promise<void> = require(main).deactivate;
    // A read is the mention search a comment box runs, a call into the node's package that holds
    // the node's context until the daemon has answered, with the inbox's refresh, which opens the
    // inbox's live query again. The refresh alone asks the node nothing: the window keeps whom it
    // acts as by its id (src/fwd.ts, `actingAs`).
    const reads: Promise<unknown>[] = [];
    let under = 0; // the reads started and not settled yet
    const read = () => {
      under++;
      reads.push(Promise.allSettled([api.mentions('@'), api.inbox.refresh()]).finally(() => under--));
    };
    /** Keeps four reads under way, starting more each turn as others settle, until `done`. */
    const readUntil = async (done: () => boolean) => {
      for (;;) {
        while (under < 4) read();
        if (done()) return;
        await new Promise((resolve) => setImmediate(resolve));
      }
    };
    const failures: string[] = [];
    for (let round = 0; round < 20; round++) {
      // Each round deactivates a millisecond later than the one before after the last node took
      // its place, so that across the rounds deactivate meets the live queries that node opened
      // before, while and after they answer. It deactivates from an I/O callback (a file's status
      // read), right after four more reads: the daemon's answers to those cannot arrive before the
      // event loop next polls for I/O, so however soon closing frees the context, they hold it.
      const deadline = Date.now() + round;
      await readUntil(() => Date.now() >= deadline);
      await new Promise<void>((resolve) => fs.stat(__filename, () => resolve()));
      for (let i = 0; i < 4; i++) read();
      await deactivate().catch((e) => failures.push(`round ${round}: deactivate threw ${e}`));
      const state = api.node();
      if (!state?.closed || state.held) failures.push(`round ${round}: deactivate left the node ${JSON.stringify(state)}`);
      await api.reconnect();
      // A new node takes the node's place while reads are under way on the node; the callback of
      // the swap, run in the same turn, checks that some were.
      let replaced = false;
      const rejoined = api
        .rejoin()
        .then(
          () => {
            if (!under) failures.push(`round ${round}: no read was under way when the new node took the node's place`);
          },
          (e) => {
            failures.push(`round ${round}: putting a new node in the node's place threw ${e}`);
          },
        )
        .finally(() => (replaced = true));
      await readUntil(() => replaced);
      await rejoined;
    }
    await Promise.all(reads);
    assert.deepStrictEqual(failures, []);
  });

  test('deactivate frees the node and removes the threads', async () => {
    assert.ok(api.node()!.held > 0, 'the node holds live queries before');
    // The module VS Code loaded (require's cache holds it), so this is the deactivate VS Code calls.
    const main = path.join(vscode.extensions.getExtension(EXTENSION)!.extensionPath, 'dist', 'extension.js');
    await require(main).deactivate();
    assert.deepStrictEqual(api.node(), { closed: true, held: 0 });
    assert.deepStrictEqual(api.threads(file), []);
  });

  // The runner stops the scratch daemon once VS Code, and with it this extension host, has exited
  // (.vscode-test.mjs), so that nothing of the window calls `fwd` after the stop. A call the
  // extension started would outlive the host; the placings it makes by itself, which the last
  // checks leave under way, end as the extension deactivates (src/comments.ts, `cancel`), and the
  // host checks here that none runs.
  suiteTeardown(async () => {
    const done = await until(() => calls().length === 0, 10_000);
    assert.ok(done, `the extension's fwd calls still running: ${calls().join('; ')}`);
    assert.strictEqual(daemons().length, 1, `ps finds one scratch daemon, which the runner stops: ${daemons().join('; ')}`);
  });
});
