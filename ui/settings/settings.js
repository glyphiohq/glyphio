// settings/settings.js — Glyphio main window.
// Sidebar (groups) → snippet list → editor, plus a Settings view.
// All snippet edits go through the SQLite store; every change regenerates the expansion
// engine's config, which hot-reloads. Rich snippets use the engine's native rich injection.

import { icon } from '../shared/icons.js';
import { resolveSettings } from '../config.js';
import { compositeBanner, isSupportedLocale, isSupportedTimezone } from '../shared/banner.js';
import { escapeHtml, escapeAttr, mdToHtml, sanitizeSnippetHtml } from '../shared/markdown.js';

const { invoke, convertFileSrc } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const app = document.getElementById('app');

const state = {
  snippets: [],
  groups: [],
  settings: null,
  selected: 'all', // 'all' | 'ungrouped' | 'settings' | <groupId>
  settingsTab: 'capture',
  search: '',
  groupSearch: '',        // sidebar group filter
  syncStatus: null,
  syncConfig: null,
  selectedTeam: null,     // team whose roster is shown in the sync panel
  teamMembers: {},        // team -> [{sub,email,lastSeen}]
  memberSearch: '',
};

const FORMATS = [
  { id: 'plain', label: 'Plain', badge: 'Aa' },
  { id: 'html', label: 'Rich', badge: '❛❜' },
  { id: 'markdown', label: 'Markdown', badge: 'M↓' },
];

const KINDS = [
  { id: 'text', label: 'Text' },
  { id: 'form', label: 'Form' },
  { id: 'popup', label: 'Popup' },
  { id: 'command', label: 'Command' },
];

const KIND_HINTS = {
  text: 'The content replaces the trigger.',
  form: 'The trigger opens a form; the filled-in content is pasted.',
  popup: 'The trigger opens a popup showing the content. Nothing is pasted.',
  command: 'Pastes the output of a shell command. Runs on this device only and never syncs.',
};

