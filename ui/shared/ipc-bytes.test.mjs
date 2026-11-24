import assert from 'node:assert/strict';
import test from 'node:test';

import { ARGS_HEADER, argsHeader, invokeBytes, toBytes } from './ipc-bytes.mjs';

test('arguments travel as percent-encoded JSON that Rust decodes byte for byte', () => {
  const h = argsHeader({ id: 'a', note: 'café — 東京 100%' })['headers'][ARGS_HEADER];
  // The same string the Rust test decodes.
  assert.equal(h, '%7B%22id%22%3A%22a%22%2C%22note%22%3A%22caf%C3%A9%20%E2%80%94%20%E6%9D%B1%E4%BA%AC%20100%25%22%7D');
  assert.match(h, /^[\x21-\x7e]+$/, 'header values must be visible ASCII');
});

test('blobs, buffers and views all become bytes', async () => {
  assert.deepEqual([...await toBytes(new Blob([new Uint8Array([1, 2])]))], [1, 2]);
  assert.deepEqual([...await toBytes(new Uint8Array([3]).buffer)], [3]);
  assert.deepEqual([...await toBytes(new DataView(new Uint8Array([9, 8, 7]).buffer, 1, 2))], [8, 7]);
  assert.equal((await toBytes(null)).length, 0);
  await assert.rejects(() => toBytes('nope'));
});

test('invokeBytes sends the bytes as the body and the rest in the header', async () => {
  const calls = [];
  const invoke = async (...a) => { calls.push(a); return 'ok'; };
  assert.equal(await invokeBytes(invoke, 'save_file', new Uint8Array([5]), { path: '/tmp/x.png' }), 'ok');
  const [cmd, body, opts] = calls[0];
  assert.equal(cmd, 'save_file');
  assert.deepEqual([...body], [5]);
  assert.equal(decodeURIComponent(opts.headers[ARGS_HEADER]), '{"path":"/tmp/x.png"}');
});
