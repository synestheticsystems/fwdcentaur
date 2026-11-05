import test from 'node:test';
import assert from 'node:assert/strict';
import { LocalHistory, changes } from '../../crates/fwd-web/src/wallboard-history.mjs';
import { Diagnostics } from '../../crates/fwd-web/src/wallboard-diagnostics.mjs';
import { buildInfo } from '../../crates/fwd-web/src/wallboard-build.mjs';

class Storage {
  values = new Map();
  get length() { return this.values.size; }
  key(i) { return [...this.values.keys()][i]; }
  getItem(key) { return this.values.get(key) ?? null; }
  setItem(key, value) { this.values.set(key, value); }
  removeItem(key) { this.values.delete(key); }
}
const frame = source => ({ source });
const issue = diagnostics => { const { code, reason, active } = diagnostics.issues.history; return [code, reason, active]; };
const spans = history => history.groups().map(group => [group.before, group.after, group.count]);

test('history is bounded by count/bytes and separated by display context, with no age cutoff', () => {
  const now = 100000000, storage = new Storage();
  const history = new LocalHistory('project-A/open/440/8', storage, () => now);
  history.append(frame('old'), frame('retained'), now - 48 * 3600000);
  history.save();
  const daysLater = new LocalHistory('project-A/open/440/8', storage, () => now + 72 * 3600000);
  assert.deepEqual(daysLater.entries, history.entries, 'quiet days and reloads must not age history out');
  for (let i = 0; i < 140; i++) history.append(frame('before'), frame('after'), now - 1000 * (140 - i));
  assert.equal(history.entries.length, 120);
  assert.ok(history.entries.every(entry => entry.after === 'after'), 'oldest entries leave only when the count cap is reached');
  for (let i = 0; i < 20; i++) history.append(frame('界'.repeat(40000)), frame('new'.repeat(40000)), now - i);
  const encoded = history.encode();
  assert.ok(new TextEncoder().encode(encoded).length <= 2 * 1024 * 1024);
  assert.ok(encoded.length * 2 <= 2 * 1024 * 1024);
  history.save();
  const restored = new LocalHistory('project-A/open/440/8', storage, () => now);
  assert.deepEqual(restored.entries, history.entries);
  assert.equal(new LocalHistory('project-B/open/440/8', storage, () => now).entries.length, 0);
  assert.equal(storage.length, 0, 'only one display configuration persists at an origin');
});

test('history groups only contemporaneous changes without compressing busy periods into overviews', () => {
  const now = 100000000, history = new LocalHistory('fixture', new Storage(), () => now);
  for (let i = 0; i < 20; i++) history.append(frame(`before${i}`), frame(`after${i}`), now - 110 * 60000 + i * 5 * 60000);
  const groups = history.groups();
  assert.equal(groups.length, 20);
  assert.equal(groups[0].before, 'before0');
  assert.equal(groups.at(-1).after, 'after19');
  assert.equal(groups.reduce((sum, group) => sum + group.count, 0), 20);
  const contemporaneous = new LocalHistory('fixture2', new Storage(), () => now);
  contemporaneous.append(frame('a'), frame('b'), now - 10000);
  contemporaneous.append(frame('b'), frame('c'), now);
  assert.deepEqual(contemporaneous.groups().map(g => [g.before, g.after, g.count]), [['a','c',2]]);
});

test('an entry records the build that wrote it, and keeps it through a save and a load', () => {
  const now = 100000000, storage = new Storage();
  const history = new LocalHistory('fixture', storage, () => now, undefined, 'build-a');
  history.append(frame('a'), frame('b'), now);
  assert.deepEqual(history.entries, [{ at: now, build: 'build-a', before: 'a', after: 'b' }]);
  history.save();
  const upgraded = new LocalHistory('fixture', storage, () => now, undefined, 'build-b');
  assert.deepEqual(upgraded.entries, history.entries, 'a later build keeps what an earlier one saved');
  const running = new LocalHistory('fixture', new Storage(), () => now);
  running.append(frame('a'), frame('b'), now);
  assert.equal(running.entries[0].build, buildInfo.content_sha256, 'unless told otherwise, the build this page runs');
});

