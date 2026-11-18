// What Settings says about a shortcut the system did not accept. The registry in the app returns
// one status per binding: { key, state: 'registered' | 'empty' | 'invalid' | 'refused', reason }.

/** The line to show beside a binding, or '' when there is nothing to warn about. */
export function registrationNotice(status) {
  if (!status) return '';
  if (status.state === 'refused') {
    return `Not registered — ${status.reason || 'the system would not give it to Glyphio'}. Record another.`;
  }
  if (status.state === 'invalid') {
    return `Not registered — ${status.reason || 'this is not a usable shortcut'}. Record another.`;
  }
  return '';
}

/** The status for one settings key, from the list the app returned. */
export function statusFor(statuses, key) {
  return (statuses ?? []).find((s) => s.key === key) ?? null;
}
