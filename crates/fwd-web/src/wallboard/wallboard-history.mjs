// Disposable witnessed history and sanitized SVG adaptation; no layout or timers.
import { buildInfo } from './wallboard-build.mjs';
import { reasonOf } from './wallboard-diagnostics.mjs';
const NS = 'http://www.w3.org/2000/svg';
const HISTORY_PREFIX = 'fwd-wallboard-history-v1:';
// Changes seen within this many milliseconds of the first of them make one replay step (`groups`).
const GROUP_WINDOW = 30000;
const bytes = value => Math.max(value.length * 2, new TextEncoder().encode(value).length);

export class LocalHistory {
  constructor(context, storage, now = Date.now, diagnostics, build = buildInfo.content_sha256) {
    this.key = HISTORY_PREFIX + context; this.diagnostics = diagnostics; this.build = build;
    this.storage = storage; this.now = now; this.entries = []; this.persisted = true;
    try {
      // One display/configuration per origin: changing project never accumulates old caches.
      for (let i = storage?.length - 1; i >= 0; i--) {
        const key = storage.key(i);
        if (key.startsWith(HISTORY_PREFIX) && key !== this.key) storage.removeItem(key);
      }
      const saved = storage?.getItem(this.key);
      if (saved) {
        if (bytes(saved) > 2 * 1024 * 1024) throw Error('Oversized history cache');
        let value;
        // A parse error's message quotes the saved snapshots, which hold task content.
        try { value = JSON.parse(saved); } catch { throw Error('Unparsable history cache'); }
        if (value.version !== 1 || !Array.isArray(value.entries)) throw Error('Invalid history cache');
        this.entries = value.entries.filter(entry => entry &&
          Number.isFinite(entry.at) && typeof entry.before === 'string' && typeof entry.after === 'string');
        const dropped = value.entries.length - this.entries.length;
        if (dropped) this.diagnostics?.issue('history', 'invalid-entry', `${dropped} of ${value.entries.length} entries unusable`);
      }
    } catch (error) { this.persisted = false; this.diagnostics?.issue('history', 'unreadable-cache', reasonOf(error)); }
    this.prune();
  }
  // Keeps the newest 120 entries within 2 MiB, in the order this display witnessed them, which a wall
  // clock stepped back would scramble if they were sorted by their stamps. Such a step leaves the
  // entries seen before it stamped ahead of now. Rather than being dropped, or flattened to one stamp,
  // which would merge the steps replay makes of them, they move back together by one offset, keeping
  // their order and spacing: the newest of them ends a grouping window before the first entry seen
  // after the step, or before now, since the time between the two is unknown and replay must not merge
  // them.
  prune() {
    const now = this.now();
    const last = this.entries.findLastIndex(entry => entry.at > now);
    if (last >= 0) {
      const end = (this.entries[last + 1]?.at ?? now) - GROUP_WINDOW - 1;
      const shift = Math.max(...this.entries.slice(0, last + 1).map(entry => entry.at)) - end;
      this.entries = this.entries.map((entry, i) => i <= last ? { ...entry, at: entry.at - shift } : entry);
    }
    this.entries = this.entries.slice(-120);
    while (this.entries.length && bytes(this.encode()) > 2 * 1024 * 1024) this.entries.shift();
  }
  encode() { return JSON.stringify({ version: 1, entries: this.entries }); }
  append(before, after, at) { this.entries.push({ at, build: this.build, before: before.source, after: after.source }); this.prune(); }
  save() {
    this.prune();
    try {
      const persist = () => this.storage?.setItem(this.key, this.encode());
      if (this.diagnostics) this.diagnostics.measure('persistence', persist); else persist();
      this.persisted = Boolean(this.storage);
      // A write replaces an unreadable or partly invalid cache and ends a write failure. A snapshot
      // that replay found unreadable is written again, so its issue stands.
      if (this.persisted) this.diagnostics?.clear('history', 'unreadable-cache', 'invalid-entry', 'write-failed');
      else this.diagnostics?.issue('history', 'storage-unavailable');
    }
    catch (error) { this.persisted = false; this.diagnostics?.issue('history', 'write-failed', reasonOf(error)); } // A full/disabled browser store must not interrupt the display.
  }
  // Merges what happened together into one replay step, which compares its first snapshot with its
  // last. What a card's revision hashes can change between builds, and a step spanning two would report
  // every card that stayed as edited, so each group holds entries of one build. Entries that record no
  // build group only among themselves. A reload of one build ends no group, since its revisions still
  // compare.
  groups() {
    this.prune();
    const groups = [];
    for (const entry of this.entries) {
      const last = groups.at(-1);
      if (last && entry.build === last.build && entry.at - last.first <= GROUP_WINDOW) { last.after = entry.after; last.at = entry.at; last.count++; }
      else groups.push({ ...entry, first: entry.at, count: 1 });
    }
    return groups;
  }
}