const CAPTURE_SECTIONS = [
  { title: 'Capture modes', fields: [
    ['enableVisibleCapture', 'toggle', 'Visible-area (current screen)'],
    ['enableSnipCapture', 'toggle', 'Region / snip (picker)'],
    ['enableFullWindowCapture', 'toggle', 'Full window (picker)'],
    ['enableFrontWindowCapture', 'toggle', 'Frontmost window (no picker — e.g. just the browser)'],
    ['enableScrollingCapture', 'toggle', 'Scrolling page / panel (stitch)'],
  ]},
  { title: 'After a capture', hint: `A silent capture skips the editor and goes straight to
    the clipboard. You can take one whenever you like — <kbd>⌘↩</kbd> on any row of the
    palette's <em>Capture</em> list, or a hotkey of its own below — so the setting here is only
    about what the <em>ordinary</em> capture keys do. Either way it still lands in history:
    open it from <strong>History</strong> to annotate it later.`, fields: [
    ['silentCapture', 'toggle', 'Make every capture silent by default'],
    ['autoCopyOnOpen', 'toggle', 'Auto-copy when the editor opens'],
    ['historyEnabled', 'toggle', 'Save captures to history'],
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
  { title: 'Details on the capture', hint: `What else the strip says about what you captured.
    The page details come from the browser and are recorded for the modes that target one
    window — <em>frontmost window</em>, <em>browser page</em> and <em>scrolling page</em>; a
    picked region has no single window to ask. They are off by default: a URL or a profile
    name baked into a screenshot travels with it wherever you paste it.`, fields: [
    ['showWindowTitle', 'toggle', 'Window / app title'],
    ['showPageTitle', 'toggle', 'Page title'],
    ['showPageUrl', 'toggle', 'Page address (URL)'],
    ['showBrowserProfile', 'toggle', 'Browser profile'],
  ]},
  { title: 'Edit tools', fields: [
    ['enableCrop', 'toggle', 'Crop'], ['enableRedact', 'toggle', 'Redact'],
    ['enableDraw', 'toggle', 'Draw'], ['enableText', 'toggle', 'Text labels'],
  ]},
  { title: 'Capture hotkeys (e.g. Alt+Shift+S)', hint: `Each mode can have a second key that
    takes the same shot <strong>straight to the clipboard</strong>, with no editor window —
    the same thing as <kbd>⌘↩</kbd> on that mode's row in the palette. Leave it blank if you
    don't want one.`, fields: [
    ['shortcutCaptureFull', 'hotkeys', 'Full window', 'shortcutCaptureFullSilent'],
    ['shortcutCaptureVisible', 'hotkeys', 'Visible area', 'shortcutCaptureVisibleSilent'],
    ['shortcutCaptureSnip', 'hotkeys', 'Region (snip)', 'shortcutCaptureSnipSilent'],
    ['shortcutCaptureFrontWindow', 'hotkeys', 'Frontmost window', 'shortcutCaptureFrontWindowSilent'],
    ['shortcutCapturePage', 'hotkeys', 'Browser page (content only)', 'shortcutCapturePageSilent'],
    ['shortcutCaptureScroll', 'hotkeys', 'Scrolling area', 'shortcutCaptureScrollSilent'],
    ['shortcutCaptureScrollPage', 'hotkeys', 'Scrolling page (frontmost window)', 'shortcutCaptureScrollPageSilent'],
    ['shortcutOpenHistory', 'text', 'Open capture history'],
  ]},
];

const SNIPPET_SECTIONS = [
  { title: 'Hotkeys', fields: [
    ['shortcutOpenPalette', 'text', 'Snippet search (Spotlight-style)'],
  ]},
];

const CLIPBOARD_SECTIONS = [
  { title: 'Clipboard history', hint: `Everything you copy, kept on this device so you can
    paste it again later. <strong>Content a password manager marks as concealed is never
    recorded</strong>, and neither is anything marked transient — that marking is the
    convention clipboard tools use, and Glyphio honours it. Nothing here is ever synced or
    sent anywhere: there is no code that could.`, fields: [
    ['clipboardHistory', 'toggle', 'Remember what I copy'],
    ['clipboardMaxItems', 'number', 'Entries kept'],
    ['clipboardMaxMb', 'number', 'Megabytes of copied images kept'],
    ['shortcutOpenClipboard', 'text', 'Open clipboard history'],
  ]},
  { title: 'Never record from these apps', hint: `One app name per line, matched loosely —
    <code>bank</code> catches “Banking”. Password managers already mark their own content and
    don't need listing.`, fields: [
    ['clipboardIgnoreApps', 'lines', 'Ignored apps'],
  ]},
];

init().catch((e) => setStatus(e.message, 'err'));

async function init() {
  renderShell();
  await reloadAll();
  wireAccessibility();
  wireScreenRecording();
  await wireSync();
  wireInvites();
  maybeShowWelcome();
  await listen('snippets-changed', reloadSnippets);
  await listen('groups-changed', reloadGroups);
  await listen('settings-changed', reloadAll); // tray "Reload" refreshes everything
}

async function reloadAll() {
  [state.settings, state.snippets, state.groups] = await Promise.all([
    invoke('get_settings'), invoke('list_snippets'), invoke('list_groups'),
  ]);
  renderSidebar();
  renderMain();
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
    <div class="ax-banner" id="ax-banner">
      <div class="ax-text">
        <strong>Text expansion is off — grant Accessibility to Glyphio.</strong>
        <p>Click <strong>Grant access…</strong> and toggle <strong>Glyphio</strong> on in the dialog that opens — macOS adds the entry for you (never use the “+” button; old <em>glyphio-engine</em> entries can be removed, they do nothing). Expansion turns on by itself within a couple of seconds.</p>
      </div>
      <div class="ax-actions">
        <button class="primary" id="ax-grant">Grant access…</button>
        <button class="secondary" id="ax-open">Open Accessibility settings</button>
        <button class="ghost" id="ax-restart">Restart engine</button>
      </div>
    </div>
    <div class="ax-banner" id="sr-banner">
      <div class="ax-text">
        <strong>Screen capture is off — grant macOS Screen Recording.</strong>
        <p>Click <strong>Grant access…</strong> and allow <strong>Glyphio</strong> in the dialog — macOS adds the entry for you. Then click <strong>Relaunch Glyphio</strong>: macOS applies this permission on the next launch. (Old or duplicate Glyphio rows in the settings list can be removed with “−”.)</p>
      </div>
      <div class="ax-actions">
        <button class="primary" id="sr-grant">Grant access…</button>
        <button class="secondary" id="sr-relaunch">Relaunch Glyphio</button>
        <button class="ghost" id="sr-open">Open settings</button>
      </div>
    </div>
    <div class="ax-banner" id="si-banner">
      <div class="ax-text">
        <strong>Expansion paused — <span id="si-holder">another app</span> is holding macOS Secure Input.</strong>
        <p>While an app captures keystrokes securely (password fields, the lock screen), no expander can see what you type — triggers resume the moment it lets go. If this persists and no password field is open, the hold is stale: lock the screen (Ctrl+Cmd+Q) and unlock, or quit the app shown above.</p>
      </div>
    </div>
    <div class="body">
      <aside class="sidebar" id="sidebar"></aside>
      <main class="main" id="main"></main>
    </div>
    <div class="status-line" id="status"></div>
  `;
  document.getElementById('ax-grant').addEventListener('click', () => { invoke('request_accessibility'); startAxPolling(); });
  document.getElementById('ax-open').addEventListener('click', () => { invoke('open_accessibility_settings'); startAxPolling(); });
  document.getElementById('ax-restart').addEventListener('click', async () => {
    await invoke('restart_engine'); setStatus('Engine restarted.', 'ok'); startAxPolling();
  });
  document.getElementById('sr-grant').addEventListener('click', async () => {
    const granted = await invoke('request_screen_recording');
    applySr(granted);
    if (!granted) setStatus('After allowing in the dialog, click “Relaunch Glyphio” to apply the permission.', 'ok');
  });
  document.getElementById('sr-open').addEventListener('click', () => invoke('open_screen_recording_settings'));
  document.getElementById('sr-relaunch').addEventListener('click', () => invoke('relaunch_app'));
}

// --- Accessibility status ---------------------------------------------------
// The engine re-checks Accessibility every ~2s and auto-restarts its worker on a grant, emitting
// `accessibility-status`. We also poll/re-query here so the banner clears promptly when the user
// returns from System Settings (macOS doesn't notify the app of a permission change).

let axPollTimer = null;

function applyAx(granted) {
  const banner = document.getElementById('ax-banner');
  if (banner) banner.classList.toggle('show', !granted);
  if (granted) stopAxPolling();
}
async function recheckAx() {
  try { applyAx(await invoke('accessibility_status')); } catch { /* window closing */ }
}
function startAxPolling() {
  if (axPollTimer) return;
  axPollTimer = setInterval(recheckAx, 2000);
}
function stopAxPolling() {
  if (axPollTimer) { clearInterval(axPollTimer); axPollTimer = null; }
}

// Secure Input: while ANY app holds it (password fields, a stale lock-screen grab), no
// expander on the system can see keystrokes — typed triggers pause. Show who holds it.
function applySecureInput(holder) {
  const banner = document.getElementById('si-banner');
  if (!banner) return;
  banner.classList.toggle('show', Boolean(holder));
  if (holder) document.getElementById('si-holder').textContent = holder;
}

async function wireAccessibility() {
  const granted = await invoke('accessibility_status');
  applyAx(granted);
  if (!granted) startAxPolling();
  await listen('accessibility-status', (e) => applyAx(Boolean(e.payload)));
  applySecureInput(await invoke('secure_input_status'));
  await listen('secure-input-status', (e) => applySecureInput(e.payload));
  // Returning from System Settings re-focuses the window — re-check then.
  window.addEventListener('focus', recheckAx);
  document.addEventListener('visibilitychange', () => { if (!document.hidden) recheckAx(); });
}

// --- Screen Recording status --------------------------------------------------
// Checked via CGPreflightScreenCaptureAccess (no prompt). Unlike Accessibility, macOS only
// applies a Screen Recording grant on the app's next launch, so the banner mainly guides the
// user through request → allow → relaunch.

let srPollTimer = null;

function applySr(granted) {
  const banner = document.getElementById('sr-banner');
  if (banner) banner.classList.toggle('show', !granted);
  if (granted && srPollTimer) { clearInterval(srPollTimer); srPollTimer = null; }
  if (!granted && !srPollTimer) {
    srPollTimer = setInterval(async () => {
      try { applySr(await invoke('screen_recording_status')); } catch { /* window closing */ }
    }, 3000);
  }
}

async function wireScreenRecording() {
  applySr(await invoke('screen_recording_status'));
  window.addEventListener('focus', async () => applySr(await invoke('screen_recording_status')));
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
    <p class="adv-hint">The group and its snippets sync with the selected team. Members see them
    per their role; access can be restricted per member from the admin dashboard.</p>
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

async function renderHistory(main) {
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
        const buf = new Uint8Array(await blob.arrayBuffer());
        let bin = '';
        for (const b of buf) bin += String.fromCharCode(b);
        await invoke('save_file', { path, pngBase64: btoa(bin) });
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

// Exportable PNG for a history row. New-format rows store content-only pixels, so the
// banner (timestamp from the original capturedAt + title + note) is composited here when
// enabled; legacy rows already contain it.
async function exportDataUrl(item) {
  const dataUrl = await invoke('read_capture_data_url', { id: item.id });
  if (item.bannerBaked || item.bannerEnabled === false) return dataUrl;
  const settings = resolveSettings(state.settings || {});
  const bmp = await createImageBitmap(await (await fetch(dataUrl)).blob());
  const cvs = document.createElement('canvas');
  compositeBanner(cvs, bmp, {
    meta: { ...bannerMeta(item), dpr: item.dpr || 1 },
    settings,
    note: item.note || '',
    enabled: true,
  });
  bmp.close?.();
  return cvs.toDataURL('image/png');
}

// Thumbnails are stored content-only; composite the banner at thumbnail scale so the grid
// preview matches what Open/Copy/Save produce (timestamp strip included).
async function bannerizeThumb(item, img) {
  if (item.bannerBaked || item.bannerEnabled === false || !item.thumbPath) return;
  const settings = resolveSettings(state.settings || {});
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
    mk('copy', 'Copy to clipboard', async () => {
      try {
        const blob = await (await fetch(await exportDataUrl(item))).blob();
        await navigator.clipboard.write([new ClipboardItem({ 'image/png': blob })]);
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
        await invoke('save_file', { path, pngBase64: await exportDataUrl(item) });
        setStatus(`Saved → ${path}`, 'ok');
      } catch (e) { setStatus(String(e), 'err'); }
    }),
    mk('trash', 'Delete', async () => { await invoke('delete_capture', { id: item.id }); redraw(); }, 'danger'),
  );
  li.append(img, meta, actions);
  return li;
}

// --- Invites & first-run welcome ----------------------------------------------
// glyphio://join links land here (from the OS handler) and from the paste box. NOTHING is
// applied without explicit confirmation — a URL scheme can be fired by any webpage, and
// silently switching the sync server would be a data-redirection attack.

async function wireInvites() {
  await listen('invite-link', (e) => confirmInvite(String(e.payload)));
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
    <p class="adv-hint">Only accept invites from your own team. Personal snippets and captures
    never leave this device either way.</p>
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

// --- Editor (two-pane modal: form + live preview) ---------------------------

const FMT_HINTS = {
  plain: 'Inserted exactly as typed.',
  html: 'Rich text — pastes with formatting. Select an image, then use the resize button in the toolbar.',
  markdown: 'Markdown source, pasted as rich text.',
};

function openEditor(existing) {
  const isEdit = Boolean(existing);
  let format = existing?.format || 'plain';
  let kind = existing?.kind || 'text';
  let htmlView = 'rich'; // for format=html: 'rich' (WYSIWYG) | 'source' (raw HTML)
  let savedRange = null; // caret saved before a toolbar popover steals focus
  let selectionHandler = null;
  // Form-kind field builder state ({name,label,type,options}); stored as variables.fields.
  let formFields = (kind === 'form' && Array.isArray(existing?.variables?.fields))
    ? existing.variables.fields.map((f) => ({ ...f }))
    : [];

  const groupOptions = `<option value="">Ungrouped</option>` +
    state.groups.map((g) => `<option value="${g.id}">${escapeHtml(g.name)}</option>`).join('');

  const { modal, close, $ } = openModal(`
    <div class="modal-head">
      <h3>${isEdit ? 'Edit snippet' : 'New snippet'}</h3>
      <button class="icon-btn" id="e-close" title="Close (Esc)" aria-label="Close">${icon('x', 15)}</button>
    </div>
    <div class="editor-panes">
      <div class="editor-form">
        <div class="mfield">
          <label for="e-trigger">Trigger</label>
          <input id="e-trigger" type="text" placeholder=":sig" autocomplete="off" spellcheck="false" autocapitalize="off" />
          <p class="fmt-hint">Lowercase, no spaces — you type this inline to expand.</p>
          <p class="field-error" id="e-trigger-err"></p>
        </div>
        <div class="mfield">
          <label>Type</label>
          <div class="seg" id="e-kind"></div>
          <p class="fmt-hint" id="e-kind-hint"></p>
        </div>
        <div class="mfield">
          <label for="e-group">Group</label>
          <select id="e-group">${groupOptions}</select>
          <p class="fmt-hint" id="e-group-hint" style="display:none">Command snippets stay personal — they never sync, even inside a shared group.</p>
        </div>
        <div class="mfield" id="e-fmt-row">
          <label>Format</label>
          <div class="seg" id="e-fmt"></div>
          <p class="fmt-hint" id="e-fmt-hint"></p>
        </div>
        <div class="mfield" id="e-shell-row" style="display:none">
          <label for="e-shell">Shell</label>
          <select id="e-shell">
            <option value="sh">sh (default)</option>
            <option value="bash">bash</option>
            <option value="zsh">zsh</option>
          </select>
        </div>
        <div class="mfield" id="e-fields-row" style="display:none">
          <label>Form fields</label>
          <p class="adv-hint">Reference a field in the content as <code>{{name}}</code> — double braces, and the <em>name</em>, not the label. Names take letters, digits, <code>_</code>, <code>.</code> and <code>-</code>; no spaces. No fields? Each <code>{{placeholder}}</code> in the content becomes a text input automatically.</p>
          <div id="e-fields"></div>
          <button type="button" class="secondary sm" id="e-field-add">Add field</button>
          <p class="field-error" id="e-fields-err"></p>
        </div>
        <div class="mfield content-field">
          <div class="content-head">
            <label>Content</label>
            <div class="seg seg-mini" id="e-htmlview" style="display:none">
              <button type="button" class="seg-opt" data-view="rich">Rich</button>
              <button type="button" class="seg-opt" data-view="source">HTML</button>
            </div>
          </div>
          <div class="rich-toolbar" id="e-rich-toolbar"></div>
          <div id="e-body-wrap"></div>
          <p class="field-error" id="e-body-err"></p>
        </div>
        <details class="adv" id="e-adv">
          <summary>Variables <span class="adv-sub">— dynamic values (advanced)</span></summary>
          <p class="adv-hint">Dynamic values inserted at expansion time. A JSON array; reference them in your content as <code>{{name}}</code>.</p>
          <textarea id="e-vars" class="mono" spellcheck="false" placeholder='[{"name":"date","type":"date","params":{"format":"%Y-%m-%d"}}]'></textarea>
          <p class="field-error" id="e-vars-err"></p>
        </details>
        <details class="adv" id="e-scope-adv">
          <summary>App scope <span class="adv-sub">— limit where this snippet expands</span></summary>
          <p class="adv-hint">Empty = everywhere. An app name matches the app's executable (e.g. <code>Slack</code>); or use <code>exec:&lt;regex&gt;</code> / <code>title:&lt;regex&gt;</code> for full control.</p>
          <input id="e-scope" type="text" autocomplete="off" spellcheck="false" placeholder="e.g. Slack" />
        </details>
      </div>
      <div class="editor-preview">
        <div class="preview-head">Live preview <span class="preview-sub" id="e-preview-fmt"></span></div>
        <div class="preview-body" id="e-preview"></div>
      </div>
    </div>
    <div class="modal-actions">
      <span class="save-hint">⌘↵ save · Esc cancel</span>
      <div class="spacer"></div>
      <button class="secondary" id="e-cancel">Cancel</button>
      <button class="primary" id="e-save">${isEdit ? 'Save' : 'Create'}</button>
    </div>`, { className: 'editor2', onClose: cleanup });

  function cleanup() {
    if (selectionHandler) { document.removeEventListener('selectionchange', selectionHandler); selectionHandler = null; }
  }

  const gsel = $('#e-group');
  let defaultGroup = '';
  if (state.selected.startsWith('team:')) {
    // Creating from a team view: default into that team's first shared group so the new
    // snippet inherits the team and syncs immediately.
    defaultGroup = state.groups.find((g) => g.team === state.selected.slice(5))?.id || '';
  } else if (!['all', 'ungrouped', 'settings', 'history'].includes(state.selected)) {
    defaultGroup = state.selected;
  }
  gsel.value = existing?.groupId || defaultGroup;
  $('#e-trigger').value = existing?.trigger || '';
  $('#e-vars').value = existing?.variables ? JSON.stringify(existing.variables, null, 2) : '';
  $('#e-scope').value = existing?.appScope || '';
  if (existing?.appScope) $('#e-scope-adv').open = true;

  const fmtSeg = $('#e-fmt');
  fmtSeg.innerHTML = FORMATS.map((f) => `<button type="button" class="seg-opt" data-fmt="${f.id}">${f.label}</button>`).join('');
  const kindSeg = $('#e-kind');
  kindSeg.innerHTML = KINDS.map((k) => `<button type="button" class="seg-opt" data-kind="${k.id}">${k.label}</button>`).join('');
  const bodyWrap = $('#e-body-wrap');
  const toolbar = $('#e-rich-toolbar');
  const hint = $('#e-fmt-hint');
  const kindHint = $('#e-kind-hint');
  const preview = $('#e-preview');
  const previewFmt = $('#e-preview-fmt');
  const htmlViewSeg = $('#e-htmlview');

  const getRich = () => bodyWrap.querySelector('.rich-editor');
  const readBody = () => {
    if (kind !== 'command' && format === 'html' && htmlView === 'rich') return getRich().innerHTML.trim();
    return bodyWrap.querySelector('#e-body').value;
  };

  function updatePreview() {
    if (kind === 'command') {
      previewFmt.textContent = 'Command';
      preview.className = 'preview-body pv-plain';
      const cmd = readBody().trim();
      preview.textContent = cmd ? `$ ${cmd}\n→ output is pasted` : '';
      if (!cmd) preview.innerHTML = '<span class="pv-empty">Nothing to preview yet.</span>';
      return;
    }
    previewFmt.textContent = FORMATS.find((f) => f.id === format)?.label || '';
    renderPreview(preview, format, readBody());
  }

  function buildBody(fmt, initial) {
    fmtSeg.querySelectorAll('.seg-opt').forEach((b) => b.classList.toggle('active', b.dataset.fmt === fmt));
    hint.textContent = FMT_HINTS[fmt];
    cleanup();
    htmlViewSeg.style.display = kind !== 'command' && fmt === 'html' ? 'inline-flex' : 'none';
    htmlViewSeg.querySelectorAll('.seg-opt').forEach((b) => b.classList.toggle('active', b.dataset.view === htmlView));
    if (kind === 'command') {
      toolbar.style.display = 'none';
      bodyWrap.innerHTML = `<textarea id="e-body" class="body-area mono" placeholder="e.g. date +%Y-%m-%d" spellcheck="false"></textarea>`;
      const ta = bodyWrap.querySelector('#e-body');
      ta.value = initial || '';
      ta.addEventListener('input', updatePreview);
    } else if (fmt === 'html' && htmlView === 'source') {
      toolbar.style.display = 'none';
      bodyWrap.innerHTML = `<textarea id="e-body" class="body-area mono" placeholder="<p>Type HTML…</p>" spellcheck="false"></textarea>`;
      const ta = bodyWrap.querySelector('#e-body');
      ta.value = initial || '';
      ta.addEventListener('input', updatePreview);
    } else if (fmt === 'html') {
      toolbar.style.display = 'flex';
      bodyWrap.innerHTML = `<div class="rich-editor" contenteditable="true"></div>`;
      const ed = getRich();
      ed.innerHTML = sanitizeSnippetHtml(initial || '');
      const refresh = renderRichToolbar(toolbar, getRich, {
        onChange: updatePreview,
        saveSel: () => { const s = window.getSelection(); savedRange = s.rangeCount ? s.getRangeAt(0).cloneRange() : null; },
        restoreSel: () => { if (savedRange) { const s = window.getSelection(); s.removeAllRanges(); s.addRange(savedRange); } },
      });
      ed.addEventListener('input', updatePreview);
      // Pasting an image file inserts it inline (downscaled data URI) — same as the
      // toolbar's Insert image.
      ed.addEventListener('paste', (e) => {
        const file = [...(e.clipboardData?.files || [])].find((f) => f.type.startsWith('image/'));
        if (!file) return;
        e.preventDefault();
        insertImageFile(file, getRich, updatePreview);
      });
      // Clicking an inline image arms the toolbar's resize button (renderRichToolbar).
      selectionHandler = () => { if (document.activeElement === ed) refresh(); };
      document.addEventListener('selectionchange', selectionHandler);
    } else {
      toolbar.style.display = 'none';
      const ph = kind === 'form'
        ? 'Hi {{name}},\nthanks for reaching out…'
        : (fmt === 'markdown' ? 'Type Markdown…' : 'Type your snippet…');
      bodyWrap.innerHTML = `<textarea id="e-body" class="body-area ${fmt === 'markdown' ? 'mono' : ''}" placeholder="${escapeAttr(ph)}"></textarea>`;
      const ta = bodyWrap.querySelector('#e-body');
      ta.value = initial || '';
      ta.addEventListener('input', updatePreview);
    }
    updatePreview();
  }

  // Show/hide the kind-specific rows and relabel the content field.
  function applyKind(initialBody) {
    kindSeg.querySelectorAll('.seg-opt').forEach((b) => b.classList.toggle('active', b.dataset.kind === kind));
    kindHint.textContent = KIND_HINTS[kind];
    $('#e-fmt-row').style.display = kind === 'command' ? 'none' : '';
    $('#e-shell-row').style.display = kind === 'command' ? '' : 'none';
    $('#e-fields-row').style.display = kind === 'form' ? '' : 'none';
    $('#e-adv').style.display = kind === 'text' ? '' : 'none';
    $('#e-group-hint').style.display = kind === 'command' ? '' : 'none';
    modal.querySelector('.content-field label').textContent = kind === 'command' ? 'Command' : 'Content';
    if (kind === 'form') renderFieldBuilder();
    buildBody(format, initialBody);
  }

  // --- form-kind field builder ----------------------------------------------
  function renderFieldBuilder() {
    const box = $('#e-fields');
    box.innerHTML = '';
    formFields.forEach((f, i) => {
      const row = el('div', { className: 'field-row' });
      row.innerHTML = `
        <input type="text" data-k="name" placeholder="firstname" spellcheck="false" />
        <input type="text" data-k="label" placeholder="Label (optional)" />
        <select data-k="type">
          <option value="text">Text</option>
          <option value="multiline">Multiline</option>
          <option value="select">Choices</option>
        </select>
        <input type="text" data-k="options" placeholder="a, b, c" style="display:none" />
        <button type="button" class="ghost sm" data-rm title="Remove field">✕</button>`;
      row.querySelector('[data-k="name"]').value = f.name || '';
      row.querySelector('[data-k="label"]').value = f.label || '';
      row.querySelector('[data-k="type"]').value = f.type || 'text';
      const optInput = row.querySelector('[data-k="options"]');
      optInput.value = Array.isArray(f.options) ? f.options.join(', ') : '';
      optInput.style.display = (f.type === 'select') ? '' : 'none';
      // Refuse the characters a {{placeholder}} can't contain rather than accept them and
      // fail at expansion time. Attached first so the handler below reads the cleaned value.
      const nameInput = row.querySelector('[data-k="name"]');
      nameInput.addEventListener('input', () => {
        const typed = nameInput.value;
        const legal = typed.replace(/[^\w.-]/g, '');
        if (legal === typed) return;
        const caret = (nameInput.selectionStart ?? legal.length) - (typed.length - legal.length);
        nameInput.value = legal;
        nameInput.setSelectionRange(caret, caret);
      });
      row.querySelectorAll('[data-k]').forEach((input) => input.addEventListener('input', () => {
        f.name = row.querySelector('[data-k="name"]').value.trim();
        f.label = row.querySelector('[data-k="label"]').value.trim();
        f.type = row.querySelector('[data-k="type"]').value;
        f.options = optInput.value.split(',').map((s) => s.trim()).filter(Boolean);
        optInput.style.display = (f.type === 'select') ? '' : 'none';
      }));
      row.querySelector('[data-rm]').addEventListener('click', () => {
        formFields.splice(i, 1);
        renderFieldBuilder();
      });
      box.appendChild(row);
    });
  }
  $('#e-field-add').addEventListener('click', () => {
    formFields.push({ name: '', label: '', type: 'text', options: [] });
    renderFieldBuilder();
    $('#e-fields .field-row:last-child [data-k="name"]')?.focus();
  });

  if (kind === 'command' && existing?.variables?.shell) {
    $('#e-shell').value = existing.variables.shell;
  }

  applyKind(existing?.replacement || '');

  fmtSeg.querySelectorAll('.seg-opt').forEach((b) => b.addEventListener('click', () => {
    if (b.dataset.fmt === format) return;
    const converted = convertContent(readBody(), format, b.dataset.fmt);
    format = b.dataset.fmt;
    buildBody(format, converted);
  }));

  kindSeg.querySelectorAll('.seg-opt').forEach((b) => b.addEventListener('click', () => {
    if (b.dataset.kind === kind) return;
    const body = readBody();
    kind = b.dataset.kind;
    applyKind(body);
  }));

  htmlViewSeg.querySelectorAll('.seg-opt').forEach((b) => b.addEventListener('click', () => {
    if (b.dataset.view === htmlView) return;
    const content = readBody();
    htmlView = b.dataset.view;
    buildBody(format, content);
  }));

  // Trigger validation (required + duplicate detection). Triggers are canonically
  // lowercase with no spaces (they're typed inline; the engine matches contiguous
  // keystrokes) — normalise live so what the user sees is what gets saved.
  const triggerInput = $('#e-trigger');
  const triggerErr = $('#e-trigger-err');
  function checkTrigger() {
    const raw = triggerInput.value;
    const normalized = raw.replace(/\s+/g, '').toLowerCase();
    if (raw !== normalized) {
      const pos = triggerInput.selectionStart;
      triggerInput.value = normalized;
      const drop = raw.length - normalized.length;
      triggerInput.setSelectionRange(Math.max(0, pos - drop), Math.max(0, pos - drop));
    }
    const t = normalized;
    const conflict = t && state.snippets.find((s) => s.id !== existing?.id && s.trigger === t);
    triggerErr.textContent = conflict ? 'Another snippet already uses this trigger.' : '';
    return !conflict;
  }
  triggerInput.addEventListener('input', checkTrigger);

  async function save() {
    checkTrigger(); // normalise even if the user never typed after paste/prefill
    const trigger = triggerInput.value.trim();
    const replacement = readBody();
    const bodyErr = $('#e-body-err');
    const varsErr = $('#e-vars-err');
    const fieldsErr = $('#e-fields-err');
    triggerErr.textContent = ''; bodyErr.textContent = ''; varsErr.textContent = '';
    fieldsErr.textContent = '';
    let ok = true;
    if (!trigger) { triggerErr.textContent = 'Trigger is required.'; ok = false; }
    else if (!checkTrigger()) { ok = false; }
    if (!replacement.trim()) {
      bodyErr.textContent = kind === 'command' ? 'Command is required.' : 'Content is required.';
      ok = false;
    }
    let variables = null;
    if (kind === 'text') {
      const vr = $('#e-vars').value.trim();
      if (vr) {
        try { variables = JSON.parse(vr); }
        catch { varsErr.textContent = 'Variables must be valid JSON.'; $('#e-adv').open = true; ok = false; }
      }
    } else if (kind === 'form') {
      const problems = formFieldProblems(formFields, replacement);
      if (problems.length) {
        fieldsErr.textContent = problems[0];
        if (problems.length > 1) fieldsErr.textContent += ` (+${problems.length - 1} more)`;
        ok = false;
      }
      const cleaned = formFields
        .filter((f) => f.name)
        .map((f) => ({
          name: f.name,
          ...(f.label ? { label: f.label } : {}),
          type: f.type || 'text',
          ...(f.type === 'select' && f.options?.length ? { options: f.options } : {}),
        }));
      if (cleaned.length) variables = { fields: cleaned };
    } else if (kind === 'command') {
      const sh = $('#e-shell').value;
      if (sh && sh !== 'sh') variables = { shell: sh };
    }
    if (!ok) return;
    const payload = {
      trigger, replacement, variables, kind,
      format: kind === 'command' ? 'plain' : format,
      enabled: existing ? existing.enabled !== false : true,
      groupId: gsel.value || null,
      appScope: $('#e-scope').value.trim() || null,
      // Preserve sync scope on edit (team assignment is managed by the Sync section).
      // The store forces command snippets personal regardless.
      team: existing?.team || null,
    };
    try {
      if (isEdit) await invoke('update_snippet', { id: existing.id, patch: payload });
      else await invoke('create_snippet', { snippet: payload });
      close();
      setStatus(isEdit ? 'Snippet saved.' : 'Snippet created.', 'ok');
    } catch (e) { setStatus(String(e), 'err'); }
  }

  $('#e-save').addEventListener('click', save);
  $('#e-cancel').addEventListener('click', close);
  $('#e-close').addEventListener('click', close);
  modal.addEventListener('keydown', (e) => {
    if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') { e.preventDefault(); save(); }
  });
  triggerInput.focus();
  checkTrigger();
}

// What a form field's name may contain — the same characters `form.js` will substitute for.
// Anything else (a space, most often) can never match a {{placeholder}}, so the form would
// collect the value and then quietly drop it.
const FIELD_NAME_RE = /^[\w.-]+$/;

/// Everything wrong with a form snippet's fields, in the order worth fixing.
///
/// A form whose fields are never referenced is not a subtle problem — it collects input and
/// pastes the template untouched — but it fails at expansion time, in a window that has
/// already closed, so nothing points at the cause. These are the checks that would have.
function formFieldProblems(fields, body) {
    const text = String(body || '');
    const problems = [];
    const named = fields.filter((f) => f.name);

    for (const f of named) {
        if (!FIELD_NAME_RE.test(f.name)) {
            problems.push(
                `Field name “${f.name}” can't be used: names take letters, digits, _ . and - ` +
                `(no spaces). Try “${f.name.replace(/[^\w.-]+/g, '')}”.`,
            );
        }
    }
    if (problems.length) return problems; // fix the names before judging the content

    const used = new Set(
        [...text.matchAll(/\{\{\s*([\w.-]+)\s*\}\}/g)].map((m) => m[1]),
    );
    const missing = named.filter((f) => !used.has(f.name));
    for (const f of missing) {
        // The single-brace slip is worth calling out by name: it looks right, and the label
        // is the obvious thing to reach for even though the name is what substitutes.
        const singles = [f.name, f.label].filter(Boolean)
            .filter((s) => text.includes(`{${s}}`));
        problems.push(
            singles.length
                ? `Content has “{${singles[0]}}” — placeholders need double braces. ` +
                  `Write “{{${f.name}}}”.`
                : `Field “${f.name}” isn't used in the content — add “{{${f.name}}}”, ` +
                  `or remove the field.`,
        );
    }
    return problems;
}

