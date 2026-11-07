// Opt-in integration test: the compiled bundle in Chromium against a daemon this test starts on its
// own temporary directories, with real time and real writes. It proves the Rust wiring that the
// synthetic checks cannot reach, and scripts/check-wallboard.py does not run it. See docs/design/wallboard.md.
const fs = require('node:fs'), path = require('node:path');
const { requestedBinaries, openEnvironment } = require('./scratch-daemon/environment.cjs');
const { openReport } = require('./scratch-daemon/report.cjs');
const { openDaemon } = require('./scratch-daemon/daemon.cjs');
const { startCaddyStandIn } = require('./scratch-daemon/caddy-stand-in.cjs');
const { BUILD } = require('./scratch-daemon/display.cjs');
const { launchChromium } = require('./browser/fixture-server.cjs');

const SCENARIOS = {
  'first-contact': require('./scratch-daemon/scenarios/first-contact.cjs'),
  dates: require('./scratch-daemon/scenarios/dates.cjs'),
  reconnect: require('./scratch-daemon/scenarios/reconnect.cjs'),
  'page-clock': require('./scratch-daemon/scenarios/page-clock.cjs'),
  membership: require('./scratch-daemon/scenarios/membership.cjs'),
  relations: require('./scratch-daemon/scenarios/relations.cjs'),
  'card-text': require('./scratch-daemon/scenarios/card-text.cjs'),
};

(async () => {
  const chosen = process.env.SCRATCH_SCENARIOS ? process.env.SCRATCH_SCENARIOS.split(',') : Object.keys(SCENARIOS);
  for (const name of chosen) if (!SCENARIOS[name]) throw Error(`No scenario ${name}; there are ${Object.keys(SCENARIOS).join(', ')}`);
  if (!fs.existsSync(path.join(__dirname, '../dist'))) throw Error('Run scripts/build-wallboard.sh first: this test drives the bundle it builds');
  const binaries = requestedBinaries();
  const root = openEnvironment();
  const report = openReport(path.join(root, 'logs'));
  const daemon = openDaemon(root, binaries, report);
  for (const signal of ['SIGINT', 'SIGTERM']) process.once(signal, () => { daemon.stop(); process.exit(1); });
  report.log(`scenarios ${chosen.join(', ')}; run ${root}`);
  let browser, server;
  try {
    daemon.assertNoneAnswers();
    report.observe('versions', daemon.versions());
    server = await startCaddyStandIn(root, report);
    browser = await launchChromium({ args: ['--autoplay-policy=no-user-gesture-required', '--mute-audio'] });
    report.observe('browser', browser.version());
    report.observe('build under test', BUILD);
    for (const name of chosen) {
      report.log(`scenario ${name}`);
      // A scenario that stops early fails the run and leaves the others to run.
      try { await SCENARIOS[name]({ report, daemon, server, browser }); }
      catch (error) { report.expect(`scenario ${name} completed: ${error.message}`, false, error.stack); }
      daemon.expectSameDaemon(`after ${name}`);
    }
  } catch (error) {
    report.expect(`the run completed: ${error.message}`, false, error.stack);
  } finally {
    await browser?.close().catch(() => {});
    await server?.close();
    daemon.stop();
    const passed = report.summarize();
    process.exitCode = passed ? 0 : 1;
    // A failed run keeps what it left behind, for its logs.
    if (!passed || process.env.SCRATCH_KEEP) report.log(`kept ${root}`);
    else fs.rmSync(root, { recursive: true, force: true });
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
