//! Webview window management. Each surface is a static HTML page under `ui/`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

struct Spec {
    url: &'static str,
    title: &'static str,
    width: f64,
    height: f64,
}

fn spec(name: &str) -> Option<Spec> {
    Some(match name {
        // The main index is the Settings / snippet-manager surface.
        "settings" => Spec { url: "index.html", title: "Glyphio", width: 1180.0, height: 820.0 },
        "editor" => Spec { url: "editor/index.html", title: "Glyphio — Edit Capture", width: 1280.0, height: 880.0 },
        _ => return None,
    })
}

/// Windows that are "the app" rather than a transient surface. While one of these is on
/// screen Glyphio behaves like a regular macOS app — Dock icon, menu bar, ⌘Q, working
/// full screen — and reverts to a menu-bar agent once they're all gone. The palette,
/// popup/form surfaces and the capture overlay deliberately don't count: they're summoned
/// over whatever you're working in and must never steal an app slot in the Dock.
fn is_main_window(label: &str) -> bool {
    label == "settings" || label == "editor" || label.starts_with("capture-")
}

/// Re-derive the macOS activation policy from what's on screen. `ignoring` excludes a window
/// that is closing right now — during `Destroyed` it may still be in the manager's map.
pub fn sync_activation_policy(app: &AppHandle, ignoring: Option<&str>) {
    #[cfg(target_os = "macos")]
    {
        let any_visible = app.webview_windows().iter().any(|(label, win)| {
            is_main_window(label)
                && Some(label.as_str()) != ignoring
                && win.is_visible().unwrap_or(false)
        });
        apply_activation_policy(app, any_visible);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, ignoring);
}

/// Set the policy, remembering what it is. The transition is visible — the Dock icon appears
/// or disappears — so it is only applied on an actual change; calling it on every window event
/// otherwise flickers.
///
/// Summoned surfaces call this with `false` before building themselves: a window created by a
/// *regular* app is born on the Space that app's other windows are on, so with Settings parked
/// on a spare display the palette, the capture overlay and the expansion popups all appeared
/// over there — or nowhere at all, if the user was in a full-screen app. Created as a menu-bar
/// agent, the same window is born where the user is. The Dock icon comes back on its own: the
/// surface's `Destroyed` event re-derives the policy from what is still on screen.
#[cfg(target_os = "macos")]
fn apply_activation_policy(app: &AppHandle, regular: bool) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static IS_REGULAR: AtomicBool = AtomicBool::new(false);

    if regular == IS_REGULAR.swap(regular, Ordering::SeqCst) {
        return;
    }
    let policy = if regular {
        tauri::ActivationPolicy::Regular
    } else {
        tauri::ActivationPolicy::Accessory
    };
    if let Err(e) = app.set_activation_policy(policy) {
        log::warn!("could not set activation policy: {e}");
    }
}

