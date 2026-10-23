// Running the `fwd` binary, for everything the extension host's node cannot do: starting the
// daemon and everything that needs git (anchoring lines, finding where an anchor sits in a
// version of its file). Also the settings that choose the binary and the principal it acts as,
// that principal found by its id and kept (`actingAs`), and small helpers the rest of the
// extension shares.

import * as vscode from 'vscode';
import { execFile } from 'child_process';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import type { HostNode } from './node';

export type Json = any;

/**
 * The binary `fwd.path` stands for when it is empty: `fwd` when the extension host's PATH has
 * one, else the one `cargo install` puts in ~/.cargo/bin, since VS Code started from the Dock
 * often has a PATH without it. Looked up once, at activation (`findBinary`).
 */
let defaultBinary = 'fwd';

/** Looks for `fwd` on the extension host's PATH and settles what an empty `fwd.path` means. */
export function findBinary(): string {
  const dirs = (process.env.PATH ?? '').split(path.delimiter).filter(Boolean);
  const onPath = dirs.some((dir) => isExecutable(path.join(dir, 'fwd')));
  defaultBinary = onPath ? 'fwd' : path.join(os.homedir(), '.cargo', 'bin', 'fwd');
  return defaultBinary;
}

function isExecutable(file: string): boolean {
  try {
    fs.accessSync(file, fs.constants.X_OK);
    return fs.statSync(file).isFile();
  } catch {
    return false;
  }
}

export function config() {
  const c = vscode.workspace.getConfiguration('fwd');
  return { bin: c.get<string>('path') || defaultBinary, as: c.get<string>('as') || '' };
}

/** Whom the window acts as, once found (`actingAs`): the principal's id, and the `fwd.as` it was found by. */
let acting: { as: string; id: string } | undefined;

/**
 * The id of the principal the window acts as: the one `fwd.as` names, read as the CLI's `--as`
 * reads it (its name, or its id), else the project's person. `node` finds it the first time it is
 * asked for, and the same id holds from then on, through a refresh of the inbox and a new node,
 * until `fwd.as` changes: so that when an agent the window acts as renames itself (`fwd principal
 * rename`), after which `fwd.as` gives a name no principal has, nothing changes but the name
 * shown. The inbox follows the agent still, its comments offer Edit still, and `fwd` names it by
 * its id as `--as`. Undefined while `fwd.as` names no one.
 */
export async function actingAs(node: HostNode): Promise<string | undefined> {
  const { as } = config();
  if (acting?.as === as) return acting.id;
  const id = await node.principal(as);
  // Kept only while `fwd.as` says what it said when the node was asked.
  if (id !== undefined && config().as === as) acting = { as, id };
  return id;
}

export function workspaceRoot(): string | undefined {
  return vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
}

/**
 * Runs `fwd --format json <args>` in the workspace root, or in `cwd`, and parses the result.
 * Aborting `signal` kills the process, and the promise rejects.
 */
export function fwd(args: string[], { cwd, signal }: { cwd?: string; signal?: AbortSignal } = {}): Promise<Json> {
  const { bin, as } = config();
  const root = cwd ?? workspaceRoot();
  // The principal `fwd.as` names, by its id once the window has found it (`actingAs`).
  const who = as && acting?.as === as ? acting.id : as;
  const full = ['--format', 'json', ...(who ? ['--as', who] : []), ...args];
  return new Promise((resolve, reject) => {
    execFile(bin, full, { cwd: root, maxBuffer: 16 * 1024 * 1024, signal }, (err, stdout, stderr) => {
      if (err) {
        reject(new Error(`fwd ${args.join(' ')}: ${stderr?.toString().trim() || err.message}`));
        return;
      }
      const text = stdout.toString().trim();
      try {
        resolve(text ? JSON.parse(text) : null);
      } catch {
        resolve(text);
      }
    });
  });
}

/** Where the folder `root` sits in its git repository: "" at the root, "sub/dir/" below it. */
export function repositoryPrefix(root: string): Promise<string | undefined> {
  return new Promise((resolve) => {
    execFile('git', ['rev-parse', '--show-prefix'], { cwd: root }, (err, stdout) => resolve(err ? undefined : stdout.toString().trim()));
  });
}

export function short(id: string): string {
  return id.slice(0, 6);
}

/**
 * `run` as a function that never overlaps itself: a call made while it runs asks for one more
 * run after it, however many such calls there were.
 */
export function serially(run: () => Promise<void>): () => void {
  let running = false;
  let again = false;
  const go = async () => {
    if (running) {
      again = true;
      return;
    }
    running = true;
    try {
      do {
        again = false;
        await run();
      } while (again);
    } catch (e) {
      console.error('fwd:', e);
    } finally {
      running = false;
    }
  };
  return () => void go();
}
