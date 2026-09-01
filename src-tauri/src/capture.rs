//! Capture orchestration: grab pixels via the native backend, encode PNG, stash the result, and
//! open the editor window. The editor pulls the payload via the `take_pending_capture` command.
//!
//! # Where the platform line falls
//!
//! Everything in *this* file is portable: choosing a mode, deciding delivery (editor vs
//! clipboard), encoding, history, and the banner contract. The three modules below are not,
//! and a Windows port replaces them rather than editing them:
//!
//! | module     | macOS today                              | Windows would need          |
//! |------------|------------------------------------------|-----------------------------|
//! | `backend`  | ScreenCaptureKit + `/usr/sbin/screencapture` | Windows.Graphics.Capture |
//! | `ax`       | Accessibility API (`AXDocument` for URLs) | UI Automation               |
//! | `scroll`   | CGEvent scroll injection                 | `SendInput`                 |
//!
//! `scroll` is only half platform-bound — the frame stitching is pure image work and ports
//! as-is; only `post_scroll` and `warp_cursor` are Quartz. See `docs/WINDOWS.md` for the
//! full port plan, including the one piece with no Windows equivalent (macOS gives us its
//! interactive window picker for free; Windows has nothing like it).

mod ax;
mod backend;
pub mod diag;
pub mod scroll;

use anyhow::anyhow;
use base64::Engine;
use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::AppState;

/// Undo any accessibility opt-in still outstanding on a browser we captured from — called on
/// app exit, since the cooldown thread that normally does it dies with us.
pub fn restore_browser_accessibility() {
    ax::restore_web_accessibility();
}

/// (origin, size) in points of the display under the cursor — used to place the
/// scrolling-capture selection overlay.
pub fn display_bounds_under_cursor() -> ((f64, f64), (f64, f64)) {
    backend::display_bounds_under_cursor()
}

/// (origin, size) in points of the display hosting the frontmost window — where the user
/// is typing. Expansion-summoned surfaces (popup/form/palette) belong there; the cursor
/// can be on a different display entirely, so it's only the fallback.
pub fn display_bounds_for_active_window() -> ((f64, f64), (f64, f64)) {
    backend::focused_window_display().unwrap_or_else(backend::display_bounds_under_cursor)
}

/// Whether a point falls on some connected display. Used to reject a remembered window
/// position belonging to a monitor that isn't attached any more — restoring it would put the
/// window somewhere the user can't reach.
pub fn point_on_a_display(x: f64, y: f64) -> bool {
    backend::display_bounds_containing_point(x, y).is_some()
}

/// (origin, size) in points of the display holding a global point — where a panel anchored to
/// something on screen has to stay inside.
pub fn display_bounds_containing(x: f64, y: f64) -> Option<((f64, f64), (f64, f64))> {
    backend::display_bounds_containing_point(x, y)
}

/// A captured, un-annotated frame handed to the editor webview.
pub struct Shot {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Backing-scale factor (2.0 on Retina) so the banner/editor can size text correctly.
    pub dpr: f64,
    /// Window/app title — empty when capturing a whole display or a picked region.
    pub title: String,
    /// What the browser said about the page, for the modes that target one window. Empty
    /// unless the user has asked for one of those banner lines.
    pub browser: ax::BrowserMeta,
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
    pub page_title: String,
    pub page_url: String,
    pub profile: String,
    pub captured_at: String,
    /// Straight to the clipboard and history, no editor — the window running this page is
    /// invisible and closes itself when it's done.
    pub silent: bool,
}

/// Whether any banner line needs the browser asked about the page.
pub(crate) fn wants_browser_details(app: &AppHandle) -> bool {
    app.state::<AppState>().settings.lock().unwrap().wants_browser_details()
}

/// Where a capture goes once it has been taken.
///
/// Every way of starting a capture can say which it wants, so "take this one straight to the
/// clipboard" is a thing you do, not only a mode you switch the app into: the tray's second
/// capture menu, ⌘↩ in the palette, and a hotkey of its own per mode. `None` at a call site
/// means "however the user has it configured".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Delivery {
    /// Open the editor, where the capture can be annotated before it goes anywhere.
    Editor,
    /// Straight to the clipboard and history, no window.
    Silent,
}

