// preview/preview.js
// Preview tab: reads the stitched content PNG from Cache Storage, composes a
// banner (timestamp + URL + note) on top, and offers Copy / Download / Retry.
// Loaded as an ES module so we can share config.js with the rest of the
// extension.

import { config, resolveSettings, DEFAULT_SHORTCUTS } from '../config.js';
import { matchesShortcut, formatShortcut, IS_MAC } from '../shared/shortcuts.js';

// Glyphio: Tauri bridge (chrome.* APIs replaced by native commands/plugins).
const { invoke } = window.__TAURI__.core;
const { save: saveDialog } = window.__TAURI__.dialog;

const LAST_NOTE_KEY = 'glyphio.lastNote';
let historySaved = false; // save the final artifact to history at most once per session

// --- DOM references -------------------------------------------------------

const canvas = document.getElementById('canvas');
const ctx = canvas.getContext('2d');
const noteInput = document.getElementById('note');
const copyBtn = document.getElementById('copy');
const downloadBtn = document.getElementById('download');
const retryBtn = document.getElementById('retry');
const cropBtn = document.getElementById('crop');
const historyBtn = document.getElementById('history');
const optionsBtn = document.getElementById('options');
const statusEl = document.getElementById('status');
const metaLineEl = document.getElementById('meta-line');
const shortcutHintEl = document.getElementById('shortcut-hint');
const cropOverlay = document.getElementById('crop-overlay');
const cropToolbar = document.getElementById('crop-toolbar');
const cropApplyBtn = document.getElementById('crop-apply');
const cropCancelBtn = document.getElementById('crop-cancel');
const redactBtn = document.getElementById('redact');
const redactOverlay = document.getElementById('redact-overlay');
const redactToolbar = document.getElementById('redact-toolbar');
const redactApplyBtn = document.getElementById('redact-apply');
const redactCancelBtn = document.getElementById('redact-cancel');
const editBtn = document.getElementById('edit');
const editDropdown = document.getElementById('edit-dropdown');
const drawBtn = document.getElementById('draw');
const drawOverlay = document.getElementById('draw-overlay');
const drawToolbar = document.getElementById('draw-toolbar');
const drawApplyBtn = document.getElementById('draw-apply');
const drawCancelBtn = document.getElementById('draw-cancel');
const drawUndoBtn = document.getElementById('draw-undo');
const textBtn = document.getElementById('text');
const textOverlay = document.getElementById('text-overlay');
const textToolbar = document.getElementById('text-toolbar');
const textApplyBtn = document.getElementById('text-apply');
const textCancelBtn = document.getElementById('text-cancel');
const textUndoBtn = document.getElementById('text-undo');
const canvasWrap = document.getElementById('canvas-wrap');

// --- Capture source -------------------------------------------------------
// Two modes: live preview via `#<id>` (reads Cache Storage; cleans up on
// unload) and history view via `?history=<id>` (reads IndexedDB; persistent).

const urlParams = new URLSearchParams(location.search);
const historyId = urlParams.get('history') || '';
const isHistoryMode = Boolean(historyId);

// --- State ---------------------------------------------------------------

let meta = null;           // json meta (from cache or history row)
let contentBitmap = null;  // ImageBitmap of the stitched content
let settings = null;       // merged user settings + defaults
let currentBlob = null;    // current banner+content as PNG Blob
let autoCopyDone = false;
let noteTimer = null;
// Once a crop has been applied, the canvas IS the image — banner + note edits
// are disabled so a subsequent render() can't undo the crop. The flag stays
// true for the life of the preview tab; users can reopen from history to get
// the uncropped original back.
let bannerBaked = false;
let cropModeActive = false;
let redactModeActive = false;
let drawModeActive = false;
let textModeActive = false;

init().catch((err) => setStatus(err.message, 'err'));

async function init() {
  settings = await loadSettings();
  renderShortcutHint();
  await loadPayload();
  document.title = `${config.name} — ${meta.title || meta.url}`;

  // Pre-fill the note field with the last one the user typed this session.
  // Skip in history mode — looking at a stored capture shouldn't mutate it.
  if (!isHistoryMode) {
    const lastNote = localStorage.getItem(LAST_NOTE_KEY);
    if (lastNote) noteInput.value = lastNote;
  }

  wireEvents();

  if (isHistoryMode) {
    // Stored image already has the banner baked in — display it directly.
    await displayStored();
  } else {
    await render({ autoCopy: settings.autoCopyOnOpen });
  }
}

async function loadSettings() {
  const stored = await invoke('get_settings');
  return resolveSettings(stored);
}

async function loadPayload() {
  if (isHistoryMode) {
    const list = await invoke('list_captures');
    const row = list.find((c) => c.id === historyId);
    if (!row) {
      throw new Error('This history entry is no longer available. It may have been deleted or evicted by retention.');
    }
    const dataUrl = await invoke('read_capture_data_url', { id: historyId });
    meta = {
      id: row.id,
      capturedAt: row.capturedAt,
      url: row.url,
      title: row.title,
      mode: row.mode,
      targetFrameUrl: '',
      imageWidthPx: row.imageWidthPx,
      imageHeightPx: row.imageHeightPx,
      dpr: row.dpr || 1,
    };
    if (contentBitmap) contentBitmap.close();
    contentBitmap = await createImageBitmap(await dataUrlToBlob(dataUrl));
  } else {
    const p = await invoke('take_pending_capture');
    if (!p) {
      throw new Error('No pending capture. Trigger a capture from the tray or a hotkey.');
    }
    meta = {
      capturedAt: p.capturedAt,
      url: p.title,
      title: p.title,
      mode: p.mode,
      targetFrameUrl: '',
      imageWidthPx: p.width,
      imageHeightPx: p.height,
      dpr: p.dpr || 1,
    };
    if (contentBitmap) contentBitmap.close();
    contentBitmap = await createImageBitmap(await dataUrlToBlob(p.pngDataUrl));
  }
  const modeLabel = isHistoryMode ? `history · ${meta.mode}` : meta.mode;
  metaLineEl.textContent = `${modeLabel} · ${meta.imageWidthPx}×${meta.imageHeightPx}px · ${meta.url}`;
}

// History mode: the stored PNG already contains the banner, so draw it straight
// onto the canvas (bypassing render()'s banner composition) and lock editing.
async function displayStored() {
  canvas.width = contentBitmap.width;
  canvas.height = contentBitmap.height;
  ctx.drawImage(contentBitmap, 0, 0);
  currentBlob = await canvasToPngBlob(canvas);
  bannerBaked = true;
}

