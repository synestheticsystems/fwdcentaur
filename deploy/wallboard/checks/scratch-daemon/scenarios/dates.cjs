// What a date does to the drawing: the first load leaves out what is not yet due, and the date
// arriving brings its card in with no write anywhere, by the clock alone. Setting and clearing a date
// are real writes that move a card out and in, and none of it is an edit.
const { CHECK_EVERY, FAR_FUTURE, openArmedDisplay, closeDisplay, expectHealthy, expectQuietStart, clockKeepers, namedBy, sameChange } = require('../display.cjs');

const iso = ms => new Date(ms).toISOString();

module.exports = async function dates(world) {
  const { report, daemon } = world;
  const { expect, observe, log, delay, waitUntil } = report;
  const project = daemon.project('dates');
  const ids = {};
  ids.far = project.task('Dated well ahead');
  ids.soon = project.task('Coming due while the display runs');
  ids.past = project.task('Dated in the past');
  ids.plain = project.task('No date at all');
  ids.waiter = project.task('Waits for the task dated well ahead', '--after', ids.far);
  ids.soonWaiter = project.task('Waits for the task coming due', '--after', ids.soon);
  ids.plainWaiter = project.task('Waits for the task with no date', '--after', ids.plain);
  ids.soonAlsoWaits = project.task('Comes due too, and waits for the task dated well ahead', '--after', ids.far);
  ids.started = project.task('Started, with a date still to come');
  const change = namedBy(ids);
  project.start(ids.started);
  project.date(ids.far, FAR_FUTURE);
  project.date(ids.past, '2000-01-01');
  project.date(ids.started, FAR_FUTURE);
  const shownBy = project.show(ids.far);
  expect('the date survives the daemon, in the client\'s JSON', shownBy.not_before === new Date(2099, 0, 1).getTime(), shownBy.not_before);
  const noDate = project.show(ids.plain);
  expect('a task with no date reads as null', noDate.not_before === null, noDate.not_before);
  const ownClientShow = project.daemonBuild(['task', 'show', ids.far], { allowFailure: true });
  const ownClientList = project.daemonBuild(['task', 'list'], { allowFailure: true });
  expect('the daemon\'s own client still reads a dated task and lists the graph', ownClientShow.status === 0 && ownClientList.status === 0, { show: ownClientShow.status, list: ownClientList.status });
  const ownClientRead = JSON.parse(ownClientShow.stdout);
  observe('readiness of a dated task by the daemon\'s own client', { readiness: ownClientRead.readiness, status: ownClientRead.status });

  const soonDate = Date.now() + 70_000;
  project.date(ids.soon, iso(soonDate));
  project.date(ids.soonAlsoWaits, iso(soonDate));
  log(`the task coming due is dated ${iso(soonDate)}`);
  const display = await openArmedDisplay(world, project, 'dates');
  let state = await display.read();
  const timeOrigin = state.timeOrigin;
  expectHealthy(report, 'after the first load', state);
  expect('the far-future task is hidden, and never appeared even during the load', !(ids.far in state.cards) && !state.page.arrivals[ids.far], state.page.arrivals[ids.far]);
  expect('the task coming due is hidden before its date', !(ids.soon in state.cards) && Date.now() < soonDate, { msToDate: soonDate - Date.now() });
  expect('the dated-past task is shown', ids.past in state.cards);
  expect('the no-date task is shown', ids.plain in state.cards);
  expect('a started task shows whatever its date', ids.started in state.cards);
  const waiter = state.cards[ids.waiter];
  expect('the task depending on the hidden one is shown as blocked', Boolean(waiter) && waiter.labels.includes('Blocked') && waiter.waiting, waiter);
  expect('no arrow is drawn to the hidden task', !state.arrows.some(arrow => arrow.includes(ids.far)), state.arrows.length);
  expect('no card or arrow names the hidden task in the DOM', await display.page.evaluate(id => !document.querySelector(`[data-id="${id}"], [data-from="${id}"], [data-to="${id}"]`), ids.far));
  const clocks = clockKeepers(state);
  expect('one clock keeper, started after the store opened', clocks.length === 1 && clocks[0].at >= state.diagnostics.boot.context, { intervals: clocks, storeOpenedAt: state.diagnostics.boot.context });
  expectQuietStart(report, 'first load', state);

  // The crossing, with no command or data change anywhere: the test only waits.
  const beforeShow = project.show(ids.soon);
  const beforeEvents = project.eventCount();
  const appeared = await waitUntil('the task coming due to appear', () => display.page.evaluate(id => __scratch.arrivals[id]?.[0], ids.soon), soonDate + CHECK_EVERY + 15_000 - Date.now(), 50);
  expect('the task coming due appears at its date, not before, within one check', Boolean(appeared) && appeared >= soonDate && appeared <= soonDate + CHECK_EVERY + 5_000,
    { dated: iso(soonDate), appeared: appeared && iso(appeared), afterItsDateMs: appeared && appeared - soonDate });
  await delay(600);
  state = await display.read();
  expect('its arrival uses the ordinary cue', state.cards[ids.soon]?.motion === 'arrival' && state.cards[ids.soon].wash > 0, state.cards[ids.soon]);
  const wavesBefore = state.page.waves.filter(wave => wave.at <= appeared);
  expect('the batched wave came first', wavesBefore.at(-1)?.wave === 'false' && wavesBefore.at(-2)?.wave === 'true', wavesBefore.slice(-2));
  expect('the crossing brought one sound, an arrival', state.played === 1 && state.lastKind === 'arrival', { played: state.played, kind: state.lastKind });
  expect('the crossing is a membership change: arrived alone, nothing edited', sameChange(change(state), ['soon', 'soonAlsoWaits'], [], []), change(state));
  expect('a task arriving while its prerequisite stays hidden reads as blocked on arrival, with no edit', state.cards[ids.soonAlsoWaits]?.labels.includes('Blocked') && state.cards[ids.soonAlsoWaits].waiting && state.cards[ids.soonAlsoWaits].glow === 0, state.cards[ids.soonAlsoWaits]);
  expect('the card joined by the new arrow is not marked edited', state.cards[ids.soonWaiter]?.glow === 0 && state.arrows.includes(`${ids.soon}>${ids.soonWaiter}`), state.cards[ids.soonWaiter]);
  expect('every other card is unmarked', Object.entries(state.cards).filter(([id]) => id !== ids.soon && id !== ids.soonAlsoWaits).every(([, card]) => card.motion === '' && card.glow === 0));
  expect('the waiting card still reads as blocked', state.cards[ids.soonWaiter]?.labels.includes('Blocked'));
  const stored = ({ updated, not_before, status, title, what, assignee, priority }) => ({ updated, not_before, status, title, what, assignee, priority });
  const afterShow = project.show(ids.soon);
  expect('no data changed: the task reads the same before and after', JSON.stringify(stored(beforeShow)) === JSON.stringify(stored(afterShow)), stored(afterShow));
  expect('no data changed: the daemon holds no new event', project.eventCount() === beforeEvents, { before: beforeEvents, after: project.eventCount() });
  expect('no reload', display.documents === 1 && state.timeOrigin === timeOrigin, { documents: display.documents });
  observe('websocket frames in the five seconds around the crossing', display.frames.filter(at => Math.abs(at - appeared) < 5_000).length);
  expectHealthy(report, 'after the crossing', state);

  // A date set on a shown task, and cleared: real writes, past the sound's four-second cooldown since
  // the crossing's cue, which would drop the next one.
  await delay(Math.max(0, appeared + 4_500 - Date.now()));
  project.date(ids.plain, FAR_FUTURE);
  const leaving = await waitUntil('the dated card to leave', async () => { const now = await display.read(); return now.cards[ids.plain]?.motion === 'departure' ? now : null; }, 15_000);
  expect('setting a date on a shown task makes it leave with the ordinary departure', Boolean(leaving), leaving?.cards[ids.plain]);
  expect('that departure is no edit, of it or of the card that waits for it', leaving && sameChange(change(leaving), [], ['plain'], []) && leaving.cards[ids.plainWaiter]?.glow === 0, leaving && change(leaving));
  expect('the card that waited for it still reads as blocked, its arrow gone once the countdown ends', leaving?.cards[ids.plainWaiter]?.labels.includes('Blocked'), leaving?.cards[ids.plainWaiter]);
  await delay(600); // A cue waits for the audio context to resume.
  let sounded = await display.read();
  expect('that departure sounded once', sounded.played === 2 && sounded.lastKind === 'departure', { played: sounded.played, kind: sounded.lastKind });
  expect('it is gone once the countdown ends', Boolean(await waitUntil('the card to go', async () => !((await display.read()).cards[ids.plain]), 8_000)));
  project.clearDate(ids.plain);
  const back = await waitUntil('the card to return', async () => { const now = await display.read(); return now.cards[ids.plain]?.motion === 'arrival' ? now : null; }, 15_000);
  expect('clearing the date brings it back with the ordinary arrival', Boolean(back), back?.cards[ids.plain]);
  expect('that arrival is no edit, of it or of the card that waits for it', back && sameChange(change(back), ['plain'], [], []) && back.cards[ids.plainWaiter]?.glow === 0, back && change(back));
  await delay(600);
  sounded = await display.read();
  expect('that arrival sounded once', sounded.played === 3 && sounded.lastKind === 'arrival', { played: sounded.played, kind: sounded.lastKind });
  await closeDisplay(report, display, 'dates');
};
