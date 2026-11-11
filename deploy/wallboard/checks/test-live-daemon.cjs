// Read-only end-to-end check against an already running daemon. Only this test's TCP relay
// is interrupted. Never starts, stops, configures, or sends mutations to the fwd daemon.
// Playwright comes from this directory's locked install (npm ci); CADDY names a Caddy binary.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const net = require('node:net');
const http = require('node:http');
const { createHash } = require('node:crypto');
const { spawn } = require('node:child_process');
const { launchChromium } = require('./browser/fixture-server.cjs');

async function freePort() {
  const server = net.createServer();
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const port = server.address().port;
  await new Promise(resolve => server.close(resolve));
  return port;
}
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
const digest = text => createHash('sha256').update(text).digest('hex');
const statusWithHeaders = (url, headers) => new Promise((resolve, reject) => {
  const request = http.get(url, { headers }, response => { response.resume(); resolve(response.statusCode); });
  request.on('error', reject);
});

(async () => {
  if (!process.env.WALLBOARD_CONFIG) throw Error('Set WALLBOARD_CONFIG to the configuration of an existing project with matching tasks');
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'fwd-wallboard-test-'));
  const config = JSON.parse(fs.readFileSync(process.env.WALLBOARD_CONFIG));
  const statuses = config.statuses || ['in_progress'];
  const viewport = { width: Number(process.env.VIEWPORT_WIDTH) || 1920, height: Number(process.env.VIEWPORT_HEIGHT) || 1080 };
  config.listen.port = await freePort();
  config.upstream.port = await freePort();
  fs.writeFileSync(path.join(root, 'wallboard.json'), JSON.stringify(config));
  fs.symlinkSync(path.join(__dirname, '../dist'), path.join(root, 'dist'));
  const sockets = new Set();
  let relay;
  let browser;
  let output = '';
  const caddy = spawn(process.env.PYTHON || 'python3', [path.join(__dirname, '../wallboard.py'),
    '--config', path.join(root, 'wallboard.json'), '--caddy', process.env.CADDY || 'caddy']);
  caddy.stderr.on('data', chunk => output += chunk);
  const startRelay = async () => {
    relay = net.createServer(incoming => {
      const daemon = new URL(config.daemon_origin);
      const outgoing = net.connect(Number(daemon.port), daemon.hostname);
      for (const socket of [incoming, outgoing]) {
        sockets.add(socket);
        socket.on('error', () => socket.destroy());
        socket.on('close', () => sockets.delete(socket));
      }
      incoming.pipe(outgoing).pipe(incoming);
      incoming.on('close', () => outgoing.destroy());
      outgoing.on('close', () => incoming.destroy());
    });
    await new Promise(resolve => relay.listen(config.upstream.port, '127.0.0.1', resolve));
  };
  const stopRelay = async () => {
    for (const socket of sockets) socket.destroy();
    if (relay) await new Promise(resolve => relay.close(resolve));
    relay = undefined;
  };
  const url = `http://127.0.0.1:${config.listen.port}`;
  try {
    for (let attempt = 0; attempt < 100; attempt++) {
      try { if ((await fetch(url)).ok) break; } catch {}
      if (caddy.exitCode !== null) throw Error(output);
      await delay(100);
    }
    browser = await launchChromium();
    const page = await browser.newPage({ viewport });
    // Content-free diagnostics from the production runtime, for the stages docs/design/wallboard.md splits a start into.
    const runtimeModule = '/' + fs.readdirSync(path.join(__dirname, '../dist'), { recursive: true })
      .find(name => name.endsWith('wallboard-runtime.mjs'));
    const sampleDiagnostics = () => page.evaluate(async module =>
      (await import(module)).diagnosticsFor(document.querySelector('.wallboard-graph')), runtimeModule);
    const errors = [];
    page.on('pageerror', error => errors.push(String(error)));
    const sessions = [];
    page.on('websocket', socket => {
      const session = { url: socket.url(), sent: 0, received: 0, taskSubscriptions: 0 };
      sessions.push(session);
      socket.on('framesent', frame => {
        session.sent++;
        const bytes = Buffer.from(frame.payload);
        // The pinned bincode wire format: Message::Presence (u32), raw 32-byte
        // EntityId, durable (bool). Check the actual advertised role and identity.
        if (bytes.length >= 37 && bytes.readUInt32LE(0) === 0) {
          session.node = bytes.subarray(4, 36).toString('hex');
          session.durable = bytes[36] !== 0;
        }
        if (bytes.length >= 8 && bytes.readUInt32LE(0) === 1 && bytes.readUInt32LE(4) === 0
            && bytes.includes(Buffer.from('in_progress'))) session.taskSubscriptions++;
      });
      socket.on('framereceived', () => session.received++);
    });
    let documents = 0;
    page.on('request', request => { if (request.isNavigationRequest()) documents++; });
    await page.goto(url);
    await page.waitForSelector('.wallboard');
    await delay(5000);
    assert.equal(await page.locator('.wallboard').getAttribute('data-ready'), 'false');
    assert.equal(await page.locator('.wallboard-connection').getAttribute('data-connected'), 'false');
    assert.equal(await page.locator('.boot-error').count(), 0);
    assert.match(await page.locator('body').innerText(), /Waiting for graph/);
    // Theme configuration must apply before the daemon is reachable.
    if (!config.theme_schedule?.enabled) {
      assert.equal(await page.locator('.wallboard').getAttribute('data-theme'), config.theme || 'dark-aubergine');
      const backgrounds=await page.evaluate(()=>['.wallboard','body','html'].map(selector=>getComputedStyle(document.querySelector(selector)).backgroundColor));
      assert.equal(new Set(backgrounds).size,1);
    }
    const timeOrigin = await page.evaluate(() => performance.timeOrigin);
    console.log('PASS cold start with upstream offline; local assets stay available');

    // The relay's start times on the page clock, so the diagnostic stages can exclude this test's outages.
    const relay = { started: await page.evaluate(() => performance.now()) };
    await startRelay();
    await page.waitForSelector('.wallboard[data-ready="true"]', { timeout: 45000 });
    await page.waitForSelector('.wallboard-graph svg', { timeout: 45000 });
    await page.waitForSelector('.wallboard-graph[data-loaded="true"]', { timeout: 45000 });
    await page.waitForSelector('.wallboard-connection[data-connected="true"]', {state:'attached'});
    await delay(1500);
    await page.waitForSelector('.wallboard-graph[data-armed="true"]', { timeout: 45000, state: 'attached' });
    const coldStart = await sampleDiagnostics();
    const ids = await page.locator('.dag [data-id]').evaluateAll(nodes => nodes.map(n => n.dataset.id));
    const count = await page.locator('.dag .node').count();
    assert.ok(count > 0, 'the configured live project must have matching tasks for this check');
    const displayed = await page.locator('.dag .node').evaluateAll(nodes => nodes.map(n => [...n.classList].find(c => c.startsWith('status-')).slice(7)));
    assert.ok(displayed.every(status => statuses.includes(status)));
    assert.equal(await page.locator('.dag .stub, .dag .band-label.closed, .dag .band-label.outside').count(), 0);
    assert.equal(await page.locator('button, input, select, textarea, a, nav, aside, [contenteditable="true"]').count(), 0);
    assert.equal(await page.locator('.page-head, .detail-pane, .shell').count(), 0);
    const box = await page.locator('.dag').boundingBox();
    assert.ok(box.x >= 0 && box.y >= 0 && box.x + box.width <= viewport.width && box.y + box.height <= viewport.height);
    const geometry = await page.locator('.wallboard-node').evaluateAll(nodes => nodes.map(node => {
      const point = node.transform.baseVal.getItem(0).matrix;
      const width = Number(node.querySelector('.wallboard-card').getAttribute('width'));
      const overflow = [...node.querySelectorAll('.title, .wallboard-what, .sub, .wallboard-parent, .wallboard-scope text, .wallboard-availability text')].filter(text => {
        const rect = text.getBoundingClientRect(); const card = node.querySelector('.wallboard-card').getBoundingClientRect();
        return rect.right > card.right + 1 || rect.bottom > card.bottom + 1 || rect.left < card.left - 1 || rect.top < card.top - 1;
      }).map(text => text.getAttribute('class'));
      return { x: point.e, y: point.f, width, overflow };
    }));
    const columns = new Map();
    for (const card of geometry) { columns.set(card.x, (columns.get(card.x) || 0) + 1); assert.deepEqual(card.overflow, []); assert.ok(Math.abs(card.width - (config.node_width || 360) * (config.text_scale_percent || 100) / 100) < .001); }
    assert.ok([...columns.values()].every(rows => rows <= (config.max_rows || 8)));
    assert.equal(await page.locator('.wallboard-node .title:nth-of-type(3)').count(), 0);
    console.log(`PASS live graph: ${count} matching cards, ${columns.size} columns, rows ${[...columns.values()]}, text within widened cards, no controls`);

    // The proxy itself enforces the narrow HTTP/WS surface and rejects other web origins.
    for (const route of ['/health', '/projects', '/mcp', '/stop', '/webview/register', '/p/other/one-store/ws']) {
      assert.equal((await fetch(url + route)).status, 404, route);
    }
    const socketPath = `/p/${config.project_id}/one-store/ws`;
    assert.equal((await fetch(url + socketPath, { method: 'POST' })).status, 404);
    assert.equal(await statusWithHeaders(url + socketPath, { Origin: 'http://other.example', Upgrade: 'websocket' }), 404);
    assert.equal(await statusWithHeaders(url + '/wallboard-config.json', { Host: 'other.example' }), 404);
    console.log('PASS project-only WebSocket allowlist, incoming Host/Origin checks, no control/MCP routes');

    await stopRelay();
    await page.waitForSelector('.wallboard-connection[data-connected="false"]', { timeout: 15000, state:'attached' });
    // Drain already delivered frames before taking the retained snapshot. The real project
    // is active, so comparing an earlier online SVG would race other people's live updates.
    await delay(200);
    assert.ok(await page.locator('.dag .node').count() > 0);
    const graph = digest(await page.locator('.wallboard-graph').innerHTML());
    await page.mouse.click(960, 540);
    await page.mouse.dblclick(960, 540);
    await page.mouse.move(900, 500);
    await page.mouse.down();
    await page.mouse.move(1200, 700);
    await page.mouse.up();
    await page.mouse.wheel(0, 600);
    await delay(2500);
    assert.equal(digest(await page.locator('.wallboard-graph').innerHTML()), graph);
    assert.equal(await page.evaluate(() => location.hash), '');
    assert.equal(await page.locator('.wallboard').getAttribute('data-ready'), 'true');
    assert.match(await page.locator('.wallboard-connection').innerText(), /Offline/);
    assert.equal((await fetch(url + '/fwd-web_bg.wasm')).status, 200);
    console.log('PASS tunnel loss: identical last graph retained; click/drag/wheel inert; local assets still served');

    relay.restarted = await page.evaluate(() => performance.now());
    await startRelay();
    await page.waitForSelector('.wallboard-connection[data-connected="true"]', { timeout: 45000, state:'attached' });
    await delay(2000);
    let reconnect = await sampleDiagnostics();
    for (let attempt = 0; attempt < 100 && !(reconnect.latest.catchUpApplied > reconnect.latest.connected); attempt++) {
      await delay(100); reconnect = await sampleDiagnostics();
    }
    assert.ok(reconnect.latest.baselineEnded > reconnect.latest.connected, "the reconnect's baseline wait ends");
    assert.ok(reconnect.latest.catchUpApplied >= reconnect.latest.baselineEnded, "the reconnect's catch-up is applied after it");
    assert.equal(await page.evaluate(() => performance.timeOrigin), timeOrigin);
    assert.equal(documents, 1, 'no page reload or navigation during transport recovery');
    assert.equal(await page.locator('.wallboard').getAttribute('data-ready'), 'true');
    assert.ok(sessions.at(-1).sent >= 2 && sessions.at(-1).received >= 2, 'reconnected node exchanges subscription frames');
    const admitted = sessions.filter(session => session.node);
    assert.ok(admitted.length >= 2);
    assert.ok(admitted.every(session => session.durable === false), 'wire presence must advertise ephemeral role');
    assert.equal(admitted.at(-1).node, admitted[0].node, 'same node survives transport loss');
    assert.ok(admitted[0].taskSubscriptions > 0 && admitted.at(-1).taskSubscriptions > 0,
      'the in_progress task subscription must be sent again after reconnect');
    assert.equal(await page.locator('.dag .node.status-done, .dag .node.status-cancelled, .dag .node.status-rejected').count(), 0);
    assert.deepEqual(errors, []);
    await page.setViewportSize({ width: 1080, height: 1920 });
    await delay(1000); // Debounced screen packing and its movement must settle.
    const portrait = await page.locator('.dag').boundingBox();
    assert.ok(portrait.x >= 0 && portrait.y >= 0 && portrait.x + portrait.width <= 1080 && portrait.y + portrait.height <= 1920);
    console.log('PASS automatic reconnect without reload; subscriptions exchange data; portrait/landscape fit');
    await page.setViewportSize(viewport);
    await delay(1000);
    const metrics = await page.evaluate(() => {
      const cards = [...document.querySelectorAll('.wallboard-card')];
      const title = document.querySelector('.wallboard-node .title');
      const body = document.querySelector('.wallboard-what');
      const scale = title.getScreenCTM().a;
      const paths = [...document.querySelectorAll('.dag .edge')];
      const rects = cards.map(card => card.getBoundingClientRect());
      let cardCrossings = 0;
      for (const path of paths) {
        const matrix = path.getScreenCTM();
        const length = path.getTotalLength();
        for (let distance = 2; distance < length - 2; distance += 2) {
          const point = path.getPointAtLength(distance).matrixTransform(matrix);
          if (rects.some(rect => point.x > rect.left + 1 && point.x < rect.right - 1 && point.y > rect.top + 1 && point.y < rect.bottom - 1)) cardCrossings++;
        }
      }
      const ports = paths.flatMap(path => { const matrix = path.getScreenCTM(); return [0, path.getTotalLength()].map(distance => { const point = path.getPointAtLength(distance).matrixTransform(matrix); return `${Math.round(point.x * 10)},${Math.round(point.y * 10)}`; }); });
      return { titlePx: parseFloat(getComputedStyle(title).fontSize) * scale, bodyPx: parseFloat(getComputedStyle(body).fontSize) * scale,
        columns: new Set(rects.map(r => Math.round(r.x))).size,
        widthUse: (Math.max(...rects.map(r => r.right)) - Math.min(...rects.map(r => r.left))) / document.querySelector('.wallboard-graph').clientWidth,
        cardWidthPx: rects[0].width, cardHeightPx: rects[0].height, edgeCount: paths.length,
        longestEdgePx: Math.max(...paths.map(path => path.getTotalLength() * path.getScreenCTM().a)),
        sharedPorts: ports.length - new Set(ports).size, sampledCardCrossings: cardCrossings,
        longestRoutePoints: Math.max(...paths.map(path => (path.getAttribute('d').match(/[CL]/g) || []).length + 1)) };
    });
    assert.equal(metrics.sampledCardCrossings, 0);
    assert.equal(metrics.sharedPorts, 0);
    console.log('GEOMETRY ' + JSON.stringify(metrics));
    if (process.env.SCREENSHOT) await page.screenshot({ path: process.env.SCREENSHOT });
    const availability = await page.locator('.wallboard-node').evaluateAll(nodes => ({
      blocked:nodes.filter(n=>[...n.querySelectorAll('.wallboard-availability text')].some(t=>t.textContent==='Blocked')).length,
      held:nodes.filter(n=>[...n.querySelectorAll('.wallboard-availability text')].some(t=>t.textContent==='On hold')).length,
      unknown:nodes.filter(n=>[...n.querySelectorAll('.wallboard-availability text')].some(t=>t.textContent==='Dependency unknown')).length,
    }));
    console.log('AVAILABILITY '+JSON.stringify(availability));
    if (process.env.EXPECTED_AVAILABILITY) {
      const expected=JSON.parse(fs.readFileSync(process.env.EXPECTED_AVAILABILITY));
      const shown=await page.locator('.wallboard-node').evaluateAll(nodes=>nodes.map(n=>({id:n.dataset.id,labels:[...n.querySelectorAll('.wallboard-availability text')].map(t=>t.textContent)})));
      for(const card of shown){
        const item=expected.find(x=>x.id===card.id);if(!item)continue;
        assert.deepEqual(card.labels,[[item.held,'On hold'],[item.blocked,'Blocked'],[item.issue,'Check dependency'],[item.unknown,'Dependency unknown']].filter(([show])=>show).map(([,label])=>label));
      }
      console.log('PASS availability labels agree with independently read fwd readiness');
    }

    if (process.env.CUE_SCREENSHOT) {
      await page.evaluate(async module=>{
        const {runtimeFor}=await import(module);const runtime=runtimeFor(document.querySelector('.wallboard-graph'));
        if(runtime.replay)runtime.finishReplay();
        const id=runtime.nodes.keys().next().value;
        runtime.effects.set(id,{edit:Date.now()});runtime.requestFrame();
      },runtimeModule);
      await delay(200);await page.screenshot({path:process.env.CUE_SCREENSHOT});
    }
    console.log(JSON.stringify({ project: config.project_id, boxes: count, documents,
      diagnostics: { relay, coldStart, reconnect },
      websocketSessions: sessions, errors, work: root }, null, 2));
  } catch (error) {
    console.error(output);
    throw error;
  } finally {
    await browser?.close();
    await stopRelay();
    caddy.kill('SIGTERM');
    // Caddy's default graceful shutdown can retain the test HTTP client's keep-alive
    // sockets. Bound cleanup of this child only; never send signals to the real daemon.
    setTimeout(() => {
      if (caddy.exitCode === null && caddy.signalCode === null) caddy.kill('SIGKILL');
    }, 1000).unref();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