impl Delivery {
    /// What was asked for, or the configured default. Public for the scrolling overlay,
    /// whose capture is finished by a command rather than by [`trigger`].
    pub fn resolve_for(app: &AppHandle, asked: Option<Delivery>) -> Delivery {
        Self::resolve(app, asked)
    }

    fn resolve(app: &AppHandle, asked: Option<Delivery>) -> Delivery {
        asked.unwrap_or_else(|| {
            if app.state::<AppState>().settings.lock().unwrap().silent_capture {
                Delivery::Silent
            } else {
                Delivery::Editor
            }
        })
    }

    pub fn is_silent(self) -> bool {
        self == Delivery::Silent
    }
}

/// Entry point for a capture. `mode` is `visible` | `snip` | `fullWindow` (interactive picker)
/// | `frontWindow` (frontmost window, no picker — e.g. just the browser window) | `pageOnly`
/// (just the web content of the frontmost browser window, chrome excluded) | `scrolling`
/// (drag a region) | `scrollingPage` (whole frontmost page, auto-targeted).
///
/// `delivery` is where the result should go; `None` follows the user's setting. It is settled
/// here, once, so a capture that takes a while can't change its mind halfway through.
pub fn trigger(app: &AppHandle, mode: &str, delivery: Option<Delivery>) -> anyhow::Result<()> {
    let delivery = Delivery::resolve(app, delivery);
    if mode == "scrolling" {
        // The region is dragged first; the overlay carries the decision back with the rect.
        return crate::windows::open_scroll_overlay(app, delivery);
    }
    if mode == "pageOnly" || mode == "scrollingPage" {
        // Both walk the AX tree (possible Chromium opt-in retry) and, for scrollingPage,
        // scroll + settle per frame — never block the main thread.
        let mode = mode.to_string();
        let app2 = app.clone();
        let details = wants_browser_details(app);
        tauri::async_runtime::spawn(async move {
            let m = mode.clone();
            let target = tauri::async_runtime::spawn_blocking(move || page_target(&m, details)).await;
            let result = match target {
            // scrollingPage runs off the main thread so its capture loop never blocks the app.
                Ok(Ok(PageTarget::Scroll { rect, title, browser })) => {
                    run_scrolling(&app2, rect).await.map(|mut shot| {
                        shot.title = title;
                        shot.browser = browser;
                        shot
                    })
                }
                Ok(Ok(PageTarget::Done(shot))) => Ok(shot),
                Ok(Err(e)) => Err(e),
                Err(e) => Err(anyhow!("page capture task failed: {e}")),
            };
            match result {
                Ok(shot) => {
                    let app3 = app2.clone();
                    let mode2 = mode.clone();
                    let _ = app2.run_on_main_thread(move || {
                        if let Err(e) = finish(&app3, shot, &mode2, delivery) {
                            report_failure(&app3, "page capture", &e);
                        }
                    });
                }
                Err(e) => report_failure(&app2, "page capture", &e),
            }
        });
        return Ok(());
    }
    let shot = backend::capture(app, mode)?;
    finish(app, shot, mode, delivery)
}

/// How long to wait for a browser that builds its accessibility tree on demand.
///
/// Measured cold on an idle Mac (Chrome 3 windows, full screen): 2.0–2.2s from the opt-in to
/// the web area appearing. The previous 3s left ~0.8s of headroom, which a loaded machine
/// eats — and the failure is a hard error telling the user their page isn't a page. The wait
/// is only ever paid on the first capture per browser per grace period, and only while the
/// tree is genuinely being built (see `ax::page_geometry`), so the ceiling is cheap to raise
/// and expensive to keep tight.
const PAGE_TREE_BUDGET: std::time::Duration = std::time::Duration::from_millis(8000);

/// Outcome of resolving what a frontmost-page capture is aimed at.
enum PageTarget {
    /// `pageOnly` — one frame, already taken.
    Done(Shot),
    /// `scrollingPage` — the region to scroll through, plus the banner details already read
    /// from the browser (the scroll loop can't ask afterwards: the page will have moved).
    Scroll { rect: (f64, f64, f64, f64), title: String, browser: ax::BrowserMeta },
}