function wireEvents() {
  if (isHistoryMode) {
    // History mode: note is locked (saved captures shouldn't be mutated);
    // retry doesn't make sense because the original tab may be gone.
    noteInput.disabled = true;
    noteInput.placeholder = 'Saved capture — note is read-only';
    retryBtn.disabled = true;
    retryBtn.title = 'Retry is not available on history entries';
  } else {
    noteInput.addEventListener('input', () => {
      clearTimeout(noteTimer);
      noteTimer = setTimeout(() => {
        render({ autoCopy: false });
        localStorage.setItem(LAST_NOTE_KEY, noteInput.value);
      }, 200);
    });
    retryBtn.addEventListener('click', () => retry().catch((e) => setStatus(e.message, 'err')));
  }

  copyBtn.addEventListener('click', () => copyToClipboard().catch((e) => setStatus(e.message, 'err')));
  downloadBtn.addEventListener('click', () => downloadPng().catch((e) => setStatus(e.message, 'err')));
  historyBtn.addEventListener('click', () => invoke('open_history_view'));
  optionsBtn.addEventListener('click', () => invoke('open_window', { name: 'settings' }));

  // --- Edit dropdown ------------------------------------------------------
  // Single "Edit ▾" button opens a menu of Crop / Redact / Draw. Each item
  // closes the menu and enters its respective mode.
  editBtn.addEventListener('click', (e) => {
    e.stopPropagation();
    toggleEditMenu();
  });
  document.addEventListener('click', (e) => {
    if (!editDropdown.hidden && !document.getElementById('edit-menu').contains(e.target)) {
      closeEditMenu();
    }
  });
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape' && !editDropdown.hidden) {
      closeEditMenu();
    }
  });

  if (settings.enableCrop) {
    cropBtn.addEventListener('click', () => { closeEditMenu(); enterCropMode(); });
    cropApplyBtn.addEventListener('click', () => applyCrop());
    cropCancelBtn.addEventListener('click', () => exitCropMode());
  } else {
    cropBtn.hidden = true;
  }

  if (settings.enableRedact !== false) {
    redactBtn.addEventListener('click', () => { closeEditMenu(); enterRedactMode(); });
    redactApplyBtn.addEventListener('click', () => applyRedact().catch((e) => setStatus(e.message, 'err')));
    redactCancelBtn.addEventListener('click', () => exitRedactMode());
  } else {
    redactBtn.hidden = true;
  }

  if (settings.enableDraw !== false) {
    drawBtn.addEventListener('click', () => { closeEditMenu(); enterDrawMode(); });
    drawApplyBtn.addEventListener('click', () => applyDraw().catch((e) => setStatus(e.message, 'err')));
    drawCancelBtn.addEventListener('click', () => exitDrawMode());
    drawUndoBtn.addEventListener('click', () => undoDraw());
  } else {
    drawBtn.hidden = true;
  }

  if (settings.enableText !== false) {
    textBtn.addEventListener('click', () => { closeEditMenu(); enterTextMode(); });
    textApplyBtn.addEventListener('click', () => applyText().catch((e) => setStatus(e.message, 'err')));
    textCancelBtn.addEventListener('click', () => exitTextMode());
    textUndoBtn.addEventListener('click', () => undoText());
  } else {
    textBtn.hidden = true;
  }

  // Hide the whole Edit button if every edit tool is disabled.
  if (
    !settings.enableCrop &&
    settings.enableRedact === false &&
    settings.enableDraw === false &&
    settings.enableText === false
  ) {
    editBtn.hidden = true;
  }

  document.addEventListener('keydown', onKeyDown);
}

function onKeyDown(e) {
  const tag = e.target?.tagName;
  const inField = tag === 'INPUT' || tag === 'TEXTAREA';
  const s = settings.shortcuts || DEFAULT_SHORTCUTS;

  // In crop mode, Esc cancels the crop and Enter applies it, regardless of
  // what `close`/other bindings are set to.
  if (cropModeActive) {
    if (e.key === 'Escape') { e.preventDefault(); exitCropMode(); return; }
    if (e.key === 'Enter')  { e.preventDefault(); applyCrop();    return; }
  }
  // Same override pattern for redact mode.
  if (redactModeActive) {
    if (e.key === 'Escape') { e.preventDefault(); exitRedactMode(); return; }
    if (e.key === 'Enter')  { e.preventDefault(); applyRedact().catch((err) => setStatus(err.message, 'err')); return; }
  }
  if (drawModeActive) {
    if (e.key === 'Escape') { e.preventDefault(); exitDrawMode(); return; }
    if (e.key === 'Enter')  { e.preventDefault(); applyDraw().catch((err) => setStatus(err.message, 'err')); return; }
    if ((e.key === 'z' || e.key === 'Z') && (e.metaKey || e.ctrlKey)) { e.preventDefault(); undoDraw(); return; }
  }
  // Text mode: Esc/Enter/undo only fire here when the inline input is NOT
  // focused. The input element's own keydown listener stops propagation
  // for those keys so this block sees them only when no input is open.
  if (textModeActive) {
    if (e.key === 'Escape') { e.preventDefault(); exitTextMode(); return; }
    if (e.key === 'Enter')  { e.preventDefault(); applyText().catch((err) => setStatus(err.message, 'err')); return; }
    if ((e.key === 'z' || e.key === 'Z') && (e.metaKey || e.ctrlKey)) { e.preventDefault(); undoText(); return; }
  }

  // Copy — disable the image copy if the user has a text selection in flight;
  // let the browser's normal copy path handle text.
  if (matchesShortcut(e, s.copy)) {
    const sel = window.getSelection?.();
    if (!sel || sel.isCollapsed) {
      e.preventDefault();
      copyToClipboard().catch((err) => setStatus(err.message, 'err'));
    }
    return;
  }

  if (matchesShortcut(e, s.save)) {
    e.preventDefault();
    downloadPng().catch((err) => setStatus(err.message, 'err'));
    return;
  }

  if (!inField && matchesShortcut(e, s.retry)) {
    e.preventDefault();
    if (!isHistoryMode) retry().catch((err) => setStatus(err.message, 'err'));
    return;
  }

  if (!inField && matchesShortcut(e, s.history)) {
    e.preventDefault();
    invoke('open_window', { name: 'history' });
    return;
  }

  if (!inField && settings.enableCrop && matchesShortcut(e, s.crop)) {
    e.preventDefault();
    if (cropModeActive) exitCropMode();
    else enterCropMode();
    return;
  }

  if (!inField && settings.enableRedact !== false && s.redact && matchesShortcut(e, s.redact)) {
    e.preventDefault();
    if (redactModeActive) exitRedactMode();
    else enterRedactMode();
    return;
  }

  if (!inField && settings.enableDraw !== false && s.draw && matchesShortcut(e, s.draw)) {
    e.preventDefault();
    if (drawModeActive) exitDrawMode();
    else enterDrawMode();
    return;
  }

  if (!inField && settings.enableText !== false && s.text && matchesShortcut(e, s.text)) {
    e.preventDefault();
    if (textModeActive) exitTextMode();
    else enterTextMode();
    return;
  }

  if (matchesShortcut(e, s.close)) {
    e.preventDefault();
    try { window.close(); } catch {}
    setTimeout(() => {
      if (!document.hidden) setStatus(IS_MAC ? 'Press ⌘W to close this tab.' : 'Press Ctrl+W to close this tab.', 'info');
    }, 50);
  }
}

