// A test-owned page of the production bundle: the page's own record of what it did, a reader of the
// running display, and what every scenario expects of it.
const fs = require('node:fs'), path = require('node:path');

const DEPLOY = path.resolve(__dirname, '..');
// How often the clock checks the dates to come.
const CHECK_EVERY = 60_000;
const FAR_FUTURE = '2099-01-01';
const BUILD = JSON.parse(fs.readFileSync(path.join(DEPLOY, 'build-info.json')));

// In the page before anything runs: every interval it starts, the wave's transitions, and when each
// card comes and goes, by the page's own wall clock.
const INSTRUMENT = () => {
  const realNow = Date.now.bind(Date);
  const seen = window.__scratch = { intervals: [], waves: [], arrivals: {}, departures: {}, realNow };
  const setIntervalNative = window.setInterval;
  window.setInterval = function (handler, ms, ...rest) {
    seen.intervals.push({ ms, at: performance.now() });
    return setIntervalNative.call(window, handler, ms, ...rest);
  };
  const each = (nodes, into) => { for (const node of nodes) if (node.nodeType === 1 && node.classList?.contains('wallboard-node')) (into[node.dataset.id] ||= []).push(realNow()); };
  new MutationObserver(records => {
    for (const record of records) {
      if (record.type === 'attributes') seen.waves.push({ wave: record.target.dataset.wave, at: realNow() });
      else { each(record.addedNodes, seen.arrivals); each(record.removedNodes, seen.departures); }
    }
  }).observe(document, { subtree: true, childList: true, attributes: true, attributeFilter: ['data-wave'] });
};

const READ = async runtime => {
  const { runtimeFor, diagnosticsFor } = await import(runtime);
  const { changes, readSnapshot } = await import(runtime.replace('wallboard-runtime', 'wallboard-history'));
  const host = document.querySelector('.wallboard-graph'), r = runtimeFor(host);
  const last = r?.history.entries.at(-1);
  const change = last && changes(readSnapshot(last.before), readSnapshot(last.after));
  return { wave: Boolean(r?.wave), banner: r?.banner.textContent,
    cards: r ? Object.fromEntries([...r.nodes].map(([id, card]) => [id, { motion: card.dom.dataset.motion || '', glow: +card.glow.style.opacity,
      wash: +card.wash.style.opacity, labels: [...card.dom.querySelectorAll('.wallboard-availability text')].map(text => text.textContent),
      waiting: card.dom.classList.contains('availability-waiting') }])) : {},
    arrows: r ? [...r.edges.keys()] : [], history: r?.history.entries.length ?? 0, lastAt: last?.at,
    lastChange: change && { arrived: change.arrived, departed: change.departed, edited: change.edited },
    played: r?.sound.played ?? 0, lastKind: r?.sound.lastKind, diagnostics: diagnosticsFor(host), timeOrigin: performance.timeOrigin,
    page: { intervals: __scratch.intervals, waves: __scratch.waves, arrivals: __scratch.arrivals, departures: __scratch.departures },
    connection: document.querySelector('.wallboard-connection')?.dataset.connected };
};

async function openDisplay({ report, server, browser }, name) {
  const context = await browser.newContext({ viewport: { width: 1920, height: 1080 } });
  const page = await context.newPage();
  const display = { page, context, errors: [], panics: [], documents: 0, frames: [] };
  page.on('pageerror', error => { display.errors.push(String(error)); report.line('console.log', `[${name}] PAGEERROR ${error}`); });
  page.on('console', message => {
    report.line('console.log', `[${name}] ${message.type()} ${message.text()}`);
    if (/panicked|unreachable/.test(message.text())) display.panics.push(message.text());
  });
  page.on('request', request => { if (request.isNavigationRequest()) display.documents++; });
  page.on('websocket', socket => {
    for (const event of ['framereceived', 'framesent']) socket.on(event, () => display.frames.push(Date.now()));
  });
  await page.addInitScript(INSTRUMENT);
  await page.goto(server.url);
  await page.waitForSelector('.wallboard');
  const runtime = '/' + fs.readdirSync(path.join(DEPLOY, 'dist'), { recursive: true }).find(file => file.endsWith('wallboard-runtime.mjs'));
  display.read = () => page.evaluate(READ, runtime);
  display.startReplay = () => page.evaluate(async runtime => (await import(runtime)).runtimeFor(document.querySelector('.wallboard-graph')).startReplay(), runtime);
  display.armed = timeout => report.waitUntil('armed', () => page.evaluate(() => document.querySelector('.wallboard-graph')?.dataset.armed === 'true'), timeout);
  return display;
}

// A page of `project` with the tunnel up, armed and past its first frames.
async function openArmedDisplay(world, project, name) {
  const { report, daemon, server } = world;
  server.serve(project.id, daemon.port());
  server.backend(true);
  const display = await openDisplay(world, name);
  report.expect(`the display arms against the scratch daemon (${name})`, Boolean(await display.armed(45_000)));
  await report.delay(1_500);
  return display;
}

const activeIssues = diagnostics => Object.entries(diagnostics.issues).filter(([, issue]) => issue.active).map(([area, issue]) => [area, issue.code]);

function expectHealthy(report, where, state) {
  const { state: reported, milestones } = state.diagnostics;
  report.expect(`diagnostics: connected, loaded and armed, no active issues (${where})`,
    reported.connected === true && reported.loaded === true && milestones.armed > 0 && activeIssues(state.diagnostics).length === 0,
    { connected: reported.connected, loaded: reported.loaded, armed: milestones.armed, active: activeIssues(state.diagnostics), nodes: reported.nodes, edges: reported.edges });
}

const clockKeepers = state => state.page.intervals.filter(interval => interval.ms === CHECK_EVERY);

function expectQuietStart(report, where, state) {
  report.expect(`start-up is quiet: no wave, no history, no sound (${where})`, state.history === 0 && state.played === 0 && !state.wave && state.page.waves.every(wave => wave.wave !== 'true'),
    { history: state.history, played: state.played, waves: state.page.waves });
}

// What every scenario ends by saying of its page, which then closes.
async function closeDisplay(report, display, where) {
  const state = await display.read();
  expectHealthy(report, `end of ${where}`, state);
  report.expect(`the page ran the bundle under test (${where})`, state.diagnostics.build.content_sha256 === BUILD.content_sha256, state.diagnostics.build);
  report.expect(`no page errors or panics (${where})`, display.errors.length === 0 && display.panics.length === 0, { errors: display.errors, panics: display.panics });
  report.expect(`one document and one clock keeper (${where})`, display.documents === 1 && clockKeepers(state).length === 1, { documents: display.documents, keepers: clockKeepers(state).length });
  await display.context.close();
}

// A change as a person reads it: `ids` names the tasks of a scenario, and the order of the ids means nothing.
const namedBy = ids => {
  const names = Object.fromEntries(Object.entries(ids).map(([name, id]) => [id, name]));
  const named = list => list.map(id => names[id] || id).sort();
  return state => state.lastChange && { arrived: named(state.lastChange.arrived), departed: named(state.lastChange.departed), edited: named(state.lastChange.edited) };
};
const sameChange = (found, arrived, departed, edited) => JSON.stringify(found) === JSON.stringify({ arrived, departed, edited });

module.exports = { BUILD, CHECK_EVERY, FAR_FUTURE, openDisplay, openArmedDisplay, closeDisplay, expectHealthy, expectQuietStart, clockKeepers, namedBy, sameChange };
