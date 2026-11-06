import test from 'node:test';
import assert from 'node:assert/strict';
import { Diagnostics, markBoot, reasonOf } from '../../crates/fwd-web/src/wallboard-diagnostics.mjs';

test('diagnostics aggregate repeated failures into one record per area, and snapshots are copies', () => {
  let now = 100, clock = 0;
  const diagnostics = new Diagnostics(() => now, () => clock++);
  for (let i = 0; i < 1000; i++) diagnostics.issue('graph', 'invalid-snapshot');
  diagnostics.measure('snapshot', () => 'ok');
  diagnostics.clear('graph');
  assert.equal(Object.keys(diagnostics.issues).length, 1);
  assert.equal(diagnostics.issues.graph.count, 1000);
  assert.equal(diagnostics.issues.graph.active, false);
  const snapshot = diagnostics.snapshot(); snapshot.issues.graph.count = 0;
  assert.equal(diagnostics.issues.graph.count, 1000);
  assert.equal(diagnostics.timings.snapshot.lastMs, 1);
});

test('a reason names the failure without its content, and stays short', () => {
  const quota = new DOMException('Setting the value of \'fwd-wallboard-history-v1:["project"]\' exceeded the quota.', 'QuotaExceededError');
  assert.equal(reasonOf(quota), 'QuotaExceededError');
  let unparsable; try { JSON.parse('{"project_id": "a project identifier"'); } catch (error) { unparsable = error; }
  assert.equal(reasonOf(unparsable), 'SyntaxError');
  assert.equal(reasonOf(Error('Invalid card')), 'Invalid card');
  assert.equal(reasonOf(Error('x'.repeat(500))).length, 160);
  const diagnostics = new Diagnostics();
  diagnostics.issue('history', 'write-failed', reasonOf(quota));
  assert.deepEqual([diagnostics.issues.history.code, diagnostics.issues.history.reason], ['write-failed', 'QuotaExceededError']);
});

test('a success clears only the failures it disproves', () => {
  const diagnostics = new Diagnostics();
  diagnostics.issue('history', 'invalid-snapshot', 'Invalid card');
  diagnostics.clear('history', 'unreadable-cache', 'write-failed');
  assert.equal(diagnostics.issues.history.active, true);
  diagnostics.clear('history', 'invalid-snapshot');
  assert.equal(diagnostics.issues.history.active, false);
});

test('milestones keep their first time and transitions their latest, on one clock', () => {
  let clock = 10;
  const diagnostics = new Diagnostics(Date.now, () => clock);
  diagnostics.milestone('armed'); diagnostics.transition('connected');
  clock = 30; diagnostics.milestone('armed'); diagnostics.transition('connected');
  const { milestones, latest } = diagnostics.snapshot();
  assert.deepEqual(milestones, { armed: 10 });
  assert.deepEqual(latest, { connected: 30 });
});

test('boot marks each known stage once, in milliseconds since navigation start', () => {
  const before = performance.now();
  markBoot('configuration');
  const first = new Diagnostics().snapshot().boot.configuration;
  markBoot('configuration'); markBoot('unknown');
  const { boot } = new Diagnostics().snapshot();
  assert.ok(first >= before && first <= performance.now());
  assert.equal(boot.configuration, first, 'a stage keeps its first mark');
  assert.equal('unknown' in boot, false);
});

test('a settings digest tells revisions apart without recording their values', () => {
  const diagnostics = new Diagnostics();
  diagnostics.recordSettings({ theme: 'charcoal', max_rows: 5 });
  const charcoal = diagnostics.state.settingsDigest;
  diagnostics.recordSettings({ theme: 'charcoal', max_rows: 5 });
  assert.equal(diagnostics.state.settingsDigest, charcoal);
  assert.match(charcoal, /^[0-9a-f]{8}$/);
  diagnostics.recordSettings({ theme: 'sand-plum', max_rows: 5 });
  assert.notEqual(diagnostics.state.settingsDigest, charcoal);
  assert.doesNotMatch(JSON.stringify(diagnostics.snapshot()), /charcoal|sand-plum/);
});
