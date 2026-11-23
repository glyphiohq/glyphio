//! macOS implementation of the platform traits: ScreenCaptureKit, the Accessibility API,
//! Quartz events, `NSPasteboard`, `SMAppService`, and libproc.

use std::path::Path;
use std::time::Duration;

use image::RgbaImage;

use super::{
    AppShell, Autostart, BrowserMeta, DisplayBounds, FrontWindow, InputInjector, PageGeometry, PageInspector, Progress,
    Pasteboard, PermissionKind, Permissions, PickKind, Platform, Rect, RectSession, RegionPicker,
    ScreenCapturer, WindowQuery,
};
use crate::autostart::{Migration, Outcome};
use crate::capture::Shot;

/// `keyAEPropData` (`prdt`): where a launch event carries why the app was launched.
const KEY_AE_PROP_DATA: u32 = u32::from_be_bytes(*b"prdt");

mod autostart;
mod capture;
mod inspect;
mod input;
mod install;
mod pasteboard;
mod permissions;
mod process;
mod shell;

pub fn platform() -> Platform {
    Platform {
        capturer: Box::new(Macos),
        picker: Box::new(Macos),
        windows: Box::new(Macos),
        inspector: Box::new(Macos),
        input: Box::new(Macos),
        pasteboard: Box::new(Macos),
        permissions: Box::new(Macos),
        autostart: Box::new(Macos),
        process: Box::new(process::Os),
        shell: Box::new(Macos),
        install: Box::new(install::Os),
    }
}

struct Macos;

impl ScreenCapturer for Macos {
    fn visible(&self) -> anyhow::Result<Shot> {
        capture::capture_display_under_cursor()
    }
    fn front_window(&self, with_browser_details: bool) -> anyhow::Result<Shot> {
        capture::capture_front_window(with_browser_details)
    }
    fn rect_image(&self, x: f64, y: f64, w: f64, h: f64) -> anyhow::Result<(RgbaImage, f64)> {
        capture::capture_rect_image(x, y, w, h)
    }
    fn rect_session(&self, rect: Rect) -> anyhow::Result<Box<dyn RectSession + '_>> {
        Ok(Box::new(capture::RectPlan::new(rect.0, rect.1, rect.2, rect.3)?))
    }
}

impl RegionPicker for Macos {
    fn pick(&self, kind: PickKind) -> anyhow::Result<Shot> {
        match kind {
            PickKind::Window => capture::screencapture_interactive(&["-i", "-W"]),
            PickKind::Region => capture::screencapture_interactive(&["-i"]),
        }
    }
}

impl WindowQuery for Macos {
    fn frontmost_window(&self) -> anyhow::Result<FrontWindow> {
        capture::frontmost_window_bounds()
    }
    fn frontmost_app(&self) -> Option<(i32, String)> {
        capture::frontmost_app()
    }
    fn focused_window_display(&self) -> Option<DisplayBounds> {
        capture::focused_window_display()
    }
    fn display_bounds_under_cursor(&self) -> DisplayBounds {
        capture::display_bounds_under_cursor()
    }
    fn display_bounds_containing_point(&self, x: f64, y: f64) -> Option<DisplayBounds> {
        capture::display_bounds_containing_point(x, y)
    }
    fn foreground_app_name(&self) -> String {
        pasteboard::foreground_app()
    }
}

impl PageInspector for Macos {
    fn page_geometry(&self, pid: i32, budget: Duration) -> Option<PageGeometry> {
        inspect::page_geometry(pid, budget)
    }
    fn browser_meta(&self, pid: i32, app_name: &str) -> BrowserMeta {
        inspect::browser_meta(pid, app_name)
    }
    fn scroll_progress(&self, point: (f64, f64)) -> Option<Progress> {
        inspect::scroll_progress(point)
    }
    fn release(&self) {
        inspect::restore_web_accessibility()
    }
}

impl InputInjector for Macos {
    fn post_scroll(&self, delta: i32) -> anyhow::Result<()> {
        input::post_scroll(delta)
    }
    fn cursor_position(&self) -> Option<(f64, f64)> {
        input::cursor_position()
    }
    fn warp_cursor(&self, p: (f64, f64)) {
        input::warp_cursor(p)
    }
    fn send_paste(&self) -> anyhow::Result<()> {
        pasteboard::send_paste()
    }
    fn can_send_paste(&self) -> bool {
        pasteboard::can_send_paste()
    }
}

impl Pasteboard for Macos {
    fn change_counter(&self) -> i64 {
        pasteboard::change_counter()
    }
    fn is_concealed(&self) -> bool {
        pasteboard::is_concealed()
    }
}

impl Permissions for Macos {
    fn accessibility_trusted(&self) -> bool {
        permissions::accessibility_trusted()
    }
    fn request_accessibility(&self) -> bool {
        permissions::request_accessibility()
    }
    fn screen_recording_granted(&self) -> bool {
        permissions::screen_recording_granted()
    }
    fn request_screen_recording(&self) -> bool {
        permissions::request_screen_recording()
    }
    fn screen_recording_probe(&self) -> bool {
        permissions::screen_recording_probe()
    }
    fn reset(&self, kind: PermissionKind) -> anyhow::Result<()> {
        permissions::reset(kind)
    }
    fn settings_url(&self, kind: PermissionKind) -> Option<&'static str> {
        Some(match kind {
            PermissionKind::Accessibility => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
            }
            PermissionKind::ScreenRecording => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
            }
        })
    }
}

impl Autostart for Macos {
    fn set_enabled(&self, want: bool) -> Outcome {
        autostart::set_enabled(want)
    }
    fn reconcile(&self, want: bool) -> Outcome {
        autostart::reconcile(want)
    }
    fn migrate_legacy(&self, marker: &Path, existing_install: bool) -> Migration {
        autostart::migrate_legacy(marker, existing_install)
    }
    fn launch_property(&self) -> Option<u32> {
        use objc2_foundation::NSAppleEventManager;
        NSAppleEventManager::sharedAppleEventManager()
            .currentAppleEvent()
            .and_then(|event| event.paramDescriptorForKeyword(KEY_AE_PROP_DATA))
            .map(|descriptor| descriptor.enumCodeValue())
    }
}


impl AppShell for Macos {
    fn init_agent(&self, app: &tauri::AppHandle) {
        shell::init_agent(app)
    }
    fn set_regular(&self, app: &tauri::AppHandle, regular: bool) {
        shell::set_regular(app, regular)
    }
    fn hide_app(&self, app: &tauri::AppHandle) {
        shell::hide_app(app)
    }
    fn follow_user_across_spaces(&self, win: &tauri::WebviewWindow) {
        shell::follow_user_across_spaces(win)
    }
}
