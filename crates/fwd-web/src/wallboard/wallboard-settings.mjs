import { contract } from './wallboard-contract.mjs';
export const LIGHT_THEMES = contract.properties.theme_schedule.properties.day_theme.enum;
export const THEMES = contract.properties.theme.enum;

// A value the contract accepts, with its defaults filled in. Throws on anything else.
export function validateSettings(value, spec = contract, path = 'settings') {
  if (value === null && spec.nullable) return null;
  const valid = { object: value !== null && typeof value === 'object' && !Array.isArray(value),
    array: Array.isArray(value), string: typeof value === 'string', boolean: typeof value === 'boolean',
    integer: Number.isSafeInteger(value), number: typeof value === 'number' && Number.isFinite(value) }[spec.type];
  if (!valid) throw Error(`${path}: expected ${spec.type}`);
  if (spec.type === 'object') {
    const result = Object.fromEntries(Object.entries(spec.properties).map(([key, child]) =>
      [key, validateSettings(Object.hasOwn(value, key) ? value[key] : structuredClone(child.default), child, `${path}.${key}`)]));
    for (const [flag, required] of Object.entries(spec.required_when || {})) {
      const missing = required.filter(key => result[key] === null);
      if (result[flag] && missing.length) throw Error(`${path}: ${flag} requires ${missing.join(', ')}`);
    }
    return result;
  }
  if (spec.type === 'array') {
    if (value.length < (spec.min_length || 0) || value.some(item => !spec.enum.includes(item))) throw Error(`${path}: invalid selection`);
    if (spec.unique && new Set(value).size !== value.length) throw Error(`${path}: duplicate selection`);
  } else if (spec.enum && !spec.enum.includes(value)) throw Error(`${path}: unknown value`);
  if (typeof value === 'number' && (value < (spec.min ?? -Infinity) || value > (spec.max ?? Infinity))) throw Error(`${path}: outside supported range`);
  return structuredClone(value);
}
