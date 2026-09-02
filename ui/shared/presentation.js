// Display-only defaults. Persisted product preferences come from native Settings.

export const PRODUCT_NAME = 'Glyphio';

export const BANNER_PRESENTATION = Object.freeze({
  paddingPx: 16,
  lineGapPx: 6,
  timestampFontPx: 20,
  urlFontPx: 14,
  noteFontPx: 14,
  fontFamily: '-apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif',
});

// Editor actions are not user-configurable product settings.
export const DEFAULT_EDITOR_SHORTCUTS = Object.freeze({
  copy: { modifiers: ['mod'], key: 'c' },
  save: { modifiers: ['mod'], key: 's' },
  retry: { modifiers: [], key: 'r' },
  close: { modifiers: [], key: '' },
  history: { modifiers: [], key: 'h' },
  crop: { modifiers: [], key: 'c' },
  redact: { modifiers: [], key: 'b' },
  draw: { modifiers: [], key: 'd' },
  text: { modifiers: [], key: 't' },
});
