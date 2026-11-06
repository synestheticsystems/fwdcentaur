import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { validateSettings } from '../../crates/fwd-web/src/wallboard-settings.mjs';

test('shared settings contract accepts and normalizes the same revisions in every consumer', () => {
  for (const example of JSON.parse(readFileSync(new URL('./settings-cases.json', import.meta.url)))) {
    if (example.invalid) assert.throws(() => validateSettings(example.input), example.name);
    else assert.deepEqual(validateSettings(example.input), example.expected, example.name);
  }
  for (const latitude of [NaN, Infinity, -Infinity]) assert.throws(() => validateSettings({ theme_schedule: { latitude } }));
});

test('an enabled schedule names the coordinates it is missing', () => {
  assert.throws(() => validateSettings({ theme_schedule: { enabled: true, latitude: 51.48 } }),
    { message: 'settings.theme_schedule: enabled requires longitude' });
  assert.throws(() => validateSettings({ theme_schedule: { enabled: true } }),
    { message: 'settings.theme_schedule: enabled requires latitude, longitude' });
});
