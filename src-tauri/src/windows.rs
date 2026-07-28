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
        use std::sync::atomic::{AtomicBool, Ordering};
        // The policy transition is visible (the Dock icon appears/disappears), so only apply
        // it on an actual change — set_activation_policy on every window event flickers.
        static IS_REGULAR: AtomicBool = AtomicBool::new(false);

        let any_visible = app.webview_windows().iter().any(|(label, win)| {
            is_main_window(label)
                && Some(label.as_str()) != ignoring
                && win.is_visible().unwrap_or(false)
        });
        if any_visible == IS_REGULAR.swap(any_visible, Ordering::SeqCst) {
            return;
        }
        let policy = if any_visible {
            tauri::ActivationPolicy::Regular
        } else {
            tauri::ActivationPolicy::Accessory
        };
        if let Err(e) = app.set_activation_policy(policy) {
            log::warn!("could not set activation policy: {e}");
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, ignoring);
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
#[cfg(target_os = "macos")]
fn follow_user_across_spaces(win: &WebviewWindow) {
    use objc2_app_kit::{NSWindow, NSWindowCollectionBehavior};

    let Ok(ptr) = win.ns_window() else { return };
    if ptr.is_null() {
        return;
    }
    // Safety: `ns_window` hands back this window's live NSWindow, and we only touch it on the
    // main thread — every caller is already there (window creation and `present`).
    unsafe {
        let ns: &NSWindow = &*(ptr as *const NSWindow);
        let behavior = ns.collectionBehavior() | NSWindowCollectionBehavior::MoveToActiveSpace;
        ns.setCollectionBehavior(behavior);
    }
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
        .build()?;
    win.set_focus()?;
    Ok(())
}

/// Toggle the Spotlight-style snippet palette: a frameless, always-on-top search window.
/// Hidden (not destroyed) on dismiss so summoning it again is instant; the page refreshes
/// its snippet list on every `palette-show`.
pub fn toggle_palette(app: &AppHandle) -> anyhow::Result<()> {
    use tauri::Emitter;
    // Summon on the display the user is working on (frontmost window's display), never
    // wherever the palette last was — Alt+Space should feel local, like Spotlight.
    const PALETTE_SIZE: (f64, f64) = (640.0, 460.0);
    let (origin, display) = crate::capture::display_bounds_for_active_window();
    let pos = (
        origin.0 + (display.0 - PALETTE_SIZE.0) / 2.0,
        // Slightly above centre, Spotlight-style; also keeps result rows from spilling
        // off-screen as the list grows downward.
        origin.1 + (display.1 - PALETTE_SIZE.1) / 3.0,
    );
    if let Some(win) = app.get_webview_window("palette") {
        if win.is_visible().unwrap_or(false) && win.is_focused().unwrap_or(false) {
            win.hide()?;
        } else {
            win.set_position(tauri::LogicalPosition::new(pos.0, pos.1))?;
            win.show()?;
            win.set_focus()?;
            let _ = app.emit_to("palette", "palette-show", ());
        }
        return Ok(());
    }
    let win = WebviewWindowBuilder::new(
        app,
        "palette",
        WebviewUrl::App("palette/index.html".into()),
    )
    .title("Glyphio Search")
    .inner_size(PALETTE_SIZE.0, PALETTE_SIZE.1)
    .position(pos.0, pos.1)
    .decorations(false)
    .transparent(true)
    .always_on_top(true)
    .skip_taskbar(true)
    .resizable(false)
    .visible_on_all_workspaces(true)
    .build()?;
    win.set_focus()?;
    Ok(())
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
    .build()?;
    win.set_focus()?;
    Ok(())
}

/// Close the selection overlay if present.
pub fn close_scroll_overlay(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("scroll-overlay") {
        let _ = win.close();
    }
}
