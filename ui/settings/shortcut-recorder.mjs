/**
 * Pure shortcut recording and validation rules shared by the Settings UI and its tests.
 * Stored values deliberately remain Tauri accelerators; only their presentation is macOS-like.
 */

export const CAPTURE_SHORTCUT_DEFAULTS = Object.freeze({
  shortcutCaptureFull: 'Alt+Shift+S',
  shortcutCaptureVisible: 'Alt+Shift+V',
  shortcutCaptureSnip: 'Alt+Shift+X',
  shortcutCaptureFrontWindow: 'Alt+Shift+W',
  shortcutCapturePage: 'Alt+Shift+B',
  shortcutCaptureScroll: 'Alt+Shift+L',
  shortcutCaptureScrollPage: 'Alt+Shift+P',
  shortcutCaptureFullSilent: '',
  shortcutCaptureVisibleSilent: '',
  shortcutCaptureSnipSilent: '',
  shortcutCaptureFrontWindowSilent: '',
  shortcutCapturePageSilent: '',
  shortcutCaptureScrollSilent: '',
  shortcutCaptureScrollPageSilent: '',
});

const MODIFIER_ALIASES = new Map([
  ['alt', 'Alt'], ['option', 'Alt'],
  ['control', 'Control'], ['ctrl', 'Control'],
  ['shift', 'Shift'],
  ['command', 'Command'], ['cmd', 'Command'], ['super', 'Command'],
  ['commandorcontrol', 'Command'], ['commandorctrl', 'Command'],
  ['cmdorcontrol', 'Command'], ['cmdorctrl', 'Command'],
]);

const MODIFIER_CODES = new Set([
  'AltLeft', 'AltRight', 'ControlLeft', 'ControlRight', 'MetaLeft', 'MetaRight',
  'ShiftLeft', 'ShiftRight',
]);

const KEY_ALIASES = new Map([
  ['`', 'Backquote'], ['\\', 'Backslash'], ['[', 'BracketLeft'], [']', 'BracketRight'],
  [',', 'Comma'], ['=', 'Equal'], ['-', 'Minus'], ['.', 'Period'], ["'", 'Quote'],
  [';', 'Semicolon'], ['/', 'Slash'],
  ['esc', 'Escape'], ['escape', 'Escape'],
  ['down', 'ArrowDown'], ['arrowdown', 'ArrowDown'],
  ['left', 'ArrowLeft'], ['arrowleft', 'ArrowLeft'],
  ['right', 'ArrowRight'], ['arrowright', 'ArrowRight'],
  ['up', 'ArrowUp'], ['arrowup', 'ArrowUp'],
  ['pausebreak', 'Pause'], ['pause', 'Pause'],
  ['numadd', 'NumpadAdd'], ['numpadplus', 'NumpadAdd'], ['numplus', 'NumpadAdd'],
  ['numdecimal', 'NumpadDecimal'], ['numdivide', 'NumpadDivide'],
  ['numenter', 'NumpadEnter'], ['numequal', 'NumpadEqual'],
  ['nummultiply', 'NumpadMultiply'], ['numsubtract', 'NumpadSubtract'],
  ['volumedown', 'AudioVolumeDown'], ['audiovolumedown', 'AudioVolumeDown'],
  ['volumeup', 'AudioVolumeUp'], ['audiovolumeup', 'AudioVolumeUp'],
  ['volumemute', 'AudioVolumeMute'], ['audiovolumemute', 'AudioVolumeMute'],
  ['mediatrackprev', 'MediaTrackPrevious'],
]);

const NAMED_CODES = new Set([
  'Backquote', 'Backslash', 'BracketLeft', 'BracketRight', 'Pause', 'Comma', 'Equal',
  'Minus', 'Period', 'Quote', 'Semicolon', 'Slash', 'Backspace', 'CapsLock', 'Enter',
  'Space', 'Tab', 'Delete', 'End', 'Home', 'Insert', 'PageDown', 'PageUp',
  'PrintScreen', 'ScrollLock', 'ArrowDown', 'ArrowLeft', 'ArrowRight', 'ArrowUp',
  'NumLock', 'NumpadAdd', 'NumpadDecimal', 'NumpadDivide', 'NumpadEnter',
  'NumpadEqual', 'NumpadMultiply', 'NumpadSubtract', 'Escape', 'AudioVolumeDown',
  'AudioVolumeUp', 'AudioVolumeMute', 'MediaPlay', 'MediaPause', 'MediaPlayPause',
  'MediaStop', 'MediaTrackNext', 'MediaTrackPrevious',
]);

const DISPLAY_KEYS = new Map([
  ['ArrowDown', '↓'], ['ArrowLeft', '←'], ['ArrowRight', '→'], ['ArrowUp', '↑'],
  ['Backspace', '⌫'], ['Delete', '⌦'], ['Enter', '↩'], ['Escape', 'Esc'],
  ['Space', 'Space'], ['Tab', '⇥'], ['PageDown', 'PgDn'], ['PageUp', 'PgUp'],
  ['BracketLeft', '['], ['BracketRight', ']'], ['Backquote', '`'], ['Backslash', '\\'],
  ['Comma', ','], ['Equal', '='], ['Minus', '-'], ['Period', '.'], ['Quote', "'"],
  ['Semicolon', ';'], ['Slash', '/'],
]);

// These combinations belong to macOS itself and cannot be dependable Glyphio bindings.
const RESERVED = new Map([
  ['Command+Space', 'Command-Space is reserved for Spotlight.'],
  ['Command+Tab', 'Command-Tab is reserved for switching applications.'],
  ['Shift+Command+3', 'Command-Shift-3 is reserved for macOS screenshots.'],
  ['Shift+Command+4', 'Command-Shift-4 is reserved for macOS screenshots.'],
  ['Shift+Command+5', 'Command-Shift-5 is reserved for macOS screenshots and recording.'],
  ['Control+Command+Q', 'Control-Command-Q is reserved for locking your Mac.'],
  ['Alt+Command+Escape', 'Option-Command-Escape is reserved for Force Quit.'],
]);