/// Blocking body of the two frontmost-page modes. `pageOnly` requires a visible web area
/// (that's the point of the mode); `scrollingPage` prefers it — no browser chrome repeating
/// in the stitched frames — and falls back to the window frame for non-browsers.
///
/// Geometry comes from the AX tree when possible: CGWindowList ordering is unreliable on
/// modern macOS (Safari's toolbar strip is its own window), and `AXWebArea` alone reports
/// the full document extent, so `ax::page_geometry` intersects it down to the viewport.
fn page_target(mode: &str, with_browser_details: bool) -> anyhow::Result<PageTarget> {
    let win = backend::frontmost_window_bounds()?;
    let geometry = if scroll::app_accessibility_trusted() {
        ax::page_geometry(win.pid, PAGE_TREE_BUDGET)
    } else {
        None
    };
    // Read after `page_geometry`, which has just opted a Chromium browser in if it needed to:
    // by now the page will answer, and this costs a single tree walk.
    let browser = if with_browser_details { win.browser_meta() } else { Default::default() };
    if mode == "pageOnly" {
        if !scroll::app_accessibility_trusted() {
            anyhow::bail!(
                "Browser page capture needs Accessibility permission for Glyphio \
                 (System Settings › Privacy & Security › Accessibility) — it reads the \
                 page's position from the browser."
            );
        }
        let (x, y, w, h) = geometry.as_ref().and_then(|g| g.web_visible).ok_or_else(|| {
            if geometry.as_ref().is_some_and(|g| g.tree_still_building) {
                // Not the user's fault and not a lasting problem: the opt-in this attempt
                // just made is what the next one will find already done.
                anyhow!(
                    "{} was still publishing the page to macOS when the capture fired — \
                     Glyphio had to ask it to, which it only has to do once. Try again.",
                    if win.app_name.is_empty() { "The browser" } else { &win.app_name }
                )
            } else {
                anyhow!(
                    "No web page found in the frontmost window (“{}”) — Browser Page capture \
                     works when a browser (Safari, Chrome, Edge, Arc…) is in front, with a page \
                     loaded rather than a settings or downloads tab.",
                    win.title
                )
            }
        })?;
        let (img, dpr) = backend::capture_rect_image(x, y, w, h)?;
        let (width, height) = img.dimensions();
        return Ok(PageTarget::Done(Shot {
            rgba: img.into_raw(),
            width,
            height,
            dpr,
            title: win.title,
            browser,
        }));
    }
    // scrollingPage — the Accessibility error, if the grant is missing, comes from
    // scroll::capture itself. Fallbacks: viewport → AX window frame → CGWindow rect
    // (title bar already inset away by frontmost_window_bounds).
    let rect = match &geometry {
        Some(g) => g.web_visible.unwrap_or(g.window),
        None => (win.x, win.y, win.w, win.h),
    };
    Ok(PageTarget::Scroll { rect, title: win.title, browser })
}

/// One scrolling capture at a time. Two would fight over the pointer, and the second one's
/// start would clear the first one's stop flag — so a second hotkey press says so instead.
static SCROLLING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Run a scrolling capture. Escape is borrowed for its duration so it can finish early while
/// retaining every frame already captured.
pub(crate) async fn run_scrolling(
    app: &AppHandle,
    rect: (f64, f64, f64, f64),
) -> anyhow::Result<Shot> {
    use std::sync::atomic::Ordering;
    // Both checks happen before the capture starts.
    scroll::require_accessibility()?;
    if SCROLLING.swap(true, Ordering::SeqCst) {
        return Err(AlreadyScrolling.into());
    }
    // From here, the running flag and borrowed Escape are restored by this guard on every exit,
    // including a panic. Leaking the Escape registration would swallow it in every application.
    let _session = ScrollingSession(app.clone());

    // A capture must not bring Glyphio forward, and the activation policy is half of that: the
    // region-drag path closes its selection overlay just before this, which re-derives the
    // policy and promotes us back to a regular app a beat before the first frame. Held for the
    // length of the capture so both scrolling paths agree, and given back by the guard.
    crate::windows::hold_agent_policy(app);

    // Before Escape is armed, not inside the capture: a press landing between arming and the
    // clear would be wiped by it, and the capture would run on as though the key had missed.
    scroll::clear_stop();

    crate::shortcuts::arm_stop_key(app);

    let out = tauri::async_runtime::spawn_blocking(move || {
        scroll::capture(scroll::Job { rect })
    })
    .await;

    out.map_err(|e| anyhow!("scrolling capture task failed: {e}"))?
}

