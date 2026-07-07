//! Capture orchestration: grab pixels via the native backend, encode PNG, stash the result, and
//! open the editor window. The editor pulls the payload via the `take_pending_capture` command.

mod backend;
pub mod scroll;

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
/// | `frontWindow` (frontmost window, no picker — e.g. just the browser window) | `scrolling`
/// (drag a region) | `scrollingPage` (whole frontmost window, auto-targeted).
pub fn trigger(app: &AppHandle, mode: &str) -> anyhow::Result<()> {
    if mode == "scrolling" {
        return crate::windows::open_scroll_overlay(app);
    }
    if mode == "scrollingPage" {
        // Long-running (scroll + settle per frame) — never block the main thread.
        let (x, y, w, h, title) = backend::frontmost_window_bounds()?;
        let app2 = app.clone();
        tauri::async_runtime::spawn(async move {
            let result = tauri::async_runtime::spawn_blocking(move || {
                scroll::capture(x, y, w, h).map(|mut s| {
                    s.title = title;
                    s
                })
            })
            .await;
            match result {
                Ok(Ok(shot)) => {
                    let app3 = app2.clone();
                    let _ = app2.run_on_main_thread(move || {
                        if let Err(e) = finish(&app3, shot, "scrollingPage") {
                            log::error!("scrolling page capture failed to finish: {e}");
                        }
                    });
                }
                Ok(Err(e)) => log::error!("scrolling page capture failed: {e}"),
                Err(e) => log::error!("scrolling page capture task failed: {e}"),
            }
        });
        return Ok(());
    }
    let shot = backend::capture(app, mode)?;
    finish(app, shot, mode)
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
