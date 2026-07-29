//! Menu-bar (tray) presence. This is Glyphio's user-facing surface — engine's own tray is
//! disabled in the generated config, so only this one appears.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
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

/// The capture modes, in menu order: (menu-id stem, capture mode, label).
const MODES: [(&str, &str, &str); 7] = [
    ("visible", "visible", "Visible Area"),
    ("snip", "snip", "Region (Snip)"),
    ("full", "fullWindow", "Full Window"),
    ("front", "frontWindow", "Frontmost Window"),
    ("page", "pageOnly", "Browser Page"),
    ("scroll", "scrolling", "Scrolling Area"),
    ("scroll_page", "scrollingPage", "Scrolling Page"),
];

fn as_refs(items: &[MenuItem<tauri::Wry>]) -> Vec<&dyn tauri::menu::IsMenuItem<tauri::Wry>> {
    items.iter().map(|i| i as &dyn tauri::menu::IsMenuItem<tauri::Wry>).collect()
}

/// The mode a menu id stands for, and whether it was the silent copy of the menu.
fn menu_action(id: &str) -> Option<(&'static str, bool)> {
    let (silent, stem) = match id.strip_prefix("silent_") {
        Some(rest) => (true, rest),
        None => (false, id.strip_prefix("cap_")?),
    };
    MODES.iter().find(|(m, _, _)| *m == stem).map(|(_, mode, _)| (*mode, silent))
}

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    // Two menus over the same modes: one opens the editor, one goes straight to the
    // clipboard. Having both means silent capture is something you can reach for once,
    // rather than a mode you have to switch the app into and back out of.
    let editor_items = MODES
        .iter()
        .map(|(stem, _, label)| {
            MenuItem::with_id(app, format!("cap_{stem}"), *label, true, None::<&str>)
        })
        .collect::<tauri::Result<Vec<_>>>()?;
    let silent_items = MODES
        .iter()
        .map(|(stem, _, label)| {
            MenuItem::with_id(app, format!("silent_{stem}"), *label, true, None::<&str>)
        })
        .collect::<tauri::Result<Vec<_>>>()?;
    let capture_menu = Submenu::with_items(app, "Capture", true, &as_refs(&editor_items))?;
    let silent_menu =
        Submenu::with_items(app, "Capture to Clipboard", true, &as_refs(&silent_items))?;

    let search = MenuItem::with_id(app, "search", "Search Snippets…", true, None::<&str>)?;
    let clipboard = MenuItem::with_id(app, "clipboard", "Clipboard History…", true, None::<&str>)?;
    let history = MenuItem::with_id(app, "history", "History…", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Snippets & Settings…", true, None::<&str>)?;
    let reload = MenuItem::with_id(app, "reload", "Reload", true, None::<&str>)?;
    let quit = PredefinedMenuItem::quit(app, Some("Quit Glyphio"))?;

    let menu = Menu::with_items(
        app,
        &[
            &search,
            &capture_menu,
            &silent_menu,
            &PredefinedMenuItem::separator(app)?,
            &clipboard,
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
                id if menu_action(id).is_some() => {
                    let (mode, silent) = menu_action(id).expect("just matched");
                    let delivery = silent.then_some(crate::capture::Delivery::Silent);
                    crate::capture::trigger_or_report(&inner, mode, delivery);
                }
                "history" => { let _ = crate::commands::open_history_view(inner.clone()); }
                "settings" => { let _ = crate::windows::open(&inner, "settings"); }
                "search" => { let _ = crate::windows::toggle_palette(&inner); }
                "clipboard" => { let _ = crate::windows::toggle_clipboard(&inner); }
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
