// palette/palette.js — one summoned window, three lists.
//
// ⌥Space opens it; it lands on the clipboard, which is what people reach for most often. The
// other two lists are a keystroke away (Tab, or ⌘1/⌘2/⌘3) and each one keeps its own idea of
// what Enter means:
//
//   clipboard  ↩ paste it where you were · ⌘↩ load the clipboard only · ⌘P pin · ⌘⌫ forget
//   capture    ↩ take the shot into the editor · ⌘↩ straight to the clipboard
//   snippets   ↩ expand into the app you came from · ⌘↩ copy the body
//
// Everything that leaves this window goes through a Rust command that hides the palette and
// steps the app aside *before* acting — a paste or an expansion aimed at the previous app
// arrives nowhere if Glyphio still has focus.

import { icon } from '../shared/icons.js';

const { invoke, convertFileSrc } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const input = document.getElementById('q');
const list = document.getElementById('list');
const foot = document.getElementById('foot');
const glyph = document.getElementById('glyph');
const viewBar = document.getElementById('views');
const kindBar = document.getElementById('kinds');

const VIEWS = ['clipboard', 'captures', 'snippets'];
const KINDS = ['all', 'text', 'image'];
const LABELS = { clipboard: 'Clipboard', captures: 'Capture', snippets: 'Snippets' };
const labelOf = (v) => LABELS[v];

/** Per-view chrome: the leading glyph, the search placeholder, and the footer legend. */
const CHROME = {
  clipboard: {
    glyph: '⎘',
    placeholder: 'Search what you’ve copied…',
    keys: [['↩', 'paste'], ['⌘↩', 'copy only'], ['⌘P', 'pin'], ['⌘⌫', 'forget']],
  },
  captures: {
    glyph: '⛶',
    placeholder: 'Search capture modes…',
    keys: [['↩', 'capture into the editor'], ['⌘↩', 'straight to the clipboard']],
  },
  snippets: {
    glyph: '⌁',
    placeholder: 'Search snippets…',
    keys: [['↩', 'expand into the active app'], ['⌘↩', 'copy the body']],
  },
};

const CAPTURES = [
  { mode: 'visible', label: 'Visible Area', keys: 'visible area screen current display' },
  { mode: 'snip', label: 'Region (Snip)', keys: 'region snip selection crop area rectangle' },
  { mode: 'fullWindow', label: 'Full Window (picker)', keys: 'full window picker choose pick' },
  { mode: 'frontWindow', label: 'Frontmost Window', keys: 'frontmost front active window' },
  { mode: 'pageOnly', label: 'Browser Page', keys: 'browser page content only web chrome safari' },
  { mode: 'scrolling', label: 'Scrolling Area', keys: 'scrolling scroll area long stitch panel' },
  { mode: 'scrollingPage', label: 'Scrolling Page', keys: 'scrolling scroll page whole long stitch' },
];

let view = 'clipboard';
let kind = 'all';
let clips = [];
let snippets = [];
let rows = [];
let selected = 0;
let error = '';

init();

async function init() {
  const asked = await invoke('palette_view').catch(() => '');
  await refresh();
  setKind('all');
  setView(VIEWS.includes(asked) ? asked : 'clipboard', { keepQuery: false });

  // Draw what we already have *before* asking for anything. The window is on screen by the
  // time this fires, so an await here is a visibly empty palette; the lists are already in
  // memory from last time and are almost always still right.
  await listen('palette-show', (e) => {
    const asked = typeof e.payload === 'string' ? e.payload : null;
    input.value = '';
    setView(VIEWS.includes(asked) ? asked : view, { keepQuery: false });
    input.focus();
    input.select();
    refresh().then(draw);
  });
  // Something copied while the palette is open should appear in it.
  await listen('clipboard-changed', async () => {
    if (!document.hasFocus()) return;
    clips = await invoke('list_clips').catch(() => clips);
    draw();
  });
  await listen('snippets-changed', async () => {
    if (!document.hasFocus()) return;
    await loadSnippets();
    draw();
  });

  window.addEventListener('blur', hide);
  input.addEventListener('input', () => { selected = 0; draw(); });
  input.addEventListener('keydown', onKey);
  viewBar.addEventListener('click', (e) => {
    const b = e.target.closest('[data-view]');
    if (b) setView(b.dataset.view);
  });
  kindBar.addEventListener('click', (e) => {
    const b = e.target.closest('[data-kind]');
    if (b) setKind(b.dataset.kind);
  });
  input.focus();
}

function hide() { invoke('palette_hide').catch(() => {}); }

/// Reload both lists. Never clears what is already on screen on failure — a transient error
/// should not blank a palette the user is looking at.
async function refresh() {
  error = '';
  await Promise.all([
    invoke('list_clips').then((c) => { clips = c; }).catch((e) => { error = String(e); }),
    loadSnippets(),
  ]);
}

async function loadSnippets() {
  try {
    const [snips, groups] = await Promise.all([invoke('list_snippets'), invoke('list_groups')]);
    const groupName = new Map(groups.map((g) => [g.id, g.name]));
    snippets = snips
      .filter((s) => s.enabled !== false)
      .map((s) => ({ ...s, group: s.groupId ? groupName.get(s.groupId) || '' : '' }));
  } catch (e) {
    error = String(e);
  }
}

