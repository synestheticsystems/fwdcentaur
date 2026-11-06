// A cold start with no tunnel: the page waits for the daemon for longer than the clock's first check
// could take to come round, then joins when the tunnel appears, caching none of the collections it
// never shows. A message then reaches a card without packing the cards again. Then a date reaches the
// open page through the daemon, on a project whose catalog first heard of the field from this
// display, while a page of the daemon's own build goes on following the graph.
const { CHECK_EVERY, FAR_FUTURE, openDisplay, closeDisplay, expectHealthy, expectQuietStart, clockKeepers, namedBy, sameChange, cachedCollections } = require('../display.cjs');

module.exports = async function firstContact(world) {
  const { report, daemon, server, browser } = world;
  const { expect, observe, line, delay, waitUntil } = report;
  const project = daemon.project('first-contact');
  const ids = { shown: project.task('Shown with no date'), toldFirst: project.task('Told its first date while the display watches') };
  const change = namedBy(ids);
  // What the display never shows, which the daemon's page caches and the display must not.
  project.decide('A ruling the display never shows');
  project.highlight('notes.md', 'A line the display never shows');
  observe('the catalog before the display connects', { not_before: project.registered('not_before') });

  const port = daemon.port();
  const daemonPageContext = await browser.newContext(), daemonPage = await daemonPageContext.newPage(), daemonPageErrors = [];
  daemonPage.on('pageerror', error => { daemonPageErrors.push(String(error)); line('console.log', `[daemon's page] PAGEERROR ${error}`); });
  await daemonPage.goto(`http://127.0.0.1:${port}/?p=${project.id}#/tasks`);
  const daemonPageShows = text => daemonPage.getByText(text).first().waitFor({ timeout: 20_000 }).then(() => true, () => false);
  expect('a page of the daemon\'s own build shows the graph before the display connects', await daemonPageShows('Shown with no date'));

  server.serve(project.id, port);
  server.backend(false);
  const started = Date.now();
  const display = await openDisplay(world, 'first-contact');
  await delay(CHECK_EVERY + 5_000);
  const offline = await display.page.evaluate(() => ({ ready: document.querySelector('.wallboard').dataset.ready, wait: document.querySelector('.wallboard-wait')?.textContent,
    intervals: __scratch.intervals.filter(interval => interval.ms === 60000) }));
  expect('offline boot: past the first check with no store and no panic', display.errors.length === 0 && display.panics.length === 0,
    { errors: display.errors, panics: display.panics, waited: Date.now() - started, refusedHandshakes: server.state.refused });
  expect('offline boot: still waiting for the graph, the connector retrying', offline.ready === 'false' && /Waiting for graph/.test(offline.wait) && server.state.refused > 0,
    { ...offline, refusedHandshakes: server.state.refused });
  expect('offline boot: no clock check was started before the store', offline.intervals.length === 0, offline.intervals);

  server.backend(true);
  expect('the display joins when the tunnel appears', Boolean(await display.armed(60_000)));
  await delay(1_500);
  let state = await display.read();
  expectHealthy(report, 'after a cold offline start', state);
  const clocks = clockKeepers(state);
  expect('the clock keeper started once, only after the store opened', clocks.length === 1 && clocks[0].at >= state.diagnostics.boot.context,
    { intervals: clocks, storeOpenedAt: state.diagnostics.boot.context });
  expect('both tasks with no date are shown', ids.shown in state.cards && ids.toldFirst in state.cards, Object.keys(state.cards).length);
  expectQuietStart(report, 'after an offline boot', state);
  expect('the display connecting registered not_before in the catalog of the daemon it reads', project.registered('not_before'));
  const byApp = await cachedCollections(daemonPage), byDisplay = await cachedCollections(display.page);
  const uncached = byApp.filter(name => !byDisplay.includes(name));
  expect('the display caches every collection the daemon\'s page does but highlights and decisions, which it never opens',
    byDisplay.every(name => byApp.includes(name)) && uncached.length === 2 && uncached.some(name => /anchor/i.test(name)) && uncached.some(name => /decision/i.test(name)),
    { app: byApp, display: byDisplay });

  const beforeMessage = await display.read();
  project.comment(ids.shown, 'A message on the task shown');
  const counted = await waitUntil('the message to be counted', async () => { const now = await display.read(); return now.cards[ids.shown]?.messages === 1 ? now : null; }, 15_000);
  const timed = (state, stage) => state?.diagnostics.timings[stage]?.count ?? 0;
  expect('a message on a task draws its count without packing the cards again',
    Boolean(counted) && timed(counted, 'rustMarkup') > timed(beforeMessage, 'rustMarkup')
      && timed(beforeMessage, 'rustPacking') > 0 && timed(counted, 'rustPacking') === timed(beforeMessage, 'rustPacking'),
    { markup: [timed(beforeMessage, 'rustMarkup'), timed(counted, 'rustMarkup')], packing: [timed(beforeMessage, 'rustPacking'), timed(counted, 'rustPacking')] });

  const stillReads = project.daemonBuild(['task', 'show', ids.toldFirst], { allowFailure: true });
  expect('the daemon\'s own client still reads the graph once the display has registered the field', stillReads.status === 0, stillReads.status);
  project.rename(ids.shown, 'Shown with no date, renamed');
  expect('the daemon\'s page, connected through the registration, still follows the graph', await daemonPageShows('Shown with no date, renamed') && daemonPageErrors.length === 0, daemonPageErrors);

  // Past a real check with the store open, the renaming's cue and its sound's cooldown done.
  await delay(CHECK_EVERY + 9_000);
  expect('the clock check runs with the store open and does not panic', display.errors.length === 0 && display.panics.length === 0, { errors: display.errors, panics: display.panics });

  project.date(ids.toldFirst, FAR_FUTURE);
  const leaving = await waitUntil('the first date ever to move a shown task out', async () => {
    const now = await display.read(); return now.cards[ids.toldFirst]?.motion === 'departure' ? now : null;
  }, 15_000);
  expect('a date set while the display watches moves the card out, the field first registered by this display', Boolean(leaving), leaving && change(leaving));
  expect('that departure is no edit', leaving && sameChange(change(leaving), [], ['toldFirst'], []), leaving && change(leaving));
  await waitUntil('the card to go', async () => !((await display.read()).cards[ids.toldFirst]), 8_000);
  project.clearDate(ids.toldFirst);
  const back = await waitUntil('the card to return', async () => { const now = await display.read(); return now.cards[ids.toldFirst]?.motion === 'arrival' ? now : null; }, 15_000);
  expect('clearing it brings the card back as an arrival', back && sameChange(change(back), ['toldFirst'], [], []), back && change(back));

  project.rename(ids.toldFirst, 'Told its first date, renamed');
  expect('the daemon\'s page still follows the graph once a task carries the field it does not know', await daemonPageShows('Told its first date, renamed') && daemonPageErrors.length === 0, daemonPageErrors);
  await daemonPageContext.close();
  await closeDisplay(report, display, 'first-contact');
};