function renderShortcutHint() {
  const s = settings.shortcuts || DEFAULT_SHORTCUTS;
  const entries = [
    [s.copy, 'copy'],
    [s.save, 'save'],
    [s.retry, 'retry'],
    [s.history, 'history']
  ];
  if (settings.enableCrop) entries.push([s.crop, 'crop']);
  if (settings.enableRedact !== false && s.redact) entries.push([s.redact, 'redact']);
  if (settings.enableDraw !== false && s.draw) entries.push([s.draw, 'draw']);
  if (settings.enableText !== false && s.text) entries.push([s.text, 'text']);
  if (s.close && s.close.key) entries.push([s.close, 'close']);

  // Build via DOM nodes + textContent. Stored shortcut specs are user-editable,
  // so any HTML-significant characters (e.g. '<', '&') must not be interpreted.
  shortcutHintEl.textContent = '';
  entries.forEach(([spec, label], i) => {
    if (i > 0) shortcutHintEl.append(document.createTextNode(' · '));
    const kbd = document.createElement('kbd');
    kbd.textContent = formatShortcut(spec);
    shortcutHintEl.append(kbd, document.createTextNode(` ${label}`));
  });
}

// --- Rendering ------------------------------------------------------------

async function render({ autoCopy }) {
  if (!contentBitmap || !meta) return;
  // After a crop is applied, #canvas already holds the final pixels and
  // currentBlob is the exported PNG. Re-running render() would overwrite
  // both from the source contentBitmap + banner, undoing the crop. Skip.
  if (bannerBaked) return;

  const scale = meta.dpr;
  const imgW = contentBitmap.width;
  const note = noteInput.value.trim();

  const bannerCssH = computeBannerCssHeight(note);
  const bannerPxH = Math.round(bannerCssH * scale);

  canvas.width = imgW;
  canvas.height = bannerPxH + contentBitmap.height;

  if (bannerPxH > 0) {
    ctx.fillStyle = settings.bannerBg;
    ctx.fillRect(0, 0, canvas.width, bannerPxH);
    drawBanner(scale, note);
  }

  ctx.drawImage(contentBitmap, 0, bannerPxH);

  currentBlob = await canvasToPngBlob(canvas);

  if (autoCopy && !autoCopyDone) {
    autoCopyDone = true;
    try {
      await writeBlobToClipboard(currentBlob);
      setStatus('Copied to clipboard. Paste into your ticket.', 'ok');
    } catch (err) {
      setStatus(`Ready. Click "Copy to clipboard" (auto-copy blocked: ${err.message})`, 'info');
    }
  }
}

function showFrameUrlLine() {
  return settings.showTargetFrameUrl
    && meta.targetFrameUrl
    && meta.targetFrameUrl !== meta.url;
}

function hasAnyBannerContent(note) {
  return Boolean(settings.showTimestamp || settings.showUrl || showFrameUrlLine() || note);
}

function computeBannerCssHeight(note) {
  if (!hasAnyBannerContent(note)) return 0;
  const b = config.banner;
  let h = b.paddingPx;
  if (settings.showTimestamp) h += b.timestampFontPx;
  if (settings.showUrl) {
    if (settings.showTimestamp) h += b.lineGapPx;
    h += b.urlFontPx;
  }
  if (showFrameUrlLine()) {
    if (settings.showTimestamp || settings.showUrl) h += b.lineGapPx;
    h += b.urlFontPx;
  }
  if (note) {
    if (settings.showTimestamp || settings.showUrl || showFrameUrlLine()) h += b.lineGapPx;
    h += b.noteFontPx;
  }
  h += b.paddingPx;
  return h;
}

function drawBanner(scale, note) {
  const b = config.banner;
  ctx.textBaseline = 'top';
  let y = b.paddingPx * scale;
  let prevLineExists = false;

  if (settings.showTimestamp) {
    ctx.font = `bold ${b.timestampFontPx * scale}px ${b.fontFamily}`;
    ctx.fillStyle = settings.bannerFg;
    ctx.fillText(formatTimestamp(meta.capturedAt), b.paddingPx * scale, y);
    y += b.timestampFontPx * scale;
    prevLineExists = true;
  }

  if (settings.showUrl) {
    if (prevLineExists) y += b.lineGapPx * scale;
    ctx.font = `${b.urlFontPx * scale}px ${b.fontFamily}`;
    ctx.fillStyle = settings.bannerMuted;
    ctx.fillText(
      truncateToWidth(meta.url, canvas.width - 2 * b.paddingPx * scale),
      b.paddingPx * scale,
      y
    );
    y += b.urlFontPx * scale;
    prevLineExists = true;
  }

  if (showFrameUrlLine()) {
    if (prevLineExists) y += b.lineGapPx * scale;
    ctx.font = `${b.urlFontPx * scale}px ${b.fontFamily}`;
    ctx.fillStyle = settings.bannerMuted;
    ctx.fillText(
      `↳ ${truncateToWidth(meta.targetFrameUrl, canvas.width - 2 * b.paddingPx * scale - 20)}`,
      b.paddingPx * scale,
      y
    );
    y += b.urlFontPx * scale;
    prevLineExists = true;
  }

  if (note) {
    if (prevLineExists) y += b.lineGapPx * scale;
    ctx.font = `${b.noteFontPx * scale}px ${b.fontFamily}`;
    ctx.fillStyle = settings.bannerFg;
    ctx.fillText(
      truncateToWidth(note, canvas.width - 2 * b.paddingPx * scale),
      b.paddingPx * scale,
      y
    );
  }
}

function formatTimestamp(iso) {
  const d = new Date(iso);
  const locale = settings.locale === 'device' ? undefined : settings.locale;
  const tz = settings.timezone === 'device' ? undefined : settings.timezone;

  switch (settings.timestampFormat) {
    case 'iso-8601':
      if (settings.timezone === 'device') {
        return d.toISOString().replace('T', ' ').replace(/\.\d+Z$/, 'Z');
      }
      return formatIsoInTz(d, tz);
    case 'utc-human':
      return new Intl.DateTimeFormat('en-GB', {
        dateStyle: 'medium', timeStyle: 'long', timeZone: 'UTC'
      }).format(d);
    case 'device-locale':
    default:
      return new Intl.DateTimeFormat(locale, {
        dateStyle: 'medium', timeStyle: 'long', timeZone: tz
      }).format(d);
  }
}

function formatIsoInTz(d, tz) {
  const parts = new Intl.DateTimeFormat('en-GB', {
    timeZone: tz, year: 'numeric', month: '2-digit', day: '2-digit',
    hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false
  }).formatToParts(d).reduce((acc, p) => (acc[p.type] = p.value, acc), {});
  return `${parts.year}-${parts.month}-${parts.day} ${parts.hour}:${parts.minute}:${parts.second} ${tz}`;
}

