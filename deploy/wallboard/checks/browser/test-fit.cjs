// Screen fitting through settings revisions. The resize observer answers real layout, which a virtual
// clock cannot drive, so this check uses native timers. Synthetic graph; no daemon connection.
const assert = require('node:assert/strict');
const { startFixtureServer, launchChromium, closeServer } = require('./fixture-server.cjs');

(async () => {
  const server = await startFixtureServer();
  const browser = await launchChromium().catch(async error => { await closeServer(server); throw error; });
  try {
    const page = await browser.newPage({ viewport: { width: 1920, height: 1080 } });
    const errors = []; page.on('pageerror', error => errors.push(String(error)));
    await page.goto(server.url);
    await page.waitForFunction(() => window.fixture);
    await page.evaluate(() => {
      fixture.settings.fit_to_screen = true; fixture.reset();
      fixture.runtime.receive(fixture.snapshot([{ id: 'a', title: 'Fitted task', priority: 1 }]), true, true);
    });
    await page.waitForFunction(() => fixture.runtime.armed && !fixture.runtime.frame);

    // A revision that turns fitting off disconnects the observer and cancels a pending reflow, so a
    // resize no longer renders.
    await page.evaluate(() => {
      const r = fixture.runtime, first = r.resizeObserver;
      window.watch = { first, disconnected: false, staleReflow: false, renders: 0 };
      const disconnect = first.disconnect.bind(first);
      first.disconnect = () => { watch.disconnected = true; disconnect(); };
      const render = r.render.bind(r);
      r.render = (...frame) => { watch.renders++; return render(...frame); };
      r.later('resize', () => { watch.staleReflow = true; }, 50);
      window.sendFit = fit_to_screen =>
        r.update(r.lastSource, true, true, JSON.stringify({ ...r.settings, fit_to_screen }));
      sendFit(false);
      watch.renders = 0;
    });
    await page.setViewportSize({ width: 1200, height: 800 });
    await page.waitForTimeout(400);
    assert.deepEqual(await page.evaluate(() => {
      const { disconnected, staleReflow, renders } = watch; return { disconnected, staleReflow, renders };
    }), { disconnected: true, staleReflow: false, renders: 0 });

    // Turning it on again replaces the observer with one that reflows on the next resize.
    assert.equal(await page.evaluate(() => { sendFit(true); return fixture.runtime.resizeObserver !== watch.first; }), true);
    await page.setViewportSize({ width: 1500, height: 900 });
    await page.waitForFunction(() => {
      const r = fixture.runtime;
      return r.host.clientWidth > 1200 && JSON.parse(r.layoutInput).viewport_width === r.host.clientWidth && !r.frame;
    });
    assert.deepEqual(errors, []);
    console.log('PASS a fitting revision disconnects the resize observer and cancels its pending reflow; turning fitting on again replaces it with one that reflows');
  } finally { await browser.close(); await closeServer(server); }
})().catch(error => { console.error(error); process.exitCode = 1; });
