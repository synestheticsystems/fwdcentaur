#!/usr/bin/env python3
"""Keep docs/design/data-models.html, the page that lists every Ankurah model fwd reads or writes, true
to the code. Write the page from fwd's own models, in crates/fwd-core/src/model.rs, and from the
Commons' models, in the Commons' sources at the revision Cargo.lock pins, which `cargo metadata` finds;
with `--check`, as CI runs it, write nothing and fail at the first difference between the committed
page and what this would write. The run stops at a model the mapping below does not place, and at an
entry of the mapping that names no model, so that the mapping names exactly the models the code
declares.

    python3 -I scripts/data-models-page.py [--check]
"""
import argparse
from collections import namedtuple
import html
import json
from pathlib import Path
import re
import subprocess
import sys

REPO = Path(__file__).resolve().parent.parent
PAGE = 'docs/design/data-models.html'
FWD_MODELS = 'crates/fwd-core/src/model.rs'
SCRIPT = 'scripts/data-models-page.py'

# The mapping: where the page places each model, and what it says of each model's place.

# fwd's own models, by their paths in model.rs, each with its Commons counterpart (None: the Commons has
# nothing for it) and a note, in HTML.
COUNTERPART = {
    'Project': (None, "Commons models have no project field; fwd's <code>Placement</code> records project membership."),
    'Principal': ('identity.principal, with identity.credential',
                  "Migrate first, with the identity rework: a session is an agent's identity."),
    'Repo': ('git.repository', ''),
    'Task': ('intention-graph.task, with records.assignment, records.creation, records.closure and '
             'intention-graph.submission', 'Migrate last; nearly everything reads it.'),
    'Relation': ('intention-graph.link, with messaging.mention on a mention',
                 "Migrate each intention's <code>assigned_to</code> relation to a Commons link of kind "
                 '<code>assigned_to</code>.'),
    'CodeAnchor': ('git.file_anchor, with highlights.fragment', ''),
    'Message': ('messaging.message, with records.assignment on a question', ''),
    'Notification': (None, 'Notification routing is specific to fwd.'),
    'Scope': ('intention-graph.scope, with git.work_location', ''),
    'Decision': ('intention-graph.decision, with records.closure', ''),
    'Placement': (None, 'Adds project membership to Commons records until Commons supports it.'),
    'FormerId': (None, 'Preserves old IDs as aliases when a migration assigns new IDs.'),
}
# fwd's models kept only to read older stores, by their paths in model.rs.
FOR_OLDER_STORES = ['before_d108::Scope']
# The Commons models fwd uses, in the order the page lists them.
COMMONS_USED = ['Intention', 'Creation', 'Attribution', 'LastEdit', 'Closure']
# The Commons models planned for fwd, each with what it replaces in fwd.
COMMONS_PLANNED = {
    'Task': 'Task', 'Decision': 'Decision', 'Link': 'Relation', 'Scope': 'Scope',
    'Submission': 'Task submission fields', 'Assignment': 'Task assignee; question addressee', 'Message': 'Message',
    'Mention': 'Relation (kind mention)', 'FileAnchor': 'CodeAnchor', 'Fragment': 'CodeAnchor', 'Repository': 'Repo',
    'WorkLocation': 'Scope (the repository and branch its context holds)', 'Principal': 'Principal',
    'Credential': 'Principal authentication subject', 'Claim': 'nothing yet', 'Perspective': 'nothing yet',
    'Ownership': 'nothing yet',
}
# The Commons namespace for other applications, whose models fwd does not compile.
NOT_FOR_FWD = 'health'

# The page's lists of models, each in the order the page gives it.
Placed = namedtuple('Placed', 'own older used planned not_for_fwd')


class PageError(Exception):
    """Why the page cannot be written from the code as it stands, in sentences for whoever ran this."""


DOC = re.compile(r'///(?!/)(?P<text>.*)')
DERIVES_MODEL = re.compile(r'\bderive\([^)]*\bModel\b')
LABEL = re.compile(r'#\[model\(.*\blabel\s*=\s*"(?P<label>[^"]+)"')
VISIBILITY = r'(pub(\([^)]*\))? )?'
MODULE = re.compile(VISIBILITY + r'mod (?P<name>\w+) \{')
STRUCT = re.compile(VISIBILITY + r'struct (?P<name>\w+)(?P<body> \{| \{\}|;)')
FIELD = re.compile(VISIBILITY + r'(?P<name>\w+): (?P<type>.+?),?')


