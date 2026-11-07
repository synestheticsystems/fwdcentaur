// Original synthesized clips, bundled with the wallboard and loaded from its own server.
import { reasonOf } from './wallboard-diagnostics.mjs';

export async function loadSounds(context) {
  return Object.fromEntries(await Promise.all(['arrival', 'departure', 'edit'].map(async kind => {
    const response = await fetch(`/sounds/${kind}.wav`, { signal: AbortSignal.timeout(5000) });
    if (!response.ok) throw Error('Wallboard sound unavailable');
    return [kind, await context.decodeAudioData(await response.arrayBuffer())];
  })));
}

export function playCue(context, buffer, volume, at = context.currentTime + .02) {
  const source = context.createBufferSource(), gain = context.createGain();
  source.buffer = buffer;
  gain.gain.value = volume / .30; // The clips were authored at the default volume, 30.
  source.connect(gain); gain.connect(context.destination);
  source.onended = () => { source.disconnect(); gain.disconnect(); };
  source.start(at);
}

export class LiveSound {
  // Settings arrive validated by the shared contract, so a volume is always from 0 to 100. The
  // cooldown runs on `clock`, a monotonic one, so that a wall clock stepped back cannot silence cues.
  constructor(settings, clock = () => performance.now(), allowed = () => true, diagnostics) {
    this.settings = settings; this.clock = clock; this.allowed = allowed; this.diagnostics = diagnostics;
    this.lastAt = -Infinity; this.pending = false; this.disposed = false;
    void this.prepare();
  }
  configure(settings) {
    this.settings = settings;
    if (settings.enabled) void this.prepare();
    else if (this.context) { clearTimeout(this.idle); void this.context.suspend().catch(() => {}); }
  }
  async prepare() {
    if (this.disposed || !this.settings.enabled) return false;
    if (this.buffers) return true;
    if (this.preparing) return this.preparing.catch(() => false);
    try {
      this.context ||= new AudioContext();
      this.preparing = (async () => {
        await this.context.suspend();
        const buffers = await loadSounds(this.context);
        if (this.disposed) return false;
        this.buffers = buffers; this.diagnostics?.clear('audio'); return true;
      })();
      return await this.preparing;
    } catch (error) { this.diagnostics?.issue('audio', 'unavailable', reasonOf(error)); return false; }
    finally { this.preparing = null; }
  }
  async play(diff) {
    const at = this.clock(), volume = this.settings.volume / 100;
    if (!this.settings.enabled || !volume || !diff.any || !this.allowed() || this.disposed || this.pending || at - this.lastAt < 4000) return false;
    if (!this.buffers) { void this.prepare(); return false; }
    this.lastAt = at; this.pending = true;
    const kind = diff.arrived.length && diff.departed.length ? 'edit'
      : diff.arrived.length ? 'arrival' : diff.departed.length ? 'departure' : 'edit';
    let timeout;
    try {
      this.context ||= new AudioContext();
      clearTimeout(this.idle);
      if (this.context.state !== 'running') await Promise.race([
        this.context.resume(), new Promise(resolve => { timeout = setTimeout(resolve, 300); }),
      ]);
      // Never queue a delayed notification behind an autoplay prompt, reconnect or replay.
      if (this.context.state !== 'running' || this.disposed || !this.settings.enabled || !this.settings.volume || !this.allowed() || this.clock() - at > 500) return false;
      playCue(this.context, this.buffers[kind], volume);
      return true;
    } catch (error) { this.diagnostics?.issue('audio', 'unavailable', reasonOf(error)); return false; } // Audio failure must never interrupt the graph.
    finally {
      clearTimeout(timeout); this.pending = false;
      if (!this.disposed && this.context) this.idle = setTimeout(() => this.context.suspend().catch(() => {}), 1250);
    }
  }
  dispose() {
    this.disposed = true; clearTimeout(this.idle);
    if (this.context) void this.context.close().catch(() => {});
  }
}