test('a replay group holds entries of one build, since the revisions of two builds do not compare', () => {
  const now = 100000000;
  // A saved cache with one entry per build named, all within 30 seconds. An entry given no build holds
  // no such field.
  const groupsOf = (...builds) => {
    const storage = new Storage();
    const entries = builds.map((build, i) => ({ at: now - 20000 + i * 5000, build, before: `before${i}`, after: `after${i}` }));
    storage.setItem('fwd-wallboard-history-v1:fixture', JSON.stringify({ version: 1, entries }));
    return spans(new LocalHistory('fixture', storage, () => now));
  };
  const alone = i => [`before${i}`, `after${i}`, 1];
  const merged = (first, last) => [`before${first}`, `after${last}`, last - first + 1];
  assert.deepEqual(groupsOf('build-a', 'build-b'), [alone(0), alone(1)], 'two builds stay two groups');
  assert.deepEqual(groupsOf('build-a', 'build-a'), [merged(0, 1)], 'one build merges as before');
  assert.deepEqual(groupsOf(undefined, 'build-a'), [alone(0), alone(1)], 'no build stays apart from a build after it');
  assert.deepEqual(groupsOf('build-a', undefined), [alone(0), alone(1)], 'and from a build before it, as after a rollback');
  assert.deepEqual(groupsOf(undefined, undefined), [merged(0, 1)], 'two with no build merge as before');
  assert.deepEqual(groupsOf(undefined, undefined, 'build-a', 'build-a'), [merged(0, 1), merged(2, 3)], 'each side of an upgrade still merges');
  assert.deepEqual(groupsOf('build-a', 'build-b', 'build-a'), [alone(0), alone(1), alone(2)], 'a group ends at another build for good');
});

test('a reload of one build continues a group through the saved cache, and an upgrade ends it', () => {
  const now = 100000000;
  const reloadedInto = build => {
    const storage = new Storage();
    const before = new LocalHistory('fixture', storage, () => now, undefined, 'build-a');
    before.append(frame('a'), frame('b'), now - 10000); before.save();
    const after = new LocalHistory('fixture', storage, () => now, undefined, build);
    after.append(frame('b'), frame('c'), now);
    return spans(after);
  };
  assert.deepEqual(reloadedInto('build-a'), [['a', 'c', 2]]);
  assert.deepEqual(reloadedInto('build-b'), [['a', 'b', 1], ['b', 'c', 1]]);
});

test('a cache whose entries record no build loads whole, and a save keeps it under its key and version', () => {
  const now = 100000000, diagnostics = new Diagnostics(), storage = new Storage();
  const key = 'fwd-wallboard-history-v1:fixture';
  const entries = [{ at: now - 20000, before: 'a', after: 'b' }, { at: now - 10000, before: 'b', after: 'c' }];
  storage.setItem(key, JSON.stringify({ version: 1, entries }));
  const history = new LocalHistory('fixture', storage, () => now, diagnostics, 'build-a');
  assert.deepEqual(history.entries, entries);
  assert.equal(history.persisted, true);
  assert.equal(diagnostics.issues.history, undefined, 'nothing about such a cache is an issue');
  history.append(frame('c'), frame('d'), now);
  assert.deepEqual(spans(history), [['a', 'c', 2], ['c', 'd', 1]]);
  history.save();
  assert.deepEqual(JSON.parse(storage.getItem(key)),
    { version: 1, entries: [...entries, { at: now, build: 'build-a', before: 'c', after: 'd' }] });
});

test('storage failure or malformed saved history cannot stop live presentation, and each is diagnosed', () => {
  const diagnostics = new Diagnostics(), storage = new Storage();
  storage.setItem('fwd-wallboard-history-v1:bad', '{invalid "Saved task title"');
  const history = new LocalHistory('bad', storage, Date.now, diagnostics);
  assert.equal(history.entries.length, 0);
  assert.deepEqual(issue(diagnostics), ['unreadable-cache', 'Unparsable history cache', true], 'the reason never quotes saved content');
  const working = storage.setItem;
  storage.setItem = () => { throw Error('quota'); };
  history.append(frame('before'), frame('after'), Date.now()); history.save();
  assert.equal(history.persisted, false);
  assert.equal(history.entries.length, 1);
  assert.deepEqual(issue(diagnostics), ['write-failed', 'quota', true]);
  storage.setItem = working; history.save();
  assert.deepEqual(issue(diagnostics), ['write-failed', 'quota', false], 'a later write ends the failure');
});

