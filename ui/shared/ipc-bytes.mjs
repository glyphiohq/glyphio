// shared/ipc-bytes.mjs — send image bytes to a Rust command without base64.
//
// The bytes go as the raw invoke body; every other argument goes as one JSON object in the
// `x-glyphio-args` header, percent-encoded so any text survives (src-tauri/src/ipc_bytes.rs).

export const ARGS_HEADER = 'x-glyphio-args';

/** The invoke options carrying `args`. */
export function argsHeader(args = {}) {
  return { headers: { [ARGS_HEADER]: encodeURIComponent(JSON.stringify(args)) } };
}

/** The bytes of a Blob, ArrayBuffer or typed array, as a Uint8Array. */
export async function toBytes(data) {
  if (data == null) return new Uint8Array(0);
  if (data instanceof Uint8Array) return data;
  if (data instanceof ArrayBuffer) return new Uint8Array(data);
  if (ArrayBuffer.isView(data)) return new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
  if (typeof data.arrayBuffer === 'function') return new Uint8Array(await data.arrayBuffer());
  throw new TypeError('expected a Blob, ArrayBuffer or typed array');
}

/** Call `cmd` with `data` as the body and `args` in the header. */
export async function invokeBytes(invoke, cmd, data, args = {}) {
  return invoke(cmd, await toBytes(data), argsHeader(args));
}
