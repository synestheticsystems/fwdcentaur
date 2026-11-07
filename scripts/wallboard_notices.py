#!/usr/bin/env python3
"""What the wallboard's bundle says of who made it and of what others made in it: the notice
scripts/build-wallboard.sh writes into the bundle as dist/NOTICE.txt. It names fwd's copyright and
license and where the bundle's source is, then every third-party work in the bundle, SunCalc and each
crate compiled into its WebAssembly, with the license texts each ships. For a crate whose package
ships none, it takes the crate under Apache-2.0 when its license offers that, and otherwise carries
the text vendored for it under VENDORED; it refuses to write a notice that leaves a crate without one."""
from collections import namedtuple
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys

SOURCE = 'https://github.com/synestheticsystems/fwdcentaur'
# The files a package ships its license or notice in.
LICENSE_FILE = re.compile(r'^(licen[cs]e|copying|notice|unlicense)', re.IGNORECASE)
# A license text kept in crates/fwd-web/licenses for crates whose packages ship none and whose license
# offers no Apache-2.0 choice, all of them MIT, which asks that its copyright and permission notice go
# with every copy: the file, where it was taken from unchanged and at which version, and the crates,
# each at its locked version, it serves. A crate at another version is not served until its text is
# taken again at that version.
Vendored = namedtuple('Vendored', 'file source crates')
VENDORED = (
    Vendored('LICENSE-leptos.txt',
             'https://github.com/leptos-rs/leptos/blob/30fa4e039455bea69c9467e8ec45cad7fce0722f/LICENSE, the '
             'commit leptos 0.8.20 was published from; the file is the same at each commit the others were',
             ('any_spawner 0.3.0', 'const_str_slice_concat 0.1.0', 'either_of 0.1.9', 'hydration_context 0.3.1',
              'leptos 0.8.20', 'leptos_config 0.8.10', 'leptos_dom 0.8.8', 'leptos_hot_reload 0.8.6',
              'leptos_server 0.8.7', 'next_tuple 0.1.0', 'oco_ref 0.2.1', 'or_poisoned 0.1.0', 'reactive_graph 0.2.14',
              'reactive_stores 0.4.3', 'server_fn 0.8.13', 'tachys 0.2.18', 'throw_error 0.3.1')),
    Vendored('LICENSE-rstml.txt',
             'https://github.com/rs-tml/rstml/blob/dfa36937d343340916cae8e13b9a9c4315acdc0e/LICENSE, the commit '
             'rstml 0.12.1 was published from', ('rstml 0.12.1',)),
    Vendored('LICENSE-y-crdt.txt',
             'https://github.com/y-crdt/y-crdt/blob/7dbc9d402533af3efa1e936342abeec499988b6b/LICENSE, the commit '
             'yrs 0.24.0 was published from', ('yrs 0.24.0',)),
    # erased's repository holds no license text at 60eebad, the commit erased 0.1.2 was published from,
    # nor since: its manifest alone says MIT, and names its authors.
    Vendored('LICENSE-MIT-spdx.txt',
             'https://github.com/spdx/license-list-data/blob/v3.29.0/text/MIT.txt, the MIT License as SPDX '
             'publishes it, since erased\'s repository holds no license text; its manifest names its authors, '
             'Jonathan Brouwer and Jonathan Dönszelmann', ('erased 0.1.2',)),
)


class Uncovered(Exception):
    """Crates compiled in whose packages ship no license text, whose license offers no Apache-2.0
    choice, and for which no text is vendored."""


def metadata(repo):
    """Cargo's account of the wallboard's build in the checkout at `repo`: wasm32, with the feature."""
    return json.loads(subprocess.run(
        ['cargo', 'metadata', '--format-version', '1', '--locked', '--filter-platform', 'wasm32-unknown-unknown',
         '--features', 'wallboard', '--manifest-path', str(repo / 'crates/fwd-web/Cargo.toml')],
        cwd=repo, check=True, capture_output=True, text=True).stdout)


