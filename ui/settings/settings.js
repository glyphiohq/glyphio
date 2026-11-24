// settings/settings.js — Glyphio main window.
// Sidebar (groups) → snippet list → editor, plus a Settings view.
// All snippet edits go through the SQLite store; every change regenerates the expansion
// engine's config, which hot-reloads. Rich snippets use the engine's native rich injection.

import { icon } from '../shared/icons.js';
import { IS_MAC } from '../shared/shortcuts.js';
import { isSupportedLocale, isSupportedTimezone } from '../shared/banner.js';
import { escapeHtml, escapeAttr } from '../shared/markdown.js';
import {
  acceleratorFromKeyboardEvent,
  SHORTCUT_DEFAULTS,
  createShortcutCaptureController,
  formatAccelerator,
  setKeyboardLayout,
  validateAccelerator,
} from './shortcut-recorder.mjs';
import { registrationNotice, statusFor } from './shortcut-status.mjs';
import { checklistVisible, grantSurface, PERMISSION_KIND } from './permissions.mjs';
import { formatDiagnostics, shortcutLine, testMessage } from './diagnostics.mjs';
import { state, FORMATS, KINDS } from './model.mjs';
import { el, setStatus, openModal, confirmDialog, promptDialog } from './dom.mjs';

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const PLATFORM_ID = IS_MAC ? 'macos' : 'windows';
const app = document.getElementById('app');
const shortcutCaptureController = createShortcutCaptureController(document);

// The native grant model (permissions.rs), the same one the setup window shows. `null` until
// the first reading arrives.
const grants = {
  [PERMISSION_KIND.ACCESSIBILITY]: null,
  [PERMISSION_KIND.SCREEN_RECORDING]: null,
};


const CAPTURE_DELIVERY_OPTIONS = [
  { value: 'preview', label: 'A small preview (copied)' },
  { value: 'editor', label: 'The editor' },
  { value: 'silent', label: 'Nothing (copied silently)' },
];

const CAPTURE_SECTIONS = [
  { title: 'Capture modes', fields: [
    ['enableVisibleCapture', 'toggle', 'Visible-area (current screen)'],
    ['enableSnipCapture', 'toggle', 'Region / snip (picker)'],
    ['enableFullWindowCapture', 'toggle', 'Full window (picker)'],
    ['enableFrontWindowCapture', 'toggle', 'Frontmost window (no picker — e.g. just the browser)'],
    ['enableScrollingCapture', 'toggle', 'Scrolling page / panel (stitch)'],
  ]},
  { title: 'After a capture', hint: `The preview copies the capture and offers Edit and Discard
    for a few seconds. Silent copies it with nothing on screen — <kbd>⌘↩</kbd> in the palette
    does one on demand. Every capture lands in History.`, fields: [
    ['captureDelivery', 'select', 'Show', () => CAPTURE_DELIVERY_OPTIONS],
    ['historyMaxCount', 'number', 'Max captures kept'],
    ['downloadSubdir', 'text', 'Download subfolder'], ['filenamePrefix', 'text', 'Filename prefix'],
  ]},
  // The strip composited above a capture. Called "timestamp" throughout the UI — that's what
  // it is and what people look for; the stored keys keep their original names.
  { title: 'Timestamp strip', fields: [
    ['showTimestamp', 'toggle', 'Show timestamp'],
    ['timestampFormat', 'select', 'Timestamp format', ['device-locale', 'iso-8601', 'utc-human']],
    ['timezone', 'select', 'Timezone', timezoneOptions],
    ['locale', 'select', 'Locale', localeOptions],
    ['bannerBg', 'color', 'Background'], ['bannerFg', 'color', 'Text'],
    ['bannerMuted', 'color', 'Muted text'],
  ]},
  { title: 'Details on the capture', hint: `Off by default — a URL baked into a screenshot
    travels with it. Only the modes that target one window can record them.`, fields: [
    ['showWindowTitle', 'toggle', 'Window / app title'],
    ['showPageTitle', 'toggle', 'Page title'],
    ['showPageUrl', 'toggle', 'Page address (URL)'],
    ['showBrowserProfile', 'toggle', 'Browser profile'],
  ]},
  { title: 'Edit tools', fields: [
    ['enableCrop', 'toggle', 'Crop'], ['enableRedact', 'toggle', 'Redact'],
    ['enableDraw', 'toggle', 'Draw'], ['enableText', 'toggle', 'Text labels'],
  ]},
  { title: 'Capture shortcuts', hint: `Select a shortcut, then press its keys together. The
    second shortcut takes the same shot straight to the clipboard.`, fields: [
    ['shortcutCaptureFull', 'hotkeys', 'Full window', 'shortcutCaptureFullSilent'],
    ['shortcutCaptureVisible', 'hotkeys', 'Visible area', 'shortcutCaptureVisibleSilent'],
    ['shortcutCaptureSnip', 'hotkeys', 'Region (snip)', 'shortcutCaptureSnipSilent'],
    ['shortcutCaptureFrontWindow', 'hotkeys', 'Frontmost window', 'shortcutCaptureFrontWindowSilent'],
    ['shortcutCapturePage', 'hotkeys', 'Browser page (content only)', 'shortcutCapturePageSilent'],
    ['shortcutCaptureScroll', 'hotkeys', 'Scrolling area', 'shortcutCaptureScrollSilent'],
    ['shortcutCaptureScrollPage', 'hotkeys', 'Scrolling page (frontmost window)', 'shortcutCaptureScrollPageSilent'],
  ]},
];

// The shortcuts that open a Glyphio window live together under General: they are app-wide,
// not tied to one tab's feature. Capture shortcuts stay on Capture, beside their modes.
const GENERAL_SECTIONS = [
  { title: 'Startup', fields: [
    ['launchAtLogin', 'toggle', 'Launch Glyphio at login'],
  ]},
  { title: 'Hotkeys', hint: 'Select a shortcut, then press its keys together.', fields: [
    ['shortcutOpenPalette', 'hotkey', 'Snippet search (Spotlight-style)'],
    ['shortcutOpenClipboard', 'hotkey', 'Open clipboard history'],
    ['shortcutOpenHistory', 'hotkey', 'Open capture history'],
  ]},
];

const EXPANSION_DELIVERY_OPTIONS = () => [
  { value: 'paste', label: 'Paste from clipboard (recommended)' },
  { value: 'keys', label: 'Type individual keys' },
];

const CLIPBOARD_SECTIONS = [
  { title: 'Clipboard history', hint: `Kept on this device, never synced. Content a password
    manager marks as concealed is never recorded.`, fields: [
    ['clipboardHistory', 'toggle', 'Remember what I copy'],
    ['clipboardMaxItems', 'number', 'Entries kept'],
    ['clipboardMaxMb', 'number', 'Megabytes of copied images kept'],
  ]},
  { title: 'Never record from these apps', hint: `One name per line, matched loosely —
    <code>bank</code> catches “Banking”.`, fields: [
    ['clipboardIgnoreApps', 'lines', 'Ignored apps'],
  ]},
];

init().catch((e) => setStatus(e.message, 'err'));

async function init() {
  await refreshKeyboardLayout();
  renderShell();
  await reloadAll();
  // Switching input source happens in another app; coming back is when labels must match.
  window.addEventListener('focus', () => refreshKeyboardLayout().then(repaintShortcutLabels));
  await wirePermissionGuidance();
  await wireSecureInput();
  await wireEngineStatus();
  await wireSync();
  wireInvites();
  maybeShowWelcome();
  await listen('snippets-changed', reloadSnippets);
  await listen('groups-changed', reloadGroups);
  await listen('settings-changed', reloadAll); // tray "Reload" refreshes everything
}

/** Name shortcut keys as the active keyboard layout names them (see shortcut-recorder.mjs). */
async function refreshKeyboardLayout() {
  try {
    const layout = await invoke('keyboard_layout');
    setKeyboardLayout(layout && Object.keys(layout).length ? layout : null);
  } catch {
    setKeyboardLayout(null);
  }
}

/** Relabel every shortcut button that is not recording, after the layout may have changed. */
function repaintShortcutLabels() {
  document.querySelectorAll('button.shortcut-recorder[data-accelerator]:not([data-recording])')
    .forEach((button) => {
      button.textContent = formatAccelerator(button.dataset.accelerator);
    });
}

async function reloadAll() {
  [state.settings, state.snippets, state.groups, state.shortcutStatus] = await Promise.all([
    invoke('get_settings'), invoke('list_snippets'), invoke('list_groups'),
    invoke('shortcut_status').catch(() => []),
  ]);
  renderSidebar();
  renderMain();
  refreshLoginNotice();
}

/** Say why Launch at Login isn't working (approval pending, refused, an old item to remove). */
async function refreshLoginNotice() {
  const input = document.querySelector('[data-key="launchAtLogin"]');
  if (!input) return;
  const field = input.closest('.field');
  let note = field.querySelector('.login-notice');
  if (!note) {
    note = el('p', { className: 'adv-hint login-notice' });
    field.append(note);
  }
  note.textContent = (await invoke('login_item_notice').catch(() => null)) || '';
}
async function reloadSnippets() { state.snippets = await invoke('list_snippets'); renderSidebar(); renderMain(); }
async function reloadGroups() { state.groups = await invoke('list_groups'); renderSidebar(); renderMain(); }

// --- Shell ------------------------------------------------------------------

function renderShell() {
  app.innerHTML = `
    <header class="app-header">
      <h1>Glyphio</h1>
      <div class="spacer"></div>
    </header>
    <section class="permission-panel" id="permission-panel" aria-label="Finish setup" hidden>
      <div class="permission-panel-head">
        <div class="permission-panel-title">Finish setup</div>
        <button type="button" class="ghost sm" data-run-setup>Run setup again</button>
      </div>
      <div data-permission-rows></div>
    </section>
    <div class="ax-banner" id="si-banner">
      <div class="ax-text">
        <strong>Expansion paused — <span id="si-holder">another app</span> is holding macOS Secure Input.</strong>
        <p>While an app captures keystrokes securely (password fields, the lock screen), no expander can see what you type — triggers resume the moment it lets go. If this persists and no password field is open, the hold is stale: lock the screen (Ctrl+Cmd+Q) and unlock, or quit the app shown above.</p>
      </div>
    </div>
    <div class="ax-banner" id="engine-banner">
      <div class="ax-text">
        <strong>The expansion engine stopped after repeated crashes.</strong>
        <p>Text expansion is off until it restarts. <button type="button" id="engine-restart">Restart engine</button></p>
      </div>
    </div>
    <div class="body">
      <aside class="sidebar" id="sidebar"></aside>
      <main class="main" id="main"></main>
    </div>
    <div class="status-line" id="status"></div>
  `;
  document.getElementById('permission-panel').addEventListener('click', handlePermissionAction);
}

