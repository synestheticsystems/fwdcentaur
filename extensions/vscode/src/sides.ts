// Which version of a file a document shows, so its threads and highlights are placed against
// that version. A plain editor, and the working-tree side of a diff, show the file itself (a
// `file:` document). The other sides of VS Code's git diffs are `git:` documents, which the
// built-in git extension serves: their query is `{"path": <the file's absolute path>, "ref": ...}`,
// where the ref is `''` for the index, `'~'` for the left side of the working tree's changes (the
// index when the file has staged changes, else HEAD, which then has the same content), `'HEAD'`,
// or a commit (`<sha>`, `<sha>^`).
//
// The git extension also opens a `git:` document of its own for each file an editor shows: the
// version the gutter's change markers compare the file against (the index, or HEAD when the file
// has staged changes; VS Code 1.139.1), named as the file with `.git` after its name. No person
// opened it and it is no side of a diff, so fwd reads it as no version of the file and shows no
// threads on it: VS Code's Comments panel lists every thread under the document it is on, and
// until 2026-09-30 it listed each thread twice, under `lib.rs` and `lib.rs.git` (task Lj8otCIi).

import * as path from 'path';
import * as vscode from 'vscode';

/**
 * The version a document shows: the working tree, or a revision, with the `--against` target
 * `fwd anchor list` resolves anchors against there (`index`, `HEAD`, a commit).
 */
export type Side = { kind: 'worktree' } | { kind: 'revision'; ref: string; against: string };

/**
 * The side `uri` shows, or undefined for a document that is not a version of a file in git, and
 * for the copy the change markers compare against.
 */
export function sideOf(uri: vscode.Uri): Side | undefined {
  if (uri.scheme === 'file') return { kind: 'worktree' };
  const query = gitQuery(uri);
  if (!query) return undefined;
  const ref = query.ref;
  return { kind: 'revision', ref, against: ref === '' || ref === '~' ? 'index' : ref };
}

/**
 * The `fwd` arguments that anchor lines of the revision `uri` shows, when it shows one, so that a
 * highlight made there has the lines as that revision has them, and says whether they were chosen
 * on the code as it was or on the change (docs/design/anchoring.md, Diff views): `--against` the
 * revision, and `--side` of the diff it is on. That is the side of the diff in the active tab that
 * shows it, the tab the menu or the comment box acts in; else the side its ref says, since the git
 * extension puts the index's own document (`''`) on the right of the staged changes and every
 * other on a left: `~` in the working tree's changes, `HEAD` in the staged changes. The file
 * itself takes none.
 */
export function revisionArgs(uri: vscode.Uri): string[] {
  const side = sideOf(uri);
  if (side?.kind !== 'revision') return [];
  const tab = vscode.window.tabGroups.activeTabGroup.activeTab?.input;
  const inDiff = tab instanceof vscode.TabInputTextDiff ? tab : undefined;
  const on = inDiff?.original.toString() === uri.toString() ? 'base' : inDiff?.modified.toString() === uri.toString() ? 'modified' : undefined;
  return ['--against', side.against, '--side', on ?? (side.ref === '' ? 'modified' : 'base')];
}

/** The absolute path of the file `uri` shows a version of: the file itself, or a side of a diff. */
export function filePathOf(uri: vscode.Uri): string | undefined {
  if (uri.scheme === 'file') return uri.fsPath;
  return gitQuery(uri)?.path;
}

/**
 * The file and the ref of a `git:` document, read from its query; none for the copy the change
 * markers compare against, which the git extension names as the file with `.git` after its name,
 * where a side of a diff keeps the file's own name.
 */
function gitQuery(uri: vscode.Uri): { path: string; ref: string } | undefined {
  if (uri.scheme !== 'git') return undefined;
  try {
    const query = JSON.parse(uri.query);
    if (typeof query?.path !== 'string' || typeof query?.ref !== 'string') return undefined;
    if (path.posix.basename(uri.path) === `${path.basename(query.path)}.git`) return undefined;
    return { path: query.path, ref: query.ref };
  } catch {
    return undefined;
  }
}
