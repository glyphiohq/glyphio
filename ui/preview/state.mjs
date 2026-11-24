// preview/state.mjs — what the post-capture card does, as plain data, so it can be tested
// without a window.
//
// The card appears after a capture that is already saved and copied. It goes away by itself
// after a few seconds, but never while the pointer is on it or while one of its buttons is
// working, and a pointer that leaves gives the user a moment before it goes.

/** How long an untouched card stays up. */
export const DISMISS_AFTER_MS = 6000;
/** The least time left after the pointer leaves the card. */
export const LINGER_MS = 2500;

/** The card for capture `id`; `error` is why the copy failed, if it did. */
export function initialPreview({ id, error = '' }) {
  return {
    id,
    status: error ? 'copy-failed' : 'copied',
    message: error,
    hovering: false,
    busy: false,
    remaining: DISMISS_AFTER_MS,
    closed: false,
  };
}

/** The line under the thumbnail. */
export function statusText(state) {
  switch (state.status) {
    case 'copied': return 'Copied to clipboard';
    case 'copy-failed': return 'Saved to History, but not copied';
    case 'error': return state.message || 'Something went wrong';
    default: return '';
  }
}

/**
 * Apply one event. Returns the next state and the action the page should take, if any:
 * `close`, `edit` or `discard`. A closed card ignores everything.
 *
 * Events: `{ type: 'hover' }`, `{ type: 'leave' }`, `{ type: 'tick', elapsed }`,
 * `{ type: 'edit' }`, `{ type: 'discard' }`, `{ type: 'dismiss' }`,
 * `{ type: 'failed', message }` (an action did not go through), and
 * `{ type: 'key', key, metaKey, ctrlKey }`.
 */
export function step(state, event) {
  if (state.closed) return { state, action: null };
  switch (event.type) {
    case 'hover':
      return { state: { ...state, hovering: true }, action: null };
    case 'leave':
      return {
        state: { ...state, hovering: false, remaining: Math.max(state.remaining, LINGER_MS) },
        action: null,
      };
    case 'tick': {
      if (state.hovering || state.busy) return { state, action: null };
      const remaining = state.remaining - Math.max(0, event.elapsed || 0);
      if (remaining <= 0) return close(state);
      return { state: { ...state, remaining }, action: null };
    }
    case 'edit':
    case 'discard':
      if (state.busy) return { state, action: null };
      return { state: { ...state, busy: true }, action: event.type };
    case 'dismiss':
      return close(state);
    case 'failed':
      return {
        state: {
          ...state,
          busy: false,
          status: 'error',
          message: event.message || '',
          remaining: Math.max(state.remaining, DISMISS_AFTER_MS),
        },
        action: null,
      };
    case 'key':
      return step(state, keyEvent(event));
    default:
      return { state, action: null };
  }
}

function close(state) {
  return { state: { ...state, closed: true }, action: 'close' };
}

/** Keys work once the card has been clicked: Escape closes, Return edits, ⌘⌫ discards. */
function keyEvent({ key, metaKey = false, ctrlKey = false }) {
  if (key === 'Escape') return { type: 'dismiss' };
  if (key === 'Enter') return { type: 'edit' };
  if ((key === 'Backspace' || key === 'Delete') && (metaKey || ctrlKey)) return { type: 'discard' };
  return { type: 'none' };
}