// --- Remembered window geometry ----------------------------------------------------------
// Size and position survive a quit: reopening Settings on a 6K display shouldn't hand back
// the small default every launch. Stored next to the rest of the app data, keyed by surface
// (every stored capture window shares one key — they're the same kind of window).

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct Geometry {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

fn geometry_key(label: &str) -> &str {
    if label.starts_with("capture-") {
        "capture"
    } else {
        label
    }
}

fn geometry_file(app: &AppHandle) -> std::path::PathBuf {
    app.state::<crate::AppState>().paths.root.join("window-state.json")
}

fn load_geometry(app: &AppHandle, label: &str) -> Option<Geometry> {
    let text = std::fs::read_to_string(geometry_file(app)).ok()?;
    let all: HashMap<String, Geometry> = serde_json::from_str(&text).ok()?;
    let g = all.get(geometry_key(label)).copied()?;
    // A window remembered on a monitor that's since been unplugged would open off-screen,
    // where it can't be reached at all — keep the size, drop the position.
    if g.w < 200.0 || g.h < 150.0 || !crate::capture::point_on_a_display(g.x + 40.0, g.y + 10.0) {
        return None;
    }
    Some(g)
}

/// Record a window's current frame. Called while the window still exists (close *request*,
/// app exit) — after destruction there's nothing left to measure.
pub fn save_geometry(app: &AppHandle, label: &str) {
    if label == SILENT_EDITOR {
        return; // never shown, so its frame is nobody's preference
    }
    let Some(win) = app.get_webview_window(label) else { return };
    // A full-screen or minimized frame is not what the user wants back on next launch.
    if win.is_fullscreen().unwrap_or(false) || win.is_minimized().unwrap_or(false) {
        return;
    }
    let (Ok(pos), Ok(size), Ok(scale)) = (win.outer_position(), win.inner_size(), win.scale_factor())
    else {
        return;
    };
    let pos = pos.to_logical::<f64>(scale);
    let size = size.to_logical::<f64>(scale);
    let path = geometry_file(app);
    let mut all: HashMap<String, Geometry> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    all.insert(
        geometry_key(label).to_string(),
        Geometry { x: pos.x, y: pos.y, w: size.width, h: size.height },
    );
    if let Ok(json) = serde_json::to_vec_pretty(&all) {
        let _ = std::fs::write(path, json);
    }
}

/// Save every open main window — used on app exit, where no per-window close event fires.
pub fn save_all_geometry(app: &AppHandle) {
    for (label, win) in app.webview_windows() {
        if is_main_window(&label) && win.is_visible().unwrap_or(false) {
            save_geometry(app, &label);
        }
    }
}

/// Where a window should open: remembered geometry, else the spec size centred on the display
/// the user is working on. The default is clamped to the screen — a 1280×880 window must not
/// open half off a 1280×800 laptop display.
fn placement(app: &AppHandle, label: &str, spec: &Spec) -> Geometry {
    if let Some(g) = load_geometry(app, label) {
        return g;
    }
    let (origin, display) = crate::capture::display_bounds_for_active_window();
    let w = spec.width.min(display.0 - 80.0).max(640.0);
    let h = spec.height.min(display.1 - 120.0).max(480.0);
    Geometry {
        x: origin.0 + (display.0 - w) / 2.0,
        y: origin.1 + (display.1 - h) / 2.0,
        w,
        h,
    }
}

/// Bring an existing window to the front, adopting the Dock/menu-bar identity that goes with
/// having a window on screen.
fn present(app: &AppHandle, win: &WebviewWindow) -> anyhow::Result<()> {
    win.show()?;
    sync_activation_policy(app, None);
    win.set_focus()?;
    Ok(())
}

/// Let a window follow the user to whichever desktop they're on, instead of dragging them back
/// to the one it was opened on.
///
/// `MoveToActiveSpace` means: when Glyphio is activated, bring this window to the current
/// Space rather than switching Spaces to reach it. That fixes the ordinary multiple-desktops
/// case, where an editor left on desktop 1 would otherwise yank you off desktop 2.
///
/// # What this deliberately does *not* fix
///
/// Capturing while another app is full screen still opens the editor on Glyphio's own Space.
/// The bit that would change that is `FullScreenAuxiliary`, and it is not worth what it costs:
/// it is mutually exclusive with `FullScreenPrimary`, which is what gives a window its own
/// green full-screen button. Buying "the editor appears over full-screen Chrome" with "the
/// editor can no longer be full-screened" is a bad trade for a window people work in.
///
/// Take a silent capture instead (⌘↩ in the palette, or the *Capture to Clipboard* menu) —
/// it opens no window at all, so the Space question never arises.
///
/// Also deliberately not `CanJoinAllSpaces`: that pins a window to *every* Space permanently,
/// which suits a palette and not a document window.
/// # Threading
///
/// `setCollectionBehavior:` is main-thread-only, and macOS 26 enforces it with a trap rather
/// than a warning: `EXC_BREAKPOINT`, "Must only be used from the main thread". Not every caller
/// is on the main thread — a page-only or scrolling capture finishes on a tokio worker and opens
/// the editor from there, which killed the app outright the first time a capture had to *create*
/// the editor window (an already-open one takes the `present` path and never reaches here, which
/// is why this could hide for a while). So the hop is done here, once, rather than trusted at
/// each call site; `run_on_main_thread` runs it inline when we are already on the main thread.
#[cfg(target_os = "macos")]
fn follow_user_across_spaces(win: &WebviewWindow) {
    use objc2_app_kit::{NSWindow, NSWindowCollectionBehavior};

    let win = win.clone();
    let app = win.app_handle().clone();
    let _ = app.run_on_main_thread(move || {
        let Ok(ptr) = win.ns_window() else { return };
        if ptr.is_null() {
            return;
        }
        // Safety: `ns_window` hands back this window's live NSWindow, and this block only runs
        // on the main thread.
        unsafe {
            let ns: &NSWindow = &*(ptr as *const NSWindow);
            let behavior = ns.collectionBehavior() | NSWindowCollectionBehavior::MoveToActiveSpace;
            ns.setCollectionBehavior(behavior);
        }
    });
}

#[cfg(not(target_os = "macos"))]
fn follow_user_across_spaces(_win: &WebviewWindow) {}

/// Re-assert focus a beat later.
///
/// A capture that used the interactive picker leaves `screencapture` as the frontmost app;
/// as it exits, macOS hands focus back to whatever was active before it — and that can land
/// *after* our `set_focus`, leaving the editor open but behind the window you just captured.
/// Asking again once the hand-back has settled is the reliable fix.
fn refocus_shortly(app: &AppHandle, label: &str) {
    let app = app.clone();
    let label = label.to_string();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(350)).await;
        let inner = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Some(win) = inner.get_webview_window(&label) {
                if win.is_visible().unwrap_or(false) {
                    let _ = win.set_focus();
                }
            }
        });
    });
}

