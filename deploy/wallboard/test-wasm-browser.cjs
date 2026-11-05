// Production WASM layout adapter + production JS runtime, with no daemon or personal data.
const assert = require('node:assert/strict');
const { startFixtureServer, builtWasm, launchChromium, closeServer } = require('./fixture-server.cjs');

(async () => {
  const server = await startFixtureServer();
  let browser;
  try {
    const { moduleURL, wasmURL } = builtWasm(server);
    browser = await launchChromium();
    const page = await browser.newPage({viewport:{width:1920,height:1080}});
    const errors = [], sockets = [];
    page.on('pageerror', error => errors.push(String(error)));
    page.on('websocket', socket => sockets.push(socket.url()));
    await page.clock.install({time:new Date('2026-11-01T12:00:00Z')});
    await page.goto(server.url);
    await page.waitForFunction(() => window.fixture);
    await page.evaluate(async ({moduleURL, wasmURL}) => {
      const compiled = await import(moduleURL);
      await compiled.default({module_or_path:wasmURL});
      fixture.useLayout(compiled.wallboard_layout_json);
      fixture.settings.fit_to_screen = true;
      fixture.reset();
    }, {moduleURL, wasmURL});
    const tasks = Array.from({length:13}, (_,i) => ({id:'t'+i,title:'Synthetic task '+i,priority:i===0?1:3}));
    const edges = [{from:'t0',to:'t12'}];
    const feed = items => page.evaluate(({items,edges}) => fixture.runtime.receive(fixture.snapshot(items,edges),true,true), {items,edges});
    await feed(tasks); await page.clock.runFor(800);
    const check = async () => page.evaluate(() => {
      const r = fixture.runtime;
      const nodes = [...r.nodes.values()].map(n=>({id:n.node.id,x:n.x,y:n.y,w:n.node.width,h:n.node.height,p:n.node.priority}));
      const edges = [...r.edges.values()].map(e=>({from:e.edge.from,to:e.edge.to,points:e.points}));
      return {nodes,edges,history:r.history.entries.length,frame:r.frame,rows:+r.host.dataset.layoutRows,diagnostics:r.diagnostics.snapshot()};
    });
    let state = await check();
    assert.equal(state.nodes.length,13); assert.ok(state.rows<8);
    const history = state.history;
    await feed(tasks.map(t=>t.id==='t11'?{...t,priority:1}:t)); await page.clock.runFor(8500);
    state = await check(); assert.equal(state.history,history+1);
    for (const a of state.nodes) for (const b of state.nodes)
      if (a.x===b.x && a.y<b.y) assert.ok(a.p<=b.p,'priority order must survive the WASM adapter');
    for (const edge of state.edges) {
      const a=state.nodes.find(n=>n.id===edge.from),b=state.nodes.find(n=>n.id===edge.to);
      assert.ok(Math.abs(edge.points[0][0]-(a.x+a.w))<1,'arrow starts at the source card');
      assert.ok(Math.abs(edge.points.at(-1)[0]-b.x)<1,'arrow ends at the target card');
    }
    assert.equal(await page.evaluate(() => fixture.runtime.startReplay()), true, 'replay starts before live data interrupts it');
    await page.clock.runFor(100);
    await feed(tasks.map(t=>({...t,what:t.id==='t0'?'A newer live edit':''})));
    await page.clock.runFor(8500);
    assert.equal(await page.evaluate(()=>Boolean(fixture.runtime.replay)),false);
    await page.setViewportSize({width:1080,height:1920}); await page.clock.runFor(1000);
    state=await check(); assert.equal(state.nodes.length,13); assert.equal(state.frame,0);
    await page.evaluate(({tasks,edges}) => {
      fixture.settings.text_scale_percent=125;
      fixture.runtime.update(fixture.snapshot(tasks,edges),true,true,JSON.stringify(fixture.settings));
    },{tasks,edges});
    await page.clock.runFor(1000);
    // The fixture renders these sizes; the runtime must adopt the fresh snapshot's geometry. Rust's own card
    // sizing has a native test: wallboard_cards_grow_with_footer_rows_and_a_parent_line_and_scale_with_text.
    state=await check(); assert.ok(state.nodes.every(n=>n.w===550 && n.h===186.25));
    assert.equal(state.history,0,'temporary SVG history starts a fresh geometry context');
    assert.equal(state.diagnostics.issues.graph?.active || false,false);
    assert.deepEqual(sockets,[],'synthetic validation must never connect to a daemon');
    assert.deepEqual(errors,[]);
    console.log('PASS production WASM layout: priority reorder, wide/portrait packing, attached arrows, replay interruption, geometry revision and idle settling');
    console.log('Timing counters:',JSON.stringify(state.diagnostics.timings));
  } finally { await browser?.close(); await closeServer(server); }
})().catch(error=>{console.error(error);process.exitCode=1;});
