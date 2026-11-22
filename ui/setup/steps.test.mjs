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
  stepsFor,
  unresolvedSkips,
  loginItemCopy,
  reduceTryIt,
  shortcutRows,
  teamAction,
  TRY_IT,
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
  assert.equal(nextStep(STEP.SCREEN_RECORDING), STEP.SHORTCUTS);
  assert.equal(nextStep(STEP.SHORTCUTS), STEP.FINISH);
  assert.equal(nextStep(STEP.FINISH), STEP.FINISH);
  assert.equal(progressLabel(STEP.SCREEN_RECORDING), '2 of 4');
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
  assert.equal(advanceIfGranted(STEP.SCREEN_RECORDING, { ...none, [screen]: GRANT.GRANTED }), STEP.SHORTCUTS);
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

test('macOS shows the permission steps; Windows skips them', () => {
  assert.deepEqual(stepsFor('macos'), ['accessibility', 'screenRecording', 'shortcuts', 'finish']);
  assert.deepEqual(stepsFor('windows'), ['shortcuts', 'finish']);
  assert.deepEqual(stepsFor('anything-else'), ['shortcuts', 'finish']);
});

test('on Windows progress, the first step and moving on follow the shorter list', () => {
  const win = stepsFor('windows');
  assert.equal(initialStep('accessibility', win), STEP.SHORTCUTS);
  assert.equal(initialStep('finish', win), STEP.FINISH);
  assert.equal(nextStep(STEP.SHORTCUTS, win), STEP.FINISH);
  assert.equal(progressLabel(STEP.SHORTCUTS, win), '1 of 2');
  assert.equal(advanceIfGranted(STEP.SHORTCUTS, none, win), STEP.SHORTCUTS);
});

test('the shortcuts step can always continue and leaves nothing to resolve later', () => {
  assert.equal(canContinue(STEP.SHORTCUTS, none), true);
  assert.deepEqual(skipStep(STEP.SHORTCUTS, []), []);
});

const settings = {
  shortcutCaptureSnip: 'Alt+Shift+X',
  shortcutOpenPalette: 'Control+Shift+Space',
  shortcutOpenClipboard: 'Alt+Shift+C',
};

test('each default shortcut is listed with how the system answered', () => {
  const rows = shortcutRows(settings, [
    { key: 'shortcutCaptureSnip', state: 'registered' },
    { key: 'shortcutOpenPalette', state: 'refused', reason: 'another app holds it' },
  ], 'macos');
  assert.deepEqual(rows.map((r) => r.label), ['Capture', 'Snippet palette', 'Clipboard history']);
  assert.equal(rows[0].keys, '⌥⇧X');
  assert.equal(rows[0].ok, true);
  assert.equal(rows[0].notice, '');
  assert.equal(rows[1].ok, false);
  assert.match(rows[1].notice, /Not registered — another app holds it/);
  // No answer yet is not a failure and not a success.
  assert.equal(rows[2].ok, false);
  assert.equal(rows[2].notice, '');
});

test('a refused shortcut offers ordered alternatives that are valid and unused', () => {
  const rows = shortcutRows(settings, [
    { key: 'shortcutOpenPalette', state: 'refused', reason: 'another app holds it' },
    { key: 'shortcutCaptureSnip', state: 'registered' },
  ], 'macos');
  const palette = rows[1];
  assert.deepEqual(palette.alternatives.map((a) => a.accelerator), [
    'Control+Alt+Space', 'Alt+Shift+Space', 'Control+Shift+P',
  ]);
  assert.equal(palette.alternatives[0].keys, '⌃⌥Space');
  // A working shortcut offers nothing, and neither does one with no answer yet.
  assert.deepEqual(rows[0].alternatives, []);
  assert.deepEqual(shortcutRows(settings, [], 'macos')[1].alternatives, []);
});

test('an invalid binding also offers alternatives; a chord already in use is not offered', () => {
  const rows = shortcutRows(
    { ...settings, shortcutOpenClipboard: 'Control+Alt+Space' },
    [{ key: 'shortcutOpenPalette', state: 'invalid', reason: 'taken' }],
    'windows',
  );
  assert.equal(rows[1].alternatives.some((a) => a.accelerator === 'Control+Alt+Space'), false);
  assert.ok(rows[1].alternatives.length > 0);
});

test('Windows shows chords as written', () => {
  const rows = shortcutRows(settings, [], 'windows');
  assert.equal(rows[0].keys, 'Alt+Shift+X');
});

test('try-it completes only when a capture is delivered', () => {
  let t = TRY_IT.WAITING;
  t = reduceTryIt(t, 'shortcut-pressed');
  assert.equal(t, TRY_IT.WAITING);
  t = reduceTryIt(t, 'capture-failed');
  assert.equal(t, TRY_IT.WAITING);
  t = reduceTryIt(t, 'capture-delivered');
  assert.equal(t, TRY_IT.DONE);
  assert.equal(reduceTryIt(t, 'capture-failed'), TRY_IT.DONE);
});

test('the login item explanation comes from the platform table and states the effect', () => {
  assert.match(loginItemCopy('macos').explanation, /Login Items/);
  assert.match(loginItemCopy('windows').explanation, /sign in/);
  assert.equal(loginItemCopy('macos').defaultOn, true);
});

test('the team choice stays in setup for "just me" and hands the rest to Settings', () => {
  assert.deepEqual(teamAction('personal'), { kind: 'stay' });
  assert.deepEqual(teamAction('invite'), { kind: 'ask-invite' });
  assert.deepEqual(teamAction('setup'), { kind: 'open-team', invite: null });
  assert.deepEqual(teamAction('invite', '  glyphio://join?server=x  '), {
    kind: 'open-team',
    invite: 'glyphio://join?server=x',
  });
  assert.deepEqual(teamAction('invite', '   '), { kind: 'ask-invite' });
});