// Rich-text toolbar. Returns a `refresh()` that syncs button active-states to the current selection.
function renderRichToolbar(bar, getEditor, { onChange, saveSel, restoreSel }) {
  bar.innerHTML = '';
  const stateBtns = [];
  const exec = (command, value) => { getEditor().focus(); document.execCommand(command, false, value); onChange(); refresh(); };
  const mk = (label, title, onClick, cmdName) => {
    const b = el('button', { type: 'button', className: 'tb-btn', title, innerHTML: label });
    if (cmdName) { b.dataset.cmd = cmdName; stateBtns.push(b); }
    b.addEventListener('mousedown', (e) => e.preventDefault()); // keep the editor selection
    b.addEventListener('click', (e) => { e.preventDefault(); onClick(b); });
    bar.append(b);
    return b;
  };

  mk('<b>B</b>', 'Bold (⌘B)', () => exec('bold'), 'bold');
  mk('<i>I</i>', 'Italic (⌘I)', () => exec('italic'), 'italic');
  mk('<u>U</u>', 'Underline (⌘U)', () => exec('underline'), 'underline');
  mk('H', 'Heading', () => exec('formatBlock', 'H3'));
  mk(icon('list'), 'Bullet list', () => exec('insertUnorderedList'), 'insertUnorderedList');
  mk(icon('listOrdered'), 'Numbered list', () => exec('insertOrderedList'), 'insertOrderedList');
  mk(icon('code'), 'Code block', () => exec('formatBlock', 'PRE'));
  mk(icon('link'), 'Insert link', (b) => openLinkPopover(b));
  mk(icon('table'), 'Insert table', (b) => openTablePopover(b));
  mk(icon('image'), 'Insert image', () => pickImage());
  const sizeBtn = mk(icon('resize'), 'Resize image — select one first', (b) => {
    if (selectedImage()) openImageSizePopover(selectedImage(), onChange, b);
  });
  mk(icon('eraser'), 'Clear formatting', () => exec('removeFormat'));

  // Image sizing belongs here, alongside every other formatting control. It used to be a
  // pill that appeared only while hovering the image, which people didn't find at all.
  // Clicking an image arms this button; the editor's hover outline hints it's selectable.
  let picked = null;
  function selectedImage() {
    // The image may have been deleted since it was picked.
    return picked && getEditor().contains(picked) ? picked : null;
  }
  function armSizeButton(img) {
    picked = img;
    const on = !!selectedImage();
    sizeBtn.disabled = !on;
    sizeBtn.classList.toggle('active', on);
    sizeBtn.title = on ? 'Resize this image' : 'Resize image — select one first';
  }
  armSizeButton(null);
  {
    const ed = getEditor();
    ed.addEventListener('click', (e) => armSizeButton(e.target.closest('img')));
    // Typing moves the point of interest away from the image.
    ed.addEventListener('input', () => armSizeButton(null));
  }

  function pickImage() {
    const input = el('input', { type: 'file', accept: 'image/*' });
    input.addEventListener('change', () => {
      const file = input.files?.[0];
      if (file) insertImageFile(file, getEditor, onChange);
    });
    input.click();
  }

  function openLinkPopover(anchor) {
    saveSel();
    openPopover(anchor, `
      <div class="pop-row"><input type="url" data-url placeholder="https://…" /></div>
      <div class="pop-actions"><button class="secondary sm" data-cancel>Cancel</button><button class="primary sm" data-ok>Insert</button></div>`,
      ({ root, close }) => {
        const url = root.querySelector('[data-url]');
        url.focus();
        const insert = () => {
          const v = url.value.trim();
          if (!v) { url.focus(); return; }
          restoreSel(); getEditor().focus();
          const sel = window.getSelection();
          if (sel && sel.toString()) exec('createLink', v);
          else exec('insertHTML', `<a href="${escapeAttr(v)}">${escapeHtml(v)}</a>`);
          close();
        };
        root.querySelector('[data-ok]').addEventListener('click', insert);
        root.querySelector('[data-cancel]').addEventListener('click', close);
        url.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); insert(); } });
      });
  }

  function openTablePopover(anchor) {
    saveSel();
    openPopover(anchor, `
      <div class="pop-row pop-grid">
        <label>Rows<input type="number" data-rows min="1" max="20" value="2" /></label>
        <label>Cols<input type="number" data-cols min="1" max="20" value="2" /></label>
      </div>
      <div class="pop-actions"><button class="secondary sm" data-cancel>Cancel</button><button class="primary sm" data-ok>Insert</button></div>`,
      ({ root, close }) => {
        const clamp = (n) => Math.max(1, Math.min(20, parseInt(n, 10) || 1));
        const insert = () => {
          const r = clamp(root.querySelector('[data-rows]').value);
          const c = clamp(root.querySelector('[data-cols]').value);
          restoreSel(); getEditor().focus();
          exec('insertHTML', tableHtml(r, c));
          close();
        };
        root.querySelector('[data-ok]').addEventListener('click', insert);
        root.querySelector('[data-cancel]').addEventListener('click', close);
        root.querySelector('[data-rows]').focus();
      });
  }

  function refresh() {
    for (const b of stateBtns) {
      let on = false;
      try { on = document.queryCommandState(b.dataset.cmd); } catch { /* unsupported */ }
      b.classList.toggle('active', on);
    }
  }
  return refresh;
}

