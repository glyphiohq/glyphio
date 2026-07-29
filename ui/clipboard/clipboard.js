// clipboard/clipboard.js — the clipboard history picker.
//
// Summoned by a global hotkey (or the tray). Enter puts the entry back on the clipboard and
// pastes it into the app you came from; ⌘Enter loads the clipboard and leaves the pasting to
// you. ⌘P pins an entry so retention can't reach it, ⌘⌫ forgets one.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const input = document.getElementById('q');
const list = document.getElementById('list');
const errEl = document.getElementById('err');

let clips = [];
let filtered = [];
let selected = 0;
/** id → data URL, so re-drawing the list doesn't re-read every PNG off disk. */
const thumbs = new Map();

init();

async function init() {
  await refresh();
  await listen('clipboard-show', async () => {
    input.value = '';
    await refresh();
    input.focus();
    input.select();
  });
  // A new copy while the picker is open should appear in it.
  await listen('clipboard-changed', () => { if (document.hasFocus()) refresh(); });
  window.addEventListener('blur', close);
  input.addEventListener('input', () => { selected = 0; draw(); });
  input.addEventListener('keydown', onKey);
  input.focus();
}

function close() {
  invoke('clipboard_hide').catch(() => {});
}

async function refresh() {
  try {
    clips = await invoke('list_clips');
    errEl.textContent = '';
  } catch (e) {
    clips = [];
    errEl.textContent = String(e);
  }
  selected = 0;
  draw();
}

function match(clip, q) {
  if (!q) return true;
  return `${clip.preview} ${clip.sourceApp}`.toLowerCase().includes(q);
}

function draw() {
  const q = input.value.trim().toLowerCase();
  filtered = clips.filter((c) => match(c, q));
  if (selected >= filtered.length) selected = Math.max(0, filtered.length - 1);
  list.textContent = '';

  if (!filtered.length) {
    const empty = document.createElement('li');
    empty.className = 'pal-empty';
    empty.textContent = clips.length
      ? 'Nothing matches that.'
      : 'Nothing copied yet. Anything you copy from now on shows up here.';
    list.appendChild(empty);
    return;
  }

  filtered.forEach((clip, i) => {
    const row = document.createElement('li');
    row.className = 'pal-row' + (i === selected ? ' active' : '');

    const pin = document.createElement('span');
    pin.className = 'clip-pin';
    pin.textContent = clip.pinned ? '●' : '';
    row.appendChild(pin);

    if (clip.kind === 'image') {
      const img = document.createElement('img');
      img.className = 'clip-thumb';
      img.alt = clip.preview;
      const cached = thumbs.get(clip.id);
      if (cached) {
        img.src = cached;
      } else {
        invoke('clip_image_data_url', { id: clip.id })
          .then((url) => { thumbs.set(clip.id, url); img.src = url; })
          .catch(() => {});
      }
      row.appendChild(img);
    }

    const preview = document.createElement('span');
    preview.className = 'clip-preview';
    preview.textContent = clip.preview;
    row.appendChild(preview);

    if (clip.sourceApp) {
      const tag = document.createElement('span');
      tag.className = 'pal-tag';
      tag.textContent = clip.sourceApp;
      row.appendChild(tag);
    }

    const when = document.createElement('span');
    when.className = 'clip-when';
    when.textContent = ago(clip.copiedAt);
    row.appendChild(when);

    row.addEventListener('click', () => { selected = i; use(true); });
    list.appendChild(row);
  });
  list.children[selected]?.scrollIntoView({ block: 'nearest' });
}

/// Rough age, because the exact second something was copied has never mattered to anyone.
function ago(iso) {
  const secs = Math.max(0, (Date.now() - new Date(iso).getTime()) / 1000);
  if (secs < 60) return 'now';
  if (secs < 3600) return `${Math.floor(secs / 60)}m`;
  if (secs < 86400) return `${Math.floor(secs / 3600)}h`;
  return `${Math.floor(secs / 86400)}d`;
}

async function use(paste) {
  const clip = filtered[selected];
  if (!clip) return;
  try {
    await invoke('clipboard_use', { id: clip.id, paste });
  } catch (e) {
    errEl.textContent = String(e);
  }
}

async function togglePin() {
  const clip = filtered[selected];
  if (!clip) return;
  try {
    await invoke('clip_set_pinned', { id: clip.id, pinned: !clip.pinned });
    await refresh();
  } catch (e) {
    errEl.textContent = String(e);
  }
}

async function remove() {
  const clip = filtered[selected];
  if (!clip) return;
  try {
    await invoke('delete_clip', { id: clip.id });
    thumbs.delete(clip.id);
    await refresh();
  } catch (e) {
    errEl.textContent = String(e);
  }
}

function onKey(e) {
  const mod = e.metaKey || e.ctrlKey;
  if (e.key === 'ArrowDown') {
    e.preventDefault();
    selected = Math.min(selected + 1, filtered.length - 1);
    draw();
  } else if (e.key === 'ArrowUp') {
    e.preventDefault();
    selected = Math.max(selected - 1, 0);
    draw();
  } else if (e.key === 'Enter') {
    e.preventDefault();
    use(!mod); // ⌘Enter loads the clipboard without pasting
  } else if (mod && (e.key === 'p' || e.key === 'P')) {
    e.preventDefault();
    togglePin();
  } else if (mod && (e.key === 'Backspace' || e.key === 'Delete')) {
    e.preventDefault();
    remove();
  } else if (e.key === 'Escape') {
    e.preventDefault();
    close();
  }
}
