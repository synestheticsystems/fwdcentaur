import assert from 'node:assert/strict';
import test from 'node:test';
import { themeState, mountTheme } from '../../../../crates/fwd-web/src/wallboard/wallboard-theme.mjs';
import { getTimes } from '../../../../crates/fwd-web/src/wallboard/suncalc.mjs';

const settings = (latitude = 51.48, longitude = 0) => ({ theme: 'charcoal', theme_schedule: {
  enabled: true, latitude, longitude, day_theme: 'linen-indigo', night_theme: 'indigo-saffron',
} });
const at = value => new Date(value).getTime();

test('manual selection and a disabled schedule keep the chosen palette', () => {
  assert.deepEqual(themeState({ theme: 'sand-plum', theme_schedule: null }), { theme: 'sand-plum', mode: 'manual', next: null });
  const disabled = settings(); disabled.theme_schedule.enabled = false;
  assert.deepEqual(themeState(disabled), { theme: 'charcoal', mode: 'manual', next: null });
});

test('sunrise and sunset switch at the event, with the next event strictly in the future', () => {
  // Greenwich at the equinox: sunrise near 06:00 UTC and sunset near 18:00 UTC.
  const date = at('2026-03-20T12:00:00Z'), configured = settings();
  const times = getTimes(new Date(date), 51.48, 0);
  assert.ok(Math.abs(+times.sunrise - at('2026-03-20T06:00:00Z')) < 20 * 60000);
  assert.ok(Math.abs(+times.sunset - at('2026-03-20T18:00:00Z')) < 20 * 60000);
  for (const [event, before, after] of [
    ['sunrise', 'indigo-saffron', 'linen-indigo'], ['sunset', 'linen-indigo', 'indigo-saffron'],
  ]) {
    const instant = +times[event];
    assert.equal(themeState(configured, instant - 1).theme, before);
    assert.equal(themeState(configured, instant).theme, after);
    assert.ok(themeState(configured, instant).next.at > instant);
  }
});

test('date line, UTC midnight and a DST transition retain the correct solar day', () => {
  for (const longitude of [-179.9, 179.9]) {
    assert.equal(themeState(settings(0, longitude), at('2026-03-20T00:00:00Z')).theme, 'linen-indigo');
    assert.equal(themeState(settings(0, longitude), at('2026-03-20T12:00:00Z')).theme, 'indigo-saffron');
  }
  const california = settings(37.77, -122.42);
  for (const date of ['2026-03-08T09:59:59Z', '2026-03-08T10:00:00Z', '2026-11-01T08:59:59Z', '2026-11-01T09:00:00Z']) {
    assert.equal(themeState(california, at(date)).theme, 'indigo-saffron');
  }
  assert.equal(themeState(california, at('2026-11-01T20:00:00Z')).theme, 'linen-indigo');
  assert.equal(themeState(california, at('2026-11-02T00:00:00Z')).theme, 'linen-indigo');
  assert.equal(themeState(california, at('2026-11-02T03:00:00Z')).theme, 'indigo-saffron');
});

test('polar days and nights have no invented next event', () => {
  for (const [latitude, summer, winter] of [[89, 'linen-indigo', 'indigo-saffron'], [-89, 'indigo-saffron', 'linen-indigo']]) {
    assert.deepEqual(themeState(settings(latitude), at('2026-06-21T12:00:00Z')), { theme: summer, mode: 'solar', next: null });
    assert.deepEqual(themeState(settings(latitude), at('2026-12-21T12:00:00Z')), { theme: winter, mode: 'solar', next: null });
  }
});

test('controller follows the clock offline, covers the page, takes each revision, and cleans up', () => {
  let now = at('2026-03-20T05:00:00Z'), nextId = 0;
  const timers = new Map();
  const window = new EventTarget();
  window.getComputedStyle = root => ({ backgroundColor: root.dataset.colorMode === 'light' ? 'rgb(241, 237, 229)' : 'rgb(0, 0, 0)' });
  const document = Object.assign(new EventTarget(), { defaultView: window, documentElement: { style: {} }, body: { style: {} } });
  const root = { dataset: {}, ownerDocument: document };
  const environment = { now: () => now, setTimer: (callback, delay) => { const id = ++nextId; timers.set(id, { callback, delay }); return id; }, clearTimer: id => timers.delete(id) };
  const controller = mountTheme(root, settings(), environment);
  assert.equal(root.dataset.theme, 'indigo-saffron'); assert.equal(timers.size, 1);
  now = at('2026-03-20T12:00:00Z'); document.dispatchEvent(new Event('visibilitychange'));
  assert.equal(root.dataset.theme, 'linen-indigo'); assert.equal(document.body.style.backgroundColor, 'rgb(241, 237, 229)');
  assert.equal(document.documentElement.style.backgroundColor, document.body.style.backgroundColor);
  assert.equal(document.documentElement.style.colorScheme, 'light');
  now = +getTimes(new Date(now), 51.48, 0).sunset - 500;
  controller.refresh(); assert.equal([...timers.values()][0].delay, 500);
  now += 500; [...timers.values()][0].callback();
  assert.equal(root.dataset.theme, 'indigo-saffron'); assert.equal(timers.size, 1);
  controller.configure({ theme: 'graphite-coral', theme_schedule: null });
  assert.equal(root.dataset.theme, 'graphite-coral'); assert.equal(root.dataset.themeMode, 'manual');
  assert.equal(timers.size, 0, 'a manual palette needs no timer');
  controller.configure(settings());
  assert.equal(root.dataset.theme, 'indigo-saffron'); assert.equal(timers.size, 1);
  const replacement = mountTheme(root, { theme: 'porcelain-coral' }, environment);
  assert.equal(controller.disposed, true, 'mounting another disposes the one before'); assert.equal(timers.size, 0);
  replacement.dispose(); now += 86400000; window.dispatchEvent(new Event('pageshow'));
  assert.equal(root.dataset.theme, 'porcelain-coral'); assert.equal(timers.size, 0);
});
