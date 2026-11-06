// Replay takes the live view's place only while that view is current. An outage, a graph that cannot
// be drawn and a refused settings revision each keep their status in view for as long as they last,
// past any number of replay intervals, and the connection's own indicator shows while replay plays.
// Production runtime through mount and update, as Rust calls them; synthetic graph, virtual clock; no
// daemon connection.
const assert = require('node:assert/strict');
const { startFixtureServer, launchChromium, pauseClock, closeServer } = require('./fixture-server.cjs');

// The shortest replay interval the settings contract allows, and how long each unhealthy view lasts
// here: past two intervals and a whole replay.
const INTERVAL = 60_000, OUTLAST = 2 * INTERVAL + 25_000;

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
    await page.evaluate(async interval => {
      window.entry = await import('/crates/fwd-web/src/wallboard/wallboard-runtime.mjs');
      fixture.runtime.dispose();
      fixture.settings.replay = { enabled: true, interval_seconds: interval / 1000 };
      window.host = document.querySelector('.wallboard-graph');
      window.revision = changes => JSON.stringify({ ...fixture.settings, ...changes });
      entry.mount(host, revision({}), fixture.layout);
    }, INTERVAL);
    const send = (tasks, { connected = true, changes = {} } = {}) => page.evaluate(
      ({ tasks, connected, changes }) => entry.update(host, fixture.snapshot(tasks), true, connected, revision(changes)),
      { tasks, connected, changes });
    const read = () => page.evaluate(() => {
      const r = entry.runtimeFor(host);
      return { banner: r.banner.textContent, mode: host.dataset.mode, history: r.history.entries.length,
        indicator: getComputedStyle(document.querySelector('.wallboard-connection')).display !== 'none' };
    });
    // What the status row said, read once a second for `ms`.
    const watch = async ms => {
      const said = new Set();
      for (let elapsed = 0; elapsed < ms; elapsed += 1000) { await page.clock.runFor(1000); said.add((await read()).banner); }
      return [...said];
    };

    const shown = [{ id: 'a', title: 'Work at hand', priority: 1 }];
    const witnessed = [...shown, { id: 'b', title: 'Work that arrived', priority: 2 }];
    await send(shown); await page.clock.runFor(800);
    await send(witnessed); await page.clock.runFor(31_000);
    let state = await read();
    assert.equal(state.history, 1, 'replay has a change to play');
    assert.equal(state.banner, 'LIVE Now Current graph');
    assert.equal(state.indicator, false, 'the status row says the connection');

    // On a current view, replay starts at its interval, with the connection's own indicator in view.
    await page.clock.runFor(INTERVAL - 31_800);
    state = await read();
    assert.deepEqual([state.mode, state.banner, state.indicator], ['replay', 'REPLAYING Just now 1 added', true]);
    await page.clock.runFor(4_100);
    state = await read();
    assert.deepEqual([state.mode, state.banner, state.indicator], ['live', 'LIVE Now Current graph', false]);
    console.log('PASS replay starts at its interval on a current view, and the connection indicator shows while it plays');

    await send(witnessed, { connected: false });
    assert.deepEqual(await watch(OUTLAST), ['OFFLINE Last view Reconnecting…']);
    await send(witnessed); await page.clock.runFor(800);
    assert.equal((await read()).banner, 'LIVE Now Current graph');
    console.log('PASS an outage longer than two replay intervals stays OFFLINE throughout');

    await page.evaluate(() => entry.update(host, '<broken', true, true, revision({})));
    assert.deepEqual(await watch(OUTLAST), ['STALE Last view Graph update failed']);
    await send(witnessed); await page.clock.runFor(800);
    assert.equal((await read()).banner, 'LIVE Now Current graph');
    console.log('PASS a graph that cannot be drawn stays STALE past two replay intervals');

    await send(witnessed, { changes: { statuses: ['in_progress'] } });
    assert.deepEqual(await watch(OUTLAST), ['STALE Last view Settings update refused']);
    await send(witnessed); await page.clock.runFor(800);
    state = await read();
    assert.deepEqual([state.mode, state.banner], ['live', 'LIVE Now Current graph']);
    assert.deepEqual(errors, []);
    console.log('PASS a refused settings revision stays STALE past two replay intervals; no browser errors');
  } finally { await browser.close(); await closeServer(server); }
})().catch(error => { console.error(error); process.exitCode = 1; });
