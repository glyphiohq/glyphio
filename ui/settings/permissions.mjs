export const PERMISSION_KIND = Object.freeze({
  ACCESSIBILITY: 'accessibility',
  SCREEN_RECORDING: 'screenRecording',
});

export const PERMISSION_REMEDY = Object.freeze({
  PROMPT: 'prompt',
  SETTINGS: 'settings',
  RELAUNCH: 'relaunch',
});

export function initialPermissionState({ promptAvailable }) {
  return {
    granted: null,
    remedy: promptAvailable
      ? PERMISSION_REMEDY.PROMPT
      : PERMISSION_REMEDY.SETTINGS,
  };
}

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

/**
 * The permission UI's observable contract. A row gets exactly one useful remedy while its
 * capability is unavailable; granted and checking rows deliberately have no action.
 */
export function permissionPresentation(kind, state) {
  const copy = COPY[kind];
  if (!copy) throw new Error(`unknown permission kind: ${kind}`);

  if (state.granted === null) {
    return { ...copy, phase: 'checking', status: 'Checking…', actions: [] };
  }
  if (state.granted) {
    return { ...copy, phase: 'granted', status: 'Granted', actions: [] };
  }

  switch (state.remedy) {
    case PERMISSION_REMEDY.PROMPT:
      return {
        ...copy,
        phase: 'promptable',
        status: 'Not granted',
        actions: [{ id: 'request', label: copy.promptLabel, style: 'primary' }],
      };
    case PERMISSION_REMEDY.RELAUNCH:
      return {
        ...copy,
        phase: 'relaunch-required',
        status: 'Relaunch required',
        actions: [{ id: 'relaunch', label: 'Relaunch Glyphio', style: 'primary' }],
      };
    case PERMISSION_REMEDY.SETTINGS:
    default:
      return {
        ...copy,
        phase: 'settings-only',
        status: 'Not granted',
        actions: [{ id: 'settings', label: copy.settingsLabel, style: 'secondary' }],
      };
  }
}

/** Both rows always belong to the inspectable Settings surface; only unmet access is a banner. */
export function permissionSurface(states) {
  const rows = Object.entries(states).map(([kind, state]) => [
    kind,
    permissionPresentation(kind, state),
  ]);
  return {
    rows,
    bannerVisible: rows.some(([, row]) => row.phase !== 'granted'),
  };
}

export function shouldShowPermissionBanner(surface, inspectingInSettings) {
  return surface.bannerVisible && !inspectingInSettings;
}

/** Keep an unspent first-prompt path, but never resurrect it after a revocation. */
export function withNativePermissionStatus(state, granted) {
  if (granted) return { ...state, granted: true };
  if (state.granted === true) {
    return { granted: false, remedy: PERMISSION_REMEDY.SETTINGS };
  }
  return { ...state, granted: false };
}

/** The Screen Recording prompt tells us whether a relaunch can apply the new grant. */
export function afterPermissionPrompt(kind, accepted) {
  if (accepted && kind === PERMISSION_KIND.SCREEN_RECORDING) {
    return { granted: false, remedy: PERMISSION_REMEDY.RELAUNCH };
  }
  if (accepted) return { granted: true, remedy: PERMISSION_REMEDY.SETTINGS };
  return { granted: false, remedy: PERMISSION_REMEDY.SETTINGS };
}

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