/// Move a window onto the display the user is working on, keeping its size. Used for the
/// capture editor: it should open where the capture happened, not wherever it last sat —
/// which, with geometry remembered across launches, can be a different monitor entirely.
fn move_to_active_display(win: &WebviewWindow) {
    let (Ok(size), Ok(scale)) = (win.inner_size(), win.scale_factor()) else { return };
    let size = size.to_logical::<f64>(scale);
    let (origin, display) = crate::capture::display_bounds_for_active_window();
    let _ = win.set_position(tauri::LogicalPosition::new(
        origin.0 + (display.0 - size.width).max(0.0) / 2.0,
        origin.1 + (display.1 - size.height).max(0.0) / 2.0,
    ));
}

/// Open a stored capture (by id) in the editor's read-only view mode. Uses a per-capture window
/// label so it never clashes with the live-capture editor window.
pub fn open_capture(app: &AppHandle, id: &str) -> anyhow::Result<()> {
    let label = format!("capture-{id}");
    if let Some(win) = app.get_webview_window(&label) {
        // Reload so the view reflects the row's current content and the latest settings
        // (the page reads both once, at load).
        let _ = win.eval("window.location.reload()");
        return present(app, &win);
    }
    let url = format!("editor/index.html?history={id}");
    let spec = spec("editor").expect("editor spec");
    let g = placement(app, &label, &spec);
    let win = WebviewWindowBuilder::new(app, &label, WebviewUrl::App(url.into()))
        .title("Glyphio — Capture")
        .inner_size(g.w, g.h)
        .position(g.x, g.y)
        .min_inner_size(640.0, 480.0)
        .build()?;
    follow_user_across_spaces(&win);
    // A menu-bar agent isn't active when a window is built, so a fresh window would appear
    // BEHIND the frontmost app. Take the regular-app policy first, then set_focus activates
    // us (activateIgnoringOtherApps).
    sync_activation_policy(app, None);
    win.set_focus()?;
    Ok(())
}

