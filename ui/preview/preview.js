// preview/preview.js — the card shown after a capture: thumbnail, "Copied", Edit, Discard.
// The capture is already saved and on the clipboard when this page loads; everything here is
// about what to do next. The behaviour lives in state.mjs; this file only wires it to the DOM.

import { icon } from '../shared/icons.js';
import { initialPreview, statusText, step } from './state.mjs';

const { invoke, convertFileSrc } = window.__TAURI__.core;

const params = new URLSearchParams(location.search);
let state = initialPreview({ id: params.get('id') || '', error: params.get('error') || '' });

const $ = (id) => document.getElementById(id);
$('edit').innerHTML = `${icon('pencil')} Edit`;
$('discard').innerHTML = `${icon('trash')} Discard`;
$('close').innerHTML = icon('x', 12);

function paint() {
  const status = $('status');
  const ok = state.status === 'copied';
  status.className = ok ? 'status' : 'status warn';
  status.innerHTML = `${icon(ok ? 'check' : 'x', 13)} <span></span>`;
  status.querySelector('span').textContent = statusText(state);
  status.title = state.status === 'copy-failed' ? state.message : '';
  $('edit').disabled = state.busy;
  $('discard').disabled = state.busy;
}

async function dispatch(event) {
  const out = step(state, event);
  state = out.state;
  paint();
  try {
    if (out.action === 'close') await invoke('preview_close');
    else if (out.action === 'edit') await invoke('preview_edit', { id: state.id });
    else if (out.action === 'discard') await invoke('preview_discard', { id: state.id });
  } catch (e) {
    dispatch({ type: 'failed', message: String(e) });
  }
}

async function loadThumb() {
  try {
    const meta = await invoke('get_capture', { id: state.id });
    const path = meta?.thumbPath || meta?.fullPath;
    if (!path) throw new Error('This capture is no longer available.');
    $('image').src = convertFileSrc(path);
  } catch (e) {
    dispatch({ type: 'failed', message: e?.message || String(e) });
  }
}

$('edit').addEventListener('click', () => dispatch({ type: 'edit' }));
$('thumb').addEventListener('click', () => dispatch({ type: 'edit' }));
$('discard').addEventListener('click', () => dispatch({ type: 'discard' }));
$('close').addEventListener('click', () => dispatch({ type: 'dismiss' }));
$('card').addEventListener('mouseenter', () => dispatch({ type: 'hover' }));
$('card').addEventListener('mouseleave', () => dispatch({ type: 'leave' }));
addEventListener('keydown', (e) => {
  dispatch({ type: 'key', key: e.key, metaKey: e.metaKey, ctrlKey: e.ctrlKey });
});

const TICK_MS = 250;
let last = performance.now();
setInterval(() => {
  const now = performance.now();
  dispatch({ type: 'tick', elapsed: now - last });
  last = now;
}, TICK_MS);

paint();
loadThumb();
