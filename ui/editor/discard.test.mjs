import assert from 'node:assert/strict';
import test from 'node:test';

import { discardCapture } from './discard.mjs';

test('deletes the saved capture before closing the native editor window', async () => {
  const commands = [];
  await discardCapture('capture-1', async (command, payload) => {
    commands.push([command, payload]);
  });

  assert.deepEqual(commands, [
    ['delete_capture', { id: 'capture-1' }],
    ['close_editor', undefined],
  ]);
});

test('closes an unsaved capture without issuing a delete', async () => {
  const commands = [];
  await discardCapture(null, async (command, payload) => {
    commands.push([command, payload]);
  });

  assert.deepEqual(commands, [['close_editor', undefined]]);
});