/// Open (or focus, if already open) a named window.
pub fn open(app: &AppHandle, name: &str) -> anyhow::Result<()> {
    if let Some(win) = app.get_webview_window(name) {
        // The editor pulls its one-shot pending capture at page load. If a capture fires
        // while an old editor window is open, focusing it would show the previous image —
        // reload so the new capture is picked up.
        if name == "editor" {
            let _ = win.eval("window.location.reload()");
            // A capture belongs where the user just was, even if this window was left open
            // on another display.
            move_to_active_display(&win);
        }
        present(app, &win)?;
        if name == "editor" {
            refocus_shortly(app, name);
        }
        return Ok(());
    }
    let spec = spec(name).ok_or_else(|| anyhow::anyhow!("unknown window: {name}"))?;
    let mut g = placement(app, name, &spec);
    if name == "editor" {
        // Keep the remembered size, but centre it on the display the capture came from.
        let (origin, display) = crate::capture::display_bounds_for_active_window();
        g.x = origin.0 + (display.0 - g.w).max(0.0) / 2.0;
        g.y = origin.1 + (display.1 - g.h).max(0.0) / 2.0;
    }
    let win = WebviewWindowBuilder::new(app, name, WebviewUrl::App(spec.url.into()))
        .title(spec.title)
        .inner_size(g.w, g.h)
        .position(g.x, g.y)
        .min_inner_size(640.0, 480.0)
        .build()?;
    follow_user_across_spaces(&win);
    // See open_capture: without this, new windows of a menu-bar agent open behind the
    // frontmost app instead of on top.
    sync_activation_policy(app, None);
    win.set_focus()?;
    if name == "editor" {
        refocus_shortly(app, name);
    }
    Ok(())
}

/// The window that runs a silent capture: the editor page, never shown.
pub const SILENT_EDITOR: &str = "editor-silent";

/// Park the silent-capture worker: the editor page, invisible, with no capture to work on
/// yet. It sits there until a capture is handed to it by [`run_silent_capture`].
///
/// A window rather than a background thread because everything a capture needs after the
/// pixels — the banner, the PNG encoding, the thumbnail — is canvas work that lives in the
/// page, and doing it twice in two languages is how the two would drift apart.
///
/// It is parked in advance, and kept, because *creating* a window activates the application
/// even when the window is invisible: with Settings open behind, a silent capture would jump
/// Glyphio in front of whatever was being captured. Parking moves that one activation to a
/// moment the user is already looking at Glyphio — launch, or switching the setting on — and
/// every capture after it only reloads a window that already exists.
pub fn ensure_silent_editor(app: &AppHandle) -> anyhow::Result<()> {
    if app.get_webview_window(SILENT_EDITOR).is_some() {
        return Ok(());
    }
    WebviewWindowBuilder::new(
        app,
        SILENT_EDITOR,
        WebviewUrl::App("editor/index.html?silent=1".into()),
    )
    .title("Glyphio")
    .inner_size(900.0, 700.0)
    .visible(false)
    .focused(false)
    .skip_taskbar(true)
    .build()?;
    Ok(())
}

/// Hand the pending capture to the parked worker. Reloading is what starts it: the page pulls
/// the capture at load, exactly as the visible editor does.
pub fn run_silent_capture(app: &AppHandle) -> anyhow::Result<()> {
    match app.get_webview_window(SILENT_EDITOR) {
        Some(win) => win.eval("window.location.reload()").map_err(Into::into),
        None => ensure_silent_editor(app),
    }
}

/// Send the worker away — silent capture has been switched off.
pub fn close_silent_editor(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(SILENT_EDITOR) {
        let _ = win.close();
    }
}

