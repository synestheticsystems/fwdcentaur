// Production runtime, synthetic graph, virtual clock. No daemon connection.
const assert = require('node:assert/strict');
const { startFixtureServer, launchChromium, pauseClock, closeServer } = require('./fixture-server.cjs');

(async () => {
  const server = await startFixtureServer();
  const browser = await launchChromium().catch(async error => { await closeServer(server); throw error; });
  try {
    const page = await browser.newPage({ viewport: { width: 1920, height: 1080 } });
    const errors = []; page.on('pageerror', error => errors.push(String(error)));
    await page.clock.install({ time: new Date('2026-11-01T12:00:00Z') });
    await page.goto(server.url);
    await page.waitForFunction(() => window.fixture);
    await pauseClock(page);
    const feed = (tasks, edges = [], loaded = true, connected = true) => page.evaluate(({ tasks, edges, loaded, connected }) =>
      fixture.runtime.receive(fixture.snapshot(tasks, edges), loaded, connected), { tasks, edges, loaded, connected });
    const inspect = () => page.evaluate(() => {
      const runtime = fixture.runtime;
      return { mode: runtime.host.dataset.mode, armed: runtime.armed, history: runtime.history.entries.length,
        wave: Boolean(runtime.wave), frame: runtime.frame, banner: runtime.banner.textContent,
        bannerVisible: !runtime.banner.hidden,
        nodes: [...runtime.nodes].map(([id, record]) => ({ id, motion: record.dom.dataset.motion,
          opacity: +record.dom.style.opacity, countdown: +record.track.getAttribute('width'), glow: +record.glow.style.opacity,
          wash: +record.wash.style.opacity,
          bob: record.bob,
          what: record.dom.querySelector('.wallboard-what').textContent,
          priority: record.dom.querySelector('.wallboard-priority text').textContent })),
        edges: [...runtime.edges.values()].map(record => ({ from: record.edge.from, to: record.edge.to, opacity: +record.dom.style.opacity })),
        empty: runtime.empty.textContent };
    });
    const tasks = [
      { id: 'a', title: 'Coordinate the work', what: 'Actual WHAT A', priority: 1 },
      { id: 'b', title: 'Collect supporting records', what: 'Actual WHAT B', priority: 2 },
      { id: 'c', title: 'Prepare the result', what: 'Actual WHAT C', priority: 2 },
    ];
    const edges = [{ from: 'a', to: 'b' }, { from: 'a', to: 'c' }];
    await feed(tasks.slice(0, 1), [], false); await page.clock.runFor(100);
    await feed(tasks, edges); await page.clock.runFor(800);
    let state = await inspect();
    assert.equal(state.armed, true); assert.equal(state.history, 0); assert.equal(state.frame, 0);
    assert.match(state.banner, /^LIVE Now Current graph$/); assert.equal(state.bannerVisible, true);
    assert.deepEqual(await page.evaluate(() => [...document.querySelectorAll('*')]
      .filter(node => getComputedStyle(node).cursor !== 'none').map(node => node.tagName)), []);
    const changed = [{ ...tasks[0], what: 'Actual revised WHAT', priority: 0 }, tasks[2],
      { id: 'd', title: 'New work', what: 'Actual WHAT D', priority: 2 }];
    const changedEdges = [{ from: 'a', to: 'c' }, { from: 'a', to: 'd' }];
    await feed(changed, changedEdges); await page.clock.runFor(550);
    state = await inspect(); assert.equal(state.wave, true); assert.equal(state.history, 0);
    assert.ok(state.nodes.some(node => Math.abs(node.bob) > 0));
    await page.clock.runFor(650);
    state = await inspect(); assert.equal(state.wave, false); assert.equal(state.history, 1); assert.equal(state.nodes.length, 4);
    assert.equal(state.nodes.find(node => node.id === 'a').what, 'Actual revised WHAT');
    assert.equal(state.nodes.find(node => node.id === 'a').priority, 'P0');
    assert.ok(state.nodes.find(node => node.id === 'a').glow > 0);
    assert.ok(state.nodes.find(node => node.id === 'b').countdown > 0);
    assert.equal(state.nodes.find(node => node.id === 'b').what, 'Actual WHAT B');
    assert.equal(state.nodes.find(node => node.id === 'd').motion, 'arrival');
    assert.doesNotMatch(await page.locator('body').innerText(), /LEAVING VIEW|Updated|updated:/);
    const styles = await page.locator('[data-id="a"]').evaluate(node => ({
      cardStroke: getComputedStyle(node.querySelector('.wallboard-card')).stroke,
      chipStroke: getComputedStyle(node.querySelector('.wallboard-scope rect')).stroke,
      priorityWeight: getComputedStyle(node.querySelector('.wallboard-priority text')).fontWeight,
    }));
    assert.equal(styles.cardStroke, 'none'); assert.equal(styles.chipStroke, 'none'); assert.equal(styles.priorityWeight, '700');
    await page.clock.runFor(4100);
    state = await inspect(); const departing = state.nodes.find(node => node.id === 'b');
    assert.ok(departing.opacity > 0 && departing.opacity < 1);
    assert.ok(state.edges.filter(edge => edge.to === 'b').every(edge => Math.abs(edge.opacity - departing.opacity * .8) < .001));
    await page.clock.runFor(2200);
    state = await inspect(); assert.equal(state.nodes.length, 3); assert.equal(state.frame, 0);
    console.log('PASS quiet cold sync; batched wave/bob; actual WHAT/priority; borderless cards; retained departures and attached edge fade; idle RAF stops');

    // Timestamp, changed content and all cues start together; departures fade without a holding period.
    await page.evaluate(() => {
      const runtime = fixture.runtime, now = Date.now(); runtime.history.entries = [];
      for (let i = 0; i < 3; i++) {
        const before = fixture.readSnapshot(fixture.snapshot([{id:'old',title:'Departing task',priority:2},
          {id:'kept',title:'Edited task',what:'Before',priority:1}]));
        const after = fixture.readSnapshot(fixture.snapshot([{id:'new',title:'Arriving task',priority:1},
          {id:'kept',title:'Edited task',what:'After',priority:1}]));
        runtime.history.append(before, after, now - (3 - i) * 60000);
      }
      if (!runtime.startReplay()) throw Error('Replay did not start');
    });
    for (let i = 0; i < 3; i++) {
      // Content must already be updated when the period changes, before advancing the clock.
      state = await inspect();
      assert.ok(state.nodes.some(node => node.id === 'new'));
      assert.equal(state.nodes.find(node => node.id === 'kept').what, 'After');
      await page.clock.runFor(20);
      state = await inspect();
      assert.equal(state.mode, 'replay');
      assert.ok(state.nodes.find(node => node.id === 'new').wash > .2);
      assert.ok(state.nodes.find(node => node.id === 'kept').glow > .9);
      assert.ok(state.nodes.find(node => node.id === 'old').opacity < .8);
      assert.ok(state.nodes.find(node => node.id === 'old').opacity > .7);
      assert.ok(state.nodes.find(node => node.id === 'old').countdown > 0);
      await page.clock.runFor(730);
      let departure = (await inspect()).nodes.find(node => node.id === 'old');
      assert.ok(departure.opacity > .5 && departure.opacity < .65);
      await page.clock.runFor(1250);
      departure = (await inspect()).nodes.find(node => node.id === 'old');
      assert.ok(departure.opacity > .2 && departure.opacity < .3);
      await page.clock.runFor(1100);
      state = await inspect();
      assert.equal(state.mode, 'replay'); assert.deepEqual(state.nodes.map(node => node.id).sort(), ['kept', 'new']);
      assert.ok(state.nodes.every(node => node.wash === 0 && node.glow === 0));
      await page.clock.runFor(800);
      assert.equal((await inspect()).frame, 0, 'Cues and layout finish before the next step');
      await page.clock.runFor(100);
    }
    state = await inspect(); assert.equal(state.mode, 'live');
    assert.deepEqual(state.nodes.map(node => node.id).sort(), ['a','c','d']);
    console.log('PASS hidden page cursor; replay content and cues start with the timestamp; three-second fades and arrival cues settle within each four-second step');

    // Busy periods select the newest meaningful changes, keeping identical step durations.
    await page.evaluate(() => {
      const runtime = fixture.runtime, now = Date.now(); runtime.history.entries = [];
      for (let i = 0; i < 12; i++) {
        const before = fixture.readSnapshot(fixture.snapshot([{ id:'h', title:'Historical task', what:'Before '+i, priority:1 }]));
        const after = fixture.readSnapshot(fixture.snapshot([{ id:'h', title:'Historical task', what:'After '+i, priority:2 }]));
        runtime.history.append(before, after, now - 110 * 60000 + i * 9 * 60000);
      }
      const unchanged = fixture.readSnapshot(fixture.snapshot([{id:'unchanged',title:'No net change',priority:1}]));
      runtime.history.append(unchanged, unchanged, now);
      window.currentSnapshot = runtime.live.source;
      if (!runtime.startReplay()) throw Error('Replay did not start');
    });
    state = await inspect(); assert.equal(state.mode, 'replay'); assert.equal(state.wave, false);
    assert.match(state.banner, /^REPLAYING 47 min ago 1 updated$/);
    for (let i = 0; i < 5; i++) {
      await page.clock.runFor(3900);
      state = await inspect();
      assert.equal(state.mode, 'replay'); assert.equal(state.nodes[0].what, 'After ' + (7 + i));
      await page.clock.runFor(100);
    }
    await page.clock.runFor(50);
    state = await inspect(); assert.equal(state.mode, 'live'); assert.equal(state.frame, 0);
    assert.match(state.banner, /^LIVE Now Current graph$/); assert.equal(state.bannerVisible, true);
    assert.equal(await page.evaluate(() => fixture.runtime.live.source === window.currentSnapshot), true);
    assert.deepEqual(state.nodes.map(node => node.id).sort(), ['a','c','d']);
    assert.equal(await page.evaluate(() => fixture.runtime.startReplay()), true);
    await page.clock.runFor(600);
    const newest = [...changed, { id:'e', title:'Fresh live task', what:'Actual newest WHAT', priority:1 }];
    await feed(newest, changedEdges); assert.equal((await inspect()).mode, 'live');
    await page.clock.runFor(8000); assert.equal((await inspect()).nodes.length, 4);
    console.log('PASS newest meaningful changes fit the fixed replay budget; equal edit/removal step duration; no replay wave; exact current-state return; fresh live data interrupts');

    // Availability is historical card content, independent of priority and lifecycle status.
    await page.evaluate(() => { fixture.runtime.history.entries = []; });
    const waiting = newest.map(node => node.id === 'a' ? {...node, priority:1, status:'in_progress', availability:['On hold','Blocked']} : node);
    await feed(waiting, changedEdges); await page.clock.runFor(1300);
    let treatment = await page.locator('[data-id="a"]').evaluate(node => ({
      labels:[...node.querySelectorAll('.wallboard-availability text')].map(el=>el.textContent),
      priority:node.querySelector('.wallboard-priority text').textContent,
      cardFilter:getComputedStyle(node).filter,
      textFilter:getComputedStyle(node.querySelector('.title')).filter,
      badgeFilter:getComputedStyle(node.querySelector('.wallboard-priority')).filter,
      opacity:node.style.opacity,
      glow:+node.querySelector('.wallboard-edit-glow').style.opacity,
      blur:getComputedStyle(node.querySelector('.wallboard-edit-glow')).filter,
      editWash:getComputedStyle(node.querySelector('.wallboard-wash')).fill,
      removedRing:!node.querySelector('foreignObject'),
    }));
    assert.deepEqual(treatment.labels,['On hold','Blocked']); assert.equal(treatment.priority,'P1');
    assert.equal(treatment.cardFilter,'opacity(0.5)'); assert.equal(treatment.textFilter,'none'); assert.equal(treatment.badgeFilter,'none');
    assert.equal(treatment.opacity,'1'); assert.ok(treatment.glow>.3); assert.match(treatment.blur,/blur/);assert.ok(treatment.removedRing);
    assert.equal(treatment.editWash,'rgba(0, 0, 0, 0)','Edit cues leave the card face unchanged');
    const glowSamples=[];
    for(let i=0;i<10;i++){
      glowSamples.push((await inspect()).nodes.find(n=>n.id==='a').glow);
      await page.clock.runFor(100);
    }
    assert.ok(Math.max(...glowSamples)-Math.min(...glowSamples)>.4,'Edit glow must visibly throb');
    await page.clock.runFor(6500);
    const available=waiting.map(node=>node.id==='a'?{...node,availability:[]}:node);
    await feed(available,changedEdges);await page.clock.runFor(1300);
    assert.equal(await page.locator('[data-id="a"] .wallboard-availability').count(),0);
    assert.equal(await page.locator('[data-id="a"]').evaluate(n=>getComputedStyle(n).filter),'none');
    assert.ok((await inspect()).nodes.find(n=>n.id==='a').glow>.3);
    await page.clock.runFor(6500);
    await page.evaluate(()=>{const r=fixture.runtime;r.history.entries=r.history.entries.slice(-1);if(!r.startReplay())throw Error('No availability replay');});
    assert.equal(await page.locator('[data-id="a"] .wallboard-availability').count(),0);
    await page.clock.runFor(50);assert.ok((await inspect()).nodes.find(n=>n.id==='a').glow>.3);
    await page.clock.runFor(4100);assert.equal((await inspect()).mode,'live');
    console.log('PASS blocked/on-hold cards faded to 50%; priority unchanged; visible update glow; unblock and historical replay cues');

    // Quiet displays reach back beyond a day instead of losing all replay content.
    await page.evaluate(() => {
      const runtime = fixture.runtime;
      runtime.history.entries = [{ at: Date.now() - 26 * 3600000, build: runtime.history.build,
        before: fixture.snapshot([{id:'quiet',title:'Quiet day',what:'Before',priority:1}]),
        after: fixture.snapshot([{id:'quiet',title:'Quiet day',what:'Yesterday',priority:1}]) }];
      if (!runtime.startReplay()) throw Error('Old retained history did not replay');
    });
    assert.match((await inspect()).banner, /^REPLAYING 26 h ago 1 updated$/);
    await page.clock.runFor(3900); assert.equal((await inspect()).nodes[0].what, 'Yesterday');
    await page.clock.runFor(150); assert.equal((await inspect()).mode, 'live');
    console.log('PASS quiet replay reaches back beyond 24 hours with the same step duration');

    // Disconnect during a wave cannot leave an old timer to overwrite the next snapshot.
    const queued = newest.map(node => node.id === 'a' ? { ...node, what:'Before disconnect' } : node);
    await feed(queued, changedEdges); await page.clock.runFor(400);
    await feed(queued, changedEdges, true, false); await page.clock.runFor(1000);
    assert.match((await inspect()).banner, /^OFFLINE Last view Reconnecting/);
    const reconnect = queued.filter(node => node.id !== 'c').map(node => node.id === 'a' ? { ...node, what:'Observed on reconnect' } : node);
    await feed(reconnect, [{from:'a',to:'d'}]); await page.clock.runFor(8500);
    state = await inspect(); assert.equal(state.nodes.length, 3); assert.equal(state.frame, 0);
    assert.match(state.banner, /^LIVE Now Current graph$/);
    assert.equal(state.nodes.find(node => node.id === 'a').what, 'Observed on reconnect');
    const historyCount = state.history;
    await page.reload(); await page.waitForFunction(() => window.fixture);
    const coldDifferent = reconnect.map(node => ({...node, what:'Changed while page was closed'}));
    await feed(coldDifferent, [{from:'a',to:'d'}]); await page.clock.runFor(900);
    state = await inspect(); assert.equal(state.history, historyCount); assert.equal(state.wave, false); assert.equal(state.frame, 0);
    await feed([]); await page.clock.runFor(1300);
    state = await inspect(); assert.equal(state.nodes.length, 3); assert.ok(state.nodes.every(node => node.countdown > 0));
    await page.clock.runFor(5000);
    state = await inspect(); assert.equal(state.nodes.length, 0); assert.match(state.empty, /No matching tasks/); assert.equal(state.frame, 0);
    console.log('PASS offline graph retained; pending wave cannot overwrite reconnect; persisted reload is quiet; final-card departure reaches empty graph');

    await page.emulateMedia({ reducedMotion:'reduce', colorScheme:'dark' });
    await feed(tasks, edges); await page.clock.runFor(400);
    state = await inspect(); assert.equal(state.wave, false); assert.ok(state.nodes.every(node => !node.bob));
    await page.clock.runFor(6500); assert.equal((await inspect()).frame, 0);
    const edited = tasks.map(node => node.id === 'a' ? {...node,what:'Reduced-motion actual WHAT'} : node);
    await feed(edited, edges); await page.clock.runFor(500);
    state = await inspect(); assert.equal(state.wave, false);
    assert.ok(state.nodes.find(node => node.id === 'a').glow > 0);
    const sanitation = await page.evaluate(() => {
      const source = fixture.snapshot([{id:'safe',title:'Safe',priority:1}]).replace('<svg ', '<svg onload="alert(1)" ')
        .replace('</svg>', '<script>alert(1)</script><image href="https://invalid.example/pixel"/></svg>');
      const safe = fixture.readSnapshot(source).source;
      return !/onload|<script|<image|https:\/\/invalid/.test(safe);
    });
    assert.equal(sanitation, true); assert.deepEqual(errors, []);
    console.log('PASS reduced-motion stationary cues/dark palette; saved SVG rejects scripts, event handlers and external resources; no browser errors');

    // A new layout caused by a priority change moves the existing card instead of jumping.
    await page.emulateMedia({ reducedMotion: 'no-preference' });
    await page.evaluate(() => fixture.reset());
    const ordered = [{id:'a',title:'Earlier work',priority:2}, {id:'b',title:'Promoted work',priority:3}];
    await feed(ordered); await page.clock.runFor(800);
    await feed([{...ordered[1],priority:1}, ordered[0]]); await page.clock.runFor(1200);
    const moving = await page.evaluate(() => {
      const r=fixture.runtime.nodes.get('b');
      return {y:r.y, priority:r.dom.querySelector('.wallboard-priority text').textContent};
    });
    assert.ok(moving.y > 24 && moving.y < 189, 'promoted card is between its old and new rows');
    assert.equal(moving.priority, 'P1');
    await page.clock.runFor(700);
    assert.equal(await page.evaluate(() => fixture.runtime.nodes.get('b').y), 24);
    console.log('PASS updated priority is immediate while its card animates upward');

    // The persistent status has its own row, readable at TV size and clear of the graph.
    await page.clock.runFor(6500);
    for (const width of [3840, 1920, 736, 320]) {
      await page.setViewportSize({ width, height: width === 3840 ? 2160 : 1080 });
      const bounds = await page.evaluate(() => {
        const banner = document.querySelector('.wallboard-status'), graph = document.querySelector('.wallboard-graph');
        return { bottom: banner.getBoundingClientRect().bottom, top: graph.getBoundingClientRect().top,
          overflow: document.documentElement.scrollWidth > innerWidth,
          tag: parseFloat(getComputedStyle(document.querySelector('.wallboard-mode-tag')).fontSize),
          time: parseFloat(getComputedStyle(document.querySelector('.wallboard-mode-time')).fontSize) };
      });
      assert.ok(bounds.bottom <= bounds.top); assert.equal(bounds.overflow, false);
      if (width === 3840) { assert.ok(bounds.tag >= 32); assert.ok(bounds.time >= 48); }
    }
    assert.equal((await inspect()).bannerVisible, true);
    assert.equal(await page.evaluate(() => document.querySelector('.wallboard').dataset.theme), 'dark-aubergine');
    for (const [theme, expected] of [['dark-aubergine', 'rgb(81, 64, 76)'], ['charcoal', 'rgb(107, 98, 88)'],
      ['oxblood-copper', 'rgb(86, 52, 65)'], ['indigo-saffron', 'rgb(56, 61, 93)'], ['graphite-coral', 'rgb(104, 104, 102)'], ['linen-indigo', 'rgb(196, 204, 230)'],
      ['porcelain-coral', 'rgb(237, 186, 169)'], ['sand-plum', 'rgb(205, 186, 212)']]) {
      await page.evaluate(theme => { fixture.settings.theme = theme; fixture.reset(); }, theme);
      await feed(tasks, edges); await page.clock.runFor(800);
      assert.equal(await page.evaluate(() => document.querySelector('.wallboard').dataset.theme), theme);
      assert.equal(await page.locator('[data-id="a"] .wallboard-card').evaluate(node => getComputedStyle(node).fill), expected);
      if (['linen-indigo', 'porcelain-coral', 'sand-plum'].includes(theme)) {
        const palette = await page.evaluate(() => {
          const root=document.querySelector('.wallboard'), style=getComputedStyle(root);
          const rgb=css => css.match(/[\d.]+/g).slice(0,3).map(Number);
          const luminance=css => rgb(css).map(n=>n/255).map(v=>v<=.04045?v/12.92:((v+.055)/1.055)**2.4)
            .reduce((sum,v,i)=>sum+v*[.2126,.7152,.0722][i],0);
          const contrast=(a,b)=>{const x=luminance(a),y=luminance(b);return (Math.max(x,y)+.05)/(Math.min(x,y)+.05);};
          const cssColor=name=>{const probe=document.createElement('i');probe.style.color=style.getPropertyValue(name);root.append(probe);
            const color=getComputedStyle(probe).color;probe.remove();return color;};
          const text=[];
          for(let i=0;i<4;i++) {
            text.push(contrast(cssColor('--wb-ink'),cssColor('--wb-p'+i+'-fill')));
            text.push(contrast(cssColor('--wb-muted'),cssColor('--wb-p'+i+'-fill')));
            text.push(contrast(cssColor('--wb-p'+i+'-text'),cssColor('--wb-p'+i+'-bg')));
          }
          text.push(contrast(cssColor('--wb-muted'),cssColor('--wb-scope')));
          text.push(contrast(cssColor('--wb-history-ink'),cssColor('--wb-history')));
          text.push(contrast(cssColor('--wb-status'),cssColor('--wb-bg')));
          return { background:style.backgroundColor,body:getComputedStyle(document.body).backgroundColor,
            html:getComputedStyle(document.documentElement).backgroundColor,luminance:luminance(style.backgroundColor),
            text, edge:contrast(cssColor('--wb-edge'),cssColor('--wb-bg')),
            glow:contrast(cssColor('--wb-edit'),cssColor('--wb-bg')),
            wash:getComputedStyle(fixture.runtime.nodes.get('a').wash).fill,
            glowColor:getComputedStyle(fixture.runtime.nodes.get('a').glow).fill,expectedGlow:cssColor('--wb-edit') };
        });
        assert.equal(palette.background,palette.body);assert.equal(palette.background,palette.html);
        assert.ok(palette.luminance>.75,'The full page must have a light background');
        assert.ok(palette.text.every(ratio=>ratio>=4.5),theme+' text contrast: '+JSON.stringify(palette.text));
        assert.ok(palette.edge>=3);assert.ok(palette.glow>=3);
        assert.equal(palette.wash,'rgba(0, 0, 0, 0)');assert.equal(palette.glowColor,palette.expectedGlow);
      }
    }
    // Solar changes do not need graph data and must not reset the live graph or history.
    await page.evaluate(() => {
      fixture.settings.theme_schedule={enabled:true,latitude:51.48,longitude:0,day_theme:'linen-indigo',night_theme:'indigo-saffron'};
      fixture.reset();
    });
    await page.clock.setSystemTime(new Date('2026-03-20T12:00:00Z'));
    await page.evaluate(()=>fixture.runtime.theme.refresh());
    assert.equal(await page.locator('.wallboard').getAttribute('data-theme'),'linen-indigo');
    assert.match((await inspect()).banner, /^CONNECTING/);
    await feed(tasks,edges);await page.clock.runFor(800);
    await page.evaluate(()=>{ window.savedRuntime=fixture.runtime;window.savedNode=fixture.runtime.nodes.get('a').dom;window.savedHistory=fixture.runtime.history.entries.length; });
    await page.clock.setSystemTime(new Date('2026-03-20T22:00:00Z'));
    await page.clock.runFor(60000);
    assert.equal(await page.locator('.wallboard').getAttribute('data-theme'),'indigo-saffron');
    assert.equal(await page.evaluate(()=>fixture.runtime===window.savedRuntime&&fixture.runtime.nodes.get('a').dom===window.savedNode),true);
    assert.equal((await inspect()).history,await page.evaluate(()=>window.savedHistory));
    await feed(tasks,edges,true,false);
    await page.clock.setSystemTime(new Date('2026-03-21T12:00:00Z'));await page.clock.runFor(60000);
    assert.equal(await page.locator('.wallboard').getAttribute('data-theme'),'linen-indigo');
    assert.match((await inspect()).banner,/^OFFLINE/);
    assert.deepEqual(errors,[]);
    console.log('PASS light full-page backgrounds, text/edge/glow contrast, themed effects, cold/offline solar switching without graph reset');
    await page.evaluate(() => fixture.reset());
    assert.match((await inspect()).banner, /^CONNECTING/);
    console.log('PASS persistent LIVE/OFFLINE status; relative replay ages; dedicated responsive banner row; explicit theme selection');
    // Native browser timers enforce their receiver; a virtual clock can mask bad bindings.
    const nativePage=await browser.newPage();const nativeErrors=[];
    nativePage.on('pageerror',error=>nativeErrors.push(String(error)));
    await nativePage.goto(server.url);
    await nativePage.waitForFunction(()=>window.fixture);
    const solarTimer=await nativePage.evaluate(()=>{
      fixture.settings.theme_schedule={enabled:true,latitude:51.48,longitude:0,day_theme:'linen-indigo',night_theme:'indigo-saffron'};
      fixture.reset();
      const theme=fixture.runtime.theme, armed=theme.timer, cleared=[], clearTimeout=window.clearTimeout;
      // Coming back into view clears the schedule's timer and sets the next with the browser's own timers.
      window.clearTimeout=timer=>{cleared.push(timer);return clearTimeout.call(window,timer);};
      try { document.dispatchEvent(new Event('visibilitychange')); } finally { window.clearTimeout=clearTimeout; }
      return {mode:document.querySelector('.wallboard').dataset.themeMode,armed:typeof armed,
              cleared:cleared.includes(armed),rearmed:typeof theme.timer==='number'&&theme.timer!==armed};
    });
    assert.deepEqual(solarTimer,{mode:'solar',armed:'number',cleared:true,rearmed:true});
    assert.deepEqual(nativeErrors,[]);
    console.log('PASS native browser timers arm the solar schedule, and arm it again when the page comes back into view');
    // Resizing a fitted graph reuses card DOM and must not manufacture task changes or sounds.
    await nativePage.evaluate(({tasks,edges})=>{
      fixture.settings.fit_to_screen=true;fixture.reset();
      const runtime=fixture.runtime, layout=runtime.layout;
      window.layoutCalls=0;window.soundCalls=0;
      runtime.layout=input=>{window.layoutCalls++;return layout(input);};
      runtime.sound.play=()=>{window.soundCalls++;};
      runtime.receive(fixture.snapshot(tasks,edges),true,true);
    },{tasks,edges});
    await nativePage.waitForFunction(()=>fixture.runtime.armed&&!fixture.runtime.frame);
    const beforeResize=await nativePage.evaluate(()=>{
      window.fittedNode=fixture.runtime.nodes.get('a').dom;
      fixture.runtime.render(fixture.runtime.scene(),false);
      return {calls:window.layoutCalls,history:fixture.runtime.history.entries.length};
    });
    await nativePage.setViewportSize({width:1080,height:1920});
    await nativePage.waitForFunction(()=>{
      const r=fixture.runtime;
      return JSON.parse(r.layoutInput).viewport_width===r.host.clientWidth&&!r.frame;
    });
    const afterResize=await nativePage.evaluate(()=>({calls:window.layoutCalls,
      history:fixture.runtime.history.entries.length,sounds:window.soundCalls,
      sameNode:fixture.runtime.nodes.get('a').dom===window.fittedNode}));
    assert.equal(afterResize.calls,beforeResize.calls+1);
    assert.equal(afterResize.history,beforeResize.history);assert.equal(afterResize.sounds,0);
    assert.equal(afterResize.sameNode,true);assert.deepEqual(nativeErrors,[]);
    // Rejected graph input cannot freeze transport health or manufacture a later catch-up cue.
    await nativePage.evaluate(() => {
      for(let i=0;i<100;i++) fixture.runtime.receive('<broken',true,false);
    });
    let diagnostic=await nativePage.evaluate(()=>({report:fixture.runtime.diagnostics.snapshot(),
      banner:fixture.runtime.banner.textContent,same:fixture.runtime.nodes.get('a').dom===window.fittedNode}));
    assert.equal(diagnostic.same,true);assert.match(diagnostic.banner,/^OFFLINE/);
    assert.equal(diagnostic.report.state.connected,false);
    assert.equal(diagnostic.report.issues.graph.count,100);
    assert.equal(diagnostic.report.issues.graph.reason,'Invalid SVG snapshot');
    await nativePage.evaluate(()=>fixture.runtime.receive('<broken',true,true));
    assert.match(await nativePage.evaluate(()=>fixture.runtime.banner.textContent),/^STALE/);
    // A new valid frame, not merely the cached last one, recovers: its change appears as silent catch-up.
    await nativePage.evaluate(({tasks,edges})=>fixture.runtime.receive(fixture.snapshot(tasks.map(task=>
      task.id==='a'?{...task,what:'Changed while updates were malformed'}:task),edges),true,true),{tasks,edges});
    await nativePage.waitForFunction(()=>fixture.runtime.nodes.get('a').dom.querySelector('.wallboard-what').textContent
      ==='Changed while updates were malformed'&&!fixture.runtime.syncing&&!fixture.runtime.pending&&!fixture.runtime.frame);
    diagnostic=await nativePage.evaluate(()=>({report:fixture.runtime.diagnostics.snapshot(),banner:fixture.runtime.banner.textContent}));
    assert.equal(diagnostic.report.issues.graph.active,false);
    assert.match(diagnostic.banner,/^LIVE Now Current graph$/);
    assert.equal(await nativePage.evaluate(()=>window.soundCalls),0);
    // Presentation revisions apply in place, and validation is atomic.
    const configured=await nativePage.evaluate(()=>{
      const r=fixture.runtime,h=r.history,node=r.nodes.get('a').dom;
      const send=next=>r.update(r.lastSource,true,true,JSON.stringify(next));
      send({...r.settings,theme:'porcelain-coral',theme_schedule:null,
        replay:{enabled:false,interval_seconds:120},sound:{enabled:false,volume:60},fit_to_screen:false});
      for(const next of [{...r.settings,theme:'invalid'},{...r.settings,statuses:['proposed']}]) send(next);
      return {sameHistory:h===r.history,sameNode:node===r.nodes.get('a').dom,
        theme:r.root.dataset.theme,replayTimer:r.timers.has('replay'),
        rejected:r.diagnostics.issues.configuration.count,connected:r.connected};
    });
    assert.deepEqual(configured,{sameHistory:true,sameNode:true,theme:'porcelain-coral',replayTimer:false,rejected:2,connected:true});
    // The status row is there to remove, so that its absence after disposal is the runtime's doing.
    assert.equal(await nativePage.locator('.wallboard-status').count(),1);
    await nativePage.evaluate(()=>fixture.runtime.dispose());
    assert.equal(await nativePage.locator('.wallboard-status').count(),0);
    assert.deepEqual(nativeErrors,[]);
    await nativePage.close();
    console.log('PASS malformed updates preserve the last graph and transport health, with their reason; a new valid frame recovers silently; settings apply atomically without replacing cards/history; disposal removes status UI');
    console.log('PASS cached layout; native viewport resize preserves card identity and creates no history or sound');
  } finally { await browser.close(); await closeServer(server); }
})().catch(error => { console.error(error); process.exitCode = 1; });
