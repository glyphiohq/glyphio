// What a key press means in the clipboard list, kept apart from the DOM so it can be tested.
//
// Clearing history is destructive, so it takes two presses: one to ask, one to confirm. While
// the question is on screen every key is an answer — Enter clears what isn't pinned, Alt+Enter
// clears everything, and anything not named here is a "no".

/**
 * @param {{key: string, metaKey?: boolean, ctrlKey?: boolean, shiftKey?: boolean, altKey?: boolean}} e
 * @param {{confirming: boolean, hasClips: boolean}} ctx
 * @returns {{type: 'clear-start' | 'clear-confirm' | 'clear-confirm-pinned' | 'clear-toggle-empty'
 *   | 'clear-cancel' | 'none'}}
 */
export function clipKeyIntent(e, { confirming, hasClips }) {
  if (confirming) {
    if (isModifierOnly(e.key)) return { type: 'none' }; // pressing Alt on the way to Alt+Enter
    if (e.key === 'Enter') {
      return { type: e.altKey ? 'clear-confirm-pinned' : 'clear-confirm' };
    }
    if ((e.key === 'e' || e.key === 'E') && !e.metaKey && !e.ctrlKey && !e.altKey) {
      return { type: 'clear-toggle-empty' };
    }
    return { type: 'clear-cancel' }; // Escape, or any other key
  }
  const mod = e.metaKey || e.ctrlKey;
  if (mod && e.shiftKey && (e.key === 'Backspace' || e.key === 'Delete') && hasClips) {
    return { type: 'clear-start' };
  }
  return { type: 'none' };
}

function isModifierOnly(key) {
  return key === 'Alt' || key === 'Shift' || key === 'Control' || key === 'Meta';
}

/** The question shown in the confirmation row. */
export function confirmationText(total, pinned) {
  const entries = `${total} clipboard ${total === 1 ? 'entry' : 'entries'}`;
  const kept = pinned > 0
    ? `  ${pinned} pinned ${pinned === 1 ? 'entry is' : 'entries are'} kept.`
    : '';
  return `Clear ${entries}?${kept}`;
}