/// Open a bridge-driven surface (`popup` | `form`). Always recreated (never focused-in-place):
/// the window must show the payload stashed for THIS expansion, not a stale one.
///
/// Summoned by TYPING a trigger, so the surface must appear centred on the display of the
/// window being typed in — by default macOS would put it on the primary display, which on a
/// multi-monitor setup can be nowhere near where the user is looking.
pub fn open_surface(app: &AppHandle, surface: &str) -> anyhow::Result<()> {
    if let Some(win) = app.get_webview_window(surface) {
        win.close().ok();
    }
    let (size, url) = match surface {
        "popup" => ((460.0, 540.0), "popup/index.html"),
        "form" => ((440.0, 420.0), "form/index.html"),
        other => anyhow::bail!("unknown surface: {other}"),
    };
    // Born where the user is typing, not where Settings is — see `apply_activation_policy`.
    #[cfg(target_os = "macos")]
    apply_activation_policy(app, false);
    let (origin, display) = crate::capture::display_bounds_for_active_window();
    let win = WebviewWindowBuilder::new(app, surface, WebviewUrl::App(url.into()))
        .title("Glyphio")
        .inner_size(size.0, size.1)
        .min_inner_size(if surface == "popup" { 280.0 } else { 320.0 }, if surface == "popup" { 200.0 } else { 240.0 })
        .position(
            origin.0 + (display.0 - size.0) / 2.0,
            origin.1 + (display.1 - size.1) / 2.0,
        )
        .always_on_top(true)
        .skip_taskbar(true)
        .build()
        .map_err(anyhow::Error::from)
        .and_then(|win| Ok(win.set_focus()?));
    if win.is_err() {
        // No window means no `Destroyed` event to hand the Dock icon back — see `toggle_palette`.
        sync_activation_policy(app, None);
    }
    win
}

/// Toggle the Spotlight-style snippet palette: a frameless, always-on-top search window.
///
/// # Why this builds a window instead of showing one
///
/// Summoning builds the window; dismissing destroys it. That is not the obvious design — this
/// used to keep one window alive and hidden, so ⌥Space cost only a `show()`. Two measured facts
/// on macOS 26 (three displays, Safari/Chrome/Ghostty full screen) rule that out:
///
/// 1. **A window can only be shown on the Space it was created on.** The kept window was
///    positioned on the right display, ordered in, and even made the key window — and the window
///    server still would not draw it, because it belonged to the desktop Space it was built on.
///    Typing went into a window nobody could see. `CanJoinAllSpaces` (which macOS reads as
///    "every *ordinary* Space", excluding full screen), `FullScreenAuxiliary`, `MoveToActiveSpace`
///    and ordering it out and back in all failed to move it.
/// 2. **A window created by a *regular* app is born on the Space that app's other windows are
///    on.** With the Settings window open, the palette could only ever appear on that window's
///    display — which is exactly "it only opens on empty screens" if Settings is parked on a
///    spare monitor. As a menu-bar agent, the same window is born where the user is, full-screen
///    Spaces included. See [`apply_activation_policy`], which the other summoned surfaces use
///    for the same reason.
///
/// So: build it fresh, as an agent, every time. The capture overlay has always been built per
/// use and never had this problem.
///
/// `view` is which of the palette's three lists to land on — `clipboard`, `captures` or
/// `snippets`. `None` means "whatever it was showing last", so the ordinary hotkey doesn't
/// reset a view the user deliberately switched to.
pub fn toggle_palette(app: &AppHandle, view: Option<&str>) -> anyhow::Result<()> {
    if let Some(win) = app.get_webview_window(PALETTE) {
        if win.is_focused().unwrap_or(true) {
            // On screen and taking keys: this press is the dismissal. (It destroys itself the
            // moment it loses focus, so there is nothing else it could mean.)
            win.destroy()?;
            return Ok(());
        }
        // On screen but not key — the summon that built it lost the focus race. The user
        // pressed the hotkey to get *to* the palette, so give it focus instead of throwing it
        // away and making them press again.
        win.set_focus()?;
        return Ok(());
    }
    // A page that isn't listening yet can't be told by an event, so it asks on load instead.
    if let Some(view) = view {
        *app.state::<crate::AppState>().palette_view.lock().unwrap() = view.to_string();
    }
    // Born where the user is, not where Settings is — see `apply_activation_policy`.
    #[cfg(target_os = "macos")]
    apply_activation_policy(app, false);
    // On failure the policy has to be put back by hand: there is no window, so no `Destroyed`
    // event will ever do it, and the app would sit there as a menu-bar agent with Settings on
    // screen and no Dock icon.
    let built = build_palette(app).and_then(|win| Ok(win.set_focus()?));
    if built.is_err() {
        sync_activation_policy(app, None);
    }
    built
}