// --- Inline images (data URIs) ----------------------------------------------
// Images live inside the HTML body as data URIs — they sync/export with the snippet and
// render in the popup/form windows and on rich paste. Downscaled on insert so a snippet
// stays well under the 1 MB sync cap.

const IMG_MAX_EDGE = 1600;
const IMG_TARGET_BYTES = 300 * 1024;

async function insertImageFile(file, getEditor, onChange) {
  try {
    const dataUrl = await downscaleImageToDataUrl(file);
    const ed = getEditor();
    ed.focus();
    const marker = `img-${Date.now()}`;
    document.execCommand('insertHTML', false, `<img src="${dataUrl}" alt="" data-new="${marker}">`);
    onChange?.();
    // Open the size popover right away — the moment of insertion is when the user is
    // thinking about size, and it teaches that images are click-to-resize.
    const img = ed.querySelector(`img[data-new="${marker}"]`);
    if (img) {
      img.removeAttribute('data-new'); // keep saved HTML clean
      const openPop = () => openImageSizePopover(img, onChange);
      if (img.complete) openPop();
      else img.addEventListener('load', openPop, { once: true });
    }
  } catch (e) {
    setStatus(`Could not insert image: ${e.message || e}`, 'err');
  }
}

/// Resize an inline snippet image: a popover with a width slider + presets, anchored to the
/// toolbar button that opened it (or to the image itself, right after inserting one). Width
/// is stored as an absolute `width` attribute (height auto-follows), which survives
/// sanitization, the popup/form surfaces, and rich paste into target apps.
function openImageSizePopover(img, onChange, anchor = img) {
  const natural = img.naturalWidth || parseInt(img.getAttribute('width'), 10) || 0;
  if (!natural) return;
  const current = parseInt(img.getAttribute('width'), 10) || natural;
  const pct = Math.max(10, Math.min(100, Math.round((current / natural) * 100)));
  openPopover(anchor, `
    <div class="pop-row img-size-row">
      <input type="range" data-size min="10" max="100" step="5" value="${pct}" aria-label="Image width" />
      <span class="img-size-val" data-val>${pct}%</span>
    </div>
    <div class="pop-actions">
      <button class="secondary sm" data-preset="25" type="button">S</button>
      <button class="secondary sm" data-preset="50" type="button">M</button>
      <button class="secondary sm" data-preset="75" type="button">L</button>
      <button class="secondary sm" data-preset="100" type="button">Full</button>
    </div>`,
    ({ root }) => {
      const slider = root.querySelector('[data-size]');
      const val = root.querySelector('[data-val]');
      const apply = (p) => {
        slider.value = String(p);
        val.textContent = `${p}%`;
        if (p >= 100) { img.removeAttribute('width'); }
        else { img.setAttribute('width', Math.max(1, Math.round((natural * p) / 100))); }
        img.removeAttribute('height'); // keep aspect ratio
        onChange?.();
      };
      slider.addEventListener('input', () => apply(parseInt(slider.value, 10)));
      root.querySelectorAll('[data-preset]').forEach((b) =>
        b.addEventListener('click', () => apply(parseInt(b.dataset.preset, 10))));
    });
}

