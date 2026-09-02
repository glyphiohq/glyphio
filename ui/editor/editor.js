// preview/preview.js
// Preview tab: reads the stitched content PNG from Cache Storage, composes a
// banner (timestamp + URL + note) on top, and offers Copy / Download / Retry.
// Loaded as an ES module so the editor can share its presentation defaults.

import { DEFAULT_EDITOR_SHORTCUTS, PRODUCT_NAME } from '../shared/presentation.js';
import { EditableCaptureArtifact } from './artifact.mjs';
import { matchesShortcut, formatShortcut, IS_MAC } from '../shared/shortcuts.js';
import { compositeBanner } from '../shared/banner.js';
import { icon } from '../shared/icons.js';

// Glyphio: Tauri bridge (chrome.* APIs replaced by native commands/plugins).
const { invoke } = window.__TAURI__.core;
const { save: saveDialog } = window.__TAURI__.dialog;

const LAST_NOTE_KEY = 'glyphio.lastNote';
let historySaved = false; // save the final artifact to history at most once per session

// --- DOM references -------------------------------------------------------

const canvas = document.getElementById('canvas');
const ctx = canvas.getContext('2d');
const noteInput = document.getElementById('note');
const bannerToggle = document.getElementById('banner-toggle');
const copyBtn = document.getElementById('copy');
const downloadBtn = document.getElementById('download');
const retryBtn = document.getElementById('retry');
const discardBtn = document.getElementById('discard');
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
const selectTextBtn = document.getElementById('select-text');
const ocrToolbar = document.getElementById('ocr-toolbar');
const ocrOverlay = document.getElementById('ocr-overlay');
const ocrCopyAllBtn = document.getElementById('ocr-copy-all');
const ocrDoneBtn = document.getElementById('ocr-done');
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
const deliverySessionId = urlParams.get('session') || '';
const isHistoryMode = Boolean(historyId);
// Silent capture: this page is running in a window that is never shown. It composites,
// copies, saves to history and reports back — see `windows::open_silent_editor`.
const isSilent = urlParams.get('silent') === '1';

// --- State ---------------------------------------------------------------

let meta = null;            // json meta (from cache or history row)
// Content-only pixels (crop/redact/draw baked in, NO banner). The visible #canvas is always
// a composite of banner + contentCanvas, so the banner/note stay editable after any edit,
// and history persists the content separately from the banner.
let contentCanvas = null;
let contentCtx = null;
let artifact = null;
let settings = null;        // native Settings snapshot
let currentBlob = null;     // current composited (banner+content) PNG Blob
let bannerEnabled = true;   // per-capture banner on/off (persisted on history rows)
let lastBannerPxH = 0;      // banner height of the last composite, for overlay→content coords
let savedId = '';           // history row id once saved — later edits update it in place
let autoCopyDone = false;
let noteTimer = null;
// Legacy history rows only (saved before the content/banner split): the stored PNG has the
// banner baked in, so banner + note edits are locked and edits are view-only (not persisted).
let bannerBaked = false;
let cropModeActive = false;
let redactModeActive = false;
let drawModeActive = false;
let textModeActive = false;
let ocrModeActive = false;
let ocrLines = [];          // recognized lines of the current OCR pass ({text,x,y,w,h})

fillIcons();
init().catch((err) => {
  // A silent capture has no window to show an error in — hand it back to the app, which
  // reports it the same way any other failed capture is reported.
  if (isSilent) reportSilent(err.message || String(err));
  else setStatus(err.message, 'err');
});

// Populate every [data-ico] control from the shared icon set. Icon-only buttons (.iconbtn)
// get the glyph alone; menu items keep their text label with the glyph in front.
function fillIcons() {
  for (const el of document.querySelectorAll('[data-ico]')) {
    const svg = icon(el.dataset.ico, 18);
    if (el.classList.contains('iconbtn')) {
      el.innerHTML = svg;
    } else {
      el.innerHTML = `${icon(el.dataset.ico, 15)}<span>${el.textContent.trim()}</span>`;
    }
  }
}

