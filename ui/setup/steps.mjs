// The setup window's step logic, free of the DOM so it can be tested.

import { GRANT, PERMISSION_KIND } from '../settings/permissions.mjs';

export const STEP = Object.freeze({
  ACCESSIBILITY: 'accessibility',
  SCREEN_RECORDING: 'screenRecording',
  FINISH: 'finish',
});

export const STEPS = Object.freeze([STEP.ACCESSIBILITY, STEP.SCREEN_RECORDING, STEP.FINISH]);

const KIND_OF_STEP = Object.freeze({
  [STEP.ACCESSIBILITY]: PERMISSION_KIND.ACCESSIBILITY,
  [STEP.SCREEN_RECORDING]: PERMISSION_KIND.SCREEN_RECORDING,
});

/** The permission a step is about, or null for the finish step. */
export function kindOfStep(step) {
  return KIND_OF_STEP[step] ?? null;
}

/** The persisted step if it is one we know, otherwise the first. */
export function initialStep(persisted) {
  return STEPS.includes(persisted) ? persisted : STEP.ACCESSIBILITY;
}

export function nextStep(step) {
  const i = STEPS.indexOf(step);
  return STEPS[Math.min(i + 1, STEPS.length - 1)];
}

export function progressLabel(step) {
  return `${STEPS.indexOf(step) + 1} of ${STEPS.length}`;
}

/** `grants` is `{ accessibility, screenRecording }` in the native grant vocabulary. */
function grantOf(step, grants) {
  const kind = kindOfStep(step);
  return kind ? grants[kind] : null;
}

/** A permission step can continue once granted; skipping is a separate, always-available action. */
export function canContinue(step, grants) {
  return step === STEP.FINISH || grantOf(step, grants) === GRANT.GRANTED;
}

/** Skipping a step records it once; the finish step cannot be skipped. */
export function skipStep(step, skipped) {
  if (step === STEP.FINISH || skipped.includes(step)) return skipped;
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
export function advanceIfGranted(step, grants) {
  return step !== STEP.FINISH && grantOf(step, grants) === GRANT.GRANTED ? nextStep(step) : step;
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