function truncateToWidth(text, maxW) {
  if (ctx.measureText(text).width <= maxW) return text;
  const ellipsis = '…';
  let lo = 0, hi = text.length;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (ctx.measureText(text.slice(0, mid) + ellipsis).width <= maxW) lo = mid;
    else hi = mid - 1;
  }
  return text.slice(0, lo) + ellipsis;
}

// --- Edit-mode helpers ----------------------------------------------------
// Shared by Crop / Redact / Draw.

/**
 * Size and place an overlay canvas exactly over #canvas. The internal
 * resolution matches the canvas's pixel buffer so drag coordinates can map
 * 1:1 to canvas-space. The CSS box matches the canvas's rendered size and
 * its offset within `.canvas-wrap` — important when the canvas is centred
 * (snip captures, where the image is narrower than the wrap and `margin: 0
 * auto` shifts it right). Without this `left/top` step, the overlay was
 * pinned to the wrap's top-left corner and only covered the LEFT half of a
 * centred snip image.
 */
function fitOverlayToCanvas(overlay) {
  overlay.width = canvas.width;
  overlay.height = canvas.height;
  const bcr = canvas.getBoundingClientRect();
  overlay.style.width = `${bcr.width}px`;
  overlay.style.height = `${bcr.height}px`;
  overlay.style.left = `${canvas.offsetLeft}px`;
  overlay.style.top = `${canvas.offsetTop}px`;
}

/**
 * Switching from one edit mode to another while in-flight work exists
 * silently destroyed that work before. Now: confirm with the user when
 * non-empty work is on-screen; cancel the switch if they decline. When
 * the other modes are empty, auto-exit silently. Returns true if it's
 * safe to enter `target`.
 */
function ensureExitOtherModes(target) {
  if (target !== 'crop' && cropModeActive) {
    if (cropState.rect && !confirm('Exit crop mode? Your current selection will be discarded.')) return false;
    exitCropMode();
  }
  if (target !== 'redact' && redactModeActive) {
    const n = redactState.rects.length;
    if (n > 0 && !confirm(`Exit redact mode? ${n} drawn region${n > 1 ? 's' : ''} will be discarded.`)) return false;
    exitRedactMode();
  }
  if (target !== 'draw' && drawModeActive) {
    const n = drawState.shapes.length;
    if (n > 0 && !confirm(`Exit draw mode? ${n} shape${n > 1 ? 's' : ''} will be discarded.`)) return false;
    exitDrawMode();
  }
  if (target !== 'text' && textModeActive) {
    // Count both committed shapes and an in-flight (unconfirmed) input.
    const inFlight = textState.current && textState.current.el && textState.current.el.value.trim().length > 0 ? 1 : 0;
    const n = textState.shapes.length + inFlight;
    if (n > 0 && !confirm(`Exit text mode? ${n} label${n > 1 ? 's' : ''} will be discarded.`)) return false;
    exitTextMode();
  }
  return true;
}

// --- Crop -----------------------------------------------------------------
// Post-capture cropping tool. Operates on #canvas pixels directly; the source
// contentBitmap / stored blob are not mutated, so reopening the entry from
// history brings the original back.

const cropState = {
  dragging: false,
  startX: 0,
  startY: 0,
  rect: null, // { x, y, w, h } in canvas pixels
  listeners: null
};

function enterCropMode() {
  if (cropModeActive || !currentBlob) return;
  if (bannerBaked) {
    setStatus('Already cropped. Reopen from History to start over.', 'info');
    return;
  }
  if (!ensureExitOtherModes('crop')) return;
  cropModeActive = true;
  document.body.classList.add('cropping');

  fitOverlayToCanvas(cropOverlay);
  cropOverlay.hidden = false;
  cropToolbar.hidden = false;
  cropState.rect = null;
  drawCropOverlay();

  const onDown = (e) => {
    if (e.button !== 0) return;
    const p = eventToCanvasXY(e);
    cropState.dragging = true;
    cropState.startX = p.x;
    cropState.startY = p.y;
    cropState.rect = { x: p.x, y: p.y, w: 0, h: 0 };
    drawCropOverlay();
    e.preventDefault();
  };
  const onMove = (e) => {
    if (!cropState.dragging) return;
    const p = eventToCanvasXY(e);
    cropState.rect = rectFromPoints(cropState.startX, cropState.startY, p.x, p.y);
    drawCropOverlay();
  };
  const onUp = () => { cropState.dragging = false; };
  cropOverlay.addEventListener('pointerdown', onDown);
  window.addEventListener('pointermove', onMove);
  window.addEventListener('pointerup', onUp);
  cropState.listeners = { onDown, onMove, onUp };
  setStatus('Crop mode. Drag on the image, then Apply or Esc.', 'info');
}

function exitCropMode() {
  if (!cropModeActive) return;
  cropModeActive = false;
  document.body.classList.remove('cropping');
  cropOverlay.hidden = true;
  cropToolbar.hidden = true;
  if (cropState.listeners) {
    cropOverlay.removeEventListener('pointerdown', cropState.listeners.onDown);
    window.removeEventListener('pointermove', cropState.listeners.onMove);
    window.removeEventListener('pointerup', cropState.listeners.onUp);
    cropState.listeners = null;
  }
  cropState.rect = null;
  cropState.dragging = false;
  setStatus('', '');
}

function eventToCanvasXY(e) {
  const r = cropOverlay.getBoundingClientRect();
  const sx = cropOverlay.width / r.width;
  const sy = cropOverlay.height / r.height;
  return {
    x: Math.round((e.clientX - r.left) * sx),
    y: Math.round((e.clientY - r.top) * sy)
  };
}

function rectFromPoints(ax, ay, bx, by) {
  return {
    x: Math.min(ax, bx),
    y: Math.min(ay, by),
    w: Math.abs(bx - ax),
    h: Math.abs(by - ay)
  };
}

function drawCropOverlay() {
  const octx = cropOverlay.getContext('2d');
  octx.clearRect(0, 0, cropOverlay.width, cropOverlay.height);
  // Dim everything…
  octx.fillStyle = 'rgba(0, 0, 0, 0.45)';
  octx.fillRect(0, 0, cropOverlay.width, cropOverlay.height);
  if (!cropState.rect) return;
  const { x, y, w, h } = cropState.rect;
  // …then punch a hole through the selected rect.
  octx.clearRect(x, y, w, h);
  // Bright 1px border around the selection.
  octx.strokeStyle = '#60a5fa';
  octx.lineWidth = Math.max(1, Math.round(meta?.dpr || 1));
  octx.strokeRect(x + 0.5, y + 0.5, Math.max(0, w - 1), Math.max(0, h - 1));
}

