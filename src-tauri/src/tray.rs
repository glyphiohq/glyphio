//! Menu-bar (tray) presence. This is Glyphio's user-facing surface — engine's own tray is
//! disabled in the generated config, so only this one appears.

use tauri::menu::{IconMenuItem, Menu, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::AppHandle;

const TRAY_ID: &str = "glyphio-tray";

/// Acknowledge a capture that opened no window: a checkmark beside the menu-bar icon for a
/// moment. Silent captures need *some* answer — a shortcut that copies to the clipboard and
/// shows nothing is indistinguishable from one that didn't fire.
pub fn flash_captured(app: &AppHandle) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else { return };
    let _ = tray.set_title(Some("✓"));
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
        let inner = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Some(tray) = inner.tray_by_id(TRAY_ID) {
                // An empty title, not `None`: clearing it with `None` leaves the checkmark
                // sitting in the menu bar for good.
                let _ = tray.set_title(Some(""));
            }
        });
    });
}

/// Brass, not black, and not left to macOS to tint.
///
/// `muda` sizes a menu item's image to 18pt but does not mark it as a template image, so the
/// system never recolours it for the current appearance: a black glyph vanishes into a dark
/// menu and a white one into a light menu. Glyphio's own brass has contrast against both, and
/// looks deliberate rather than like a monochrome icon that failed to tint.
fn menu_icon(bytes: &'static [u8]) -> Option<tauri::image::Image<'static>> {
    match tauri::image::Image::from_bytes(bytes) {
        Ok(image) => Some(image),
        Err(e) => {
            log::warn!("a menu icon failed to decode: {e}");
            None // an item with no icon still works; a missing menu does not
        }
    }
}

/// The menu bar holds one way in, not fourteen.
///
/// It used to carry every capture mode twice — once for the editor, once for the clipboard —
/// a wall of near-identical rows to read every time you wanted any of them. All of it lives in
/// the palette now, where the list is searchable and ⌘↩ is the clipboard variant of whatever is
/// selected, so the menu's job is just to be the discoverable way to summon it for anyone who
/// hasn't learned ⌥Space yet.
pub fn build(app: &AppHandle) -> tauri::Result<()> {
    // No accelerator text and no ellipses: four rows that read the same way. The shortcut is
    // in Settings and on the palette itself, which is where someone looks for it a second
    // time; spelling it here only made this one row longer than its neighbours.
    let item = |id: &str, text: &str, png: &'static [u8]| {
        IconMenuItem::with_id(app, id, text, true, menu_icon(png), None::<&str>)
    };
    let open = item("open", "Search", include_bytes!("../icons/menu/menu-search.png"))?;
    let history = item("history", "History", include_bytes!("../icons/menu/menu-history.png"))?;
    let settings = item(
        "settings",
        "Snippets & Settings",
        include_bytes!("../icons/menu/menu-settings.png"),
    )?;
    let reload = item("reload", "Reload", include_bytes!("../icons/menu/menu-reload.png"))?;
    let quit = PredefinedMenuItem::quit(app, Some("Quit Glyphio"))?;

    let menu = Menu::with_items(
        app,
        &[
            &open,
            &PredefinedMenuItem::separator(app)?,
            &history,
            &settings,
            &PredefinedMenuItem::separator(app)?,
            &reload,
            &quit,
        ],
    )?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .tooltip("Glyphio")
        .on_menu_event(|app, event| {
            let id = event.id().as_ref().to_string();
            let inner = app.clone();
            let _ = app.run_on_main_thread(move || match id.as_str() {
                "open" => { let _ = crate::windows::toggle_palette(&inner, None); }
                "history" => { let _ = crate::commands::open_history_view(inner.clone()); }
                "settings" => { let _ = crate::windows::open(&inner, "settings"); }
                "reload" => {
                    if let Err(e) = crate::commands::do_reload(&inner) {
                        log::error!("reload failed: {e}");
                    }
                }
                _ => {}
            });
        });

    // Dedicated monochrome menu-bar mark (capture frame + text caret). A template image is
    // black+alpha; macOS tints it for the light/dark menu bar. Falling back to the full app
    // icon (a colour square) as a template just renders a black blob, so prefer the bundled
    // tray.png and only fall back if it fails to decode.
    match tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png")) {
        Ok(icon) => { builder = builder.icon(icon).icon_as_template(true); }
        Err(e) => {
            log::warn!("tray icon decode failed ({e}); using app icon");
            if let Some(icon) = app.default_window_icon().cloned() {
                builder = builder.icon(icon).icon_as_template(true);
            }
        }
    }
    builder.build(app)?;
    Ok(())
}