def models_in(source, file):
    """Every Ankurah model the Rust file at `source` declares, in order: its name, its path within the
    file (`before_d108::Scope`), its label, `file`, the line of its derive, its doc comment and its
    fields. A line of a model this cannot read stops the run, rather than leave out what is on it."""
    lines = source.read_text(encoding='utf-8').splitlines()
    models, modules, docs, attributes = [], [], [], []
    at = 0
    while at < len(lines):
        text = lines[at].strip()
        if doc := DOC.fullmatch(text):
            docs.append(doc['text'].strip())
            at += 1
        elif text.startswith('#['):
            attribute, after = attribute_at(lines, at)
            attributes.append((at + 1, attribute))
            at = after
        elif text.startswith('//'):
            at += 1
        else:
            derive = next((number for number, attribute in attributes if DERIVES_MODEL.search(attribute)), None)
            if derive is not None:
                struct = STRUCT.fullmatch(text)
                if not struct:
                    raise PageError(f'{file}:{at + 1}: the Model derive on line {derive} is not on a struct '
                                    f'{SCRIPT} can read.')
                label = next((found['label'] for _, attribute in attributes if (found := LABEL.match(attribute))),
                             None)
                path = '::'.join([*(name for _, name in modules), struct['name']])
                fields, at = fields_at(lines, at + 1, file) if struct['body'] == ' {' else ([], at + 1)
                models.append({'name': struct['name'], 'path': path, 'label': label, 'file': file, 'line': derive,
                               'doc': ' '.join(docs), 'fields': fields})
            else:
                # A module's body ends at the brace that rustfmt indents as far as the module.
                indent = len(lines[at]) - len(lines[at].lstrip())
                if module := MODULE.fullmatch(text):
                    modules.append((indent, module['name']))
                elif text == '}' and modules and modules[-1][0] == indent:
                    modules.pop()
                at += 1
            docs, attributes = [], []
    return models


def attribute_at(lines, at):
    """The attribute that starts on line `at` (counted from 0), on one line even where rustfmt wrapped it,
    and the line after it."""
    attribute, at = lines[at].strip(), at + 1
    while attribute.count('[') > attribute.count(']') and at < len(lines):
        attribute, at = attribute + lines[at].strip(), at + 1
    return attribute, at


def fields_at(lines, at, file):
    """The fields of the struct whose body starts on line `at` (counted from 0), each with its name, type
    and doc comment, and the line after the body."""
    fields, docs = [], []
    while (text := lines[at].strip()) != '}':
        if doc := DOC.fullmatch(text):
            docs.append(doc['text'].strip())
        elif field := FIELD.fullmatch(text):
            fields.append({'name': field['name'], 'type': field['type'], 'doc': ' '.join(docs)})
            docs = []
        elif not (text == '' or text.startswith('//') or (text.startswith('#[') and text.endswith(']'))):
            raise PageError(f'{file}:{at + 1}: {SCRIPT} cannot read this line of a model: {text}')
        at += 1
    return fields, at + 1


def commons_sources():
    """The directory of the Commons' sources at the revision Cargo.lock pins, that revision, and the
    address its files have at that revision."""
    metadata = subprocess.run(['cargo', 'metadata', '--format-version', '1', '--locked'], cwd=REPO, check=True,
                              stdout=subprocess.PIPE, text=True).stdout
    packages = [package for package in json.loads(metadata)['packages'] if package['name'] == 'ankurah-commons']
    if len(packages) != 1:
        raise PageError(f'cargo metadata lists {len(packages)} ankurah-commons packages, and the page describes one.')
    source = re.fullmatch(r'git\+(?P<repository>[^?#]+?)(\.git)?(\?[^#]*)?#(?P<revision>[0-9a-f]{40})',
                          packages[0]['source'] or '')
    if not source:
        raise PageError(f"ankurah-commons comes from {packages[0]['source'] or 'a path'}, not from a git "
                        'revision the page can link its sources at.')
    return (Path(packages[0]['manifest_path']).parent, source['revision'],
            f"{source['repository']}/blob/{source['revision']}")