/// Asked for a scrolling capture while one was already running.
///
/// A type rather than a message, because this one failure must not be reported the way the
/// others are: [`report_failure`] puts a dialog up, and a dialog activates Glyphio — which
/// deactivates the window the *running* capture is photographing and greys its toolbar in every
/// remaining frame. Pressing the hotkey twice would damage the shot it was meant to protect.
#[derive(Debug)]
pub struct AlreadyScrolling;

impl std::fmt::Display for AlreadyScrolling {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "A scrolling capture is already running — Esc finishes it.")
    }
}

impl std::error::Error for AlreadyScrolling {}

/// Everything a running scrolling capture has to give back, released by dropping it.
struct ScrollingSession(AppHandle);

impl Drop for ScrollingSession {
    fn drop(&mut self) {
        crate::shortcuts::release_stop_key(&self.0);
        // Back to whatever the windows on screen say we should be — the Dock icon returns if a
        // main window is still up. See `windows::hold_agent_policy`.
        crate::windows::sync_activation_policy(&self.0, None);
        SCROLLING.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Log a capture failure AND put it in front of the user — captures fire from global
/// shortcuts and the tray, where a silent log line reads as "nothing happened".
pub fn report_failure(app: &AppHandle, context: &str, e: &anyhow::Error) {
    use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
    log::error!("{context} failed: {e}");
    if e.downcast_ref::<AlreadyScrolling>().is_some() {
        return; // see AlreadyScrolling: a dialog here would spoil the capture that IS running
    }
    app.dialog()
        .message(format!("{e:#}"))
        .title("Glyphio — capture failed")
        .kind(MessageDialogKind::Error)
        .show(|_| {});
}

/// [`trigger`] + user-visible error reporting; the fire-and-forget entry point used by the
/// tray menu and global shortcuts.
pub fn trigger_or_report(app: &AppHandle, mode: &str, delivery: Option<Delivery>) {
    if let Err(e) = trigger(app, mode, delivery) {
        report_failure(app, &format!("capture ({mode})"), &e);
    }
}

/// Shared tail of every capture path: encode, stash for the editor, open it.
///
/// "Open it" is where silent mode parts company — the same page does the work in a window
/// that is never shown, so the banner, the clipboard write and the history row are produced
/// by exactly one implementation either way, and a silent capture is still a capture you can
/// open and annotate afterwards.
pub fn finish(app: &AppHandle, shot: Shot, mode: &str, delivery: Delivery) -> anyhow::Result<()> {
    let png = encode_png(&shot)?;
    let data_url = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&png)
    );
    let silent = delivery.is_silent();
    let pending = PendingCapture {
        png_data_url: data_url,
        width: shot.width,
        height: shot.height,
        dpr: shot.dpr,
        mode: mode.to_string(),
        title: shot.title,
        page_title: shot.browser.page_title,
        page_url: shot.browser.url,
        profile: shot.browser.profile,
        captured_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        silent,
    };
    *app.state::<AppState>().pending_capture.lock().unwrap() = Some(pending);
    if silent {
        crate::windows::run_silent_capture(app)
    } else {
        crate::windows::open(app, "editor")
    }
}

fn encode_png(shot: &Shot) -> anyhow::Result<Vec<u8>> {
    let buf = image::RgbaImage::from_raw(shot.width, shot.height, shot.rgba.clone())
        .ok_or_else(|| anyhow::anyhow!("capture buffer size mismatch"))?;
    let mut out = std::io::Cursor::new(Vec::new());
    buf.write_to(&mut out, image::ImageFormat::Png)?;
    Ok(out.into_inner())
}
