//! The operating-system boundary.
//!
//! Everything that talks to the OS (screen capture, accessibility, the pasteboard, synthetic
//! input, permissions, the login item, process control) sits behind a small trait here, and
//! `cfg(target_os)` plus the OS crates live only under this directory. The rest of the app asks
//! [`current`] for the implementation, so the pure cores can be driven with [`fake`] in tests and
//! a new OS is an additive directory (`docs/WINDOWS.md`, `docs/LINUX.md`). Targets without one
//! get [`unsupported`], which compiles everywhere and refuses each call; `linux` reuses it for
//! the capabilities it has not implemented yet.
//!
//! Geometry is in global points with a top-left origin, as before. Methods are synchronous; the
//! capture worker and other threads call them directly, never the main thread's event loop.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use image::RgbaImage;

use crate::autostart::{Migration, Outcome};
use crate::capture::Shot;

pub mod files;
pub mod local_socket;
pub mod reserved;
pub mod types;
pub use types::{BrowserMeta, FrontWindow, PageGeometry, Progress};

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(not(target_os = "macos"))]
pub mod unsupported;

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
    /// Begin capturing `rect` over and over, as a scrolling capture does. A platform that can
    /// settle what to capture once (which displays, which windows to leave out) does so here;
    /// the default asks [`ScreenCapturer::rect_image`] every time.
    fn rect_session(&self, rect: Rect) -> anyhow::Result<Box<dyn RectSession + '_>> {
        Ok(Box::new(EachTime { capturer: self, rect }))
    }
}

/// One rectangle, captured repeatedly. See [`ScreenCapturer::rect_session`].
pub trait RectSession {
    /// The rectangle now, as an image plus its backing scale.
    fn frame(&mut self) -> anyhow::Result<(RgbaImage, f64)>;
}

/// The default session: nothing to reuse, every frame is a fresh [`ScreenCapturer::rect_image`].
struct EachTime<'a, C: ?Sized> {
    capturer: &'a C,
    rect: Rect,
}

impl<C: ScreenCapturer + ?Sized> RectSession for EachTime<'_, C> {
    fn frame(&mut self) -> anyhow::Result<(RgbaImage, f64)> {
        let (x, y, w, h) = self.rect;
        self.capturer.rect_image(x, y, w, h)
    }
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
    /// Scroll position of the container under `point` (global points): the page's web area
    /// when there is one, else the nearest native scroll area. Keeps the browser's tree warm.
    /// `None` when the platform cannot tell; callers then fall back to comparing pixels.
    fn scroll_progress(&self, point: (f64, f64)) -> Option<Progress>;
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
    /// Whether a real capture-content query succeeds. Slower than the preflight and may itself
    /// prompt on a fresh system, so callers use it only after the user was already prompted.
    fn screen_recording_probe(&self) -> bool;
    /// Forget this app's grant for `kind` (user scope, no administrator rights) so the next
    /// request shows a fresh prompt and adds a clean entry.
    fn reset(&self, kind: PermissionKind) -> anyhow::Result<()>;
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

pub trait Installation: Send + Sync {
    /// Whether `path` is on a read-only volume (a mounted disk image, for one).
    fn read_only(&self, path: &Path) -> bool;
    /// Whether this process can create entries in `dir`.
    fn dir_writable(&self, dir: &Path) -> bool;
    /// The bundle identifier of the app at `bundle`.
    fn bundle_identifier(&self, bundle: &Path) -> Option<String>;
    /// Copy an app bundle, preserving its signature, attributes and symlinks.
    fn copy_bundle(&self, from: &Path, to: &Path) -> anyhow::Result<()>;
    /// Launch a new instance of the app at `bundle`.
    fn open_bundle(&self, bundle: &Path) -> anyhow::Result<()>;
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
    pub install: Box<dyn Installation>,
}

static CURRENT: OnceLock<Platform> = OnceLock::new();

/// The platform for this build, constructed on first use.
pub fn current() -> &'static Platform {
    CURRENT.get_or_init(|| {
        #[cfg(target_os = "macos")]
        {
            macos::platform()
        }
        #[cfg(target_os = "linux")]
        {
            linux::platform()
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            unsupported::platform()
        }
    })
}

/// The OS folders [`crate::paths::location`] chooses between.
pub struct Folders {
    /// The folder the user sees the app in: the one holding `Glyphio.app` on macOS, the
    /// AppImage's folder on Linux (not its temporary mount), the executable's folder otherwise.
    /// A portable copy's data folder sits here.
    pub app_dir: Option<PathBuf>,
    /// The OS's log folder for Glyphio, or `None` to keep the log inside the data root.
    pub log_dir: Option<PathBuf>,
}

pub fn folders() -> Folders {
    let exe = std::env::current_exe().ok().and_then(|e| e.canonicalize().ok());
    let exe_dir = exe.as_deref().and_then(Path::parent).map(Path::to_path_buf);
    #[cfg(target_os = "macos")]
    {
        let bundle_dir = exe.as_deref().and_then(crate::install::bundle_of).and_then(|b| b.parent().map(Path::to_path_buf));
        Folders {
            app_dir: bundle_dir.or(exe_dir),
            log_dir: dirs::home_dir().map(|h| h.join("Library/Logs/Glyphio")),
        }
    }
    #[cfg(target_os = "linux")]
    {
        Folders { app_dir: linux::app_dir().or(exe_dir), log_dir: dirs::state_dir().map(|d| d.join("Glyphio")) }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = exe;
        Folders { app_dir: exe_dir, log_dir: None }
    }
}

/// Whether `event` is the app being opened again while it runs (macOS: Finder, Launchpad or
/// `open -a`), which surfaces Settings the way a Dock click would.
pub fn is_reopen(event: &tauri::RunEvent) -> bool {
    #[cfg(target_os = "macos")]
    {
        matches!(event, tauri::RunEvent::Reopen { .. })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = event;
        false
    }
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
