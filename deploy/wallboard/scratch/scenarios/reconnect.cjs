// A date passes during an outage, and another is set during it. The last view stands through the
// outage, and what changed appears as silent catch-up after the reconnect.
const { CHECK_EVERY, FAR_FUTURE, openArmedDisplay, closeDisplay, expectHealthy, namedBy, sameChange } = require('../display.cjs');

module.exports = async function reconnect(world) {
  const { report, daemon, server } = world;
  const { expect, observe, delay, waitUntil } = report;
  const project = daemon.project('reconnect');
  const ids = {
    outage: project.task('Comes due during the outage'),
    outageEdit: project.task('Told a date during the outage'),
    steady: project.task('Shown throughout'),
  };
  const change = namedBy(ids);
  const display = await openArmedDisplay(world, project, 'reconnect');

  const outageDate = Date.now() + 30_000;
  project.date(ids.outage, new Date(outageDate).toISOString());
  const leaving = await waitUntil('the outage task to leave', async () => { const now = await display.read(); return now.cards[ids.outage]?.motion === 'departure' ? now : null; }, 15_000);
  expect('the outage task leaves first, its date still to come', Boolean(leaving));
  await waitUntil('its card to go', async () => !((await display.read()).cards[ids.outage]), 8_000);
  await delay(4_500);
  const beforeOutage = await display.read();

  server.backend(false);
  await waitUntil('the display to see the outage', async () => (await display.read()).connection === 'false', 15_000);
  project.date(ids.outageEdit, FAR_FUTURE);
  await delay(Math.max(0, outageDate + CHECK_EVERY + 2_000 - Date.now()));
  const during = await display.read();
  expect('during the outage the last view stands', !(ids.outage in during.cards) && ids.outageEdit in during.cards && /^OFFLINE Last view/.test(during.banner),
    { banner: during.banner, outageTask: ids.outage in during.cards, editTask: ids.outageEdit in during.cards, dateIsPast: Date.now() > outageDate });

  server.backend(true);
  const caughtUp = await waitUntil('the catch-up to apply', async () => {
    const now = await display.read();
    return now.connection === 'true' && ids.outage in now.cards && now.cards[ids.outageEdit]?.motion !== undefined && !(now.banner || '').startsWith('SYNCING') && now.history > beforeOutage.history ? now : null;
  }, 45_000, 50);
  expect('the reconnect brings both changes as catch-up, and it invents no edit', caughtUp && sameChange(change(caughtUp), ['outage'], ['outageEdit'], []), caughtUp && change(caughtUp));
  expect('the catch-up is silent', caughtUp?.played === beforeOutage.played, { before: beforeOutage.played, after: caughtUp?.played });
  const { connected, baselineEnded, catchUpApplied } = caughtUp?.diagnostics.latest ?? {};
  expect('the catch-up marks follow the reconnect', connected < baselineEnded && baselineEnded <= catchUpApplied, { connected, baselineEnded, catchUpApplied });
  observe('what a reconnect catch-up shows', { waveThisCatchUp: caughtUp?.page.waves.slice(-2), motion: caughtUp?.cards[ids.outage]?.motion });
  await delay(6_500);
  expectHealthy(report, 'after the reconnect', await display.read());
  await closeDisplay(report, display, 'reconnect');
};
