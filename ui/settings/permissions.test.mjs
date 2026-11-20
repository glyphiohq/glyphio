import assert from 'node:assert/strict';
import test from 'node:test';

import {
  afterPermissionPrompt,
  GRANT,
  grantPresentation,
  initialPermissionState,
  PERMISSION_KIND,
  PERMISSION_REMEDY,
  permissionPresentation,
  permissionSurface,
  shouldShowPermissionBanner,
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

test('granted permissions remain inspectable while the global banner stays quiet', () => {
  const surface = permissionSurface({
    [ax]: { granted: true, remedy: PERMISSION_REMEDY.SETTINGS },
    [screen]: { granted: true, remedy: PERMISSION_REMEDY.SETTINGS },
  });

  assert.equal(surface.bannerVisible, false);
  assert.equal(shouldShowPermissionBanner(surface, false), false);
  assert.deepEqual(surface.rows.map(([kind, row]) => [kind, row.status]), [
    [ax, 'Granted'],
    [screen, 'Granted'],
  ]);
});

test('permission remedies appear in one place while their Settings status is open', () => {
  const surface = permissionSurface({
    [ax]: { granted: false, remedy: PERMISSION_REMEDY.SETTINGS },
    [screen]: { granted: true, remedy: PERMISSION_REMEDY.SETTINGS },
  });

  assert.equal(shouldShowPermissionBanner(surface, false), true);
  assert.equal(shouldShowPermissionBanner(surface, true), false);
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

test('each native grant has exactly one remedy, and granted has none', () => {
  const expected = {
    [GRANT.GRANTED]: [],
    [GRANT.NOT_DETERMINED]: ['request'],
    [GRANT.DENIED]: ['settings'],
    [GRANT.STALE]: ['repair'],
    [GRANT.NEEDS_RELAUNCH]: ['relaunch'],
  };
  for (const kind of [ax, screen]) {
    for (const [grant, ids] of Object.entries(expected)) {
      assert.deepEqual(grantPresentation(kind, grant).actions.map((a) => a.id), ids, `${kind} ${grant}`);
    }
  }
});

test('a stale grant explains itself and offers Fix it; relaunch is only offered when needed', () => {
  const stale = grantPresentation(ax, GRANT.STALE);
  assert.match(stale.explanation, /still blocking it/);
  assert.equal(stale.actions[0].label, 'Fix it');
  for (const grant of [GRANT.GRANTED, GRANT.NOT_DETERMINED, GRANT.DENIED, GRANT.STALE]) {
    assert.ok(!grantPresentation(screen, grant).actions.some((a) => a.id === 'relaunch'));
  }
  assert.equal(grantPresentation(screen, GRANT.NEEDS_RELAUNCH).actions[0].label, 'Relaunch to finish');
});

test('status is conveyed in words, not only colour', () => {
  for (const grant of Object.values(GRANT)) {
    assert.ok(grantPresentation(ax, grant).status.length > 0);
  }
});
