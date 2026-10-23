// Message text for reading in the editor. Posting a message stores each reference in its text,
// `@handle` or an id, as `[[<full id>]]` (docs/design/cli.md, Messages), outside code; the
// extension shows a person as `@handle` and anything else as its name in brackets, as the CLI's
// text output does (fwd-core's `render_references`), and in a comment thread draws what a click
// can open as a link naming it in a few words, as the page's lozenges do (`markdown`). Each is
// named as it is now, so that a mention follows a renaming: a person or an agent by the handle it
// has now (src/principals.ts), anything else by the name src/referents.ts keeps. A comment edited
// in place opens on its text written as its writer would type it again (`editable`). The links of
// a thread's line of what its highlight is joined to (src/comments.ts) are made here too
// (`taskLink`, `openLink`). A file a message names at a version (fwd-core's `file_mention`) is
// stored as its whole-file highlight's id with the version after an at-sign, and reads as the
// file's path and that version (`docs/cli.md e13b8e2`, `index`, `on disk`), whose link opens the
// file at that version.

import { HostNode, Referent } from './node';
import { Principals } from './principals';
import { Referents } from './referents';

/**
 * A stored reference: the full id of an entity, 43 characters of URL-safe base64, and for a file
 * a message names at a version, an at-sign and the version (`worktree`, `index` or a commit's id).
 */
const STORED = /\[\[([A-Za-z0-9_-]{43})(?:@(worktree|index|[0-9a-fA-F]{7,64}))?\]\]/g;

/** The version of a file a stored mention names, as `STORED` reads it after the id. */
export const FILE_VERSION = /^(worktree|index|[0-9a-fA-F]{7,64})$/;

/**
 * The command a reference's link runs (extension.ts), which the thread's Markdown must trust, and
 * the links of the line of what a highlight is joined to run too.
 */
export const OPEN_REFERENCE = 'fwd.openReference';

/**
 * A link in a thread's Markdown that opens what `id` names (`OPEN_REFERENCE`), reading `words`,
 * whose tooltip is `full`: `kind` says what it is and so where it opens, a task, a decision or a
 * scope's tasks on the page, lines of code (`code`) in the editor, a file a message names at
 * `version` (`file`) in the editor at that version, and a highlight's own page on the page
 * (`highlight`).
 */
export function openLink(id: string, kind: string, words: string, full: string, version?: string): string {
  const args = version === undefined ? [id, kind] : [id, kind, version];
  return `[${escapeText(words)}](command:${OPEN_REFERENCE}?${encodeURIComponent(JSON.stringify(args))} "${escapeTitle(full)}")`;
}

/** A link that opens task `id` on the page, naming it by `title` cut within 44 characters, as its lozenge does. */
export function taskLink(id: string, title: string): string {
  return openLink(id, 'task', cut(title, 44), title);
}

export class References {
  constructor(
    private node: () => HostNode | undefined,
    private principals: Principals,
    private referents: Referents,
  ) {}

  /** `text` with each reference read as its name; one whose name is not known stays as stored. */
  render(text: string): string {
    return this.replace(text, (_id, named, version) => (version && named.kind === 'code' ? `[${this.fileName(named.place, version)}]` : nameOf(named)));
  }

  /**
   * `text` as Markdown for a comment in a thread, where the box is narrow: each reference to a
   * task, a decision, a scope or lines of code a link that names it in a few words, as its lozenge
   * on the page does (a task by its title cut within 44 characters, a decision by its number and
   * its title cut within 36), whose tooltip names it in full and whose click opens it
   * (`OPEN_REFERENCE`); a file a message names at a version by its path and the version, whose
   * click opens the file at that version; a person stays `@handle` and a message its first words
   * in brackets, as `render` reads them.
   */
  markdown(text: string): string {
    return this.replace(text, (id, named, version) => {
      switch (named.kind) {
        case 'task':
          return taskLink(id, named.title);
        case 'decision':
          return openLink(id, named.kind, `D${named.number} ${cut(named.title, 36)}`, `D${named.number}: ${named.title}`);
        case 'scope':
          return openLink(id, named.kind, named.title, named.title);
        case 'code': {
          if (!version) return openLink(id, named.kind, named.place, named.place);
          const file = this.fileName(named.place, version);
          return openLink(id, 'file', file, file, version);
        }
        default:
          return nameOf(named);
      }
    });
  }

