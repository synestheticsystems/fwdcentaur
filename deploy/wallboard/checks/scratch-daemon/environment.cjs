// What keeps a scratch integration run from touching a daemon it did not start. Everything the run
// starts inherits this process's environment, so that is the one that gets isolated, before anything
// starts.
const assert = require('node:assert/strict');
const fs = require('node:fs'), os = require('node:os'), path = require('node:path');

// What could choose the daemon or the principal a fwd command speaks to, and the session it says it
// belongs to. By prefix, so that a later build's new variable is gone too.
const IDENTITY = /^(FWD_|CLAUDE|CODEX_)/;
const SCRATCH_DIRECTORIES = { FWD_RUNTIME_DIR: 'run', FWD_DATA_DIR: 'data', FWD_DEV_RUNNER_DIR: 'dev-runner' };

// Takes everything `IDENTITY` names out of `env`, and points fwd's directories under `root`.
function isolate(env, root) {
  for (const name of Object.keys(env)) if (IDENTITY.test(name)) delete env[name];
  for (const [name, directory] of Object.entries(SCRATCH_DIRECTORIES)) env[name] = path.join(root, directory);
}

// Refuses `env` unless fwd's directories all lie under `root` and it holds nothing else `IDENTITY` names.
function assertIsolated(env, root) {
  for (const name of Object.keys(env)) {
    assert.ok(!IDENTITY.test(name) || name in SCRATCH_DIRECTORIES, `${name} would reach this run's processes`);
  }
  for (const name of Object.keys(SCRATCH_DIRECTORIES)) {
    const inside = path.relative(root, env[name] ?? '');
    assert.ok(inside && !inside.startsWith('..') && !path.isAbsolute(inside), `${name} is not under ${root}: ${env[name]}`);
  }
}

// The fwd binaries a run was told to use, read before its environment is touched.
function requestedBinaries() {
  const binary = name => {
    if (!process.env[name]) throw Error('Set SCRATCH_DAEMON to the fwd binary to run as a scratch daemon, and SCRATCH_CLIENT to fwd built from this checkout');
    const resolved = path.resolve(process.env[name]);
    fs.accessSync(resolved, fs.constants.X_OK);
    return resolved;
  };
  return { daemon: binary('SCRATCH_DAEMON'), client: binary('SCRATCH_CLIENT') };
}

// Makes this run's temporary directory, and isolates this process's environment to it, asserted.
function openEnvironment() {
  const root = fs.mkdtempSync(path.join(fs.realpathSync(os.tmpdir()), 'fwd-wallboard-scratch-'));
  isolate(process.env, root);
  assertIsolated(process.env, root);
  return root;
}

module.exports = { isolate, assertIsolated, requestedBinaries, openEnvironment };