// --- Permission guidance ----------------------------------------------------
// One checklist, shared with the setup window: the same grant states and the same single
// action per row. The global panel appears only while something is unresolved; Settings →
// General always shows both rows, so a hidden panel never means "unknown".

function renderPermissionGuidance() {
  const panel = document.getElementById('permission-panel');
  if (!panel) return;
  const surface = grantSurface(grants);
  const inspectingInSettings = state.selected === 'settings' && state.settingsTab === 'general';
  panel.hidden = !checklistVisible(surface, inspectingInSettings);
  const markup = surface.rows.map(([kind, row]) => `
    <div class="perm-row" data-kind="${kind}">
      <div class="perm-info"><strong>${row.name}</strong>
        <span class="perm-sub">${row.explanation || row.description}</span></div>
      <span class="perm-state" data-phase="${row.phase}" role="status" aria-live="polite">${row.status}</span>
      <div class="perm-actions">${row.actions.map((action) =>
        `<button type="button" class="${action.style}" data-permission-action="${action.id}">${action.label}</button>`).join('')}</div>
    </div>`).join('');
  document.querySelectorAll('[data-permission-rows]').forEach((rows) => { rows.innerHTML = markup; });
}

function applyGrants(status) {
  if (!status) return;
  for (const kind of Object.values(PERMISSION_KIND)) grants[kind] = status[kind] ?? grants[kind];
  renderPermissionGuidance();
}

async function refreshPermissionGuidance() {
  try { applyGrants(await invoke('permission_status')); } catch { /* window closing */ }
}

async function handlePermissionAction(event) {
  const setupButton = event.target.closest('[data-run-setup]');
  if (setupButton) {
    try { await invoke('open_setup'); } catch (error) { setStatus(String(error), 'err'); }
    return;
  }
  const button = event.target.closest('[data-permission-action]');
  if (!button) return;
  const kind = button.closest('[data-kind]')?.dataset.kind;
  const action = button.dataset.permissionAction;
  if (!kind) return;

  button.disabled = true;
  try {
    if (action === 'request') applyGrants(await invoke('permission_request', { kind }));
    else if (action === 'settings') await invoke('permission_open_settings', { kind });
    else if (action === 'repair') applyGrants(await invoke('permission_repair', { kind }));
    else if (action === 'relaunch') await invoke('relaunch_app');
  } catch (error) {
    setStatus(String(error), 'err');
  }
  button.disabled = false;
}

// Live while the window is on screen: the native watcher emits `permission-status` on every
// change (it is how System Settings toggles show up without a click), and it is switched off
// again when the window is hidden so an idle Settings window costs nothing.
async function wirePermissionGuidance() {
  renderPermissionGuidance();
  await listen('permission-status', (event) => applyGrants(event.payload));
  // The engine confirming (or losing) its event tap is an Accessibility change too.
  await listen('accessibility-status', refreshPermissionGuidance);
  const watch = (active) => invoke('permission_watch', { active }).catch(() => {});
  await refreshPermissionGuidance();
  if (!document.hidden) watch(true);
  window.addEventListener('focus', refreshPermissionGuidance);
  document.addEventListener('visibilitychange', () => {
    watch(!document.hidden);
    if (!document.hidden) refreshPermissionGuidance();
  });
  window.addEventListener('beforeunload', () => watch(false));
}

// Secure Input: while ANY app holds it (password fields, a stale lock-screen grab), no
// expander on the system can see keystrokes — typed triggers pause. Show who holds it.
function applySecureInput(holder) {
  const banner = document.getElementById('si-banner');
  if (!banner) return;
  banner.classList.toggle('show', Boolean(holder));
  if (holder) document.getElementById('si-holder').textContent = holder;
}

// The engine is left stopped when its respawn budget is spent; offer the one-click recovery.
function applyEngineStopped(stopped) {
  document.getElementById('engine-banner')?.classList.toggle('show', Boolean(stopped));
}

async function wireEngineStatus() {
  applyEngineStopped(await invoke('engine_exhausted'));
  await listen('engine-status', (e) => applyEngineStopped(e.payload));
  document.getElementById('engine-restart')?.addEventListener('click', async () => {
    try {
      await invoke('restart_engine');
    } catch (e) {
      console.error('restart_engine failed', e);
    }
  });
}

async function wireSecureInput() {
  applySecureInput(await invoke('secure_input_status'));
  await listen('secure-input-status', (e) => applySecureInput(e.payload));
}

// --- Sidebar ----------------------------------------------------------------

function countIn(pred) { return state.snippets.filter(pred).length; }

function renderSidebar() {
  const sb = document.getElementById('sidebar');
  const item = (id, label, count, opts = {}) => `
    <button class="nav-item ${state.selected === id ? 'active' : ''}" data-nav="${id}">
      <span class="nav-label">${label}</span>
      ${count != null ? `<span class="nav-count">${count}</span>` : ''}
      ${opts.actions || ''}
    </button>`;

  // Filter groups by the sidebar search (matches name or shared-team name).
  const gq = state.groupSearch.trim().toLowerCase();
  const visibleGroups = gq
    ? state.groups.filter((g) => g.name.toLowerCase().includes(gq) || (g.team || '').toLowerCase().includes(gq))
    : state.groups;

  const groupItems = visibleGroups.map((g) => item(
    g.id,
    escapeHtml(g.name) + (g.team ? ` <span class="team-badge" title="Shared with team ${escapeAttr(g.team)}">${icon('share', 9)} ${escapeHtml(g.team)}</span>` : ''),
    countIn((s) => s.groupId === g.id),
    { actions: `<span class="grp-actions">
        <span class="grp-btn" data-share="${g.id}" title="${g.team ? 'Change team sharing' : 'Share with a team'}">${icon('share', 12)}</span>
        <span class="grp-btn" data-export="${g.id}" title="Export group">${icon('download', 12)}</span>
        <span class="grp-btn" data-rename="${g.id}" title="Rename">${icon('pencil', 12)}</span>
        <span class="grp-btn" data-delgrp="${g.id}" title="Delete group">${icon('trash', 12)}</span>
      </span>` },
  )).join('');

  sb.innerHTML = `
    <div class="nav-group">
      <div class="nav-heading">Snippets</div>
      ${item('all', 'All snippets', state.snippets.length)}
      ${item('ungrouped', 'Ungrouped', countIn((s) => !s.groupId))}
    </div>
    <div class="nav-group">
      <div class="nav-heading">Groups <button class="add-group" id="add-group" title="New group">${icon('plus', 12)}</button></div>
      <input class="nav-search" id="group-search" type="search" placeholder="Filter groups…" value="${escapeAttr(state.groupSearch)}" />
      ${groupItems || `<div class="nav-empty">${gq ? 'No matching groups' : 'No groups yet'}</div>`}
    </div>
    ${renderTeamNav(item)}
    <div class="nav-group nav-bottom">
      <button class="nav-item ${state.selected === 'history' ? 'active' : ''}" data-nav="history"><span class="nav-label">History</span></button>
      ${item('settings', 'Settings', null)}
    </div>
  `;
  const gs = sb.querySelector('#group-search');
  gs.addEventListener('input', () => {
    const pos = gs.selectionStart;
    state.groupSearch = gs.value;
    renderSidebar();
    const again = document.getElementById('group-search');
    again.focus(); again.setSelectionRange(pos, pos);
  });
  sb.querySelectorAll('[data-nav]').forEach((b) => b.addEventListener('click', (e) => {
    if (e.target.closest('.grp-actions')) return;
    state.selected = b.dataset.nav; renderSidebar(); renderMain();
  }));
  sb.querySelector('#add-group').addEventListener('click', addGroup);
  sb.querySelectorAll('[data-rename]').forEach((el) => el.addEventListener('click', (e) => {
    e.stopPropagation(); renameGroup(el.dataset.rename);
  }));
  sb.querySelectorAll('[data-delgrp]').forEach((el) => el.addEventListener('click', (e) => {
    e.stopPropagation(); deleteGroup(el.dataset.delgrp);
  }));
  sb.querySelectorAll('[data-share]').forEach((el) => el.addEventListener('click', (e) => {
    e.stopPropagation(); shareGroup(el.dataset.share);
  }));
  sb.querySelectorAll('[data-export]').forEach((el) => el.addEventListener('click', (e) => {
    e.stopPropagation(); exportSnippets(el.dataset.export);
  }));
}

async function addGroup() {
  const name = await promptDialog('New group', { label: 'Group name', placeholder: 'e.g. Support replies', confirmLabel: 'Create' });
  if (!name) return;
  await invoke('create_group', { group: { name } });
  setStatus('Group created.', 'ok');
}
async function renameGroup(id) {
  const g = state.groups.find((x) => x.id === id);
  const name = await promptDialog('Rename group', { label: 'Group name', value: g?.name || '', confirmLabel: 'Rename' });
  if (!name) return;
  await invoke('update_group', { id, patch: { name } });
  setStatus('Group renamed.', 'ok');
}
/// Share a group (and its snippets) with one of the signed-in identity's teams.
/// A searchable picker of YOUR teams (server-attested) — no free typing, no typos.
async function shareGroup(id) {
  const g = state.groups.find((x) => x.id === id);
  const teams = state.syncStatus?.identity?.teams || [];
  const roles = state.syncStatus?.identity?.roles || {};
  if (!teams.length && !g?.team) {
    setStatus('Sign in under Settings → Team sync first — teams come from your sync identity.', 'err');
    return;
  }
  const { modal, close } = openModal(`
    <h3>Share “${escapeHtml(g?.name || '')}”</h3>
    <p class="adv-hint">Members see it per their role.</p>
    <input type="search" id="ts-search" placeholder="Search your teams…" autocomplete="off" />
    <ul class="team-pick" id="ts-list"></ul>
    <div class="modal-actions">
      ${g?.team ? '<button class="danger" id="ts-unshare">Stop sharing</button>' : ''}
      <div class="spacer"></div>
      <button class="secondary" id="ts-cancel">Cancel</button>
    </div>`, { className: 'small' });

  const apply = async (team) => {
    close();
    try {
      await invoke('set_group_team', { id, team });
      setStatus(team ? `Group shared with “${team}” — its snippets will sync.` : 'Group is no longer shared.', 'ok');
    } catch (e) { setStatus(String(e), 'err'); }
  };

  const list = modal.querySelector('#ts-list');
  const search = modal.querySelector('#ts-search');
  const draw = () => {
    const q = search.value.trim().toLowerCase();
    const rows = teams
      .filter((t) => !q || t.toLowerCase().includes(q))
      .map((t) => `
        <li class="team-pick-row ${t === g?.team ? 'current' : ''}" data-team="${escapeAttr(t)}">
          <span class="team-pick-name">${escapeHtml(t)}</span>
          ${roles[t] ? `<span class="role-tag">${escapeHtml(roles[t])}</span>` : ''}
          ${t === g?.team ? '<span class="team-pick-cur">current</span>' : ''}
        </li>`).join('');
    list.innerHTML = rows || '<li class="team-pick-row muted">No matching teams</li>';
    list.querySelectorAll('[data-team]').forEach((r) =>
      r.addEventListener('click', () => apply(r.dataset.team)));
  };
  search.addEventListener('input', draw);
  draw();
  modal.querySelector('#ts-cancel').addEventListener('click', close);
  modal.querySelector('#ts-unshare')?.addEventListener('click', () => apply(null));
  search.focus();
}

