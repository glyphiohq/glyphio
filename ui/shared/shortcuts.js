// shared/shortcuts.js
// Platform-adaptive keyboard-shortcut helpers used by the preview page and
// the options page. No storage I/O. Module load reads `navigator` once to
// detect the platform; the pure helpers (matchesShortcut, formatShortcut,
// eventToSpec, isReserved) take plain data in and return plain data out.
// The `mod` logical modifier maps to Cmd on macOS, Ctrl on Windows/Linux.

/** @returns {'mac' | 'other'} */
function detectPlatform() {
  try {
    const uap = navigator.userAgentData;
    if (uap && typeof uap.platform === 'string') {
      return /mac|darwin/i.test(uap.platform) ? 'mac' : 'other';
    }
  } catch { /* fall through */ }
  return /mac|iphone|ipad|ipod/i.test(navigator.platform || '') ? 'mac' : 'other';
}

export const PLATFORM = detectPlatform();
export const IS_MAC = PLATFORM === 'mac';

/** OS/browser combos we refuse to let users bind — they belong to the system. */
const RESERVED_COMBOS = [
  { modifiers: ['meta'],        key: 'q' },       // macOS Quit
  { modifiers: ['meta'],        key: 'w' },       // Close tab/window
  { modifiers: ['meta'],        key: 'Tab' },     // macOS switch apps
  { modifiers: ['alt'],         key: 'Tab' },     // Win/Linux switch windows
  { modifiers: ['alt'],         key: 'F4' },      // Windows close
  { modifiers: ['ctrl'],        key: 'w' },       // Close tab
  { modifiers: ['ctrl'],        key: 'Tab' },     // Switch tabs
  { modifiers: ['meta'],        key: ' ' },       // Spotlight
  { modifiers: ['ctrl', 'alt'], key: 'Delete' }   // Windows task manager
];

/**
 * True if `event` matches the stored shortcut spec `{ modifiers, key }`.
 * `modifiers` is a list drawn from `'mod' | 'shift' | 'alt' | 'ctrl' | 'meta'`.
 */
export function matchesShortcut(event, spec) {
  if (!spec || typeof spec.key !== 'string' || !Array.isArray(spec.modifiers)) return false;
  const wantMeta  = spec.modifiers.includes('meta')  || (IS_MAC  && spec.modifiers.includes('mod'));
  const wantCtrl  = spec.modifiers.includes('ctrl')  || (!IS_MAC && spec.modifiers.includes('mod'));
  const wantAlt   = spec.modifiers.includes('alt');
  const wantShift = spec.modifiers.includes('shift');
  if (Boolean(event.metaKey)  !== wantMeta)  return false;
  if (Boolean(event.ctrlKey)  !== wantCtrl)  return false;
  if (Boolean(event.altKey)   !== wantAlt)   return false;
  if (Boolean(event.shiftKey) !== wantShift) return false;
  const a = normaliseKey(event.key);
  const b = normaliseKey(spec.key);
  return a === b;
}

/** Human-readable label like `⌘C` on Mac, `Ctrl+C` elsewhere. */
export function formatShortcut(spec) {
  if (!spec || typeof spec.key !== 'string' || !Array.isArray(spec.modifiers)) return '';
  const parts = [];
  const mods = spec.modifiers;
  const glyph = IS_MAC
    ? { meta: '⌘', ctrl: '⌃', alt: '⌥', shift: '⇧', mod: '⌘' }
    : { meta: 'Win', ctrl: 'Ctrl', alt: 'Alt', shift: 'Shift', mod: 'Ctrl' };
  const order = ['ctrl', 'alt', 'shift', 'meta', 'mod'];
  for (const m of order) {
    if (mods.includes(m)) parts.push(glyph[m]);
  }
  const sep = IS_MAC ? '' : '+';
  const keyLabel = labelKey(spec.key);
  return parts.length ? parts.join(sep) + sep + keyLabel : keyLabel;
}

function labelKey(key) {
  if (!key) return '';
  if (key === ' ') return 'Space';
  if (key.length === 1) return key.toUpperCase();
  return key; // e.g. 'Escape', 'F1'
}

function normaliseKey(key) {
  if (!key) return '';
  return key.length === 1 ? key.toLowerCase() : key;
}

/**
 * Convert a live KeyboardEvent into a spec `{ modifiers, key }` suitable for
 * storage. Returns null if the event is modifier-only or otherwise invalid.
 */
export function eventToSpec(event) {
  const keyName = event.key;
  if (!keyName) return null;
  // Ignore events that are ONLY a modifier being pressed.
  if (['Meta', 'Control', 'Alt', 'Shift', 'CapsLock', 'Dead'].includes(keyName)) {
    return null;
  }
  const modifiers = [];
  // Prefer `mod` (platform-agnostic) if the relevant platform modifier is
  // held: Cmd on Mac, Ctrl elsewhere. If BOTH Cmd and Ctrl are held we
  // record them literally.
  if (IS_MAC && event.metaKey && !event.ctrlKey) modifiers.push('mod');
  else if (!IS_MAC && event.ctrlKey && !event.metaKey) modifiers.push('mod');
  else {
    if (event.metaKey) modifiers.push('meta');
    if (event.ctrlKey) modifiers.push('ctrl');
  }
  if (event.altKey)   modifiers.push('alt');
  if (event.shiftKey) modifiers.push('shift');
  return { modifiers, key: keyName };
}

/**
 * Expand a spec to a comparable shape where `mod` is resolved to meta or
 * ctrl based on the current platform. Used only by the reservation check.
 */
function resolveMod(spec) {
  const mods = spec.modifiers.map((m) =>
    m === 'mod' ? (IS_MAC ? 'meta' : 'ctrl') : m
  );
  return { modifiers: mods, key: normaliseKey(spec.key) };
}

/** True if the spec collides with a reserved OS/browser combo. */
export function isReserved(spec) {
  const s = resolveMod(spec);
  return RESERVED_COMBOS.some((r) => {
    const a = r.modifiers.slice().sort().join('|');
    const b = s.modifiers.slice().sort().join('|');
    return a === b && normaliseKey(r.key) === s.key;
  });
}
