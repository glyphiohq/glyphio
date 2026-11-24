//! The platform for targets without an implementation yet (Windows, until `docs/WINDOWS.md` is
//! built). It compiles everywhere, so the rest of the app type-checks for those targets, and
//! every capability answers that it is not supported rather than pretending to work.

use std::path::{Path, PathBuf};
use std::time::Duration;

use image::RgbaImage;

use super::{
    AppShell, Autostart, BrowserMeta, DateStyle, DisplayBounds, LocaleDates, FrontWindow, InputInjector, Installation, PageGeometry,
    PageInspector, Pasteboard, PermissionKind, Permissions, PickKind, Platform, ProcessControl, Progress,
    RegionPicker, ScreenCapturer, WindowQuery,
};
use crate::autostart::{self, LoginItem, Migration, Outcome, Status};
use crate::capture::Shot;

/// The error every unsupported capability returns.
pub fn unsupported(what: &str) -> anyhow::Error {
    anyhow::anyhow!("{what} is not supported on this operating system yet")
}

pub fn platform() -> Platform {
    Platform {
        capturer: Box::new(Unsupported),
        picker: Box::new(Unsupported),
        windows: Box::new(Unsupported),
        inspector: Box::new(Unsupported),
        input: Box::new(Unsupported),
        pasteboard: Box::new(Unsupported),
        permissions: Box::new(Unsupported),
        autostart: Box::new(Unsupported),
        process: Box::new(Unsupported),
        shell: Box::new(Unsupported),
        install: Box::new(Unsupported),
        dates: Box::new(Unsupported),
    }
}

pub struct Unsupported;

impl ScreenCapturer for Unsupported {
    fn visible(&self) -> anyhow::Result<Shot> {
        Err(unsupported("Screen capture"))
    }
    fn front_window(&self, _: bool) -> anyhow::Result<Shot> {
        Err(unsupported("Window capture"))
    }
    fn rect_image(&self, _: f64, _: f64, _: f64, _: f64) -> anyhow::Result<(RgbaImage, f64)> {
        Err(unsupported("Region capture"))
    }
}

impl RegionPicker for Unsupported {
    fn pick(&self, _: PickKind) -> anyhow::Result<Shot> {
        Err(unsupported("Picking a region"))
    }
}

impl WindowQuery for Unsupported {
    fn frontmost_window(&self) -> anyhow::Result<FrontWindow> {
        Err(unsupported("Finding the front window"))
    }
    fn frontmost_app(&self) -> Option<(i32, String)> {
        None
    }
    fn focused_window_display(&self) -> Option<DisplayBounds> {
        None
    }
    fn display_bounds_under_cursor(&self) -> DisplayBounds {
        ((0.0, 0.0), (0.0, 0.0))
    }
    fn display_bounds_containing_point(&self, _: f64, _: f64) -> Option<DisplayBounds> {
        None
    }
    fn foreground_app_name(&self) -> String {
        String::new()
    }
    fn session_locked(&self) -> bool {
        false
    }
}

impl PageInspector for Unsupported {
    fn page_geometry(&self, _: i32, _: Duration) -> Option<PageGeometry> {
        None
    }
    fn browser_meta(&self, _: i32, _: &str) -> BrowserMeta {
        BrowserMeta::default()
    }
    fn scroll_progress(&self, _: (f64, f64)) -> Option<Progress> {
        None
    }
    fn release(&self) {}
}

impl InputInjector for Unsupported {
    fn post_scroll(&self, _: i32) -> anyhow::Result<()> {
        Err(unsupported("Scrolling another app"))
    }
    fn cursor_position(&self) -> Option<(f64, f64)> {
        None
    }
    fn warp_cursor(&self, _: (f64, f64)) {}
    fn send_paste(&self) -> anyhow::Result<()> {
        Err(unsupported("Pasting into another app"))
    }
    fn can_send_paste(&self) -> bool {
        false
    }
}

impl Pasteboard for Unsupported {
    fn change_counter(&self) -> i64 {
        0
    }
    fn is_concealed(&self) -> bool {
        false
    }
    fn write_png(&self, _: &[u8]) -> anyhow::Result<()> {
        Err(unsupported("Copying an image"))
    }
}

