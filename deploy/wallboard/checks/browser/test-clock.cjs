// The page's wall clock stepping back or forward while cues play changes none of their timing, nor the
// sound's cooldown, since both run on the page's monotonic clock; and the history keeps the entries a
// step back leaves stamped ahead of now. Production runtime through mount and update, with real Web
// Audio; synthetic graph, virtual clock whose wall time the check steps; no daemon connection.
const assert = require('node:assert/strict');
const { startFixtureServer, launchChromium, pauseClock, closeServer } = require('./fixture-server.cjs');

const HOUR = 3_600_000;
// From a frame to its cues on screen: the 250 ms batch wait, the 800 ms wave, and the animation frame
// after.
const TO_CUES = 1_070;
// A card that leaves holds for four seconds and fades for 450 ms; an edit glows for three seconds, and
// an arriving card is marked for six.
const DEPARTURE = 4_450, EDIT = 3_000, ARRIVAL = 6_000;

(async () => {
  const server = await startFixtureServer();
  const browser = await launchChromium({ args: ['--autoplay-policy=no-user-gesture-required', '--mute-audio'] })
    .catch(async error => { await closeServer(server); throw error; });
  try {
    const page = await browser.newPage({ viewport: { width: 1920, height: 1080 } });
    const errors = []; page.on('pageerror', error => errors.push(String(error)));
    await page.clock.install({ time: new Date('2026-11-01T12:00:00Z') });
    await page.goto(server.url);
    await page.waitForFunction(() => window.fixture);
    await pauseClock(page);
    await page.evaluate(async () => {
      // Each cue that starts playing, as Web Audio sees it.
      window.cuesPlayed = 0;
      const start = AudioBufferSourceNode.prototype.start;
      AudioBufferSourceNode.prototype.start = function (...args) { cuesPlayed++; return start.apply(this, args); };
      window.entry = await import('/crates/fwd-web/src/wallboard/wallboard-runtime.mjs');
      fixture.runtime.dispose();
      Object.assign(fixture.settings, { replay: { enabled: false, interval_seconds: 180 }, sound: { enabled: true, volume: 30 } });
      window.host = document.querySelector('.wallboard-graph');
      window.revision = JSON.stringify(fixture.settings);
      entry.mount(host, revision, fixture.layout);
    });
    // Sounds load and audio resumes in real time, which the virtual clock does not move.
    const settle = async (what, condition) => {
      for (let tries = 0; tries < 100; tries++) {
        if (await page.evaluate(condition)) return;
        await new Promise(resolve => setTimeout(resolve, 50));
      }
      throw Error(`timed out waiting for ${what}`);
    };
    await settle('the sounds to load', () => Boolean(entry.runtimeFor(host).sound.buffers));
    const send = tasks => page.evaluate(tasks => entry.update(host, fixture.snapshot(tasks), true, true, revision), tasks);
    const read = () => page.evaluate(() => {
      const r = entry.runtimeFor(host);
      return { history: r.history.entries.length, frame: r.frame,
        cards: Object.fromEntries([...r.nodes].map(([id, card]) => [id, { motion: card.dom.dataset.motion,
          countdown: +card.track.getAttribute('width'), width: card.node.width, opacity: +card.dom.style.opacity }])) };
    });
    const step = by => page.evaluate(by => Date.now() + by, by).then(time => page.clock.setSystemTime(time));
    // Samples the cues every 100 ms for `ms`, each sample as the time since the cues began and the cards.
    const sample = async (ms, since) => {
      const samples = [];
      for (let elapsed = since; elapsed < ms; elapsed += 100) { await page.clock.runFor(100); samples.push([elapsed + 100, (await read()).cards]); }
      return samples;
    };

    const task = (id, what = 'As it stands') => ({ id, title: `Work ${id}`, what, priority: 2 });
    await send([task('a'), task('b')]); await page.clock.runFor(800);
    await send([task('a'), task('b'), task('c')]); await page.clock.runFor(TO_CUES);
    await settle('the arrival cue', () => cuesPlayed === 1);
    await page.clock.runFor(7_000);
    assert.equal((await read()).history, 1);

    // One batch: a edited, b leaving, d arriving. Once its cues show, the wall clock steps back an hour.
    await send([task('a', 'Edited'), task('c'), task('d')]); await page.clock.runFor(TO_CUES);
    await settle('the batch cue', () => cuesPlayed === 2);
    await step(-HOUR);
    const back = await sample(ARRIVAL + 500, TO_CUES - 1_050);
    for (const [elapsed, cards] of back) {
      if (cards.b) assert.ok(cards.b.countdown <= cards.b.width - 12 && cards.b.opacity <= 1, `the leaving card's countdown fits it at ${elapsed} ms`);
      assert.equal(Boolean(cards.b), elapsed < DEPARTURE, `the leaving card is gone at ${DEPARTURE} ms, not before: ${elapsed} ms`);
      assert.equal(cards.a.motion === 'edit', elapsed < EDIT, `the edit glows for ${EDIT} ms: ${elapsed} ms`);
      assert.equal(cards.d.motion === 'arrival', elapsed < ARRIVAL, `the arrival is marked for ${ARRIVAL} ms: ${elapsed} ms`);
    }
    let state = await read();
    assert.equal(state.frame, 0, 'no animation frame is asked for once the cues have ended');
    assert.equal(state.history, 2);
    console.log('PASS the wall clock stepped back an hour mid-cue: the countdown fits its card, and each cue ends on time');

    // A change after the step, stamped an hour before the entries already kept: they stay, through a
    // reload too, and its cue sounds, the cooldown measured on the monotonic clock.
    await send([task('a', 'Edited'), task('c'), task('d'), task('e')]); await page.clock.runFor(TO_CUES);
    await settle('a cue after the step', () => cuesPlayed === 3);
    assert.equal((await read()).history, 3, 'the entries stamped ahead are kept');
    await page.evaluate(() => { entry.dispose(host); entry.mount(host, revision, fixture.layout); });
    assert.equal((await read()).history, 3, 'and kept through a reload');
    await send([task('a', 'Edited'), task('c'), task('d'), task('e')]); await page.clock.runFor(7_000);
    console.log('PASS entries stamped ahead by the step are kept, through a reload, and a cue after the step sounds');

    // Forward: the next batch's cues run their whole length although the wall clock jumps two hours.
    await send([task('a', 'Edited again'), task('c'), task('e')]); await page.clock.runFor(TO_CUES);
    await step(2 * HOUR);
    const ahead = await sample(DEPARTURE + 200, TO_CUES - 1_050);
    for (const [elapsed, cards] of ahead) {
      assert.equal(Boolean(cards.d), elapsed < DEPARTURE, `the leaving card is gone at ${DEPARTURE} ms, not before: ${elapsed} ms`);
      assert.equal(cards.a.motion === 'edit', elapsed < EDIT, `the edit glows for ${EDIT} ms: ${elapsed} ms`);
    }
    await page.clock.runFor(2_000);
    assert.equal((await read()).frame, 0);
    assert.deepEqual(errors, []);
    console.log('PASS the wall clock stepped forward two hours mid-cue: each cue runs its whole length; no browser errors');
  } finally { await browser.close(); await closeServer(server); }
})().catch(error => { console.error(error); process.exitCode = 1; });
