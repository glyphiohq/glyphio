// The shortcut self-test's wording and the diagnostics report. Pure, so the rules (what each
// outcome says, what the report may contain) are tested without a window.

/** What the Test button says for each outcome from the native `shortcut_test`. */
export function testMessage(outcome) {
  switch (outcome) {
    case 'received':
      return { ok: true, text: 'Works: Glyphio received it.' };
    case 'notReceived':
      return {
        ok: false,
        text: 'Registered, but the press never reached Glyphio. Another app or a system shortcut is probably taking it; pick a different chord.',
      };
    case 'notRegistered':
      return { ok: false, text: 'Not registered, so Glyphio cannot receive it. Save a different chord.' };
    default:
      return { ok: false, text: `Test failed: ${outcome}` };
  }
}

const STATE_WORDS = {
  registered: 'registered',
  empty: 'not set',
  invalid: 'invalid',
  refused: 'refused by the system',
};

/** One readable line per binding. */
export function shortcutLine(row, labelOf) {
  const label = labelOf(row.key);
  const chord = row.accelerator || '(none)';
  const state = STATE_WORDS[row.state] ?? row.state;
  return `${label}: ${chord} — ${state}${row.reason ? ` (${row.reason})` : ''}`;
}

// Anything URL-shaped is cut from the report, wherever it came from: a report is pasted into
// issues and chats, and a page address must never travel with it.
const URL_PATTERN = /\b(?:[a-z][a-z0-9+.-]*:\/\/|www\.)\S+/gi;

export function redact(text) {
  return String(text).replace(URL_PATTERN, '[address removed]');
}

/** The plain-text report for Copy diagnostics. `d` is the native `diagnostics` result. */
export function formatDiagnostics(d, labelOf = (key) => key) {
  const lines = [
    `Glyphio ${d.version} on ${d.os} (${d.arch})`,
    '',
    'Shortcuts',
    ...(d.shortcuts ?? []).map((row) => `  ${shortcutLine(row, labelOf)}`),
    '',
    `Capture: ${d.capture || 'Ready'}`,
    '',
    'Permissions',
    `  Accessibility: ${d.permissions?.accessibility ?? 'unknown'}`,
    `  Screen Recording: ${d.permissions?.screenRecording ?? 'unknown'}`,
    `  Text expansion running: ${d.permissions?.expansionLive ? 'yes' : 'no'}`,
    '',
    'Engine',
    `  Stopped after repeated crashes: ${d.engine?.stoppedAfterCrashes ? 'yes' : 'no'}`,
    `  Secure Input held by another app: ${d.engine?.secureInputHeld ? 'yes' : 'no'}`,
  ];
  return redact(lines.join('\n'));
}
