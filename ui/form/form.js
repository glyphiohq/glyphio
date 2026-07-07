// form/form.js — the form-snippet surface. Collects the fields, fills the body template
// ({{name}} placeholders), and hands the result back to the engine (which is blocked on
// the bridge until we answer). Cancelling (Esc / Cancel / closing the window) aborts the
// expansion cleanly.

import { escapeHtml } from '../shared/markdown.js';

const { invoke } = window.__TAURI__.core;

const fieldsEl = document.getElementById('fields');
const triggerEl = document.getElementById('trigger');
const errorEl = document.getElementById('error');

let requestId = '';
let snippet = null;
let fields = [];
let resolved = false; // submit/cancel sent — don't double-send on unload

init().catch((err) => {
  errorEl.textContent = err.message || String(err);
});

async function init() {
  const payload = await invoke('take_pending_payload', { label: 'form' });
  if (!payload?.snippet) {
    errorEl.textContent = 'Nothing to fill — trigger a form snippet.';
    return;
  }
  requestId = payload.requestId;
  snippet = payload.snippet;
  triggerEl.textContent = snippet.trigger || '';
  document.title = `Glyphio — ${snippet.trigger || 'form'}`;

  fields = fieldSpec(snippet);
  if (fields.length === 0) {
    errorEl.textContent = 'This form snippet has no fields — add some in the snippet editor.';
    return;
  }
  renderFields();
  fieldsEl.querySelector('input, textarea, select')?.focus();
}

// Field list: the editor's form builder stores `{"fields":[...]}` in `variables`; snippets
// without one fall back to a text input per unique {{placeholder}} in the body.
function fieldSpec(s) {
  const declared = s.variables?.fields;
  if (Array.isArray(declared) && declared.length) {
    return declared.filter((f) => f && typeof f.name === 'string' && f.name);
  }
  const seen = new Set();
  const out = [];
  for (const m of String(s.replacement || '').matchAll(/\{\{\s*([\w.-]+)\s*\}\}/g)) {
    if (seen.has(m[1])) continue;
    seen.add(m[1]);
    out.push({ name: m[1], type: 'text' });
  }
  return out;
}

function renderFields() {
  fieldsEl.textContent = '';
  for (const f of fields) {
    const wrap = document.createElement('div');
    wrap.className = 'ffield';
    const label = document.createElement('label');
    label.textContent = f.label || f.name;
    wrap.appendChild(label);

    let input;
    if (f.type === 'multiline') {
      input = document.createElement('textarea');
    } else if (f.type === 'select' && Array.isArray(f.options) && f.options.length) {
      input = document.createElement('select');
      for (const opt of f.options) {
        const o = document.createElement('option');
        o.value = o.textContent = String(opt);
        input.appendChild(o);
      }
    } else {
      input = document.createElement('input');
      input.type = 'text';
    }
    input.dataset.name = f.name;
    if (f.placeholder && 'placeholder' in input) input.placeholder = f.placeholder;
    if (f.default != null) input.value = String(f.default);
    wrap.appendChild(input);
    fieldsEl.appendChild(wrap);
  }
}

function collectValues() {
  const values = {};
  for (const input of fieldsEl.querySelectorAll('[data-name]')) {
    values[input.dataset.name] = input.value;
  }
  return values;
}

// Fill {{name}} placeholders. For html-format snippets the values are user-typed plain
// text and must be escaped into the markup (newlines become <br>).
function fillTemplate(body, format, values) {
  return String(body).replace(/\{\{\s*([\w.-]+)\s*\}\}/g, (whole, name) => {
    if (!(name in values)) return whole;
    const v = values[name];
    return format === 'html' ? escapeHtml(v).replace(/\n/g, '<br>') : v;
  });
}

async function submit() {
  if (resolved || !snippet) return;
  resolved = true;
  const text = fillTemplate(snippet.replacement || '', snippet.format, collectValues());
  try {
    await invoke('form_submit', { requestId, text });
  } catch (err) {
    resolved = false;
    errorEl.textContent = err.message || String(err);
  }
}

function cancel() {
  if (resolved) return;
  resolved = true;
  invoke('form_cancel', { requestId }).catch(() => {});
}

document.getElementById('submit').addEventListener('click', submit);
document.getElementById('cancel').addEventListener('click', cancel);
document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') { e.preventDefault(); cancel(); }
  else if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') { e.preventDefault(); submit(); }
});
// Closing the window any other way must not leave the engine hanging until timeout.
window.addEventListener('beforeunload', cancel);
