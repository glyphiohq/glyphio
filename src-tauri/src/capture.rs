//! Capture orchestration: grab pixels via the native backend, encode PNG, stash the result, and
//! open the editor window. The editor pulls the payload via the `take_pending_capture` command.

mod ax;
mod backend;
pub mod diag;
pub mod scroll;

use anyhow::anyhow;
use base64::Engine;
use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::AppState;

/// (origin, size) in points of the display under the cursor — used to place the
/// scrolling-capture selection overlay.
pub fn display_bounds_under_cursor() -> ((f64, f64), (f64, f64)) {
    backend::display_bounds_under_cursor()
}

/// A captured, un-annotated frame handed to the editor webview.
pub struct Shot {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Backing-scale factor (2.0 on Retina) so the banner/editor can size text correctly.
    pub dpr: f64,
    /// Window/app title (banner "url" field natively) — empty when capturing a whole display.
    pub title: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingCapture {
    pub png_data_url: String,
    pub width: u32,
    pub height: u32,
    pub dpr: f64,
    pub mode: String,
    pub title: String,
    pub captured_at: String,
}

/// Entry point for a capture. `mode` is `visible` | `snip` | `fullWindow` (interactive picker)
/// | `frontWindow` (frontmost window, no picker — e.g. just the browser window) | `pageOnly`
/// (just the web content of the frontmost browser window, chrome excluded) | `scrolling`
/// (drag a region) | `scrollingPage` (whole frontmost page, auto-targeted).
pub fn trigger(app: &AppHandle, mode: &str) -> anyhow::Result<()> {
    if mode == "scrolling" {
        return crate::windows::open_scroll_overlay(app);
    }
    if mode == "pageOnly" || mode == "scrollingPage" {
        // Both walk the AX tree (possible Chromium opt-in retry) and, for scrollingPage,
        // scroll + settle per frame — never block the main thread.
        let mode = mode.to_string();
        let app2 = app.clone();
        tauri::async_runtime::spawn(async move {
            let m = mode.clone();
            let result = tauri::async_runtime::spawn_blocking(move || capture_page(&m)).await;
            match result {
                Ok(Ok(shot)) => {
                    let app3 = app2.clone();
                    let mode2 = mode.clone();
                    let _ = app2.run_on_main_thread(move || {
                        if let Err(e) = finish(&app3, shot, &mode2) {
                            report_failure(&app3, "page capture", &e);
                        }
                    });
                }
                Ok(Err(e)) => report_failure(&app2, "page capture", &e),
                Err(e) => log::error!("page capture task failed: {e}"),
            }
        });
        return Ok(());
    }
    let shot = backend::capture(app, mode)?;
    finish(app, shot, mode)
}

/// Blocking body of the two frontmost-page modes. `pageOnly` requires a visible web area
/// (that's the point of the mode); `scrollingPage` prefers it — no browser chrome repeating
/// in the stitched frames — and falls back to the window frame for non-browsers.
///
/// Geometry comes from the AX tree when possible: CGWindowList ordering is unreliable on
/// modern macOS (Safari's toolbar strip is its own window), and `AXWebArea` alone reports
/// the full document extent, so `ax::page_geometry` intersects it down to the viewport.
fn capture_page(mode: &str) -> anyhow::Result<Shot> {
    let win = backend::frontmost_window_bounds()?;
    let geometry = if scroll::app_accessibility_trusted() {
        ax::page_geometry(win.pid)
    } else {
        None
    };
    if mode == "pageOnly" {
        if !scroll::app_accessibility_trusted() {
            anyhow::bail!(
                "Browser page capture needs Accessibility permission for Glyphio \
                 (System Settings › Privacy & Security › Accessibility) — it reads the \
                 page's position from the browser."
            );
        }
        let (x, y, w, h) = geometry.and_then(|g| g.web_visible).ok_or_else(|| {
            anyhow!(
                "No web page found in the frontmost window — Browser Page capture works \
                 when a browser (Safari, Chrome, Edge, Arc…) is in front."
            )
        })?;
        let (img, dpr) = backend::capture_rect_image(x, y, w, h)?;
        let (width, height) = img.dimensions();
        return Ok(Shot { rgba: img.into_raw(), width, height, dpr, title: win.title });
    }
    // scrollingPage — the Accessibility error, if the grant is missing, comes from
    // scroll::capture itself. Fallbacks: viewport → AX window frame → CGWindow rect
    // (title bar already inset away by frontmost_window_bounds).
    let (x, y, w, h) = match &geometry {
        Some(g) => g.web_visible.unwrap_or(g.window),
        None => (win.x, win.y, win.w, win.h),
    };
    scroll::capture(x, y, w, h).map(|mut s| {
        s.title = win.title;
        s
    })
}

/// Log a capture failure AND put it in front of the user — captures fire from global
/// shortcuts and the tray, where a silent log line reads as "nothing happened".
pub fn report_failure(app: &AppHandle, context: &str, e: &anyhow::Error) {
    use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
    log::error!("{context} failed: {e}");
    app.dialog()
        .message(format!("{e:#}"))
        .title("Glyphio — capture failed")
        .kind(MessageDialogKind::Error)
        .show(|_| {});
}

/// [`trigger`] + user-visible error reporting; the fire-and-forget entry point used by the
/// tray menu and global shortcuts.
pub fn trigger_or_report(app: &AppHandle, mode: &str) {
    if let Err(e) = trigger(app, mode) {
        report_failure(app, &format!("capture ({mode})"), &e);
    }
}

/// Shared tail of every capture path: encode, stash for the editor, open it.
pub fn finish(app: &AppHandle, shot: Shot, mode: &str) -> anyhow::Result<()> {
    let png = encode_png(&shot)?;
    let data_url = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&png)
    );
    let pending = PendingCapture {
        png_data_url: data_url,
        width: shot.width,
        height: shot.height,
        dpr: shot.dpr,
        mode: mode.to_string(),
        title: shot.title,
        captured_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    };
    *app.state::<AppState>().pending_capture.lock().unwrap() = Some(pending);
    crate::windows::open(app, "editor")?;
    Ok(())
}

fn encode_png(shot: &Shot) -> anyhow::Result<Vec<u8>> {
    let buf = image::RgbaImage::from_raw(shot.width, shot.height, shot.rgba.clone())
        .ok_or_else(|| anyhow::anyhow!("capture buffer size mismatch"))?;
    let mut out = std::io::Cursor::new(Vec::new());
    buf.write_to(&mut out, image::ImageFormat::Png)?;
    Ok(out.into_inner())
}
