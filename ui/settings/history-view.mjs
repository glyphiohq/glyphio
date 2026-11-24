// settings/history-view.mjs — the History view: captures and clipboard entries in one
// timeline. Loaded the first time History is opened (PERF-05).

import { icon } from '../shared/icons.js';
import { invokeBytes } from '../shared/ipc-bytes.mjs';
import { compositeBanner } from '../shared/banner.js';
import { escapeHtml } from '../shared/markdown.js';
import { state } from './model.mjs';
import { confirmDialog, setStatus } from './dom.mjs';

const { invoke, convertFileSrc } = window.__TAURI__.core;

// --- Capture history (in-window view, replaces the old separate window) --------

/// History is one timeline of everything this device kept for you: screenshots you took and
/// things you copied. They were always the same act — "hold on to this" — and keeping them in
/// two places meant knowing in advance which list a thing had gone into.
///
/// The three views split it the way people actually look for something: everything, the
/// things that are words, the things that are pictures. A screenshot and a copied image are
/// both pictures, so `image` holds both.
const HISTORY_VIEWS = [
  ['all', 'All'],
  ['text', 'Text'],
  ['image', 'Images'],
];

export async function renderHistory(main) {
  state.historyView = state.historyView || 'all';
  state.historyQuery = state.historyQuery || '';
  const tabs = HISTORY_VIEWS.map(([id, label]) =>
    `<button type="button" class="seg-opt ${state.historyView === id ? 'active' : ''}" data-hview="${id}">${label}</button>`).join('');
  // Two rows: what this is, then how to narrow it. One row put a title, three tabs, a search
  // field, a stat line and a destructive button in the same space, and at any window width
  // something ended up squeezed against something else.
  main.innerHTML = `
    <div class="main-head">
      <h2 class="main-title" style="margin:0">History</h2>
      <div class="spacer"></div>
      <span class="hist-stats" id="hist-stats"></span>
      <button class="danger" id="hist-clear">Clear all</button>
    </div>
    <div class="hist-bar">
      <div class="seg settings-tabs" id="hist-views">${tabs}</div>
      <input type="search" id="hist-q" class="hist-search" placeholder="Search history…"
             autocomplete="off" spellcheck="false">
    </div>
    <ul class="hist-grid" id="hist-grid"></ul>
    <div class="empty" id="hist-empty" hidden></div>`;

  main.querySelectorAll('[data-hview]').forEach((b) => b.addEventListener('click', () => {
    state.historyView = b.dataset.hview;
    renderHistory(main);
  }));
  const search = main.querySelector('#hist-q');
  search.value = state.historyQuery;
  search.addEventListener('input', () => {
    state.historyQuery = search.value;
    draw();
  });

  const [captures, clips] = await Promise.all([
    invoke('list_captures').catch(() => []),
    invoke('list_clips').catch(() => []),
  ]);
  // One timeline, newest first. Each entry carries where it came from so the card renderer
  // and the delete button both know what they are dealing with.
  const all = [
    ...captures.map((c) => ({ source: 'capture', kind: 'image', at: c.capturedAt, item: c })),
    ...clips.map((c) => ({ source: 'clip', kind: c.kind, at: c.copiedAt, item: c })),
  ].sort((a, b) => String(b.at).localeCompare(String(a.at)));

  const grid = main.querySelector('#hist-grid');
  const stats = main.querySelector('#hist-stats');
  const empty = main.querySelector('#hist-empty');
  let shown = [];

  function draw() {
    const q = state.historyQuery.trim().toLowerCase();
    shown = all.filter((e) => {
      if (state.historyView !== 'all' && e.kind !== state.historyView) return false;
      if (!q) return true;
      const hay = e.source === 'capture'
        ? `${e.item.title || ''} ${e.item.url || ''} ${e.item.pageTitle || ''} ${e.item.note || ''}`
        : `${e.item.preview || ''} ${e.item.sourceApp || ''}`;
      return hay.toLowerCase().includes(q);
    });
    grid.textContent = '';
    for (const entry of shown) {
      grid.appendChild(
        entry.source === 'capture'
          ? historyCard(entry.item, () => renderHistory(main))
          : clipCard(entry.item, () => renderHistory(main)),
      );
    }
    const bytes = shown.reduce((sum, e) => sum + (e.item.sizeBytes || 0), 0);
    const size = bytes > 1048576
      ? `${(bytes / 1048576).toFixed(1)} MB`
      : `${Math.max(1, Math.round(bytes / 1024))} KB`;
    stats.textContent = shown.length === all.length
      ? `${all.length} ${all.length === 1 ? 'entry' : 'entries'} · ${size}`
      : `${shown.length} of ${all.length} · ${size}`;
    empty.hidden = shown.length > 0;
    empty.textContent = all.length
      ? 'Nothing here matches that.'
      : 'Nothing yet — press ⌥⇧X to snip something, or copy anything at all.';
    main.querySelector('#hist-clear').disabled = all.length === 0;
  }

  // Always "Clear all", and it always clears all of it. A label that changed with the filter
  // made the button's reach a thing you had to read before trusting; the confirmation names
  // exactly what goes instead.
  main.querySelector('#hist-clear').addEventListener('click', async () => {
    const ok = await confirmDialog(
      'Delete every capture and every clipboard entry, pinned ones included?',
      { confirmLabel: 'Clear all', danger: true },
    );
    if (!ok) return;
    try {
      await Promise.all([invoke('clear_captures'), invoke('clear_clips')]);
    } catch (err) {
      setStatus(String(err), 'err');
    }
    renderHistory(main);
  });

  draw();
}