pub const PALETTE: &str = "palette";
const PALETTE_SIZE: (f64, f64) = (640.0, 460.0);

/// Build the palette, on screen, on the Space the user is looking at right now.
fn build_palette(app: &AppHandle) -> anyhow::Result<WebviewWindow> {
    let pos = palette_position(app);
    let win = WebviewWindowBuilder::new(app, PALETTE, WebviewUrl::App("palette/index.html".into()))
        .title("Glyphio Search")
        .inner_size(PALETTE_SIZE.0, PALETTE_SIZE.1)
        .position(pos.0, pos.1)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .build()?;
    // No Spaces flags at all, deliberately — see [`toggle_palette`].
    Ok(win)
}

fn palette_position(app: &AppHandle) -> (f64, f64) {
    let _ = app;
    let (origin, display) = crate::capture::display_bounds_for_active_window();
    (
        origin.0 + (display.0 - PALETTE_SIZE.0) / 2.0,
        // Slightly above centre, Spotlight-style; also keeps result rows from spilling
        // off-screen as the list grows downward.
        origin.1 + (display.1 - PALETTE_SIZE.1) / 3.0,
    )
}

/// Open the transparent scrolling-capture selection overlay, sized to the display under the
/// cursor. Label `scroll-overlay`; it closes itself via the run/cancel commands.
///
/// `delivery` rides along in the URL and comes back with the selected rect: the user chose
/// how this capture should be delivered before they started dragging, and dragging a region
/// shouldn't quietly change the answer.
pub fn open_scroll_overlay(app: &AppHandle, delivery: crate::capture::Delivery) -> anyhow::Result<()> {
    if let Some(win) = app.get_webview_window("scroll-overlay") {
        win.close().ok(); // stale one — restart fresh
    }
    // Born where the user is, not where Settings is — see `apply_activation_policy`.
    #[cfg(target_os = "macos")]
    apply_activation_policy(app, false);
    let (origin, size) = crate::capture::display_bounds_under_cursor();
    let url = if delivery.is_silent() {
        "scroll-overlay/index.html?silent=1"
    } else {
        "scroll-overlay/index.html"
    };
    let win = WebviewWindowBuilder::new(app, "scroll-overlay", WebviewUrl::App(url.into()))
    .title("Select scrolling region")
    .position(origin.0, origin.1)
    .inner_size(size.0, size.1)
    .decorations(false)
    .transparent(true)
    .always_on_top(true)
    .skip_taskbar(true)
    .resizable(false)
    .build()
    .map_err(anyhow::Error::from)
    .and_then(|win| Ok(win.set_focus()?));
    if win.is_err() {
        // No window means no `Destroyed` event to hand the Dock icon back — see `toggle_palette`.
        sync_activation_policy(app, None);
    }
    win
}

/// Close the selection overlay if present.
pub fn close_scroll_overlay(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("scroll-overlay") {
        let _ = win.close();
    }
}

/// The readout that sits beside a running scrolling capture.
pub const SCROLL_HUD: &str = "scroll-hud";
/// One line: a spinner, the frame count, and the key that ends it. Nothing needing a second.
const SCROLL_HUD_SIZE: (f64, f64) = (272.0, 44.0);
/// Clearance between the readout and the region, and from the screen edge.
const SCROLL_HUD_GAP: f64 = 14.0;

/// Just outside the region being captured — below it when there is room, above it otherwise —
/// so it never covers what is being photographed. Clamped to the display the region is on.
fn scroll_hud_position(rect: (f64, f64, f64, f64)) -> (f64, f64) {
    let (origin, size) = crate::capture::display_bounds_containing(
        rect.0 + rect.2 / 2.0,
        rect.1 + rect.3 / 2.0,
    )
    .unwrap_or_else(crate::capture::display_bounds_under_cursor);
    scroll_hud_slot(rect, origin, size)
}

