// First-run setup window (ONB-02, ONB-03): permissions, shortcuts with a try-it capture and the
// Launch at Login choice, then the team choice.
// Step logic is in steps.mjs and row presentation in ../settings/permissions.mjs; this file
// only draws them and talks to the native permission model.

import { PERMISSION_KIND, grantPresentation } from '../settings/permissions.mjs';
import { IS_MAC } from '../shared/shortcuts.js';
import { acceleratorFromKeyboardEvent, setKeyboardLayout, validateAccelerator } from '../settings/shortcut-recorder.mjs';
import {
  STEP, TRY_IT, advanceIfGranted, announcement, canContinue, initialStep, kindOfStep, loginItemCopy,
  installNotice, nextStep, progressLabel, reduceTryIt, shortcutRows, skipStep, stepsFor, teamAction,
} from './steps.mjs';

const platform = IS_MAC ? 'macos' : 'windows';
const STEPS = stepsFor(platform);
const loginCopy = loginItemCopy(platform);

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const LEGACY_FLAGS_IMPORTED = 'glyphio-permission-flags-imported';
const LEGACY_PROMPTED = {
  [PERMISSION_KIND.ACCESSIBILITY]: 'glyphio-accessibility-prompted',
  [PERMISSION_KIND.SCREEN_RECORDING]: 'glyphio-screen-recording-prompted',
};

const TITLES = {
  [STEP.ACCESSIBILITY]: 'Accessibility',
  [STEP.SCREEN_RECORDING]: 'Screen Recording',
  [STEP.SHORTCUTS]: 'Shortcuts',
  [STEP.FINISH]: 'Finish',
};

const state = {
  step: STEPS[0],
  settings: {},
  shortcutStatus: [],
  tryIt: TRY_IT.WAITING,
  loginItem: loginCopy.defaultOn,
  team: null, // the option chosen on the finish step
  inviteOpen: false,
  recording: null, // the shortcut key being recorded, if any
  skipped: [],
  grants: { [PERMISSION_KIND.ACCESSIBILITY]: null, [PERMISSION_KIND.SCREEN_RECORDING]: null },
  install: null, // native install status: disk image, translocation, other copies
};

const $ = (id) => document.getElementById(id);

function save(patch) {
  return invoke('onboarding_update', patch).catch((e) => console.error('onboarding_update', e));
}

function setStep(step) {
  state.step = step;
  save({ step });
  render();
}

function applyStatus(status) {
  for (const kind of Object.values(PERMISSION_KIND)) {
    const before = state.grants[kind];
    const after = status[kind];
    state.grants[kind] = after;
    const say = before === null ? null : announcement(kind, before, after);
    if (say) $('live').textContent = say;
  }
  render();
}

function renderInstallNotice() {
  const box = $('install');
  const notice = installNotice(state.install);
  box.hidden = !notice;
  if (!notice) return;
  box.innerHTML = `
    <h2>${notice.title}</h2>
    <p>${escapeText(notice.body)}</p>
    <div class="row">${notice.actions.map((a) =>
      `<button type="button" class="${a.style}" data-install="${a.id}">${a.label}</button>`).join('')}</div>`;
}

function escapeText(s) {
  return String(s).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c]);
}

