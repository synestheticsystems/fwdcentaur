// Wallboard-only presentation. Ankurah owns live state; this layer never writes to the graph.
// History contains only transitions witnessed by this display, timestamped on observation.
import { validateSettings } from './wallboard-settings.mjs';
import { Diagnostics, reasonOf } from './wallboard-diagnostics.mjs';
import { mountTheme } from './wallboard-theme.mjs';
import { LiveSound } from './wallboard-sound.mjs';
import { LocalHistory, readSnapshot, changes, historyContext } from './wallboard-history.mjs';

const NS = 'http://www.w3.org/2000/svg';
const ARRIVAL = 6000, DEPARTURE = 4000, FADE = 450, EDIT = 3000, WAVE = 800, SETTLE = 650;
const REPLAY_ARRIVAL = 3000, REPLAY_FADE = 3000, REPLAY_STEP = 4000, REPLAY_LIMIT = 23000;
const smooth = n => { n = Math.max(0, Math.min(1, n)); return n * n * (3 - 2 * n); };
const lerp = (a, b, t) => a + (b - a) * t;
const element = (tag, attributes = {}) => {
  const node = document.createElementNS(NS, tag);
  for (const [key, value] of Object.entries(attributes)) node.setAttribute(key, value);
  return node;
};

const pathOf = points => points.map(([x, y], i) => {
  if (!i) return `M${x.toFixed(2)},${y.toFixed(2)}`;
  const [px, py] = points[i - 1];
  return py === y ? `L${x.toFixed(2)},${y.toFixed(2)}` : `C${((px + x) / 2).toFixed(2)},${py.toFixed(2)} ${((px + x) / 2).toFixed(2)},${y.toFixed(2)} ${x.toFixed(2)},${y.toFixed(2)}`;
}).join(' ');

