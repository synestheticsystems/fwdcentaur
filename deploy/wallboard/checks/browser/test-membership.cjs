// A card entering or leaving the view, as a not-before date does when it passes or is set, reaches
// the runtime as an ordinary frame. Production runtime through mount and update, as Rust calls them;
// synthetic graph, virtual clock; no daemon connection.
const assert = require('node:assert/strict');
const { startFixtureServer, launchChromium, pauseClock, closeServer } = require('./fixture-server.cjs');

// Cues show from the animation frame after a render.
const ONE_FRAME = 20;

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
      window.changes = (await import('/crates/fwd-web/src/wallboard/wallboard-history.mjs')).changes;
      fixture.runtime.dispose();
      // Replay starts only when a check asks for it, so that it cannot interrupt the others.
      fixture.settings.replay = { enabled: false, interval_seconds: 180 };
      window.host = document.querySelector('.wallboard-graph');
      window.revision = JSON.stringify(fixture.settings);
      entry.mount(host, revision, fixture.layout);
      // What would sound: the runtime asks for a cue, and its own gate lets it through.
      const r = entry.runtimeFor(host); window.sounded = [];
      r.sound.play = async diff => { if (r.sound.allowed()) sounded.push(diff); return true; };
    });
    const send = (tasks, edges = [], { loaded = true, connected = true } = {}) => page.evaluate(
      ({ tasks, edges, loaded, connected }) => entry.update(host, fixture.snapshot(tasks, edges), loaded, connected, revision),
      { tasks, edges, loaded, connected });
    const inspect = () => page.evaluate(() => {
      const r = entry.runtimeFor(host), last = r.history.entries.at(-1);
      const { arrived, departed, edited } = last ? changes(fixture.readSnapshot(last.before), fixture.readSnapshot(last.after)) : {};
      return { banner: r.banner.textContent, armed: r.armed, wave: Boolean(r.wave), history: r.history.entries.length,
        lastChange: last && { arrived, departed, edited }, sounded: sounded.length, lastSounded: sounded.at(-1),
        effects: r.effects.size, frame: r.frame, latest: r.diagnostics.latest,
        cards: Object.fromEntries([...r.nodes].map(([id, card]) => [id, { motion: card.dom.dataset.motion, glow: +card.glow.style.opacity,
          wash: +card.wash.style.opacity, countdown: +card.track.getAttribute('width') }])),
        arrows: [...r.edges.keys()] };
    });
    const unmarked = { motion: '', glow: 0, wash: 0, countdown: 0 };
    const hide = hidden => page.evaluate(hidden => {
      if (hidden) Object.defineProperty(document, 'hidden', { configurable: true, get: () => true });
      else delete document.hidden;
      document.dispatchEvent(new Event('visibilitychange', { bubbles: true }));
    }, hidden);

    const shown = { id: 'shown', title: 'Work at hand', priority: 1 };
    const waiting = { id: 'waiting', title: 'Work that waits', priority: 2, availability: ['Blocked'] };
    const dated = { id: 'dated', title: 'Work with a date', priority: 2 };
    const during = { id: 'during', title: 'Work answering during the load', priority: 3 };
    const waits = [{ from: 'dated', to: 'waiting' }];

    // Start-up: cards joining while the graph loads and through the baseline wait come with no cue,
    // no sound and no history.
    let state;
    const quietStart = async () => {
      state = await inspect();
      assert.equal(state.wave, false); assert.equal(state.history, 0); assert.equal(state.effects, 0); assert.equal(state.sounded, 0);
    };
    await send([shown], [], { loaded: false, connected: false }); await page.clock.runFor(100); await quietStart();
    await send([shown], [], { loaded: false }); await page.clock.runFor(100); await quietStart();
    await send([shown, waiting]); await page.clock.runFor(100); await quietStart();
    assert.equal(state.armed, false);
    await send([shown, waiting, during]); await page.clock.runFor(800); await quietStart();
    assert.equal(state.armed, true);
    assert.deepEqual(state.cards, { shown: unmarked, waiting: unmarked, during: unmarked });
    console.log('PASS start-up is quiet: cards joining through the load and the baseline wait bring no cue, sound or history');
    // The checks below start from the two cards, with nothing recorded of the third leaving.
    await send([shown, waiting]); await page.clock.runFor(6000);
    assert.deepEqual(Object.keys((await inspect()).cards).sort(), ['shown', 'waiting']);
    await page.evaluate(() => { const r = entry.runtimeFor(host); r.history.entries = []; sounded.length = 0; });

    // Live arrival: the ordinary batch and wave, then the arrival cue on that card alone. The arrow it
    // brings to a card already shown is no edit of that card.
    await send([shown, waiting, dated], waits); await page.clock.runFor(249);
    state = await inspect();
    assert.equal(state.wave, false); assert.equal('dated' in state.cards, false, 'batched for 250 ms');
    await page.clock.runFor(1);
    state = await inspect();
    assert.equal(state.wave, true); assert.equal('dated' in state.cards, false, 'the wave passes first');
    await page.clock.runFor(800 + ONE_FRAME);
    state = await inspect();
    assert.equal(state.wave, false); assert.equal(state.cards.dated.motion, 'arrival'); assert.ok(state.cards.dated.wash > 0);
    assert.deepEqual(state.arrows, ['dated>waiting']);
    assert.deepEqual(state.cards.shown, unmarked); assert.deepEqual(state.cards.waiting, unmarked);
    assert.deepEqual(state.lastChange, { arrived: ['dated'], departed: [], edited: [] });
    assert.equal(state.history, 1); assert.equal(state.sounded, 1);
    assert.deepEqual(state.lastSounded.edited, []); assert.deepEqual(state.lastSounded.arrived, ['dated']);
    await page.clock.runFor(31000);
    state = await inspect();
    assert.deepEqual(state.cards.dated, unmarked); assert.equal(state.frame, 0);
    console.log('PASS a card entering the view: batched live cue, arrival on that card alone, no invented edit, one history entry and one sound');

    // Live departure: the ordinary batch and wave, then the countdown on that card alone.
    await send([shown, waiting]); await page.clock.runFor(1050 + ONE_FRAME);
    state = await inspect();
    assert.equal(state.cards.dated.motion, 'departure'); assert.ok(state.cards.dated.countdown > 0);
    assert.deepEqual(state.cards.shown, unmarked); assert.deepEqual(state.cards.waiting, unmarked);
    assert.deepEqual(state.lastChange, { arrived: [], departed: ['dated'], edited: [] });
    assert.equal(state.history, 2); assert.equal(state.sounded, 2);
    assert.deepEqual(state.lastSounded.edited, []); assert.deepEqual(state.lastSounded.departed, ['dated']);
    await page.clock.runFor(5700);
    state = await inspect();
    assert.deepEqual(Object.keys(state.cards).sort(), ['shown', 'waiting']); assert.deepEqual(state.arrows, []); assert.equal(state.frame, 0);
    console.log('PASS a card leaving the view: batched live cue, countdown on that card alone, no invented edit, one history entry and one sound');

    // Replay tells these as a card added and a card removed, never as an update, and silently.
    assert.equal(await page.evaluate(() => entry.runtimeFor(host).startReplay()), true);
    assert.match((await inspect()).banner, /^REPLAYING Just now 1 added$/);
    await page.clock.runFor(4000);
    assert.match((await inspect()).banner, /^REPLAYING Just now 1 removed$/);
    await page.clock.runFor(4100);
    state = await inspect();
    assert.match(state.banner, /^LIVE Now Current graph$/); assert.equal(state.sounded, 2);
    console.log('PASS replay names the membership changes it witnessed as added and removed, with no historical edit and no sound');

    // Reconnect: a frame that changed the view during the outage leaves the last view standing, then
    // appears as silent catch-up after a baseline wait.
    await send([shown, waiting], [], { connected: false }); await page.clock.runFor(1000);
    await send([shown, waiting, dated], waits, { connected: false }); await page.clock.runFor(1000);
    state = await inspect();
    assert.equal('dated' in state.cards, false, 'the last view stands'); assert.match(state.banner, /^OFFLINE Last view/);
    await send([shown, waiting, dated], waits); await page.clock.runFor(749);
    state = await inspect();
    assert.equal('dated' in state.cards, false); assert.match(state.banner, /^SYNCING Catching up/);
    await page.clock.runFor(1 + 1050);
    state = await inspect();
    assert.equal('dated' in state.cards, true); assert.match(state.banner, /^LIVE Now Current graph$/);
    assert.equal(state.sounded, 2, 'catch-up is silent');
    assert.deepEqual(state.lastChange, { arrived: ['dated'], departed: [], edited: [] });
    assert.equal(state.latest.catchUpApplied - state.latest.baselineEnded, 1050);
    await page.clock.runFor(31000);
    console.log('PASS reconnect: what changed the view during the outage waits for the baseline, then appears silently, as an arrival');

    // Hidden page: a frame received while hidden applies at once, with no wave, cue or sound, in
    // either direction, and nothing plays on coming back into view.
    const historyBefore = state.history;
    await hide(true);
    await send([shown, waiting]); await page.clock.runFor(250 + ONE_FRAME);
    state = await inspect();
    assert.equal('dated' in state.cards, false, 'no countdown holds it'); assert.equal(state.wave, false);
    assert.deepEqual(state.lastChange, { arrived: [], departed: ['dated'], edited: [] });
    await send([shown, waiting, dated], waits); await page.clock.runFor(250 + ONE_FRAME);
    state = await inspect();
    assert.equal(state.wave, false); assert.deepEqual(state.cards.dated, unmarked); assert.equal(state.effects, 0);
    assert.deepEqual(state.lastChange, { arrived: ['dated'], departed: [], edited: [] });
    assert.equal(state.history, historyBefore + 2); assert.equal(state.sounded, 2);
    await hide(false); await page.clock.runFor(1100);
    state = await inspect();
    assert.equal(state.wave, false); assert.deepEqual(state.cards.dated, unmarked); assert.equal(state.sounded, 2); assert.equal(state.frame, 0);
    console.log('PASS hidden page: frames that change the view apply at once with no wave, cue or sound, and coming back plays nothing');

    // A real dependency between two cards that stay, batched with a card arriving or departing. Rust
    // hashes what a card states of its relations into its revision, so both cards' revisions change
    // here, and nothing else about them need: a link touches no task, and a card already Blocked says
    // so before and after.
    const awaited = { id: 'awaited', title: 'Work already awaited', priority: 2 };
    const awaitedNext = { id: 'awaitedNext', title: 'Work awaited next', priority: 2 };
    const blocked = { id: 'blocked', title: 'Work already blocked', priority: 2, availability: ['Blocked'] };
    const unrelated = { id: 'unrelated', title: 'Work with no relation to these', priority: 3 };
    const standing = [{ from: 'awaited', to: 'blocked' }];
    const linked = [...standing, { from: 'awaitedNext', to: 'blocked' }];
    const relatedTo = (card, other) => ({ ...card, dependency: other });
    const settleOn = async (tasks, edges) => {
      await send(tasks, edges); await page.clock.runFor(31000);
      await page.evaluate(() => { entry.runtimeFor(host).history.entries = []; sounded.length = 0; });
    };
    const expectReplay = async (banner, edited) => {
      await page.clock.runFor(7000);
      assert.equal(await page.evaluate(() => entry.runtimeFor(host).startReplay()), true);
      await page.clock.runFor(ONE_FRAME);
      state = await inspect();
      assert.equal(state.banner, banner);
      for (const id of edited) { assert.equal(state.cards[id].motion, 'edit'); assert.ok(state.cards[id].glow > 0); }
      await page.clock.runFor(4100);
      state = await inspect();
      assert.equal(state.banner, 'LIVE Now Current graph'); assert.equal(state.sounded, 1, 'replay is silent');
    };
    // The ordinary batch and wave, with the frame's change on screen only after them.
    const throughBatchAndWave = async arrows => {
      await page.clock.runFor(249);
      state = await inspect();
      assert.equal(state.wave, false); assert.deepEqual(state.arrows, arrows, 'batched for 250 ms');
      await page.clock.runFor(1);
      state = await inspect();
      assert.equal(state.wave, true); assert.deepEqual(state.arrows, arrows, 'the wave passes first');
      await page.clock.runFor(800 + ONE_FRAME);
      state = await inspect();
      assert.equal(state.wave, false);
    };
    await settleOn([awaited, awaitedNext, blocked], standing);
    await send([awaited, relatedTo(awaitedNext, 'blocked'), relatedTo(blocked, 'awaitedNext'), unrelated], linked);
    await throughBatchAndWave(['awaited>blocked']);
    assert.deepEqual(state.lastChange, { arrived: ['unrelated'], departed: [], edited: ['awaitedNext', 'blocked'] });
    assert.deepEqual(state.arrows, ['awaited>blocked', 'awaitedNext>blocked']);
    for (const id of ['awaitedNext', 'blocked']) { assert.equal(state.cards[id].motion, 'edit'); assert.ok(state.cards[id].glow > 0); }
    assert.equal(state.cards.unrelated.motion, 'arrival');
    assert.deepEqual(state.cards.awaited, unmarked);
    assert.equal(state.history, 1); assert.equal(state.sounded, 1);
    await expectReplay('REPLAYING Just now 1 added · 2 updated', ['awaitedNext', 'blocked']);
    console.log('PASS a dependency added between two staying cards, batched with an unrelated arrival: both glow, history and replay name them');

    await page.evaluate(() => { entry.runtimeFor(host).history.entries = []; sounded.length = 0; });
    await send([awaited, awaitedNext, blocked], standing);
    await throughBatchAndWave(['awaited>blocked', 'awaitedNext>blocked']);
    assert.deepEqual(state.lastChange, { arrived: [], departed: ['unrelated'], edited: ['awaitedNext', 'blocked'] });
    assert.deepEqual(state.arrows, ['awaited>blocked']);
    for (const id of ['awaitedNext', 'blocked']) { assert.equal(state.cards[id].motion, 'edit'); assert.ok(state.cards[id].glow > 0); }
    assert.equal(state.cards.unrelated.motion, 'departure');
    assert.deepEqual(state.cards.awaited, unmarked);
    assert.equal(state.history, 1); assert.equal(state.sounded, 1);
    await expectReplay('REPLAYING Just now 2 updated · 1 removed', ['awaitedNext', 'blocked']);
    console.log('PASS a dependency retracted between two staying cards, batched with an unrelated departure: both glow, history and replay name them');

    // An arrow that changes with no card arriving, departing or changing revision. A build whose
    // revisions state dependencies does not write this frame, but history saved by an earlier one can
    // hold it: it draws, and replay says something rather than nothing.
    await settleOn([awaited, awaitedNext, blocked], []);
    await send([awaited, awaitedNext, blocked], standing);
    await throughBatchAndWave([]);
    assert.deepEqual(state.arrows, ['awaited>blocked']);
    assert.deepEqual(state.lastChange, { arrived: [], departed: [], edited: [] });
    assert.deepEqual(state.cards, { awaited: unmarked, awaitedNext: unmarked, blocked: unmarked });
    assert.equal(state.history, 1);
    await expectReplay('REPLAYING Just now Observed change', []);
    console.log('PASS arrows changing alone edit nobody: batched and drawn, one history entry, and replay reads Observed change');

    // A reload with one entry already saved, within 30 seconds of a live change after it. What a card's
    // revision hashes can change between builds, so `scheme` stands for the build that drew a card.
    const work = (scheme, ...ids) => ids.map(id => ({ id, title: `Work ${id}`, priority: 2, revision: `${scheme}-${id}` }));
    const thisBuild = await page.evaluate(async () => (await import('/crates/fwd-web/src/wallboard/wallboard-build.mjs')).buildInfo.content_sha256);
    const reloadOnto = async ({ build, before, after }, tasks) => {
      await page.evaluate(({ build, before, after }) => {
        const { key } = entry.runtimeFor(host).history;
        entry.dispose(host);
        localStorage.setItem(key, JSON.stringify({ version: 1,
          entries: [{ at: Date.now(), build, before: fixture.snapshot(before), after: fixture.snapshot(after) }] }));
        entry.mount(host, revision, fixture.layout);
      }, { build, before, after });
      await send(tasks); await page.clock.runFor(800);
      state = await inspect();
      assert.equal(state.armed, true); assert.equal(state.wave, false); assert.equal(state.effects, 0); assert.equal(state.history, 1);
    };
    const changeThenReplay = async tasks => {
      await send(tasks); await page.clock.runFor(1050 + ONE_FRAME + 7000);
      const [saved, live] = await page.evaluate(() => entry.runtimeFor(host).history.entries.map(({ at, build }) => ({ at, build })));
      assert.ok(live.at - saved.at <= 30000, 'both fall within the 30 seconds of one group');
      assert.equal(live.build, thisBuild);
      assert.equal(await page.evaluate(() => entry.runtimeFor(host).startReplay()), true);
      await page.clock.runFor(ONE_FRAME);
      state = await inspect();
    };
    // A card arrives just before the reload and leaves just after it, and the cards that stay change
    // nothing but what their revisions hash. Compared across the reload, those two would read as edited
    // and the arrival and departure would vanish.
    const replayAcrossUpgradeFrom = async build => {
      await reloadOnto({ build, before: work('old', 'a', 'b'), after: work('old', 'a', 'b', 'x') }, work('new', 'a', 'b', 'x'));
      await changeThenReplay(work('new', 'a', 'b'));
      assert.deepEqual(state.lastChange, { arrived: [], departed: ['x'], edited: [] });
      assert.equal(state.banner, 'REPLAYING Just now 1 added');
      assert.equal(state.cards.x.motion, 'arrival');
      assert.deepEqual([state.cards.a, state.cards.b], [unmarked, unmarked]);
      await page.clock.runFor(4000);
      state = await inspect();
      assert.equal(state.banner, 'REPLAYING Just now 1 removed');
      assert.equal(state.cards.x.motion, 'departure');
      assert.deepEqual([state.cards.a, state.cards.b], [unmarked, unmarked]);
      await page.clock.runFor(4100);
      assert.equal((await inspect()).banner, 'LIVE Now Current graph');
    };
    await replayAcrossUpgradeFrom('another-build');
    console.log('PASS a saved entry of another build and a live change within 30 seconds replay as two steps, 1 added then 1 removed, with no edit invented on the cards that stayed');

    // A reload of one build leaves revisions comparable, so it ends no group: a card arriving either
    // side of it is one step.
    await reloadOnto({ build: thisBuild, before: work('same', 'a', 'b'), after: work('same', 'a', 'b', 'x') }, work('same', 'a', 'b', 'x'));
    await changeThenReplay(work('same', 'a', 'b', 'x', 'y'));
    assert.equal(state.banner, 'REPLAYING Just now 2 added');
    assert.equal(state.cards.x.motion, 'arrival'); assert.equal(state.cards.y.motion, 'arrival');
    assert.deepEqual([state.cards.a, state.cards.b], [unmarked, unmarked]);
    await page.clock.runFor(4100);
    assert.equal((await inspect()).banner, 'LIVE Now Current graph');
    assert.deepEqual(errors, []);
    console.log('PASS a reload of one build ends no group: a saved entry and a live change within 30 seconds replay as one step; no browser errors');
  } finally { await browser.close(); await closeServer(server); }
})().catch(error => { console.error(error); process.exitCode = 1; });
