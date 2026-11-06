// Synthetic presentation baseline. CDP must refer to a test-owned browser, never a user's kiosk.
const fs=require('node:fs');
const {chromium}=require('playwright');
const {startFixtureServer,builtWasm,launchChromium,closeServer}=require('./fixture-server.cjs');
(async()=>{
 const server=await startFixtureServer(Number(process.env.LOCAL_PORT)||0);
 let browser,context;
 try {
  const {moduleURL,wasmURL}=builtWasm(server);
  browser=process.env.TEST_CDP ? await chromium.connectOverCDP(process.env.TEST_CDP) : await launchChromium();
  context=await browser.newContext({viewport:{width:3840,height:2160}});
  const page=await context.newPage(),errors=[],sockets=[];
  page.on('pageerror',error=>errors.push(String(error)));page.on('websocket',socket=>sockets.push(socket.url()));
  const begin=Date.now();
  await page.goto(process.env.TEST_URL||server.url);
  await page.waitForFunction(()=>window.fixture);
  await page.evaluate(async({moduleURL,wasmURL})=>{
   const compiled=await import(moduleURL);await compiled.default({module_or_path:wasmURL});
   fixture.useLayout(compiled.wallboard_layout_json);
   fixture.settings.fit_to_screen=true;fixture.settings.replay={enabled:false,interval_seconds:180};fixture.reset();
   window.tasks=Array.from({length:13},(_,i)=>({id:'t'+i,title:'Synthetic task '+i,what:'Synthetic context',priority:i===0?1:i<5?2:3}));
   window.edges=[{from:'t0',to:'t11'},{from:'t0',to:'t12'}];
   fixture.runtime.receive(fixture.snapshot(tasks,edges),true,true);
  },{moduleURL,wasmURL});
  await page.waitForFunction(()=>fixture.runtime.armed&&!fixture.runtime.frame,{},{timeout:60000});
  const coldPresentationMs=Date.now()-begin;
  const before=await page.evaluate(()=>fixture.runtime.diagnostics.snapshot());
  await page.evaluate(()=>{
   tasks[1]={...tasks[1],what:'A representative live edit'};
   fixture.runtime.receive(fixture.snapshot(tasks,edges),true,true);
  });
  await page.waitForFunction(()=>fixture.runtime.history.entries.length===1&&!fixture.runtime.frame&&!fixture.runtime.pending);
  const afterUpdate=await page.evaluate(()=>fixture.runtime.diagnostics.snapshot());
  // A synthetic reconnect with no catch-up: the runtime's 750 ms quiet baseline wait is most of it.
  const reconnect=Date.now();
  await page.evaluate(()=>{
   const r=fixture.runtime;r.receive(r.lastSource,true,false);r.receive(r.lastSource,true,true);
  });
  await page.waitForFunction(()=>!fixture.runtime.syncing&&!fixture.runtime.frame);
  const reconnectBaselineWaitMs=Date.now()-reconnect;
  const cdp=await context.newCDPSession(page);await cdp.send('Performance.enable');
  const metrics=async()=>Object.fromEntries((await cdp.send('Performance.getMetrics')).metrics.map(m=>[m.name,m.value]));
  const first=await metrics();await page.waitForTimeout(3000);const last=await metrics();
  const report={kind:'synthetic presentation only; excludes Ankurah sync',browser:browser.version(),viewport:[3840,2160],cards:13,edges:2,
   coldPresentationMs,reconnectBaselineWaitMs,idleMainThreadPercent:100*(last.TaskDuration-first.TaskDuration)/(last.Timestamp-first.Timestamp),
   before,afterUpdate,final:await page.evaluate(()=>fixture.runtime.diagnostics.snapshot()),errors,sockets};
  if(errors.length||sockets.length)throw Error('Unexpected browser error or data connection during synthetic profiling');
  if(process.env.PROFILE_OUTPUT)fs.writeFileSync(process.env.PROFILE_OUTPUT,JSON.stringify(report,null,2)+'\n');
  console.log(JSON.stringify(report,null,2));
 } finally {await context?.close();await browser?.close();await closeServer(server);}
})().catch(error=>{console.error(error);process.exitCode=1;});
