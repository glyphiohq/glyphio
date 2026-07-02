// config.js — single source of truth for the extension.
//
// All extension-wide defaults live here so that renaming, rebinding the
// shortcut, or changing default behaviours is a one-line edit. After editing
// any field marked "(manifest)", run:
//
//     node scripts/sync-manifest.js
//
// to regenerate manifest.json from these values.
//
// The module is consumed from three places:
//   - background/service_worker.js      (ES module import)
//   - preview/preview.js                (ES module import via <script type=module>)
//   - options/options.js                (ES module import via <script type=module>)
//   - scripts/sync-manifest.js          (Node)
// Content scripts cannot use ES imports; the service worker forwards any
// config values the scroller needs via its message payloads.

/** @typedef {Readonly<typeof config>} Config */
export const config = {
  // --- Identity (manifest) --------------------------------------------------
  /** Product name (rebranded from Checkpoint). */
  name: 'Glyphio',
  /** Short name used where space is tight (toolbar tooltip, etc.). */
  shortName: 'Glyphio',
  /** SemVer, surfaced in manifest.json. */
  version: '2.3.0',

  // --- Developer toggles ----------------------------------------------------
  /**
   * When true, verbose per-step logs are emitted to the service-worker
   * console. Flip to true while iterating on the capture pipeline; keep
   * false for day-to-day use. Errors are always logged regardless.
   */
  debug: false,
  /** One-line blurb for the Extensions page. */
  description:
    'Timestamped screenshots with inner-scroll, iframe support, local history, and snip + crop tools. Built at .',

  // --- Keyboard shortcuts (manifest commands) -------------------------------
  // All four are rebindable at chrome://extensions/shortcuts. Chrome requires
  // at least one modifier for extension commands, so these are not bare
  // letters the way the preview-page shortcuts can be.
  //
  // Defaults are `Option+Shift+<letter>` on every platform. Chrome's
  // `Alt+Shift+…` manifest string maps to Option+Shift on macOS (Mac
  // keyboards label the Alt key as Option), so the same shorthand works
  // cross-platform. This avoids clashing with OS-level Command-Shift-*
  // shortcuts on macOS (Command+Shift+H = Hide Others, etc.).
  shortcut: {
    default: 'Alt+Shift+S',
    mac: 'Alt+Shift+S',
    commandId: 'capture',
    description: 'Capture full page with timestamp'
  },
  historyShortcut: {
    default: 'Alt+Shift+H',
    mac: 'Alt+Shift+H',
    commandId: 'open-history',
    description: 'Open Checkpoint history'
  },
  visibleShortcut: {
    default: 'Alt+Shift+V',
    mac: 'Alt+Shift+V',
    commandId: 'capture-visible',
    description: 'Capture visible area only'
  },
  snipShortcut: {
    default: 'Alt+Shift+X',
    mac: 'Alt+Shift+X',
    commandId: 'capture-snip',
    description: 'Snip a region of the visible tab'
  },

  // --- Downloads ------------------------------------------------------------
  download: {
    /**
     * Subfolder under the browser's Downloads directory. '' saves flat.
     * Chrome does not permit absolute paths — this is always relative to
     * the user's download root as configured in chrome://settings/downloads.
     */
    subdir: 'Checkpoint',
    /** Prefix for the downloaded filename. Final shape: `<prefix>-<timestamp>.png`. */
    filenamePrefix: 'checkpoint'
  },

  // --- Capture behaviour ----------------------------------------------------
  capture: {
    /**
     * Ms to wait after scrolling and before capturing, so the page has time
     * to repaint. Independent of the rate limit below.
     */
    postScrollDelayMs: 120,
    /**
     * Chrome's MAX_CAPTURE_VISIBLE_TAB_CALLS_PER_SECOND is 2 — i.e. a 500ms
     * hard minimum between captureVisibleTab calls per window. We use 550ms
     * to give ourselves margin for clock skew. If you lower this, expect
     * sporadic "quota exceeded" errors on long pages.
     */
    captureMinIntervalMs: 550,
    /** Number of retry attempts if Chrome returns a quota error. */
    captureQuotaRetries: 3,
    /**
     * Hard cap on stitched image height in CSS pixels. Canvas2D in Chromium
     * tops out around 32767px per side; we stay under that.
     */
    maxStitchHeight: 32000,
    /** Safety: maximum scroll iterations before we bail. */
    maxCaptureLoopGuard: 200,
    /**
     * When true, the extension scans all frames and picks the best scrollable
     * target (iframe, overflow:auto div, or window). When false, always
     * scrolls the top window.
     */
    autoScrollContainers: true,
    /**
     * Minimum content "overflow" (scrollHeight - clientHeight) to treat an
     * element/frame as a candidate scroll target, in CSS pixels.
     */
    minScrollableOverflowPx: 200
  },

  // --- Banner ---------------------------------------------------------------
  banner: {
    /** 'device-locale' | 'iso-8601' | 'utc-human' */
    timestampFormat: 'device-locale',
    /** 'device' (OS default) or IANA zone such as 'UTC', 'Europe/London'. */
    timezone: 'device',
    /** 'device' or BCP-47 tag such as 'en-GB'. */
    locale: 'device',
    /** Render URL line under the timestamp in the banner. */
    showUrl: true,
    /** Colours are hex strings. Defaults tuned for legibility on light screens. */
    backgroundColor: '#1f2937',
    foregroundColor: '#ffffff',
    mutedColor: '#cbd5e1',
    // Typography (CSS px; scaled on the canvas to match captured image DPR).
    paddingPx: 16,
    lineGapPx: 6,
    timestampFontPx: 20,
    urlFontPx: 14,
    noteFontPx: 14,
    fontFamily:
      '-apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif'
  },

  // --- Storage --------------------------------------------------------------
  storage: {
    /** chrome.storage.sync key for user-editable settings. */
    settingsKey: 'userSettings',
    /** Prefix for chrome.storage.session entries holding capture payloads. */
    sessionPayloadPrefix: 'capture:'
  },

  // --- History (local IndexedDB store) --------------------------------------
  history: {
    /** IndexedDB database + store names. */
    dbName: 'checkoutScreenshotHistory',
    storeName: 'captures',
    /** Retention caps. Oldest entries are evicted first when either is exceeded. */
    maxCount: 50,
    maxBytes: 200 * 1024 * 1024, // 200 MB
    /** Width of the generated thumbnail in physical pixels. */
    thumbnailWidthPx: 320
  }
};