  /**
   * The name of the whole file at `path` as a mention names it at `version`: its path, then the
   * version in the words the page and the CLI use (`HostNode.fileVersion`), `docs/cli.md e13b8e2`.
   */
  private fileName(path: string, version: string): string {
    return `${path} ${this.node()?.fileVersion(version) ?? version}`;
  }

  /**
   * `text` as `markdown` draws it, then `line`, fwd's own, as a paragraph after it: a thread's line
   * of what its highlight is joined to. A text that ends inside a fenced code block (one its writer
   * left open) has the block closed first, as its writer opened it, or the line would read as code.
   */
  markdownThen(text: string, line: string): string {
    const drawn = this.markdown(text);
    const fence = fences(text).open;
    return fence ? `${drawn}${drawn.endsWith('\n') ? '' : '\n'}${fence}\n\n${line}` : `${drawn}\n\n${line}`;
  }

  /**
   * `text` with each reference outside code whose name is known written by `write`, given the
   * version a file mention names; the others stay as stored.
   */
  private replace(text: string, write: (id: string, named: Referent, version: string | undefined) => string): string {
    const code = codeRanges(text);
    return text.replace(STORED, (token, id: string, version: string | undefined, at: number) => {
      const named = this.named(id);
      return inside(code, at) || !named ? token : write(id, named, version);
    });
  }

  /** What `id` names, as it is now: a person or an agent by the handle it has now, anything else as src/referents.ts has it. */
  private named(id: string): Referent | undefined {
    const handle = this.principals.handle(id);
    return handle === undefined ? this.referents.named(id) : { kind: 'person', handle };
  }

  /**
   * A stored `text` as its writer would type it again, for editing a comment in place: each
   * reference written as fwd-core's `references::editable` writes it, a person as `@handle`, a
   * decision as `[[D12]]`, anything else as the first eight characters of its id in double
   * brackets, so the comment's editor shows names and `fwd message edit` resolves each back to what
   * it named. A handle is written only where it reads back as that handle: after a space or an
   * opening mark, and before anything that cannot continue a handle. A file a message names keeps
   * its version after the start of its highlight's id (`[[vwugaN8h@index]]`), which the edit reads
   * back as the same file at the same version.
   */
  async editable(text: string): Promise<string> {
    const node = this.node();
    const code = codeRanges(text);
    let out = '';
    let from = 0;
    for (const match of text.matchAll(STORED)) {
      const at = match.index!;
      if (inside(code, at)) continue;
      const id = match[1];
      const end = at + match[0].length;
      out += text.slice(from, at);
      if (match[2] !== undefined) {
        out += `[[${id.slice(0, 8)}@${match[2]}]]`;
        from = end;
        continue;
      }
      const written = node ? await node.writtenAs(id).catch(() => undefined) : undefined;
      const short = `[[${id.slice(0, 8)}]]`;
      if (written?.startsWith('@')) {
        const handle = written.slice(1);
        const readsBack = /^[A-Za-z0-9_.-]+$/.test(handle) && !handle.endsWith('.') && opensMention(out[out.length - 1]) && endsHandle(text.slice(end));
        out += readsBack ? written : short;
      } else {
        out += written ? `[[${written}]]` : short;
      }
      from = end;
    }
    return out + text.slice(from);
  }
}

/** What `named` reads as in text, as fwd-core's `render_references` writes it. */
function nameOf(named: Referent): string {
  switch (named.kind) {
    case 'person':
      return `@${named.handle}`;
    case 'task':
    case 'scope':
      return `[${named.title}]`;
    case 'decision':
      return `[D${named.number} (${named.title})]`;
    case 'code':
      return `[${named.place}]`;
    case 'message':
      return `[“${named.words}”]`;
  }
}

/**
 * `text` cut to at most `n` characters, as the page's lozenges cut a name, and at the end of a
 * word when one ends in the second half, since the name is read within a sentence; an ellipsis
 * marks the cut.
 */