async function applyCrop() {
  const r = cropState.rect;
  if (!r || r.w < 4 || r.h < 4) {
    setStatus('Drag a region first.', 'err');
    return;
  }
  // Snapshot the pixels, resize #canvas, redraw at origin.
  const tmp = new OffscreenCanvas(r.w, r.h);
  tmp.getContext('2d').drawImage(canvas, r.x, r.y, r.w, r.h, 0, 0, r.w, r.h);
  canvas.width = r.w;
  canvas.height = r.h;
  ctx.drawImage(tmp, 0, 0);
  currentBlob = await canvasToPngBlob(canvas);
  bannerBaked = true;
  noteInput.disabled = true;
  noteInput.placeholder = 'Cropped — note locked';
  exitCropMode();
  setStatus(`Cropped to ${r.w}×${r.h}px. Copy/Download now export the crop.`, 'ok');
}

// --- Redact mode ----------------------------------------------------------
// Paint black boxes or Gaussian-blurred regions over the current canvas.
// Mirrors cropMode but supports multiple rectangles. Like crop, mutates the
// preview canvas only — the stored blob / history row stays unchanged so
// reopening from History gives the unredacted original back.

const redactState = {
  dragging: false,
  startX: 0,
  startY: 0,
  current: null,  // in-flight rect while dragging
  rects: [],      // committed rects (pixel coords)
  listeners: null
};

function enterRedactMode() {
  if (redactModeActive || !currentBlob) return;
  if (!ensureExitOtherModes('redact')) return;
  redactModeActive = true;
  document.body.classList.add('cropping'); // reuse styling hook

  fitOverlayToCanvas(redactOverlay);
  redactOverlay.hidden = false;
  redactToolbar.hidden = false;
  redactState.rects = [];
  redactState.current = null;
  drawRedactOverlay();

  const onDown = (e) => {
    if (e.button !== 0) return;
    const p = overlayEventToCanvasXY(redactOverlay, e);
    redactState.dragging = true;
    redactState.startX = p.x;
    redactState.startY = p.y;
    redactState.current = { x: p.x, y: p.y, w: 0, h: 0 };
    drawRedactOverlay();
    e.preventDefault();
  };
  const onMove = (e) => {
    if (!redactState.dragging) return;
    const p = overlayEventToCanvasXY(redactOverlay, e);
    redactState.current = rectFromPoints(redactState.startX, redactState.startY, p.x, p.y);
    drawRedactOverlay();
  };
  const onUp = () => {
    if (!redactState.dragging) return;
    redactState.dragging = false;
    // Commit the in-flight rect if it has real size; reject tiny accidental clicks.
    const r = redactState.current;
    if (r && r.w >= 4 && r.h >= 4) redactState.rects.push(r);
    redactState.current = null;
    drawRedactOverlay();
  };
  redactOverlay.addEventListener('pointerdown', onDown);
  window.addEventListener('pointermove', onMove);
  window.addEventListener('pointerup', onUp);
  redactState.listeners = { onDown, onMove, onUp };
  setStatus('Redact mode. Drag any number of regions, then Apply or Esc.', 'info');
}

function exitRedactMode() {
  if (!redactModeActive) return;
  redactModeActive = false;
  document.body.classList.remove('cropping');
  redactOverlay.hidden = true;
  redactToolbar.hidden = true;
  if (redactState.listeners) {
    redactOverlay.removeEventListener('pointerdown', redactState.listeners.onDown);
    window.removeEventListener('pointermove', redactState.listeners.onMove);
    window.removeEventListener('pointerup', redactState.listeners.onUp);
    redactState.listeners = null;
  }
  redactState.rects = [];
  redactState.current = null;
  redactState.dragging = false;
  setStatus('', '');
}

function overlayEventToCanvasXY(overlayEl, e) {
  const r = overlayEl.getBoundingClientRect();
  const sx = overlayEl.width / r.width;
  const sy = overlayEl.height / r.height;
  return {
    x: Math.round((e.clientX - r.left) * sx),
    y: Math.round((e.clientY - r.top) * sy)
  };
}

function drawRedactOverlay() {
  const octx = redactOverlay.getContext('2d');
  octx.clearRect(0, 0, redactOverlay.width, redactOverlay.height);
  // Show each committed rect as a red-tinted fill with a 1px outline —
  // visible without being the final redacted look.
  octx.fillStyle = 'rgba(255, 89, 26, 0.35)';
  octx.strokeStyle = '#ff591a';
  octx.lineWidth = Math.max(1, Math.round(meta?.dpr || 1));
  for (const r of redactState.rects) {
    octx.fillRect(r.x, r.y, r.w, r.h);
    octx.strokeRect(r.x + 0.5, r.y + 0.5, Math.max(0, r.w - 1), Math.max(0, r.h - 1));
  }
  if (redactState.current) {
    const { x, y, w, h } = redactState.current;
    octx.fillStyle = 'rgba(255, 89, 26, 0.25)';
    octx.fillRect(x, y, w, h);
    octx.setLineDash([4, 3]);
    octx.strokeRect(x + 0.5, y + 0.5, Math.max(0, w - 1), Math.max(0, h - 1));
    octx.setLineDash([]);
  }
}

async function applyRedact() {
  const rects = redactState.rects.slice();
  if (rects.length === 0) {
    setStatus('Draw at least one region first.', 'err');
    return;
  }
  const mode = document.querySelector('input[name="redact-mode"]:checked')?.value || 'black';

  if (mode === 'black') {
    ctx.save();
    ctx.fillStyle = '#000';
    for (const r of rects) ctx.fillRect(r.x, r.y, r.w, r.h);
    ctx.restore();
  } else {
    // Blur: copy each rect through a Canvas2D blur filter back onto itself.
    // Snapshot first so we read from a stable source even when rects overlap.
    const snap = new OffscreenCanvas(canvas.width, canvas.height);
    snap.getContext('2d').drawImage(canvas, 0, 0);
    ctx.save();
    ctx.filter = 'blur(12px)';
    for (const r of rects) {
      ctx.drawImage(snap, r.x, r.y, r.w, r.h, r.x, r.y, r.w, r.h);
    }
    ctx.restore();
  }

  currentBlob = await canvasToPngBlob(canvas);
  bannerBaked = true;
  noteInput.disabled = true;
  noteInput.placeholder = 'Redacted — note locked';
  exitRedactMode();
  setStatus(`Redacted ${rects.length} region${rects.length > 1 ? 's' : ''} (${mode}). Copy/Download now export the redacted image.`, 'ok');
}

// --- Edit dropdown helpers -------------------------------------------------

function toggleEditMenu() {
  if (editDropdown.hidden) openEditMenu();
  else closeEditMenu();
}
function openEditMenu() {
  editDropdown.hidden = false;
  editBtn.setAttribute('aria-expanded', 'true');
}
function closeEditMenu() {
  editDropdown.hidden = true;
  editBtn.setAttribute('aria-expanded', 'false');
}

// --- Draw mode ------------------------------------------------------------
// Annotate the preview canvas with rectangles (outlined / filled), arrows
// and freehand marker strokes. Designed for low overhead:
//   - Committed shapes are stored as plain objects; overlay is redrawn on
//     each pointermove via requestAnimationFrame so we coalesce multiple
//     events per frame instead of redrawing synchronously.
//   - Marker strokes accumulate points into a polyline — rendering cost is
//     O(N) per redraw, trivial for typical human-scale drawings (<500 pts).
//   - Apply makes a single pass through the shape list to rasterise onto
//     the main canvas; the overlay canvas is cleared and discarded.
// Like crop / redact, never mutates the stored blob — original lives on in
// History.

