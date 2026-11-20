import assert from 'node:assert/strict';
import test from 'node:test';

import { clipKeyIntent, confirmationText } from './clip-keys.mjs';

const idle = { confirming: false, hasClips: true };
const asking = { confirming: true, hasClips: true };
const intent = (e, ctx) => clipKeyIntent(e, ctx).type;

test('command-shift-backspace asks, on macOS and with ctrl on Windows', () => {
  assert.equal(intent({ key: 'Backspace', metaKey: true, shiftKey: true }, idle), 'clear-start');
  assert.equal(intent({ key: 'Backspace', ctrlKey: true, shiftKey: true }, idle), 'clear-start');
});

test('plain command-backspace is still forget-one, not clear', () => {
  assert.equal(intent({ key: 'Backspace', metaKey: true }, idle), 'none');
});

test('there is nothing to ask about when the list is empty', () => {
  assert.equal(intent({ key: 'Backspace', metaKey: true, shiftKey: true },
    { confirming: false, hasClips: false }), 'none');
});

test('enter clears what is not pinned; alt-enter clears everything', () => {
  assert.equal(intent({ key: 'Enter' }, asking), 'clear-confirm');
  assert.equal(intent({ key: 'Enter', altKey: true }, asking), 'clear-confirm-pinned');
});

test('escape and any unrelated key cancel', () => {
  assert.equal(intent({ key: 'Escape' }, asking), 'clear-cancel');
  assert.equal(intent({ key: 'a' }, asking), 'clear-cancel');
  assert.equal(intent({ key: 'ArrowDown' }, asking), 'clear-cancel');
});

test('pressing alt on the way to alt-enter does not cancel', () => {
  assert.equal(intent({ key: 'Alt', altKey: true }, asking), 'none');
});

test('e toggles emptying the current clipboard', () => {
  assert.equal(intent({ key: 'e' }, asking), 'clear-toggle-empty');
});

test('the shortcut itself does nothing while already asking beyond cancelling', () => {
  assert.equal(intent({ key: 'Backspace', metaKey: true, shiftKey: true }, asking), 'clear-cancel');
});

test('the question states the total and what is kept', () => {
  assert.equal(confirmationText(37, 3), 'Clear 37 clipboard entries?  3 pinned entries are kept.');
  assert.equal(confirmationText(1, 1), 'Clear 1 clipboard entry?  1 pinned entry is kept.');
  assert.equal(confirmationText(5, 0), 'Clear 5 clipboard entries?');
});
