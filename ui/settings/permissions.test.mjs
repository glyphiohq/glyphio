import assert from 'node:assert/strict';
import test from 'node:test';

import {
  afterPermissionPrompt,
  initialPermissionState,
  PERMISSION_KIND,
  PERMISSION_REMEDY,
  permissionPresentation,
  withNativePermissionStatus,
} from './permissions.mjs';

const ax = PERMISSION_KIND.ACCESSIBILITY;
const screen = PERMISSION_KIND.SCREEN_RECORDING;

test('only an explicitly available first prompt starts promptable', () => {
  assert.equal(initialPermissionState({ promptAvailable: true }).remedy,
    PERMISSION_REMEDY.PROMPT);
  assert.equal(initialPermissionState({ promptAvailable: false }).remedy,
    PERMISSION_REMEDY.SETTINGS);
});

test('an ungranted capability exposes only its currently useful remedy', () => {
  const row = permissionPresentation(ax, {
    granted: false,
    remedy: PERMISSION_REMEDY.SETTINGS,
  });

  assert.equal(row.status, 'Not granted');
  assert.equal(row.phase, 'settings-only');
  assert.deepEqual(row.actions.map(({ id }) => id), ['settings']);
});

test('a promptable capability offers one legitimate first prompt', () => {
  const row = permissionPresentation(ax, {
    granted: false,
    remedy: PERMISSION_REMEDY.PROMPT,
  });

  assert.equal(row.phase, 'promptable');
  assert.deepEqual(row.actions.map(({ id }) => id), ['request']);
  assert.match(row.description, /text expansion/);
  assert.match(row.description, /scrolling capture/);
});

test('a consumed or unavailable prompt leaves settings as the only action', () => {
  const row = permissionPresentation(screen, {
    granted: false,
    remedy: PERMISSION_REMEDY.SETTINGS,
  });

  assert.equal(row.phase, 'settings-only');
  assert.deepEqual(row.actions, [{
    id: 'settings',
    label: 'Open Screen Recording settings',
    style: 'secondary',
  }]);
});

test('an accepted Screen Recording prompt offers only the required relaunch', () => {
  const state = afterPermissionPrompt(screen, true);
  const row = permissionPresentation(screen, state);

  assert.equal(row.phase, 'relaunch-required');
  assert.deepEqual(row.actions.map(({ id }) => id), ['relaunch']);
  assert.match(row.description, /capture mode/);
  assert.match(row.description, /relaunch/);
});

test('a granted capability has no remediation action', () => {
  for (const kind of [ax, screen]) {
    const row = permissionPresentation(kind, {
      granted: true,
      remedy: PERMISSION_REMEDY.SETTINGS,
    });
    assert.equal(row.phase, 'granted');
    assert.deepEqual(row.actions, []);
  }
});

test('native refresh preserves an unused prompt and turns a revocation into settings-only', () => {
  assert.deepEqual(
    withNativePermissionStatus({ granted: null, remedy: PERMISSION_REMEDY.PROMPT }, false),
    { granted: false, remedy: PERMISSION_REMEDY.PROMPT },
  );
  assert.deepEqual(
    withNativePermissionStatus({ granted: true, remedy: PERMISSION_REMEDY.PROMPT }, false),
    { granted: false, remedy: PERMISSION_REMEDY.SETTINGS },
  );
});