async function deleteGroup(id) {
  const g = state.groups.find((x) => x.id === id);
  if (!(await confirmDialog(`Delete group “${g?.name}”? Its snippets move to Ungrouped.`, { confirmLabel: 'Delete group', danger: true }))) return;
  await invoke('delete_group', { id });
  if (state.selected === id) state.selected = 'all';
  setStatus('Group deleted.', 'ok');
}

// Teams you belong to (server-attested) plus any team tags found locally.
function knownTeams() {
  const t = new Set(state.syncStatus?.identity?.teams || []);
  state.groups.forEach((g) => g.team && t.add(g.team));
  state.snippets.forEach((sn) => sn.team && t.add(sn.team));
  return [...t].sort();
}

function renderTeamNav(item) {
  const teams = knownTeams();
  if (!teams.length) return '';
  const items = teams.map((t) =>
    item('team:' + t, icon('users', 12) + ' ' + escapeHtml(t), countIn((s) => s.team === t))).join('');
  return `<div class="nav-group"><div class="nav-heading">Teams</div>${items}</div>`;
}

// --- Main pane --------------------------------------------------------------

function renderMain() {
  renderPermissionGuidance();
  const main = document.getElementById('main');
  if (state.selected === 'settings') { renderSettings(main); return; }
  if (state.selected === 'history') { renderHistory(main); return; }

  const title = state.selected === 'all' ? 'All snippets'
    : state.selected === 'ungrouped' ? 'Ungrouped'
    : state.selected.startsWith('team:') ? `Team · ${state.selected.slice(5)}`
    : (state.groups.find((g) => g.id === state.selected)?.name || 'Snippets');

  const teamName = state.selected.startsWith('team:') ? state.selected.slice(5) : null;
  const teamGroups = teamName ? state.groups.filter((g) => g.team === teamName) : [];
  main.innerHTML = `
    <div class="main-head">
      <input class="search" type="search" placeholder="Search snippets…" value="${escapeAttr(state.search)}" />
      <button class="ghost" id="import-snips" title="${currentGroupId() ? 'Import snippets into this group' : 'Import snippets (Glyphio export or YAML match files)'}">Import</button>
      <button class="ghost" id="export-snips" title="${isGroupView() ? 'Export this group' : 'Export all snippets'}">Export</button>
      <button class="primary" id="new-snippet">+ New snippet</button>
    </div>
    <h2 class="main-title">${escapeHtml(title)}</h2>
    ${teamName ? `<div class="team-groups">${
      teamGroups.length
        ? teamGroups.map((g) => `<button class="team-group-chip" data-goto="${g.id}">${icon('share', 10)} ${escapeHtml(g.name)} <span class="nav-count">${state.snippets.filter((s) => s.groupId === g.id).length}</span></button>`).join('')
        : '<span class="adv-hint">No groups shared with this team yet — use ⇅ on a group, or create a snippet here.</span>'
    }</div>` : ''}
    <div class="snip-list" id="snip-list"></div>
  `;
  const search = main.querySelector('.search');
  search.addEventListener('input', () => { state.search = search.value; drawList(); });
  main.querySelector('#new-snippet').addEventListener('click', () => openEditor(null));
  main.querySelectorAll('[data-goto]').forEach((b) => b.addEventListener('click', () => {
    state.selected = b.dataset.goto; renderSidebar(); renderMain();
  }));
  // Importing while a group is open defaults to that group — the dialog can still redirect it.
  main.querySelector('#import-snips').addEventListener('click', () => importSnippets(currentGroupId()));
  main.querySelector('#export-snips').addEventListener('click', () =>
    exportSnippets(isGroupView() ? state.selected : null));
  drawList();
}

function isGroupView() {
  return !['all', 'ungrouped', 'settings'].includes(state.selected);
}

/// The real group being viewed, if any — `isGroupView` also covers history and team views.
function currentGroupId() {
  return state.groups.some((g) => g.id === state.selected) ? state.selected : null;
}

// --- Capture history: history-view.mjs, loaded on first visit (PERF-05) --------

async function renderHistory(main) {
  const view = await import('./history-view.mjs');
  return view.renderHistory(main);
}

// --- Invites & first-run welcome ----------------------------------------------
// glyphio://join links land here (from the OS handler) and from the paste box. NOTHING is
// applied without explicit confirmation — a URL scheme can be fired by any webpage, and
// silently switching the sync server would be a data-redirection attack.

async function wireInvites() {
  await listen('invite-link', (e) => confirmInvite(String(e.payload)));
  // From the setup window's "Set up team sync".
  await listen('show-sync', () => {
    state.selected = 'settings'; state.settingsTab = 'sync';
    renderSidebar(); renderMain();
  });
  await listen('show-history', () => {
    state.selected = 'history';
    renderSidebar(); renderMain();
  });
}

async function confirmInvite(url) {
  let info;
  try { info = await invoke('parse_invite', { url }); }
  catch (e) { setStatus(String(e), 'err'); return; }
  const { modal, close } = openModal(`
    <h3>${info.joinOnly ? 'Join another team?' : 'Join team sync?'}</h3>
    <p class="confirm-body">${info.joinOnly
      ? 'This invite adds a team on the server you already use. Your current teams are kept.'
      : 'This invite configures Glyphio to sync team snippets with:'}</p>
    <div class="invite-summary">
      <div><span class="invite-k">Server</span><code>${escapeHtml(info.server)}</code></div>
      <div><span class="invite-k">Sign-in</span>${info.authMode === 'oidc' ? 'Single sign-on (SSO)' : 'API token' + (info.hasToken ? ' (included in the invite)' : '')}</div>
    </div>
    <p class="adv-hint">Only accept invites you expect. Personal snippets and captures never
    leave this device either way.</p>
    <div class="modal-actions"><div class="spacer"></div>
      <button class="secondary" data-no>Cancel</button>
      <button class="primary" data-yes>Join</button>
    </div>`, { className: 'small' });
  modal.querySelector('[data-no]').addEventListener('click', close);
  modal.querySelector('[data-yes]').addEventListener('click', async () => {
    close();
    try {
      await invoke('apply_invite', { url });
      localStorage.setItem('glyphio-welcomed', '1');
      setStatus(info.joinOnly
        ? 'Joined — the new team’s snippets are on their way.'
        : info.authMode === 'oidc'
          ? 'Connected — now sign in with SSO under Settings → Sync.'
          : 'Connected — syncing with your team.', 'ok');
      state.selected = 'settings'; state.settingsTab = 'sync';
      renderSidebar(); renderMain();
      await refreshSync();
      if (info.joinOnly) await reloadAll();
    } catch (e) { setStatus(String(e), 'err'); }
  });
}

function maybeShowWelcome() {
  if (localStorage.getItem('glyphio-welcomed')) return;
  if (state.syncConfig?.enabled || state.snippets.length > 2) {
    localStorage.setItem('glyphio-welcomed', '1'); // existing user — never nag
    return;
  }
  const { modal, close } = openModal(`
    <h3>Welcome to Glyphio</h3>
    <p class="confirm-body">Text expansion and screenshots, local-first. How will you use it?</p>
    <div class="welcome-cards">
      <button class="welcome-card" data-w="personal">
        <strong>Just me</strong>
        <span>Everything stays on this device. You can join a team any time later.</span>
      </button>
      <button class="welcome-card" data-w="invite">
        <strong>I have an invite</strong>
        <span>Paste the invite link or code from your team admin.</span>
      </button>
      <button class="welcome-card" data-w="setup">
        <strong>Set up team sync</strong>
        <span>Connect to your organization's server or single sign-on.</span>
      </button>
    </div>
    <div class="welcome-paste" id="welcome-paste" style="display:none">
      <input type="text" id="welcome-invite" placeholder="glyphio://join?server=…" spellcheck="false" autocomplete="off" />
      <button class="primary" id="welcome-join">Continue</button>
    </div>`, { className: 'welcome' });
  const done = () => { localStorage.setItem('glyphio-welcomed', '1'); close(); };
  modal.querySelector('[data-w="personal"]').addEventListener('click', done);
  modal.querySelector('[data-w="setup"]').addEventListener('click', () => {
    done();
    state.selected = 'settings'; state.settingsTab = 'sync';
    renderSidebar(); renderMain();
  });
  modal.querySelector('[data-w="invite"]').addEventListener('click', () => {
    modal.querySelector('#welcome-paste').style.display = 'flex';
    modal.querySelector('#welcome-invite').focus();
  });
  modal.querySelector('#welcome-join').addEventListener('click', () => {
    const url = modal.querySelector('#welcome-invite').value.trim();
    if (!url) return;
    done();
    confirmInvite(url);
  });
}

// --- Import / export ---------------------------------------------------------
// Exports are portable Glyphio JSON (content only — no team/owner/sync state).
// Imports match on CONTENT HASH, not trigger: a snippet already here byte-for-byte is
// skipped silently, and a trigger that exists with different content is a conflict the
// user settles in the dialog below. Nothing is overwritten without being asked.

async function exportSnippets(groupId) {
  const g = groupId ? state.groups.find((x) => x.id === groupId) : null;
  const slug = (g?.name || 'all').toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '');
  const path = await window.__TAURI__.dialog.save({
    title: g ? `Export “${g.name}”` : 'Export all snippets',
    defaultPath: `glyphio-${slug}.json`,
    filters: [{ name: 'Glyphio export', extensions: ['json'] }],
  });
  if (!path) return;
  try {
    await invoke('export_snippets', { path, groupId });
    setStatus(`Exported ${g ? `“${g.name}”` : 'all snippets'} → ${path}`, 'ok');
  } catch (e) { setStatus(String(e), 'err'); }
}

