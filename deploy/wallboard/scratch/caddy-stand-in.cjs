// A small stand-in for the Caddy that wallboard.py configures. It reads its routes from that very
// generation, so a route the bundle needs and wallboard.py does not generate still fails here.
const fs = require('node:fs'), http = require('node:http'), net = require('node:net'), path = require('node:path');
const { execFileSync } = require('node:child_process');

const DEPLOY = path.resolve(__dirname, '..');
const TYPES = { '.css': 'text/css', '.html': 'text/html', '.js': 'text/javascript', '.json': 'application/json', '.mjs': 'text/javascript',
  '.wasm': 'application/wasm', '.wav': 'audio/wav', '.png': 'image/png', '.md': 'text/plain', '.txt': 'text/plain' };

// Serves what wallboard.py generates: the settings route, the one WebSocket path with its Host and
// Origin checks and rewrite, the allowlisted assets, and 404 for everything else. `backend(false)`
// plays a tunnel outage: only this server's connections end, and the daemon notices nothing.
async function startCaddyStandIn(root, report) {
  const dist = path.join(DEPLOY, 'dist');
  const state = { routes: null, enabled: true, sockets: new Set(), refused: 0, upstreamPort: 0, authority: '', daemonOrigin: '', browserOrigin: '' };
  const server = http.createServer((request, response) => {
    const url = new URL(request.url, 'http://local');
    const hostname = (request.headers.host || '').split(':')[0];
    if (hostname !== '127.0.0.1') { response.writeHead(404); response.end(); return; }
    if (request.method === 'GET' && url.pathname === '/wallboard-config.json') {
      response.writeHead(200, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' });
      response.end(state.routes.config);
      return;
    }
    const asset = url.pathname === '/' ? '/index.html' : url.pathname;
    if ((request.method === 'GET' || request.method === 'HEAD') && state.routes.assets.has(asset)) {
      response.writeHead(200, { 'Content-Type': TYPES[path.extname(asset)] || 'application/octet-stream', 'Cache-Control': 'no-cache' });
      response.end(request.method === 'HEAD' ? undefined : fs.readFileSync(path.join(dist, asset)));
      return;
    }
    response.writeHead(404); response.end();
  });
  server.on('upgrade', (request, socket, head) => {
    const url = new URL(request.url, 'http://local');
    const hostname = (request.headers.host || '').split(':')[0];
    const allowed = hostname === '127.0.0.1' && request.method === 'GET' && url.pathname === state.routes.socket
      && request.headers.origin === state.browserOrigin && (request.headers.upgrade || '').toLowerCase() === 'websocket';
    if (!allowed) { socket.end('HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n'); return; }
    if (!state.enabled) { state.refused++; socket.end('HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\n\r\n'); return; }
    const upstream = net.connect(state.upstreamPort, '127.0.0.1', () => {
      const headers = { ...request.headers, host: state.authority, origin: state.daemonOrigin };
      upstream.write(`GET ${request.url} HTTP/1.1\r\n${Object.entries(headers).map(([key, value]) => `${key}: ${value}`).join('\r\n')}\r\n\r\n`);
      if (head?.length) upstream.write(head);
      upstream.pipe(socket); socket.pipe(upstream);
    });
    for (const end of [socket, upstream]) {
      state.sockets.add(end);
      end.on('error', () => end.destroy());
      end.on('close', () => { state.sockets.delete(end); (end === socket ? upstream : socket).destroy(); });
    }
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const port = server.address().port;
  state.browserOrigin = `http://127.0.0.1:${port}`;
  return {
    url: state.browserOrigin, state,
    close: () => new Promise(resolve => { for (const end of state.sockets) end.destroy(); server.close(resolve); server.closeAllConnections?.(); }),
    backend(enabled) {
      state.enabled = enabled;
      if (!enabled) for (const end of state.sockets) end.destroy();
      report.log(`backend ${enabled ? 'up' : 'down'}`);
    },
    serve(project, upstreamPort) {
      state.upstreamPort = upstreamPort; state.authority = `127.0.0.1:${upstreamPort}`; state.daemonOrigin = `http://127.0.0.1:${upstreamPort}`;
      const settings = { project_id: project, project_name: 'Scratch integration', statuses: ['proposed', 'open', 'in_progress', 'in_review'],
        sound: { enabled: true, volume: 30 }, replay: { enabled: true, interval_seconds: 3600 }, fit_to_screen: true,
        listen: { host: '127.0.0.1', port }, upstream: { host: '127.0.0.1', port: upstreamPort }, daemon_origin: state.daemonOrigin };
      const settingsFile = path.join(root, 'wallboard-settings.json');
      fs.writeFileSync(settingsFile, JSON.stringify(settings, null, 2));
      const generated = JSON.parse(execFileSync('python3', ['-c',
        'import json, sys; from pathlib import Path; import wallboard; print(json.dumps(wallboard.configuration(json.load(open(sys.argv[1])), Path(sys.argv[2]))))',
        settingsFile, dist], { cwd: DEPLOY, encoding: 'utf8', env: { ...process.env, PYTHONDONTWRITEBYTECODE: '1' } }));
      const routes = generated.apps.http.servers.wallboard.routes;
      state.routes = { config: routes[0].handle[0].body, socket: routes[1].match[0].path[0], assets: new Set(routes[2].match[0].path) };
      if (routes[1].match[0].header.Origin[0] !== state.browserOrigin) throw Error('wallboard.py names another browser origin');
      if (routes[1].handle[0].headers.request.set.Host[0] !== state.authority) throw Error('wallboard.py rewrites Host otherwise');
      report.log(`serving ${state.routes.assets.size} assets and the project's socket from daemon port ${upstreamPort}`);
    },
  };
}

module.exports = { startCaddyStandIn };