/**
 * Default preview-page keyboard shortcut bindings.
 *
 * Stored as plain data (no code strings). `modifiers` is a list whose entries
 * are drawn from the enum below; `mod` is a logical modifier that matches
 * Cmd on macOS and Ctrl on Windows/Linux.
 *
 *   'mod' | 'shift' | 'alt'
 */
export const DEFAULT_SHORTCUTS = Object.freeze({
  copy:    { modifiers: ['mod'], key: 'c' },
  save:    { modifiers: ['mod'], key: 's' },
  retry:   { modifiers: [],      key: 'r' },
  // `close` is unbound by default. Chrome's F11 fullscreen mode treats Esc
  // as "exit fullscreen" at the browser-chrome level (preventDefault from
  // the page can't suppress it), so binding close to Esc meant one keypress
  // unmaximised the window AND closed the tab. Cmd+W / Ctrl+W still close
  // the tab natively. Users can rebind to any key under Settings.
  close:   { modifiers: [],      key: '' },
  history: { modifiers: [],      key: 'h' },
  crop:    { modifiers: [],      key: 'c' },
  redact:  { modifiers: [],      key: 'b' },
  draw:    { modifiers: [],      key: 'd' },
  text:    { modifiers: [],      key: 't' }
});

/** User-editable settings shape (subset of config, persisted in chrome.storage.sync). */
export const userSettingsShape = {
  // Capture
  autoScrollContainers: config.capture.autoScrollContainers,
  /**
   * Before capturing viewports 2..N of a full-page scroll-and-stitch, hide any
   * elements that the scroller's scroll-and-measure detector identifies as
   * pinned to the top of the viewport. Catches sticky nav bars that use
   * JS-based positioning (e.g. scroll-synced transforms) — which the purely
   * CSS-based neutralise pass can't see. Default on; turn off if you need a
   * site's nav bar to appear in every stacked viewport of the final image.
   */
  hideStickyOnScroll: true,
  // Capture modes (individually toggleable; shortcuts stay registered either way)
  enableVisibleCapture: true,
  enableSnipCapture: true,
  enableCrop: true,
  enableRedact: true,
  enableDraw: true,
  enableText: true,
  // Banner
  showTimestamp: true,
  timestampFormat: config.banner.timestampFormat,
  timezone: config.banner.timezone,
  locale: config.banner.locale,
  showUrl: config.banner.showUrl,
  showTargetFrameUrl: false,
  bannerBg: config.banner.backgroundColor,
  bannerFg: config.banner.foregroundColor,
  bannerMuted: config.banner.mutedColor,
  // Downloads
  downloadSubdir: config.download.subdir,
  filenamePrefix: config.download.filenamePrefix,
  // Workflow
  autoCopyOnOpen: true,
  // History
  historyEnabled: true,
  // Preview-page keyboard shortcuts
  shortcuts: { ...DEFAULT_SHORTCUTS }
};

/**
 * Merge user-saved settings (from chrome.storage.sync) with defaults.
 * Unknown keys in stored are dropped; missing keys fall back to defaults.
 * The `shortcuts` sub-object is merged per-action so a partially-stored
 * shortcut table still yields a complete set.
 */
export function resolveSettings(stored) {
  const resolved = { ...userSettingsShape, shortcuts: { ...DEFAULT_SHORTCUTS } };
  if (stored && typeof stored === 'object') {
    for (const k of Object.keys(userSettingsShape)) {
      if (k === 'shortcuts') continue;
      if (k in stored) resolved[k] = stored[k];
    }
    if (stored.shortcuts && typeof stored.shortcuts === 'object') {
      for (const action of Object.keys(DEFAULT_SHORTCUTS)) {
        const s = stored.shortcuts[action];
        if (s && typeof s === 'object' && typeof s.key === 'string' && Array.isArray(s.modifiers)) {
          resolved.shortcuts[action] = { modifiers: [...s.modifiers], key: s.key };
        }
      }
    }
  }
  return resolved;
}
