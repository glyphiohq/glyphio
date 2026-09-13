import assert from 'node:assert/strict';
import test from 'node:test';

import {
  acceleratorFromKeyboardEvent,
  formatAccelerator,
  normalizeAccelerator,
  parseAccelerator,
  validateAccelerator,
} from './shortcut-recorder.mjs';

test('records a keyboard chord as a Tauri accelerator', () => {
  assert.deepEqual(acceleratorFromKeyboardEvent({
    code: 'KeyS', key: 's', metaKey: true, shiftKey: true, altKey: false, ctrlKey: false,
  }), { accelerator: 'Shift+Command+S' });
});

test('modifier-only and unsupported events are rejected with useful feedback', () => {
  assert.match(acceleratorFromKeyboardEvent({ code: 'ShiftLeft', key: 'Shift' }).error, /non-modifier/);
  assert.match(acceleratorFromKeyboardEvent({ code: 'IntlRo', key: '\\' }).error, /cannot be used/);
});

test('old aliases remain parseable and compare by the key they register on macOS', () => {
  assert.deepEqual(parseAccelerator('Alt+Shift+KeyS'), {
    modifiers: ['Alt', 'Shift'], key: 'S',
  });
  assert.equal(normalizeAccelerator('CmdOrCtrl+Shift+KeyS'), 'Shift+Command+S');
  assert.equal(normalizeAccelerator('Shift+Super+S'), 'Shift+Command+S');
});

test('formats existing and recorded accelerators in macOS notation', () => {
  assert.equal(formatAccelerator('Alt+Shift+S'), '⌥⇧S');
  assert.equal(formatAccelerator('Command+Shift+ArrowUp'), '⇧⌘↑');
  assert.equal(formatAccelerator(''), 'Not set');
});

test('rejects reserved macOS shortcuts and collisions across aliases', () => {
  assert.match(validateAccelerator('Command+Space').error, /Spotlight/);
  assert.match(validateAccelerator('Command+Shift+Digit4').error, /screenshots/);
  assert.deepEqual(validateAccelerator('Command+Shift+S', [
    { value: 'Shift+Super+KeyS', label: 'Visible area' },
  ]), { ok: false, error: 'Already used by Visible area.' });
});

test('accepts cleared and distinct shortcuts', () => {
  assert.deepEqual(validateAccelerator(''), { ok: true });
  assert.deepEqual(validateAccelerator('Control+Alt+P', [
    { value: 'Alt+Shift+P', label: 'Scrolling page' },
  ]), { ok: true });
});
