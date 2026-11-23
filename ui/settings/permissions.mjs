export const PERMISSION_KIND = Object.freeze({
  ACCESSIBILITY: 'accessibility',
  SCREEN_RECORDING: 'screenRecording',
});

const COPY = Object.freeze({
  [PERMISSION_KIND.ACCESSIBILITY]: {
    name: 'Accessibility',
    description: 'Enables text expansion and lets scrolling capture move content automatically.',
    promptLabel: 'Grant Accessibility…',
    settingsLabel: 'Open Accessibility settings',
  },
  [PERMISSION_KIND.SCREEN_RECORDING]: {
    name: 'Screen Recording',
    description: 'Enables every capture mode. A new grant may require Glyphio to relaunch.',
    promptLabel: 'Grant Screen Recording…',
    settingsLabel: 'Open Screen Recording settings',
  },
});

/** The grant states the native permission model reports (`permissions.rs`). */
export const GRANT = Object.freeze({
  GRANTED: 'granted',
  NOT_DETERMINED: 'notDetermined',
  DENIED: 'denied',
  STALE: 'stale',
  NEEDS_RELAUNCH: 'needsRelaunch',
});

/**
 * One row for one native grant. Exactly one useful action while the capability is unavailable;
 * none while granted. The first-run prompt is offered only for `notDetermined`, never inferred
 * from web storage, so clearing webview data cannot change which button appears.
 */
export function grantPresentation(kind, grant) {
  const copy = COPY[kind];
  if (!copy) throw new Error(`unknown permission kind: ${kind}`);
  switch (grant) {
    case GRANT.GRANTED:
      return { ...copy, phase: 'granted', status: 'Granted', actions: [] };
    case GRANT.NOT_DETERMINED:
      return {
        ...copy,
        phase: 'promptable',
        status: 'Not granted',
        actions: [{ id: 'request', label: copy.promptLabel, style: 'primary' }],
      };
    case GRANT.DENIED:
      return {
        ...copy,
        phase: 'settings-only',
        status: 'Waiting for you to switch it on',
        actions: [{ id: 'settings', label: copy.settingsLabel, style: 'primary' }],
      };
    case GRANT.STALE:
      return {
        ...copy,
        phase: 'stale',
        status: 'Blocked by macOS',
        explanation: 'macOS shows Glyphio as enabled but is still blocking it. This happens after an update.',
        actions: [{ id: 'repair', label: 'Fix it', style: 'primary' }],
      };
    case GRANT.NEEDS_RELAUNCH:
      return {
        ...copy,
        phase: 'relaunch-required',
        status: 'One more step',
        actions: [{ id: 'relaunch', label: 'Relaunch to finish', style: 'primary' }],
      };
    default:
      return { ...copy, phase: 'checking', status: 'Checking…', actions: [] };
  }
}

/** Both rows for the native grants, in a stable order. `grants` maps kind → grant or null. */
export function grantSurface(grants) {
  const rows = Object.values(PERMISSION_KIND).map((kind) => [
    kind,
    grantPresentation(kind, grants[kind] ?? 'checking'),
  ]);
  return {
    rows,
    // Unknown is not unresolved: the panel waits for a reading rather than flashing.
    unresolved: rows.some(([, row]) => row.phase !== 'granted' && row.phase !== 'checking'),
  };
}

/**
 * The global "Finish setup" checklist shows only while something is unresolved, and not while
 * the user is already looking at the same rows in Settings → General.
 */
export function checklistVisible(surface, inspectingInSettings) {
  return surface.unresolved && !inspectingInSettings;
}
