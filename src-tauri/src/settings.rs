//! User settings. Replaces Checkpoint's `chrome.storage.sync` with a JSON file on disk.
//! Keys mirror Checkpoint's `userSettingsShape` (camelCase) so the ported editor/history JS
//! consumes them unchanged.

use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    // ---- capture mode toggles ----
    pub enable_visible_capture: bool,
    pub enable_snip_capture: bool,
    pub enable_full_window_capture: bool,
    pub enable_front_window_capture: bool,
    pub enable_scrolling_capture: bool,

    // ---- edit tool toggles (Settings -> Capture modes pattern) ----
    pub enable_crop: bool,
    pub enable_redact: bool,
    pub enable_draw: bool,
    pub enable_text: bool,

    // ---- banner options ----
    pub show_timestamp: bool,
    pub timestamp_format: String, // "device-locale" | "iso-8601" | "utc-human"
    pub timezone: String,         // "device" | IANA zone
    pub locale: String,           // "device" | BCP-47
    /// The window/app title, for every capture. Named `showUrl` before there was a real URL
    /// to show; the alias keeps existing settings files working.
    #[serde(alias = "showUrl")]
    pub show_window_title: bool,
    // What the browser knows about the page, for window-targeted captures. All off by
    // default: a URL or a profile name baked into a screenshot follows it everywhere it's
    // pasted, so it is opted into rather than out of.
    pub show_page_title: bool,
    pub show_page_url: bool,
    pub show_browser_profile: bool,
    pub banner_bg: String,
    pub banner_fg: String,
    pub banner_muted: String,

    // ---- downloads / workflow ----
    pub download_subdir: String,
    pub filename_prefix: String,
    pub auto_copy_on_open: bool,
    /// Capture straight to the clipboard and history without opening the editor.
    pub silent_capture: bool,
    /// Ask GitHub on launch whether a newer Glyphio exists. The only network call an
    /// otherwise-unconfigured app makes, so it is a setting rather than an assumption.
    pub check_for_updates: bool,
    /// Keep the menu-bar tools available after signing in. A login launch stays windowless.
    pub launch_at_login: bool,

    // ---- history ----
    pub history_enabled: bool,
    pub history_max_count: u32,
    pub history_max_bytes: u64,

    // ---- clipboard history ----
    /// Record what you copy. On by default — a clipboard manager that has to be switched on
    /// remembers nothing about the day you switched it on — but content marked concealed by
    /// a password manager is never recorded either way. See `docs/SECURITY.md`.
    pub clipboard_history: bool,
    pub clipboard_max_items: u32,
    /// Megabytes of stored images to keep. Text costs nothing next to this, so the cap is
    /// expressed in the unit that actually fills a disk.
    pub clipboard_max_mb: u64,
    /// Apps whose copies are never recorded, matched case-insensitively as a substring of
    /// the frontmost app's name. Password managers ship the concealed marker and don't need
    /// listing; this is for everything else a user would rather not keep.
    pub clipboard_ignore_apps: Vec<String>,

    // ---- global capture hotkeys (Tauri accelerator syntax) ----
    pub shortcut_capture_visible: String,
    pub shortcut_capture_snip: String,
    pub shortcut_capture_full: String,
    pub shortcut_capture_front_window: String,
    /// Just the web content of the frontmost browser window (chrome excluded, via AX).
    pub shortcut_capture_page: String,
    pub shortcut_capture_scroll: String,
    pub shortcut_capture_scroll_page: String,
    pub shortcut_open_history: String,
    /// Summons the Spotlight-style snippet search palette.
    pub shortcut_open_palette: String,
    /// Summons the clipboard history picker.
    pub shortcut_open_clipboard: String,

    // ---- the same captures, delivered straight to the clipboard ----
    // Empty by default: nothing is registered until a key is put here, and each one is a
    // silent twin of the mode above it rather than a mode of its own.
    pub shortcut_capture_visible_silent: String,
    pub shortcut_capture_snip_silent: String,
    pub shortcut_capture_full_silent: String,
    pub shortcut_capture_front_window_silent: String,
    pub shortcut_capture_page_silent: String,
    pub shortcut_capture_scroll_silent: String,
    pub shortcut_capture_scroll_page_silent: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enable_visible_capture: true,
            enable_snip_capture: true,
            enable_full_window_capture: true,
            enable_front_window_capture: true,
            enable_scrolling_capture: true,
            enable_crop: true,
            enable_redact: true,
            enable_draw: true,
            enable_text: true,
            show_timestamp: true,
            timestamp_format: "device-locale".into(),
            timezone: "device".into(),
            locale: "device".into(),
            show_window_title: true,
            show_page_title: false,
            show_page_url: false,
            show_browser_profile: false,
            banner_bg: "#1f2937".into(),
            banner_fg: "#ffffff".into(),
            banner_muted: "#cbd5e1".into(),
            download_subdir: "Glyphio".into(),
            filename_prefix: "glyphio".into(),
            auto_copy_on_open: true,
            silent_capture: false,
            check_for_updates: true,
            launch_at_login: true,
            history_enabled: true,
            history_max_count: 50,
            history_max_bytes: 200 * 1024 * 1024, // 200 MB, matching Checkpoint
            clipboard_history: true,
            clipboard_max_items: 200,
            clipboard_max_mb: 100,
            clipboard_ignore_apps: Vec::new(),
            // Alt+Shift+S/V/X mirrors Checkpoint; H opens history.
            shortcut_capture_full: "Alt+Shift+S".into(),
            shortcut_capture_visible: "Alt+Shift+V".into(),
            shortcut_capture_snip: "Alt+Shift+X".into(),
            shortcut_capture_front_window: "Alt+Shift+W".into(), // W = frontmost window
            shortcut_capture_page: "Alt+Shift+B".into(),         // B = browser page (content only)
            shortcut_capture_scroll: "Alt+Shift+L".into(),       // L = long/scrolling area
            shortcut_capture_scroll_page: "Alt+Shift+P".into(),  // P = whole page/window
            shortcut_open_history: "Alt+Shift+H".into(),
            shortcut_open_palette: "Alt+Space".into(),
            shortcut_open_clipboard: "Alt+Shift+C".into(), // C = clipboard
            shortcut_capture_visible_silent: String::new(),
            shortcut_capture_snip_silent: String::new(),
            shortcut_capture_full_silent: String::new(),
            shortcut_capture_front_window_silent: String::new(),
            shortcut_capture_page_silent: String::new(),
            shortcut_capture_scroll_silent: String::new(),
            shortcut_capture_scroll_page_silent: String::new(),
        }
    }
}