/// A clipboard entry in the history grid. Text gets its words, an image gets its pixels; both
/// get the app they came from, because "where was I when I copied this" is most of how people
/// recognise a thing they copied an hour ago.
function clipCard(clip, redraw) {
  const li = document.createElement('li');
  li.className = 'hist-card';

  if (clip.kind === 'image' && clip.imagePath) {
    const img = document.createElement('img');
    img.className = 'hist-thumb';
    img.alt = clip.preview || 'copied image';
    img.src = convertFileSrc(clip.imagePath);
    li.append(img);
  } else {
    const body = document.createElement('div');
    body.className = 'hist-thumb hist-text';
    body.textContent = clip.preview || '';
    li.append(body);
  }

  const meta = document.createElement('div');
  meta.className = 'hist-meta';
  const dt = new Date(clip.copiedAt);
  const badge = clip.kind === 'image' ? 'Copied image' : 'Copied text';
  meta.innerHTML = `<span class="hist-when"><span class="hist-badge">${badge}</span>${isNaN(dt) ? '' : dt.toLocaleString()}${clip.pinned ? ' · pinned' : ''}</span>
    <span class="hist-title">${escapeHtml(clip.sourceApp || 'clipboard')}</span>`;

  const actions = document.createElement('div');
  actions.className = 'hist-actions';
  const mk = (name, title, fn, cls = 'ghost') => {
    const b = document.createElement('button');
    b.className = `${cls} iconbtn sm`;
    b.innerHTML = icon(name, 15);
    b.title = title;
    b.setAttribute('aria-label', title);
    b.addEventListener('click', fn);
    return b;
  };
  actions.append(
    mk('copy', 'Copy again', async () => {
      try {
        await invoke('clipboard_use', { id: clip.id, paste: false });
        setStatus('Back on the clipboard.', 'ok');
      } catch (e) { setStatus(String(e), 'err'); }
    }),
    mk(clip.pinned ? 'pinFilled' : 'pin', clip.pinned ? 'Unpin' : 'Pin', async () => {
      try {
        await invoke('clip_set_pinned', { id: clip.id, pinned: !clip.pinned });
        redraw();
      } catch (e) { setStatus(String(e), 'err'); }
    }),
  );
  if (clip.kind === 'image' && clip.imagePath) {
    actions.append(mk('download', 'Save to file…', async () => {
      const iso = (clip.copiedAt || '').replace(/[:T]/g, '-').slice(0, 19);
      const path = await window.__TAURI__.dialog.save({
        defaultPath: `${(state.settings?.filenamePrefix || 'glyphio')}-copied-${iso}.png`,
        filters: [{ name: 'PNG image', extensions: ['png'] }],
      });
      if (!path) return;
      try {
        const blob = await (await fetch(convertFileSrc(clip.imagePath))).blob();
        await invokeBytes(invoke, 'save_file', blob, { path });
        setStatus(`Saved → ${path}`, 'ok');
      } catch (e) { setStatus(String(e), 'err'); }
    }));
  }
  actions.append(
    mk('trash', 'Delete', async () => { await invoke('delete_clip', { id: clip.id }); redraw(); }, 'danger'),
  );
  li.append(meta, actions);
  return li;
}