async function init() {
  settings = await loadSettings();
  renderShortcutHint();
  // The silent worker is parked in this page ahead of time, so on load there is usually no
  // capture waiting: it sits idle until a capture reloads it. See `ensure_silent_editor`.
  if (isSilent && !(await loadPayload({ optional: true }))) return;
  if (!isSilent) await loadPayload();
  document.title = `${PRODUCT_NAME} — ${meta.title || meta.windowTitle}`;

  if (isSilent) {
    await render({ autoCopy: false });
    await writeBlobToClipboard(currentBlob);
    await saveToHistoryOnce();
    reportSilent(null);
    releaseCapture();
    return;
  }

  if (isHistoryMode) {
    savedId = historyId;
    bannerEnabled = meta.bannerEnabled !== false;
    if (!meta.bannerBaked) noteInput.value = meta.note || '';
  } else {
    // Pre-fill the note field with the last one the user typed this session.
    const lastNote = localStorage.getItem(LAST_NOTE_KEY);
    if (lastNote) noteInput.value = lastNote;
  }
  bannerToggle.checked = bannerEnabled;

  wireEvents();

  if (isHistoryMode && meta.bannerBaked) {
    // Legacy row: stored image already has the banner baked in — display it directly.
    await displayStored();
  } else {
    await render({ autoCopy: !isHistoryMode && settings.autoCopyOnOpen });
    // Every capture lands in history as soon as it exists (when history is enabled) —
    // not only after a manual Copy/Download. Note, banner and edit changes then persist
    // to the row automatically.
    await saveToHistoryOnce();
  }
}

async function loadSettings() {
  return invoke('get_settings');
}

// Tell the app the silent capture is over; it closes this window and either acknowledges the
// capture in the menu bar or shows the error. Best-effort: if the call itself fails there is
// nothing left to try, and the window has a watchdog behind it.
function reportSilent(error) {
  invoke('capture_done_silently', { error }).catch((e) => console.error('silent report failed', e));
}

/**
 * Load the capture this page is for. Returns false only when `optional` and there is nothing
 * pending — the parked silent worker's normal state between captures.
 */
async function loadPayload({ optional = false } = {}) {
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
      windowTitle: row.title || row.url,
      pageTitle: row.pageTitle || '',
      pageUrl: row.pageUrl || '',
      profile: row.profile || '',
      title: row.title,
      mode: row.mode,
      imageWidthPx: row.imageWidthPx,
      imageHeightPx: row.imageHeightPx,
      dpr: row.dpr || 1,
      note: row.note || '',
      bannerEnabled: row.bannerEnabled !== false,
      bannerBaked: Boolean(row.bannerBaked),
    };
    await setContentFromBlob(await dataUrlToBlob(dataUrl));
  } else {
    if (!deliverySessionId) {
      if (optional) return false;
      throw new Error('No delivery session. Trigger a capture from the tray or a hotkey.');
    }
    const p = await invoke('take_pending_capture', {
      sessionId: deliverySessionId,
      silent: isSilent,
    });
    if (!p) {
      if (optional) return false;
      throw new Error('No pending capture. Trigger a capture from the tray or a hotkey.');
    }
    meta = {
      capturedAt: p.capturedAt,
      windowTitle: p.title,
      pageTitle: p.pageTitle || '',
      pageUrl: p.pageUrl || '',
      profile: p.profile || '',
      title: p.title,
      mode: p.mode,
      imageWidthPx: p.width,
      imageHeightPx: p.height,
      dpr: p.dpr || 1,
      bannerBaked: false,
    };
    await setContentFromBlob(await dataUrlToBlob(p.pngDataUrl));
  }
  const modeLabel = isHistoryMode ? `history · ${meta.mode}` : meta.mode;
  metaLineEl.textContent =
    `${modeLabel} · ${meta.imageWidthPx}×${meta.imageHeightPx}px · ${meta.pageUrl || meta.windowTitle}`;
  return true;
}

// Drop a finished silent capture. The worker window stays parked for the next one, and a
// stitched page can be tens of megabytes of canvas — not something to sit on until then.
function releaseCapture() {
  contentCanvas = null;
  contentCtx = null;
  artifact = null;
  currentBlob = null;
  canvas.width = canvas.height = 0;
}

