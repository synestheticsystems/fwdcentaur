// Opening a highlight in the editor: when a step or a highlight is clicked on the page, when a
// notification about one is opened from the inbox, and when a link naming lines of code is clicked
// in a thread. So that a click on a highlight takes the reader to the code as it is, always in the
// same kind of editor, it opens the file on disk in the normal editor and never a diff, wherever
// the highlight was made and whatever change its lines lie in (docs/DECISIONS.md, D115, "Clicking
// a highlight in VS Code opens the file at its lines in the normal editor, never a diff"). One
// `fwd anchor show` says where the lines are in the working tree. The cursor is put at the start
// of the lines, which are scrolled into view, and their thread stays open or closed as it was, for
// the reader to open from VS Code's comment icon in the gutter (docs/DECISIONS.md, D98, "In VS
// Code a highlight's thread opens from the gutter's comment icon, not when the cursor enters its
// lines").
//
// A file a message names at a version (fwd-core's `file_mention`) opens at that version, so that
// the reader sees the file the writer meant: on disk (`worktree`), the file itself, as a
// whole-file highlight opens; in the index, or at a commit, the git extension's own document of
// that version, as a side of a diff shows it (src/sides.ts), where the file's threads show too.

import * as vscode from 'vscode';
import { fwd, Json } from './fwd';

/**
 * Opens the highlight whose id is `anchor`, in `column`, as above. A highlight on the whole file
 * opens the file with nothing to put the cursor on; one whose lines are gone from the working tree
 * opens its file, where its threads are on the whole file, and says the lines are gone; one whose
 * file is gone from the working tree says so, and opens nothing. With `version`, the version of a
 * file a message names, the whole file opens at that version (`revealVersion`).
 */
export async function reveal(anchor: string, absPath: (rel: string) => string, column: vscode.ViewColumn, version?: string) {
  if (version !== undefined && version !== 'worktree') return revealVersion(anchor, version, absPath, column);
  const shown: Json = await fwd(['anchor', 'show', anchor]);
  const resolved = shown.resolved ?? {};
  const rel: string = resolved.path || shown.path;
  const file = vscode.Uri.file(absPath(rel));
  try {
    await vscode.workspace.fs.stat(file);
  } catch (e) {
    if (!(e instanceof vscode.FileSystemError && e.code === 'FileNotFound')) throw e;
    void vscode.window.showInformationMessage(`The file of this highlight, ${rel}, is no longer in the working tree.`);
    return;
  }
  const editor = await vscode.window.showTextDocument(file, { viewColumn: column, preview: false });
  if (shown.whole_file === true) return;
  if (!resolved.range) {
    void vscode.window.showInformationMessage('The lines of this highlight are no longer in the working tree; its threads are on the whole file.');
    return;
  }
  const lines = new vscode.Range(resolved.range.start_line, 0, resolved.range.end_line - 1, 0);
  editor.selection = new vscode.Selection(lines.start, lines.start);
  editor.revealRange(lines, vscode.TextEditorRevealType.InCenterIfOutsideViewport);
}

/**
 * Opens the whole file the highlight `anchor` holds as `version` has it, `index` or a commit's id,
 * in `column`: the git extension's document of the file at that version, which VS Code names as the
 * git extension names the sides of its diffs (`{path, ref}` in a `git:` URI's query, the ref `''`
 * for the index), at the path the file has there. A version this checkout does not have, and a
 * version that does not hold the file, open nothing and say so.
 */
async function revealVersion(anchor: string, version: string, absPath: (rel: string) => string, column: vscode.ViewColumn) {
  const shown: Json = await fwd(['anchor', 'show', anchor, '--against', version]);
  const resolved = shown.resolved ?? {};
  const where = version === 'index' ? 'the index' : `commit ${version.slice(0, 7)}`;
  if (resolved.label === 'absent') {
    void vscode.window.showInformationMessage(`This checkout does not have ${where}, where the message names ${shown.path}.`);
    return;
  }
  if (resolved.label === 'orphaned') {
    void vscode.window.showInformationMessage(`${shown.path} is not in ${where}.`);
    return;
  }
  const file = vscode.Uri.file(absPath(resolved.path || shown.path));
  const uri = file.with({ scheme: 'git', path: file.path, query: JSON.stringify({ path: file.fsPath, ref: version === 'index' ? '' : version }) });
  await vscode.window.showTextDocument(uri, { viewColumn: column, preview: false });
}
