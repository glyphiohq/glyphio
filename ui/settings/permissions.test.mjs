import assert from 'node:assert/strict';
import test from 'node:test';

import {
  checklistVisible,
  GRANT,
  grantPresentation,
  grantSurface,
  PERMISSION_KIND,
} from './permissions.mjs';

const ax = PERMISSION_KIND.ACCESSIBILITY;
const screen = PERMISSION_KIND.SCREEN_RECORDING;

test('no checklist when everything is resolved, but the rows stay inspectable', () => {
  const surface = grantSurface({ [ax]: GRANT.GRANTED, [screen]: GRANT.GRANTED });
  assert.equal(checklistVisible(surface, false), false);
  assert.deepEqual(surface.rows.map(([kind, row]) => [kind, row.status]),
    [[ax, 'Granted'], [screen, 'Granted']]);
});

test('an unresolved grant shows the checklist, except while General shows the same rows', () => {
  for (const grant of [GRANT.NOT_DETERMINED, GRANT.DENIED, GRANT.STALE, GRANT.NEEDS_RELAUNCH]) {
    const surface = grantSurface({ [ax]: GRANT.GRANTED, [screen]: grant });
    assert.equal(checklistVisible(surface, false), true, grant);
    assert.equal(checklistVisible(surface, true), false, grant);
  }
});

test('the checklist waits for a first reading instead of flashing', () => {
  const surface = grantSurface({ [ax]: null, [screen]: null });
  assert.equal(checklistVisible(surface, false), false);
  assert.ok(surface.rows.every(([, row]) => row.phase === 'checking'));
});

test('rows follow live grant changes', () => {
  const before = grantSurface({ [ax]: GRANT.DENIED, [screen]: GRANT.GRANTED });
  const after = grantSurface({ [ax]: GRANT.GRANTED, [screen]: GRANT.GRANTED });
  assert.equal(before.rows[0][1].actions.length, 1);
  assert.equal(after.rows[0][1].actions.length, 0);
  assert.equal(checklistVisible(after, false), false);
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
