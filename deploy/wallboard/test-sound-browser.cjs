// Real Web Audio rendering and runtime delivery checks, with synthetic graph data.
const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path');
const { startFixtureServer, launchChromium, pauseClock, closeServer } = require('./fixture-server.cjs');
(async()=>{
  const server=await startFixtureServer();
  const browser=await launchChromium({args:['--autoplay-policy=no-user-gesture-required','--mute-audio']}).catch(async error=>{await closeServer(server);throw error;});
  try {
    const page=await browser.newPage(), errors=[];page.on('pageerror',error=>errors.push(String(error)));
    await page.goto(server.url);await page.waitForFunction(()=>window.fixture);
    const clips=await page.evaluate(async()=>{
      const {loadSounds,playCue,LiveSound}=await import('/crates/fwd-web/src/wallboard-sound.mjs');
      const clips=[];
      for(const kind of ['arrival','departure','edit']){
        const context=new OfflineAudioContext(1,57600,48000);
        const buffers=await loadSounds(context);playCue(context,buffers[kind],.30,0);
        const samples=Array.from((await context.startRendering()).getChannelData(0));
        clips.push({kind,samples,length:buffers[kind].length});
      }
      let now=0,allowed=true;const settings={enabled:true,volume:30};
      const sound=new LiveSound(settings,()=>now,()=>allowed);
      const diff=(arrived=[],departed=[],edited=[])=>({arrived,departed,edited,any:!!(arrived.length+departed.length+edited.length)});
      const early=await sound.play(diff(['early']));
      if(early || !await sound.prepare()) throw Error('Preloading must be silent and complete');
      if(sound.context.state!=='suspended') throw Error('Preloading must leave audio suspended');
      const results=[];
      results.push(await sound.play(diff(['a'])),sound.lastKind);
      results.push(await sound.play(diff([],[],['a'])));
      now+=4000;results.push(await sound.play(diff([],['a'])),sound.lastKind);
      await new Promise(resolve=>setTimeout(resolve,1080));
      if(sound.context.state!=='running') throw Error('Full departure tail must finish before suspension');
      await new Promise(resolve=>setTimeout(resolve,250));
      if(sound.context.state!=='suspended') throw Error('Audio must suspend after the cue');
      now+=4000;results.push(await sound.play(diff(['b'],['a'])),sound.lastKind);
      now+=4000;settings.volume=0;results.push(await sound.play(diff(['c'])));
      settings.volume=30;settings.enabled=false;results.push(await sound.play(diff(['c'])));
      settings.enabled=true;allowed=false;results.push(await sound.play(diff(['c'])));
      allowed=true;sound.dispose();results.push(await sound.play(diff(['c'])));
      return {clips,results};
    });
    assert.deepEqual(clips.results,[true,'arrival',false,true,'departure',true,'edit',false,false,false,false]);
    for(const {kind,samples,length} of clips.clips){
      const peak=Math.max(...samples.map(Math.abs));assert.ok(peak>.06&&peak<.12,kind+' has a quiet, unclipped signal');
      assert.ok(samples.every(Number.isFinite));assert.equal(samples[0],0);
      assert.ok(samples.slice(length).every(sample=>sample===0),'cue must finish rather than leave a continuous signal');
      const approved=fs.readFileSync(path.join(__dirname,'sounds',kind+'.wav'));
      assert.equal(length,(approved.length-44)/2);
      const pcmError=Math.max(...samples.slice(0,length).map((value,i)=>Math.abs(value-approved.readInt16LE(44+i*2)/32768)));
      console.log(kind+' maximum PCM rounding difference: '+pcmError);
      assert.ok(pcmError<1/32768,'playback preserves the approved clip within one 16-bit PCM step');
      const jumps=samples.slice(1).map((sample,i)=>Math.abs(sample-samples[i]));assert.ok(Math.max(...jumps)<.02,'soft envelope avoids clicks');
      if(process.env.WALLBOARD_SOUND_PREVIEW_DIR){
        const output=path.join(process.env.WALLBOARD_SOUND_PREVIEW_DIR,kind+'.wav');fs.mkdirSync(path.dirname(output),{recursive:true});
        const wav=Buffer.alloc(44+samples.length*2);wav.write('RIFF');wav.writeUInt32LE(wav.length-8,4);wav.write('WAVEfmt ',8);
        wav.writeUInt32LE(16,16);wav.writeUInt16LE(1,20);wav.writeUInt16LE(1,22);wav.writeUInt32LE(48000,24);
        wav.writeUInt32LE(96000,28);wav.writeUInt16LE(2,32);wav.writeUInt16LE(16,34);wav.write('data',36);wav.writeUInt32LE(samples.length*2,40);
        samples.forEach((sample,i)=>wav.writeInt16LE(Math.round(sample*32767),44+i*2));fs.writeFileSync(output,wav);
      }
    }

    await page.route('**/sounds/*.wav',route=>route.abort());
    const failure=await page.evaluate(async()=>{
      const {LiveSound}=await import('/crates/fwd-web/src/wallboard-sound.mjs');
      const {Diagnostics}=await import('/crates/fwd-web/src/wallboard-diagnostics.mjs');
      const diagnostics=new Diagnostics();
      const sound=new LiveSound({enabled:true,volume:30},Date.now,()=>true,diagnostics);
      const played=await sound.play({any:true,arrived:['a'],departed:[],edited:[]});
      const ready=await sound.prepare();sound.dispose();
      const {code,reason,active}=diagnostics.issues.audio;return {played,ready,code,reason,active};
    });
    assert.deepEqual(failure,{played:false,ready:false,code:'unavailable',reason:'Failed to fetch',active:true});
    await page.unroute('**/sounds/*.wav');

    // A cue arms one idle timer to suspend audio processing. A later cue replaces it, a revision that
    // disables sound suspends at once and cancels it, and disposal cancels it, so none acts late.
    const idle=await page.evaluate(async()=>{
      const {LiveSound}=await import('/crates/fwd-web/src/wallboard-sound.mjs');
      const cue={any:true,arrived:['a'],departed:[],edited:[]},wait=ms=>new Promise(resolve=>setTimeout(resolve,ms));
      const afterCue=async()=>{
        let now=0;const sound=new LiveSound({enabled:true,volume:30},()=>now,()=>true);
        if(!await sound.prepare()||!await sound.play(cue)) throw Error('The cue must play');
        const suspend=sound.context.suspend.bind(sound.context);
        sound.suspended=0;sound.context.suspend=()=>{sound.suspended++;return suspend();};
        sound.playAgain=()=>{now+=4000;return sound.play(cue);};
        return sound;
      };
      const replaced=await afterCue(),disabled=await afterCue(),disposed=await afterCue();
      disabled.configure({enabled:false,volume:30});disposed.dispose();
      const atRevision=disabled.suspended;
      await wait(1000);
      if(!await replaced.playAgain()) throw Error('The later cue must play');
      await wait(400);
      const pastTheFirstTimer={afterRevision:disabled.suspended,afterDisposal:disposed.suspended,afterLaterCue:replaced.suspended};
      await wait(1000);
      const afterTheLaterCuesTimer=replaced.suspended;
      disabled.dispose();replaced.dispose();
      return {atRevision,...pastTheFirstTimer,afterTheLaterCuesTimer};
    });
    assert.deepEqual(idle,{atRevision:1,afterRevision:1,afterDisposal:0,afterLaterCue:0,afterTheLaterCuesTimer:1});
    console.log('PASS approved wood-and-air clips play intact; silent preload, complete tails, idle suspension, cooldown, mixed-batch cue, mute, disabled and disposal gates; unavailable audio is diagnosed; a later cue, a disabling revision and disposal each cancel the idle timer');

    await page.clock.install({time:new Date('2026-11-01T12:00:00Z')});
    await page.reload();await page.waitForFunction(()=>window.fixture);
    await pauseClock(page);
    await page.evaluate(()=>{fixture.runtime.sound.play=async diff=>{window.soundCalls=(window.soundCalls||[]).concat([diff]);return true;};});
    const feed=(tasks,connected=true)=>page.evaluate(({tasks,connected})=>fixture.runtime.receive(fixture.snapshot(tasks),true,connected),{tasks,connected});
    const count=()=>page.evaluate(()=>window.soundCalls?.length||0);
    const a={id:'a',title:'Initial task',priority:1},b={id:'b',title:'Added task',priority:2},c={id:'c',title:'Another addition',priority:2};
    await feed([a]);await page.clock.runFor(800);assert.equal(await count(),0);
    await feed([a,b]);await page.clock.runFor(100);await feed([a,b,c]);await page.clock.runFor(1300);assert.equal(await count(),1);
    assert.equal(await page.evaluate(()=>soundCalls[0].arrived.length),2);
    await page.clock.runFor(7000);assert.equal(await page.evaluate(()=>fixture.runtime.startReplay()),true);
    await page.clock.runFor(23000);assert.equal(await count(),1,'replay is silent');
    await feed([a,b,c],false);await feed([a,c]);await page.clock.runFor(8500);assert.equal(await count(),1,'reconnect catch-up is silent');
    await feed([a,{...c,what:'New live edit'}]);await page.clock.runFor(1400);assert.equal(await count(),2);
    assert.deepEqual(errors,[]);
    console.log('PASS cold load and replay silent; one cue per live batch; reconnect catch-up silent; later live edits audible; no browser errors');
  } finally {await browser.close();await closeServer(server);}
})().catch(error=>{console.error(error);process.exitCode=1;});
