// The extension's version for a build: package.json's version, then the number of commits the
// build's commit has behind it, `.g` and its short hash, and `.dirty.<UTC time>` when the checkout
// has changes not committed (`0.0.2-<commits>.g<short hash>[.dirty.20260928213000]`), so that
// `code --list-extensions --show-versions` tells one build from another. It is a semver
// prerelease, which VS Code orders by its parts: the count first, so a later build installs over
// an earlier one without `--force` (a hash alone sorts as its letters do). The VSIX is never
// published, and the Marketplace refuses only publishing a prerelease.
//
//   node version.mjs --write   works it out, writes it to dist/version.json (where the extension
//                              reads it) and prints it: `npm run build` runs this first
//   node version.mjs --read    prints the version the last build wrote: `npm run package` gives
//                              the VSIX this one, so the VSIX and its build agree
//   node version.mjs           works it out and prints it

import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const written = path.join(here, 'dist', 'version.json');

if (process.argv.includes('--read')) {
  if (!fs.existsSync(written)) throw new Error('no dist/version.json: run `npm run build` first');
  console.log(JSON.parse(fs.readFileSync(written, 'utf8')).version);
} else {
  const version = stamped();
  if (process.argv.includes('--write')) {
    fs.mkdirSync(path.dirname(written), { recursive: true });
    fs.writeFileSync(written, `${JSON.stringify({ version })}\n`);
  }
  console.log(version);
}

function stamped() {
  const base = JSON.parse(fs.readFileSync(path.join(here, 'package.json'), 'utf8')).version;
  // --no-optional-locks: asking must not rewrite the index, which a build may run beside other git work.
  const git = (...args) => execFileSync('git', ['--no-optional-locks', ...args], { cwd: here, encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] }).trim();
  let count;
  let hash;
  let dirty;
  try {
    count = git('rev-list', '--count', 'HEAD');
    hash = git('rev-parse', '--short', 'HEAD');
    dirty = git('status', '--porcelain', '--untracked-files=no') !== '';
  } catch {
    return base; // not a git checkout
  }
  const time = new Date().toISOString().replace(/[-:T]/g, '').slice(0, 14);
  return `${base}-${count}.g${hash}${dirty ? `.dirty.${time}` : ''}`;
}