def compiled(found):
    """The third-party packages compiled into the WebAssembly: the web crate's normal dependencies,
    taken transitively, less the procedural macros, which run while it compiles and put no code of
    their own into it, and less this repository's own packages."""
    packages = {package['id']: package for package in found['packages']}
    nodes = {node['id']: node for node in found['resolve']['nodes']}
    reached, todo = set(), [found['resolve']['root']]
    while todo:
        for dependency in nodes[todo.pop()]['deps']:
            normal = any(kind['kind'] is None for kind in dependency['dep_kinds'])
            macro = any('proc-macro' in target['kind'] for target in packages[dependency['pkg']]['targets'])
            if normal and not macro and dependency['pkg'] not in reached:
                reached.add(dependency['pkg'])
                todo.append(dependency['pkg'])
    third_party = [packages[key] for key in reached if packages[key]["source"] is not None]
    return sorted(third_party, key=lambda package: (package['name'], package['version']))


def license_files(package):
    """The license and notice files `package` ships: in its own directory, or for a package taken
    from a Git repository, in the nearest directory above it in its checkout that has any."""
    directory = Path(package['manifest_path']).parent
    while True:
        found = sorted(path for path in directory.iterdir() if path.is_file() and LICENSE_FILE.match(path.name))
        at_root = (directory / '.cargo-ok').exists() or directory.parent == directory
        if found or not package['source'].startswith('git+') or at_root:
            return found
        directory = directory.parent


def apache_choice(crate):
    """Whether `crate`'s license lets it be taken under Apache-2.0 alone."""
    return 'Apache-2.0' in re.split(r'\s+OR\s+|/', crate['license'] or '')


def notice(repo, found):
    """The bundle's NOTICE.txt, for the build Cargo describes in `found`."""
    crates = compiled(found)
    files = {crate['id']: license_files(crate) for crate in crates}
    # The Apache-2.0 text a crate taken under Apache-2.0 points at: the first a package ships.
    apache = next((f'{crate["name"]} {crate["version"]} ({path.name})' for crate in crates
                   for path in files[crate['id']] if path.name.lower().startswith('license-apache')), None)
    vendored = {name: entry for entry in VENDORED for name in entry.crates}
    lines = ['fwd wallboard', '',
             'Copyright (c) 2026 Synesthetic Systems LLC. Licensed under the GNU Affero General Public License,',
             'version 3 only: LICENSE, beside the launcher. Its source is at',
             f'{SOURCE}, at the commit build-info.json names as source_revision.', '',
             'It contains these works of others, each under its own license, whose texts follow.', '',
             'SunCalc 2.1.1 (suncalc.mjs, beside the runtime), BSD-2-Clause: https://github.com/mourner/suncalc', '']
    lines.append(f'The {len(crates)} crates compiled into its WebAssembly:')
    texts, served, uncovered = {}, {}, []
    for crate in crates:
        name, license = f'{crate["name"]} {crate["version"]}', crate['license'] or crate['license_file']
        if files[crate['id']]:
            unshipped = ''
        elif apache_choice(crate) and apache:
            unshipped = f'; its package ships no license text: it is taken under Apache-2.0, whose text {apache} ships'
        elif name in vendored:
            served.setdefault(vendored[name], []).append(name)
            unshipped = f'; its package ships no license text: its license\'s text follows, headed {vendored[name].file}'
        else:
            uncovered.append(f'{name} ({license})')
            continue
        lines.append(f'- {name}, {license}: {crate["repository"] or crate["source"]}{unshipped}')
        for path in files[crate['id']]:
            text = path.read_text(errors='replace').strip()
            texts.setdefault(hashlib.sha256(text.encode()).hexdigest(), (text, []))[1].append(f'{name} ({path.name})')
    if uncovered:
        raise Uncovered('these crates ship no license text, offer no Apache-2.0 choice, and have none vendored '
                        f'under VENDORED in scripts/wallboard_notices.py: {", ".join(uncovered)}')
    suncalc = (repo / 'crates/fwd-web/src/wallboard/LICENSE-suncalc.txt').read_text().strip()
    for text, shipped_by in [(suncalc, ['SunCalc 2.1.1 (LICENSE-suncalc.txt)']), *texts.values()]:
        lines += ['', '=' * 100, f'Shipped by {", ".join(shipped_by)}:', '', text]
    for entry, names in served.items():
        text = (repo / 'crates/fwd-web/licenses' / entry.file).read_text().strip()
        lines += ['', '=' * 100, f'{entry.file}, for {", ".join(names)}, taken unchanged from {entry.source}:', '', text]
    return '\n'.join(lines) + '\n'


def main():
    repo, out = Path(sys.argv[1]), Path(sys.argv[2])
    try:
        out.write_text(notice(repo, metadata(repo)))
    except Uncovered as error:
        sys.exit(f'wallboard_notices.py: {error}')


if __name__ == '__main__':
    main()
