// All astronomy runs locally; no location lookup, weather service, or graph connection.
import { getTimes } from './suncalc.mjs';

import { LIGHT_THEMES, THEMES } from './wallboard-settings.mjs';
const DAY = 86400000;

// Settings arrive validated by the shared contract: an enabled schedule always has coordinates
// and a light day palette and a dark night palette, so nothing here guesses a location.
export function themeState(settings, now = Date.now()) {
  const schedule = settings.theme_schedule;
  if (!schedule?.enabled) return { theme: settings.theme, mode: 'manual', next: null };
  // Adjacent solar days cover midnight and either side of the date line. Instants are UTC;
  // changing timezone or daylight saving time cannot shift the physical sunrise or sunset.
  const events = [];
  let current;
  for (const offset of [-1, 0, 1]) {
    const times = getTimes(new Date(now + offset * DAY), schedule.latitude, schedule.longitude);
    if (!offset) current = times;
    for (const [name, theme] of [['sunrise', schedule.day_theme], ['sunset', schedule.night_theme]]) {
      const at = times[name]?.getTime();
      if (Number.isFinite(at)) events.push({ at, theme, event: name });
    }
  }
  events.sort((a, b) => a.at - b.at);
  const preceding = events.filter(event => event.at <= now).at(-1);
  const next = events.find(event => event.at > now) || null;
  const theme = preceding?.theme ?? (current.alwaysUp ? schedule.day_theme : schedule.night_theme);
  return { theme, mode: 'solar', next };
}

export class ThemeController {
  constructor(root, settings, environment = {}) {
    this.root = root; this.settings = settings; this.now = environment.now || Date.now;
    this.setTimer = environment.setTimer || ((callback, delay) => setTimeout(callback, delay));
    this.clearTimer = environment.clearTimer || (timer => clearTimeout(timer));
    this.document = root.ownerDocument; this.window = this.document.defaultView;
    this.onWake = () => this.refresh();
    this.document.addEventListener('visibilitychange', this.onWake);
    this.window.addEventListener('pageshow', this.onWake);
    this.refresh();
  }
  // A new settings revision replaces the palette and schedule, and ends any preview.
  configure(settings) {
    this.settings = settings; this.preview = null;
    return this.refresh();
  }
  refresh() {
    this.clearTimer(this.timer);
    if (this.disposed) return;
    const now = this.now();
    if (this.preview && now >= this.preview.until) this.preview = null;
    const state = this.preview ? { theme: this.preview.theme, mode: 'preview', next: null } : themeState(this.settings, now);
    this.root.dataset.theme = state.theme;
    this.root.dataset.colorMode = LIGHT_THEMES.includes(state.theme) ? 'light' : 'dark';
    this.root.dataset.themeMode = state.mode;
    if (state.next) this.root.dataset.themeNext = new Date(state.next.at).toISOString();
    else delete this.root.dataset.themeNext;
    const background = this.window.getComputedStyle(this.root).backgroundColor;
    this.document.documentElement.style.backgroundColor = background;
    this.document.body.style.backgroundColor = background;
    this.document.documentElement.style.colorScheme = this.root.dataset.colorMode;
    // At most one idle timer: recheck the clock after sleep/clock correction, and at the event.
    const due = this.preview?.until ?? state.next?.at;
    if (this.preview || state.mode === 'solar') {
      this.timer = this.setTimer(() => this.refresh(), Math.min(60000, Math.max(100, (due ?? now + 60000) - now)));
    }
    return state;
  }
  showPreview(theme, duration = 30000) {
    if (!THEMES.includes(theme)) throw new Error('Unknown wallboard theme');
    this.preview = { theme, until: this.now() + Math.max(1000, Math.min(300000, duration)) };
    return this.refresh();
  }
  resume() { this.preview = null; return this.refresh(); }
  dispose() {
    this.disposed = true; this.clearTimer(this.timer);
    this.document.removeEventListener('visibilitychange', this.onWake);
    this.window.removeEventListener('pageshow', this.onWake);
    if (controllers.get(this.root) === this) controllers.delete(this.root);
  }
}

const controllers = new WeakMap();
export function mountTheme(root, settings, environment) {
  controllers.get(root)?.dispose();
  const controller = new ThemeController(root, settings, environment);
  controllers.set(root, controller);
  return controller;
}
export function themeFor(root) { return controllers.get(root); }
// Called as soon as local configuration loads, even while the graph is waiting for its peer.
export function configureTheme(settings) {
  const root = document.querySelector('.wallboard');
  if (root) mountTheme(root, JSON.parse(settings));
}
