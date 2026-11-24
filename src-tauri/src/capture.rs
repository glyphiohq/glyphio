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
//! | `platform/macos` (capture) | ScreenCaptureKit + `/usr/sbin/screencapture` | Windows.Graphics.Capture |
//! | `platform/macos` (inspect) | Accessibility API (`AXDocument` for URLs) | UI Automation               |
//! | `platform/macos` (input) | CGEvent scroll injection                 | `SendInput`                 |
//!
//! `scroll` is only half platform-bound — the frame stitching is pure image work and ports
//! as-is; only `post_scroll` and `warp_cursor` are Quartz. See `docs/WINDOWS.md` for the
//! full port plan, including the one piece with no Windows equivalent (macOS gives us its
//! interactive window picker for free; Windows has nothing like it).

pub mod delivery;
pub mod diag;
pub mod lifecycle;
pub mod orchestration;
pub mod scroll;
pub mod trace;
pub mod worker;

use anyhow::anyhow;
use base64::Engine;
use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::platform::{self, BrowserMeta, PickKind};
use crate::AppState;
use delivery::DeliveryRoute;
use lifecycle::CaptureMode;

/// Undo any accessibility opt-in still outstanding on a browser we captured from — called on
/// app exit, since the cooldown thread that normally does it dies with us.
pub fn restore_browser_accessibility() {
    platform::current().inspector.release();
}

/// (origin, size) in points of the display under the cursor — used to place the
/// scrolling-capture selection overlay.
pub fn display_bounds_under_cursor() -> ((f64, f64), (f64, f64)) {
    platform::current().windows.display_bounds_under_cursor()
}

/// (origin, size) in points of the display hosting the frontmost window — where the user
/// is typing. Expansion-summoned surfaces (popup/form/palette) belong there; the cursor
/// can be on a different display entirely, so it's only the fallback.
pub fn display_bounds_for_active_window() -> ((f64, f64), (f64, f64)) {
    platform::current()
        .windows
        .focused_window_display()
        .unwrap_or_else(|| platform::current().windows.display_bounds_under_cursor())
}

/// Whether a point falls on some connected display. Used to reject a remembered window
/// position belonging to a monitor that isn't attached any more — restoring it would put the
/// window somewhere the user can't reach.
pub fn point_on_a_display(x: f64, y: f64) -> bool {
    platform::current().windows.display_bounds_containing_point(x, y).is_some()
}

/// (origin, size) in points of the display holding a global point — where a panel anchored to
/// something on screen has to stay inside.
pub fn display_bounds_containing(x: f64, y: f64) -> Option<((f64, f64), (f64, f64))> {
    platform::current().windows.display_bounds_containing_point(x, y)
}

/// A captured, un-annotated frame handed to the editor webview.
pub struct Shot {
    /// RGBA8 pixels, row-major. Empty when the capture arrives already encoded (`png`).
    pub rgba: Vec<u8>,
    /// The capture as a PNG file, when it was encoded as it was taken: a scrolling capture
    /// streams its rows into the encoder and never holds the whole page uncompressed.
    pub png: Option<Vec<u8>>,
    pub width: u32,
    pub height: u32,
    /// Backing-scale factor (2.0 on Retina) so the banner/editor can size text correctly.
    pub dpr: f64,
    /// Window/app title — empty when capturing a whole display or a picked region.
    pub title: String,
    /// What the browser said about the page, for the modes that target one window. Empty
    /// unless the user has asked for one of those banner lines.
    pub browser: BrowserMeta,
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
    /// The row the capture was already saved as natively, before any window opened. The page
    /// loads the image from `file_path` and never saves it again. Empty on the legacy path,
    /// where `png_data_url` carries the pixels instead.
    pub history_id: String,
    pub file_path: String,
}