impl LocaleDates for Unsupported {
    fn format(&self, _: i64, _: DateStyle, _: DateStyle, _: Option<&str>, _: Option<&str>) -> Option<String> {
        None
    }
}

impl Permissions for Unsupported {
    fn accessibility_trusted(&self) -> bool {
        false
    }
    fn request_accessibility(&self) -> bool {
        false
    }
    fn screen_recording_granted(&self) -> bool {
        false
    }
    fn request_screen_recording(&self) -> bool {
        false
    }
    fn screen_recording_probe(&self) -> bool {
        false
    }
    fn reset(&self, _: PermissionKind) -> anyhow::Result<()> {
        Err(unsupported("Resetting permissions"))
    }
    fn settings_url(&self, _: PermissionKind) -> Option<&'static str> {
        None
    }
}

/// A login item that is never registered and refuses to be.
struct NoLoginItem;

impl LoginItem for NoLoginItem {
    fn status(&self) -> Status {
        Status::NotRegistered
    }
    fn register(&mut self) -> Result<(), String> {
        Err(unsupported("Opening at login").to_string())
    }
    fn unregister(&mut self) -> Result<(), String> {
        Ok(())
    }
}

impl Autostart for Unsupported {
    fn set_enabled(&self, want: bool) -> Outcome {
        autostart::apply_setting(&mut NoLoginItem, want)
    }
    fn reconcile(&self, want: bool) -> Outcome {
        autostart::reconcile_at_launch(&mut NoLoginItem, want)
    }
    fn migrate_legacy(&self, _: &Path, _: bool) -> Migration {
        // There was never a scripted item to remove here.
        autostart::migrate_scripted_item(true, || Ok(()))
    }
    fn launch_property(&self) -> Option<u32> {
        None
    }
}

impl AppShell for Unsupported {
    fn init_agent(&self, _: &tauri::AppHandle) {}
    fn set_regular(&self, _: &tauri::AppHandle, _: bool) {}
    fn hide_app(&self, _: &tauri::AppHandle) {}
    fn follow_user_across_spaces(&self, _: &tauri::WebviewWindow) {}
}

impl ProcessControl for Unsupported {
    fn exe_path(&self, _: u32) -> Option<PathBuf> {
        None
    }
    fn kill_tree(&self, _: u32) {}
    fn daemon_lock_free(&self, _: &Path) -> bool {
        true
    }
    fn bundled_engine_exe(&self) -> Option<PathBuf> {
        None
    }
}

impl Installation for Unsupported {
    fn read_only(&self, _: &Path) -> bool {
        false
    }
    fn dir_writable(&self, _: &Path) -> bool {
        false
    }
    fn bundle_identifier(&self, _: &Path) -> Option<String> {
        None
    }
    fn copy_bundle(&self, _: &Path, _: &Path) -> anyhow::Result<()> {
        Err(unsupported("Moving the app"))
    }
    fn open_bundle(&self, _: &Path) -> anyhow::Result<()> {
        Err(unsupported("Opening another copy of the app"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_capability_reports_unsupported_instead_of_pretending() {
        let p = platform();
        let err = p.capturer.visible().err().unwrap().to_string();
        assert!(err.contains("not supported"), "{err}");
        assert!(p.picker.pick(PickKind::Region).is_err());
        assert!(p.capturer.rect_session((0.0, 0.0, 1.0, 1.0)).unwrap().frame().is_err());
        assert!(p.windows.frontmost_window().is_err());
        assert!(p.inspector.scroll_progress((0.0, 0.0)).is_none());
        assert!(p.input.send_paste().is_err() && !p.input.can_send_paste());
        assert!(!p.permissions.accessibility_trusted());
        assert!(matches!(p.autostart.set_enabled(true), Outcome::Failed(_)));
        assert!(p.process.bundled_engine_exe().is_none());
        assert!(p.install.copy_bundle(Path::new("a"), Path::new("b")).is_err());
        assert_eq!(p.pasteboard.change_counter(), 0);
    }
}
