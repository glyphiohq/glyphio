//! Menu-bar (tray) presence. This is Glyphio's user-facing surface — engine's own tray is
//! disabled in the generated config, so only this one appears.

use tauri::menu::{IconMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager};

use crate::capture::lifecycle::{
    Action, CaptureInProgress, CaptureToken, Lifecycle, Presentation, ResultRoute,
};

const TRAY_ID: &str = "glyphio-tray";
const CAPTURE_STATUS_ID: &str = "capture-status";
const FEEDBACK_DURATION: std::time::Duration = std::time::Duration::from_millis(3200);

/// Capture feedback's platform-independent state plus the native menu row that exposes the
/// current result. The icon/title and tooltip are updated from the same presentation.
#[derive(Default)]
pub struct CaptureFeedback {
    lifecycle: Lifecycle,
    status_item: Option<MenuItem<tauri::Wry>>,
}

fn presentation(app: &AppHandle) -> Presentation {
    app.state::<crate::AppState>()
        .capture_feedback
        .lock()
        .unwrap()
        .lifecycle
        .presentation()
}

fn apply(app: &AppHandle, value: Presentation) {
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        let Some(tray) = app.tray_by_id(TRAY_ID) else {
            return;
        };
        let _ = tray.set_title(Some(value.title));
        let _ = tray.set_tooltip(Some(&value.tooltip));
        let item = app
            .state::<crate::AppState>()
            .capture_feedback
            .lock()
            .unwrap()
            .status_item
            .clone();
        if let Some(item) = item {
            let _ = item.set_text(&value.menu_text);
            let _ = item.set_enabled(value.menu_enabled);
        }
    });
}

fn schedule_reset(app: &AppHandle, revision: u64) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FEEDBACK_DURATION).await;
        let expired = app
            .state::<crate::AppState>()
            .capture_feedback
            .lock()
            .unwrap()
            .lifecycle
            .expire(revision);
        if expired {
            apply(&app, presentation(&app));
        }
    });
}

/// Begin the one capture currently represented by the menu-bar icon.
pub fn capture_started(
    app: &AppHandle,
    mode: &str,
) -> Result<CaptureToken, CaptureInProgress> {
    let token = app
        .state::<crate::AppState>()
        .capture_feedback
        .lock()
        .unwrap()
        .lifecycle
        .begin(mode)?;
    apply(app, presentation(app));
    Ok(token)
}

pub fn active_capture(app: &AppHandle) -> Option<CaptureToken> {
    app.state::<crate::AppState>()
        .capture_feedback
        .lock()
        .unwrap()
        .lifecycle
        .active_token()
}

/// Tie the active icon state to the editor/worker session that will acknowledge delivery.
pub fn capture_delivery_started(app: &AppHandle, token: CaptureToken, session_id: &str) {
    app.state::<crate::AppState>()
        .capture_feedback
        .lock()
        .unwrap()
        .lifecycle
        .bind_delivery(token, session_id);
}

pub fn capture_delivery_finished(
    app: &AppHandle,
    session_id: &str,
    silent: bool,
    history_id: Option<String>,
    error: Option<String>,
) {
    let route = if silent {
        history_id
            .clone()
            .map(ResultRoute::History)
            .unwrap_or(ResultRoute::Editor)
    } else {
        ResultRoute::Editor
    };
    let recovery = error.as_ref().and(history_id.map(ResultRoute::History));
    let revision = app
        .state::<crate::AppState>()
        .capture_feedback
        .lock()
        .unwrap()
        .lifecycle
        .finish_delivery(session_id, route, error, recovery);
    if let Some(revision) = revision {
        apply(app, presentation(app));
        schedule_reset(app, revision);
    }
}

pub fn capture_failed(app: &AppHandle, token: CaptureToken, message: String) {
    let revision = app
        .state::<crate::AppState>()
        .capture_feedback
        .lock()
        .unwrap()
        .lifecycle
        .fail(token, message);
    if let Some(revision) = revision {
        apply(app, presentation(app));
        schedule_reset(app, revision);
    }
}

pub fn capture_failed_current(app: &AppHandle, message: String) {
    let revision = app
        .state::<crate::AppState>()
        .capture_feedback
        .lock()
        .unwrap()
        .lifecycle
        .fail_current(message);
    if let Some(revision) = revision {
        apply(app, presentation(app));
        schedule_reset(app, revision);
    }
}

pub fn capture_cancelled(app: &AppHandle) {
    let changed = app
        .state::<crate::AppState>()
        .capture_feedback
        .lock()
        .unwrap()
        .lifecycle
        .cancel_current();
    if changed {
        apply(app, presentation(app));
    }
}

/// Acknowledge something that finished without opening a window: a checkmark beside the
/// menu-bar icon for a moment. A shortcut that copies to the clipboard and shows nothing is
/// indistinguishable from one that didn't fire.
pub fn flash_ack(app: &AppHandle) {
    let revision = app
        .state::<crate::AppState>()
        .capture_feedback
        .lock()
        .unwrap()
        .lifecycle
        .acknowledge();
    if let Some(revision) = revision {
        apply(app, presentation(app));
        schedule_reset(app, revision);
    }
}

fn activate_capture_status(app: &AppHandle) {
    let action = app
        .state::<crate::AppState>()
        .capture_feedback
        .lock()
        .unwrap()
        .lifecycle
        .presentation()
        .action;
    match action {
        Action::None => {}
        Action::OpenResult(ResultRoute::Editor) => {
            if let Err(e) = crate::windows::reveal_capture_editor(app) {
                log::warn!("could not reveal capture result: {e}");
            }
        }
        Action::OpenResult(ResultRoute::History(id)) => {
            if let Err(e) = crate::windows::open_capture(app, &id) {
                log::warn!("could not open saved capture: {e}");
            }
        }
        Action::ShowError(message) => {
            use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
            app.dialog()
                .message(message)
                .title("Glyphio — capture failed")
                .kind(MessageDialogKind::Error)
                .show(|_| {});
        }
    }
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
    let capture_status = MenuItem::with_id(
        app,
        CAPTURE_STATUS_ID,
        "Capture status: Ready",
        false,
        None::<&str>,
    )?;

    app.state::<crate::AppState>()
        .capture_feedback
        .lock()
        .unwrap()
        .status_item = Some(capture_status.clone());

    let menu = Menu::with_items(
        app,
        &[
            &capture_status,
            &PredefinedMenuItem::separator(app)?,
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
                CAPTURE_STATUS_ID => activate_capture_status(&inner),
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