/// Whether any banner line needs the browser asked about the page.
pub(crate) fn wants_browser_details(app: &AppHandle) -> bool {
    app.state::<AppState>()
        .settings
        .lock()
        .unwrap()
        .wants_browser_details()
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
            if app
                .state::<AppState>()
                .settings
                .lock()
                .unwrap()
                .silent_capture
            {
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
    let mode = CaptureMode::try_from(mode).map_err(anyhow::Error::msg)?;
    trigger_mode(app, mode, delivery)
}

fn trigger_mode(
    app: &AppHandle,
    mode: CaptureMode,
    delivery: Option<Delivery>,
) -> anyhow::Result<()> {
    // This is deliberately before any picker, browser-tree lookup, or pixel work: a global
    // shortcut must visibly acknowledge that it fired before capture can become slow.
    let activity = crate::tray::capture_started(app, mode)?;
    trace::begin(mode);
    let delivery = Delivery::resolve(app, delivery);
    // Everything slow happens on the capture worker; this thread only queues the request. The
    // picker, the browser's accessibility tree and the scroll loop used to run right here, on
    // whichever thread the hotkey, tray or palette had put us on — usually the main one.
    let request = CaptureRequest {
        mode,
        delivery,
        activity,
    };
    worker(app).submit(request).map_err(|e| {
        let e = anyhow::Error::from(e);
        report_failure_for(app, activity, "capture", &e);
        e
    })
}

/// A capture that has been begun (the lifecycle knows) and is waiting for, or running on, the
/// worker.
pub struct CaptureRequest {
    mode: CaptureMode,
    delivery: Delivery,
    activity: lifecycle::CaptureToken,
}

/// The capture worker, started on first use. It holds an `AppHandle` for the rest of the run.
fn worker(app: &AppHandle) -> &'static worker::Worker<CaptureRequest> {
    static WORKER: std::sync::OnceLock<worker::Worker<CaptureRequest>> =
        std::sync::OnceLock::new();
    WORKER.get_or_init(|| {
        let app = app.clone();
        worker::Worker::spawn(move |request| run_request(&app, request))
    })
}

/// Run one request to the point where a window takes over, reporting any failure.
fn run_request(app: &AppHandle, request: CaptureRequest) {
    let CaptureRequest {
        mode,
        delivery,
        activity,
    } = request;
    let context = match mode {
        CaptureMode::Scrolling => "scrolling capture selection".to_string(),
        CaptureMode::PageOnly | CaptureMode::ScrollingPage => "page capture".to_string(),
        _ => format!("capture ({})", mode.as_str()),
    };
    if let Err(e) = capture_request(app, mode, delivery, activity) {
        report_failure_for(app, activity, &context, &e);
    }
}

fn capture_request(
    app: &AppHandle,
    mode: CaptureMode,
    delivery: Delivery,
    activity: lifecycle::CaptureToken,
) -> anyhow::Result<()> {
    if mode == CaptureMode::Scrolling {
        // The region is dragged first; the overlay carries the decision back with the rect.
        let app2 = app.clone();
        return on_main(app, move || {
            crate::windows::open_scroll_overlay(&app2, delivery)
        });
    }
    let shot = if matches!(mode, CaptureMode::PageOnly | CaptureMode::ScrollingPage) {
        // Both walk the AX tree (possible Chromium opt-in retry) and, for scrollingPage,
        // scroll + settle per frame. That is exactly why this runs on the worker.
        match page_target(mode, wants_browser_details(app))? {
            PageTarget::Scroll {
                rect,
                title,
                browser,
            } => {
                let outcome = tauri::async_runtime::block_on(run_scrolling(app, rect))?;
                note_scroll_end(app, outcome.reason);
                let mut shot = outcome.shot;
                shot.title = title;
                shot.browser = browser;
                shot
            }
            PageTarget::Done(shot) => shot,
        }
    } else {
        acquire(mode, wants_browser_details(app))?
    };
    trace::mark(trace::Stage::Acquire);
    finish(app, shot, mode, delivery, activity)
}

