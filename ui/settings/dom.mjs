// settings/dom.mjs — DOM helpers shared by the Settings window and its lazily loaded views:
// the status line, modals and popovers. No Tauri calls, so views can import it freely.

import { escapeHtml, escapeAttr } from '../shared/markdown.js';

// --- Reusable modal + popover primitives ------------------------------------

export function openModal(innerHtml, { className = '', onClose } = {}) {
  const backdrop = el('div', { className: 'modal-backdrop' });
  backdrop.innerHTML = `<div class="modal ${className}">${innerHtml}</div>`;
  document.body.appendChild(backdrop);
  const modal = backdrop.querySelector('.modal');
  let closed = false;
  const close = () => {
    if (closed) return; closed = true;
    backdrop.remove();
    onClose?.();
  };
  modal.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') { e.preventDefault(); close(); }
    else if (e.key === 'Tab') trapFocus(modal, e);
  });
  backdrop.addEventListener('mousedown', (e) => { if (e.target === backdrop) close(); });
  return { backdrop, modal, close, $: (s) => modal.querySelector(s) };
}

function trapFocus(container, e) {
  const f = [...container.querySelectorAll('button, [href], input, select, textarea, [contenteditable="true"], [tabindex]:not([tabindex="-1"])')]
    .filter((n) => !n.disabled && n.offsetParent !== null);
  if (!f.length) return;
  const first = f[0], last = f[f.length - 1];
  if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last.focus(); }
  else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first.focus(); }
}

export function confirmDialog(message, { confirmLabel = 'Confirm', danger = false } = {}) {
  return new Promise((resolve) => {
    const { modal, close } = openModal(`
      <div class="confirm-body"></div>
      <div class="modal-actions">
        <div class="spacer"></div>
        <button class="secondary" data-no>Cancel</button>
        <button class="${danger ? 'danger' : 'primary'}" data-yes>${escapeHtml(confirmLabel)}</button>
      </div>`, { className: 'small', onClose: () => resolve(false) });
    modal.querySelector('.confirm-body').textContent = message;
    modal.querySelector('[data-no]').addEventListener('click', close);
    modal.querySelector('[data-yes]').addEventListener('click', () => { resolve(true); close(); });
    modal.querySelector('[data-yes]').focus();
  });
}

export function promptDialog(title, { label = '', value = '', placeholder = '', confirmLabel = 'Save', allowEmpty = false } = {}) {
  return new Promise((resolve) => {
    const { modal, close } = openModal(`
      <h3>${escapeHtml(title)}</h3>
      <div class="mfield">
        ${label ? `<label>${escapeHtml(label)}</label>` : ''}
        <input type="text" data-input placeholder="${escapeAttr(placeholder)}" />
        <p class="field-error" data-err></p>
      </div>
      <div class="modal-actions">
        <div class="spacer"></div>
        <button class="secondary" data-no>Cancel</button>
        <button class="primary" data-yes>${escapeHtml(confirmLabel)}</button>
      </div>`, { className: 'small', onClose: () => resolve(null) });
    const input = modal.querySelector('[data-input]');
    const errEl = modal.querySelector('[data-err]');
    input.value = value;
    const submit = () => {
      const v = input.value.trim();
      if (!v && !allowEmpty) { errEl.textContent = 'Required.'; input.focus(); return; }
      resolve(v); close();
    };
    modal.querySelector('[data-no]').addEventListener('click', close);
    modal.querySelector('[data-yes]').addEventListener('click', submit);
    input.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); submit(); } });
    input.focus(); input.select();
  });
}

// A small floating panel anchored under a toolbar button (link URL / table size).
export function openPopover(anchor, innerHtml, onMount) {
  document.querySelectorAll('.gp-popover').forEach((p) => p.remove());
  const pop = el('div', { className: 'gp-popover' });
  pop.innerHTML = innerHtml;
  document.body.appendChild(pop);
  const rect = anchor.getBoundingClientRect();
  pop.style.top = `${rect.bottom + 6}px`;
  pop.style.left = `${Math.max(8, Math.min(rect.left, window.innerWidth - pop.offsetWidth - 12))}px`;
  let closed = false;
  const close = () => {
    if (closed) return; closed = true;
    pop.remove();
    document.removeEventListener('mousedown', outside, true);
    document.removeEventListener('keydown', onKey, true);
  };
  const outside = (e) => { if (!pop.contains(e.target) && e.target !== anchor) close(); };
  const onKey = (e) => { if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); close(); } };
  setTimeout(() => {
    document.addEventListener('mousedown', outside, true);
    document.addEventListener('keydown', onKey, true);
  }, 0);
  onMount({ root: pop, close });
}

// --- utils ------------------------------------------------------------------

export function el(tag, props = {}) { return Object.assign(document.createElement(tag), props); }

export function setStatus(text, kind = '') {
  const el = document.getElementById('status');
  if (!el) return;
  el.textContent = text; el.className = `status-line ${kind}`;
  if (kind === 'ok') setTimeout(() => { if (el.textContent === text) el.textContent = ''; }, 3000);
}