function render() {
  renderInstallNotice();
  $('progress').textContent = progressLabel(state.step, STEPS);
  $('steps').innerHTML = STEPS.map((step) => {
    const kind = kindOfStep(step);
    const done = kind ? state.grants[kind] === 'granted' : false;
    const skipped = state.skipped.includes(step);
    const mark = done ? '✓' : skipped ? '–' : step === state.step ? '●' : '○';
    const label = `${TITLES[step]}${done ? ', done' : skipped ? ', skipped' : ''}`;
    return `<li ${step === state.step ? 'aria-current="step"' : ''}><span class="mark" aria-hidden="true">${mark}</span><span>${label}</span></li>`;
  }).join('');

  const panel = $('panel');
  const kind = kindOfStep(state.step);
  if (kind) {
    const row = grantPresentation(kind, state.grants[kind] ?? 'checking');
    panel.innerHTML = `
      <h2 id="panel-title">${row.name}</h2>
      <p>${row.description}</p>
      ${row.explanation ? `<p>${row.explanation}</p>` : ''}
      <div class="row">
        ${row.actions.map((a) => `<button type="button" class="${a.style}" data-action="${a.id}">${a.label}</button>`).join('')}
        <span class="state" data-phase="${row.phase}">${row.status}</span>
      </div>`;
  } else if (state.step === STEP.SHORTCUTS) {
    const rows = shortcutRows(state.settings, state.shortcutStatus, platform);
    const snip = rows[0];
    panel.innerHTML = `
      <h2 id="panel-title">Shortcuts</h2>
      <p>These work from any app. You can change them in Settings.</p>
      <ul class="shortcuts">${rows.map((r) => `
        <li><span>${r.label}</span><kbd>${r.keys}</kbd>
        <span class="state" data-phase="${r.ok ? 'granted' : r.notice ? 'stale' : 'pending'}">${r.ok ? 'Ready' : r.notice ? 'Not registered' : 'Checking…'}</span>
        ${r.notice ? `<span class="note">${r.notice}</span>
        <span class="alts">${r.alternatives.map((a) => `<button type="button" class="secondary" data-use="${r.key}|${a.accelerator}">Use ${a.keys}</button>`).join('')}
        <button type="button" class="ghost" data-record="${r.key}">${state.recording === r.key ? 'Press the new shortcut… (Esc cancels)' : 'Record another'}</button></span>` : ''}</li>`).join('')}
      </ul>
      <p class="tryit" data-phase="${state.tryIt}">${state.tryIt === TRY_IT.DONE
        ? '✓ That worked — your capture was delivered.'
        : `Try it: press <kbd>${snip.keys}</kbd> and drag over something.`}</p>
      <label class="switch"><input type="checkbox" id="login" ${state.loginItem ? 'checked' : ''}>
        <span>Launch Glyphio at login</span></label>
      <p class="note">${loginCopy.explanation}</p>`;
  } else {
    const left = state.skipped.length;
    const cards = [
      ['personal', 'Just me', 'Everything stays on this device. You can join a team any time later.'],
      ['invite', 'I have an invite', 'Paste the invite link or code from your team admin.'],
      ['setup', 'Set up team sync', "Connect to your organization's server or single sign-on."],
    ];
    panel.innerHTML = `
      <h2 id="panel-title">${left ? 'Almost done' : "You're set"}</h2>
      <p>${left ? 'Anything you skipped stays available in Settings. ' : ''}How will you use Glyphio?</p>
      <div class="cards" role="group" aria-label="How will you use Glyphio?">${cards.map(([id, t, d]) => `
        <button type="button" class="card" data-team="${id}" aria-pressed="${state.team === id}">
          <strong>${t}</strong><span>${d}</span></button>`).join('')}</div>
      ${state.inviteOpen ? `<div class="row invite"><input type="text" id="invite" placeholder="glyphio://join?server=…" spellcheck="false" autocomplete="off" aria-label="Invite link">
        <button type="button" class="primary" data-action="invite">Continue</button></div>` : ''}
      <div class="row finish">
        <button type="button" class="primary" data-action="open">Open Glyphio</button>
        <button type="button" class="ghost" data-action="menubar">Keep in menu bar</button>
      </div>`;
  }
  const isPermission = kind !== null;
  $('skip').hidden = !isPermission;
  $('continue').hidden = state.step === STEP.FINISH;
  $('continue').disabled = !canContinue(state.step, state.grants);
}

/** Mark setup done, run `then`, stop the permission watcher and close the window. */
async function finish(then) {
  await save({ completed: true });
  // Settings must not show its own welcome on top of this one.
  try { localStorage.setItem('glyphio-welcomed', '1'); } catch { /* storage unavailable */ }
  await then();
  if (platform === 'macos') await invoke('permission_watch', { active: false });
  await window.__TAURI__.window.getCurrentWindow().close();
}

async function perform(action) {
  const kind = kindOfStep(state.step);
  try {
    if (action === 'request') applyStatus(await invoke('permission_request', { kind }));
    else if (action === 'settings') await invoke('permission_open_settings', { kind });
    else if (action === 'repair') applyStatus(await invoke('permission_repair', { kind }));
    else if (action === 'relaunch') {
      await save({ step: state.step }); // reopen on this step; it advances itself once granted
      await invoke('relaunch_app');
    } else if (action === 'open' || action === 'menubar') {
      await finish(() => (action === 'open' ? invoke('open_window', { name: 'settings' }) : null));
    } else if (action === 'invite') {
      const act = teamAction('invite', $('invite')?.value ?? '');
      if (act.kind === 'open-team') await finish(() => invoke('onboarding_open_team', { invite: act.invite }));
    }
  } catch (e) {
    $('live').textContent = `Something went wrong: ${e}`;
  }
}

$('install').addEventListener('click', async (e) => {
  const button = e.target.closest('[data-install]');
  if (!button) return;
  button.disabled = true;
  try {
    if (button.dataset.install === 'install-move') {
      $('live').textContent = 'Moving Glyphio…';
      await invoke('install_move'); // quits this copy and opens the moved one
    } else {
      await invoke('install_dismiss_duplicates', { paths: state.install.duplicates });
      state.install = { ...state.install, duplicates: [] };
      render();
    }
  } catch (err) {
    $('live').textContent = `Could not move Glyphio: ${err}`;
    button.disabled = false;
  }
});

