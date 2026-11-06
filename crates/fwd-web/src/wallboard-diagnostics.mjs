// Bounded, content-free diagnostics: one record per failure area, timing stage, milestone and
// transition. No background timer, task text, URLs or credentials.
import { buildInfo } from './wallboard-build.mjs';

const BOOT_STAGES = ['mounted', 'configuration', 'node', 'context'];
const boot = {};
// Boot happens once per page, before any runtime exists. Each stage is in milliseconds since
// navigation start (performance.timeOrigin), the clock the runtime's milestones use too.
export function markBoot(stage) {
  if (BOOT_STAGES.includes(stage)) boot[stage] ??= performance.now();
}

// What failed, without what it failed on. Platform exceptions and parse errors keep only their
// standard names, since their messages can quote storage keys, addresses or the text they could
// not parse; the wallboard's own errors carry fixed messages, cut short to keep records small.
export function reasonOf(error) {
  if (error instanceof DOMException || error instanceof SyntaxError) return error.name;
  return String(error?.message ?? error).slice(0, 160);
}

// FNV-1a over UTF-8: a short identity that tells revisions apart without recording their values.
function digest(text) {
  let hash = 0x811c9dc5;
  for (const byte of new TextEncoder().encode(text)) hash = Math.imul(hash ^ byte, 0x01000193) >>> 0;
  return hash.toString(16).padStart(8, '0');
}

export class Diagnostics {
  constructor(now = Date.now, clock = () => performance.now()) {
    this.now = now; this.clock = clock; this.startedAt = now();
    this.issues = {}; this.timings = {}; this.state = {}; this.milestones = {}; this.latest = {};
  }
  issue(area, code, reason) {
    const previous = this.issues[area];
    this.issues[area] = { code, reason, active: true, count: (previous?.count || 0) + 1,
      firstAt: previous?.firstAt ?? this.now(), lastAt: this.now() };
  }
  // Without codes, any issue in the area ends; with them, only an issue with one of those codes,
  // so an operation's success clears just the failures it disproves.
  clear(area, ...codes) {
    const issue = this.issues[area];
    if (issue && (!codes.length || codes.includes(issue.code))) issue.active = false;
  }
  // The first time the display reaches a stage, kept once, on the boot clock.
  milestone(name) { this.milestones[name] ??= this.clock(); }
  // The latest time a repeated transition happened, on the same clock.
  transition(name) { this.latest[name] = this.clock(); }
  // Identifies the active validated settings by digest, so samples can be compared.
  recordSettings(settings) { this.state.settingsDigest = digest(JSON.stringify(settings)); }
  measure(stage, operation) {
    const start = this.clock();
    try { return operation(); }
    finally {
      this.sample(stage, Math.max(0, this.clock() - start));
    }
  }
  sample(stage, ms) {
    if (!Number.isFinite(ms) || ms < 0) return;
    const previous = this.timings[stage];
    this.timings[stage] = { count: (previous?.count || 0) + 1, lastMs: ms,
      totalMs: (previous?.totalMs || 0) + ms, maxMs: Math.max(previous?.maxMs || 0, ms) };
  }
  snapshot() { return structuredClone({ build: buildInfo, boot, startedAt: this.startedAt, state: this.state,
    milestones: this.milestones, latest: this.latest, issues: this.issues, timings: this.timings }); }
}
