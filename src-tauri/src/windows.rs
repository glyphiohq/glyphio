//! Webview window management. Each surface is a static HTML page under `ui/`.

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

struct Spec {
    url: &'static str,
    title: &'static str,
    width: f64,
    height: f64,
}

fn spec(name: &str) -> Option<Spec> {
    Some(match name {
        // The main index is the Settings / snippet-manager surface.
        "settings" => Spec { url: "index.html", title: "Glyphio", width: 940.0, height: 660.0 },
        "editor" => Spec { url: "editor/index.html", title: "Glyphio — Edit Capture", width: 1100.0, height: 820.0 },
        _ => return None,
    })
}

/// Open a stored capture (by id) in the editor's read-only view mode. Uses a per-capture window
/// label so it never clashes with the live-capture editor window.
pub fn open_capture(app: &AppHandle, id: &str) -> anyhow::Result<()> {
    let label = format!("capture-{id}");
    if let Some(win) = app.get_webview_window(&label) {
        win.show()?;
        win.set_focus()?;
        return Ok(());
    }
    let url = format!("editor/index.html?history={id}");
    WebviewWindowBuilder::new(app, &label, WebviewUrl::App(url.into()))
        .title("Glyphio — Capture")
        .inner_size(1100.0, 820.0)
        .min_inner_size(640.0, 480.0)
        .build()?;
    Ok(())
}

/// Open (or focus, if already open) a named window.
pub fn open(app: &AppHandle, name: &str) -> anyhow::Result<()> {
    if let Some(win) = app.get_webview_window(name) {
        win.show()?;
        win.set_focus()?;
        return Ok(());
    }
    let spec = spec(name).ok_or_else(|| anyhow::anyhow!("unknown window: {name}"))?;
    WebviewWindowBuilder::new(app, name, WebviewUrl::App(spec.url.into()))
        .title(spec.title)
        .inner_size(spec.width, spec.height)
        .min_inner_size(640.0, 480.0)
        .build()?;
    Ok(())
}

/// Open the transparent scrolling-capture selection overlay, sized to the display under the
/// cursor. Label `scroll-overlay`; it closes itself via the run/cancel commands.
pub fn open_scroll_overlay(app: &AppHandle) -> anyhow::Result<()> {
    if let Some(win) = app.get_webview_window("scroll-overlay") {
        win.close().ok(); // stale one — restart fresh
    }
    let (origin, size) = crate::capture::display_bounds_under_cursor();
    let win = WebviewWindowBuilder::new(
        app,
        "scroll-overlay",
        WebviewUrl::App("scroll-overlay/index.html".into()),
    )
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