$('panel').addEventListener('click', (e) => {
  const use = e.target.closest('[data-use]');
  if (use) return applyShortcut(...use.dataset.use.split('|'));
  const record = e.target.closest('[data-record]');
  if (record) {
    state.recording = state.recording === record.dataset.record ? null : record.dataset.record;
    return render();
  }
  const team = e.target.closest('[data-team]');
  if (team) return chooseTeam(team.dataset.team);
  const button = e.target.closest('[data-action]');
  if (button) perform(button.dataset.action);
});
$('panel').addEventListener('change', (e) => {
  if (e.target.id === 'login') state.loginItem = e.target.checked;
});

/** Save one shortcut change and show how the system answered. */
async function applyShortcut(key, accelerator) {
  state.recording = null;
  const next = { ...state.settings, [key]: accelerator };
  try {
    state.shortcutStatus = await invoke('save_settings', { settings: next });
    state.settings = next;
    $('live').textContent = 'Shortcut changed.';
  } catch (e) {
    $('live').textContent = String(e);
  }
  render();
}

// Recording listens on the document: WebKit does not reliably focus a button after a click.
document.addEventListener('keydown', (e) => {
  if (!state.recording) return;
  e.preventDefault();
  if (e.key === 'Escape') { state.recording = null; return render(); }
  const recorded = acceleratorFromKeyboardEvent(e);
  if (recorded.error) { if (!['Alt', 'Control', 'Meta', 'Shift'].includes(e.key)) $('live').textContent = recorded.error; return; }
  const key = state.recording;
  const others = ['shortcutCaptureSnip', 'shortcutOpenPalette', 'shortcutOpenClipboard']
    .filter((k) => k !== key).map((k) => ({ value: state.settings[k], label: k }));
  const check = validateAccelerator(recorded.accelerator, others, platform);
  if (!check.ok) { $('live').textContent = check.error; return; }
  applyShortcut(key, recorded.accelerator);
});

async function chooseTeam(choice) {
  state.team = choice;
  const act = teamAction(choice);
  state.inviteOpen = act.kind === 'ask-invite';
  if (act.kind === 'open-team') {
    try { await finish(() => invoke('onboarding_open_team', { invite: act.invite })); }
    catch (e) { $('live').textContent = `Something went wrong: ${e}`; }
    return;
  }
  render();
  if (state.inviteOpen) $('invite')?.focus();
}

$('continue').addEventListener('click', async () => {
  if (state.step === STEP.SHORTCUTS) {
    // The login item is registered here, once the user has seen what it does and chosen.
    try {
      const note = await invoke('onboarding_set_login_item', { want: state.loginItem });
      if (note) $('live').textContent = note;
    } catch (e) {
      $('live').textContent = String(e);
    }
  }
  setStep(nextStep(state.step, STEPS));
});
$('skip').addEventListener('click', () => {
  state.skipped = skipStep(state.step, state.skipped);
  save({ skipped: state.skipped });
  setStep(nextStep(state.step, STEPS));
});

async function init() {
  // Name shortcut keys as the active keyboard layout does (AZERTY's A is the US Q position).
  setKeyboardLayout(await invoke('keyboard_layout').catch(() => null));
  const persisted = await invoke('onboarding_state');
  state.skipped = persisted.skipped;
  if (platform === 'macos') {
    // One-time import of the old web-storage "already prompted" flags.
    if (!localStorage.getItem(LEGACY_FLAGS_IMPORTED)) {
      await invoke('permission_import_legacy_flags', {
        accessibility: Boolean(localStorage.getItem(LEGACY_PROMPTED[PERMISSION_KIND.ACCESSIBILITY])),
        screenRecording: Boolean(localStorage.getItem(LEGACY_PROMPTED[PERMISSION_KIND.SCREEN_RECORDING])),
      });
      localStorage.setItem(LEGACY_FLAGS_IMPORTED, '1');
    }
    const status = await invoke('permission_status');
    for (const kind of Object.values(PERMISSION_KIND)) state.grants[kind] = status[kind];
    await listen('permission-status', (e) => applyStatus(e.payload));
    // Poll only while this window is the one being looked at.
    await invoke('permission_watch', { active: true });
    window.addEventListener('beforeunload', () => invoke('permission_watch', { active: false }));
  }
  state.settings = await invoke('get_settings');
  state.shortcutStatus = await invoke('shortcut_status');
  if (platform === 'macos') state.install = await invoke('install_status').catch(() => null);
  await listen('capture-delivered', () => {
    const before = state.tryIt;
    state.tryIt = reduceTryIt(state.tryIt, 'capture-delivered');
    if (before !== state.tryIt) $('live').textContent = 'Your capture was delivered.';
    if (state.step === STEP.SHORTCUTS) render();
  });
  const resumed = initialStep(persisted.step, STEPS);
  state.step = advanceIfGranted(resumed, state.grants, STEPS);
  if (state.step !== resumed) save({ step: state.step });
  render();
  $('panel').querySelector('button, input')?.focus();
}

init();