async function importSnippets(intoGroupId = null) {
  const path = await window.__TAURI__.dialog.open({
    title: 'Import snippets',
    multiple: false,
    filters: [{ name: 'Glyphio export / YAML matches', extensions: ['json', 'yml', 'yaml'] }],
  });
  if (!path) return;
  let plan;
  try {
    plan = await invoke('preview_import', { path });
  } catch (e) { setStatus(String(e), 'err'); return; }

  const options = await importDialog(plan, path, intoGroupId);
  if (!options) return;
  try {
    const r = await invoke('import_snippets', { path, options });
    const bits = [];
    if (r.imported) bits.push(`${r.imported} added`);
    if (r.replaced) bits.push(`${r.replaced} replaced`);
    if (r.groupsCreated) bits.push(`${r.groupsCreated} group${r.groupsCreated === 1 ? '' : 's'} created`);
    if (r.skipped.length) bits.push(`${r.skipped.length} already here`);
    if (r.conflicts.length) bits.push(`${r.conflicts.length} kept as-is`);
    if (r.unsupported?.length) bits.push(`${r.unsupported.length} unsupported`);
    const quarantined = r.quarantined?.length
      ? ` · ${r.quarantined.length} disabled for review (can run code: ${r.quarantined.slice(0, 3).join(', ')}${r.quarantined.length > 3 ? '…' : ''})`
      : '';
    setStatus(`Import: ${bits.join(' · ') || 'nothing to do'}${quarantined}`, quarantined ? 'info' : 'ok');
    await reloadAll();
  } catch (e) { setStatus(String(e), 'err'); }
}

