// palette/palette.js — Spotlight-style command palette.
// Summoned by a global hotkey (or the tray). Searches enabled snippets AND the capture
// actions in one list. Enter on a snippet hands its trigger to the engine worker, which
// expands it into the previously focused app exactly as if typed (variables, forms, popups
// and command snippets all take their normal path); ⌘Enter copies the body. Enter on a
// capture action runs that screenshot mode.

import { icon } from '../shared/icons.js';

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const input = document.getElementById('q');
const list = document.getElementById('list');

// Capture actions, always available and searchable by their keywords.
const CAPTURES = [
  { mode: 'visible', label: 'Capture Visible Area', keys: 'capture screenshot visible area screen shot' },
  { mode: 'snip', label: 'Capture Region (Snip)', keys: 'capture screenshot region snip selection crop area' },
  { mode: 'fullWindow', label: 'Capture Full Window', keys: 'capture screenshot full window picker choose' },
  { mode: 'frontWindow', label: 'Capture Frontmost Window', keys: 'capture screenshot frontmost front active window' },
  { mode: 'scrolling', label: 'Capture Scrolling Area', keys: 'capture screenshot scrolling scroll area long stitch' },
  { mode: 'scrollingPage', label: 'Capture Scrolling Page', keys: 'capture screenshot scrolling scroll page full long stitch' },
].map((c) => ({ type: 'capture', ...c }));

let snippets = [];   // enabled snippets (type:'snippet'), enriched with their group name
let filtered = [];
let selected = 0;

init();

async function init() {
  await refresh();
  await listen('palette-show', async () => {
    input.value = '';
    await refresh();
    input.focus();
    input.select();
  });
  // Losing focus dismisses the palette, Spotlight-style.
  window.addEventListener('blur', () => invoke('palette_hide'));
  input.addEventListener('input', () => { selected = 0; draw(); });
  input.addEventListener('keydown', onKey);
  input.focus();
}

async function refresh() {
  try {
    const [snips, groups] = await Promise.all([invoke('list_snippets'), invoke('list_groups')]);
    const groupName = new Map(groups.map((g) => [g.id, g.name]));
    snippets = snips
      .filter((s) => s.enabled !== false)
      .map((s) => ({ type: 'snippet', ...s, group: s.groupId ? groupName.get(s.groupId) || '' : '' }));
  } catch {
    snippets = [];
  }
  selected = 0;
  draw();
}

// Lightweight fuzzy match: every query token must appear (substring) in the item's search
// corpus. Scored so a trigger/label prefix beats a substring beats a body/keyword match.
function score(item, tokens) {
  const primary = (item.type === 'snippet' ? item.trigger : item.label).toLowerCase();
  const corpus = item.type === 'snippet'
    ? `${item.replacement || ''} ${item.group || ''} ${item.kind || 'text'}`.toLowerCase()
    : item.keys;
  let total = 0;
  for (const t of tokens) {
    let best = -1;
    if (primary.startsWith(t)) best = 100;
    else if (primary.includes(t)) best = 60;
    else if (corpus.includes(t)) best = 15;
    if (best < 0) return -1;
    total += best;
  }
  return total;
}

function previewText(s) {
  let t = s.replacement || '';
  if (s.format === 'html') t = t.replace(/<[^>]+>/g, ' ');
  return t.replace(/\s+/g, ' ').trim().slice(0, 120);
}

function draw() {
  const tokens = input.value.trim().toLowerCase().split(/\s+/).filter(Boolean);
  const pool = [...snippets, ...CAPTURES];
  if (tokens.length) {
    filtered = pool
      .map((it) => [score(it, tokens), it])
      .filter(([sc]) => sc >= 0)
      .sort((a, b) => b[0] - a[0])
      .map(([, it]) => it);
  } else {
    // Empty query: snippets first, capture actions after.
    filtered = pool;
  }
  if (selected >= filtered.length) selected = Math.max(0, filtered.length - 1);

  if (!filtered.length) {
    list.innerHTML = `<li class="pal-empty">${snippets.length ? 'No matches' : 'No snippets yet — create one in Glyphio'}</li>`;
    return;
  }
  list.innerHTML = '';
  filtered.forEach((it, i) => {
    list.appendChild(it.type === 'capture' ? captureRow(it, i) : snippetRow(it, i));
  });
  markActive();
}

function snippetRow(s, i) {
  const li = row(i);
  const trig = document.createElement('span');
  trig.className = 'pal-trigger';
  trig.textContent = s.trigger;
  const prev = document.createElement('span');
  prev.className = 'pal-preview';
  prev.textContent = previewText(s);
  li.append(trig, prev);
  if (s.kind && s.kind !== 'text') li.append(tag(s.kind));
  if (s.group) li.append(tag(s.group));
  li.addEventListener('click', () => exec(s));
  return li;
}

function captureRow(c, i) {
  const li = row(i);
  li.classList.add('pal-action');
  const ico = document.createElement('span');
  ico.className = 'pal-ico';
  ico.innerHTML = icon('camera', 15);
  const label = document.createElement('span');
  label.className = 'pal-preview pal-action-label';
  label.textContent = c.label;
  li.append(ico, label, tag('capture'));
  li.addEventListener('click', () => runCapture(c));
  return li;
}

function row(i) {
  const li = document.createElement('li');
  li.className = 'pal-row' + (i === selected ? ' active' : '');
  li.addEventListener('mousemove', () => { if (selected !== i) { selected = i; markActive(); } });
  return li;
}

function tag(text) {
  const el = document.createElement('span');
  el.className = 'pal-tag';
  el.textContent = text;
  return el;
}

function markActive() {
  [...list.children].forEach((el, i) => el.classList.toggle('active', i === selected));
  list.children[selected]?.scrollIntoView({ block: 'nearest' });
}

function activate(item, copy) {
  if (!item) return;
  if (item.type === 'capture') runCapture(item);
  else if (copy) copyBody(item);
  else exec(item);
}

function onKey(e) {
  if (e.key === 'ArrowDown') { e.preventDefault(); move(1); }
  else if (e.key === 'ArrowUp') { e.preventDefault(); move(-1); }
  else if (e.key === 'Enter') {
    e.preventDefault();
    activate(filtered[selected], e.metaKey || e.ctrlKey);
  } else if (e.key === 'Escape') {
    e.preventDefault();
    invoke('palette_hide');
  }
}

function move(d) {
  if (!filtered.length) return;
  selected = (selected + d + filtered.length) % filtered.length;
  markActive();
}

/// Expand into the previously focused app via the engine (the backend hides this
/// window first so focus lands back where the user was typing).
async function exec(s) {
  try {
    await invoke('palette_exec', { trigger: s.trigger });
  } catch (err) {
    console.warn('palette exec failed:', err);
  }
}

async function runCapture(c) {
  try {
    await invoke('palette_capture', { mode: c.mode });
  } catch (err) {
    console.warn('palette capture failed:', err);
  }
}

async function copyBody(s) {
  try {
    await navigator.clipboard.writeText(s.replacement || '');
  } finally {
    invoke('palette_hide');
  }
}