async function downscaleImageToDataUrl(blob) {
  const bmp = await createImageBitmap(blob);
  const scale = Math.min(1, IMG_MAX_EDGE / Math.max(bmp.width, bmp.height));
  const w = Math.max(1, Math.round(bmp.width * scale));
  const h = Math.max(1, Math.round(bmp.height * scale));
  const canvas = el('canvas', { width: w, height: h });
  canvas.getContext('2d').drawImage(bmp, 0, 0, w, h);
  bmp.close?.();
  // PNG keeps screenshots/text crisp (and transparency); step down through JPEG
  // qualities only when the PNG would blow past the target size.
  let out = canvas.toDataURL('image/png');
  const budget = IMG_TARGET_BYTES * (4 / 3); // data-URI base64 overhead
  if (out.length > budget) {
    for (const q of [0.85, 0.7, 0.55]) {
      out = canvas.toDataURL('image/jpeg', q);
      if (out.length <= budget) break;
    }
  }
  return out;
}

function tableHtml(rows, cols) {
  const cell = 'style="border:1px solid #888;padding:4px 8px"';
  let out = '<table style="border-collapse:collapse"><tbody>';
  for (let r = 0; r < rows; r++) {
    out += '<tr>';
    for (let c = 0; c < cols; c++) out += `<td ${cell}>&nbsp;</td>`;
    out += '</tr>';
  }
  return out + '</tbody></table><p></p>';
}

