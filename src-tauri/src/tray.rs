//! Menu-bar (tray) presence. This is Glyphio's user-facing surface — engine's own tray is
//! disabled in the generated config, so only this one appears.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::TrayIconBuilder;
use tauri::AppHandle;

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let cap_visible = MenuItem::with_id(app, "cap_visible", "Capture Visible Area", true, None::<&str>)?;
    let cap_snip = MenuItem::with_id(app, "cap_snip", "Capture Region (Snip)", true, None::<&str>)?;
    let cap_full = MenuItem::with_id(app, "cap_full", "Capture Full Window", true, None::<&str>)?;
    let cap_front = MenuItem::with_id(app, "cap_front", "Capture Frontmost Window", true, None::<&str>)?;
    let cap_scroll = MenuItem::with_id(app, "cap_scroll", "Capture Scrolling Area", true, None::<&str>)?;
    let cap_scroll_page = MenuItem::with_id(app, "cap_scroll_page", "Capture Scrolling Page", true, None::<&str>)?;
    let capture_menu = Submenu::with_items(app, "Capture", true, &[&cap_visible, &cap_snip, &cap_full, &cap_front, &cap_scroll, &cap_scroll_page])?;

    let history = MenuItem::with_id(app, "history", "History…", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Snippets & Settings…", true, None::<&str>)?;
    let quit = PredefinedMenuItem::quit(app, Some("Quit Glyphio"))?;

    let menu = Menu::with_items(
        app,
        &[
            &capture_menu,
            &PredefinedMenuItem::separator(app)?,
            &history,
            &settings,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    let mut builder = TrayIconBuilder::with_id("glyphio-tray")
        .menu(&menu)
        .tooltip("Glyphio")
        .on_menu_event(|app, event| {
            let id = event.id().as_ref().to_string();
            let inner = app.clone();
            let _ = app.run_on_main_thread(move || match id.as_str() {
                "cap_visible" => { let _ = crate::capture::trigger(&inner, "visible"); }
                "cap_snip" => { let _ = crate::capture::trigger(&inner, "snip"); }
                "cap_scroll" => { let _ = crate::capture::trigger(&inner, "scrolling"); }
                "cap_scroll_page" => { let _ = crate::capture::trigger(&inner, "scrollingPage"); }
                "cap_full" => { let _ = crate::capture::trigger(&inner, "fullWindow"); }
                "cap_front" => { let _ = crate::capture::trigger(&inner, "frontWindow"); }
                "history" => { let _ = crate::commands::open_history_view(inner.clone()); }
                "settings" => { let _ = crate::windows::open(&inner, "settings"); }
                _ => {}
            });
        });

    // Use the app's bundle icon as the tray icon if available.
    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon).icon_as_template(true);
    }
    builder.build(app)?;
    Ok(())
}