/// What a stored capture tells its banner. Rows saved before the browser fields existed have
/// only the window title, under its original `url` name.
function bannerMeta(item) {
  return {
    capturedAt: item.capturedAt,
    windowTitle: item.title || item.url || '',
    pageTitle: item.pageTitle || '',
    pageUrl: item.pageUrl || '',
    profile: item.profile || '',
  };
}

// Thumbnails are stored content-only; composite the banner at thumbnail scale so the grid
// preview matches what Open/Copy/Save produce (timestamp strip included).
async function bannerizeThumb(item, img) {
  if (item.bannerBaked || item.bannerEnabled === false || !item.thumbPath) return;
  const settings = state.settings;
  const thumb = new Image();
  await new Promise((resolve, reject) => {
    thumb.onload = resolve;
    thumb.onerror = () => reject(new Error('thumbnail failed to load'));
    thumb.src = convertFileSrc(item.thumbPath);
  });
  const ratio = item.imageWidthPx ? thumb.naturalWidth / item.imageWidthPx : 1;
  const cvs = document.createElement('canvas');
  compositeBanner(cvs, thumb, {
    meta: { ...bannerMeta(item), dpr: (item.dpr || 1) * ratio },
    settings,
    note: item.note || '',
    enabled: true,
  });
  img.src = cvs.toDataURL('image/png');
}

function historyCard(item, redraw) {
  const li = document.createElement('li');
  li.className = 'hist-card';
  const img = document.createElement('img');
  img.className = 'hist-thumb';
  img.alt = item.title || 'capture';
  if (item.thumbPath) img.src = convertFileSrc(item.thumbPath);
  bannerizeThumb(item, img).catch(() => { /* keep the plain thumb */ });
  img.addEventListener('click', () => invoke('open_capture', { id: item.id }));
  const meta = document.createElement('div');
  meta.className = 'hist-meta';
  const dt = new Date(item.capturedAt);
  // The badge earns its space in a mixed grid: a screenshot of a text editor and a copied
  // block of text can look alike at thumbnail size, and they answer to different buttons.
  meta.innerHTML = `<span class="hist-when"><span class="hist-badge">Screenshot</span>${isNaN(dt) ? '' : dt.toLocaleString()}</span>
    <span class="hist-title">${escapeHtml(item.title || item.url || '')}</span>`;
  const actions = document.createElement('div');
  actions.className = 'hist-actions';
  const mk = (name, title, fn, cls = 'ghost') => {
    const b = document.createElement('button');
    b.className = `${cls} iconbtn sm`;
    b.innerHTML = icon(name, 15);
    b.title = title;
    b.setAttribute('aria-label', title);
    b.addEventListener('click', fn);
    return b;
  };
  actions.append(
    mk('open', 'Open', () => invoke('open_capture', { id: item.id })),
    // Through the OS, like the editor's copy button — `navigator.clipboard.write` only works
    // during a user gesture, and going via the command means both paths behave identically
    // instead of one of them landing in clipboard history and the other not.
    mk('copy', 'Copy to clipboard', async () => {
      try {
        // Natively, banner included when it is on — the same strip a fresh capture gets.
        await invoke('copy_stored_capture', { id: item.id });
        setStatus('Capture copied to clipboard.', 'ok');
      } catch (e) { setStatus(String(e), 'err'); }
    }),
    mk('download', 'Save to file…', async () => {
      const iso = (item.capturedAt || '').replace(/[:T]/g, '-').slice(0, 19);
      const path = await window.__TAURI__.dialog.save({
        defaultPath: `${(state.settings?.filenamePrefix || 'glyphio')}-${iso}.png`,
        filters: [{ name: 'PNG image', extensions: ['png'] }],
      });
      if (!path) return;
      try {
        await invoke('export_stored_capture', { id: item.id, path });
        setStatus(`Saved → ${path}`, 'ok');
      } catch (e) { setStatus(String(e), 'err'); }
    }),
    mk('trash', 'Delete', async () => { await invoke('delete_capture', { id: item.id }); redraw(); }, 'danger'),
  );
  li.append(img, meta, actions);
  return li;
}