// Decode a PNG blob into the content canvas (detached; the visible canvas composites it).
async function setContentFromBlob(blob) {
  const bmp = await createImageBitmap(blob);
  contentCanvas = document.createElement('canvas');
  contentCanvas.width = bmp.width;
  contentCanvas.height = bmp.height;
  contentCtx = contentCanvas.getContext('2d');
  contentCtx.drawImage(bmp, 0, 0);
  bmp.close?.();
  artifact = new EditableCaptureArtifact({
    content: contentCanvas,
    metadata: meta,
    banner: {
      note: meta.note || '',
      enabled: meta.bannerEnabled !== false,
      baked: Boolean(meta.bannerBaked),
    },
  });
}

function artifactState() {
  if (!artifact || !contentCanvas) return null;
  artifact.replaceContent(contentCanvas);
  artifact.setBanner({
    note: noteInput.value.trim(),
    enabled: bannerEnabled,
    baked: bannerBaked,
  });
  return artifact.persistence();
}

// Legacy history row: the stored PNG already contains the banner, so draw it straight
// onto the canvas (bypassing render()'s banner composition) and lock editing.
async function displayStored() {
  canvas.width = contentCanvas.width;
  canvas.height = contentCanvas.height;
  ctx.drawImage(contentCanvas, 0, 0);
  currentBlob = await canvasToPngBlob(canvas);
  bannerBaked = true;
  artifactState();
}

function wireEvents() {
  // Retry re-runs the original capture — impossible for a stored history entry, so hide it
  // entirely rather than show a dead button.
  if (isHistoryMode) retryBtn.hidden = true;

  if (isHistoryMode && meta.bannerBaked) {
    // Legacy history row (banner baked into the PNG): note + banner are locked.
    noteInput.disabled = true;
    noteInput.placeholder = 'Saved before editable history — note is read-only';
    bannerToggle.disabled = true;
  } else if (isHistoryMode) {
    // Editable history row: note + banner edits persist to the stored entry.
    // The banner timestamp always renders from the original captured_at.
    noteInput.addEventListener('input', () => {
      clearTimeout(noteTimer);
      noteTimer = setTimeout(() => {
        render({ autoCopy: false }).then(() => persistMeta());
      }, 300);
    });
  } else {
    noteInput.addEventListener('input', () => {
      clearTimeout(noteTimer);
      noteTimer = setTimeout(() => {
        render({ autoCopy: false }).then(() => persistMeta());
        localStorage.setItem(LAST_NOTE_KEY, noteInput.value);
      }, 200);
    });
    retryBtn.addEventListener('click', () => retry().catch((e) => setStatus(e.message, 'err')));
  }

  bannerToggle.addEventListener('change', () => {
    bannerEnabled = bannerToggle.checked;
    render({ autoCopy: false }).then(() => persistMeta()).catch((e) => setStatus(e.message, 'err'));
  });

  copyBtn.addEventListener('click', () => copyToClipboard().catch((e) => setStatus(e.message, 'err')));
  downloadBtn.addEventListener('click', () => downloadPng().catch((e) => setStatus(e.message, 'err')));
  historyBtn.addEventListener('click', () => invoke('open_history_view'));
  optionsBtn.addEventListener('click', () => invoke('open_window', { name: 'settings' }));
  discardBtn.addEventListener('click', () => discardCapture());

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

  selectTextBtn.addEventListener('click', () => {
    if (ocrModeActive) exitOcrMode();
    else enterOcrMode().catch((e) => setStatus(e.message || String(e), 'err'));
  });
  ocrDoneBtn.addEventListener('click', () => exitOcrMode());
  // Absolutely-positioned spans copy as one run-on string natively; rebuild the
  // selected text line-by-line so pasted output keeps its layout.
  ocrOverlay.addEventListener('copy', (e) => {
    const sel = window.getSelection();
    if (!sel || sel.isCollapsed || !e.clipboardData) return;
    const range = sel.getRangeAt(0);
    const chunks = [];
    for (const span of ocrOverlay.querySelectorAll('.ocr-line')) {
      const textNode = span.firstChild;
      if (!textNode || !sel.containsNode(span, true)) continue;
      const start = range.startContainer === textNode ? range.startOffset : 0;
      const end = range.endContainer === textNode ? range.endOffset : textNode.length;
      const piece = textNode.data.slice(start, end);
      if (piece) chunks.push(piece);
    }
    if (!chunks.length) return;
    e.clipboardData.setData('text/plain', chunks.join('\n'));
    e.preventDefault();
  });
  ocrCopyAllBtn.addEventListener('click', async () => {
    try {
      await navigator.clipboard.writeText(ocrLines.map((l) => l.text).join('\n'));
      setStatus(`Copied ${ocrLines.length} line${ocrLines.length === 1 ? '' : 's'} of text.`, 'ok');
    } catch (e) { setStatus(String(e), 'err'); }
  });

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
  const s = DEFAULT_EDITOR_SHORTCUTS;

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
  if (ocrModeActive && e.key === 'Escape') { e.preventDefault(); exitOcrMode(); return; }

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
  const s = DEFAULT_EDITOR_SHORTCUTS;
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
  const state = artifactState();
  if (!state) return;
  // Legacy history rows never re-render — the stored PNG (banner baked in) is the image.
  if (state.banner.baked) return;

  // Recompositing changes the canvas geometry — a live OCR overlay would misalign.
  if (ocrModeActive) exitOcrMode();

  lastBannerPxH = artifact.render(({ content, metadata, banner }) => compositeBanner(canvas, content, {
    meta: metadata,
    settings,
    note: banner.note,
    enabled: banner.enabled,
  }));

  currentBlob = await canvasToPngBlob(canvas);

  if (autoCopy && !autoCopyDone) {
    autoCopyDone = true;
    try {
      await writeBlobToClipboard(currentBlob);
      setStatus('Copied to clipboard.', 'ok');
    } catch (err) {
      setStatus(`Ready. Click "Copy to clipboard" (copy failed: ${err.message || err})`, 'info');
    }
  }
}

