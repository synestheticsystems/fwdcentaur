// Diagnostics through the runtime's production entry points, as Rust feeds them and as
// diagnosticsFor reports them. Synthetic graph, virtual clock; no daemon connection.
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
    await page.evaluate(async () => {
      window.entry = await import('/crates/fwd-web/src/wallboard/wallboard-runtime.mjs');
      fixture.runtime.dispose();
      window.host = document.querySelector('.wallboard-graph');
      window.revision = changes => JSON.stringify({ ...fixture.settings, ...changes });
      // Rust times its first recomputation before the runtime exists; mount adopts that record.
      entry.recordRustTiming(host, 'rustMarkup', 12.5);
      entry.mount(host, revision({}), fixture.layout);
    });
    const tasks = [{ id: 'a', title: 'First task', priority: 1 }, { id: 'b', title: 'Second task', priority: 2 }];
    const send = (items, { loaded = true, connected = true, changes = {} } = {}) => page.evaluate(
      ({ items, loaded, connected, changes }) => entry.update(host, fixture.snapshot(items), loaded, connected, revision(changes)),
      { items, loaded, connected, changes });
    const read = () => page.evaluate(() => { const r = entry.runtimeFor(host); return { report: entry.diagnosticsFor(host),
      banner: r.banner.textContent, what: r.nodes.get('a')?.dom.querySelector('.wallboard-what').textContent }; });

    // Cold start: the connection comes up, the standing queries answer, then the baseline wait ends.
    await send(tasks.slice(0, 1), { loaded: false, connected: false }); await page.clock.runFor(100);
    await send(tasks.slice(0, 1), { loaded: false });
    let { report } = await read();
    assert.equal(report.timings.rustMarkup.count, 1);
    assert.deepEqual(report.milestones, {});
    assert.deepEqual(Object.keys(report.latest), ['connected']);
    await page.clock.runFor(100);
    await send(tasks); await page.clock.runFor(800);
    ({ report } = await read());
    assert.ok(report.milestones.firstLoaded > report.latest.connected);
    assert.equal(report.milestones.armed - report.milestones.firstLoaded, 750, 'arming follows the baseline wait');
    assert.equal(report.latest.baselineEnded, report.milestones.armed);
    assert.equal('catchUpApplied' in report.latest, false, 'arming presents the first graph itself');
    assert.match(report.state.settingsDigest, /^[0-9a-f]{8}$/);

    // Reconnect with nothing new: the latest transitions move, and the once-only milestones stay.
    await send(tasks, { connected: false }); await page.clock.runFor(1000);
    await send(tasks); await page.clock.runFor(800);
    const reconnected = (await read()).report;
    assert.ok(reconnected.latest.connected - reconnected.latest.disconnected >= 1000);
    assert.equal(reconnected.latest.baselineEnded - reconnected.latest.connected, 750, 'catch-up waits for its own baseline');
    assert.equal(reconnected.latest.catchUpApplied, reconnected.latest.baselineEnded, 'nothing new means nothing further to wait for');
    assert.deepEqual(reconnected.milestones, report.milestones);

    // Reconnect with a change: the view still shows the old content when the baseline ends, and
    // the change appears only after the deliberate batch wait and wave.
    const edited = [{ ...tasks[0], what: 'Changed during the outage' }, tasks[1]];
    await send(tasks, { connected: false }); await page.clock.runFor(1000);
    await send(edited); await page.clock.runFor(750);
    let sample = await read();
    assert.equal(sample.report.latest.baselineEnded - sample.report.latest.connected, 750);
    assert.ok(sample.report.latest.catchUpApplied < sample.report.latest.baselineEnded, 'the earlier catch-up is still the latest');
    assert.notEqual(sample.what, edited[0].what);
    await page.clock.runFor(1049);
    assert.notEqual((await read()).what, edited[0].what);
    await page.clock.runFor(1);
    sample = await read();
    assert.equal(sample.what, edited[0].what);
    assert.equal(sample.report.latest.catchUpApplied - sample.report.latest.baselineEnded, 1050);
    // A live edit is not catch-up, so it moves neither mark.
    const { baselineEnded, catchUpApplied } = sample.report.latest;
    await send(tasks); await page.clock.runFor(1100);
    sample = await read();
    assert.equal(sample.what, 'Actual task WHAT stays visible.');
    assert.deepEqual([sample.report.latest.baselineEnded, sample.report.latest.catchUpApplied], [baselineEnded, catchUpApplied]);

    // An applied revision changes the settings identity, and no sample holds setting values or task text.
    await send(tasks, { changes: { theme: 'sand-plum' } });
    const themed = (await read()).report;
    assert.notEqual(themed.state.settingsDigest, report.state.settingsDigest);
    assert.doesNotMatch(JSON.stringify(themed), /sand-plum|First task|Second task/);

    // A layout that cannot place the graph fails the render: STALE, with its reason, until a render succeeds.
    await page.evaluate(() => { const r = entry.runtimeFor(host); window.workingLayout = r.layout; r.layout = () => 'null'; });
    await send(tasks, { changes: { theme: 'sand-plum', fit_to_screen: true } });
    let state = await read();
    const failed = state.report.issues.graph;
    assert.deepEqual([failed.code, failed.reason, failed.active], ['render-failed', 'Invalid wallboard layout', true]);
    assert.match(state.banner, /^STALE Last view Graph update failed$/);
    // A baseline can end while that lasts, but catch-up that cannot be rendered has not been applied.
    const rendered = state.report.latest.catchUpApplied;
    await send(edited, { connected: false, changes: { theme: 'sand-plum', fit_to_screen: true } }); await page.clock.runFor(100);
    await send(tasks, { changes: { theme: 'sand-plum', fit_to_screen: true } }); await page.clock.runFor(1900);
    state = await read();
    assert.ok(state.report.latest.baselineEnded > rendered); assert.equal(state.report.latest.catchUpApplied, rendered);
    assert.match(state.banner, /^STALE Last view Graph update failed$/);
    await page.evaluate(() => { entry.runtimeFor(host).layout = window.workingLayout; });
    await send([...tasks, { id: 'c', title: 'Third task', priority: 2 }], { changes: { theme: 'sand-plum', fit_to_screen: true } });
    assert.match((await read()).banner, /^STALE Last view Graph update failed$/, 'a readable frame alone is not a successful render');
    await page.clock.runFor(1500);
    state = await read();
    assert.equal(state.report.issues.graph.active, false); assert.match(state.banner, /^LIVE Now Current graph$/);
    assert.deepEqual(errors, []);
    console.log('PASS diagnostics through mount/update: Rust timing kept from before mount; connection, first-loaded and armed times on one clock; reconnect transitions, with baseline end and applied catch-up apart; settings digest without values; render failure with its reason and recovery');
  } finally { await browser.close(); await closeServer(server); }
})().catch(error => { console.error(error); process.exitCode = 1; });