// --- Live preview + format conversion --------------------------------------

function renderPreview(container, fmt, content) {
  if (!content || !content.trim()) {
    container.className = 'preview-body';
    container.innerHTML = '<span class="pv-empty">Nothing to preview yet.</span>';
    return;
  }
  if (fmt === 'plain') {
    container.className = 'preview-body pv-plain';
    container.textContent = content;
  } else {
    container.className = 'preview-body pv-rich';
    // Sanitized: html bodies can arrive from teammates via sync, and this webview holds IPC.
    container.innerHTML = fmt === 'markdown' ? mdToHtml(content) : sanitizeSnippetHtml(content);
  }
}

// Convert content when the user switches format, so switching never leaves raw markup behind.
function convertContent(content, from, to) {
  if (from === to || !content) return content;
  if (from === 'plain' && to === 'html') return textToHtml(content);
  if (from === 'plain' && to === 'markdown') return content;
  if (from === 'markdown' && to === 'plain') return mdToText(content);
  if (from === 'markdown' && to === 'html') return mdToHtml(content);
  if (from === 'html' && to === 'plain') return htmlToText(content);
  if (from === 'html' && to === 'markdown') return htmlToMarkdown(content);
  return content;
}

function textToHtml(t) { return escapeHtml(t).replace(/\n/g, '<br>'); }