// Banner layout/drawing lives in ../shared/banner.js (shared with the history list).

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
  // OCR selection holds no unsaved work — always safe to leave silently.
  if (target !== 'ocr' && ocrModeActive) exitOcrMode();
  return true;
}

// --- Crop -----------------------------------------------------------------
// Cropping tool. The selection is drawn over the composited canvas; Apply maps it into
// content coordinates and rewrites contentCanvas (the banner is regenerated on render,
// never cropped). On history rows the stored content PNG is updated in place.

const cropState = {
  dragging: false,
  mode: 'new',  // 'new' (drag a fresh rect) | 'move' | 'resize' (from a corner handle)
  startX: 0,    // anchor point: drag origin, or the fixed opposite corner while resizing
  startY: 0,
  grabDX: 0,    // pointer offset inside the rect while moving
  grabDY: 0,
  rect: null,   // { x, y, w, h } in canvas pixels
  listeners: null
};

// Which part of the current selection a point grabs: a corner handle name, 'move' for the
// interior, or null (outside — starts a new selection).
function cropHandleAt(p) {
  const r = cropState.rect;
  if (!r) return null;
  const tol = Math.max(10, Math.round(10 * (meta?.dpr || 1)));
  const corners = {
    nw: [r.x, r.y], ne: [r.x + r.w, r.y],
    sw: [r.x, r.y + r.h], se: [r.x + r.w, r.y + r.h],
  };
  for (const [name, [cx, cy]] of Object.entries(corners)) {
    if (Math.abs(p.x - cx) <= tol && Math.abs(p.y - cy) <= tol) return name;
  }
  if (p.x > r.x && p.x < r.x + r.w && p.y > r.y && p.y < r.y + r.h) return 'move';
  return null;
}