/// Run `f` on the main thread and wait for its result. Window creation and activation-policy
/// changes belong there; the worker waits, the main thread does not.
fn on_main<T: Send + 'static>(
    app: &AppHandle,
    f: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> anyhow::Result<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    app.run_on_main_thread(move || {
        let _ = tx.send(f());
    })
    .map_err(|e| anyhow!("could not reach the main thread: {e}"))?;
    rx.recv()
        .map_err(|_| anyhow!("the main thread dropped the window request"))?
}

/// How long to wait for a browser that builds its accessibility tree on demand.
///
/// Measured cold on an idle Mac (Chrome 3 windows, full screen): 2.0–2.2s from the opt-in to
/// the web area appearing. The previous 3s left ~0.8s of headroom, which a loaded machine
/// eats — and the failure is a hard error telling the user their page isn't a page. The wait
/// is only ever paid on the first capture per browser per grace period, and only while the
/// tree is genuinely being built (see `PageInspector::page_geometry`), so the ceiling is cheap to raise
/// and expensive to keep tight.
const PAGE_TREE_BUDGET: std::time::Duration = std::time::Duration::from_millis(8000);

/// Outcome of resolving what a frontmost-page capture is aimed at.
enum PageTarget {
    /// `pageOnly` — one frame, already taken.
    Done(Shot),
    /// `scrollingPage` — the region to scroll through, plus the banner details already read
    /// from the browser (the scroll loop can't ask afterwards: the page will have moved).
    Scroll {
        rect: (f64, f64, f64, f64),
        title: String,
        browser: BrowserMeta,
    },
}

