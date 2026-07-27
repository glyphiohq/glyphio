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

    // ---- history ----
    pub history_enabled: bool,
    pub history_max_count: u32,
    pub history_max_bytes: u64,

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
            history_enabled: true,
            history_max_count: 50,
            history_max_bytes: 200 * 1024 * 1024, // 200 MB, matching Checkpoint
            // Alt+Shift+S/V/X mirrors Checkpoint; H opens history.
            shortcut_capture_full: "Alt+Shift+S".into(),
            shortcut_capture_visible: "Alt+Shift+V".into(),
            shortcut_capture_snip: "Alt+Shift+X".into(),
            shortcut_capture_front_window: "Alt+Shift+W".into(), // W = frontmost window
            shortcut_capture_page: "Alt+Shift+B".into(), // B = browser page (content only)
            shortcut_capture_scroll: "Alt+Shift+L".into(), // L = long/scrolling area
            shortcut_capture_scroll_page: "Alt+Shift+P".into(), // P = whole page/window
            shortcut_open_history: "Alt+Shift+H".into(),
            shortcut_open_palette: "Alt+Space".into(),
        }
    }
}

impl Settings {
    /// Whether any banner line needs the browser to be asked about the page. Nothing is read
    /// from the browser unless one of these is on — see `capture::ax::browser_meta`.
    pub fn wants_browser_details(&self) -> bool {
        self.show_page_title || self.show_page_url || self.show_browser_profile
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
}
