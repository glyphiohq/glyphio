/** Delete the saved artifact, when present, then close its native editor window. */
export async function discardCapture(savedId, invoke) {
  if (savedId) await invoke('delete_capture', { id: savedId });
  await invoke('close_editor');
}
