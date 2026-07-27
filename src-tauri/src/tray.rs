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
                let _ = tray.set_title(None::<&str>);
            }
        });
    });
}

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let cap_visible = MenuItem::with_id(app, "cap_visible", "Capture Visible Area", true, None::<&str>)?;
    let cap_snip = MenuItem::with_id(app, "cap_snip", "Capture Region (Snip)", true, None::<&str>)?;
    let cap_full = MenuItem::with_id(app, "cap_full", "Capture Full Window", true, None::<&str>)?;
    let cap_front = MenuItem::with_id(app, "cap_front", "Capture Frontmost Window", true, None::<&str>)?;
    let cap_page = MenuItem::with_id(app, "cap_page", "Capture Browser Page", true, None::<&str>)?;
    let cap_scroll = MenuItem::with_id(app, "cap_scroll", "Capture Scrolling Area", true, None::<&str>)?;
    let cap_scroll_page = MenuItem::with_id(app, "cap_scroll_page", "Capture Scrolling Page", true, None::<&str>)?;
    let capture_menu = Submenu::with_items(app, "Capture", true, &[&cap_visible, &cap_snip, &cap_full, &cap_front, &cap_page, &cap_scroll, &cap_scroll_page])?;

    let search = MenuItem::with_id(app, "search", "Search Snippets…", true, None::<&str>)?;
    let history = MenuItem::with_id(app, "history", "History…", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Snippets & Settings…", true, None::<&str>)?;
    let reload = MenuItem::with_id(app, "reload", "Reload", true, None::<&str>)?;
    let quit = PredefinedMenuItem::quit(app, Some("Quit Glyphio"))?;

    let menu = Menu::with_items(
        app,
        &[
            &search,
            &capture_menu,
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
                "cap_visible" => crate::capture::trigger_or_report(&inner, "visible"),
                "cap_snip" => crate::capture::trigger_or_report(&inner, "snip"),
                "cap_scroll" => crate::capture::trigger_or_report(&inner, "scrolling"),
                "cap_scroll_page" => crate::capture::trigger_or_report(&inner, "scrollingPage"),
                "cap_full" => crate::capture::trigger_or_report(&inner, "fullWindow"),
                "cap_front" => crate::capture::trigger_or_report(&inner, "frontWindow"),
                "cap_page" => crate::capture::trigger_or_report(&inner, "pageOnly"),
                "history" => { let _ = crate::commands::open_history_view(inner.clone()); }
                "settings" => { let _ = crate::windows::open(&inner, "settings"); }
                "search" => { let _ = crate::windows::toggle_palette(&inner); }
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
