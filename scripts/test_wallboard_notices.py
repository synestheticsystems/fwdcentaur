"""The wallboard bundle's notice (wallboard_notices.py) names who made it, where its source is, and
every third-party package compiled into its WebAssembly, with the license text of each. The test
of this build's notice runs cargo, and needs the wallboard's dependencies fetched, as any build of it
does."""
from pathlib import Path
import tempfile
import unittest

import wallboard_notices

REPO = Path(__file__).resolve().parent.parent
REGISTRY = 'registry+https://github.com/rust-lang/crates.io-index'


def package(name, source=REGISTRY, kind='lib'):
    return {'id': name, 'name': name, 'version': '1.0.0', 'source': source, 'targets': [{'kind': [kind]}]}


def depends(on, kind=None):
    return {'pkg': on, 'dep_kinds': [{'kind': kind, 'target': None}]}


class Notices(unittest.TestCase):
    def test_what_is_compiled_in_is_the_normal_dependencies_less_macros_and_this_repositorys_own(self):
        found = {
            'packages': [package('fwd-web', None), package('fwd-core', None), package('serde'), package('itoa'),
                         package('serde_derive', kind='proc-macro'), package('syn'), package('cc'),
                         package('pretty_assertions')],
            'resolve': {'root': 'fwd-web', 'nodes': [
                {'id': 'fwd-web', 'deps': [depends('fwd-core'), depends('serde'), depends('cc', 'build'),
                                           depends('pretty_assertions', 'dev')]},
                {'id': 'fwd-core', 'deps': [depends('serde')]},
                {'id': 'serde', 'deps': [depends('serde_derive'), depends('itoa')]},
                {'id': 'serde_derive', 'deps': [depends('syn')]},
                *({'id': name, 'deps': []} for name in ('itoa', 'syn', 'cc', 'pretty_assertions')),
            ]},
        }
        self.assertEqual([compiled['name'] for compiled in wallboard_notices.compiled(found)], ['itoa', 'serde'])

    def test_a_package_from_git_takes_the_license_its_checkout_ships_above_it(self):
        with tempfile.TemporaryDirectory() as directory:
            checkout = Path(directory)
            (checkout / '.cargo-ok').touch()
            (checkout / 'LICENSE_MIT').write_text('MIT')
            (checkout / 'core').mkdir()
            (checkout / 'core/Cargo.toml').touch()
            from_git = dict(package('core', 'git+https://example.invalid/repository?rev=1'),
                            manifest_path=str(checkout / 'core/Cargo.toml'))
            self.assertEqual(wallboard_notices.license_files(from_git), [checkout / 'LICENSE_MIT'])
            published = dict(from_git, source=REGISTRY)
            self.assertEqual(wallboard_notices.license_files(published), [], 'a published package ships its own')

    def test_a_crate_that_ships_no_text_is_taken_under_apache_2_if_it_may_and_else_needs_a_text_vendored(self):
        with tempfile.TemporaryDirectory() as directory:
            def shipping(name, license, *files):
                (Path(directory) / name).mkdir()
                for file in files:
                    (Path(directory) / name / file).write_text('Apache License, Version 2.0')
                return dict(package(name), license=license, license_file=None, repository=None,
                            manifest_path=str(Path(directory) / name / 'Cargo.toml'))

            crates = [shipping('serde', 'MIT OR Apache-2.0', 'LICENSE-APACHE'), shipping('gloo-net', 'MIT OR Apache-2.0'),
                      shipping('mit-only', 'MIT')]
            found = {'packages': [package('fwd-web', None), *crates],
                     'resolve': {'root': 'fwd-web', 'nodes': [
                         {'id': 'fwd-web', 'deps': [depends(crate['id']) for crate in crates]},
                         *({'id': crate['id'], 'deps': []} for crate in crates)]}}
            with self.assertRaisesRegex(wallboard_notices.Uncovered, r': mit-only 1\.0\.0 \(MIT\)$'):
                wallboard_notices.notice(REPO, found)
            found['resolve']['nodes'][0]['deps'].pop()
            self.assertIn('\n- gloo-net 1.0.0, MIT OR Apache-2.0: registry+https://github.com/rust-lang/crates.io-index; '
                          'its package ships no license text: it is taken under Apache-2.0, whose text serde 1.0.0 '
                          '(LICENSE-APACHE) ships\n', wallboard_notices.notice(REPO, found))

    def test_this_builds_notice_names_its_source_and_each_crate_compiled_in_with_its_license(self):
        text = wallboard_notices.notice(REPO, wallboard_notices.metadata(REPO))
        self.assertIn('Synesthetic Systems LLC', text)
        self.assertIn(wallboard_notices.SOURCE, text)
        self.assertIn('SunCalc 2.1.1', text)
        for crate in ('wasm-bindgen', 'serde', 'leptos', 'ankurah'):
            self.assertRegex(text, rf'\n- {crate} \S+, \S', f'{crate} is compiled in')
        for macro in ('serde_derive', 'wasm-bindgen-macro', 'leptos_macro'):
            self.assertNotRegex(text, rf'\n- {macro} ', f'{macro} runs while the wallboard compiles, and is no part of it')
        self.assertIn('Apache License', text, 'the license texts the crates ship follow')
        self.assertIn('\nCopyright (c) 2022 Greg Johnston\n', text, 'the MIT notice of Leptos, whose packages ship none')


if __name__ == '__main__':
    unittest.main()