function enterCropMode() {
  if (cropModeActive || !currentBlob) return;
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
    const grab = cropHandleAt(p);
    cropState.dragging = true;
    if (grab === 'move') {
      cropState.mode = 'move';
      cropState.grabDX = p.x - cropState.rect.x;
      cropState.grabDY = p.y - cropState.rect.y;
    } else if (grab) {
      // Resize from a corner: anchor the OPPOSITE corner and drag as usual.
      const r = cropState.rect;
      cropState.mode = 'resize';
      cropState.startX = grab.includes('w') ? r.x + r.w : r.x;
      cropState.startY = grab.includes('n') ? r.y + r.h : r.y;
      cropState.rect = rectFromPoints(cropState.startX, cropState.startY, p.x, p.y);
    } else {
      cropState.mode = 'new';
      cropState.startX = p.x;
      cropState.startY = p.y;
      cropState.rect = { x: p.x, y: p.y, w: 0, h: 0 };
    }
    drawCropOverlay();
    e.preventDefault();
  };
  const onMove = (e) => {
    if (!cropState.dragging) return;
    const p = eventToCanvasXY(e);
    if (cropState.mode === 'move') {
      const r = cropState.rect;
      r.x = Math.max(0, Math.min(p.x - cropState.grabDX, cropOverlay.width - r.w));
      r.y = Math.max(0, Math.min(p.y - cropState.grabDY, cropOverlay.height - r.h));
    } else {
      cropState.rect = rectFromPoints(cropState.startX, cropState.startY, p.x, p.y);
    }
    drawCropOverlay();
  };
  const onUp = () => { cropState.dragging = false; };
  cropOverlay.addEventListener('pointerdown', onDown);
  window.addEventListener('pointermove', onMove);
  window.addEventListener('pointerup', onUp);
  cropState.listeners = { onDown, onMove, onUp };
  setStatus('Crop mode. Drag on the image — corners resize, inside moves. Enter applies, Esc cancels.', 'info');
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
  // Corner drag handles.
  const hs = Math.max(6, Math.round(6 * (meta?.dpr || 1)));
  octx.fillStyle = '#60a5fa';
  for (const [cx, cy] of [[x, y], [x + w, y], [x, y + h], [x + w, y + h]]) {
    octx.fillRect(cx - hs / 2, cy - hs / 2, hs, hs);
  }
}

/// Map a rect from composited-canvas coords to content coords, clipped to the content
/// bounds (the banner region above y=lastBannerPxH is regenerated, never cropped/edited).
function toContentRect(r) {
  const x = Math.max(0, r.x);
  const y = Math.max(0, r.y - lastBannerPxH);
  const right = Math.min(r.x + r.w, contentCanvas.width);
  const bottom = Math.min(r.y + r.h - lastBannerPxH, contentCanvas.height);
  if (right - x < 1 || bottom - y < 1) return null;
  return { x, y, w: right - x, h: bottom - y };
}

// Recomposite + persist after an edit landed on contentCanvas. Legacy rows (banner baked)
// instead refresh the flat canvas — their edits stay view-only, exactly as before.
async function refreshAfterEdit() {
  if (bannerBaked) {
    canvas.width = contentCanvas.width;
    canvas.height = contentCanvas.height;
    ctx.drawImage(contentCanvas, 0, 0);
    currentBlob = await canvasToPngBlob(canvas);
    return;
  }
  await render({ autoCopy: false });
  await persistContent();
}

async function applyCrop() {
  const raw = cropState.rect;
  if (!raw || raw.w < 4 || raw.h < 4) {
    setStatus('Drag a region first.', 'err');
    return;
  }
  const r = toContentRect(raw);
  if (!r || r.w < 4 || r.h < 4) {
    setStatus('The selection must include image content (the timestamp strip is regenerated, not cropped).', 'err');
    return;
  }
  // Snapshot the content pixels, resize the content canvas, redraw at origin.
  const tmp = new OffscreenCanvas(r.w, r.h);
  tmp.getContext('2d').drawImage(contentCanvas, r.x, r.y, r.w, r.h, 0, 0, r.w, r.h);
  contentCanvas.width = r.w;
  contentCanvas.height = r.h;
  contentCtx.drawImage(tmp, 0, 0);
  exitCropMode();
  await refreshAfterEdit();
  setStatus(`Cropped to ${r.w}×${r.h}px.`, 'ok');
}