function setView(next, { keepQuery = true } = {}) {
  view = VIEWS.includes(next) ? next : 'clipboard';
  selected = 0;
  if (!keepQuery) input.value = '';
  glyph.textContent = CHROME[view].glyph;
  input.placeholder = CHROME[view].placeholder;
  kindBar.hidden = view !== 'clipboard';
  viewBar.querySelectorAll('[data-view]').forEach((b) => {
    b.classList.toggle('active', b.dataset.view === view);
  });
  draw();
  input.focus(); // switching lists must never cost you the search field
}

function setKind(next) {
  kind = KINDS.includes(next) ? next : 'all';
  selected = 0;
  kindBar.querySelectorAll('[data-kind]').forEach((b) => {
    b.classList.toggle('active', b.dataset.kind === kind);
  });
  draw();
  input.focus();
}

/** Everything the active view offers, narrowed by the query (and, for clips, the kind). */
function visible() {
  const q = input.value.trim().toLowerCase();
  const hit = (hay) => !q || hay.toLowerCase().includes(q);
  if (view === 'clipboard') {
    return clips.filter((c) => (kind === 'all' || c.kind === kind)
      && hit(`${c.preview} ${c.sourceApp}`));
  }
  if (view === 'captures') {
    return CAPTURES.filter((c) => hit(`${c.label} ${c.keys}`));
  }
  return snippets.filter((s) => hit(`${s.trigger} ${s.replacement} ${s.group}`));
}

/** How many each view would show for the current query — the counts on the tabs. */
function counts() {
  const q = input.value.trim().toLowerCase();
  const hit = (hay) => !q || hay.toLowerCase().includes(q);
  return {
    clipboard: clips.filter((c) => (kind === 'all' || c.kind === kind)
      && hit(`${c.preview} ${c.sourceApp}`)).length,
    captures: CAPTURES.filter((c) => hit(`${c.label} ${c.keys}`)).length,
    snippets: snippets.filter((s) => hit(`${s.trigger} ${s.replacement} ${s.group}`)).length,
  };
}

function draw() {
  rows = visible();
  if (selected >= rows.length) selected = Math.max(0, rows.length - 1);

  const n = counts();
  viewBar.querySelectorAll('[data-view]').forEach((b) => {
    b.querySelector('[data-count]').textContent = n[b.dataset.view] || '';
  });

  list.textContent = '';
  if (!rows.length) {
    list.appendChild(emptyRow(n));
  } else {
    rows.forEach((row, i) => list.appendChild(rowFor(row, i)));
    list.children[selected]?.scrollIntoView({ block: 'nearest' });
  }
  drawFoot();
}

function emptyRow(n) {
  const li = document.createElement('li');
  li.className = 'pal-empty';
  const q = input.value.trim();
  if (view === 'clipboard' && !clips.length) {
    li.innerHTML = 'Nothing copied yet.<br>Anything you copy from now on shows up here.';
    return li;
  }
  if (view === 'snippets' && !snippets.length) {
    li.innerHTML = 'No snippets yet.<br>Add some from <b>Snippets &amp; Settings</b> in the menu bar.';
    return li;
  }
  // Nothing here, but maybe somewhere else — say so rather than look broken.
  const elsewhere = VIEWS.filter((v) => v !== view && n[v] > 0);
  li.innerHTML = q ? `Nothing in ${labelOf(view)} matches “${escapeHtml(q)}”.` : `Nothing in ${labelOf(view)}.`;
  if (elsewhere.length) {
    li.innerHTML += `<br><b>${elsewhere
      .map((v) => `${n[v]} in ${labelOf(v)} (⌘${VIEWS.indexOf(v) + 1})`)
      .join(' · ')}</b>`;
  }
  return li;
}

function rowFor(row, i) {
  const li = document.createElement('li');
  li.className = 'pal-row' + (i === selected ? ' active' : '');
  li.addEventListener('click', () => { selected = i; act(false); });

  if (view === 'clipboard') {
    const pin = document.createElement('span');
    pin.className = 'clip-pin';
    pin.textContent = row.pinned ? '●' : '';
    li.append(pin);
    if (row.kind === 'image' && row.imagePath) {
      const img = document.createElement('img');
      img.className = 'clip-thumb';
      img.alt = row.preview;
      img.src = convertFileSrc(row.imagePath);
      li.append(img);
    }
    const body = document.createElement('span');
    body.className = 'clip-body';
    body.textContent = row.preview;
    li.append(body);
    if (row.sourceApp) {
      const tag = document.createElement('span');
      tag.className = 'pal-tag';
      tag.textContent = row.sourceApp;
      li.append(tag);
    }
    const when = document.createElement('span');
    when.className = 'clip-when';
    when.textContent = ago(row.copiedAt);
    li.append(when);
    return li;
  }

  if (view === 'captures') {
    const ico = document.createElement('span');
    ico.className = 'pal-ico';
    ico.innerHTML = icon('camera', 15);
    const label = document.createElement('span');
    label.className = 'pal-action-label';
    label.textContent = row.label;
    li.append(ico, label);
    return li;
  }

  const trigger = document.createElement('span');
  trigger.className = 'pal-trigger';
  trigger.textContent = row.trigger;
  const preview = document.createElement('span');
  preview.className = 'pal-preview';
  preview.textContent = plain(row.replacement);
  li.append(trigger, preview);
  if (row.group) {
    const tag = document.createElement('span');
    tag.className = 'pal-tag';
    tag.textContent = row.group;
    li.append(tag);
  }
  return li;
}

