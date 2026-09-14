import assert from 'node:assert/strict';
import test from 'node:test';

import { deliverCapture, deliveryReport } from './delivery.mjs';

test('reports completion only after history and clipboard both succeed', async () => {
  const calls = [];
  const outcome = await deliverCapture({
    saveToHistory: async () => { calls.push('history'); return 'capture-1'; },
    copyToClipboard: async () => { calls.push('clipboard'); },
  });

  assert.deepEqual(calls, ['history', 'clipboard']);
  assert.deepEqual(deliveryReport(outcome), { historyId: 'capture-1', error: null });
});

test('does not settle while the clipboard outcome is still pending', async () => {
  let releaseClipboard;
  const clipboard = new Promise((resolve) => { releaseClipboard = resolve; });
  let settled = false;
  const delivery = deliverCapture({
    saveToHistory: async () => 'capture-pending',
    copyToClipboard: () => clipboard,
  }).then((outcome) => { settled = true; return outcome; });

  await Promise.resolve();
  await Promise.resolve();
  assert.equal(settled, false);

  releaseClipboard();
  assert.equal((await delivery).clipboard.status, 'copied');
  assert.equal(settled, true);
});

test('retains the history recovery target when clipboard copy fails', async () => {
  const outcome = await deliverCapture({
    saveToHistory: async () => 'capture-2',
    copyToClipboard: async () => { throw new Error('pasteboard unavailable'); },
  });

  assert.deepEqual(deliveryReport(outcome), {
    historyId: 'capture-2',
    error: 'Clipboard copy failed: pasteboard unavailable',
  });
});

test('still attempts clipboard delivery when history persistence fails', async () => {
  let copied = false;
  const outcome = await deliverCapture({
    saveToHistory: async () => { throw new Error('disk full'); },
    copyToClipboard: async () => { copied = true; },
  });

  assert.equal(copied, true);
  assert.deepEqual(deliveryReport(outcome), {
    historyId: null,
    error: 'Capture history failed: disk full',
  });
});

test('reports both independent delivery failures', async () => {
  const outcome = await deliverCapture({
    saveToHistory: async () => { throw new Error('disk full'); },
    copyToClipboard: async () => { throw new Error('pasteboard unavailable'); },
  });

  assert.deepEqual(deliveryReport(outcome), {
    historyId: null,
    error: 'Capture history failed: disk full\nClipboard copy failed: pasteboard unavailable',
  });
});