/// Blocking body of the two frontmost-page modes. `pageOnly` requires a visible web area
/// (that's the point of the mode); `scrollingPage` prefers it — no browser chrome repeating
/// in the stitched frames — and falls back to the window frame for non-browsers.
///
/// Geometry comes from the AX tree when possible: CGWindowList ordering is unreliable on
/// modern macOS (Safari's toolbar strip is its own window), and `AXWebArea` alone reports
/// the full document extent, so `PageInspector::page_geometry` intersects it down to the viewport.
fn page_target(mode: CaptureMode, with_browser_details: bool) -> anyhow::Result<PageTarget> {
    let win = platform::current().windows.frontmost_window()?;
    let geometry = if platform::current().permissions.accessibility_trusted() {
        platform::current().inspector.page_geometry(win.pid, PAGE_TREE_BUDGET)
    } else {
        None
    };
    // Read after `page_geometry`, which has just opted a Chromium browser in if it needed to:
    // by now the page will answer, and this costs a single tree walk.
    let browser = if with_browser_details {
        platform::current().inspector.browser_meta(win.pid, &win.app_name)
    } else {
        Default::default()
    };
    if mode == CaptureMode::PageOnly {
        if !platform::current().permissions.accessibility_trusted() {
            anyhow::bail!(
                "Browser page capture needs Accessibility permission for Glyphio \
                 (System Settings › Privacy & Security › Accessibility) — it reads the \
                 page's position from the browser."
            );
        }
        let (x, y, w, h) = geometry
            .as_ref()
            .and_then(|g| g.web_visible)
            .ok_or_else(|| {
                if geometry.as_ref().is_some_and(|g| g.tree_still_building) {
                    // Not the user's fault and not a lasting problem: the opt-in this attempt
                    // just made is what the next one will find already done.
                    anyhow!(
                        "{} was still publishing the page to macOS when the capture fired — \
                     Glyphio had to ask it to, which it only has to do once. Try again.",
                        if win.app_name.is_empty() {
                            "The browser"
                        } else {
                            &win.app_name
                        }
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
        let (img, dpr) = platform::current().capturer.rect_image(x, y, w, h)?;
        let (width, height) = img.dimensions();
        return Ok(PageTarget::Done(Shot {
            rgba: img.into_raw(),
            png: None,
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
    Ok(PageTarget::Scroll {
        rect,
        title: win.title,
        browser,
    })
}

/// One scrolling capture at a time. Two would fight over the pointer, and the second one's
/// start would clear the first one's stop flag — so a second hotkey press says so instead.
static SCROLLING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Run a scrolling capture. Escape is borrowed for its duration so it can finish early while
/// retaining every frame already captured.
pub(crate) async fn run_scrolling(
    app: &AppHandle,
    rect: (f64, f64, f64, f64),
) -> anyhow::Result<scroll::Outcome> {
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

    let beat = app.clone();
    let shown = app.clone();
    let max_height_pt =
        f64::from(app.state::<AppState>().settings.lock().unwrap().scroll_max_height_pt.max(1));
    let out = tauri::async_runtime::spawn_blocking(move || {
        scroll::capture(
            scroll::Job { rect, max_height_pt },
            || crate::tray::capture_heartbeat(&beat),
            |text| crate::tray::capture_progress(&shown, text),
        )
    })
    .await;

    out.map_err(|e| anyhow!("scrolling capture task failed: {e}"))?
}

/// Tell the user when a scrolling capture ended before the end of the page.
pub(crate) fn note_scroll_end(app: &AppHandle, reason: scroll::StopReason) {
    if reason.is_incomplete() && reason != scroll::StopReason::Stopped {
        crate::tray::capture_result_note(app, reason.describe().to_string());
    }
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
        write!(
            f,
            "A scrolling capture is already running — Esc finishes it."
        )
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

/// The user backed out of an interactive picker. Not a failure: nothing is shown.
#[derive(Debug)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("capture cancelled")
    }
}

impl std::error::Error for Cancelled {}

/// Log a capture failure and expose it through the non-activating menu-bar feedback surface.
/// The details dialog is shown only if the user chooses "Show details" from that menu.
pub fn report_failure(app: &AppHandle, context: &str, e: &anyhow::Error) {
    if e.downcast_ref::<Cancelled>().is_some() {
        return; // already cleared, for its own token, where it was raised
    }
    log::error!("{context} failed: {e}");
    trace::discard();
    if e.downcast_ref::<AlreadyScrolling>().is_some()
        || e.downcast_ref::<lifecycle::CaptureInProgress>().is_some()
    {
        return; // changing the feedback would spoil the capture that IS running
    }
    crate::tray::capture_failed_current(app, format!("{e:#}"));
}

fn report_failure_for(
    app: &AppHandle,
    activity: lifecycle::CaptureToken,
    context: &str,
    e: &anyhow::Error,
) {
    if e.downcast_ref::<Cancelled>().is_some() {
        log::info!("{context} cancelled by the user");
        crate::tray::capture_cancelled_for(app, activity);
        return;
    }
    log::error!("{context} failed: {e}");
    if e.downcast_ref::<AlreadyScrolling>().is_some() {
        return;
    }
    crate::tray::capture_failed(app, activity, format!("{e:#}"));
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
pub fn finish(
    app: &AppHandle,
    mut shot: Shot,
    mode: CaptureMode,
    delivery: Delivery,
    activity: lifecycle::CaptureToken,
) -> anyhow::Result<()> {
    trace::mark_if_missing(trace::Stage::Acquire); // page and scrolling modes acquire off this path
    trace::set_size(shot.width, shot.height);
    let png = match shot.png.take() {
        Some(png) => png,
        None => encode_png(&shot)?,
    };
    trace::mark(trace::Stage::Encode);
    let silent = delivery.is_silent();
    let captured_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let legacy = app.state::<AppState>().settings.lock().unwrap().legacy_delivery;
    let (data_url, history_id, file_path) = if legacy {
        let data_url = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&png)
        );
        (data_url, String::new(), String::new())
    } else {
        let row = persist_native(app, &shot, mode, &captured_at, &png)?;
        (String::new(), row.id, row.full_path)
    };
    let pending = PendingCapture {
        png_data_url: data_url,
        width: shot.width,
        height: shot.height,
        dpr: shot.dpr,
        mode: mode.as_str().to_string(),
        title: shot.title,
        page_title: shot.browser.page_title,
        page_url: shot.browser.url,
        profile: shot.browser.profile,
        captured_at,
        silent,
        history_id,
        file_path,
    };
    let session_id =
        crate::tray::capture_completed(app, activity, DeliveryRoute::from_silent(silent), pending)?;
    // Only the window needs the main thread; the encoding above does not.
    let app = app.clone();
    let handed_off = on_main(&app.clone(), move || {
        if silent {
            crate::windows::run_silent_capture(&app, session_id.as_str())
        } else {
            crate::windows::open_editor_session(&app, session_id.as_str())
        }
    });
    trace::mark(trace::Stage::Present);
    trace::finish();
    handed_off
}

/// Save the capture before anything else can go wrong: a history row (or a scratch row when
/// history is off), its PNG and a thumbnail, all written here rather than by the editor page.
/// Whatever happens to the page afterwards, the capture is on disk.
fn persist_native(
    app: &AppHandle,
    shot: &Shot,
    mode: CaptureMode,
    captured_at: &str,
    png: &[u8],
) -> anyhow::Result<crate::history::CaptureMeta> {
    let state = app.state::<AppState>();
    let (keep, max_count, max_bytes) = {
        let s = state.settings.lock().unwrap();
        let keep = if s.history_enabled {
            crate::history::Keep::History
        } else {
            crate::history::Keep::Scratch
        };
        (keep, s.history_max_count, s.history_max_bytes)
    };
    let thumb = thumbnail_png(shot, png, THUMB_WIDTH)?;
    let row = state.history.persist(
        new_capture(shot, mode, captured_at),
        png,
        &thumb,
        keep,
        max_count,
        max_bytes,
    )?;
    trace::mark(trace::Stage::Persist);
    // A new capture replaces whatever scratch capture an earlier editor left behind.
    if keep == crate::history::Keep::Scratch {
        let _ = state.history.purge_scratch(Some(&row.id));
    }
    Ok(row)
}

/// The history row for a capture as it was taken: no note yet, banner on.
pub(crate) fn new_capture(shot: &Shot, mode: CaptureMode, captured_at: &str) -> crate::history::NewCapture {
    crate::history::NewCapture {
        captured_at: captured_at.to_string(),
        url: shot.title.clone(),
        title: shot.title.clone(),
        page_title: shot.browser.page_title.clone(),
        page_url: shot.browser.url.clone(),
        profile: shot.browser.profile.clone(),
        mode: mode.as_str().to_string(),
        image_width_px: i64::from(shot.width),
        image_height_px: i64::from(shot.height),
        dpr: shot.dpr,
        note: String::new(),
        banner_enabled: true,
    }
}

/// Width of a history thumbnail, as the history view lays them out.
pub const THUMB_WIDTH: u32 = 320;

/// A `width`-wide PNG thumbnail (aspect kept; never upscaled) from the capture's pixels, or
/// from its PNG when it arrived already encoded.
pub fn thumbnail_png(shot: &Shot, png: &[u8], width: u32) -> anyhow::Result<Vec<u8>> {
    let decoded;
    let full = if shot.rgba.len() as u64 == u64::from(shot.width) * u64::from(shot.height) * 4 && !shot.rgba.is_empty() {
        image::RgbaImage::from_raw(shot.width, shot.height, shot.rgba.clone())
            .ok_or_else(|| anyhow::anyhow!("capture buffer size mismatch"))?
    } else {
        decoded = image::load_from_memory(png)?.to_rgba8();
        decoded
    };
    let (w, h) = full.dimensions();
    let (tw, th) = thumb_size(w, h, width);
    let thumb = if (tw, th) == (w, h) {
        full
    } else {
        image::imageops::resize(&full, tw, th, image::imageops::FilterType::Triangle)
    };
    let mut out = std::io::Cursor::new(Vec::new());
    thumb.write_to(&mut out, image::ImageFormat::Png)?;
    Ok(out.into_inner())
}

/// Thumbnail dimensions: `width` wide with the aspect kept, never larger than the original.
pub fn thumb_size(w: u32, h: u32, width: u32) -> (u32, u32) {
    if w <= width || w == 0 {
        return (w.max(1), h.max(1));
    }
    let th = ((u64::from(h) * u64::from(width)) / u64::from(w)).max(1) as u32;
    (width, th)
}

pub fn encode_png(shot: &Shot) -> anyhow::Result<Vec<u8>> {
    if let Some(png) = &shot.png {
        return Ok(png.clone());
    }
    let buf = image::RgbaImage::from_raw(shot.width, shot.height, shot.rgba.clone())
        .ok_or_else(|| anyhow::anyhow!("capture buffer size mismatch"))?;
    let mut out = std::io::Cursor::new(Vec::new());
    buf.write_to(&mut out, image::ImageFormat::Png)?;
    Ok(out.into_inner())
}

/// Pixels for the four single-shot modes, through the platform.
fn acquire(mode: CaptureMode, with_browser_details: bool) -> anyhow::Result<Shot> {
    let p = platform::current();
    match mode {
        CaptureMode::FullWindow => p.picker.pick(PickKind::Window),
        CaptureMode::Snip => p.picker.pick(PickKind::Region),
        CaptureMode::FrontWindow => p.capturer.front_window(with_browser_details),
        _ => p.capturer.visible(), // Visible
    }
}

/// Non-interactive capture without an app handle, for `examples/bench_capture.rs` only.
#[doc(hidden)]
pub fn bench_capture(mode: &str) -> anyhow::Result<Shot> {
    let p = platform::current();
    match mode {
        "frontWindow" => p.capturer.front_window(false),
        _ => p.capturer.visible(),
    }
}

#[cfg(test)]
mod native_delivery_tests {
    use super::*;

    fn shot(w: u32, h: u32) -> Shot {
        Shot {
            rgba: vec![200; (w * h * 4) as usize],
            png: None,
            width: w,
            height: h,
            dpr: 2.0,
            title: "Window".into(),
            browser: BrowserMeta { page_title: "Page".into(), url: "https://example.com".into(), profile: String::new() },
        }
    }

    #[test]
    fn thumbnails_keep_the_aspect_and_never_upscale() {
        assert_eq!(thumb_size(5120, 2880, 320), (320, 180));
        assert_eq!(thumb_size(200, 100, 320), (200, 100));
        assert_eq!(thumb_size(10_000, 1, 320), (320, 1));
    }

    #[test]
    fn a_thumbnail_comes_from_pixels_or_from_an_already_encoded_capture() {
        let s = shot(640, 100);
        let png = encode_png(&s).unwrap();
        let from_pixels = image::load_from_memory(&thumbnail_png(&s, &png, 320).unwrap()).unwrap();
        assert_eq!((from_pixels.width(), from_pixels.height()), (320, 50));

        let encoded = Shot { rgba: Vec::new(), png: Some(png.clone()), ..shot(640, 100) };
        let from_png = image::load_from_memory(&thumbnail_png(&encoded, &png, 320).unwrap()).unwrap();
        assert_eq!((from_png.width(), from_png.height()), (320, 50));
    }

    #[test]
    fn the_native_row_carries_what_the_banner_needs() {
        let row = new_capture(&shot(4, 2), CaptureMode::FrontWindow, "2026-11-24T00:00:00.000Z");
        assert_eq!((row.image_width_px, row.image_height_px, row.dpr), (4, 2, 2.0));
        assert_eq!((row.page_title.as_str(), row.page_url.as_str()), ("Page", "https://example.com"));
        assert_eq!(row.mode, "frontWindow");
        assert!(row.banner_enabled && row.note.is_empty());
    }
}

