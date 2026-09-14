// One delivery contract shared by the visible editor and the invisible capture worker.
// History is attempted first so a clipboard failure still has a durable recovery target.

export async function deliverCapture({ saveToHistory, copyToClipboard }) {
  const outcome = {
    history: { status: 'failed', id: '', error: '' },
    clipboard: { status: 'failed', error: '' },
  };

  try {
    outcome.history = { status: 'saved', id: (await saveToHistory()) || '', error: '' };
  } catch (error) {
    outcome.history.error = errorMessage(error);
  }

  // A history failure must not prevent the image reaching the clipboard. We wait for both
  // independent outcomes before acknowledging the capture to the native lifecycle.
  try {
    await copyToClipboard();
    outcome.clipboard = { status: 'copied', error: '' };
  } catch (error) {
    outcome.clipboard.error = errorMessage(error);
  }

  return outcome;
}

export function deliveryReport(outcome) {
  const errors = [];
  if (outcome.history.status !== 'saved') {
    errors.push(`Capture history failed: ${outcome.history.error}`);
  }
  if (outcome.clipboard.status !== 'copied') {
    errors.push(`Clipboard copy failed: ${outcome.clipboard.error}`);
  }
  return {
    historyId: outcome.history.status === 'saved' ? outcome.history.id : null,
    error: errors.length ? errors.join('\n') : null,
  };
}

function errorMessage(error) {
  return error?.message || String(error);
}
