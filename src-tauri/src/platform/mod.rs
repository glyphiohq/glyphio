//! The operating-system boundary.
//!
//! Everything that talks to the OS (screen capture, accessibility, the pasteboard, synthetic
//! input, permissions, the login item, process control) sits behind a small trait here, and
//! `cfg(target_os)` plus the OS crates live only under this directory. The rest of the app asks
//! [`current`] for the implementation, so the pure cores can be driven with [`fake`] in tests and
//! a Windows port is an additive directory (`docs/WINDOWS.md`).
//!
//! Geometry is in global points with a top-left origin, as before. Methods are synchronous; the
//! capture worker and other threads call them directly, never the main thread's event loop.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use image::RgbaImage;

use crate::autostart::{Migration, Outcome};
use crate::capture::Shot;

pub mod types;
pub use types::{BrowserMeta, FrontWindow, PageGeometry};

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(test)]
pub mod fake;

/// `(x, y, w, h)` in global points.
pub type Rect = (f64, f64, f64, f64);
/// `((x, y), (w, h))` of a display in global points.
pub type DisplayBounds = ((f64, f64), (f64, f64));

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickKind {
    Region,
    Window,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionKind {
    Accessibility,
    ScreenRecording,
}

pub trait ScreenCapturer: Send + Sync {
    /// The display under the cursor.
    fn visible(&self) -> anyhow::Result<Shot>;
    /// The frontmost window not owned by Glyphio.
    fn front_window(&self, with_browser_details: bool) -> anyhow::Result<Shot>;
    /// A rectangle, as an image plus its backing scale.
    fn rect_image(&self, x: f64, y: f64, w: f64, h: f64) -> anyhow::Result<(RgbaImage, f64)>;
}

pub trait RegionPicker: Send + Sync {
    /// Blocks until the user picks or cancels (cancel is an error).
    fn pick(&self, kind: PickKind) -> anyhow::Result<Shot>;
}

pub trait WindowQuery: Send + Sync {
    fn frontmost_window(&self) -> anyhow::Result<FrontWindow>;
    fn frontmost_app(&self) -> Option<(i32, String)>;
    fn focused_window_display(&self) -> Option<DisplayBounds>;
    fn display_bounds_under_cursor(&self) -> DisplayBounds;
    fn display_bounds_containing_point(&self, x: f64, y: f64) -> Option<DisplayBounds>;
    /// Name of the application that currently has focus (empty if unknown).
    fn foreground_app_name(&self) -> String;
}

pub trait PageInspector: Send + Sync {
    fn page_geometry(&self, pid: i32, budget: Duration) -> Option<PageGeometry>;
    fn browser_meta(&self, pid: i32, app_name: &str) -> BrowserMeta;
    /// Undo any accessibility opt-in still outstanding on a browser.
    fn release(&self);
}

pub trait InputInjector: Send + Sync {
    fn post_scroll(&self, delta: i32) -> anyhow::Result<()>;
    fn cursor_position(&self) -> Option<(f64, f64)>;
    fn warp_cursor(&self, p: (f64, f64));
    fn send_paste(&self) -> anyhow::Result<()>;
    fn can_send_paste(&self) -> bool;
}

pub trait Pasteboard: Send + Sync {
    fn change_counter(&self) -> i64;
    fn is_concealed(&self) -> bool;
}

pub trait Permissions: Send + Sync {
    fn accessibility_trusted(&self) -> bool;
    fn request_accessibility(&self) -> bool;
    fn screen_recording_granted(&self) -> bool;
    fn request_screen_recording(&self) -> bool;
    /// A URL that opens the OS settings page for `kind`, if the OS has one.
    fn settings_url(&self, kind: PermissionKind) -> Option<&'static str>;
}

pub trait Autostart: Send + Sync {
    fn set_enabled(&self, want: bool) -> Outcome;
    fn reconcile(&self, want: bool) -> Outcome;
    fn migrate_legacy(&self, marker: &Path, existing_install: bool) -> Migration;
    /// The launch property of the event that started this process (macOS: the Apple event's
    /// property data), for recognising a login launch.
    fn launch_property(&self) -> Option<u32>;
}

pub trait AppShell: Send + Sync {
    /// Start as a menu-bar agent. Called once at launch.
    fn init_agent(&self, app: &tauri::AppHandle);
    /// Show (`true`) or hide (`false`) the Dock icon; applied only on a change.
    fn set_regular(&self, app: &tauri::AppHandle, regular: bool);
    /// Deactivate the whole app so focus returns to the app the user came from.
    fn hide_app(&self, app: &tauri::AppHandle);
    fn follow_user_across_spaces(&self, win: &tauri::WebviewWindow);
}

pub trait ProcessControl: Send + Sync {
    /// Executable path of a live process, or `None` if the pid is not running.
    fn exe_path(&self, pid: u32) -> Option<PathBuf>;
    /// Terminate the process and its children.
    fn kill_tree(&self, pid: u32);
    /// Whether the engine daemon's lock under `runtime` is free (no daemon holds it).
    fn daemon_lock_free(&self, runtime: &Path) -> bool;
    /// This bundle's engine executable.
    fn bundled_engine_exe(&self) -> Option<PathBuf>;
}

/// One implementation of each capability, built once.
pub struct Platform {
    pub capturer: Box<dyn ScreenCapturer>,
    pub picker: Box<dyn RegionPicker>,
    pub windows: Box<dyn WindowQuery>,
    pub inspector: Box<dyn PageInspector>,
    pub input: Box<dyn InputInjector>,
    pub pasteboard: Box<dyn Pasteboard>,
    pub permissions: Box<dyn Permissions>,
    pub autostart: Box<dyn Autostart>,
    pub process: Box<dyn ProcessControl>,
    pub shell: Box<dyn AppShell>,
}

static CURRENT: OnceLock<Platform> = OnceLock::new();

/// The platform for this build, constructed on first use.
pub fn current() -> &'static Platform {
    CURRENT.get_or_init(|| {
        #[cfg(target_os = "macos")]
        {
            macos::platform()
        }
        #[cfg(all(not(target_os = "macos"), not(test)))]
        {
            compile_error!("no platform implementation for this target yet; see docs/WINDOWS.md")
        }
    })
}

/// System-wide managed configuration deployed by IT/MDM (root-owned).
pub fn managed_config_path() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "/Library/Application Support/Glyphio/managed.toml"
    }
    #[cfg(not(target_os = "macos"))]
    {
        "/etc/glyphio/managed.toml"
    }
}
