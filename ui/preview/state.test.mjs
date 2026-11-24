import assert from 'node:assert/strict';
import test from 'node:test';

import { DISMISS_AFTER_MS, LINGER_MS, initialPreview, statusText, step } from './state.mjs';

const run = (state, ...events) => {
  const actions = [];
  for (const e of events) {
    const out = step(state, e);
    state = out.state;
    if (out.action) actions.push(out.action);
  }
  return { state, actions };
};

test('a copied capture says so, and a failed copy says what still happened', () => {
  assert.equal(statusText(initialPreview({ id: 'a' })), 'Copied to clipboard');
  const failed = initialPreview({ id: 'a', error: 'Clipboard copy failed: busy' });
  assert.equal(failed.status, 'copy-failed');
  assert.equal(statusText(failed), 'Saved to History, but not copied');
});

test('an untouched card closes itself once its time is up', () => {
  const { actions } = run(initialPreview({ id: 'a' }),
    { type: 'tick', elapsed: DISMISS_AFTER_MS - 1 });
  assert.deepEqual(actions, []);
  const done = run(initialPreview({ id: 'a' }),
    { type: 'tick', elapsed: DISMISS_AFTER_MS - 1 }, { type: 'tick', elapsed: 1 });
  assert.deepEqual(done.actions, ['close']);
  assert.ok(done.state.closed);
});

test('the card waits while the pointer is on it, then lingers after it leaves', () => {
  const { state, actions } = run(initialPreview({ id: 'a' }),
    { type: 'tick', elapsed: DISMISS_AFTER_MS - 100 },
    { type: 'hover' },
    { type: 'tick', elapsed: 60_000 });
  assert.deepEqual(actions, []);
  const left = run(state, { type: 'leave' }, { type: 'tick', elapsed: LINGER_MS - 1 });
  assert.deepEqual(left.actions, []);
  assert.deepEqual(run(left.state, { type: 'tick', elapsed: 1 }).actions, ['close']);
});

test('edit and discard each ask once, and the card does not time out meanwhile', () => {
  const { state, actions } = run(initialPreview({ id: 'a' }),
    { type: 'edit' }, { type: 'edit' }, { type: 'discard' },
    { type: 'tick', elapsed: DISMISS_AFTER_MS * 2 });
  assert.deepEqual(actions, ['edit']);
  assert.ok(state.busy && !state.closed);
});

test('an action that fails is shown, and the buttons work again', () => {
  const { state } = run(initialPreview({ id: 'a' }),
    { type: 'discard' }, { type: 'failed', message: 'This capture is gone.' });
  assert.equal(statusText(state), 'This capture is gone.');
  assert.equal(state.busy, false);
  assert.deepEqual(run(state, { type: 'discard' }).actions, ['discard']);
});

test('keys: Escape closes, Return edits, command-backspace discards, others do nothing', () => {
  const fresh = () => initialPreview({ id: 'a' });
  assert.deepEqual(run(fresh(), { type: 'key', key: 'Escape' }).actions, ['close']);
  assert.deepEqual(run(fresh(), { type: 'key', key: 'Enter' }).actions, ['edit']);
  assert.deepEqual(run(fresh(), { type: 'key', key: 'Backspace', metaKey: true }).actions, ['discard']);
  assert.deepEqual(run(fresh(), { type: 'key', key: 'Backspace' }).actions, []);
  assert.deepEqual(run(fresh(), { type: 'key', key: 'x' }).actions, []);
});

test('a closed card ignores everything', () => {
  const { state } = run(initialPreview({ id: 'a' }), { type: 'dismiss' });
  assert.deepEqual(run(state, { type: 'edit' }, { type: 'discard' }).actions, []);
});
