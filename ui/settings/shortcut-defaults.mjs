// Ordered fallbacks for a default shortcut the system refused, so setup can offer a working
// chord instead of a dead end. Pure: filtering is by the same validation Settings uses.

import { validateAccelerator } from './shortcut-recorder.mjs';

const ALTERNATIVES = Object.freeze({
  shortcutCaptureSnip: ['Control+Alt+X', 'Control+Shift+X', 'Alt+Shift+Z'],
  shortcutOpenPalette: ['Control+Alt+Space', 'Alt+Shift+Space', 'Control+Shift+P'],
  shortcutOpenClipboard: ['Control+Alt+C', 'Control+Shift+C', 'Alt+Shift+V'],
});

/**
 * Up to `limit` alternatives for `key`, in order, skipping chords that are reserved on this
 * platform or already used by another Glyphio shortcut. `configured` is `[{ value, label }]`.
 */
export function alternativesFor(key, { platform = 'macos', configured = [], limit = 3 } = {}) {
  return (ALTERNATIVES[key] ?? [])
    .filter((accelerator) => validateAccelerator(accelerator, configured, platform).ok)
    .slice(0, limit);
}
