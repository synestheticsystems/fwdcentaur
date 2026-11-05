#!/usr/bin/env python3
"""Build the wallboard and run every check of it that needs no daemon, with synthetic data only:
the Rust tests and lints of the web crate with and without the wallboard feature, the launcher's
tests, the build identity's tests, the runtime's unit tests and the browser checks against the
production WASM. It never contacts a daemon. docs/design/wallboard.md says what each check covers."""
import argparse
from collections import namedtuple
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time

TRUNK = 'trunk 0.21.14'
NODE_MAJOR = 22
# Launches the browser the browser checks use, for its version. The named module chooses it.
CHROMIUM_VERSION = """
require(process.argv[1]).launchChromium()
  .then(async browser => { console.log(browser.version()); await browser.close(); });
"""
# One check of the suite. A command that depends on what the build writes is a function of the
# directory the check runs in, called when the check runs.
Check = namedtuple('Check', 'name command cwd')


def runtime_unit_tests(repo):
    tests = sorted(str(path.relative_to(repo)) for path in (repo / 'deploy/wallboard').glob('test-*.mjs'))
    return ['node', '--test', *tests]


def main():
    repo = Path(__file__).resolve().parent.parent
    deploy = repo / 'deploy/wallboard'
    # What the checks keep between runs: Playwright's browser, and their report.
    output = repo / 'target/wallboard'
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--install-browser', action='store_true',
                        help=f'Install Playwright\'s pinned Chromium into {output / "ms-playwright"}')
    args = parser.parse_args()
    output.mkdir(parents=True, exist_ok=True)
    # Playwright's browser stays in this repository's target directory, never the user-wide cache.
    env = {**os.environ, 'PLAYWRIGHT_BROWSERS_PATH': str(output / 'ms-playwright')}

    def save():
        (output / 'checks.json').write_text(json.dumps(report, indent=2) + '\n')

    def fail(message):
        report['error'] = message
        save()
        parser.error(message)

    def record_browser():
        report['tools']['playwright'] = json.loads((deploy / 'node_modules/playwright/package.json').read_text())['version']
        chromium = subprocess.run(['node', '-e', CHROMIUM_VERSION, str(deploy / 'fixture-server.cjs')],
                                  cwd=deploy, env=env, capture_output=True, text=True)
        if chromium.returncode:
            fail('Chromium could not be launched: rerun with --install-browser to install Playwright\'s pinned build '
                 f'into {output / "ms-playwright"}, or set CHROME. {chromium.stderr.strip()[-400:]}')
        report['tools']['chromium'] = chromium.stdout.strip()

    def record_build():
        report['build'] = json.loads((deploy / 'build-info.json').read_text())

    crate = ['--locked', '--manifest-path', 'crates/fwd-web/Cargo.toml']
    wasm = ['cargo', 'clippy', *crate, '--target', 'wasm32-unknown-unknown']
    # Everything the suite does, in order. Each check names itself once, here; the plain functions
    # between them record or complete what the checks use.
    steps = [
        Check('Install locked browser test dependencies', ['npm', 'ci', '--ignore-scripts', '--no-audit', '--no-fund'], deploy),
        *([Check('Install pinned Chromium', ['node', 'node_modules/playwright/cli.js', 'install', 'chromium'], deploy)]
          if args.install_browser else []),
        record_browser,
        Check('Rust behavior and layout', ['cargo', 'test', *crate, '--features', 'wallboard'], repo),
        Check('The web crate without the wallboard feature', ['cargo', 'test', *crate], repo),
        Check('Clippy for wasm32', [*wasm, '--', '-D', 'warnings'], repo),
        Check('Clippy for wasm32 with the wallboard feature', [*wasm, '--features', 'wallboard', '--', '-D', 'warnings'], repo),
        Check('Rust formatting', ['cargo', 'fmt', '--all', '--check', '--manifest-path', 'crates/fwd-web/Cargo.toml'], repo),
        Check('Host configuration', [sys.executable, '-m', 'unittest', 'discover', '-s',
                                     'deploy/wallboard', '-p', 'test_*.py', '-v'], repo),
        Check('Build identity', [sys.executable, '-m', 'unittest', 'discover', '-s', 'scripts',
                                 '-p', 'test_wallboard_identity.py', '-v'], repo),
        # The build writes the two modules the runtime's unit tests import, so it comes before them.
        Check('Production WASM build', ['bash', 'scripts/build-wallboard.sh'], repo),
        record_build,
        Check('Runtime unit tests', runtime_unit_tests, repo),
        *(Check(f'{path.name.removeprefix("test-").removesuffix("-browser.cjs").title()} browser integration',
                ['node', str(path.relative_to(repo))], repo)
          for path in sorted(deploy.glob('test-*-browser.cjs'))),
    ]
    # Until the last check passes, the report says it has not: a run that stops early shows what
    # failed and what never ran, never an earlier run's result.
    report = {'passed': False, 'planned': [step.name for step in steps if isinstance(step, Check)],
              'checks': [], 'tools': {}}
    save()
    for command in ('git', 'cargo', 'trunk', 'node', 'npm'):
        if not shutil.which(command):
            fail(f'Missing {command}; see deploy/wallboard/README.md, What you need')
    report['revision'] = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=repo, text=True).strip()
    # Dirty when what was checked differs from that revision.
    report['dirty'] = bool(subprocess.check_output(['git', 'status', '--porcelain'], cwd=repo, text=True))
    report['tools'].update({name: subprocess.check_output([name, '--version'], text=True).strip()
                            for name in ('node', 'npm', 'trunk')})
    report['tools']['rustc'] = subprocess.check_output(['rustc', '--version'], cwd=repo, text=True).strip()
    if report['tools']['trunk'] != TRUNK:
        fail(f'This build pins {TRUNK}; install it or explicitly update the pin in scripts/check-wallboard.py')
    if int(report['tools']['node'].lstrip('v').split('.')[0]) < NODE_MAJOR:
        fail(f'Node {NODE_MAJOR} or newer is required; found {report["tools"]["node"]}')

    def run(check):
        print(f'\n{check.name}', flush=True)
        command = check.command(check.cwd) if callable(check.command) else check.command
        start = time.monotonic()
        result = subprocess.run(command, cwd=check.cwd, env=env)
        report['checks'].append({'name': check.name, 'seconds': round(time.monotonic() - start, 3),
                                 'exit_code': result.returncode})
        save()
        if result.returncode:
            raise SystemExit(result.returncode)

    for step in steps:
        if isinstance(step, Check):
            run(step)
        else:
            step()
    report['passed'] = True
    save()
    print(f'\nAll checks passed. Report: {output / "checks.json"}')


if __name__ == '__main__':
    main()
