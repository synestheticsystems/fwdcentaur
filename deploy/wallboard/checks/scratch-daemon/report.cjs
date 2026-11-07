// The record of a run, on standard output and in its logs, and the waits that say there when they run out.
const fs = require('node:fs'), path = require('node:path');

function openReport(logs) {
  fs.mkdirSync(logs);
  const line = (file, text) => fs.appendFileSync(path.join(logs, file), `${new Date().toISOString()} ${text}\n`);
  const log = text => { line('run.log', text); console.log(text); };
  const results = [];
  const expect = (name, ok, detail) => {
    results.push({ name, ok: Boolean(ok), detail });
    log(`${ok ? 'PASS' : 'FAIL'} ${name}${detail === undefined ? '' : ' ' + JSON.stringify(detail)}`);
  };
  const observe = (name, detail) => { results.push({ name, observation: detail }); log(`OBSERVE ${name} ${JSON.stringify(detail)}`); };
  const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
  const waitUntil = async (what, condition, timeout, every = 100) => {
    const end = Date.now() + timeout;
    for (;;) {
      const value = await condition();
      if (value) return value;
      if (Date.now() > end) { log(`timed out waiting: ${what}`); return null; }
      await delay(every);
    }
  };
  // Says how the run went, and whether it passed.
  const summarize = () => {
    fs.writeFileSync(path.join(logs, 'results.json'), JSON.stringify(results, null, 2));
    const failed = results.filter(result => result.ok === false);
    log(`${results.filter(result => result.ok).length} passed, ${failed.length} failed, ${results.filter(result => result.observation !== undefined).length} observations`);
    for (const failure of failed) log(`FAILED: ${failure.name} ${JSON.stringify(failure.detail)}`);
    return failed.length === 0;
  };
  return { line, log, expect, observe, delay, waitUntil, summarize };
}

module.exports = { openReport };
