// The setup window's step logic, free of the DOM so it can be tested.

import { GRANT, PERMISSION_KIND } from '../settings/permissions.mjs';
import { formatAccelerator } from '../settings/shortcut-recorder.mjs';
import { registrationNotice, statusFor } from '../settings/shortcut-status.mjs';

export const STEP = Object.freeze({
  ACCESSIBILITY: 'accessibility',
  SCREEN_RECORDING: 'screenRecording',
  SHORTCUTS: 'shortcuts',
  FINISH: 'finish',
});

// The platform table: which steps exist and what they say. macOS needs two permissions before
// capture and expansion work; Windows has no equivalent, so it starts at shortcuts.
const PLATFORM_STEPS = Object.freeze({
  macos: Object.freeze([STEP.ACCESSIBILITY, STEP.SCREEN_RECORDING, STEP.SHORTCUTS, STEP.FINISH]),
  windows: Object.freeze([STEP.SHORTCUTS, STEP.FINISH]),
});

const LOGIN_ITEM = Object.freeze({
  macos: Object.freeze({
    defaultOn: true,
    explanation:
      'Glyphio starts when you log in so expansion and shortcuts are always ready. It is added to Login Items in System Settings, where you can remove it at any time.',
  }),
  windows: Object.freeze({
    defaultOn: true,
    explanation:
      'Glyphio starts when you sign in to Windows so expansion and shortcuts are always ready. You can turn this off in Settings.',
  }),
});

/** The steps this platform shows, in order. Anything unrecognised gets the permission-free list. */
export function stepsFor(platform) {
  return PLATFORM_STEPS[platform] ?? PLATFORM_STEPS.windows;
}

export function loginItemCopy(platform) {
  return LOGIN_ITEM[platform] ?? LOGIN_ITEM.windows;
}

export const STEPS = PLATFORM_STEPS.macos;

const KIND_OF_STEP = Object.freeze({
  [STEP.ACCESSIBILITY]: PERMISSION_KIND.ACCESSIBILITY,
  [STEP.SCREEN_RECORDING]: PERMISSION_KIND.SCREEN_RECORDING,
});

/** The permission a step is about, or null for the finish step. */
export function kindOfStep(step) {
  return KIND_OF_STEP[step] ?? null;
}

/** The persisted step if it is one we know, otherwise the first. */
export function initialStep(persisted, steps = STEPS) {
  return steps.includes(persisted) ? persisted : steps[0];
}

export function nextStep(step, steps = STEPS) {
  const i = steps.indexOf(step);
  return steps[Math.min(i + 1, steps.length - 1)];
}

export function progressLabel(step, steps = STEPS) {
  return `${steps.indexOf(step) + 1} of ${steps.length}`;
}

/** `grants` is `{ accessibility, screenRecording }` in the native grant vocabulary. */
function grantOf(step, grants) {
  const kind = kindOfStep(step);
  return kind ? grants[kind] : null;
}

/** A permission step can continue once granted; skipping is a separate, always-available action. */
export function canContinue(step, grants) {
  return kindOfStep(step) === null || grantOf(step, grants) === GRANT.GRANTED;
}

/** Skipping records a permission step once; shortcuts and finish have nothing to come back to. */
export function skipStep(step, skipped) {
  if (kindOfStep(step) === null || skipped.includes(step)) return skipped;
  return [...skipped, step];
}

/** A skipped step that has since been granted no longer needs resolving. */
export function unresolvedSkips(skipped, grants) {
  return skipped.filter((step) => grantOf(step, grants) !== GRANT.GRANTED);
}

/**
 * After a relaunch the window reopens on the persisted step. If that step's permission is now
 * granted it moves on by itself, so the user is never asked to do anything twice.
 */
export function advanceIfGranted(step, grants, steps = STEPS) {
  return grantOf(step, grants) === GRANT.GRANTED ? nextStep(step, steps) : step;
}

const NAMES = Object.freeze({
  [PERMISSION_KIND.ACCESSIBILITY]: 'Accessibility',
  [PERMISSION_KIND.SCREEN_RECORDING]: 'Screen Recording',
});

/** Text for the live region when a grant changes, or null when nothing worth saying happened. */
export function announcement(kind, before, after) {
  if (before === after) return null;
  const name = NAMES[kind];
  switch (after) {
    case GRANT.GRANTED:
      return `${name} is on.`;
    case GRANT.STALE:
      return `${name} is blocked by macOS. Choose Fix it.`;
    case GRANT.NEEDS_RELAUNCH:
      return `${name} needs one relaunch to finish.`;
    default:
      return null;
  }
}

const SHORTCUT_ROWS = Object.freeze([
  { key: 'shortcutCaptureSnip', label: 'Capture' },
  { key: 'shortcutOpenPalette', label: 'Snippet palette' },
  { key: 'shortcutOpenClipboard', label: 'Clipboard history' },
]);

/** The chord as the user will press it: symbols on macOS, as written elsewhere. */
export function displayChord(accelerator, platform) {
  return platform === 'macos' ? formatAccelerator(accelerator) : accelerator;
}

/**
 * One row per default shortcut with what the system said. `statuses` is the registration list
 * from the app; a binding with no answer yet is neither shown as working nor as failed.
 */
export function shortcutRows(settings, statuses, platform) {
  return SHORTCUT_ROWS.map(({ key, label }) => {
    const status = statusFor(statuses, key);
    return {
      key,
      label,
      keys: displayChord(settings[key], platform),
      ok: status?.state === 'registered',
      notice: registrationNotice(status),
    };
  });
}

export const TRY_IT = Object.freeze({ WAITING: 'waiting', DONE: 'done' });

/**
 * The try-it capture finishes only when the app reports a capture delivered, which proves the
 * whole path (shortcut, permission, capture, result). A shortcut press or a failure does not.
 */
export function reduceTryIt(state, event) {
  return state === TRY_IT.DONE || event === 'capture-delivered' ? TRY_IT.DONE : TRY_IT.WAITING;
}

/** What choosing a team option does: stay in setup, ask for an invite, or hand over to Settings. */
export function teamAction(choice, invite = '') {
  const url = invite.trim();
  if (choice === 'personal') return { kind: 'stay' };
  if (choice === 'setup') return { kind: 'open-team', invite: null };
  return url ? { kind: 'open-team', invite: url } : { kind: 'ask-invite' };
}