// --- Redact mode ----------------------------------------------------------
// Paint black boxes or Gaussian-blurred regions over the current canvas.
// Mirrors cropMode but supports multiple rectangles. Like crop, Apply lands on
// contentCanvas and (on saved/history rows) persists — redaction is an intentional,
// permanent removal of sensitive pixels. Legacy banner-baked rows stay view-only.

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

  // Regions land on the content canvas — clip away any part covering the banner.
  const contentRects = rects.map(toContentRect).filter(Boolean);
  if (contentRects.length === 0) {
    setStatus('The regions must cover image content (the timestamp strip is regenerated, not stored).', 'err');
    return;
  }

  if (mode === 'black') {
    contentCtx.save();
    contentCtx.fillStyle = '#000';
    for (const r of contentRects) contentCtx.fillRect(r.x, r.y, r.w, r.h);
    contentCtx.restore();
  } else {
    // Blur: copy each rect through a Canvas2D blur filter back onto itself.
    // Snapshot first so we read from a stable source even when rects overlap.
    const snap = new OffscreenCanvas(contentCanvas.width, contentCanvas.height);
    snap.getContext('2d').drawImage(contentCanvas, 0, 0);
    contentCtx.save();
    contentCtx.filter = 'blur(12px)';
    for (const r of contentRects) {
      contentCtx.drawImage(snap, r.x, r.y, r.w, r.h, r.x, r.y, r.w, r.h);
    }
    contentCtx.restore();
  }

  exitRedactMode();
  await refreshAfterEdit();
  setStatus(`Redacted ${contentRects.length} region${contentRects.length > 1 ? 's' : ''} (${mode}).`, 'ok');
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
// Like crop / redact, Apply lands on contentCanvas and persists to the saved
// history row (legacy banner-baked rows stay view-only).

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
    const cur = drawState.current;
    if (cur.tool === 'marker') {
      if (e.shiftKey) {
        // Shift turns freehand into a ruler: the stroke becomes a straight segment from
        // where it began to the cursor. Releasing Shift resumes freehand from that point,
        // so you can rule a line and keep scribbling in one gesture.
        cur.points = [cur.points[0], [p.x, p.y]];
      } else {
        // Throttle point adds: only record a point if it's moved a few pixels
        // from the last one. Cuts stored points by ~5-10x on fast sweeps
        // without visibly changing stroke quality.
        const pts = cur.points;
        const [lx, ly] = pts[pts.length - 1];
        if (Math.abs(p.x - lx) + Math.abs(p.y - ly) >= 2) pts.push([p.x, p.y]);
      }
    } else if (e.shiftKey) {
      // Same modifier, the meaning every drawing tool gives it: squares for rectangles,
      // 45°-snapped angles for arrows.
      const [x, y] = cur.tool === 'arrow'
        ? snapToAngle(cur.x1, cur.y1, p.x, p.y)
        : squareOff(cur.x1, cur.y1, p.x, p.y);
      cur.x2 = x;
      cur.y2 = y;
    } else {
      cur.x2 = p.x;
      cur.y2 = p.y;
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
  setStatus('Draw mode. Pick a tool + colour, then drag — hold ⇧ for straight lines, squares and 45° arrows. Enter to apply, Esc cancels.', 'info');
}

/// Snap (x2,y2) onto the nearest 45° ray from (x1,y1), keeping the drag's length.
function snapToAngle(x1, y1, x2, y2) {
  const dx = x2 - x1;
  const dy = y2 - y1;
  const len = Math.hypot(dx, dy);
  if (len < 1) return [x2, y2];
  const step = Math.PI / 4;
  const angle = Math.round(Math.atan2(dy, dx) / step) * step;
  return [x1 + Math.cos(angle) * len, y1 + Math.sin(angle) * len];
}

/// Force a rectangle to a square, taking the larger drag extent and keeping its direction.
function squareOff(x1, y1, x2, y2) {
  const side = Math.max(Math.abs(x2 - x1), Math.abs(y2 - y1));
  return [x1 + Math.sign(x2 - x1 || 1) * side, y1 + Math.sign(y2 - y1 || 1) * side];
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
  // Single pass to rasterise all shapes onto the content canvas (shifted out of the
  // banner region — anything drawn over the banner clips away, since the banner is
  // regenerated on every render).
  contentCtx.save();
  contentCtx.translate(0, -lastBannerPxH);
  for (const s of drawState.shapes) drawShape(contentCtx, s);
  contentCtx.restore();
  const count = drawState.shapes.length;
  exitDrawMode();
  await refreshAfterEdit();
  setStatus(`Annotated ${count} shape${count > 1 ? 's' : ''}.`, 'ok');
}