def place(fwd, commons):
    """The page's lists of `fwd`'s models and the `commons`' models, as the mapping places them. Raises
    PageError, naming each, for the models the mapping does not place and its entries that name no model."""
    problems = [f"fwd model {m['path']} ({m['file']}:{m['line']}) is not in the mapping: add it to COUNTERPART, "
                f'or to FOR_OLDER_STORES if fwd keeps it only to read older stores, in {SCRIPT}.'
                for m in fwd if m['path'] not in COUNTERPART and m['path'] not in FOR_OLDER_STORES]
    problems += [f"Commons model {m['path']} ({m['file']}:{m['line']}) is not in the mapping: add it to "
                 f'COMMONS_USED or COMMONS_PLANNED in {SCRIPT}.'
                 for m in commons
                 if m['path'] not in COMMONS_USED and m['path'] not in COMMONS_PLANNED and namespace(m) != NOT_FOR_FWD]
    fwd_paths = {m['path'] for m in fwd}
    problems += [f'{SCRIPT} places fwd model {path}, which {FWD_MODELS} does not declare: take it out of the mapping.'
                 for path in [*COUNTERPART, *FOR_OLDER_STORES] if path not in fwd_paths]
    commons_by_path = {m['path']: m for m in commons}
    problems += [f'{SCRIPT} places Commons model {path}, which the Commons does not declare: take it out of the '
                 'mapping.' for path in [*COMMONS_USED, *COMMONS_PLANNED] if path not in commons_by_path]
    if problems:
        raise PageError('\n'.join(problems))
    return Placed(own=[m for m in fwd if m['path'] in COUNTERPART],
                  older=[m for m in fwd if m['path'] in FOR_OLDER_STORES],
                  used=[commons_by_path[path] for path in COMMONS_USED],
                  planned=[m for m in commons if m['path'] in COMMONS_PLANNED],
                  not_for_fwd=[m for m in commons if namespace(m) == NOT_FOR_FWD])


def namespace(m):
    """The Commons namespace of the Commons model `m`, which begins its label."""
    return (m['label'] or '').partition('.')[0]


def doc_html(s):
    """A doc comment as HTML: escaped, `code` as <code>, rustdoc [`links`] as code."""
    s = html.escape(s)
    s = re.sub(r"\[`([^`]+)`\](\([^)]*\))?", r'<code>\1</code>', s)
    return re.sub(r'`([^`]+)`', r'<code>\1</code>', s)


def first_sentence(s):
    """`s` up to its first full stop, colon or semicolon."""
    found = re.match(r'(.+?[.:;])(\s|$)', s)
    return found.group(1) if found else s


def fields_table(m):
    if not m['fields']:
        return '<p class=muted>No fields.</p>'
    rows = ''.join(f'<tr><td><code>{html.escape(f["name"])}</code></td><td><code>{html.escape(f["type"])}</code></td>'
                   f'<td>{doc_html(f["doc"])}</td></tr>' for f in m['fields'])
    return (f'<table class=fields><thead><tr><th>Field</th><th>Type</th><th>What it holds</th></tr></thead>'
            f'<tbody>{rows}</tbody></table>')


def section(anchor, heading, source, m, counterpart=''):
    """The card of the model `m`: its heading, where it is declared, its doc comment, its Commons
    counterpart, if any, and its fields."""
    return (f'<section class=model id="{anchor}"><h3>{heading}</h3><p class=source>{source}</p>'
            f'<p>{doc_html(m["doc"])}</p>{counterpart}<details><summary>{len(m["fields"])} fields</summary>'
            f'{fields_table(m)}</details></section>')


def fwd_source(m):
    return f'<a href="../../{m["file"]}">{m["file"]}:{m["line"]}</a>'


def commons_source(m, blob, shown):
    return f'<a href="{blob}/{m["file"]}#L{m["line"]}">{shown}</a>'


def glance_row(m):
    counterpart, note = COUNTERPART[m['path']]
    shown = f'<code>{html.escape(counterpart)}</code>' if counterpart else '<span class=muted>none</span>'
    return (f'<tr><td><a href="#fwd-{m["name"].lower()}"><code>{m["name"]}</code></a></td><td>{shown}</td>'
            f'<td>{note}</td></tr>')


