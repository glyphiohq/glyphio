// settings/model.mjs — the Settings window's shared state, plus the snippet vocabulary the
// list and the editor both draw from. Kept apart from settings.js so the views it loads on
// demand (snippet-editor.mjs, history-view.mjs) can share it without importing the shell.

export const state = {
  snippets: [],
  groups: [],
  settings: null,
  selected: 'all', // 'all' | 'ungrouped' | 'settings' | <groupId>
  settingsTab: 'capture',
  search: '',
  groupSearch: '',        // sidebar group filter
  syncStatus: null,
  syncConfig: null,
  selectedTeam: null,     // team whose roster is shown in the sync panel
  teamMembers: {},        // team -> [{sub,email,lastSeen}]
  memberSearch: '',
};

export const FORMATS = [
  { id: 'plain', label: 'Plain', badge: 'Aa' },
  { id: 'html', label: 'Rich', badge: '❛❜' },
  { id: 'markdown', label: 'Markdown', badge: 'M↓' },
];

export const KINDS = [
  { id: 'text', label: 'Text' },
  { id: 'form', label: 'Form' },
  { id: 'popup', label: 'Popup' },
  { id: 'command', label: 'Command' },
];

export const KIND_HINTS = {
  text: 'The content replaces the trigger.',
  form: 'The trigger opens a form; the filled-in content is pasted.',
  popup: 'The trigger opens a popup showing the content. Nothing is pasted.',
  command: 'Pastes the output of a shell command. Runs on this device only and never syncs.',
};
