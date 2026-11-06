// When the page's clock checks. A hidden page applies what the daemon sends quietly, but the clock
// does not check there, so a date passing shows only on return. A clock corrected in either direction
// moves a card, on the ordinary cue. Hiding is simulated by overriding what the page reads, and a
// correction by moving the page's Date.now, not the operating system's clock.
const { CHECK_EVERY, FAR_FUTURE, openArmedDisplay, closeDisplay, namedBy, sameChange } = require('../display.cjs');

const DAY = 86_400_000;

module.exports = async function pageClock(world) {
  const { report, daemon } = world;
  const { expect, observe, delay, waitUntil } = report;
  const project = daemon.project('page-clock');
  const ids = {
    hidden: project.task('Comes due while the page is hidden'),
    hiddenEdit: project.task('Comes back while the page is hidden'),
    clock: project.task('Comes due only if the clock runs ahead'),
    steady: project.task('Shown throughout'),
  };
  const change = namedBy(ids);
  project.date(ids.clock, new Date(Date.now() + 2 * DAY).toISOString());
  const display = await openArmedDisplay(world, project, 'page-clock');

  const hide = hidden => display.page.evaluate(hidden => {
    if (hidden) Object.defineProperty(document, 'hidden', { configurable: true, get: () => true });
    else delete document.hidden;
    document.dispatchEvent(new Event('visibilitychange', { bubbles: true }));
  }, hidden);
  project.date(ids.hidden, FAR_FUTURE);
  await waitUntil('the hidden task to leave', async () => !((await display.read()).cards[ids.hidden]), 15_000);
  project.date(ids.hiddenEdit, FAR_FUTURE);
  await waitUntil('the other task to leave', async () => !((await display.read()).cards[ids.hiddenEdit]), 15_000);
  await delay(6_500);
  const beforeHidden = await display.read();
  await hide(true);
  project.clearDate(ids.hiddenEdit);
  const hiddenFrame = await waitUntil('the frame received while hidden to apply', async () => { const now = await display.read(); return ids.hiddenEdit in now.cards ? now : null; }, 15_000);
  await delay(300);
  const hiddenApplied = await display.read();
  expect('a data frame while hidden applies quietly: no wave, cue or sound', Boolean(hiddenFrame) && !hiddenApplied.wave && hiddenApplied.cards[ids.hiddenEdit].motion === '' && hiddenApplied.played === beforeHidden.played
    && !hiddenApplied.page.waves.slice(beforeHidden.page.waves.length).some(wave => wave.wave === 'true'),
    { motion: hiddenApplied.cards[ids.hiddenEdit]?.motion, played: hiddenApplied.played });
  const hiddenDate = Date.now() + 5_000;
  project.date(ids.hidden, new Date(hiddenDate).toISOString());
  await delay(CHECK_EVERY + 8_000);
  const stillHidden = await display.read();
  expect('a date passing on a hidden page changes nothing until the page returns, since the clock does not check while hidden', !(ids.hidden in stillHidden.cards), { dateIsPast: Date.now() > hiddenDate });
  await hide(false);
  const returned = await waitUntil('the card to appear on return', async () => { const now = await display.read(); return ids.hidden in now.cards ? now : null; }, 10_000, 20);
  await delay(1_500);
  const onReturn = await display.read();
  observe('a date passed while hidden, on return', { appeared: Boolean(returned), motion: onReturn.cards[ids.hidden]?.motion, waveCameFirst: onReturn.page.waves.slice(stillHidden.page.waves.length),
    playedBefore: stillHidden.played, playedAfter: onReturn.played, kind: onReturn.lastKind, change: change(onReturn) });
  expect('on return the crossing invents no edit', sameChange(change(onReturn), ['hidden'], [], []), change(onReturn));
  await delay(6_500);

  const moveClock = ms => display.page.evaluate(ms => { Date.now = () => __scratch.realNow() + ms; }, ms);
  await moveClock(3 * DAY);
  const ahead = await waitUntil('the check to find the clock ahead', async () => { const now = await display.read(); return now.cards[ids.clock]?.motion === 'arrival' ? now : null; }, CHECK_EVERY + 10_000, 100);
  expect('a clock run ahead past a date brings the card in, on the ordinary arrival', ahead && sameChange(change(ahead), ['clock'], [], []), ahead && change(ahead));
  await delay(6_500);
  await moveClock(0);
  const corrected = await waitUntil('the check to find the clock corrected', async () => { const now = await display.read(); return now.cards[ids.clock]?.motion === 'departure' ? now : null; }, CHECK_EVERY + 10_000, 100);
  expect('correcting it back takes the card out again, on the ordinary departure', corrected && sameChange(change(corrected), [], ['clock'], []), corrected && change(corrected));
  await delay(5_500);
  await closeDisplay(report, display, 'page-clock');
};
