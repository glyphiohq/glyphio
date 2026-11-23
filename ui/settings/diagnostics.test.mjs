import assert from 'node:assert/strict';
import test from 'node:test';

import { formatDiagnostics, redact, shortcutLine, testMessage } from './diagnostics.mjs';

const sample = {
  version: '1.2.2',
  os: 'macos',
  arch: 'aarch64',
  shortcuts: [
    { key: 'shortcutCaptureSnip', accelerator: 'Alt+Shift+X', state: 'registered' },
    { key: 'shortcutCaptureFull', accelerator: 'Alt+Shift+S', state: 'refused', reason: 'taken by another app' },
    { key: 'shortcutCaptureSnipSilent', accelerator: '', state: 'empty' },
  ],
  capture: 'Capture status: Ready',
  permissions: { accessibility: 'granted', screenRecording: 'denied', expansionLive: true },
  engine: { stoppedAfterCrashes: false, secureInputHeld: true },
};

test('the test distinguishes not registered from registered but swallowed', () => {
  assert.equal(testMessage('received').ok, true);
  assert.match(testMessage('notReceived').text, /Registered, but/);
  assert.match(testMessage('notRegistered').text, /Not registered/);
  assert.notEqual(testMessage('notReceived').text, testMessage('notRegistered').text);
});

test('the report covers every binding, capture state, permissions and engine', () => {
  const text = formatDiagnostics(sample, (k) => k.replace('shortcut', ''));
  assert.match(text, /Glyphio 1\.2\.2 on macos \(aarch64\)/);
  assert.match(text, /CaptureSnip: Alt\+Shift\+X — registered/);
  assert.match(text, /CaptureFull: Alt\+Shift\+S — refused by the system \(taken by another app\)/);
  assert.match(text, /CaptureSnipSilent: \(none\) — not set/);
  assert.match(text, /Capture: Capture status: Ready/);
  assert.match(text, /Screen Recording: denied/);
  assert.match(text, /Secure Input held by another app: yes/);
});

test('the report never carries an address', () => {
  const leaky = { ...sample, capture: 'Saved https://example.com/secret?q=1 and www.example.org/x' };
  const text = formatDiagnostics(leaky);
  assert.doesNotMatch(text, /example\.(com|org)/);
  assert.match(text, /\[address removed\]/);
  assert.equal(redact('file:///Users/a/x.png ok'), '[address removed] ok');
});

test('a shortcut line reads as a sentence', () => {
  assert.equal(
    shortcutLine({ key: 'k', accelerator: 'Alt+V', state: 'invalid', reason: 'already used by another action' }, () => 'Visible'),
    'Visible: Alt+V — invalid (already used by another action)',
  );
});