def used_section(m, blob):
    return section(f'commons-{m["name"].lower()}',
                   f'<code>{m["name"]}</code> <span class=label>{html.escape(m["label"] or "")}</span>',
                   commons_source(m, blob, f'{m["file"]}:{m["line"]}'), m)


def own_section(m):
    counterpart = COUNTERPART[m['path']][0]
    shown = f'<code>{html.escape(counterpart)}</code>' if counterpart else 'nothing yet'
    return section(f'fwd-{m["name"].lower()}', f'<code>{m["path"]}</code>', fwd_source(m), m,
                   f'<p class=counterpart><strong>In the Commons:</strong> {shown}</p>')


def older_section(m):
    return section(f'fwd-legacy-{m["name"].lower()}', f'<code>{m["path"]}</code>', fwd_source(m), m)


def planned_row(m, blob):
    name = f'<code>{m["name"]}</code>'
    return (f'<tr><td>{commons_source(m, blob, name)}</td><td><code>{html.escape(m["label"] or "")}</code></td>'
            f'<td>{doc_html(first_sentence(m["doc"]))}</td><td>{html.escape(COMMONS_PLANNED[m["path"]])}</td></tr>')


STYLE = """:root { --bg: #fbfbfa; --fg: #1d1d1f; --muted: #6b6b70; --line: #e3e3e0; --card: #ffffff; --accent: #2f5fa7; --code-bg: #f1f1ee; }
@media (prefers-color-scheme: dark) { :root:not([data-theme="light"]) { --bg: #17181a; --fg: #e8e8e6; --muted: #9a9aa0; --line: #2e3033; --card: #1e2023; --accent: #8db4f0; --code-bg: #26282c; } }
:root[data-theme="dark"] { --bg: #17181a; --fg: #e8e8e6; --muted: #9a9aa0; --line: #2e3033; --card: #1e2023; --accent: #8db4f0; --code-bg: #26282c; }
* { box-sizing: border-box; }
body { margin: 0; background: var(--bg); color: var(--fg); font: 15px/1.55 -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; }
main { max-width: 1040px; margin: 0 auto; padding: 32px 16px 64px; }
h1 { font-size: 28px; margin: 0 0 6px; }
h2 { font-size: 20px; margin: 40px 0 10px; padding-top: 8px; border-top: 1px solid var(--line); }
h3 { font-size: 16px; margin: 0 0 4px; }
a { color: var(--accent); }
code { font: 13px/1.4 ui-monospace, SFMono-Regular, Menlo, monospace; background: var(--code-bg); padding: 1px 4px; border-radius: 4px; overflow-wrap: anywhere; }
.lede { color: var(--muted); margin: 0 0 18px; max-width: 760px; }
.muted { color: var(--muted); }
.counts { display: flex; flex-wrap: wrap; gap: 12px; margin: 18px 0; }
.count { background: var(--card); border: 1px solid var(--line); border-radius: 8px; padding: 10px 14px; min-width: 150px; }
.count b { display: block; font-size: 22px; }
.table-wrap { overflow-x: auto; }
table { border-collapse: collapse; width: 100%; font-size: 14px; }
th, td { text-align: left; vertical-align: top; padding: 7px 10px; border-bottom: 1px solid var(--line); }
th { color: var(--muted); font-weight: 600; }
.model { background: var(--card); border: 1px solid var(--line); border-radius: 8px; padding: 14px 16px; margin: 12px 0; }
.model p { margin: 6px 0; }
.source { font-size: 13px; color: var(--muted); }
.label { font-weight: 400; color: var(--muted); font-size: 13px; }
.counterpart { font-size: 14px; }
details summary { cursor: pointer; color: var(--muted); font-size: 13px; margin-top: 6px; }
table.fields td:first-child, table.fields td:nth-child(2), table.glance td:first-child { white-space: nowrap; }
@media (max-width: 640px) { table.fields td:nth-child(2) { white-space: normal; } table.glance, table.waiting { min-width: 620px; } }
"""


def page(placed, revision, blob):
    """The page, as HTML, for the models `placed` and the Commons at `revision`, whose files are under `blob`."""
    glance = ''.join(glance_row(m) for m in placed.own)
    used = ''.join(used_section(m, blob) for m in placed.used)
    own = ''.join(own_section(m) for m in placed.own)
    older = ''.join(older_section(m) for m in placed.older)
    planned = ''.join(planned_row(m, blob) for m in placed.planned)
    not_for_fwd = ', '.join(f'<code>{m["name"]}</code>' for m in placed.not_for_fwd)
    return f"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>fwd Data Models</title>
