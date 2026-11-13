"""The wallboard build's identity (wallboard_identity.py) changes whenever what Cargo builds the
wallboard from changes, and the build records it in the bundle, whole or not at all, only for a dist
that carries it. The checks of the digest run cargo and git, and need the wallboard's dependencies
fetched, as any build of it does."""
import errno
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest
from unittest import mock

import wallboard_identity

REPO = Path(__file__).resolve().parent.parent


def run(command, cwd):
    result = subprocess.run(command, cwd=cwd, capture_output=True, text=True)
    if result.returncode:
        raise AssertionError(f'{" ".join(command[:2])} failed: {result.stderr.strip()[-800:]}')
    return result.stdout


def metadata(repo):
    """Cargo's account of the wallboard's build in the checkout at `repo`: wasm32, with the feature."""
    return json.loads(run(['cargo', 'metadata', '--format-version', '1', '--locked', '--filter-platform',
                           'wasm32-unknown-unknown', '--features', 'wallboard', '--manifest-path',
                           str(repo / 'crates/fwd-web/Cargo.toml')], repo))


class BuildIdentity(unittest.TestCase):
    def test_its_inputs_hold_every_manifest_and_the_lockfile_cargo_reads_for_the_build(self):
        # Each package built from a path, the root of the workspace it inherits settings and
        # dependencies from, and the build's lockfile.
        found = metadata(REPO)
        manifests = [Path(package['manifest_path']) for package in found['packages'] if package['source'] is None]
        roots = [Path(run(['cargo', 'locate-project', '--workspace', '--message-format', 'plain',
                           '--manifest-path', str(manifest)], REPO).strip()) for manifest in manifests]
        read = {path.resolve().relative_to(REPO).as_posix()
                for path in [*manifests, *roots, Path(found['workspace_root']) / 'Cargo.lock']}
        self.assertIn('Cargo.toml', read, 'fwd-core inherits its settings and dependencies from the root manifest')
        missing = read - set(wallboard_identity.inputs(REPO))
        self.assertFalse(missing, f'Cargo reads these to build the wallboard, and the identity leaves them out: '
                                  f'{sorted(missing)}')

    def test_editing_a_check_or_the_launcher_keeps_the_digest_and_editing_the_contract_changes_it(self):
        beside = ['deploy/wallboard/wallboard.py', 'deploy/wallboard/checks/unit/test-settings.mjs',
                  'deploy/wallboard/checks/browser/test-motion.cjs']
        with tempfile.TemporaryDirectory() as directory:
            copy = Path(directory)
            for name in [*wallboard_identity.inputs(REPO), *beside]:
                (copy / name).parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(REPO / name, copy / name)
            run(['git', 'init', '-q'], copy)
            before = wallboard_identity.content_sha256(copy)
            for name in beside:
                with (copy / name).open('a') as file:
                    file.write('\n// An edit the bundle never sees.\n')
            self.assertEqual(wallboard_identity.content_sha256(copy), before, 'the bundle is not built from these')
            with (copy / 'deploy/wallboard/wallboard-contract.json').open('a') as contract:
                contract.write('\n')
            self.assertNotEqual(wallboard_identity.content_sha256(copy), before, 'the build reads the contract')

    def test_a_dependency_feature_the_core_inherits_from_the_root_manifest_changes_the_digest(self):
        with tempfile.TemporaryDirectory() as directory:
            # A checkout of the identity's inputs alone, so that Cargo can read nothing else either.
            copy = Path(directory)
            for name in wallboard_identity.inputs(REPO):
                (copy / name).parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(REPO / name, copy / name)
            run(['git', 'init', '-q'], copy)
            before = wallboard_identity.content_sha256(copy)
            self.assertEqual(before, wallboard_identity.content_sha256(REPO), 'the copy holds every input')

            def serde_json_features():
                return [node['features'] for node in metadata(copy)['resolve']['nodes'] if '#serde_json@' in node['id']]

            compiled = serde_json_features()
            manifest = copy / 'Cargo.toml'
            text, edits = re.subn(r'^serde_json(\s*)= "([^"]+)"$',
                                  r'serde_json\1= { version = "\2", features = ["arbitrary_precision"] }',
                                  manifest.read_text(), flags=re.MULTILINE)
            self.assertEqual(edits, 1, 'the root manifest names serde_json by its version alone')
            manifest.write_text(text)
            self.assertNotEqual(serde_json_features(), compiled, 'the edit changes what the build compiles')
            self.assertNotEqual(wallboard_identity.content_sha256(copy), before)


class RecordedIdentity(unittest.TestCase):
    IDENTITY = {'source_revision': 'a' * 40, 'content_sha256': 'b' * 64}

    def bundle(self):
        """A bundle whose dist holds the runtime in one snippet directory, as trunk leaves it, and the
        path of the module the build copies in beside it."""
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        bundle = Path(directory.name)
        modules = bundle / 'dist/snippets/fwd-web-0/src/wallboard'
        modules.mkdir(parents=True)
        (modules / 'wallboard-runtime.mjs').touch()
        return bundle, modules / 'wallboard-build.mjs'

    def test_an_identity_is_recorded_only_for_a_dist_that_carries_its_module(self):
        bundle, module = self.bundle()
        with self.assertRaisesRegex(ValueError, 'carries no wallboard-build.mjs'):
            wallboard_identity.record(bundle, self.IDENTITY)
        # The module another build's `identify` wrote: what the dist carries when a second build in the
        # same checkout rewrites the module after this build's `identify` and before its copy.
        module.write_text(wallboard_identity.module(dict(self.IDENTITY, content_sha256='c' * 64)))
        with self.assertRaisesRegex(ValueError, r'fwd-web-0/src/wallboard/wallboard-build\.mjs is not the module of '
                                                rf'the build with content_sha256 {"b" * 64}'):
            wallboard_identity.record(bundle, self.IDENTITY)
        self.assertFalse((bundle / 'build-info.json').exists(), 'a refused dist gets no identity')
        module.write_text(wallboard_identity.module(self.IDENTITY))
        wallboard_identity.record(bundle, self.IDENTITY)
        self.assertEqual(json.loads((bundle / 'build-info.json').read_text()), self.IDENTITY)

    def test_a_write_of_the_identity_that_fails_partway_leaves_no_build_info(self):
        bundle, module = self.bundle()
        module.write_text(wallboard_identity.module(self.IDENTITY))
        write_text = Path.write_text

        def disk_full(path, text, *arguments, **options):
            # Half the text reaches the disk before it fills.
            write_text(path, text[:len(text) // 2], *arguments, **options)
            raise OSError(errno.ENOSPC, os.strerror(errno.ENOSPC), str(path))

        with mock.patch.object(Path, 'write_text', disk_full):
            with self.assertRaises(OSError):
                wallboard_identity.record(bundle, self.IDENTITY)
        self.assertEqual([path.name for path in bundle.iterdir()], ['dist'], 'neither build-info.json nor a part of it')


if __name__ == '__main__':
    unittest.main()
