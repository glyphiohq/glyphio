// popup/popup.js — the popup-snippet surface (cheatsheets). Shows the snippet body in an
// always-on-top window; nothing is pasted. The engine already resolved the expansion to an
// empty string before this window opened, so closing it has no effect on the typed text.

import { escapeHtml, mdToHtml, sanitizeSnippetHtml } from '../shared/markdown.js';

const { invoke } = window.__TAURI__.core;

const bodyEl = document.getElementById('body');
const triggerEl = document.getElementById('trigger');

init().catch((err) => {
  bodyEl.innerHTML = `<span class="popup-empty">${escapeHtml(err.message || String(err))}</span>`;
});

async function init() {
  const payload = await invoke('take_pending_payload', { label: 'popup' });
  if (!payload?.snippet) {
    bodyEl.innerHTML = '<span class="popup-empty">Nothing to show — trigger a popup snippet.</span>';
    return;
  }
  const s = payload.snippet;
  triggerEl.textContent = s.trigger || '';
  document.title = `Glyphio — ${s.trigger || 'popup'}`;

  // Sanitized: popup bodies can arrive from teammates via sync, and this webview holds IPC.
  if (s.format === 'html') bodyEl.innerHTML = sanitizeSnippetHtml(s.replacement || '');
  else if (s.format === 'markdown') bodyEl.innerHTML = mdToHtml(s.replacement || '');
  else bodyEl.textContent = s.replacement || '';
}

function close() {
  window.__TAURI__.window.getCurrentWindow().close();
}

document.getElementById('close').addEventListener('click', close);
document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') { e.preventDefault(); close(); }
});
