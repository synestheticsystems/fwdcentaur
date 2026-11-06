import test from 'node:test';
import assert from 'node:assert/strict';
import { isolate, assertIsolated } from './environment.cjs';

const ROOT = '/run-root';
const DIRECTORIES = { FWD_RUNTIME_DIR: `${ROOT}/run`, FWD_DATA_DIR: `${ROOT}/data`, FWD_DEV_RUNNER_DIR: `${ROOT}/dev-runner` };
const ISOLATED = { PATH: '/bin', ...DIRECTORIES };

test('isolating takes out whatever could choose a daemon or a principal, and points fwd under the run', () => {
  const env = { PATH: '/bin', FWD_RUNTIME_DIR: '/elsewhere/run', FWD_DATA_DIR: '/elsewhere/data', FWD_AS: 'someone',
    FWD_PROJECT: 'a-project', FWD_SCOPE: 'a-scope', FWD_DAEMON_BIN: '/elsewhere/fwd', FWD_CLAUDE_SESSION: 'a-session',
    FWD_CLAUDE_AGENT: 'an-agent', CLAUDE_CODE_SESSION_ID: 'a-session', CLAUDECODE: '1', CODEX_THREAD_ID: 'a-thread' };
  isolate(env, ROOT);
  assert.deepEqual(env, ISOLATED);
  assertIsolated(env, ROOT);
});

test('the assertion refuses what isolating would never have left', () => {
  for (const name of ['FWD_AS', 'FWD_A_LATER_BUILDS_VARIABLE', 'CLAUDE_CODE_SESSION_ID', 'CODEX_THREAD_ID'])
    assert.throws(() => assertIsolated({ ...ISOLATED, [name]: 'a-value' }, ROOT), new RegExp(name));
  for (const name of Object.keys(DIRECTORIES)) {
    for (const elsewhere of ['/elsewhere', `${ROOT}/../elsewhere`, ROOT])
      assert.throws(() => assertIsolated({ ...ISOLATED, [name]: elsewhere }, ROOT), new RegExp(name));
    const { [name]: missing, ...without } = ISOLATED;
    assert.throws(() => assertIsolated(without, ROOT), new RegExp(name));
  }
});
