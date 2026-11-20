import assert from 'node:assert/strict';
import test from 'node:test';

import { registrationNotice, statusFor } from './shortcut-status.mjs';

test('a refused chord says it is not registered and why', () => {
  const text = registrationNotice({ key: 'k', state: 'refused', reason: 'taken by another app' });
  assert.match(text, /Not registered/);
  assert.match(text, /taken by another app/);
});

test('an invalid chord is flagged too', () => {
  assert.match(registrationNotice({ key: 'k', state: 'invalid', reason: 'already used by another action' }),
    /Not registered — already used/);
});

test('working, empty and unknown bindings show nothing', () => {
  assert.equal(registrationNotice({ key: 'k', state: 'registered' }), '');
  assert.equal(registrationNotice({ key: 'k', state: 'empty' }), '');
  assert.equal(registrationNotice(null), '');
});

test('finds a status by settings key', () => {
  const list = [{ key: 'a', state: 'registered' }, { key: 'b', state: 'refused', reason: 'x' }];
  assert.equal(statusFor(list, 'b').state, 'refused');
  assert.equal(statusFor(list, 'zzz'), null);
  assert.equal(statusFor(undefined, 'a'), null);
});