/// The import dialog: where everything lands, and what to do about triggers that already
/// exist with different content. Resolves to the options for `import_snippets`, or null on
/// cancel. Conflicts default to KEEPING what's stored — replacing is always a deliberate act.
function importDialog(plan, path, intoGroupId) {
  return new Promise((resolve) => {
    const conflicts = plan.items.filter((i) => i.status === 'conflict');
    const file = String(path).split('/').pop();
    const groupOpt = (g) => {
      const team = g.team ? ` — shared with ${escapeHtml(g.team)}` : '';
      return `<option value="${escapeAttr(g.id)}"${g.id === intoGroupId ? ' selected' : ''}>${escapeHtml(g.name)}${team}</option>`;
    };
    const keepLabel = plan.groups.length
      ? `Keep the file's groups (${plan.groups.map(escapeHtml).join(', ')})`
      : 'No group';

    const { modal, close } = openModal(`
      <h3>Import snippets</h3>
      <p class="modal-sub">${escapeHtml(file)} — <strong>${plan.newCount}</strong> new,
        <strong>${plan.identicalCount}</strong> already here,
        <strong>${conflicts.length}</strong> conflicting.</p>

      <div class="mfield">
        <label for="imp-group">Import into</label>
        <select id="imp-group" data-group>
          <option value=""${intoGroupId ? '' : ' selected'}>${keepLabel}</option>
          ${state.groups.map(groupOpt).join('')}
        </select>
        <p class="hint">Snippets in a team-shared group sync with that team.</p>
      </div>

      ${conflicts.length ? `
        <div class="imp-conflicts">
          <div class="imp-conflicts-head">
            <span>These triggers already exist with different content</span>
            <span class="imp-bulk">
              <button class="link" type="button" data-all>Replace all</button> ·
              <button class="link" type="button" data-none>Keep all</button>
            </span>
          </div>
          <div class="imp-rows">
            ${conflicts.map((c) => `
              <label class="imp-row">
                <input type="checkbox" data-conflict value="${escapeAttr(c.trigger)}" />
                <span class="imp-row-body">
                  <span class="imp-row-top">
                    <code>${escapeHtml(c.trigger)}</code>
                    ${c.executable ? '<span class="imp-exec" title="Arrives disabled until you review it">can run code</span>' : ''}
                  </span>
                  <span class="imp-side"><em>yours</em> ${escapeHtml(c.existing || '')}</span>
                  <span class="imp-side"><em>file</em> ${escapeHtml(c.incoming)}</span>
                </span>
              </label>`).join('')}
          </div>
          <p class="hint">Ticked entries are overwritten by the file. Unticked keep what you have.</p>
        </div>` : ''}

      ${plan.unsupported.length ? `<p class="hint imp-unsupported">${plan.unsupported.length} entr${plan.unsupported.length === 1 ? 'y' : 'ies'} in this file can't be represented in Glyphio and will be left out: ${escapeHtml(plan.unsupported.slice(0, 3).join(', '))}${plan.unsupported.length > 3 ? '…' : ''}</p>` : ''}

      <div class="modal-actions">
        <div class="spacer"></div>
        <button class="secondary" data-cancel>Cancel</button>
        <button class="primary" data-ok>Import</button>
      </div>`, { className: 'import-modal', onClose: () => resolve(null) });

    const boxes = [...modal.querySelectorAll('[data-conflict]')];
    modal.querySelector('[data-all]')?.addEventListener('click', () => boxes.forEach((b) => { b.checked = true; }));
    modal.querySelector('[data-none]')?.addEventListener('click', () => boxes.forEach((b) => { b.checked = false; }));
    modal.querySelector('[data-cancel]').addEventListener('click', close);
    modal.querySelector('[data-ok]').addEventListener('click', () => {
      resolve({
        groupId: modal.querySelector('[data-group]').value || null,
        replace: boxes.filter((b) => b.checked).map((b) => b.value),
      });
      close();
    });
    modal.querySelector('[data-ok]').focus();
  });
}

function visibleSnippets() {
  const f = state.search.trim().toLowerCase();
  return state.snippets.filter((s) => {
    const inScope = state.selected === 'all'
      || (state.selected === 'ungrouped' && !s.groupId)
      || (state.selected.startsWith('team:') && s.team === state.selected.slice(5))
      || s.groupId === state.selected;
    const matches = !f || s.trigger.toLowerCase().includes(f) || s.replacement.toLowerCase().includes(f);
    return inScope && matches;
  });
}

function drawList() {
  const list = document.getElementById('snip-list');
  const rows = visibleSnippets();
  if (rows.length === 0) {
    list.innerHTML = `<div class="empty">No snippets here yet. <button class="link" id="empty-new">Create one</button>.</div>`;
    list.querySelector('#empty-new').addEventListener('click', () => openEditor(null));
    return;
  }
  list.innerHTML = '';
  for (const s of rows) {
    const fmt = FORMATS.find((f) => f.id === s.format) || FORMATS[0];
    const kind = s.kind && s.kind !== 'text' ? KINDS.find((k) => k.id === s.kind) : null;
    const disabled = s.enabled === false;
    const card = document.createElement('div');
    card.className = 'snip-card' + (disabled ? ' snip-disabled' : '');
    card.innerHTML = `
      <div class="snip-main">
        <div class="snip-top">
          <code class="snip-trigger"></code>
          <span class="fmt-badge" title="${fmt.label}">${fmt.badge}</span>
          ${kind ? `<span class="kind-badge">${kind.label}</span>` : ''}
          ${disabled ? `<span class="kind-badge off-badge" title="Imported or synced content that can run code arrives disabled until you review it.">off — review</span>` : ''}
        </div>
        <div class="snip-preview"></div>
      </div>
      <div class="snip-actions">
        ${disabled ? '<button class="primary sm" data-enable>Enable</button>' : ''}
        <button class="secondary sm" data-edit>Edit</button>
        <button class="danger sm" data-del>Delete</button>
      </div>`;
    card.querySelector('.snip-trigger').textContent = s.trigger;
    card.querySelector('.snip-preview').textContent = previewText(s);
    card.querySelector('[data-edit]').addEventListener('click', () => openEditor(s));
    card.querySelector('[data-del]').addEventListener('click', () => deleteSnippet(s));
    card.querySelector('[data-enable]')?.addEventListener('click', () => enableSnippet(s));
    card.querySelector('.snip-main').addEventListener('click', (e) => {
      if (!e.target.closest('button')) openEditor(s);
    });
    list.appendChild(card);
  }
}

// Turn a quarantined/disabled snippet live — after an explicit confirmation when it can
// execute code (that's exactly what the quarantine exists for).
async function enableSnippet(s) {
  const canRun = s.kind === 'command'
    || (Array.isArray(s.variables) && s.variables.some((v) => ['shell', 'script'].includes(v?.type)));
  if (canRun) {
    const what = s.kind === 'command' ? s.replacement : JSON.stringify(s.variables);
    const ok = await confirmDialog(
      `“${s.trigger}” can run code on this machine:\n\n${what}\n\nOnly enable it if you understand and trust this command.`,
      { confirmLabel: 'Enable — I trust it', danger: true },
    );
    if (!ok) return;
  }
  try {
    await invoke('update_snippet', {
      id: s.id,
      patch: {
        trigger: s.trigger, replacement: s.replacement, format: s.format, kind: s.kind,
        enabled: true, variables: s.variables, groupId: s.groupId, appScope: s.appScope,
        team: s.team || null,
      },
    });
    setStatus(`“${s.trigger}” enabled.`, 'ok');
  } catch (e) { setStatus(String(e), 'err'); }
}

function previewText(s) {
  let t = s.replacement || '';
  if (s.format === 'html') t = t.replace(/<[^>]+>/g, ' ');
  t = t.replace(/\s+/g, ' ').trim();
  return t.length > 140 ? t.slice(0, 140) + '…' : t;
}

async function deleteSnippet(s) {
  if (!(await confirmDialog(`Delete snippet “${s.trigger}”?`, { confirmLabel: 'Delete', danger: true }))) return;
  await invoke('delete_snippet', { id: s.id });
  setStatus('Snippet deleted.', 'ok');
}

// --- Editor: snippet-editor.mjs, loaded the first time a snippet opens (PERF-05) -

async function openEditor(existing) {
  try {
    const editor = await import('./snippet-editor.mjs');
    return editor.openEditor(existing);
  } catch (e) { setStatus(`Could not open the editor: ${e}`, 'err'); }
}

// --- Settings view ----------------------------------------------------------

// --- Sync (Settings → Sync) ---------------------------------------------------
// Status + config for team snippet sync. Personal snippets and capture history never sync.
// Config holds no secrets: OIDC sessions and static tokens live in the OS keychain.

const SYNC_STATE_LABELS = {
  disabled: 'Off — not configured',
  signedOut: 'Signed out',
  idle: 'Up to date',
  syncing: 'Syncing…',
  error: 'Error',
};

// Team sync is one build with three experiences, driven entirely by configuration — no
// separate "personal" vs "self-host" binaries:
//   • locked  — a system-wide managed config is present (dropped by IT/an admin). The server
//               is fixed; users just sign in. No connection fields, no invite, no manual setup.
//   • active  — the user has configured/joined a backend themselves; the editable form shows.
//   • off     — personal install (default). No backend form at all — just a calm off state
//               with "Join with an invite link". The manual backend form is tucked behind a
//               discreet disclosure for the rare self-hoster configuring their first client.
function renderSyncSection(form) {
  const div = document.createElement('div');
  div.className = 'form-section';
  const st = state.syncStatus || { state: 'disabled' };
  const cfg = state.syncConfig || {};
  const mode = cfg.managed ? 'locked' : (cfg.enabled ? 'active' : 'off');
  const who = st.identity
    ? `${escapeHtml(st.identity.email || st.identity.sub)} · teams: ${st.identity.teams.map(escapeHtml).join(', ') || '—'}`
    : '';

  const cardHtml = `
    <div class="sync-card">
      <div class="sync-state" data-state="${escapeHtml(st.state)}">${SYNC_STATE_LABELS[st.state] || escapeHtml(st.state)}</div>
      ${who ? `<div class="sync-who">${who}</div>` : ''}
      ${st.lastSync ? `<div class="sync-last">Last sync: ${escapeHtml(st.lastSync)}</div>` : ''}
      ${st.error ? `<div class="sync-error">${escapeHtml(st.error)}</div>` : ''}
      <div class="sync-actions">
        ${cfg.enabled && cfg.authMode === 'oidc' && st.state === 'signedOut' ? '<button class="primary" id="sync-signin">Sign in with SSO</button>' : ''}
        ${cfg.enabled && cfg.authMode === 'token' && st.state === 'signedOut' ? '<button class="primary" id="sync-settoken">Set API token…</button>' : ''}
        ${['idle', 'error', 'syncing'].includes(st.state) ? '<button class="secondary" id="sync-now">Sync now</button> <button class="ghost" id="sync-signout">Sign out</button>' : ''}
      </div>
      ${renderTeamPanel(st)}
    </div>`;

  const formHtml = `
    <div class="mfield"><label>Enable sync</label><input type="checkbox" id="sc-enabled" ${cfg.enabled ? 'checked' : ''}></div>
    <div class="mfield"><label>Backend URL</label><input type="text" id="sc-backend" placeholder="https://sync.example.com" value="${escapeHtml(cfg.backendUrl || '')}"></div>
    <div class="mfield"><label>Auth mode</label>
      <select id="sc-mode">
        <option value="oidc" ${cfg.authMode !== 'token' ? 'selected' : ''}>OIDC single sign-on</option>
        <option value="token" ${cfg.authMode === 'token' ? 'selected' : ''}>Static API token</option>
      </select></div>
    <div class="mfield sc-oidc"><label>OIDC issuer</label><input type="text" id="sc-issuer" placeholder="https://your-tenant.okta.com" value="${escapeHtml(cfg.issuer || '')}"></div>
    <div class="mfield sc-oidc"><label>Client ID</label><input type="text" id="sc-client" value="${escapeHtml(cfg.clientId || '')}"></div>
    <div class="mfield sc-oidc"><label>Scopes (beyond openid)</label><input type="text" id="sc-scopes" placeholder="profile email offline_access groups" value="${escapeHtml((cfg.scopes || []).join(' '))}"></div>
    <button class="primary" id="sc-save">Save sync settings</button>`;

  // An invite link is the normal way in, so it leads; the hand-entered connection fields are
  // for self-hosters and sit under Advanced.
  const inviteHtml = `
    <div class="sync-invite">
      <label for="sync-invite-input">${mode === 'active' ? 'Join another team: paste an invite link' : 'Paste an invite link'}</label>
      <div class="sync-invite-row">
        <input type="text" id="sync-invite-input" placeholder="glyphio://join?server=…" spellcheck="false" autocomplete="off" />
        <button class="primary" id="sync-invite-go">Join</button>
      </div>
    </div>`;
  const advancedHtml = `
    <details class="sync-advanced"${mode === 'active' && !cfg.backendUrl ? ' open' : ''}>
      <summary>Advanced: connect manually</summary>
      ${formHtml}
    </details>`;

  let body;
  if (mode === 'locked') {
    body = `
      <div class="managed-note">
        <strong>Managed by your organization.</strong> The sync connection is configured
        centrally${cfg.backendUrl ? ` (<code>${escapeHtml(cfg.backendUrl)}</code>)` : ''} and can't
        be changed here. Just sign in; team sharing works as normal.
      </div>
      ${cardHtml}`;
  } else if (mode === 'active') {
    body = `${cardHtml}${inviteHtml}${advancedHtml}`;
  } else {
    // Off / personal. Joining still routes through the invite confirmation dialog, so a
    // pasted link never switches servers silently.
    body = `
      <div class="sync-off">
        <p class="adv-hint">Team sync is off — everything stays on this device. Join a team with an
        invite link from your admin.</p>
        ${inviteHtml}
      </div>
      ${advancedHtml}`;
  }

  div.innerHTML = `
    <h3>Team sync</h3>
    <p class="adv-hint">Team-shared groups only. Personal snippets, captures and clipboard never
    leave this device.</p>
    ${body}`;

  const toggleOidc = () => div.querySelectorAll('.sc-oidc').forEach((el) => {
    el.style.display = div.querySelector('#sc-mode').value === 'oidc' ? '' : 'none';
  });
  if (div.querySelector('#sc-mode')) {
    toggleOidc();
    div.querySelector('#sc-mode').addEventListener('change', toggleOidc);
  }
  const joinFromField = () => {
    const url = div.querySelector('#sync-invite-input').value.trim();
    if (url) confirmInvite(url);
  };
  div.querySelector('#sync-invite-go')?.addEventListener('click', joinFromField);
  div.querySelector('#sync-invite-input')?.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') joinFromField();
  });
  div.querySelector('#sc-save')?.addEventListener('click', async () => {
    const config = {
      ...cfg,
      enabled: div.querySelector('#sc-enabled').checked,
      backendUrl: div.querySelector('#sc-backend').value.trim(),
      authMode: div.querySelector('#sc-mode').value,
      issuer: div.querySelector('#sc-issuer').value.trim(),
      clientId: div.querySelector('#sc-client').value.trim(),
      scopes: div.querySelector('#sc-scopes').value.trim().split(/\s+/).filter(Boolean),
    };
    try {
      await invoke('save_sync_config', { config });
      state.syncConfig = config;
      setStatus('Sync settings saved.', 'ok');
      await refreshSync();
    } catch (e) { setStatus(String(e), 'err'); }
  });
  div.querySelector('#sync-signin')?.addEventListener('click', async () => {
    setStatus('Complete the sign-in in your browser…');
    try { await invoke('sync_sign_in'); setStatus('Signed in.', 'ok'); }
    catch (e) { setStatus(String(e), 'err'); }
    await refreshSync();
  });
  div.querySelector('#sync-settoken')?.addEventListener('click', async () => {
    const token = await promptDialog('Set API token', {
      label: 'Paste the token from your sync server admin', confirmLabel: 'Save token',
    });
    if (!token) return;
    try { await invoke('sync_set_token', { token }); setStatus('Token saved to the system keychain.', 'ok'); }
    catch (e) { setStatus(String(e), 'err'); }
    await refreshSync();
  });
  div.querySelector('#sync-now')?.addEventListener('click', () => invoke('sync_now').catch((e) => setStatus(String(e), 'err')));
  div.querySelector('#sync-signout')?.addEventListener('click', async () => {
    await invoke('sync_sign_out');
    state.teamMembers = {};
    await refreshSync();
  });
  wireTeamPanel(div);
  form.append(div);
}

// Teams + roster inside the sync card. You can belong to as many teams as you're invited to:
// joining redeems an invite with the credential you're already signed in with (so it adds a
// team rather than replacing the connection), and leaving drops one. Who else is in a team is
// still owned by the IdP or the server's config — hence the "how to add someone" help.
function renderTeamPanel(st) {
  const teams = st.identity?.teams || [];
  if (!teams.length) return renderJoinTeam(st, true);
  if (!state.selectedTeam || !teams.includes(state.selectedTeam)) state.selectedTeam = teams[0];
  const roles = st.identity?.roles || {};
  const chips = teams.map((t) =>
    `<button class="team-chip ${t === state.selectedTeam ? 'active' : ''}" data-team="${escapeAttr(t)}">${escapeHtml(t)}${roles[t] ? ` <span class="role-tag">${escapeHtml(roles[t])}</span>` : ''}</button>`).join('');
  const members = state.teamMembers[state.selectedTeam];
  const mq = state.memberSearch.trim().toLowerCase();
  const filtered = (members || []).filter((m) =>
    !mq || m.sub.toLowerCase().includes(mq) || (m.email || '').toLowerCase().includes(mq));
  const rows = members === undefined
    ? '<li class="member-row muted">Loading members…</li>'
    : filtered.length
      ? filtered.map((m) => `
          <li class="member-row">
            <span class="member-id">${escapeHtml(m.email || m.sub)}</span>
            ${m.email ? `<span class="member-sub">${escapeHtml(m.sub)}</span>` : ''}
            <span class="member-seen">${m.lastSeen ? 'seen ' + escapeHtml(m.lastSeen.slice(0, 10)) : 'never signed in'}</span>
          </li>`).join('')
      : `<li class="member-row muted">${mq ? 'No members match' : 'No members known yet'}</li>`;
  const myRole = roles[state.selectedTeam];
  return `
    <div class="team-panel">
      <div class="team-panel-head">
        <span class="team-panel-title">Your teams</span>
        <div class="team-chips">${chips}</div>
      </div>
      <div class="member-tools">
        <input type="search" id="member-search" placeholder="Search members…" value="${escapeAttr(state.memberSearch)}" />
        <button class="secondary" id="add-member">Add member…</button>
        <button class="ghost danger-text" id="leave-team" title="Leave ${escapeAttr(state.selectedTeam)}">Leave</button>
      </div>
      <ul class="member-list">${rows}</ul>
      ${myRole === 'owner' ? '<p class="adv-hint">You own this team — hand ownership to someone else before leaving it.</p>' : ''}
    </div>
    ${renderJoinTeam(st, false)}`;
}

/// The join box. `soleContent` renders the empty state (signed in, no teams yet) with a
/// fuller explanation; otherwise it's a compact row under the team panel.
function renderJoinTeam(st, soleContent) {
  if (!st.identity) return '';
  return `
    <div class="join-team${soleContent ? ' join-team-empty' : ''}">
      ${soleContent
        ? `<p class="adv-hint">Signed in, but not in a team yet — paste an invite to join one.</p>`
        : ''}
      <div class="join-row">
        <input type="text" id="join-code" placeholder="Paste an invite link or code…" autocomplete="off" spellcheck="false" />
        <button class="secondary" id="join-team">Join team</button>
      </div>
    </div>`;
}

function wireTeamPanel(div) {
  div.querySelectorAll('.team-chip').forEach((c) => c.addEventListener('click', () => {
    state.selectedTeam = c.dataset.team;
    state.memberSearch = '';
    renderMain();
    loadMembers(state.selectedTeam);
  }));
  const ms = div.querySelector('#member-search');
  ms?.addEventListener('input', () => {
    const pos = ms.selectionStart;
    state.memberSearch = ms.value;
    renderMain();
    const again = document.getElementById('member-search');
    if (again) { again.focus(); again.setSelectionRange(pos, pos); }
  });
  div.querySelector('#add-member')?.addEventListener('click', showAddMemberHelp);
  div.querySelector('#leave-team')?.addEventListener('click', () => leaveTeam(state.selectedTeam));
  const joinInput = div.querySelector('#join-code');
  const join = () => joinTeam(joinInput.value);
  div.querySelector('#join-team')?.addEventListener('click', join);
  joinInput?.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); join(); } });
  if (state.selectedTeam && state.teamMembers[state.selectedTeam] === undefined) {
    loadMembers(state.selectedTeam);
  }
}

/// Redeem an invite for the backend already signed in — additive, so existing teams stay.
async function joinTeam(code) {
  const value = (code || '').trim();
  if (!value) return;
  try {
    setStatus('Joining…', 'info');
    const teams = await invoke('sync_join_team', { code: value });
    setStatus(`Joined. You're now in ${teams.length} team${teams.length === 1 ? '' : 's'}: ${teams.join(', ')}`, 'ok');
    await refreshSync();
    await reloadAll();
  } catch (e) { setStatus(String(e), 'err'); }
}

/// Leaving is destructive enough to confirm: shared groups become personal again, and
/// getting back in needs a fresh invite.
async function leaveTeam(team) {
  if (!team) return;
  const shared = state.groups.filter((g) => g.team === team);
  const ok = await confirmDialog(
    `Leave “${team}”?\n\n`
    + (shared.length
      ? `${shared.length} group${shared.length === 1 ? '' : 's'} (${shared.map((g) => g.name).join(', ')}) will stop syncing and become personal again. Your snippets stay on this Mac.\n\n`
      : 'Its shared snippets stop syncing to this Mac.\n\n')
    + 'You will need a new invite to rejoin.',
    { confirmLabel: 'Leave team', danger: true },
  );
  if (!ok) return;
  try {
    await invoke('sync_leave_team', { team });
    state.selectedTeam = null;
    state.teamMembers = { ...state.teamMembers, [team]: undefined };
    setStatus(`Left “${team}”.`, 'ok');
    await refreshSync();
    await reloadAll();
  } catch (e) { setStatus(String(e), 'err'); }
}

async function loadMembers(team) {
  try {
    const members = await invoke('sync_team_members', { team });
    state.teamMembers = { ...state.teamMembers, [team]: members };
  } catch (e) {
    state.teamMembers = { ...state.teamMembers, [team]: [] };
    setStatus(String(e), 'err');
  }
  if (state.selected === 'settings') renderMain();
}

// Membership is managed where identity lives, so "add member" is guidance, not an API call.
function showAddMemberHelp() {
  const mode = state.syncConfig?.authMode === 'token' ? 'token' : 'oidc';
  const team = state.selectedTeam || 'your team';
  const body = mode === 'oidc'
    ? `<p>Team membership comes from your identity provider. To add someone to
       <strong>${escapeHtml(team)}</strong>:</p>
       <ol>
         <li>In your IdP admin (Okta, Entra, Keycloak…), add the person to the group named
             <code>${escapeHtml(team)}</code>.</li>
         <li>Make sure they're assigned to the Glyphio app in the IdP.</li>
         <li>They install Glyphio, enter the same backend + issuer settings, and sign in —
             membership applies on their next sign-in.</li>
       </ol>`
    : `<p>In static-token mode the sync server's config defines members. To add someone to
       <strong>${escapeHtml(team)}</strong>:</p>
       <ol>
         <li>Generate a token: <code>openssl rand -hex 32</code>, hash it:
             <code>echo -n &lt;token&gt; | shasum -a 256</code>.</li>
         <li>Add <code>{"tokenSha256":"&lt;hash&gt;","sub":"&lt;name&gt;","teams":["${escapeAttr(team)}"]}</code>
             to the server's <code>STATIC_TOKENS</code> and restart it.</li>
         <li>Send them the token privately; they paste it via “Set API token…”.</li>
       </ol>
       <p class="adv-hint">Details: <code>SETUP.md</code> §1b in the repo.</p>`;
  const { modal, close } = openModal(`
    <h3>Add a member to ${escapeHtml(team)}</h3>
    <div class="add-member-help">${body}</div>
    <div class="modal-actions"><div class="spacer"></div>
      <button class="primary" data-ok>Got it</button></div>`, { className: 'small' });
  modal.querySelector('[data-ok]').addEventListener('click', close);
}

async function refreshSync() {
  try {
    [state.syncStatus, state.syncConfig] = await Promise.all([
      invoke('sync_status'), invoke('get_sync_config'),
    ]);
  } catch { /* window closing */ }
  if (state.selected === 'settings') renderMain();
}

async function wireSync() {
  await refreshSync();
  await listen('sync-status', (e) => {
    state.syncStatus = e.payload;
    if (state.selected === 'settings') renderMain();
  });
}

const SETTINGS_TABS = [
  ['general', 'General'],
  ['capture', 'Capture'],
  ['snippets', 'Snippets'],
  ['clipboard', 'Clipboard'],
  ['sync', 'Sync'],
  ['about', 'About'],
];

function renderSettings(main) {
  const tabs = SETTINGS_TABS.map(([id, label]) =>
    `<button type="button" class="seg-opt ${state.settingsTab === id ? 'active' : ''}" data-tab="${id}">${label}</button>`).join('');
  main.innerHTML = `
    <h2 class="main-title">Settings</h2>
    <div class="seg settings-tabs" id="settings-tabs">${tabs}</div>
    <div id="settings-form"></div>`;
  main.querySelectorAll('[data-tab]').forEach((b) => b.addEventListener('click', () => {
    state.settingsTab = b.dataset.tab;
    renderMain();
  }));
  const form = main.querySelector('#settings-form');
  switch (state.settingsTab) {
    case 'general': renderGeneralTab(form); break;
    case 'snippets': renderSnippetsTab(form); break;
    case 'clipboard': renderClipboardTab(form); break;
    case 'sync': renderSyncSection(form); break;
    case 'about': renderAboutTab(form); break;
    default: renderSections(form, CAPTURE_SECTIONS);
  }
}

function renderGeneralTab(form) {
  const permissions = document.createElement('section');
  permissions.className = 'form-section permission-settings';
  permissions.setAttribute('aria-label', 'macOS permission status');
  permissions.innerHTML = `
    <h3>macOS permissions</h3>
    <p class="adv-hint">Current access for Glyphio. Actions appear only when macOS needs one.</p>
    <div data-permission-rows></div>
    <div class="sync-actions"><button type="button" class="secondary" data-run-setup>Run setup again</button></div>`;
  permissions.addEventListener('click', handlePermissionAction);
  const tail = document.createElement('div');
  tail.append(permissions, renderDiagnosticsSection());
  renderSections(form, GENERAL_SECTIONS, tail);
  renderPermissionGuidance();
}

/** Every shortcut's label as Settings shows it, for the diagnostics list. */
function shortcutLabelFor(key) {
  for (const sec of [...CAPTURE_SECTIONS, ...GENERAL_SECTIONS]) {
    for (const [fieldKey, type, label, opts] of sec.fields) {
      if (fieldKey === key) return label;
      if (type === 'hotkeys' && opts === key) return `${label} (to the clipboard)`;
    }
  }
  return shortcutLabel(key);
}

/// Each binding with what the system said, plus capture state, permissions and engine, and a
/// copyable plain-text report for a bug report. Nothing in it is content or an address.
function renderDiagnosticsSection() {
  const sec = el('section', { className: 'form-section diagnostics' });
  sec.innerHTML = `
    <h3>Diagnostics</h3>
    <p class="adv-hint">What each shortcut is bound to and whether macOS accepted it, the capture
    state, permissions and the expansion engine. The copied report holds no snippets, captures,
    window titles or addresses.</p>
    <pre class="diagnostics-report" aria-live="polite">Loading…</pre>
    <div class="sync-actions"><button type="button" class="secondary" data-copy-diagnostics>Copy diagnostics</button></div>`;
  const pre = sec.querySelector('pre');
  let report = '';
  invoke('diagnostics')
    .then((d) => {
      report = formatDiagnostics(d, shortcutLabelFor);
      pre.textContent = (d.shortcuts ?? []).map((row) => shortcutLine(row, shortcutLabelFor)).join('\n');
    })
    .catch((e) => { pre.textContent = `Could not read diagnostics: ${e}`; });
  sec.querySelector('[data-copy-diagnostics]').addEventListener('click', async () => {
    try {
      await navigator.clipboard.writeText(report);
      setStatus('Diagnostics copied.', 'ok');
    } catch (e) {
      setStatus(`Could not copy: ${e}`, 'err');
    }
  });
  return sec;
}

/// Render setting sections plus the Save button they share. Every input carries its key, so
/// `saveSettings` collects whatever is on screen.
/// `tail` is an extra section to place after the generated ones. It exists so **Save settings
/// stays the last thing on the page**: anything rendered below that button looks like
/// something the button doesn't cover, and it butts straight up against it besides.
function renderSections(form, sections, tail) {
  for (const sec of sections) {
    const div = document.createElement('div');
    div.className = 'form-section';
    div.innerHTML = `<h3>${sec.title}</h3>${sec.hint ? `<p class="adv-hint">${sec.hint}</p>` : ''}`;
    for (const [key, type, label, opts] of sec.fields) div.append(renderField(key, type, label, opts));
    form.append(div);
  }
  if (tail) form.append(tail);
  const save = document.createElement('button');
  save.className = 'primary'; save.textContent = 'Save settings';
  save.addEventListener('click', saveSettings);
  form.append(save);
}

function renderSnippetsTab(form) {
  form.append(renderExpansionDeliverySection());
  const div = document.createElement('div');
  div.className = 'form-section';
  div.innerHTML = `
    <h3>Portability</h3>
    <p class="adv-hint">Portable Glyphio JSON, content only. Imports also accept
    <code>matches:</code>-style YAML. Duplicates are skipped; conflicting triggers are shown for you to choose.${state.syncStatus?.identity?.policy?.exportTeamGroups && state.syncStatus.identity.policy.exportTeamGroups !== 'open'
      ? ' <strong>Note:</strong> your organization restricts exporting team-shared groups.' : ''}</p>
    <div class="sync-actions">
      <button class="secondary" id="tab-export">Export all snippets…</button>
      <button class="secondary" id="tab-import">Import…</button>
    </div>`;
  div.querySelector('#tab-export').addEventListener('click', () => exportSnippets(null));
  div.querySelector('#tab-import').addEventListener('click', () => importSnippets(null));
  form.append(div);
  renderSections(form, []); // just the shared Save button, for the delivery settings above
}

/**
 * Expansion output is one general policy plus explicit exceptions. Bundle identifiers are
 * intentionally shown and stored here: display names are mutable and fuzzy matching could send
 * a typed-key exception to the wrong application.
 */
function renderExpansionDeliverySection() {
  const div = el('div', { className: 'form-section' });
  div.append(el('h3', { textContent: 'Expansion delivery' }));
  div.append(el('p', {
    className: 'adv-hint',
    textContent: 'Paste reliably handles long, formatted, and international text while restoring what was previously on your clipboard. Add an application only when it needs typed-key delivery.',
  }));
  div.append(renderField(
    'expansionDelivery',
    'select',
    'General delivery method',
    EXPANSION_DELIVERY_OPTIONS,
  ));

  const heading = el('div', { className: 'delivery-app-head' });
  heading.append(
    el('strong', { textContent: 'Application overrides' }),
    el('span', { textContent: 'Apps not listed keep the general method.' }),
  );
  div.append(heading);

  const rows = el('div', { className: 'delivery-apps' });
  const problem = el('p', { className: 'field-error delivery-error' });

  const addRow = (bundleId = '', delivery = 'keys') => {
    const row = el('div', { className: 'delivery-app-row' });
    const bundle = el('input', {
      type: 'text',
      value: bundleId,
      placeholder: 'com.example.Application',
      spellcheck: false,
      ariaLabel: 'Application bundle identifier',
    });
    bundle.dataset.deliveryBundle = '';
    const mode = el('select', { ariaLabel: `Delivery method for ${bundleId || 'application'}` });
    for (const option of EXPANSION_DELIVERY_OPTIONS()) {
      mode.append(el('option', { value: option.value, textContent: option.label }));
    }
    mode.value = delivery;
    mode.dataset.deliveryMode = '';
    const remove = el('button', {
      type: 'button',
      className: 'ghost sm',
      textContent: 'Remove',
      ariaLabel: `Remove ${bundleId || 'application'} override`,
    });
    remove.addEventListener('click', () => row.remove());
    bundle.addEventListener('input', () => { problem.textContent = ''; });
    row.append(bundle, mode, remove);
    rows.append(row);
  };

  for (const [bundleId, delivery] of Object.entries(state.settings.expansionAppOverrides || {})
    .sort(([a], [b]) => a.localeCompare(b))) {
    addRow(bundleId, delivery);
  }
  div.append(rows, problem);
  const add = el('button', {
    type: 'button',
    className: 'secondary sm',
    textContent: 'Add application override',
  });
  add.addEventListener('click', () => {
    addRow();
    rows.lastElementChild?.querySelector('input')?.focus();
  });
  div.append(add);
  return div;
}

function renderClipboardTab(form) {
  const div = document.createElement('div');
  div.className = 'form-section';
  div.innerHTML = `
    <h3>Forget everything</h3>
    <p class="adv-hint">Every entry and copied image, pinned included. Cannot be undone.</p>
    <div class="sync-actions"><button class="secondary" id="clip-clear">Clear clipboard history</button></div>`;
  div.querySelector('#clip-clear').addEventListener('click', async () => {
    const ok = await confirmDialog(
      'Delete every stored clipboard entry and copied image, pinned ones included?',
      { confirmLabel: 'Clear all', danger: true },
    );
    if (!ok) return;
    try { await invoke('clear_clips'); setStatus('Clipboard history cleared.', 'ok'); }
    catch (e) { setStatus(String(e), 'err'); }
  });
  renderSections(form, CLIPBOARD_SECTIONS, div);
}

async function renderAboutTab(form) {
  const div = document.createElement('div');
  div.className = 'form-section';
  let version = '';
  try { version = await window.__TAURI__.app.getVersion(); } catch { /* capability absent */ }
  div.innerHTML = `
    <h3>About Glyphio</h3>
    <p class="adv-hint">Local-first text expansion, screenshot capture and clipboard history.
    ${version ? `Version <code>${escapeHtml(version)}</code>.` : ''}</p>
    <p class="adv-hint">App licensed
    GPL-3.0-or-later; sync protocol and reference server Apache-2.0. Your snippets and
    captures stay on this device unless you share a group with a team.</p>`;
  form.append(div);
  form.append(updatesSection());
}

/**
 * Updates. Glyphio checks quietly on launch and says nothing unless there is something to say —
 * this is where a user comes to ask, and where the answer waits.
 *
 * An update is verified against Glyphio's own signing key, which is why this works on an
 * unsigned build: it does not depend on Apple, only on the key baked into this app.
 */
function updatesSection() {
  const div = el('div', { className: 'form-section' });
  const status = el('p', { className: 'adv-hint', textContent: 'Checking for updates…' });
  const action = el('div', { className: 'update-action' });
  div.append(el('h3', { textContent: 'Updates' }), status, action);

  // The launch check is the only network call an unconfigured Glyphio makes, so it gets a
  // switch. Turning it off doesn't hide the button below — that one is the user asking.
  // This tab has no Save button, so the toggle applies as soon as it's flipped.
  const auto = renderField('checkForUpdates', 'toggle', 'Check for updates on launch');
  auto.classList.add('update-auto');
  auto.querySelector('input').addEventListener('change', async (e) => {
    const next = { ...state.settings, checkForUpdates: e.target.checked };
    try { await invoke('save_settings', { settings: next }); state.settings = next; }
    catch (err) { e.target.checked = !e.target.checked; setStatus(String(err), 'err'); }
  });
  div.append(auto);

  const show = (result) => {
    action.replaceChildren();
    if (!result) { status.textContent = 'Could not reach the update server.'; return; }
    switch (result.state) {
      case 'upToDate':
        status.textContent = `Glyphio ${result.version} is the latest version.`;
        break;
      case 'available': {
        status.innerHTML = `<strong>Version ${escapeHtml(result.version)} is available.</strong>` +
          (result.notes ? ` ${escapeHtml(result.notes)}` : '');
        const go = el('button', { className: 'primary', textContent: `Update to ${result.version}` });
        go.addEventListener('click', async () => {
          go.disabled = true;
          go.textContent = 'Downloading…';
          try {
            // Restarts into the new version on success, so nothing after this runs.
            await invoke('install_update');
          } catch (e) {
            go.disabled = false;
            go.textContent = `Update to ${result.version}`;
            status.textContent = `Update failed: ${e}`;
          }
        });
        action.append(go);
        break;
      }
      case 'managedElsewhere':
        // Homebrew owns this install; self-updating would leave brew's records wrong.
        status.innerHTML = `Version ${escapeHtml(result.version)} is available. This copy was ` +
          `installed with Homebrew, so update it the same way:`;
        action.append(el('code', { className: 'update-command', textContent: result.command }));
        break;
      default:
        status.textContent = 'Could not check for updates right now.';
    }
  };

  invoke('check_for_update').then(show).catch(() => show(null));
  return div;
}

function renderField(key, type, label, opts) {
  const field = document.createElement('div');
  field.className = 'field';
  const name = document.createElement('label');
  name.className = 'name'; name.textContent = label;
  field.append(name);
  // A capture mode's two keys, side by side: one opens the editor, one goes straight to the
  // clipboard. Seeing them together is what makes the second one discoverable at all.
  if (type === 'hotkeys') {
    field.append(hotkeyPair(key, opts, label));
    return field;
  }
  // One app-wide shortcut, recorded from a key press exactly like the capture shortcuts.
  if (type === 'hotkey') {
    field.append(shortcutRecorder(key, label, '', 'Not set'));
    return field;
  }
  let input;
  if (type === 'toggle') { input = el('input', { type: 'checkbox' }); input.checked = Boolean(state.settings[key]); }
  else if (type === 'select') { input = renderSelect(key, opts); }
  else if (type === 'color') { input = el('input', { type: 'color', className: 'color' }); input.value = state.settings[key] || '#000000'; }
  else if (type === 'number') { input = el('input', { type: 'number', min: '1' }); input.value = state.settings[key]; }
  // A list the user edits as lines, stored as an array. One per line beats comma-separated
  // for names that may contain commas — and app names do.
  else if (type === 'lines') {
    input = el('textarea', { rows: 4, spellcheck: false });
    input.value = (state.settings[key] ?? []).join('\n');
  }
  else { input = el('input', { type: 'text' }); input.value = state.settings[key] ?? ''; }
  input.dataset.key = key; input.dataset.type = type;
  field.append(input);
  if (key.startsWith('shortcut')) field.append(shortcutWarning(key));
  return field;
}

/** The "not registered" line under one shortcut, kept current by `paintShortcutStatus`. */
function shortcutWarning(key) {
  const span = el('span', { className: 'field-error shortcut-warning' });
  span.dataset.statusKey = key;
  span.textContent = registrationNotice(statusFor(state.shortcutStatus, key));
  return span;
}

/** Show what the system said about each shortcut after a save. */
function paintShortcutStatus(statuses) {
  state.shortcutStatus = statuses ?? [];
  document.querySelectorAll('[data-status-key]').forEach((span) => {
    span.textContent = registrationNotice(statusFor(state.shortcutStatus, span.dataset.statusKey));
  });
}

/** The editor key and the straight-to-clipboard key for one capture mode. */
function hotkeyPair(key, silentKey, label) {
  const wrap = el('div', { className: 'hotkey-pair' });
  wrap.append(
    shortcutRecorder(key, label, 'opens the editor', ''),
    shortcutRecorder(silentKey, label, 'to the clipboard', 'not set'),
  );
  return wrap;
}

/**
 * One shortcut field: select it, press the keys together, and the chord is validated and
 * stored in a hidden input that `saveSettings` collects like any other field.
 */
function shortcutRecorder(settingsKey, label, caption, placeholder) {
  const spoken = caption ? `${label}, ${caption}` : label;
  const box = el('div', { className: 'shortcut-box' });
  const input = el('input', { type: 'hidden' });
  input.value = state.settings[settingsKey] ?? '';
  input.dataset.key = settingsKey;
  input.dataset.type = 'text';
  input.dataset.label = caption ? `${label} (${caption})` : label;

  const record = el('button', {
    type: 'button',
    className: 'shortcut-recorder',
    textContent: input.value ? formatAccelerator(input.value) : (placeholder || 'Not set'),
    title: `Record ${label.toLowerCase()} shortcut${caption ? ` ${caption}` : ''}`,
  });
  if (input.value) record.dataset.accelerator = input.value;
  record.setAttribute('aria-label', `${spoken}: ${record.textContent}. Select to record.`);
  const error = el('span', { className: 'field-error shortcut-error' });
  error.setAttribute('role', 'alert');

  const showValue = () => {
    shortcutCaptureController.stop(recorderSession);
    record.textContent = input.value ? formatAccelerator(input.value) : (placeholder || 'Not set');
    if (input.value) record.dataset.accelerator = input.value;
    else delete record.dataset.accelerator;
    record.setAttribute('aria-label', `${spoken}: ${record.textContent}. Select to record.`);
    delete record.dataset.recording;
  };
  const showError = (message) => { error.textContent = message; delete error.dataset.ok; };
  const beginRecording = () => {
    record.dataset.recording = 'yes';
    record.textContent = 'Press shortcut…';
    record.setAttribute('aria-label', `${spoken}: press a shortcut. Escape cancels.`);
    showError('Press the keys together. Escape cancels.');
    shortcutCaptureController.start(recorderSession);
    record.focus({ preventScroll: true });
  };

  const recorderSession = {
    cancel() {
      showValue();
      showError('');
    },
    handleKeydown(event) {
      if (event.code === 'Escape') {
        showValue();
        showError('');
        return;
      }
      const recorded = acceleratorFromKeyboardEvent(event);
      if (recorded.error) {
        showError(recorded.error);
        return;
      }
      const result = validateAccelerator(recorded.accelerator, configuredShortcuts(settingsKey), PLATFORM_ID);
      if (!result.ok) {
        showError(result.error);
        return;
      }
      input.value = recorded.accelerator;
      showValue();
      showError('');
    },
  };

  record.addEventListener('click', () => {
    if (record.dataset.recording === 'yes') {
      showValue();
      showError('');
    } else beginRecording();
  });
  record.addEventListener('blur', () => {
    if (record.dataset.recording === 'yes') {
      showValue();
      showError('');
    }
  });

  const clear = el('button', { type: 'button', className: 'ghost shortcut-action', textContent: 'Clear' });
  clear.addEventListener('click', () => {
    input.value = '';
    showValue();
    showError('');
  });
  const reset = el('button', { type: 'button', className: 'ghost shortcut-action', textContent: 'Reset' });
  reset.addEventListener('click', () => {
    input.value = SHORTCUT_DEFAULTS[settingsKey] ?? '';
    showValue();
    const result = validateAccelerator(input.value, configuredShortcuts(settingsKey), PLATFORM_ID);
    showError(result.ok ? '' : result.error);
  });
  // Listens for the saved chord through the real handler, so it tells "not registered" apart
  // from "registered, but something in front takes the press".
  const testButton = el('button', { type: 'button', className: 'ghost shortcut-action', textContent: 'Test' });
  testButton.addEventListener('click', async () => {
    if (input.value !== (state.settings[settingsKey] ?? '')) {
      showError('Save settings first, then test.');
      return;
    }
    testButton.disabled = true;
    showError(`Press ${input.value ? formatAccelerator(input.value) : 'it'} now…`);
    try {
      const result = testMessage(await invoke('shortcut_test', { key: settingsKey }));
      showError(result.text);
      error.dataset.ok = result.ok ? 'yes' : 'no';
    } catch (e) {
      showError(`Test failed: ${e}`);
    }
    testButton.disabled = false;
  });
  const actions = el('span', { className: 'shortcut-actions' });
  actions.append(testButton, clear, reset);
  box.append(input, record, actions);
  if (caption) box.append(el('span', { className: 'hotkey-caption', textContent: caption }));
  box.append(error, shortcutWarning(settingsKey));
  return box;
}

/** Current values for every other Glyphio shortcut, including fields on other settings tabs. */
function configuredShortcuts(exceptKey) {
  const values = new Map(Object.entries(state.settings ?? {})
    .filter(([key]) => key.startsWith('shortcut'))
    .map(([key, value]) => [key, { key, value, label: shortcutLabel(key) }]));
  document.querySelectorAll('#settings-form [data-key]').forEach((input) => {
    const key = input.dataset.key;
    if (key.startsWith('shortcut')) {
      values.set(key, { key, value: input.value, label: input.dataset.label || shortcutLabel(key) });
    }
  });
  values.delete(exceptKey);
  return [...values.values()];
}

function shortcutLabel(key) {
  const words = key.replace(/^shortcut/, '').replace(/([a-z])([A-Z])/g, '$1 $2').toLowerCase();
  return words || 'another Glyphio shortcut';
}

function validateCaptureRecorders() {
  let valid = true;
  document.querySelectorAll('#settings-form .shortcut-box').forEach((box) => {
    const input = box.querySelector('input[data-key]');
    const error = box.querySelector('.shortcut-error');
    const result = validateAccelerator(input.value, configuredShortcuts(input.dataset.key), PLATFORM_ID);
    error.textContent = result.ok ? '' : result.error;
    if (!result.ok) valid = false;
  });
  return valid;
}

/**
 * A `<select>` for `key`. `opts` is either a list of plain values or a function returning
 * `{ value, label }` entries, optionally wrapped in `{ group, items }` for `<optgroup>`s.
 * A stored value the list doesn't offer is kept and shown first rather than silently
 * rewritten — except that an unusable one says so, since it is why a timestamp went missing.
 */
function renderSelect(key, opts) {
  const spec = typeof opts === 'function' ? opts() : opts.map((o) => ({ value: o, label: o }));
  const select = el('select');
  const current = state.settings[key] ?? '';
  let offered = false;
  const add = (parent, o) => {
    parent.append(el('option', { value: o.value, textContent: o.label }));
    if (o.value === current) offered = true;
  };
  for (const entry of spec) {
    if (entry.group) {
      const group = el('optgroup', { label: entry.group });
      for (const item of entry.items) add(group, item);
      select.append(group);
    } else add(select, entry);
  }
  if (!offered && current !== '') {
    const usable = key === 'timezone' ? isSupportedTimezone(current)
      : key === 'locale' ? isSupportedLocale(current) : true;
    select.prepend(el('option', {
      value: current,
      textContent: usable ? current : `${current} — not recognised, using device default`,
    }));
  }
  select.value = current;
  return select;
}

/**
 * Every IANA zone this Mac knows, grouped by region. Typing one by hand was a trap: `Intl`
 * rejects anything that isn't an exact zone name, and a rejected zone took the whole
 * timestamp strip with it.
 */
function timezoneOptions() {
  const zones = typeof Intl.supportedValuesOf === 'function'
    ? Intl.supportedValuesOf('timeZone') : [];
  const groups = new Map();
  for (const zone of zones) {
    if (!zone.includes('/')) continue; // UTC and legacy aliases — pinned above instead
    const [region, ...rest] = zone.split('/');
    if (!groups.has(region)) groups.set(region, []);
    groups.get(region).push({ value: zone, label: rest.join(' / ').replace(/_/g, ' ') });
  }
  return [
    { value: 'device', label: `Device default (${deviceTimezone()})` },
    { value: 'UTC', label: 'UTC' },
    ...[...groups.entries()].map(([region, items]) => ({ group: region.replace(/_/g, ' '), items })),
  ];
}

function deviceTimezone() {
  try { return Intl.DateTimeFormat().resolvedOptions().timeZone || 'device'; }
  catch { return 'device'; }
}

/**
 * Locales worth offering, named in their own language. There is no way to enumerate what
 * `Intl` supports, so this is a list — anything already stored survives (see `renderSelect`).
 */
const LOCALE_TAGS = [
  'en-US', 'en-GB', 'en-AU', 'en-CA', 'en-IN', 'fr-FR', 'fr-CA', 'de-DE', 'de-CH', 'es-ES',
  'es-MX', 'pt-BR', 'pt-PT', 'it-IT', 'nl-NL', 'sv-SE', 'nb-NO', 'da-DK', 'fi-FI', 'is-IS',
  'pl-PL', 'cs-CZ', 'sk-SK', 'hu-HU', 'ro-RO', 'el-GR', 'tr-TR', 'ru-RU', 'uk-UA', 'he-IL',
  'ar-SA', 'fa-IR', 'hi-IN', 'bn-IN', 'ta-IN', 'th-TH', 'vi-VN', 'id-ID', 'ms-MY', 'ja-JP',
  'ko-KR', 'zh-CN', 'zh-TW', 'zh-HK',
];

function localeOptions() {
  const label = (tag) => {
    try {
      const name = new Intl.DisplayNames([tag], { type: 'language' }).of(tag);
      return name && name !== tag ? `${name} (${tag})` : tag;
    } catch { return tag; }
  };
  return [
    { value: 'device', label: `Device default (${deviceLocale()})` },
    ...LOCALE_TAGS.filter(isSupportedLocale).map((tag) => ({ value: tag, label: label(tag) })),
  ];
}

function deviceLocale() {
  try { return new Intl.DateTimeFormat().resolvedOptions().locale || 'device'; }
  catch { return 'device'; }
}

async function saveSettings() {
  if (!validateCaptureRecorders()) {
    setStatus('Fix the highlighted shortcuts before saving.', 'err');
    return;
  }
  const next = { ...state.settings };
  document.querySelectorAll('#settings-form [data-key]').forEach((el) => {
    const { key, type } = el.dataset;
    if (type === 'toggle') next[key] = el.checked;
    else if (type === 'number') next[key] = parseInt(el.value, 10) || state.settings[key];
    else if (type === 'lines') {
      next[key] = el.value.split('\n').map((s) => s.trim()).filter(Boolean);
    }
    else next[key] = el.value;
  });
  const deliveryError = document.querySelector('.delivery-error');
  // Other Settings tabs share this save function. Only replace the override mapping while its
  // editor is mounted; saving Capture or Clipboard must not erase app exceptions.
  if (deliveryError) {
    const appOverrides = {};
    for (const row of document.querySelectorAll('.delivery-app-row')) {
      const bundleId = row.querySelector('[data-delivery-bundle]').value.trim();
      const delivery = row.querySelector('[data-delivery-mode]').value;
      if (!/^[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)+$/.test(bundleId)) {
        deliveryError.textContent = 'Enter a bundle identifier such as com.apple.TextEdit.';
        row.querySelector('[data-delivery-bundle]').focus();
        return;
      }
      if (Object.prototype.hasOwnProperty.call(appOverrides, bundleId)) {
        deliveryError.textContent = `${bundleId} is already listed.`;
        row.querySelector('[data-delivery-bundle]').focus();
        return;
      }
      appOverrides[bundleId] = delivery;
    }
    next.expansionAppOverrides = appOverrides;
  }
  try {
    const registered = await invoke('save_settings', { settings: next });
    state.settings = next;
    paintShortcutStatus(registered);
    const refused = (registered ?? []).filter((s) => registrationNotice(s)).length;
    setStatus(refused ? 'Settings saved. Some shortcuts could not be registered.' : 'Settings saved.',
      refused ? 'err' : 'ok');
  }
  catch (e) { setStatus(String(e), 'err'); }
  refreshLoginNotice();
}
