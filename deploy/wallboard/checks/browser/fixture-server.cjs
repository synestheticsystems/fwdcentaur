// The loopback server, browser launch and clock pause shared by the synthetic browser checks and the profiler.
// It serves the fixture page, the runtime's sources with the modules the build writes beside them,
// the bundled sounds and, once built, the production WASM bundle under /built. It never proxies to a
// daemon.
const fs = require('node:fs'), http = require('node:http'), path = require('node:path');
const { chromium } = require('playwright');

const source = path.resolve(__dirname, '../../../..');
const bundle = path.resolve(__dirname, '../..');
const types = { '.css': 'text/css', '.html': 'text/html', '.js': 'text/javascript', '.json': 'application/json',
  '.mjs': 'text/javascript', '.wasm': 'application/wasm', '.wav': 'audio/wav' };

function collect(directory, prefix, assets) {
  for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
    const file = path.join(directory, entry.name), url = prefix + '/' + entry.name;
    if (entry.isDirectory()) collect(file, url, assets); else assets.set(url, file);
  }
  return assets;
}

async function startFixtureServer(port = 0) {
  const assets = new Map([['/', path.join(__dirname, 'fixture.html')]]);
  collect(path.join(source, 'crates/fwd-web/src'), '/crates/fwd-web/src', assets);
  collect(path.join(bundle, 'sounds'), '/sounds', assets);
  if (fs.existsSync(path.join(bundle, 'dist'))) collect(path.join(bundle, 'dist'), '/built', assets);
  const server = http.createServer((request, response) => {
    const file = assets.get(request.url);
    if (!file) { response.writeHead(404); response.end(); return; }
    response.setHeader('Content-Type', types[path.extname(file)] || 'application/octet-stream');
    response.end(fs.readFileSync(file));
  });
  await new Promise(resolve => server.listen(port, '127.0.0.1', resolve));
  return Object.assign(server, { assets, url: 'http://127.0.0.1:' + server.address().port });
}

// The production bundle's JavaScript module and WASM binary, as the server publishes them.
function builtWasm(server) {
  const urls = [...server.assets.keys()];
  const moduleURL = urls.find(name => /^\/built\/[^/]+\.js$/.test(name));
  const wasmURL = urls.find(name => /^\/built\/[^/]+\.wasm$/.test(name));
  if (!moduleURL || !wasmURL) throw Error('Build the WASM bundle (scripts/build-wallboard.sh) before this check');
  return { moduleURL, wasmURL };
}

// Playwright's pinned Chromium, unless CHROME names another browser.
function launchChromium(options = {}) {
  return chromium.launch({ headless: true, ...options, ...(process.env.CHROME ? { executablePath: process.env.CHROME } : {}) });
}

// clock.install leaves a page's virtual clock following real time between runFor calls, so a check
// of exact timing would race its own round trips. Paused, time moves only when the check runs it.
const pauseClock = async page => page.clock.pauseAt(await page.evaluate(() => Date.now() + 5000));

const closeServer = server => new Promise(resolve => server.close(resolve));

module.exports = { startFixtureServer, builtWasm, launchChromium, pauseClock, closeServer };