<style>
{STYLE}</style>
</head>
<body>
<main>
<h1>fwd Data Models</h1>
<p class="lede">Every Ankurah model fwd reads or writes, with source locations in fwd's crate or Ankurah Commons. fwd is replacing its models with Commons models, one record type at a time (<a href="../DECISIONS.md">docs/DECISIONS.md</a>, D223).</p>
<p class="muted">Generated by <a href="../../{SCRIPT}">{SCRIPT}</a> from fwd's crate and Commons at <code>{revision[:7]}</code>. Field descriptions come from model doc comments.</p>

<div class="counts">
  <div class="count"><b>{len(placed.own)}</b>fwd models</div>
  <div class="count"><b>{len(placed.used)}</b>Commons models fwd uses</div>
  <div class="count"><b>{len(placed.planned)}</b>Commons models planned for fwd</div>
</div>

<h2>At a glance</h2>
<div class="table-wrap"><table class="glance"><thead><tr><th>fwd's model</th><th>Its Commons counterpart</th><th>Note</th></tr></thead><tbody>{glance}</tbody></table></div>

<h2>Commons models fwd uses</h2>
<p>An intention requires the Commons' <code>Intention</code> and <code>Creation</code>. Add <code>Attribution</code> when recording someone else's intention, <code>LastEdit</code> after rewording and <code>Closure</code> after fulfillment or withdrawal. Store these components on one entity with fwd's <code>Placement</code> for project membership. Read Commons views directly (D213). Commons <code>identity.principal</code> is used only as the type of these components' references to people and agents; these hold fwd principal IDs until principals migrate.</p>
{used}

<h2>fwd's own models</h2>
<p>Defined in <a href="../../{FWD_MODELS}">{FWD_MODELS}</a>. Each model except <code>Project</code> requires a project reference, which <code>ProjectGraph</code> uses to filter reads.</p>
{own}
<h3 style="margin-top:24px">Kept only to read older stores</h3>
{older}

<h2>Commons models fwd does not use yet</h2>
<p><code>Claim</code>, <code>Perspective</code> and <code>Ownership</code> add new record types.</p>
<div class="table-wrap"><table class="waiting"><thead><tr><th>Model</th><th>Label</th><th>What it is</th><th>Replaces in fwd</th></tr></thead><tbody>{planned}</tbody></table></div>

<h2>Commons models not for fwd</h2>
<p>The Commons <code>{NOT_FOR_FWD}</code> namespace ({not_for_fwd}) is for other applications. fwd does not compile these models or read their records.</p>
</main>
</body>
</html>
"""


def first_difference(committed, written):
    """Where the committed page first differs from what this writes, and what to run."""
    at = next((i for i, (a, b) in enumerate(zip(committed, written)) if a != b), min(len(committed), len(written)))
    line, line_start = committed.count('\n', 0, at) + 1, committed.rfind('\n', 0, at) + 1
    start = max(line_start, at - 20)
    return (f'{PAGE} is not what {SCRIPT} writes: at line {line}, column {at - line_start + 1}, it has '
            f'{committed[start:at + 40]!r} where the script writes {written[start:at + 40]!r}. Run python3 -I '
            f'{SCRIPT} and commit the page.')


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--check', action='store_true',
                        help=f'write nothing, and fail when {PAGE} is not what this would write')
    check = parser.parse_args().check
    try:
        root, revision, blob = commons_sources()
        sources = sorted(root.glob('src/**/*.rs'), key=lambda source: source.relative_to(root).parts)
        commons = [model for source in sources for model in models_in(source, source.relative_to(root).as_posix())]
        written = page(place(models_in(REPO / FWD_MODELS, FWD_MODELS), commons), revision, blob)
    except PageError as error:
        sys.exit(str(error))
    if not check:
        (REPO / PAGE).write_text(written, encoding='utf-8')
    elif (committed := (REPO / PAGE).read_text(encoding='utf-8')) != written:
        sys.exit(first_difference(committed, written))


if __name__ == '__main__':
    main()
