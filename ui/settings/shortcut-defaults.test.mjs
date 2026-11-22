import assert from 'node:assert/strict';
import test from 'node:test';

import { alternativesFor } from './shortcut-defaults.mjs';
import { validateAccelerator } from './shortcut-recorder.mjs';

test('alternatives come in order and are all valid on the platform', () => {
  for (const platform of ['macos', 'windows']) {
    for (const key of ['shortcutCaptureSnip', 'shortcutOpenPalette', 'shortcutOpenClipboard']) {
      const alts = alternativesFor(key, { platform });
      assert.ok(alts.length > 0, `${key} on ${platform}`);
      for (const a of alts) assert.equal(validateAccelerator(a, [], platform).ok, true);
    }
  }
  assert.equal(alternativesFor('shortcutOpenPalette')[0], 'Control+Alt+Space');
});

test('an alternative another Glyphio shortcut already uses is skipped', () => {
  const taken = [{ value: 'Control+Alt+Space', label: 'Something' }];
  assert.equal(alternativesFor('shortcutOpenPalette', { configured: taken })[0], 'Alt+Shift+Space');
});

test('the number of alternatives is capped', () => {
  assert.equal(alternativesFor('shortcutOpenPalette', { limit: 1 }).length, 1);
});

test('an unknown binding has no alternatives', () => {
  assert.deepEqual(alternativesFor('nope'), []);
});