const DRAW_TOOLS = new Set(['rect', 'rect-fill', 'arrow', 'marker']);
const drawState = {
  tool: 'rect',
  color: '#ff3b30',
  strokeWidth: 3,
  arrowHeadPx: 14,
  dragging: false,
  current: null,        // shape in progress
  shapes: [],           // committed shapes
  rafScheduled: false,
  listeners: null
};

function enterDrawMode() {
  if (drawModeActive || !currentBlob) return;
  if (!ensureExitOtherModes('draw')) return;
  drawModeActive = true;
  document.body.classList.add('cropping');

  fitOverlayToCanvas(drawOverlay);
  drawOverlay.hidden = false;
  drawToolbar.hidden = false;
  drawState.shapes = [];
  drawState.current = null;
  // Scale stroke to roughly track the captured image's resolution so 3 CSS
  // pixels look similar across retina + standard dpr captures.
  drawState.strokeWidth = Math.max(2, Math.round(3 * (meta?.dpr || 1)));
  drawState.arrowHeadPx = Math.max(10, Math.round(14 * (meta?.dpr || 1)));
  initDrawToolUI();
  scheduleDrawRender();

  const onDown = (e) => {
    if (e.button !== 0) return;
    const p = overlayEventToCanvasXY(drawOverlay, e);
    drawState.dragging = true;
    const base = { tool: drawState.tool, color: drawState.color, width: drawState.strokeWidth };
    if (drawState.tool === 'marker') {
      drawState.current = { ...base, points: [[p.x, p.y]] };
    } else {
      drawState.current = { ...base, x1: p.x, y1: p.y, x2: p.x, y2: p.y };
    }
    scheduleDrawRender();
    e.preventDefault();
  };
  const onMove = (e) => {
    if (!drawState.dragging || !drawState.current) return;
    const p = overlayEventToCanvasXY(drawOverlay, e);
    if (drawState.current.tool === 'marker') {
      // Throttle point adds: only record a point if it's moved a few pixels
      // from the last one. Cuts stored points by ~5-10x on fast sweeps
      // without visibly changing stroke quality.
      const pts = drawState.current.points;
      const [lx, ly] = pts[pts.length - 1];
      if (Math.abs(p.x - lx) + Math.abs(p.y - ly) >= 2) pts.push([p.x, p.y]);
    } else {
      drawState.current.x2 = p.x;
      drawState.current.y2 = p.y;
    }
    scheduleDrawRender();
  };
  const onUp = () => {
    if (!drawState.dragging) return;
    drawState.dragging = false;
    const s = drawState.current;
    drawState.current = null;
    if (!s) return;
    // Reject obviously accidental tiny shapes.
    if (s.tool === 'marker' && s.points.length < 2) { scheduleDrawRender(); return; }
    if (s.tool !== 'marker') {
      const w = Math.abs(s.x2 - s.x1), h = Math.abs(s.y2 - s.y1);
      if (w < 4 && h < 4) { scheduleDrawRender(); return; }
    }
    drawState.shapes.push(s);
    scheduleDrawRender();
  };
  drawOverlay.addEventListener('pointerdown', onDown);
  window.addEventListener('pointermove', onMove);
  window.addEventListener('pointerup', onUp);
  drawState.listeners = { onDown, onMove, onUp };
  setStatus('Draw mode. Pick a tool + colour, then drag. Enter to apply, Esc cancels.', 'info');
}

function initDrawToolUI() {
  const toolBtns = drawToolbar.querySelectorAll('[data-draw-tool]');
  toolBtns.forEach((b) => {
    b.setAttribute('aria-pressed', b.dataset.drawTool === drawState.tool ? 'true' : 'false');
    b.onclick = () => {
      if (!DRAW_TOOLS.has(b.dataset.drawTool)) return;
      drawState.tool = b.dataset.drawTool;
      toolBtns.forEach((bb) => bb.setAttribute('aria-pressed', bb === b ? 'true' : 'false'));
    };
  });
  const colorBtns = drawToolbar.querySelectorAll('[data-draw-color]');
  colorBtns.forEach((b) => {
    b.setAttribute('aria-pressed', b.dataset.drawColor === drawState.color ? 'true' : 'false');
    b.onclick = () => {
      drawState.color = b.dataset.drawColor;
      colorBtns.forEach((bb) => bb.setAttribute('aria-pressed', bb === b ? 'true' : 'false'));
    };
  });
}

function exitDrawMode() {
  if (!drawModeActive) return;
  drawModeActive = false;
  document.body.classList.remove('cropping');
  drawOverlay.hidden = true;
  drawToolbar.hidden = true;
  if (drawState.listeners) {
    drawOverlay.removeEventListener('pointerdown', drawState.listeners.onDown);
    window.removeEventListener('pointermove', drawState.listeners.onMove);
    window.removeEventListener('pointerup', drawState.listeners.onUp);
    drawState.listeners = null;
  }
  drawState.shapes = [];
  drawState.current = null;
  drawState.dragging = false;
  setStatus('', '');
}

function undoDraw() {
  if (!drawModeActive) return;
  if (drawState.shapes.length === 0) return;
  drawState.shapes.pop();
  scheduleDrawRender();
}

function scheduleDrawRender() {
  if (drawState.rafScheduled) return;
  drawState.rafScheduled = true;
  requestAnimationFrame(() => {
    drawState.rafScheduled = false;
    renderDrawOverlay();
  });
}

function renderDrawOverlay() {
  const octx = drawOverlay.getContext('2d');
  octx.clearRect(0, 0, drawOverlay.width, drawOverlay.height);
  for (const s of drawState.shapes) drawShape(octx, s);
  if (drawState.current) drawShape(octx, drawState.current);
}

