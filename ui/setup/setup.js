// First-run setup window (ONB-02): Accessibility and Screen Recording, then finish.
// Step logic is in steps.mjs and row presentation in ../settings/permissions.mjs; this file
// only draws them and talks to the native permission model.

import { PERMISSION_KIND, grantPresentation } from '../settings/permissions.mjs';
import {
  STEP, STEPS, advanceIfGranted, announcement, canContinue, initialStep, kindOfStep,
  nextStep, progressLabel, skipStep,
} from './steps.mjs';

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
  [STEP.FINISH]: 'Finish',
};

const state = {
  step: STEP.ACCESSIBILITY,
  skipped: [],
  grants: { [PERMISSION_KIND.ACCESSIBILITY]: null, [PERMISSION_KIND.SCREEN_RECORDING]: null },
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

function render() {
  $('progress').textContent = progressLabel(state.step);
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
  } else {
    const left = state.skipped.length;
    panel.innerHTML = `
      <h2 id="panel-title">You're set</h2>
      <p>${left ? 'Anything you skipped stays available in Settings.' : 'Glyphio is ready.'}</p>
      <div class="row">
        <button type="button" class="primary" data-action="open">Open Glyphio</button>
        <button type="button" class="ghost" data-action="menubar">Keep in menu bar</button>
      </div>`;
  }
  $('skip').hidden = state.step === STEP.FINISH;
  $('continue').hidden = state.step === STEP.FINISH;
  $('continue').disabled = !canContinue(state.step, state.grants);
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
      await save({ completed: true });
      if (action === 'open') await invoke('open_window', { name: 'settings' });
      await invoke('permission_watch', { active: false });
      await window.__TAURI__.window.getCurrentWindow().close();
    }
  } catch (e) {
    $('live').textContent = `Something went wrong: ${e}`;
  }
}

$('panel').addEventListener('click', (e) => {
  const button = e.target.closest('[data-action]');
  if (button) perform(button.dataset.action);
});
$('continue').addEventListener('click', () => setStep(nextStep(state.step)));
$('skip').addEventListener('click', () => {
  state.skipped = skipStep(state.step, state.skipped);
  save({ skipped: state.skipped });
  setStep(nextStep(state.step));
});

async function init() {
  // One-time import of the old web-storage "already prompted" flags.
  if (!localStorage.getItem(LEGACY_FLAGS_IMPORTED)) {
    await invoke('permission_import_legacy_flags', {
      accessibility: Boolean(localStorage.getItem(LEGACY_PROMPTED[PERMISSION_KIND.ACCESSIBILITY])),
      screenRecording: Boolean(localStorage.getItem(LEGACY_PROMPTED[PERMISSION_KIND.SCREEN_RECORDING])),
    });
    localStorage.setItem(LEGACY_FLAGS_IMPORTED, '1');
  }
  const persisted = await invoke('onboarding_state');
  state.skipped = persisted.skipped;
  const status = await invoke('permission_status');
  for (const kind of Object.values(PERMISSION_KIND)) state.grants[kind] = status[kind];
  const resumed = initialStep(persisted.step);
  state.step = advanceIfGranted(resumed, state.grants);
  if (state.step !== resumed) save({ step: state.step });
  await listen('permission-status', (e) => applyStatus(e.payload));
  // Poll only while this window is the one being looked at.
  await invoke('permission_watch', { active: true });
  window.addEventListener('beforeunload', () => invoke('permission_watch', { active: false }));
  render();
  $('panel').querySelector('button')?.focus();
}

init();