function cut(text: string, n: number): string {
  const chars = [...text];
  if (chars.length <= n) return text;
  const head = chars.slice(0, n).join('');
  const space = head.lastIndexOf(' ');
  return `${(space > n / 2 ? head.slice(0, space) : head).replace(/[\s,;:.]+$/, '')}…`;
}

/** `text` as a link's words in Markdown, each mark that Markdown reads escaped. */
function escapeText(text: string): string {
  return text.replace(/[\\`*_{}[\]()#+\-.!|<>~]/g, '\\$&');
}

/** `text` as a link's tooltip in Markdown, between double quotes. */
function escapeTitle(text: string): string {
  return text.replace(/["\\]/g, '\\$&');
}

/** Whether an `@` after `before` starts a mention, as fwd-core's `opens_mention` reads it: at the start of the text, or after a space or an opening mark. */
export function opensMention(before: string | undefined): boolean {
  return before === undefined || /\s/.test(before) || '([{,;:!?"\'*>'.includes(before);
}

/**
 * Whether a handle ends where `rest` begins: `rest` starts with nothing a handle continues with,
 * full stops aside when nothing a handle is made of follows them, since a handle's trailing full
 * stops end the sentence rather than the handle.
 */
function endsHandle(rest: string): boolean {
  const next = rest.replace(/^\.+/, '')[0];
  return next === undefined || !/[A-Za-z0-9_.-]/.test(next);
}

/** The ids `text` refers to outside code. */
export function referencesIn(text: string): string[] {
  const code = codeRanges(text);
  return [...text.matchAll(STORED)].filter((m) => !inside(code, m.index!)).map((m) => m[1]);
}

function inside(ranges: [number, number][], at: number): boolean {
  return ranges.some(([start, end]) => at >= start && at < end);
}

/**
 * The ranges of `text` that are code, as fwd-core reads them (references.rs, `code_ranges`):
 * fenced blocks (a line opening with three or more backticks or tildes, to a line closing with as
 * many of the same) and, within a line, code spans (a run of backticks to the next run of as
 * many).
 */
function codeRanges(text: string): [number, number][] {
  const { ranges, prose } = fences(text);
  for (const [start, end] of prose) {
    let i = start;
    while (i < end) {
      if (text[i] !== '`') {
        i++;
        continue;
      }
      let open = 0;
      while (text[i + open] === '`') open++;
      let j = i + open;
      let close: number | undefined;
      while (j < end) {
        if (text[j] !== '`') {
          j++;
          continue;
        }
        let run = 0;
        while (text[j + run] === '`') run++;
        if (run === open) {
          close = j + run;
          break;
        }
        j += run;
      }
      if (close === undefined) {
        i += open; // a run with no partner is plain text
      } else {
        ranges.push([i, close]);
        i = close;
      }
    }
  }
  return ranges;
}

/**
 * The fenced code blocks of `text`, as `codeRanges` reads them, and the lines outside them
 * (`prose`); `open` is the fence of a block the text ends inside, with no line closing it.
 */
function fences(text: string): { ranges: [number, number][]; prose: [number, number][]; open?: string } {
  const ranges: [number, number][] = [];
  const prose: [number, number][] = [];
  let fence: { mark: string; length: number; start: number } | undefined;
  let offset = 0;
  for (const line of text.split(/(?<=\n)/)) {
    const trimmed = line.trimStart();
    const mark = trimmed[0] === '`' || trimmed[0] === '~' ? trimmed[0] : undefined;
    const run = mark ? trimmed.length - trimmed.replace(new RegExp(`^\\${mark}+`), '').length : 0;
    if (fence) {
      if (mark === fence.mark && run >= fence.length && trimmed.slice(run).trim() === '') {
        ranges.push([fence.start, offset + line.length]);
        fence = undefined;
      }
    } else if (mark && run >= 3) {
      fence = { mark, length: run, start: offset };
    } else {
      prose.push([offset, offset + line.length]);
    }
    offset += line.length;
  }
  if (fence) ranges.push([fence.start, text.length]);
  return { ranges, prose, open: fence && fence.mark.repeat(fence.length) };
}