// --- Text mode ------------------------------------------------------------
// Add text labels to the preview canvas. Single-line per label; multiple
// labels accumulate in a shape array (mirrors draw mode). Click to spawn
// an inline <input> at that point; Enter commits, Esc cancels just that
// edit. User text is rendered via ctx.fillText() — glyphs only, never
// HTML — and read via inputEl.value, capped at TEXT_MAX_LEN. Like the
// other edit modes, Apply lands on contentCanvas and persists to saved rows.

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
  contentCtx.save();
  contentCtx.translate(0, -lastBannerPxH);
  for (const s of textState.shapes) drawTextShape(contentCtx, s);
  contentCtx.restore();
  const count = textState.shapes.length;
  exitTextMode();
  await refreshAfterEdit();
  setStatus(`Annotated ${count} text label${count > 1 ? 's' : ''}.`, 'ok');
}

// --- Select text (OCR) ------------------------------------------------------
// Live-Text-style selection: recognize text on-device (Vision sidecar), then lay
// transparent, selectable spans over each recognized line so the user can drag-select
// and ⌘C text straight off the image. Runs on the CURRENT content pixels, so crops
// and redactions are respected.

async function enterOcrMode() {
  if (ocrModeActive || !contentCanvas) return;
  if (!ensureExitOtherModes('ocr')) return;

  setStatus('Recognizing text…', 'info');
  selectTextBtn.disabled = true;
  let result;
  try {
    result = await invoke('ocr_image', { pngBase64: contentCanvas.toDataURL('image/png') });
  } finally {
    selectTextBtn.disabled = false;
  }
  ocrLines = (result && Array.isArray(result.lines)) ? result.lines : [];
  if (ocrLines.length === 0) {
    setStatus('No text recognized in this capture.', 'info');
    return;
  }

  ocrModeActive = true;
  ocrOverlay.hidden = false; // must be visible before layout — spans measure at 0 when hidden
  layoutOcrOverlay();
  ocrToolbar.hidden = false;
  setStatus(`Recognized ${ocrLines.length} line${ocrLines.length === 1 ? '' : 's'} — select on the image, ⌘C copies.`, 'ok');
}

/// Position one transparent span per recognized line, in CSS pixels over the canvas.
/// Boxes are normalized to the content image; the banner (when composited above the
/// content) shifts everything down by lastBannerPxH canvas pixels.
function layoutOcrOverlay() {
  const bcr = canvas.getBoundingClientRect();
  const scaleX = bcr.width / canvas.width;
  const scaleY = bcr.height / canvas.height;
  ocrOverlay.style.width = `${bcr.width}px`;
  ocrOverlay.style.height = `${bcr.height}px`;
  ocrOverlay.style.left = `${canvas.offsetLeft}px`;
  ocrOverlay.style.top = `${canvas.offsetTop}px`;
  ocrOverlay.textContent = '';

  const bannerOffset = bannerBaked ? 0 : lastBannerPxH;
  for (const line of ocrLines) {
    const span = document.createElement('span');
    span.className = 'ocr-line';
    span.textContent = line.text;
    const x = line.x * contentCanvas.width * scaleX;
    const y = (bannerOffset + line.y * contentCanvas.height) * scaleY;
    const w = line.w * contentCanvas.width * scaleX;
    const h = line.h * contentCanvas.height * scaleY;
    // Pad the hit area above and below the text band so a drag doesn't have to land
    // pixel-perfectly on a thin line — the text stays vertically centred in the taller box.
    const pad = Math.max(3, h * 0.3);
    span.style.left = `${x}px`;
    span.style.top = `${y - pad}px`;
    span.style.paddingTop = `${pad}px`;
    span.style.paddingBottom = `${pad}px`;
    span.style.fontSize = `${Math.max(4, h * 0.85)}px`;
    span.style.lineHeight = `${h}px`;
    ocrOverlay.appendChild(span);
    // Stretch the glyphs to the recognized box so selection highlights track the pixels.
    // Vertical padding doesn't affect the measured width, so scaleX stays accurate.
    const natural = span.getBoundingClientRect().width;
    if (natural > 0 && w > 0) span.style.transform = `scaleX(${w / natural})`;
  }
}

function exitOcrMode() {
  if (!ocrModeActive) return;
  ocrModeActive = false;
  ocrOverlay.hidden = true;
  ocrToolbar.hidden = true;
  ocrOverlay.textContent = '';
  window.getSelection?.()?.removeAllRanges();
  setStatus('', '');
}