test('unusable saved entries and a missing store are diagnosed, and a save keeps a replay finding', () => {
  const now = 100000000, diagnostics = new Diagnostics(), storage = new Storage();
  storage.setItem('fwd-wallboard-history-v1:mixed', JSON.stringify({ version: 1,
    entries: [{ at: now, before: 'a', after: 'b' }, { at: 'yesterday', before: 'Saved task title' }] }));
  const history = new LocalHistory('mixed', storage, () => now, diagnostics);
  assert.equal(history.entries.length, 1);
  assert.deepEqual(issue(diagnostics), ['invalid-entry', '1 of 2 entries unusable', true]);
  history.save();
  assert.equal(diagnostics.issues.history.active, false, 'rewriting the cache drops the unusable entries');
  diagnostics.issue('history', 'invalid-snapshot', 'Invalid card'); history.save();
  assert.deepEqual(issue(diagnostics), ['invalid-snapshot', 'Invalid card', true], 'the unreadable snapshot is still saved');
  const unstored = new Diagnostics();
  new LocalHistory('none', undefined, () => now, unstored).save();
  assert.equal(unstored.issues.history.code, 'storage-unavailable');
});

test('semantic revisions detect edits without treating count/layout changes as edits', () => {
  const node = revision => ({ revision });
  const before = { nodes: new Map([['a',node('1')],['b',node('1')]]), edges: new Map() };
  const countsOnly = { nodes: new Map([['a',{...node('1'),messageCount:9}],['b',node('1')]]), edges: new Map() };
  assert.equal(changes(before, countsOnly).any, false);
  const after = { nodes: new Map([['a',node('2')],['c',node('1')]]), edges: new Map() };
  assert.deepEqual(changes(before, after), { arrived:['c'], departed:['b'], edited:['a'], any:true });
  // A revision states the relations a card's task holds, so an arrow that changes with neither card's
  // revision is no edit of either, though the frame is still worth drawing.
  const relation = { nodes: before.nodes, edges: new Map([['a>b',{from:'a',to:'b'}]]) };
  assert.deepEqual(changes(before, relation), { arrived:[], departed:[], edited:[], any:true });
  assert.deepEqual(changes(relation, before), { arrived:[], departed:[], edited:[], any:true });
  const proposal = { nodes: before.nodes, edges: new Map([['a>b?',{from:'a',to:'b'}]]) };
  assert.deepEqual(changes(proposal, relation), { arrived:[], departed:[], edited:[], any:true });
  // A real relation changes both cards' revisions, and that edits both whatever else the frame holds.
  const relatedCards = { nodes: new Map([['a',node('2')],['b',node('2')]]), edges: relation.edges };
  assert.deepEqual(changes(before, relatedCards), { arrived:[], departed:[], edited:['a','b'], any:true });
  const relatedWithArrival = { nodes: new Map([...relatedCards.nodes, ['c',node('1')]]), edges: relation.edges };
  assert.deepEqual(changes(before, relatedWithArrival), { arrived:['c'], departed:[], edited:['a','b'], any:true });
  const withDeparting = { nodes: new Map([...before.nodes, ['c',node('1')]]), edges: new Map() };
  assert.deepEqual(changes(withDeparting, relatedCards), { arrived:[], departed:['c'], edited:['a','b'], any:true });
  // Arrows that come or go with a card are that card's change, not an edit of the cards they join:
  // whether one reaches a card that stays, or a chain through the card replaces the arrow it implies.
  const withC = new Map([...before.nodes, ['c',node('1')]]);
  const joined = { nodes: withC, edges: new Map([['c>a',{from:'c',to:'a'}]]) };
  assert.deepEqual(changes(before, joined), { arrived:['c'], departed:[], edited:[], any:true });
  assert.deepEqual(changes(joined, before), { arrived:[], departed:['c'], edited:[], any:true });
  const chained = { nodes: withC, edges: new Map([['a>c',{from:'a',to:'c'}],['c>b',{from:'c',to:'b'}]]) };
  assert.deepEqual(changes(relation, chained), { arrived:['c'], departed:[], edited:[], any:true });
  assert.deepEqual(changes(chained, relation), { arrived:[], departed:['c'], edited:[], any:true });
});
