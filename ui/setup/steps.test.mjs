import assert from 'node:assert/strict';
import test from 'node:test';

import { GRANT, PERMISSION_KIND } from '../settings/permissions.mjs';
import {
  advanceIfGranted,
  announcement,
  canContinue,
  initialStep,
  kindOfStep,
  nextStep,
  progressLabel,
  skipStep,
  STEP,
  unresolvedSkips,
} from './steps.mjs';

const ax = PERMISSION_KIND.ACCESSIBILITY;
const screen = PERMISSION_KIND.SCREEN_RECORDING;
const none = { [ax]: GRANT.NOT_DETERMINED, [screen]: GRANT.NOT_DETERMINED };

test('the window opens on the persisted step, or the first when that is unknown', () => {
  assert.equal(initialStep('screenRecording'), STEP.SCREEN_RECORDING);
  assert.equal(initialStep('bogus'), STEP.ACCESSIBILITY);
  assert.equal(initialStep(undefined), STEP.ACCESSIBILITY);
});

test('steps advance in order and stop at finish', () => {
  assert.equal(nextStep(STEP.ACCESSIBILITY), STEP.SCREEN_RECORDING);
  assert.equal(nextStep(STEP.SCREEN_RECORDING), STEP.FINISH);
  assert.equal(nextStep(STEP.FINISH), STEP.FINISH);
  assert.equal(progressLabel(STEP.SCREEN_RECORDING), '2 of 3');
});

test('a permission step continues only once granted; finish always can', () => {
  assert.equal(canContinue(STEP.ACCESSIBILITY, none), false);
  assert.equal(canContinue(STEP.ACCESSIBILITY, { ...none, [ax]: GRANT.GRANTED }), true);
  assert.equal(canContinue(STEP.SCREEN_RECORDING, { ...none, [ax]: GRANT.GRANTED }), false);
  assert.equal(canContinue(STEP.FINISH, none), true);
});

test('skipping records a step once and never the finish step', () => {
  assert.deepEqual(skipStep(STEP.ACCESSIBILITY, []), ['accessibility']);
  assert.deepEqual(skipStep(STEP.ACCESSIBILITY, ['accessibility']), ['accessibility']);
  assert.deepEqual(skipStep(STEP.FINISH, []), []);
});

test('a skipped step that was granted afterwards is no longer unresolved', () => {
  const skipped = ['accessibility', 'screenRecording'];
  assert.deepEqual(unresolvedSkips(skipped, { ...none, [screen]: GRANT.GRANTED }), ['accessibility']);
});

test('after a relaunch a step whose permission is already granted moves on by itself', () => {
  assert.equal(advanceIfGranted(STEP.SCREEN_RECORDING, { ...none, [screen]: GRANT.GRANTED }), STEP.FINISH);
  assert.equal(advanceIfGranted(STEP.SCREEN_RECORDING, none), STEP.SCREEN_RECORDING);
  assert.equal(advanceIfGranted(STEP.FINISH, none), STEP.FINISH);
});

test('each permission step maps to its permission kind', () => {
  assert.equal(kindOfStep(STEP.ACCESSIBILITY), ax);
  assert.equal(kindOfStep(STEP.SCREEN_RECORDING), screen);
  assert.equal(kindOfStep(STEP.FINISH), null);
});

test('only meaningful transitions are announced', () => {
  assert.equal(announcement(ax, GRANT.DENIED, GRANT.GRANTED), 'Accessibility is on.');
  assert.match(announcement(ax, GRANT.DENIED, GRANT.STALE), /Fix it/);
  assert.match(announcement(screen, GRANT.DENIED, GRANT.NEEDS_RELAUNCH), /relaunch/);
  assert.equal(announcement(ax, GRANT.GRANTED, GRANT.GRANTED), null);
  assert.equal(announcement(ax, GRANT.NOT_DETERMINED, GRANT.DENIED), null);
});
