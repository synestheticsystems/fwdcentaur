// Settings revisions through the runtime's production entry points, mount and update, as Rust
// calls them. Synthetic graph, virtual clock; no daemon connection.
const assert = require('node:assert/strict');
const { startFixtureServer, launchChromium, pauseClock, closeServer } = require('./fixture-server.cjs');

(async () => {
  const server = await startFixtureServer();
  const browser = await launchChromium().catch(async error => { await closeServer(server); throw error; });
  try {
    const page = await browser.newPage({ viewport: { width: 1920, height: 1080 } });
    const errors = []; page.on('pageerror', error => errors.push(String(error)));
    await page.clock.install({ time: new Date('2026-11-01T12:00:00Z') });
    await page.goto(server.url);
    await page.waitForFunction(() => window.fixture);
    await pauseClock(page);
    // Rust mounts one runtime on the graph host, then sends each frame with its settings revision.
    await page.evaluate(async () => {
      window.entry = await import('/crates/fwd-web/src/wallboard/wallboard-runtime.mjs');
      fixture.runtime.dispose();
      fixture.settings.statuses = ['open', 'in_progress'];
      window.host = document.querySelector('.wallboard-graph');
      window.revision = changes => JSON.stringify({ ...fixture.settings, ...changes });
      entry.mount(host, revision({}), fixture.layout);
    });
    const send = (tasks, changes = {}, connected = true) => page.evaluate(({ tasks, changes, connected }) =>
      entry.update(host, fixture.snapshot(tasks), true, connected, revision(changes)), { tasks, changes, connected });
    const inspect = () => page.evaluate(() => {
      const r = entry.runtimeFor(host);
      return { banner: r.banner.textContent, cards: [...r.nodes.keys()].sort(), departing: r.departures.size,
        wave: Boolean(r.wave), history: r.history.entries.length, statuses: r.settings.statuses,
        issue: r.diagnostics.issues.configuration ?? null, graphIssue: r.diagnostics.issues.graph ?? null,
        project: r.settings.project_id, historyKey: r.history.key, theme: r.root.dataset.theme,
        connected: r.diagnostics.state.connected, width: r.settings.node_width, cardWidth: r.nodes.get('a')?.node.width,
        what: r.nodes.get('a')?.dom.querySelector('.wallboard-what').textContent, syncing: Boolean(r.syncing),
        pending: Boolean(r.pending), latest: r.diagnostics.latest, cues: window.cues };
    });
    const tasks = [{ id: 'a', title: 'Open work', status: 'open', priority: 1 },
      { id: 'b', title: 'Work in progress', status: 'in_progress', priority: 2 }];
    await send(tasks); await page.clock.runFor(800);
    let state = await inspect();
    assert.deepEqual(state.cards, ['a', 'b']); assert.match(state.banner, /^LIVE Now Current graph$/);

    // A revision that drops a status arrives with the frame Rust drew under it, without card a.
    // Card a must not depart as a live change: the revision is refused and its frame held.
    await send(tasks.slice(1), { statuses: ['in_progress'] }); await page.clock.runFor(2000);
    state = await inspect();
    assert.deepEqual(state.cards, ['a', 'b']); assert.equal(state.departing, 0); assert.equal(state.wave, false);
    assert.equal(state.history, 0); assert.deepEqual(state.statuses, ['open', 'in_progress']);
    assert.match(state.banner, /^STALE Last view Settings update refused$/);
    assert.deepEqual([state.issue.code, state.issue.reason, state.issue.active],
      ['revision-rejected', 'Query changes require replacing the data subscription first', true]);
    // A revision that names another project is refused by the same rule, and changes nothing.
    const refused = state;
    await send(tasks.slice(1), { project_id: 'another-project' }); await page.clock.runFor(2000);
    state = await inspect();
    assert.deepEqual(state.cards, ['a', 'b']); assert.equal(state.departing, 0); assert.equal(state.wave, false);
    assert.equal(state.project, 'fixture-project'); assert.equal(state.historyKey, refused.historyKey);
    assert.match(state.banner, /^STALE Last view Settings update refused$/);
    assert.deepEqual([state.issue.reason, state.issue.count],
      ['Query changes require replacing the data subscription first', refused.issue.count + 1]);
    // Transport health still follows the held frames.
    await send(tasks.slice(1), { statuses: ['in_progress'] }, false);
    state = await inspect(); assert.equal(state.connected, false); assert.match(state.banner, /^OFFLINE/);
    // The held revision returns with the connection: frames apply again and the issue clears.
    await send(tasks); await page.clock.runFor(3000);
    state = await inspect();
    assert.equal(state.issue.active, false); assert.equal(state.connected, true);
    assert.match(state.banner, /^LIVE Now Current graph$/); assert.deepEqual(state.cards, ['a', 'b']);
    console.log('PASS settings through update(): a refused selection or project holds its frame as STALE while transport updates; the held revision recovers');

    // The same refusal with no disconnect at all: the last view stays through a fresh baseline wait,
    // what changed meanwhile appears as silent catch-up, and only the next edit after that is live.
    await page.evaluate(() => {
      window.cues = 0; entry.runtimeFor(host).sound.play = async () => { cues++; return true; };
    });
    const edited = what => [{ ...tasks[0], what }, tasks[1]];
    const shown = state.what;
    await send(tasks, { statuses: ['in_progress'] }); await page.clock.runFor(1000);
    const held = await inspect();
    assert.match(held.banner, /^STALE Last view Settings update refused$/);
    await send(edited('Changed while the revision was refused')); await page.clock.runFor(749);
    state = await inspect();
    assert.equal(state.what, shown); assert.equal(state.syncing, true);
    assert.match(state.banner, /^SYNCING Catching up Keeping the last view$/);
    await page.clock.runFor(1);
    state = await inspect();
    assert.equal(state.what, shown, 'the baseline has ended, and the catch-up is still to come');
    assert.equal(state.syncing, false); assert.equal(state.pending, true);
    assert.ok(state.latest.baselineEnded > held.latest.baselineEnded, 'a held frame moves the mark');
    await page.clock.runFor(1050);
    state = await inspect();
    assert.equal(state.what, 'Changed while the revision was refused'); assert.equal(state.cues, 0);
    assert.match(state.banner, /^LIVE Now Current graph$/);
    assert.equal(state.latest.catchUpApplied - state.latest.baselineEnded, 1050);
    assert.deepEqual([state.latest.connected, state.latest.disconnected], [held.latest.connected, held.latest.disconnected],
      'with no reconnection');
    await send(edited('A later live edit')); await page.clock.runFor(1100);
    state = await inspect();
    assert.equal(state.what, 'A later live edit'); assert.equal(state.cues, 1);
    console.log('PASS refused frame, then recovery with no disconnect: the last view stays through the baseline wait, catch-up is silent, the next live edit is audible');

    // An unchanged revision is compared as text, not validated again with every frame.
    const validations = await page.evaluate(() => {
      const r = entry.runtimeFor(host), read = r.readRevision.bind(r); let count = 0;
      r.readRevision = text => { count++; return read(text); };
      for (let i = 0; i < 5; i++) entry.update(host, r.lastSource, true, true, revision({}));
      entry.update(host, r.lastSource, true, true, revision({ theme: 'charcoal' }));
      return count;
    });
    assert.equal(validations, 1, 'only the changed revision is validated');
    assert.equal(await page.locator('.wallboard').getAttribute('data-theme'), 'charcoal');

    // A geometry revision starts a fresh history context without first saving the one it discards.
    const geometry = await page.evaluate(() => {
      const r = entry.runtimeFor(host), before = r.history.key, writes = [];
      r.history.append(r.live, r.live, Date.now()); r.history.save();
      const setItem = Storage.prototype.setItem;
      Storage.prototype.setItem = function (key, value) { writes.push(key); return setItem.call(this, key, value); };
      try { entry.update(host, r.lastSource, true, true, revision({ theme: 'charcoal', max_rows: 6 })); }
      finally { Storage.prototype.setItem = setItem; }
      return { changed: r.history.key !== before, writes, discarded: localStorage.getItem(before) === null,
        entries: r.history.entries.length };
    });
    assert.deepEqual(geometry, { changed: true, writes: [], discarded: true, entries: 0 });
    console.log('PASS settings through update(): unchanged revisions skip validation; a geometry change discards history without saving it first');

    // A valid geometry revision with a frame that cannot be read is the graph's failure, not the
    // settings': it ends a standing refusal, stays unapplied whole, and the next readable frame under
    // it applies it, quietly, with no disconnect.
    await page.evaluate(() => { Object.assign(fixture.settings, { theme: 'charcoal', max_rows: 6 }); });
    await send(tasks); await page.clock.runFor(1500);
    await page.evaluate(() => { window.cues = 0; });
    await send(tasks, { statuses: ['in_progress'] });
    assert.equal((await inspect()).issue.active, true);
    const identity = () => page.evaluate(() => {
      const { history, diagnostics } = entry.runtimeFor(host);
      return { history: history.key, digest: diagnostics.state.settingsDigest,
        revisions: diagnostics.state.settingsRevision, connected: diagnostics.latest.connected };
    });
    const standing = await identity();
    await page.evaluate(() => {
      Object.assign(fixture.settings, { node_width: 480, theme: 'sand-plum' });
      entry.update(host, '<broken', true, true, revision({}));
    });
    state = await inspect();
    assert.match(state.banner, /^STALE Last view Graph update failed$/);
    assert.equal(state.issue.active, false);
    assert.deepEqual([state.graphIssue.code, state.graphIssue.reason, state.graphIssue.active],
      ['invalid-snapshot', 'Invalid SVG snapshot', true]);
    assert.deepEqual([state.width, state.cardWidth, state.theme], [440, 440, 'charcoal']);
    assert.deepEqual(await identity(), standing);
    await send(tasks);
    state = await inspect();
    assert.deepEqual([state.width, state.cardWidth, state.theme], [480, 480, 'sand-plum']);
    assert.equal(state.graphIssue.active, false); assert.match(state.banner, /^SYNCING/);
    await page.clock.runFor(800);
    state = await inspect();
    assert.match(state.banner, /^LIVE Now Current graph$/); assert.equal(state.history, 0); assert.equal(state.cues, 0);
    const recovered = await identity();
    assert.notEqual(recovered.history, standing.history); assert.notEqual(recovered.digest, standing.digest);
    assert.equal(recovered.revisions, standing.revisions + 1); assert.equal(recovered.connected, standing.connected);

    // A geometry revision replaces the catch-up it interrupts. That catch-up's silence goes with it,
    // and its frame brings what the baseline wait held back, so the catch-up has been applied then.
    const interrupt = async what => {
      await send(tasks, { statuses: ['in_progress'] });
      await send(edited(what)); await page.clock.runFor(800);
      const waiting = await inspect();
      assert.equal(waiting.pending, true); assert.notEqual(waiting.what, what);
      assert.ok(waiting.latest.catchUpApplied < waiting.latest.baselineEnded, 'the earlier catch-up is still the latest');
      return waiting;
    };
    await interrupt('Changed while another revision was refused');
    await page.evaluate(() => { fixture.settings.node_width = 500; });
    await send(edited('Brought by the new geometry'));
    state = await inspect();
    assert.deepEqual([state.what, state.cardWidth, state.pending], ['Brought by the new geometry', 500, false]);
    assert.equal(state.latest.catchUpApplied - state.latest.baselineEnded, 50, 'applied as the revision arrived, 800 ms into a 750 ms wait');
    const { catchUpApplied } = state.latest;
    await page.clock.runFor(1500);
    state = await inspect();
    assert.equal(state.cues, 0); assert.equal(state.latest.catchUpApplied, catchUpApplied, 'no second catch-up follows');
    await send(edited('A live edit after the new geometry'));
    await page.clock.runFor(1100);
    state = await inspect();
    assert.equal(state.cues, 1); assert.equal(state.latest.catchUpApplied, catchUpApplied, 'a live edit is not catch-up');
    // Nor is a live batch that new geometry replaces.
    await send(edited('A live batch the new geometry replaces'));
    assert.equal((await inspect()).pending, true);
    await page.evaluate(() => { fixture.settings.node_width = 510; });
    await send(edited('Brought by geometry with no catch-up waiting'));
    state = await inspect();
    assert.deepEqual([state.what, state.cardWidth, state.pending], ['Brought by geometry with no catch-up waiting', 510, false]);
    assert.equal(state.latest.catchUpApplied, catchUpApplied);

    // A replacement that cannot be rendered is not on screen, so it applies no catch-up.
    const unrendered = await interrupt('Changed before a render that fails');
    await page.evaluate(() => {
      entry.runtimeFor(host).layout = () => 'null';
      Object.assign(fixture.settings, { node_width: 520, fit_to_screen: true });
    });
    await send(edited('Never drawn'));
    state = await inspect();
    assert.deepEqual([state.width, state.pending, state.graphIssue.code, state.graphIssue.active], [520, false, 'render-failed', true]);
    assert.notEqual(state.what, 'Never drawn');
    assert.equal(state.latest.catchUpApplied, unrendered.latest.catchUpApplied);
    assert.deepEqual(errors, []);
    console.log('PASS valid geometry with an unreadable frame: a graph failure, unapplied whole; the next readable frame applies it quietly with no disconnect; geometry that replaces catch-up applies it once rendered, silently; a later live edit is audible');
  } finally { await browser.close(); await closeServer(server); }
})().catch(error => { console.error(error); process.exitCode = 1; });