// Replay compares snapshots rendered for one query with one card geometry. These settings name that
// context, and a change to any of them starts a fresh history. Text scale joins the key only when it
// is not 100, so that histories saved before text scaling existed stay readable.
export function historyContext(settings) {
  const fields = [settings.project_id, [...settings.statuses].sort(), settings.max_rows, settings.node_width];
  if (settings.text_scale_percent !== 100) fields.push(settings.text_scale_percent);
  return JSON.stringify(fields);
}

// Saved history is data, not executable markup. Only the SVG vocabulary emitted by Rust is
// accepted; no script, foreignObject, href, event handler, style, or external resource survives.
export function readSnapshot(source) {
  if (typeof source !== 'string' || bytes(source) > 2 * 1024 * 1024) throw Error('Invalid graph snapshot');
  const svg = new DOMParser().parseFromString(source, 'image/svg+xml').documentElement;
  if (svg.localName !== 'svg' || svg.namespaceURI !== NS || svg.querySelector('parsererror')) throw Error('Invalid SVG snapshot');
  const tags = new Set(['svg', 'defs', 'marker', 'path', 'g', 'rect', 'text', 'title', 'circle']);
  const attributes = new Set(['id', 'class', 'transform', 'x', 'y', 'width', 'height', 'viewBox', 'rx',
    'refX', 'refY', 'markerWidth', 'markerHeight', 'orient', 'marker-end', 'd', 'r', 'text-anchor', 'xmlns']);
  for (const node of [svg, ...svg.querySelectorAll('*')]) {
    if (!tags.has(node.localName) || node.namespaceURI !== NS) { node.remove(); continue; }
    for (const attribute of [...node.attributes]) {
      if (!attributes.has(attribute.name) && !attribute.name.startsWith('data-')) node.removeAttribute(attribute.name);
    }
    if (node.hasAttribute('marker-end') && node.getAttribute('marker-end') !== 'url(#arrow)') node.removeAttribute('marker-end');
  }
  const nodes = new Map(), edges = new Map();
  const finite = n => Number.isFinite(n) && Math.abs(n) < 1e7;
  for (const template of svg.querySelectorAll('.wallboard-node')) {
    const id = template.dataset.id;
    const card = template.querySelector('.wallboard-card');
    const position = (template.getAttribute('transform') || '').match(/^translate\(([-\d.e+]+)[ ,]+([-\d.e+]+)\)$/i);
    const x = Number(position?.[1]), y = Number(position?.[2]);
    const width = Number(card?.getAttribute('width')), height = Number(card?.getAttribute('height'));
    if (!id || !card || ![x, y, width, height].every(finite) || width <= 0 || height <= 0) throw Error('Invalid card');
    const priority = template.dataset.priority === '' ? null : Number(template.dataset.priority);
    nodes.set(id, { id, template, x, y, width, height, priority: Number.isFinite(priority) ? priority : null,
      title: template.dataset.layoutTitle || template.querySelector('.title')?.textContent || '',
      status: [...template.classList].find(name => name.startsWith('status-'))?.slice(7) || 'open',
      revision: template.dataset.revision || template.textContent });
  }
  for (const template of svg.querySelectorAll('.edge')) {
    const from = template.dataset.from, to = template.dataset.to;
    if (!nodes.has(from) || !nodes.has(to)) continue;
    let points;
    try { points = JSON.parse(template.dataset.route); } catch {
      points = (template.getAttribute('d')?.match(/[MLC][^MLC]*/g) || []).map(command =>
        command.slice(1).match(/-?\d+(?:\.\d+)?/g)?.slice(-2).map(Number));
    }
    if (!Array.isArray(points) || points.length < 2 || !points.every(p => Array.isArray(p) && p.length === 2 && p.every(finite))) throw Error('Invalid edge');
    const key = from + '>' + to + (template.classList.contains('edge-proposed') ? '?' : '');
    edges.set(key, { key, from, to, template, points });
  }
  const viewBox = (svg.getAttribute('viewBox') || '').split(/\s+/).map(Number);
  if (viewBox.length !== 4 || !viewBox.every(finite) || viewBox[2] <= 0 || viewBox[3] <= 0) throw Error('Invalid viewBox');
  return { source: new XMLSerializer().serializeToString(svg), template: svg, nodes, edges,
    width: viewBox[2], height: viewBox[3] };
}

export function changes(before, after) {
  const arrived = [...after.nodes.keys()].filter(id => !before.nodes.has(id));
  const departed = [...before.nodes.keys()].filter(id => !after.nodes.has(id));
  // A card's revision states its task's relations, so a real relation change edits both its cards. A
  // changed arrow alone says nothing of either: arrows come and go with the cards at their ends, and a
  // chain through an arriving card replaces the arrow it implies between two others.
  const edited = [...after.nodes.keys()].filter(id => before.nodes.has(id) && before.nodes.get(id).revision !== after.nodes.get(id).revision);
  const arrowsChanged = [...before.edges.keys(), ...after.edges.keys()].some(key => before.edges.has(key) !== after.edges.has(key));
  return { arrived, departed, edited, any: Boolean(arrived.length || departed.length || edited.length || arrowsChanged) };
}