function htmlToText(html) {
  const tmp = el('div');
  tmp.innerHTML = html
    .replace(/<\s*br\s*\/?>/gi, '\n')
    .replace(/<li[^>]*>/gi, '• ')
    .replace(/<\/(p|div|h[1-6]|li|tr)>/gi, '\n');
  return (tmp.textContent || '').replace(/\n{3,}/g, '\n\n').trim();
}

// Markdown rendering, HTML escaping, and snippet-HTML sanitization live in
// ../shared/markdown.js (shared with the popup/form surfaces).

function mdToText(md) {
  return md
    .replace(/^#{1,6}\s+/gm, '')
    .replace(/\*\*([^*]+)\*\*/g, '$1')
    .replace(/\*([^*]+)\*/g, '$1')
    .replace(/`([^`]+)`/g, '$1')
    .replace(/\[([^\]]+)\]\([^)]+\)/g, '$1')
    .replace(/^\s*[-*]\s+/gm, '• ')
    .trim();
}

function htmlToMarkdown(html) {
  const s = html
    .replace(/<\s*br\s*\/?>/gi, '\n')
    .replace(/<(strong|b)>([\s\S]*?)<\/\1>/gi, '**$2**')
    .replace(/<(em|i)>([\s\S]*?)<\/\1>/gi, '*$2*')
    .replace(/<code>([\s\S]*?)<\/code>/gi, '`$1`')
    .replace(/<a[^>]*href="([^"]*)"[^>]*>([\s\S]*?)<\/a>/gi, '[$2]($1)')
    .replace(/<h([1-6])>([\s\S]*?)<\/h\1>/gi, (m, l, t) => `\n${'#'.repeat(+l)} ${t}\n`)
    .replace(/<li[^>]*>([\s\S]*?)<\/li>/gi, '- $1\n')
    .replace(/<\/(p|div|ul|ol)>/gi, '\n');
  const tmp = el('div');
  tmp.innerHTML = s;
  return (tmp.textContent || '').replace(/\n{3,}/g, '\n\n').trim();
}

// --- Reusable modal + popover primitives ------------------------------------

function openModal(innerHtml, { className = '', onClose } = {}) {
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

function confirmDialog(message, { confirmLabel = 'Confirm', danger = false } = {}) {
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

function promptDialog(title, { label = '', value = '', placeholder = '', confirmLabel = 'Save', allowEmpty = false } = {}) {
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
function openPopover(anchor, innerHtml, onMount) {
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
  const showForm = mode === 'active';
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
    body = `${cardHtml}${formHtml}`;
  } else {
    // off / personal — provisioning is invite-link only (an admin-managed config is the other
    // path; both keep an arbitrary backend URL out of the user's hands and route through the
    // invite confirmation dialog).
    body = `
      <div class="sync-off">
        <p class="adv-hint">Team sync is off — everything stays on this device. Join a team with an
        invite link from your admin.</p>
        <div class="sync-actions">
          <button class="primary" id="sync-join">Join with an invite link…</button>
        </div>
      </div>`;
  }

  div.innerHTML = `
    <h3>Team sync</h3>
    <p class="adv-hint">Syncs <strong>team-shared</strong> snippet groups through your configured
    backend. Personal snippets and capture history never leave this device.</p>
    ${body}`;

  const toggleOidc = () => div.querySelectorAll('.sc-oidc').forEach((el) => {
    el.style.display = div.querySelector('#sc-mode').value === 'oidc' ? '' : 'none';
  });
  if (showForm) {
    toggleOidc();
    div.querySelector('#sc-mode').addEventListener('change', toggleOidc);
  }
  div.querySelector('#sync-join')?.addEventListener('click', async () => {
    const url = await promptDialog('Join a team', {
      label: 'Paste the invite link or code from your admin',
      placeholder: 'glyphio://join?server=…', confirmLabel: 'Continue',
    });
    if (url) confirmInvite(url);
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
        ? `<p class="adv-hint">You're signed in but not in any team yet. Paste an invite from your
           admin to join one — you can be in as many teams as you're invited to.</p>`
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
  ['capture', 'Capture'],
  ['snippets', 'Snippets'],
  ['clipboard', 'Clipboard'],
  ['sync', 'Sync'],
  ['permissions', 'Permissions'],
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
    case 'snippets': renderSnippetsTab(form); break;
    case 'clipboard': renderClipboardTab(form); break;
    case 'sync': renderSyncSection(form); break;
    case 'permissions': renderPermissionsTab(form); break;
    case 'about': renderAboutTab(form); break;
    default: renderSections(form, CAPTURE_SECTIONS);
  }
}