function drawShape(octx, s) {
  octx.save();
  octx.strokeStyle = s.color;
  octx.fillStyle = s.color;
  octx.lineWidth = s.width;
  octx.lineCap = 'round';
  octx.lineJoin = 'round';
  if (s.tool === 'rect' || s.tool === 'rect-fill') {
    const x = Math.min(s.x1, s.x2), y = Math.min(s.y1, s.y2);
    const w = Math.abs(s.x2 - s.x1), h = Math.abs(s.y2 - s.y1);
    if (s.tool === 'rect-fill') octx.fillRect(x, y, w, h);
    else octx.strokeRect(x, y, w, h);
  } else if (s.tool === 'arrow') {
    const { x1, y1, x2, y2 } = s;
    octx.beginPath();
    octx.moveTo(x1, y1);
    octx.lineTo(x2, y2);
    octx.stroke();
    // Triangular arrowhead.
    const dx = x2 - x1, dy = y2 - y1;
    const len = Math.hypot(dx, dy);
    if (len > 0.5) {
      const ux = dx / len, uy = dy / len;
      const head = drawState.arrowHeadPx + s.width;
      // Two wings perpendicular to the line.
      const px = -uy, py = ux;
      const bx = x2 - ux * head, by = y2 - uy * head;
      const wing = head * 0.55;
      octx.beginPath();
      octx.moveTo(x2, y2);
      octx.lineTo(bx + px * wing, by + py * wing);
      octx.lineTo(bx - px * wing, by - py * wing);
      octx.closePath();
      octx.fill();
    }
  } else if (s.tool === 'marker') {
    if (s.points.length < 2) { octx.restore(); return; }
    octx.beginPath();
    octx.moveTo(s.points[0][0], s.points[0][1]);
    // Smoothing: quadratic midpoints between consecutive points.
    for (let i = 1; i < s.points.length - 1; i++) {
      const [x, y] = s.points[i];
      const [nx, ny] = s.points[i + 1];
      octx.quadraticCurveTo(x, y, (x + nx) / 2, (y + ny) / 2);
    }
    const [lx, ly] = s.points[s.points.length - 1];
    octx.lineTo(lx, ly);
    octx.stroke();
  }
  octx.restore();
}

async function applyDraw() {
  if (drawState.shapes.length === 0) {
    setStatus('Draw something first, or Cancel.', 'err');
    return;
  }
  // Single pass to rasterise all shapes onto the main canvas.
  for (const s of drawState.shapes) drawShape(ctx, s);
  currentBlob = await canvasToPngBlob(canvas);
  bannerBaked = true;
  noteInput.disabled = true;
  noteInput.placeholder = 'Annotated — note locked';
  const count = drawState.shapes.length;
  exitDrawMode();
  setStatus(`Annotated ${count} shape${count > 1 ? 's' : ''}. Copy/Download now export the annotated image.`, 'ok');
}

// --- Text mode ------------------------------------------------------------
// Add text labels to the preview canvas. Single-line per label; multiple
// labels accumulate in a shape array (mirrors draw mode). Click to spawn
// an inline <input> at that point; Enter commits, Esc cancels just that
// edit. User text is rendered via ctx.fillText() — glyphs only, never
// HTML — and read via inputEl.value, capped at TEXT_MAX_LEN. Like the
// other edit modes, never mutates the stored blob.

const TEXT_SIZE_MIN = 12;
const TEXT_SIZE_MAX = 56;
const TEXT_SIZE_DEFAULT = 24;
const TEXT_MAX_LEN = 500;
const TEXT_FONT_FAMILY = '-apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif';

const textState = {
  color: '#ff3b30',
  sizePx: TEXT_SIZE_DEFAULT,  // CSS px in [TEXT_SIZE_MIN, TEXT_SIZE_MAX]; * dpr at draw time
  shapes: [],          // committed: { x, y, text, color, fontPx }
  current: null,       // in-flight: { el, x, y } — colour/size read live from textState
  rafScheduled: false,
  listeners: null
};

function enterTextMode() {
  if (textModeActive || !currentBlob) return;
  if (!ensureExitOtherModes('text')) return;
  textModeActive = true;
  document.body.classList.add('texting');

  fitOverlayToCanvas(textOverlay);
  textOverlay.hidden = false;
  textToolbar.hidden = false;
  textState.shapes = [];
  textState.current = null;
  initTextToolUI();
  scheduleTextRender();

  const onDown = (e) => {
    if (e.button !== 0) return;
    // If a click lands on the existing inline input, let the input handle
    // it (don't spawn a new one).
    if (textState.current && textState.current.el && textState.current.el.contains(e.target)) return;
    // Commit any existing in-flight input before starting a new one.
    if (textState.current) commitInlineText();
    const p = overlayEventToCanvasXY(textOverlay, e);
    spawnInlineInput(p.x, p.y);
    e.preventDefault();
  };
  textOverlay.addEventListener('pointerdown', onDown);
  textState.listeners = { onDown };
  setStatus('Text mode. Click on the image, type, Enter commits. Esc cancels.', 'info');
}

function initTextToolUI() {
  const slider = document.getElementById('text-size');
  const valueEl = document.getElementById('text-size-value');
  if (slider) {
    slider.min = String(TEXT_SIZE_MIN);
    slider.max = String(TEXT_SIZE_MAX);
    slider.value = String(textState.sizePx);
    if (valueEl) valueEl.textContent = String(textState.sizePx);
    slider.oninput = () => {
      const n = Math.max(TEXT_SIZE_MIN, Math.min(TEXT_SIZE_MAX, Number(slider.value) || TEXT_SIZE_DEFAULT));
      textState.sizePx = n;
      if (valueEl) valueEl.textContent = String(n);
      if (textState.current && textState.current.el) restyleInlineInput();
    };
  }
  const colorBtns = textToolbar.querySelectorAll('[data-text-color]');
  colorBtns.forEach((b) => {
    b.setAttribute('aria-pressed', b.dataset.textColor === textState.color ? 'true' : 'false');
    b.onclick = () => {
      textState.color = b.dataset.textColor;
      colorBtns.forEach((bb) => bb.setAttribute('aria-pressed', bb === b ? 'true' : 'false'));
      if (textState.current && textState.current.el) restyleInlineInput();
    };
  });
}

function spawnInlineInput(canvasX, canvasY) {
  const el = document.createElement('input');
  el.type = 'text';
  el.autocomplete = 'off';
  el.spellcheck = false;
  el.maxLength = TEXT_MAX_LEN;
  el.className = 'text-edit-input';
  // CSS-pixel position. `canvas.offsetLeft / offsetTop` are the canvas's
  // position within #canvas-wrap (its offsetParent — wrap has
  // `position: relative`). That handles `margin: 0 auto` centring on snip
  // captures correctly. Then add the click point in canvas pixels scaled
  // back to CSS pixels.
  const bcr = canvas.getBoundingClientRect();
  const scaleX = bcr.width / canvas.width;
  const scaleY = bcr.height / canvas.height;
  el.style.left = `${canvas.offsetLeft + canvasX * scaleX}px`;
  el.style.top  = `${canvas.offsetTop  + canvasY * scaleY}px`;

  textState.current = { el, x: canvasX, y: canvasY };
  restyleInlineInput();

  el.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      e.stopPropagation();
      commitInlineText();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      cancelInlineText();
    }
  });
  // Treat blur as commit so a click elsewhere finalises the text rather
  // than dropping it. Use a small delay so the click that caused the blur
  // (e.g. on a colour swatch or another spot on the canvas) gets to run
  // its own logic first.
  el.addEventListener('blur', () => {
    setTimeout(() => {
      if (textState.current && textState.current.el === el) commitInlineText();
    }, 0);
  });

  canvasWrap.appendChild(el);
  el.focus();
}

function restyleInlineInput() {
  const cur = textState.current;
  if (!cur || !cur.el) return;
  cur.el.style.fontSize = `${textState.sizePx}px`;
  cur.el.style.color = textState.color;
}

