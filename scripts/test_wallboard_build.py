"""What the wallboard's build (build-wallboard.sh) refuses before it writes anything, what a build
that fails leaves of the bundle it found, and what a build that succeeds records."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

import wallboard_identity

REPO = Path(__file__).resolve().parent.parent
# What an earlier build left in the bundle each scratch build runs over.
EARLIER = '{"source_revision": "earlier", "content_sha256": "earlier"}\n'
# A stand-in trunk that builds a page, with the runtime in one snippet directory, as wasm-bindgen
# leaves it.
PAGE = ('mkdir -p wallboard-dist/snippets/fwd-web-0/src/wallboard\n'
        'echo "the page" > wallboard-dist/index.html\n'
        'touch wallboard-dist/snippets/fwd-web-0/src/wallboard/wallboard-runtime.mjs\n')


class OfficialBuild(unittest.TestCase):
    def test_it_refuses_the_rustflags_cargo_would_take_in_place_of_the_remapping(self):
        bash = shutil.which('bash')
        for variable in ('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS'):
            with self.subTest(variable):
                env = {name: value for name, value in os.environ.items()
                       if name not in ('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS')}
                # Set, though empty, as Cargo still takes it. The search path leaves trunk out, so that a
                # build that failed to refuse stops at its check for trunk instead of building.
                env.update({'WALLBOARD_REMAP_PATHS': '1', variable: '', 'PATH': '/usr/bin:/bin'})
                result = subprocess.run([bash, str(REPO / 'scripts/build-wallboard.sh')], cwd=REPO, env=env,
                                        capture_output=True, text=True)
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertIn(f'refuses {variable}:', result.stderr)


def scratch_build(case, trunk, notices):
    """Runs the build in a scratch checkout of its own files, over the bundle an earlier build left
    there, with `trunk` and `notices` as the bodies of stand-ins for trunk and the notice writer.
    Returns the checkout and the build's result."""
    directory = tempfile.TemporaryDirectory()
    case.addCleanup(directory.cleanup)
    checkout, tools = Path(directory.name, 'checkout'), Path(directory.name, 'tools')
    for name in ('scripts/build-wallboard.sh', 'scripts/wallboard_identity.py', 'deploy/wallboard/wallboard-contract.json',
                 'crates/fwd-web/src/wallboard/LICENSE-suncalc.txt', 'crates/fwd-web/src/wallboard/SUNCALC.md', 'LICENSE'):
        (checkout / name).parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(REPO / name, checkout / name)
    (checkout / 'deploy/wallboard/sounds').mkdir()
    (checkout / 'scripts/wallboard_notices.py').write_text(notices)
    # No GIT_* variable from the caller, such as the GIT_DIR or GIT_INDEX_FILE a hook exports, so that
    # neither the scratch commit nor the build's own git commands reach the caller's repository; none
    # of this machine's Git configuration, so that the commit neither signs nor runs hooks; and a local
    # build, whatever this machine's environment asks for.
    env = {name: value for name, value in os.environ.items()
           if not name.startswith('GIT_') and name != 'WALLBOARD_REMAP_PATHS'}
    env.update({'GIT_CONFIG_GLOBAL': os.devnull, 'GIT_CONFIG_NOSYSTEM': '1'})
    for command in (['init', '-q'], ['add', '.'],
                    ['-c', 'user.name=build', '-c', 'user.email=build@example.invalid', 'commit', '-q', '-m', 'build']):
        subprocess.run(['git', *command], cwd=checkout, env=env, check=True, capture_output=True)
    bundle = checkout / 'deploy/wallboard'
    (bundle / 'build-info.json').write_text(EARLIER)
    (bundle / 'dist').mkdir()
    (bundle / 'dist/index.html').write_text('the earlier page')
    tools.mkdir()
    (tools / 'trunk').write_text(f'#!/bin/sh\n{trunk}')
    (tools / 'trunk').chmod(0o755)
    (tools / 'python3').symlink_to(sys.executable)
    # The build empties wasm-bindgen's output under CARGO_TARGET_DIR: a target directory in the scratch
    # checkout keeps that out of a developer's own.
    env.update({'PATH': f'{tools}:/usr/bin:/bin', 'CARGO_TARGET_DIR': str(checkout / 'target')})
    result = subprocess.run([shutil.which('bash'), str(checkout / 'scripts/build-wallboard.sh')], cwd=checkout,
                            env=env, capture_output=True, text=True)
    case.assertIn('export const buildInfo', (checkout / 'crates/fwd-web/src/wallboard/wallboard-build.mjs').read_text(),
                  f'the build ran as far as trunk: {result.stderr}')
    return checkout, result


class FailedBuild(unittest.TestCase):
    def test_a_build_whose_page_fails_to_build_leaves_the_bundle_it_found_with_its_identity(self):
        checkout, result = scratch_build(self, trunk='echo "trunk: the page did not build" >&2\nexit 1\n', notices='')
        self.assertEqual(result.returncode, 1)
        self.assertIn('trunk: the page did not build', result.stderr, 'the build failed at trunk')
        bundle = checkout / 'deploy/wallboard'
        self.assertEqual((bundle / 'build-info.json').read_text(), EARLIER)
        self.assertEqual((bundle / 'dist/index.html').read_text(), 'the earlier page')

    def test_a_build_that_fails_once_it_has_replaced_the_dist_leaves_no_build_info(self):
        # A page that builds, and a notice refused, as the notice is for a crate with no license text.
        checkout, result = scratch_build(self, trunk=PAGE, notices='import sys\nsys.exit("a crate has no license text")\n')
        self.assertEqual(result.returncode, 1)
        self.assertIn('a crate has no license text', result.stderr, 'the build failed at the notice')
        bundle = checkout / 'deploy/wallboard'
        self.assertEqual((bundle / 'dist/index.html').read_text(), 'the page\n', 'the build replaced the dist')
        self.assertFalse((bundle / 'build-info.json').exists(), 'no identity names a dist it was not recorded for')


class SuccessfulBuild(unittest.TestCase):
    def test_a_build_that_succeeds_records_the_identity_its_dist_carries(self):
        checkout, result = scratch_build(self, trunk=PAGE, notices='from pathlib import Path\nimport sys\n'
                                                                  'Path(sys.argv[2]).write_text("the notice")\n')
        self.assertEqual(result.returncode, 0, result.stderr)
        bundle = checkout / 'deploy/wallboard'
        self.assertEqual((bundle / 'dist/index.html').read_text(), 'the page\n')
        recorded = wallboard_identity.module(json.loads((bundle / 'build-info.json').read_text()))
        self.assertEqual(recorded, (checkout / 'crates/fwd-web/src/wallboard/wallboard-build.mjs').read_text(),
                         'build-info.json records the identity the build wrote into the runtime')
        self.assertEqual(recorded, (bundle / 'dist/snippets/fwd-web-0/src/wallboard/wallboard-build.mjs').read_text(),
                         'and the dist carries it')

if __name__ == '__main__':
    unittest.main()