function drawFoot() {
  foot.textContent = '';
  if (error) {
    const e = document.createElement('span');
    e.className = 'pal-err';
    e.textContent = error;
    foot.append(e);
    return;
  }
  for (const [key, what] of CHROME[view].keys) {
    const span = document.createElement('span');
    span.innerHTML = `<kbd>${key}</kbd> ${what}`;
    foot.append(span);
  }
  const spacer = document.createElement('span');
  spacer.className = 'spacer';
  const tab = document.createElement('span');
  tab.innerHTML = '<kbd>tab</kbd> switch list';
  const esc = document.createElement('span');
  esc.innerHTML = '<kbd>esc</kbd> close';
  foot.append(spacer, tab, esc);
}

/** A snippet body as one line of plain text — HTML bodies show their words, not their markup. */
function plain(body) {
  const text = String(body || '');
  if (!/[<&]/.test(text)) return text.replace(/\s+/g, ' ').trim();
  const div = document.createElement('div');
  div.innerHTML = text;
  return (div.textContent || '').replace(/\s+/g, ' ').trim();
}

function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, (c) =>
    ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}

/// Rough age — the exact second something was copied has never mattered to anyone.
function ago(iso) {
  const secs = Math.max(0, (Date.now() - new Date(iso).getTime()) / 1000);
  if (secs < 60) return 'now';
  if (secs < 3600) return `${Math.floor(secs / 60)}m`;
  if (secs < 86400) return `${Math.floor(secs / 3600)}h`;
  return `${Math.floor(secs / 86400)}d`;
}

/** Enter (`alt` = false) or ⌘Enter (`alt` = true) on the selected row. */
async function act(alt) {
  const row = rows[selected];
  if (!row) return;
  try {
    if (view === 'clipboard') {
      await invoke('clipboard_use', { id: row.id, paste: !alt });
    } else if (view === 'captures') {
      await invoke('palette_capture', { mode: row.mode, silent: alt });
    } else if (alt) {
      // The body as stored, matching what ⌘↩ has always put on the clipboard here. (For an
      // HTML-format snippet that is its markup — a pre-existing wart, not a new one.)
      await navigator.clipboard.writeText(row.replacement || '');
      hide();
    } else {
      await invoke('palette_exec', { trigger: row.trigger });
    }
  } catch (e) {
    error = String(e);
    drawFoot();
  }
}

async function clipAction(fn) {
  const row = rows[selected];
  if (!row || view !== 'clipboard') return;
  try {
    await fn(row);
    clips = await invoke('list_clips');
    draw();
  } catch (e) {
    error = String(e);
    drawFoot();
  }
}

function onKey(e) {
  const mod = e.metaKey || e.ctrlKey;
  // ⌘1/⌘2/⌘3 jump straight to a list, which is also what the empty state advertises.
  if (mod && ['1', '2', '3'].includes(e.key)) {
    e.preventDefault();
    setView(VIEWS[Number(e.key) - 1]);
    return;
  }
  switch (e.key) {
    case 'ArrowDown':
      e.preventDefault();
      selected = Math.min(selected + 1, rows.length - 1);
      draw();
      return;
    case 'ArrowUp':
      e.preventDefault();
      selected = Math.max(selected - 1, 0);
      draw();
      return;
    case 'Tab':
      e.preventDefault();
      setView(VIEWS[(VIEWS.indexOf(view) + (e.shiftKey ? VIEWS.length - 1 : 1)) % VIEWS.length]);
      return;
    case 'Enter':
      e.preventDefault();
      act(mod);
      return;
    case 'Escape':
      e.preventDefault();
      hide();
      return;
    default:
      break;
  }
  if (view === 'clipboard') {
    if (mod && (e.key === 'p' || e.key === 'P')) {
      e.preventDefault();
      clipAction((row) => invoke('clip_set_pinned', { id: row.id, pinned: !row.pinned }));
      return;
    }
    if (mod && (e.key === 'Backspace' || e.key === 'Delete')) {
      e.preventDefault();
      clipAction((row) => invoke('delete_clip', { id: row.id }));
      return;
    }
    // ← / → step the kind filter, but only when the caret has nothing to do with them.
    if ((e.key === 'ArrowLeft' || e.key === 'ArrowRight') && !input.value) {
      e.preventDefault();
      const step = e.key === 'ArrowRight' ? 1 : KINDS.length - 1;
      setKind(KINDS[(KINDS.indexOf(kind) + step) % KINDS.length]);
    }
  }
}