// --- Actions --------------------------------------------------------------

async function copyToClipboard() {
  if (!currentBlob) throw new Error('No image rendered yet');
  await writeBlobToClipboard(currentBlob);
  await saveToHistoryOnce();
  setStatus('Copied to clipboard.', 'ok');
}

// Copy through the OS, not `navigator.clipboard`: the web API only writes during a
// transient user activation (a real click), so copy-on-open — which has no click behind it —
// was always refused with "not allowed by the user agent".
async function writeBlobToClipboard(blob) {
  await invoke('copy_image_to_clipboard', { pngBase64: await blobToDataUrl(blob) });
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

// Persist the capture to local history — at most once per editor session, and never for a
// capture opened FROM history. Stores the CONTENT-ONLY PNG plus banner meta (note, on/off):
// the banner is composited at view/export time, so it stays editable and the original
// captured_at timestamp is preserved verbatim.
async function saveToHistoryOnce() {
  if (historySaved || isHistoryMode || !settings.historyEnabled || !contentCanvas) return;
  const state = artifactState();
  if (!state) return;
  historySaved = true;
  try {
    const contentBlob = await canvasToPngBlob(state.content);
    const fullDataUrl = await blobToDataUrl(contentBlob);
    const thumbDataUrl = await makeThumbnailDataUrl(contentBlob, 320);
    const row = await invoke('save_capture', {
      meta: {
        capturedAt: state.metadata.capturedAt,
        url: state.metadata.windowTitle || '',
        title: state.metadata.title || '',
        pageTitle: state.metadata.pageTitle || '',
        pageUrl: state.metadata.pageUrl || '',
        profile: state.metadata.profile || '',
        mode: state.metadata.mode || '',
        imageWidthPx: state.content.width,
        imageHeightPx: state.content.height,
        dpr: state.metadata.dpr || 1,
        note: state.banner.note,
        bannerEnabled: state.banner.enabled,
      },
      fullPngBase64: fullDataUrl,
      thumbPngBase64: thumbDataUrl,
    });
    savedId = row?.id || '';
  } catch (err) {
    historySaved = false; // allow a retry on the next action
    console.warn('save to history failed:', err);
  }
}

// Push note / banner-toggle changes to the stored history row (history mode, or a live
// session that has already saved once). No-op otherwise — the eventual save picks them up.
async function persistMeta() {
  if (!savedId || bannerBaked) return;
  const state = artifactState();
  if (!state) return;
  try {
    await invoke('update_capture', {
      id: savedId,
      patch: { note: state.banner.note, bannerEnabled: state.banner.enabled },
    });
  } catch (err) {
    console.warn('history meta update failed:', err);
  }
}

// Push an edited content PNG (crop/redact/draw/text) to the stored history row.
async function persistContent() {
  if (!savedId || bannerBaked || !contentCanvas) return;
  const state = artifactState();
  if (!state) return;
  try {
    const contentBlob = await canvasToPngBlob(state.content);
    await invoke('update_capture', {
      id: savedId,
      patch: {
        note: state.banner.note,
        bannerEnabled: state.banner.enabled,
        imageWidthPx: state.content.width,
        imageHeightPx: state.content.height,
      },
      fullPngBase64: await blobToDataUrl(contentBlob),
      thumbPngBase64: await makeThumbnailDataUrl(contentBlob, 320),
    });
  } catch (err) {
    console.warn('history content update failed:', err);
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
  // Re-taking a capture from the editor lands in the editor, whatever the default delivery.
  await invoke('trigger_capture', { mode: meta.mode, silent: false });
  // The native adapter navigates this window to the new delivery-session id.
}

// Discard the current capture: remove it from history (if it was saved) and close the window.
// Works both for a just-taken capture (auto-saved on open) and one opened from history.
async function discardCapture() {
  if (!confirm('Delete this capture? This cannot be undone.')) return;
  try {
    if (savedId) await invoke('delete_capture', { id: savedId });
  } catch (err) {
    setStatus(`Could not delete: ${err.message || err}`, 'err');
    return;
  }
  try { window.close(); } catch { /* fall back to a status note */ }
  setStatus('Capture deleted.', 'ok');
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