/// Render setting sections plus the Save button they share. Every input carries its key, so
/// `saveSettings` collects whatever is on screen.
function renderSections(form, sections) {
  for (const sec of sections) {
    const div = document.createElement('div');
    div.className = 'form-section';
    div.innerHTML = `<h3>${sec.title}</h3>${sec.hint ? `<p class="adv-hint">${sec.hint}</p>` : ''}`;
    for (const [key, type, label, opts] of sec.fields) div.append(renderField(key, type, label, opts));
    form.append(div);
  }
  const save = document.createElement('button');
  save.className = 'primary'; save.textContent = 'Save settings';
  save.addEventListener('click', saveSettings);
  form.append(save);
}

function renderSnippetsTab(form) {
  const div = document.createElement('div');
  div.className = 'form-section';
  div.innerHTML = `
    <h3>Portability</h3>
    <p class="adv-hint">Exports are portable Glyphio JSON — content only, no team or sync state.
    Imports also accept <code>matches:</code>-style YAML from other expanders. You pick the group they land in;
    snippets you already have are skipped, and a trigger that arrives with different content is shown side by side
    so you can replace it or keep yours.${state.syncStatus?.identity?.policy?.exportTeamGroups && state.syncStatus.identity.policy.exportTeamGroups !== 'open'
      ? ' <strong>Note:</strong> your organization restricts exporting team-shared groups.' : ''}</p>
    <div class="sync-actions">
      <button class="secondary" id="tab-export">Export all snippets…</button>
      <button class="secondary" id="tab-import">Import…</button>
    </div>`;
  div.querySelector('#tab-export').addEventListener('click', () => exportSnippets(null));
  div.querySelector('#tab-import').addEventListener('click', () => importSnippets(null));
  form.append(div);
  renderSections(form, SNIPPET_SECTIONS);
}

function renderClipboardTab(form) {
  renderSections(form, CLIPBOARD_SECTIONS);
  const div = document.createElement('div');
  div.className = 'form-section';
  div.innerHTML = `
    <h3>Forget everything</h3>
    <p class="adv-hint">Deletes every stored entry and every copied image, pinned ones
    included. There is no undo, and nothing to recover from — that's the point.</p>
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
  form.append(div);
}

function renderPermissionsTab(form) {
  const div = document.createElement('div');
  div.className = 'form-section';
  div.innerHTML = `
    <h3>macOS permissions</h3>
    <div class="perm-row" id="perm-ax">
      <div class="perm-info"><strong>Accessibility</strong>
        <span class="perm-sub">One grant to “Glyphio” covers text expansion (the engine runs inside the app) and scrolling capture.</span></div>
      <span class="perm-state" data-ok="">checking…</span>
      <button class="ghost" data-act="ax-grant">Grant access…</button>
      <button class="ghost" data-act="ax-settings">System Settings</button>
    </div>
    <div class="perm-row" id="perm-sr">
      <div class="perm-info"><strong>Screen Recording</strong>
        <span class="perm-sub">Required for captures. Granted on first capture; applies after relaunch.</span></div>
      <span class="perm-state" data-ok="">checking…</span>
      <button class="ghost" data-act="sr-settings">System Settings</button>
      <button class="ghost" data-act="relaunch">Relaunch</button>
    </div>`;
  const setState = (rowId, ok) => {
    const el = div.querySelector(`#${rowId} .perm-state`);
    el.textContent = ok ? 'granted' : 'not granted';
    el.dataset.ok = ok ? 'yes' : 'no';
  };
  invoke('app_accessibility_status').then((ok) => setState('perm-ax', ok));
  invoke('screen_recording_status').then((ok) => setState('perm-sr', ok));
  div.querySelector('[data-act="ax-settings"]').addEventListener('click', () => invoke('open_accessibility_settings'));
  div.querySelector('[data-act="ax-grant"]').addEventListener('click', () => invoke('request_accessibility'));
  div.querySelector('[data-act="sr-settings"]').addEventListener('click', () => invoke('open_screen_recording_settings'));
  div.querySelector('[data-act="relaunch"]').addEventListener('click', () => invoke('relaunch_app'));
  form.append(div);
}

async function renderAboutTab(form) {
  const div = document.createElement('div');
  div.className = 'form-section';
  let version = '';
  try { version = await window.__TAURI__.app.getVersion(); } catch { /* capability absent */ }
  div.innerHTML = `
    <h3>About Glyphio</h3>
    <p class="adv-hint">Local-first text expansion and screenshot capture with self-hostable,
    role-based team sync. ${version ? `Version <code>${escapeHtml(version)}</code>.` : ''}</p>
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
    field.append(hotkeyPair(key, opts));
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
  return field;
}

/** The editor key and the straight-to-clipboard key for one capture mode. */
function hotkeyPair(key, silentKey) {
  const wrap = el('div', { className: 'hotkey-pair' });
  const one = (k, caption, placeholder) => {
    const box = el('div');
    const input = el('input', { type: 'text', placeholder });
    input.value = state.settings[k] ?? '';
    input.dataset.key = k;
    input.dataset.type = 'text';
    box.append(input, el('span', { className: 'hotkey-caption', textContent: caption }));
    return box;
  };
  wrap.append(one(key, 'opens the editor', ''), one(silentKey, 'to the clipboard', 'not set'));
  return wrap;
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
  try { await invoke('save_settings', { settings: next }); state.settings = next; setStatus('Settings saved.', 'ok'); }
  catch (e) { setStatus(String(e), 'err'); }
}

// --- utils ------------------------------------------------------------------

function el(tag, props = {}) { return Object.assign(document.createElement(tag), props); }

function setStatus(text, kind = '') {
  const el = document.getElementById('status');
  if (!el) return;
  el.textContent = text; el.className = `status-line ${kind}`;
  if (kind === 'ok') setTimeout(() => { if (el.textContent === text) el.textContent = ''; }, 3000);
}