/// The placement arithmetic on its own, so it can be checked without a display attached.
fn scroll_hud_slot(
    rect: (f64, f64, f64, f64),
    origin: (f64, f64),
    size: (f64, f64),
) -> (f64, f64) {
    let (rx, ry, rw, rh) = rect;
    let (w, h) = SCROLL_HUD_SIZE;
    let g = SCROLL_HUD_GAP;
    // `f64::clamp` panics if the bounds cross, which they do on a display narrower than the
    // readout plus both gaps — so the upper bound is never allowed below the lower one.
    let left = origin.0 + g;
    let right = (origin.0 + size.0 - w - g).max(left);
    let x = (rx + (rw - w) / 2.0).clamp(left, right);
    let (below, above) = (ry + rh + g, ry - h - g);
    let y = if below + h + g <= origin.1 + size.1 {
        below
    } else if above >= origin.1 + g {
        above
    } else {
        // The region fills the display. Sit at the bottom edge, over the region: it is still
        // left out of the frames, and only the live view has it on top of anything.
        origin.1 + size.1 - h - g
    };
    (x, y)
}

/// Where the readout waits between captures: off every display, so it is simply not on screen.
const SCROLL_HUD_PARK: (f64, f64) = (-20000.0, -20000.0);

/// Build the readout up front and leave it parked off screen.
///
/// Same trick and same reason as [`ensure_silent_editor`]: *creating* a window activates the
/// application, and a capture is the worst possible moment for that — Glyphio coming forward
/// deactivates the window being photographed, so its toolbar greys and its selection fades,
/// baked into every frame from that moment on. Creating it at launch spends the activation while
/// the user is already looking at Glyphio; a capture then only moves a window that exists.
pub fn park_scroll_hud(app: &AppHandle) {
    if app.get_webview_window(SCROLL_HUD).is_some() {
        return;
    }
    let (w, h) = SCROLL_HUD_SIZE;
    let mut builder = WebviewWindowBuilder::new(
        app,
        SCROLL_HUD,
        WebviewUrl::App("scroll-hud/index.html".into()),
    )
    .title("Capturing")
    .position(SCROLL_HUD_PARK.0, SCROLL_HUD_PARK.1)
    .inner_size(w, h)
    .decorations(false)
    .transparent(true)
    .shadow(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .resizable(false)
    .focused(false);
    #[cfg(target_os = "macos")]
    {
        // Nothing here is clickable, but a stray click must not activate Glyphio mid-capture:
        // that would deactivate the window being photographed and grey its toolbar in the
        // frames from then on.
        builder = builder.accept_first_mouse(true);
    }
    if let Err(e) = builder.build() {
        log::warn!("could not park the scrolling readout: {e}");
    }
}

/// Bring the readout to the region being captured and return the frame it occupies, so the
/// capture knows which patch of screen is its own readout rather than the content to scroll.
///
/// A move, not a show: see [`park_scroll_hud`] for why nothing may be created here.
pub fn open_scroll_hud(app: &AppHandle, rect: (f64, f64, f64, f64)) -> Option<(f64, f64, f64, f64)> {
    let (x, y) = scroll_hud_position(rect);
    let (w, h) = SCROLL_HUD_SIZE;
    move_scroll_hud(app, (x, y))?;
    Some((x, y, w, h))
}

/// Send the readout back off screen. Left in place, not closed — closing it would mean creating
/// it again next time, which is the one thing a capture must not do.
pub fn close_scroll_hud(app: &AppHandle) {
    if move_scroll_hud(app, SCROLL_HUD_PARK).is_some() {
        return;
    }
    // It wasn't there to park. Now — with the capture over — is the safe moment to build one,
    // so the next capture has its readout. See `park_scroll_hud` for why not a moment sooner.
    let repair = app.clone();
    let _ = app.run_on_main_thread(move || park_scroll_hud(&repair));
}

fn move_scroll_hud(app: &AppHandle, (x, y): (f64, f64)) -> Option<()> {
    let inner = app.clone();
    // A missing readout is left missing for this capture: rebuilding it here would create a
    // window, and creating a window activates Glyphio — in front of whatever is being
    // photographed. `close_scroll_hud` does the repair once the frames are taken.
    if app.get_webview_window(SCROLL_HUD).is_none() {
        log::warn!("the scrolling readout is missing; this capture runs without it");
        return None;
    }
    app.run_on_main_thread(move || {
        if let Some(win) = inner.get_webview_window(SCROLL_HUD) {
            if let Err(e) = win.set_position(tauri::LogicalPosition::new(x, y)) {
                log::warn!("could not move the scrolling readout: {e}");
            }
            // Something opened over it since the last capture — take the top back.
            let _ = win.set_always_on_top(true);
        }
    })
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: ((f64, f64), (f64, f64)) = ((0.0, 0.0), (1920.0, 1080.0));

    /// The readout has to stay on the display and, wherever possible, off the region: it must
    /// not sit on top of what is being captured.
    #[test]
    fn the_readout_sits_below_the_region_when_there_is_room_and_above_it_otherwise() {
        let (w, h) = SCROLL_HUD_SIZE;
        let g = SCROLL_HUD_GAP;

        // Room underneath: centred on the region, in the gap below it.
        let region = (400.0, 100.0, 800.0, 600.0);
        let (x, y) = scroll_hud_slot(region, SCREEN.0, SCREEN.1);
        assert_eq!(x, 400.0 + (800.0 - w) / 2.0);
        assert_eq!(y, 700.0 + g);

        // Region runs to the bottom of the screen: the readout goes above it.
        let (_, y) = scroll_hud_slot((400.0, 400.0, 800.0, 680.0), SCREEN.0, SCREEN.1);
        assert_eq!(y, 400.0 - h - g);

        // Region fills the display: nowhere clear, so the bottom edge — inset, still on screen.
        let (x, y) = scroll_hud_slot((0.0, 0.0, 1920.0, 1080.0), SCREEN.0, SCREEN.1);
        assert_eq!(y, 1080.0 - h - g);
        assert!(x >= g && x + w <= 1920.0 - g);

        // A narrow region at the screen edge can't centre the readout off the display.
        let (x, _) = scroll_hud_slot((1860.0, 100.0, 50.0, 200.0), SCREEN.0, SCREEN.1);
        assert_eq!(x, 1920.0 - w - g);
        let (x, _) = scroll_hud_slot((10.0, 100.0, 50.0, 200.0), SCREEN.0, SCREEN.1);
        assert_eq!(x, g);
    }

    /// A second display starts at a non-zero origin, and everything above still has to hold
    /// relative to it — the readout belongs on the screen the region is on.
    #[test]
    fn the_readout_follows_the_region_onto_a_second_display() {
        let (w, h) = SCROLL_HUD_SIZE;
        let g = SCROLL_HUD_GAP;
        let (origin, size) = ((1920.0, 34.0), (1920.0, 1080.0));
        let (x, y) = scroll_hud_slot((2000.0, 134.0, 600.0, 500.0), origin, size);
        assert_eq!((x, y), (2000.0 + (600.0 - w) / 2.0, 634.0 + g));

        let (x, y) = scroll_hud_slot((1920.0, 34.0, 1920.0, 1080.0), origin, size);
        assert_eq!(y, 34.0 + 1080.0 - h - g);
        assert!(x >= 1920.0 + g && x + w <= 1920.0 + 1920.0 - g);
    }

    /// A display too narrow for the readout and both gaps used to cross `clamp`'s bounds, which
    /// panics — on the async task, so the capture would die with nothing to show for it.
    #[test]
    fn a_display_narrower_than_the_readout_places_it_instead_of_panicking() {
        let (w, _) = SCROLL_HUD_SIZE;
        let origin = (0.0, 0.0);
        for width in [200.0, 272.0, 285.0, 299.0] {
            let (x, _) = scroll_hud_slot((0.0, 0.0, width, 400.0), origin, (width, 600.0));
            assert!(x >= origin.0, "{width}pt display put the readout off the left edge");
            assert!(x <= origin.0 + SCROLL_HUD_GAP, "{width}pt display: {x} is not pinned left");
            let _ = w;
        }
    }
}