function commitInlineText() {
  const cur = textState.current;
  if (!cur || !cur.el) return;
  // Defensive cap (maxlength on the element should already enforce this).
  const raw = (cur.el.value || '').slice(0, TEXT_MAX_LEN);
  const text = raw.trim();
  if (text.length === 0) {
    cancelInlineText();
    return;
  }
  textState.shapes.push({
    x: cur.x,
    y: cur.y,
    text,
    color: textState.color,
    fontPx: textState.sizePx * (meta?.dpr || 1)
  });
  cur.el.remove();
  textState.current = null;
  scheduleTextRender();
}

function cancelInlineText() {
  const cur = textState.current;
  if (!cur || !cur.el) return;
  cur.el.remove();
  textState.current = null;
}

function exitTextMode() {
  if (!textModeActive) return;
  if (textState.current) cancelInlineText();
  textModeActive = false;
  document.body.classList.remove('texting');
  textOverlay.hidden = true;
  textToolbar.hidden = true;
  if (textState.listeners) {
    textOverlay.removeEventListener('pointerdown', textState.listeners.onDown);
    textState.listeners = null;
  }
  textState.shapes = [];
  setStatus('', '');
}

function undoText() {
  if (!textModeActive) return;
  if (textState.shapes.length === 0) return;
  textState.shapes.pop();
  scheduleTextRender();
}

function scheduleTextRender() {
  if (textState.rafScheduled) return;
  textState.rafScheduled = true;
  requestAnimationFrame(() => {
    textState.rafScheduled = false;
    renderTextOverlay();
  });
}

function renderTextOverlay() {
  const octx = textOverlay.getContext('2d');
  octx.clearRect(0, 0, textOverlay.width, textOverlay.height);
  for (const s of textState.shapes) drawTextShape(octx, s);
}

function drawTextShape(octx, s) {
  octx.save();
  octx.font = `${s.fontPx}px ${TEXT_FONT_FAMILY}`;
  octx.fillStyle = s.color;
  octx.textBaseline = 'top';
  // fillText rasterises glyphs from a plain string — never parses HTML.
  octx.fillText(s.text, s.x, s.y);
  octx.restore();
}

async function applyText() {
  // Commit any in-flight input first so a user pressing Apply without
  // pressing Enter doesn't lose their last label.
  if (textState.current) commitInlineText();
  if (textState.shapes.length === 0) {
    setStatus('Type something first, or Cancel.', 'err');
    return;
  }
  for (const s of textState.shapes) drawTextShape(ctx, s);
  currentBlob = await canvasToPngBlob(canvas);
  bannerBaked = true;
  noteInput.disabled = true;
  noteInput.placeholder = 'Annotated — note locked';
  const count = textState.shapes.length;
  exitTextMode();
  setStatus(`Annotated ${count} text label${count > 1 ? 's' : ''}. Copy/Download now export the annotated image.`, 'ok');
}

// --- Actions --------------------------------------------------------------

async function copyToClipboard() {
  if (!currentBlob) throw new Error('No image rendered yet');
  await writeBlobToClipboard(currentBlob);
  await saveToHistoryOnce();
  setStatus('Copied to clipboard.', 'ok');
}

async function writeBlobToClipboard(blob) {
  await navigator.clipboard.write([new ClipboardItem({ 'image/png': blob })]);
}

async function downloadPng() {
  if (!currentBlob) throw new Error('No image rendered yet');
  const filename = buildFilename().split('/').pop();
  const path = await saveDialog({
    defaultPath: filename,
    filters: [{ name: 'PNG image', extensions: ['png'] }],
  });
  if (!path) return; // user cancelled
  const dataUrl = await blobToDataUrl(currentBlob);
  await invoke('save_file', { path, pngBase64: dataUrl });
  await saveToHistoryOnce();
  setStatus(`Saved ${path}`, 'ok');
}

// Persist the current (banner + edits) artifact to local history — at most once
// per editor session, and never for a capture opened FROM history.
async function saveToHistoryOnce() {
  if (historySaved || isHistoryMode || !settings.historyEnabled || !currentBlob) return;
  historySaved = true;
  try {
    const fullDataUrl = await blobToDataUrl(currentBlob);
    const thumbDataUrl = await makeThumbnailDataUrl(currentBlob, 320);
    await invoke('save_capture', {
      meta: {
        url: meta.url || '',
        title: meta.title || '',
        mode: meta.mode || '',
        imageWidthPx: canvas.width,
        imageHeightPx: canvas.height,
        dpr: meta.dpr || 1,
      },
      fullPngBase64: fullDataUrl,
      thumbPngBase64: thumbDataUrl,
    });
  } catch (err) {
    historySaved = false; // allow a retry on the next action
    console.warn('save to history failed:', err);
  }
}

function buildFilename() {
  const ts = new Date(meta.capturedAt);
  const pad = (n) => String(n).padStart(2, '0');
  const stamp = `${ts.getFullYear()}-${pad(ts.getMonth() + 1)}-${pad(ts.getDate())}-${pad(ts.getHours())}${pad(ts.getMinutes())}${pad(ts.getSeconds())}`;
  const base = `${settings.filenamePrefix}-${stamp}.png`;
  const subdir = (settings.downloadSubdir || '').trim().replace(/^\/+|\/+$/g, '');
  return subdir ? `${subdir}/${base}` : base;
}

async function retry() {
  setStatus('Re-capturing…', 'info');
  await invoke('trigger_capture', { mode: meta.mode });
  // The capture flow refreshes the pending payload; reload to pick it up.
  location.reload();
}

function canvasToPngBlob(cvs) {
  return new Promise((resolve, reject) => {
    cvs.toBlob((blob) => blob ? resolve(blob) : reject(new Error('Canvas export failed')), 'image/png');
  });
}

// --- Glyphio helpers ------------------------------------------------------

function dataUrlToBlob(dataUrl) {
  return fetch(dataUrl).then((r) => r.blob());
}

function blobToDataUrl(blob) {
  return new Promise((resolve, reject) => {
    const fr = new FileReader();
    fr.onload = () => resolve(fr.result);
    fr.onerror = () => reject(fr.error);
    fr.readAsDataURL(blob);
  });
}

// 320px-wide thumbnail (aspect preserved), mirroring Checkpoint's makeThumbnail.
async function makeThumbnailDataUrl(blob, targetW) {
  const bmp = await createImageBitmap(blob);
  const scale = targetW / bmp.width;
  const w = Math.max(1, Math.round(bmp.width * scale));
  const h = Math.max(1, Math.round(bmp.height * scale));
  const oc = document.createElement('canvas');
  oc.width = w;
  oc.height = h;
  oc.getContext('2d').drawImage(bmp, 0, 0, w, h);
  bmp.close?.();
  return oc.toDataURL('image/png');
}

function setStatus(text, kind = '') {
  statusEl.textContent = text;
  statusEl.className = `status ${kind}`;
}