impl Settings {
    /// Whether any banner line needs the browser to be asked about the page. Nothing is read
    /// from the browser unless one of these is on — see `capture::ax::browser_meta`.
    pub fn wants_browser_details(&self) -> bool {
        self.show_page_title || self.show_page_url || self.show_browser_profile
    }

    /// The clipboard image cap in bytes. Stored as MB because that is the number a person
    /// has an opinion about.
    pub fn clipboard_max_bytes(&self) -> u64 {
        self.clipboard_max_mb.saturating_mul(1024 * 1024)
    }

    /// Whether the silent-capture worker should be parked in advance: the user has either
    /// made silent the default, or given a mode a straight-to-clipboard key. The tray and the
    /// palette can still ask for one out of the blue — that just creates the worker then.
    pub fn wants_silent_worker(&self) -> bool {
        self.silent_capture
            || self
                .capture_shortcuts()
                .iter()
                .any(|(acc, _, silent)| *silent && !acc.trim().is_empty())
    }

    /// Every capture accelerator: the key, the mode it takes, and whether that key delivers
    /// silently. Both `shortcuts::register` and its handler read this one list, so a key can
    /// never be registered without something to dispatch it to.
    pub fn capture_shortcuts(&self) -> [(&str, &'static str, bool); 14] {
        [
            (&self.shortcut_capture_visible, "visible", false),
            (&self.shortcut_capture_snip, "snip", false),
            (&self.shortcut_capture_full, "fullWindow", false),
            (&self.shortcut_capture_front_window, "frontWindow", false),
            (&self.shortcut_capture_page, "pageOnly", false),
            (&self.shortcut_capture_scroll, "scrolling", false),
            (&self.shortcut_capture_scroll_page, "scrollingPage", false),
            (&self.shortcut_capture_visible_silent, "visible", true),
            (&self.shortcut_capture_snip_silent, "snip", true),
            (&self.shortcut_capture_full_silent, "fullWindow", true),
            (
                &self.shortcut_capture_front_window_silent,
                "frontWindow",
                true,
            ),
            (&self.shortcut_capture_page_silent, "pageOnly", true),
            (&self.shortcut_capture_scroll_silent, "scrolling", true),
            (
                &self.shortcut_capture_scroll_page_silent,
                "scrollingPage",
                true,
            ),
        ]
    }

    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Settings;

    /// `showUrl` was the window/app title before there was a real URL to show. A settings
    /// file written by an older build still means it.
    #[test]
    fn a_settings_file_from_before_the_rename_is_still_understood() {
        let stored: Settings = serde_json::from_str(r#"{"showUrl": false}"#).unwrap();
        assert!(!stored.show_window_title);
        assert!(stored.show_timestamp, "everything else keeps its default");

        let default: Settings = serde_json::from_str("{}").unwrap();
        assert!(default.show_window_title);
    }

    /// The clipboard settings are the contract between this struct and the Settings page,
    /// which speaks camelCase. A field renamed on one side and not the other would silently
    /// stop saving, so pin the names — and confirm a file written before the feature existed
    /// still loads, with recording on.
    #[test]
    fn the_clipboard_settings_survive_a_round_trip_and_an_older_file() {
        let older: Settings = serde_json::from_str(r#"{"showTimestamp": true}"#).unwrap();
        assert!(older.clipboard_history, "recording is the default");
        assert_eq!(older.clipboard_max_items, 200);
        assert_eq!(older.shortcut_open_clipboard, "Alt+Shift+C");

        let stored: Settings = serde_json::from_str(
            r#"{"clipboardHistory": false, "clipboardMaxItems": 25, "clipboardMaxMb": 7,
                "clipboardIgnoreApps": ["Banking", "  "], "shortcutOpenClipboard": "Alt+V"}"#,
        )
        .unwrap();
        assert!(!stored.clipboard_history);
        assert_eq!(stored.clipboard_max_items, 25);
        assert_eq!(stored.clipboard_max_bytes(), 7 * 1024 * 1024);
        assert_eq!(stored.clipboard_ignore_apps, vec!["Banking", "  "]);
        assert_eq!(stored.shortcut_open_clipboard, "Alt+V");

        // And back out again under the same names the page reads.
        let json = serde_json::to_string(&stored).unwrap();
        for key in [
            "clipboardHistory",
            "clipboardMaxItems",
            "clipboardMaxMb",
            "clipboardIgnoreApps",
            "shortcutOpenClipboard",
        ] {
            assert!(json.contains(key), "{key} must survive serialization");
        }
    }

    /// A silent hotkey is enough to want the worker parked, even with silent capture off as
    /// the default — otherwise the first press pays for creating its window.
    #[test]
    fn a_silent_hotkey_parks_the_worker() {
        assert!(!Settings::default().wants_silent_worker());

        let with_key: Settings =
            serde_json::from_str(r#"{"shortcutCaptureSnipSilent": "Alt+Shift+C"}"#).unwrap();
        assert!(with_key.wants_silent_worker());
        assert!(
            !with_key.silent_capture,
            "the ordinary keys still open the editor"
        );

        // And that key dispatches the same mode, delivered silently.
        let fired = with_key.capture_shortcuts();
        let hit = fired
            .iter()
            .find(|(acc, _, _)| *acc == "Alt+Shift+C")
            .expect("registered");
        assert_eq!((hit.1, hit.2), ("snip", true));
    }

    /// Nothing is read from the browser until a line that needs it is switched on.
    #[test]
    fn browser_details_are_opt_in() {
        let default = Settings::default();
        assert!(!default.wants_browser_details());
        assert!(!default.silent_capture);

        for on in [
            r#"{"showPageTitle": true}"#,
            r#"{"showPageUrl": true}"#,
            r#"{"showBrowserProfile": true}"#,
        ] {
            let s: Settings = serde_json::from_str(on).unwrap();
            assert!(s.wants_browser_details(), "{on}");
        }
    }

    /// Starting the menu-bar tools with macOS is the useful default, but the value remains
    /// an ordinary preference that can be switched off and survives a JSON round-trip.
    #[test]
    fn launch_at_login_defaults_on_and_can_be_saved_off() {
        let default: Settings = serde_json::from_str("{}").unwrap();
        assert!(default.launch_at_login);

        let off: Settings = serde_json::from_str(r#"{"launchAtLogin": false}"#).unwrap();
        assert!(!off.launch_at_login);
        assert!(serde_json::to_string(&off)
            .unwrap()
            .contains(r#""launchAtLogin":false"#));
    }

    #[test]
    fn launch_at_login_preference_survives_a_settings_file_reload() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let settings = Settings {
            launch_at_login: false,
            ..Settings::default()
        };

        settings.save(&path).unwrap();
        assert!(!Settings::load(&path).launch_at_login);
    }
}
