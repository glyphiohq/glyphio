//! User settings. Replaces Checkpoint's `chrome.storage.sync` with a JSON file on disk.
//! Keys mirror Checkpoint's `userSettingsShape` (camelCase) so the ported editor/history JS
//! consumes them unchanged. `showUrl` now toggles the window/app title (no browser URL natively).

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
    pub show_url: bool,           // shows window/app title natively
    pub banner_bg: String,
    pub banner_fg: String,
    pub banner_muted: String,

    // ---- downloads / workflow ----
    pub download_subdir: String,
    pub filename_prefix: String,
    pub auto_copy_on_open: bool,

    // ---- history ----
    pub history_enabled: bool,
    pub history_max_count: u32,
    pub history_max_bytes: u64,

    // ---- global capture hotkeys (Tauri accelerator syntax) ----
    pub shortcut_capture_visible: String,
    pub shortcut_capture_snip: String,
    pub shortcut_capture_full: String,
    pub shortcut_capture_front_window: String,
    pub shortcut_capture_scroll: String,
    pub shortcut_capture_scroll_page: String,
    pub shortcut_open_history: String,
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
            show_url: true,
            banner_bg: "#1f2937".into(),
            banner_fg: "#ffffff".into(),
            banner_muted: "#cbd5e1".into(),
            download_subdir: "Glyphio".into(),
            filename_prefix: "glyphio".into(),
            auto_copy_on_open: true,
            history_enabled: true,
            history_max_count: 50,
            history_max_bytes: 200 * 1024 * 1024, // 200 MB, matching Checkpoint
            // Alt+Shift+S/V/X mirrors Checkpoint; H opens history.
            shortcut_capture_full: "Alt+Shift+S".into(),
            shortcut_capture_visible: "Alt+Shift+V".into(),
            shortcut_capture_snip: "Alt+Shift+X".into(),
            shortcut_capture_front_window: "Alt+Shift+W".into(), // W = frontmost window
            shortcut_capture_scroll: "Alt+Shift+L".into(), // L = long/scrolling area
            shortcut_capture_scroll_page: "Alt+Shift+P".into(), // P = whole page/window
            shortcut_open_history: "Alt+Shift+H".into(),
        }
    }
}

impl Settings {
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