export class WallboardRuntime {
  constructor(host, settings, layout, environment = {}) {
    const validated = validateSettings(settings);
    settings = { ...validated, project_id: settings.project_id };
    this.host = host; this.settings = settings; this.layout = layout;
    this.now = environment.now || Date.now;
    this.diagnostics = environment.diagnostics || new Diagnostics(this.now, environment.clock);
    this.diagnostics.recordSettings(validated);
    let storage; try { storage = environment.storage ?? localStorage; } catch {}
    this.history = new LocalHistory(historyContext(settings), storage, this.now, this.diagnostics);
    this.root = host.closest('.wallboard') || host.parentElement;
    this.theme = mountTheme(this.root, settings, { now: this.now });
    this.waveElement = document.createElement('div'); this.waveElement.className = 'wallboard-wave'; this.waveElement.setAttribute('aria-hidden', 'true');
    this.banner = document.createElement('div'); this.banner.className = 'wallboard-replay'; this.banner.setAttribute('role', 'status');
    this.empty = document.createElement('p'); this.empty.className = 'wallboard-wait';
    this.root.prepend(this.waveElement); this.root.append(this.banner); host.append(this.empty);
    this.nodes = new Map(); this.edges = new Map(); this.effects = new Map(); this.departures = new Map();
    this.timers = new Map(); this.armed = false; this.connected = false; this.disposed = false; this.frame = 0;
    this.sound = new LiveSound(settings.sound, this.now, () =>
      this.connected && this.loaded && !this.syncing && !this.replay && !this.disposed && !document.hidden, this.diagnostics);
    this.motion = matchMedia('(prefers-reduced-motion: reduce)'); this.reduced = this.motion.matches;
    this.motionListener = event => {
      this.reduced = event.matches;
      if (this.reduced && this.wave) {
        this.clear('wave'); this.wave = null; this.host.dataset.wave = 'false'; this.waveElement.style.opacity = '0';
        this.applyPending();
      }
      if (this.reduced) this.settling = null;
      this.requestFrame();
    };
    this.motion.addEventListener('change', this.motionListener);
    this.visibilityListener = () => { if (document.hidden && this.replay) this.finishReplay(); this.scheduleReplay(); };
    document.addEventListener('visibilitychange', this.visibilityListener);
    this.host.dataset.mode = 'live'; this.diagnostics.state.mode = 'live'; this.host.dataset.armed = 'false';
    this.showLiveStatus();
    this.scheduleReplay();
    this.configureResize();
  }
  configureResize() {
    this.resizeObserver?.disconnect(); this.clear('resize');
    this.viewport = null;
    const host = this.host;
    if (this.settings.fit_to_screen) {
      this.resizeObserver = new ResizeObserver(() => {
        const viewport = [Math.round(host.clientWidth), Math.round(host.clientHeight)].join(':');
        if (viewport === this.viewport) return;
        this.viewport = viewport;
        this.later('resize', () => { if (this.live) this.render(this.scene(), true); }, 150);
      });
      this.resizeObserver.observe(host);
    }
  }
  // Reads a full settings revision without changing anything, and throws to refuse it: it must be
  // valid and keep the task query, whose ownership stays with the data layer.
  readRevision(text) {
    const value = JSON.parse(text);
    const validated = validateSettings(value);
    const next = { ...validated, project_id: value.project_id ?? this.settings.project_id };
    const statuses = settings => JSON.stringify([...settings.statuses].sort());
    if (next.project_id !== this.settings.project_id || statuses(next) !== statuses(this.settings))
      throw Error('Query changes require replacing the data subscription first');
    return { text, validated, next };
  }
  // Applies a revision readRevision accepted, whole, with the frame Rust rendered under it: geometry
  // needs that frame's newly rendered cards, never scaled old markup.
  applySettings({ text, validated, next }, snapshot) {
    this.settingsText = text;
    if (JSON.stringify(next) === JSON.stringify(this.settings)) return;
    // The query is unchanged here, so a different history context means different card geometry.
    const geometry = historyContext(next) !== historyContext(this.settings);
    // New geometry replaces the pending batch with its own frame. If that batch was a baseline wait's
    // catch-up, the frame now brings what the wait held back.
    const catchUp = geometry && this.pendingSilent;
    if (this.replay && (geometry || !next.replay.enabled)) this.finishReplay();
    if (geometry) {
      for (const timer of ['batch', 'batchMax', 'wave', 'persist']) this.clear(timer);
      this.wave = null; this.host.dataset.wave = 'false'; this.waveElement.style.opacity = '0';
      this.pending = null; this.pendingSilent = false; this.effects.clear(); this.departures.clear();
      // The new context replaces the old one's saved history, so the old one is not saved first.
      this.history = new LocalHistory(historyContext(next), this.history.storage, this.now, this.diagnostics);
      this.live = snapshot;
      this.host.dataset.historyCount = String(this.history.entries.length);
    }
    const resize = next.fit_to_screen !== this.settings.fit_to_screen;
    this.settings = next; this.layoutInput = null;
    this.theme.configure(next);
    this.sound.configure(next.sound);
    if (resize) this.configureResize();
    this.scheduleReplay();
    if (geometry || resize) this.render(this.scene(), true);
    if (catchUp) this.markCatchUpApplied();
    this.diagnostics.recordSettings(validated);
    this.diagnostics.state.settingsRevision = (this.diagnostics.state.settingsRevision || 0) + 1;
  }
  later(name, fn, delay) {
    clearTimeout(this.timers.get(name));
    this.timers.set(name, setTimeout(() => { this.timers.delete(name); if (!this.disposed) fn(); }, delay));
  }
  clear(name) { clearTimeout(this.timers.get(name)); this.timers.delete(name); }
  status(mode, tag, when, detail) {
    if (!this.statusFields) {
      this.statusFields = ['tag', 'time', 'detail'].map(name => {
        const field = document.createElement('span'); field.className = 'wallboard-mode-' + name; return field;
      });
      this.banner.replaceChildren(this.statusFields[0], ' ', this.statusFields[1], ' ', this.statusFields[2]);
    }
    this.banner.hidden = false; this.banner.className = 'wallboard-replay ' + mode;
    for (const [i, value] of [tag, when, detail].entries()) {
      if (this.statusFields[i].textContent !== value) this.statusFields[i].textContent = value;
    }
  }
  showLiveStatus() {
    if (this.replay) return;
    if (!this.connected) this.status('offline', this.armed ? 'OFFLINE' : 'CONNECTING',
      this.armed ? 'Last view' : 'Waiting', this.armed ? 'Reconnecting…' : 'Waiting for the live connection');
    else if (this.diagnostics.issues.graph?.active) this.status('offline', 'STALE', 'Last view', 'Graph update failed');
    else if (this.diagnostics.issues.configuration?.active) this.status('offline', 'STALE', 'Last view', 'Settings update refused');
    else if (!this.armed || !this.loaded || this.syncing) this.status('syncing', 'SYNCING',
      'Catching up', this.armed ? 'Keeping the last view' : 'Loading the graph');
    else this.status('current', 'LIVE', 'Now', 'Current graph');
  }
  replayAge(at) {
    const minutes = Math.max(0, Math.floor((this.now() - at) / 60000));
    if (!minutes) return 'Just now';
    const hours = Math.floor(minutes / 60), rest = minutes % 60;
    return (hours ? `${hours} h${rest ? ` ${rest} min` : ''}` : `${minutes} min`) + ' ago';
  }
  // Rust sends every frame with the complete settings revision it was rendered under. A refused
  // revision means the frame shows settings this runtime does not hold, possibly another query's
  // tasks, so the frame is never presented: the last view stays, marked STALE. A revision it accepts
  // applies only with a frame it can read, so an unreadable frame is the graph's failure alone.
  update(source, loaded, connected, settings) {
    if (this.disposed) return;
    let revision = null;
    if (settings !== this.settingsText) {
      try {
        revision = this.readRevision(settings);
      } catch (error) {
        this.diagnostics.issue('configuration', 'revision-rejected', reasonOf(error));
        this.observeTransport(loaded, connected); this.holdLastView(); return;
      }
    }
    this.diagnostics.clear('configuration');
    this.receive(source, loaded, connected, revision);
  }
  // Transport health and loading follow every update, whether or not its frame can be shown.
  observeTransport(loaded, connected) {
    this.awaitingBaseline ||= connected && !this.connected;
    if (connected !== this.connected) this.diagnostics.transition(connected ? 'connected' : 'disconnected');
    this.connected = connected; this.loaded = loaded;
    if (loaded) this.diagnostics.milestone('firstLoaded');
    Object.assign(this.diagnostics.state, { connected, loaded, lastReceivedAt: this.now() });
  }
  // Keeps the last good view while the newest frame cannot be presented. Pending changes and the
  // baseline wait come from older frames, so they stop. The next frame that can be shown waits for
  // a baseline as after a reconnect, so its changes appear as silent catch-up, never as live cues.
  holdLastView() {
    for (const timer of ['batch', 'batchMax', 'wave', 'baseline', 'baselineMax']) this.clear(timer);
    this.pending = null; this.pendingSilent = false; this.syncing = this.connected;
    this.wave = null; this.host.dataset.wave = 'false'; this.waveElement.style.opacity = '0';
    if (this.replay) this.finishReplay();
    this.showLiveStatus();
  }
  receive(source, loaded, connected, revision = null) {
    if (this.disposed) return;
    this.observeTransport(loaded, connected);
    let snapshot;
    try {
      snapshot = source === this.lastSource ? this.received
        : this.diagnostics.measure('snapshot', () => readSnapshot(source));
    } catch (error) {
      this.diagnostics.issue('graph', 'invalid-snapshot', reasonOf(error));
      this.holdLastView(); return;
    }
    // A readable frame disproves an unreadable one, not a failed render: only a render clears that.
    this.diagnostics.clear('graph', 'invalid-snapshot');
    this.received = snapshot; this.lastSource = source;
    if (revision) this.applySettings(revision, snapshot);
    this.diagnostics.state.lastValidAt = this.now();
    const reconnect = this.awaitingBaseline; this.awaitingBaseline = false;
    this.showLiveStatus();
    if (!connected) {
      for (const timer of ['batch', 'batchMax', 'wave', 'baseline', 'baselineMax']) this.clear(timer);
      this.syncing = false; this.wave = null; this.host.dataset.wave = 'false'; this.waveElement.style.opacity = '0';
      if (this.replay) this.finishReplay();
      // A frame already witnessed before transport loss is still real. Apply it quietly now;
      // never leave an old wave callback able to overwrite the reconnect's newer snapshot.
      if (this.pending) this.applyPending(true);
      if (!this.live) { this.live = snapshot; this.render(snapshot, false); }
      return;
    }
    if (!this.armed || reconnect || this.syncing) {
      if (!this.armed) { this.live = snapshot; this.render(snapshot, false); }
      if (!loaded) return;
      this.syncing = true;
      this.showLiveStatus();
      const settle = () => {
        this.clear('baseline'); this.clear('baselineMax'); this.syncing = false;
        if (!this.connected) return;
        this.diagnostics.transition('baselineEnded');
        if (!this.armed) {
          this.live = this.received; this.render(this.live, false); this.armed = true;
          this.host.dataset.armed = 'true'; this.diagnostics.milestone('armed');
        } else this.queue(this.received, true);
        this.showLiveStatus();
      };
      this.later('baseline', settle, 750);
      if (!this.timers.has('baselineMax')) this.later('baselineMax', settle, 2500);
      return;
    }
    this.queue(snapshot);
  }
  queue(snapshot, silent = false) {
    const diff = changes(this.live, snapshot);
    if (!diff.any && !this.pending && !this.wave) {
      this.live = snapshot;
      if (!this.replay) this.render(this.scene(), false);
      if (silent) this.markCatchUpApplied();
      return;
    }
    if (diff.any && this.replay) this.finishReplay();
    this.pending = snapshot; this.pendingAt = this.now();
    this.pendingSilent = silent || this.pendingSilent;
    if (this.wave) return; // Fold new frames into the wave already in flight; never restart it.
    if (!this.timers.has('batchMax')) this.later('batchMax', () => this.beginBatch(), 1200);
    this.later('batch', () => this.beginBatch(), 250);
  }
  beginBatch() {
    this.clear('batch'); this.clear('batchMax');
    this.reduced = this.motion.matches;
    if (!this.pending || !this.connected) return;
    if (this.reduced || document.hidden || !changes(this.live, this.pending).any) { this.applyPending(); return; }
    this.wave = { at: this.now() }; this.host.dataset.wave = 'true'; this.requestFrame();
    this.later('wave', () => { this.wave = null; this.host.dataset.wave = 'false'; this.waveElement.style.opacity = '0'; this.applyPending(); }, WAVE);
  }
  applyPending(quiet = false) {
    if (!this.pending) return;
    const before = this.live, after = this.pending, at = this.pendingAt, silent = this.pendingSilent;
    this.pending = null; this.pendingSilent = false;
    const diff = changes(before, after);
    this.live = after;
    if (diff.any) {
      this.history.append(before, after, at); this.host.dataset.historyCount = String(this.history.entries.length);
      this.later('persist', () => this.history.save(), 1000);
      this.transition(before, after, diff, quiet || document.hidden);
      if (!quiet && !silent) void this.sound.play(diff);
    } else this.render(this.scene(), false);
    if (silent) this.markCatchUpApplied();
  }
  // Catch-up is what a baseline wait held back. The view shows it only once its render has succeeded.
  markCatchUpApplied() {
    if (!this.diagnostics.issues.graph?.active) this.diagnostics.transition('catchUpApplied');
  }
  transition(before, after, diff, quiet = false) {
    const at = this.now();
    for (const id of diff.arrived) { this.departures.delete(id); this.effects.set(id, { arrival: at }); }
    for (const id of diff.edited) {
      const effect = this.effects.get(id) || {};
      this.effects.set(id, { ...effect, edit: effect.edit !== undefined && at - effect.edit < EDIT ? effect.edit : at });
    }
    for (const id of diff.departed) {
      if (!this.departures.has(id)) this.departures.set(id, { node: before.nodes.get(id),
        edges: [...before.edges.values()].filter(edge => edge.from === id || edge.to === id), at,
        hold: this.replay ? 0 : DEPARTURE, fade: this.replay ? REPLAY_FADE : FADE });
      this.effects.set(id, { departure: this.departures.get(id) });
    }
    this.present = after;
    if (quiet) { this.effects.clear(); this.departures.clear(); }
    this.render(this.scene(), !quiet);
  }
  scene() {
    const target = this.replay ? this.present : this.live;
    if (!target || !this.departures.size) return target;
    const nodes = new Map(target.nodes), edges = new Map(target.edges);
    for (const [id, departing] of this.departures) {
      if (!nodes.has(id)) nodes.set(id, departing.node);
      for (const edge of departing.edges) if (nodes.has(edge.from) && nodes.has(edge.to)) edges.set(edge.key, edge);
    }
    // A relation joining two simultaneous departures may only become valid after both are added.
    for (const departing of this.departures.values()) for (const edge of departing.edges)
      if (nodes.has(edge.from) && nodes.has(edge.to)) edges.set(edge.key, edge);
    return { ...target, nodes, edges };
  }
  arrange(target) {
    const { nodes, edges } = target;
    const input = JSON.stringify({
      nodes: [...nodes.values()].map(({ id, title, priority, status, height }) => ({ id, title, priority, status, height })),
      edges: [...edges.values()].map(({ from, to }) => ({ from, to })),
      max_rows: this.settings.max_rows,
      node_width: this.settings.node_width * this.settings.text_scale_percent / 100,
      ...(this.settings.fit_to_screen ? { viewport_width: this.host.clientWidth, viewport_height: this.host.clientHeight } : {}),
    });
    if (input !== this.layoutInput) {
      this.layoutResult = this.diagnostics.measure('layout', () => JSON.parse(this.layout(input))); this.layoutInput = input;
    }
    const placed = this.layoutResult;
    if (!placed || placed.routes.length !== edges.size) throw Error('Invalid wallboard layout');
    this.host.dataset.layoutRows = String(placed.rows || this.settings.max_rows);
    const positions = new Map(placed.boxes.map(box => [box.id, box]));
    return { ...target, width: placed.width, height: placed.height,
      nodes: new Map([...nodes].map(([id, node]) => [id, { ...node, ...positions.get(id) }])),
      edges: new Map([...edges].map(([key, edge], i) => [key, { ...edge, points: placed.routes[i] }])) };
  }
  render(scene, animate) {
    try {
      return this.diagnostics.measure('render', () => this.renderScene(scene, animate));
    } catch (error) {
      this.diagnostics.issue('graph', 'render-failed', reasonOf(error)); this.showLiveStatus();
    }
  }
  renderScene(scene, animate) {
    if (!scene) return;
    if (this.settings.fit_to_screen || this.departures.size) scene = this.arrange(scene);
    const at = this.now();
    if (!this.svg) {
      this.svg = scene.template.cloneNode(false); this.svg.classList.add('wallboard-dag');
      const defs = scene.template.querySelector('defs'); if (defs) this.svg.append(defs.cloneNode(true));
      this.edgeLayer = element('g'); this.nodeLayer = element('g'); this.svg.append(this.edgeLayer, this.nodeLayer);
      this.host.prepend(this.svg);
    }
    this.empty.textContent = scene.nodes.size ? '' : this.loaded ? 'No matching tasks' : 'Loading graph…';
    this.empty.hidden = Boolean(scene.nodes.size);
    for (const [id, record] of this.nodes) if (!scene.nodes.has(id)) { record.dom.remove(); this.nodes.delete(id); this.effects.delete(id); }
    for (const [id, node] of scene.nodes) {
      let record = this.nodes.get(id);
      if (!record) {
        record = { dom: node.template.cloneNode(true), x: node.x, y: node.y, content: null };
        this.nodes.set(id, record); this.nodeLayer.append(record.dom);
      }
      const content = node.template.innerHTML;
      if (record.content !== content) {
        record.dom.replaceChildren(...[...node.template.childNodes].map(child => child.cloneNode(true)));
        record.dom.setAttribute('class', node.template.getAttribute('class'));
        for (const [name, value] of Object.entries(node.template.dataset)) record.dom.dataset[name] = value;
        record.content = content;
        record.wash = element('rect', { class: 'wallboard-wash', width: node.width, height: node.height, rx: 9 });
        record.dom.querySelector('.wallboard-card').after(record.wash);
        record.track = element('rect', { class: 'wallboard-countdown', x: 6, y: node.height - 4, width: node.width - 12, height: 3, rx: 1.5 });
        record.dom.append(record.track);
        record.glow = element('rect', { x: -2, y: -2, width: node.width + 4, height: node.height + 4, rx: 11, class: 'wallboard-edit-glow' });
        record.dom.querySelector('.wallboard-card').before(record.glow);
      }
      record.from = [record.x, record.y]; record.to = [node.x, node.y]; record.node = node;
    }
    for (const [key, record] of this.edges) if (!scene.edges.has(key)) { record.dom.remove(); this.edges.delete(key); }
    for (const [key, edge] of scene.edges) {
      let record = this.edges.get(key);
      if (!record) { record = { dom: edge.template.cloneNode(true), points: edge.points }; this.edges.set(key, record); this.edgeLayer.append(record.dom); }
      record.from = record.points; record.to = edge.points; record.edge = edge;
    }
    this.sizeFrom = this.size || [scene.width, scene.height]; this.sizeTo = [scene.width, scene.height];
    this.settling = animate && !this.reduced ? at : null;
    this.diagnostics.state.lastAppliedAt = this.now();
    this.diagnostics.state.nodes = scene.nodes.size; this.diagnostics.state.edges = scene.edges.size;
    if (this.diagnostics.issues.graph?.code === 'render-failed') {
      this.diagnostics.clear('graph'); this.showLiveStatus();
    }
    this.requestFrame();
  }
  requestFrame() { if (!this.frame && !this.disposed) this.frame = requestAnimationFrame(() => this.tick()); }
  tick() {
    this.frame = 0;
    const now = this.now();
    let active = false, expired = false;
    for (const [id, departing] of this.departures) if (now >= departing.at + departing.hold + departing.fade) {
      this.departures.delete(id); this.effects.delete(id); expired = true;
    }
    if (expired) { this.render(this.scene(), true); return; }
    const settle = this.settling === null ? 1 : smooth((now - this.settling) / SETTLE);
    if (settle < 1) active = true;
    if (this.sizeTo) {
      this.size = this.sizeTo.map((value, i) => lerp(this.sizeFrom[i], value, settle));
      this.svg.setAttribute('viewBox', `0 0 ${this.size[0]} ${this.size[1]}`);
      this.svg.setAttribute('width', this.size[0]); this.svg.setAttribute('height', this.size[1]);
    }
    const matrix = this.svg?.getScreenCTM(), scale = matrix?.a || 1;
    let waveCenter, waveHalf;
    if (this.wave && !this.reduced) {
      const progress = (now - this.wave.at) / WAVE, width = this.root.clientWidth;
      waveHalf = width * .24; waveCenter = -waveHalf + progress * width * 1.48;
      this.waveElement.style.opacity = 'var(--wb-wave-opacity)';
      this.waveElement.style.background = `linear-gradient(90deg, transparent ${waveCenter - waveHalf}px, var(--wb-wave) ${waveCenter}px, transparent ${waveCenter + waveHalf}px)`;
      active = true;
    }
    const pulse = (age, duration, cycles, immediate = false) => this.reduced ? .12 :
      .24 * Math.sin(Math.PI * (cycles * age / duration + (immediate ? .5 : 0))) ** 2 *
      (immediate ? 1 - smooth((age - duration + 600) / 600) : 1);
    for (const [id, record] of this.nodes) {
      record.x = lerp(record.from[0], record.to[0], settle); record.y = lerp(record.from[1], record.to[1], settle);
      const center = matrix ? matrix.a * (record.x + record.node.width / 2) + matrix.e : 0;
      const pass = waveCenter === undefined ? 0 : (waveCenter - center + waveHalf) / (2 * waveHalf);
      record.bob = pass > 0 && pass < 1 ? -2 / scale * Math.sin(Math.PI * pass) ** 2 : 0;
      record.dom.setAttribute('transform', `translate(${record.x},${record.y + record.bob})`);
      const effect = this.effects.get(id) || {};
      let wash = 0, opacity = 1, glow = 0, countdown = 0, type = '';
      const arrivalDuration = this.replay ? REPLAY_ARRIVAL : ARRIVAL;
      if (effect.arrival !== undefined && now - effect.arrival < arrivalDuration) {
        wash = pulse(now - effect.arrival, arrivalDuration, this.replay ? 2 : 3, Boolean(this.replay)); type = 'arrival'; active = true;
      }
      if (effect.edit !== undefined && now - effect.edit < EDIT) {
        const age = now - effect.edit;
        const envelope = (this.replay || this.reduced ? 1 : smooth(age / 120)) * (1 - smooth((age - 2550) / 450));
        const throb = this.reduced ? .6 : .5 + .5 * Math.cos(age / EDIT * Math.PI * 6);
        glow = envelope * (.4 + .6 * throb);
        if (!type) { wash = envelope * (.04 + .12 * throb); type = 'edit'; }
        active = true;
      }
      if (effect.departure !== undefined) {
        const { at, hold, fade } = effect.departure, age = now - at, countdownDuration = hold || fade;
        wash = this.reduced ? .12 : .06 + pulse(Math.min(age, countdownDuration), countdownDuration, 2) * .75;
        glow = 0; type = 'departure'; countdown = Math.max(0, 1 - age / countdownDuration);
        // Replay marks the departure immediately, then fades throughout the step. Live keeps its warning hold.
        opacity = hold ? 1 - smooth((age - hold) / fade) : .78 * Math.max(0, 1 - age / fade); active = true;
      }
      record.dom.dataset.motion = type;
      record.dom.style.opacity = String(opacity);
      record.wash.setAttribute('class', `wallboard-wash ${type}`); record.wash.style.opacity = String(wash);
      record.track.style.opacity = countdown > 0 ? '1' : '0'; record.track.setAttribute('width', (record.node.width - 12) * countdown);
      record.glow.style.opacity = String(glow);
      if (!type && !glow && effect.departure === undefined) this.effects.delete(id);
    }
    for (const record of this.edges.values()) {
      const a = this.nodes.get(record.edge.from), b = this.nodes.get(record.edge.to);
      const count = record.to.length;
      record.points = record.to.map((point, i) => {
        const old = record.from[Math.round(i / (count - 1) * (record.from.length - 1))];
        return point.map((value, axis) => lerp(old[axis], value, settle));
      });
      const bobbed = record.points.map(([x, y], i) => [x, y + lerp(a?.bob || 0, b?.bob || 0, i / (count - 1))]);
      record.dom.setAttribute('d', pathOf(bobbed));
      record.dom.style.opacity = String(Math.min(Number(a?.dom.style.opacity || 1), Number(b?.dom.style.opacity || 1)) * .8);
    }
    this.host.dataset.animationCount = String(this.effects.size);
    if (active) this.requestFrame();
  }
  scheduleReplay() {
    this.clear('replay');
    if (this.settings.replay.enabled) this.later('replay', () => {
      if (!document.hidden) this.startReplay();
      if (!this.replay) this.scheduleReplay();
    }, this.settings.replay.interval_seconds * 1000);
  }
  startReplay() {
    if (!this.armed || this.pending || this.wave || this.replay || this.effects.size || document.hidden) return false;
    let unreadable = false;
    const groups = this.history.groups().flatMap(group => {
      try { const before = readSnapshot(group.before), after = readSnapshot(group.after); const diff = changes(before, after);
        return diff.any ? [{ ...group, before, after, diff }] : []; }
      catch (error) { unreadable = true; this.diagnostics.issue('history', 'invalid-snapshot', reasonOf(error)); return []; }
    }).slice(-Math.floor(REPLAY_LIMIT / REPLAY_STEP));
    // The issue stands for as long as the saved history still holds an unreadable snapshot.
    if (!unreadable) this.diagnostics.clear('history', 'invalid-snapshot');
    if (!groups.length) return false;
    this.clear('replay'); this.replay = { groups, index: 0, start: this.now() }; this.host.dataset.mode = 'replay'; this.diagnostics.state.mode = 'replay';
    this.playGroup(); return true;
  }
  playGroup() {
    if (!this.replay) return;
    const group = this.replay.groups[this.replay.index];
    if (!group || this.now() - this.replay.start >= REPLAY_LIMIT) { this.finishReplay(); return; }
    this.effects.clear(); this.departures.clear(); this.present = group.before; this.render(group.before, false);
    const detail = [[group.diff.arrived.length, 'added'], [group.diff.edited.length, 'updated'], [group.diff.departed.length, 'removed']]
        .filter(([count]) => count).map(([count, label]) => `${count} ${label}`).join(' · ') || 'Observed change';
    this.status('replaying', 'REPLAYING', this.replayAge(group.at), detail);
    // The timestamp, changed content and cues all begin together.
    this.transition(group.before, group.after, group.diff);
    this.later('replayStep', () => { if (this.replay) { this.replay.index++; this.playGroup(); } }, REPLAY_STEP);
    this.later('replayEnd', () => this.finishReplay(), Math.max(0, REPLAY_LIMIT - (this.now() - this.replay.start)));
  }
  finishReplay() {
    this.clear('replayStep'); this.clear('replayEnd'); this.replay = null; this.present = null;
    this.effects.clear(); this.departures.clear(); this.host.dataset.mode = 'live'; this.diagnostics.state.mode = 'live';
    if (this.live) this.render(this.live, false);
    this.showLiveStatus(); this.scheduleReplay();
  }
  dispose() {
    this.disposed = true; cancelAnimationFrame(this.frame);
    this.sound.dispose(); this.theme.dispose(); this.resizeObserver?.disconnect();
    for (const timer of this.timers.values()) clearTimeout(timer);
    this.timers.clear(); this.effects.clear(); this.departures.clear();
    this.nodes.clear(); this.edges.clear(); this.frame = 0;
    this.diagnostics.state.disposed = true;
    this.history.save(); this.motion.removeEventListener('change', this.motionListener);
    document.removeEventListener('visibilitychange', this.visibilityListener);
    this.waveElement.remove(); this.banner.remove(); this.host.replaceChildren();
  }
}

const runtimes = new WeakMap(), pendingDiagnostics = new WeakMap();
export function recordRustRender(host, elapsedMs) {
  let diagnostics = runtimes.get(host)?.diagnostics || pendingDiagnostics.get(host);
  if (!diagnostics) { diagnostics = new Diagnostics(); pendingDiagnostics.set(host, diagnostics); }
  diagnostics.sample('rustRender', elapsedMs);
}
export function mount(host, settings, layout) {
  runtimes.get(host)?.dispose();
  runtimes.set(host, new WallboardRuntime(host, JSON.parse(settings), layout, { diagnostics: pendingDiagnostics.get(host) }));
  pendingDiagnostics.delete(host);
}
export function update(host, source, loaded, connected, settings) {
  runtimes.get(host)?.update(source, loaded, connected, settings);
}
export function dispose(host) { runtimes.get(host)?.dispose(); runtimes.delete(host); pendingDiagnostics.delete(host); }
export function runtimeFor(host) { return runtimes.get(host); }

export function diagnosticsFor(host) { return runtimes.get(host)?.diagnostics.snapshot(); }