const MODIFIER_ORDER = ['Control', 'Alt', 'Shift', 'Command'];

/**
 * Own the one active shortcut-recording session at document scope.
 *
 * macOS WebKit does not reliably focus a button after a pointer click. Listening on the
 * document keeps physical keyboard input observable even when focus remains on the page.
 */
export function createShortcutCaptureController(eventTarget) {
  let activeSession = null;
  const onKeydown = (event) => {
    if (!activeSession) return;
    event.preventDefault();
    event.stopPropagation();
    activeSession.handleKeydown(event);
  };

  eventTarget.addEventListener('keydown', onKeydown, true);
  return {
    start(session) {
      if (activeSession === session) return;
      const previousSession = activeSession;
      activeSession = session;
      previousSession?.cancel?.();
    },
    stop(session) {
      if (activeSession === session) activeSession = null;
    },
    destroy() {
      activeSession?.cancel?.();
      activeSession = null;
      eventTarget.removeEventListener('keydown', onKeydown, true);
    },
  };
}

function canonicalKey(raw) {
  const token = raw.trim();
  const lower = token.toLowerCase();
  if (KEY_ALIASES.has(lower)) return KEY_ALIASES.get(lower);
  if (/^key[a-z]$/i.test(token)) return token.slice(-1).toUpperCase();
  if (/^[a-z]$/i.test(token)) return token.toUpperCase();
  if (/^digit[0-9]$/i.test(token)) return token.slice(-1);
  if (/^[0-9]$/.test(token)) return token;
  if (/^f(?:[1-9]|1[0-9]|2[0-4])$/i.test(token)) return token.toUpperCase();
  if (/^numpad[0-9]$/i.test(token)) return `Numpad${token.slice(-1)}`;
  if (/^num[0-9]$/i.test(token)) return `Numpad${token.slice(-1)}`;
  const named = [...NAMED_CODES].find((key) => key.toLowerCase() === lower);
  return named ?? null;
}

/** Parse every accelerator form accepted by the existing macOS settings. */
export function parseAccelerator(accelerator) {
  if (typeof accelerator !== 'string' || !accelerator.trim()) return null;
  const tokens = accelerator.split('+').map((part) => part.trim());
  if (tokens.some((token) => !token)) return null;
  const modifiers = new Set();
  let key = null;
  for (const token of tokens) {
    const modifier = MODIFIER_ALIASES.get(token.toLowerCase());
    if (modifier && key === null) {
      modifiers.add(modifier);
      continue;
    }
    if (key !== null) return null;
    key = canonicalKey(token);
    if (!key) return null;
  }
  if (!key) return null;
  return { modifiers: MODIFIER_ORDER.filter((modifier) => modifiers.has(modifier)), key };
}

/** A stable representation for equality checks across old accelerator aliases. */
export function normalizeAccelerator(accelerator) {
  const parsed = parseAccelerator(accelerator);
  return parsed ? [...parsed.modifiers, parsed.key].join('+') : null;
}

/** Format an accelerator for a macOS user without changing its persisted representation. */
export function formatAccelerator(accelerator) {
  const parsed = parseAccelerator(accelerator);
  if (!parsed) return accelerator || 'Not set';
  const symbols = { Control: '⌃', Alt: '⌥', Shift: '⇧', Command: '⌘' };
  const key = DISPLAY_KEYS.get(parsed.key) ?? parsed.key.replace(/^Numpad/, 'Num ');
  return `${parsed.modifiers.map((modifier) => symbols[modifier]).join('')}${key}`;
}

function eventKey(code) {
  if (/^Key[A-Z]$/.test(code)) return code.slice(-1);
  if (/^Digit[0-9]$/.test(code)) return code.slice(-1);
  if (/^F(?:[1-9]|1[0-9]|2[0-4])$/.test(code)) return code;
  if (/^Numpad[0-9]$/.test(code)) return code;
  return NAMED_CODES.has(code) ? code : null;
}

/** Turn a real KeyboardEvent into a Tauri accelerator, never text typed into a field. */
export function acceleratorFromKeyboardEvent(event) {
  if (MODIFIER_CODES.has(event.code) || ['Alt', 'Control', 'Meta', 'Shift'].includes(event.key)) {
    return { error: 'Add a non-modifier key to finish the shortcut.' };
  }
  const key = eventKey(event.code);
  if (!key) return { error: 'That key cannot be used in a global shortcut.' };
  const modifiers = [];
  if (event.ctrlKey) modifiers.push('Control');
  if (event.altKey) modifiers.push('Alt');
  if (event.shiftKey) modifiers.push('Shift');
  if (event.metaKey) modifiers.push('Command');
  return { accelerator: [...modifiers, key].join('+') };
}

/**
 * Validate one recorded shortcut. `configured` contains all of Glyphio's other shortcuts,
 * including the unrelated text fields that this feature deliberately leaves unchanged.
 */
export function validateAccelerator(accelerator, configured = []) {
  if (!accelerator) return { ok: true };
  const normalized = normalizeAccelerator(accelerator);
  if (!normalized) return { ok: false, error: 'Glyphio does not recognise that shortcut.' };
  const reserved = RESERVED.get(normalized);
  if (reserved) return { ok: false, error: reserved };
  const collision = configured.find(({ value }) => value && normalizeAccelerator(value) === normalized);
  if (collision) {
    return { ok: false, error: `Already used by ${collision.label || 'another Glyphio shortcut'}.` };
  }
  return { ok: true };
}
