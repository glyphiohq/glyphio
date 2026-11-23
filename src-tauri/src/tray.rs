//! Menu-bar (tray) presence. This is Glyphio's user-facing surface — engine's own tray is
//! disabled in the generated config, so only this one appears.

use tauri::menu::{IconMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager};

use crate::capture::lifecycle::{
    Action, CaptureInProgress, CaptureMode, CaptureToken, DeadlineCheck, Presentation,
    ResultRoute,
};
use crate::capture::orchestration::{CaptureOrchestration, DeliveryOutcome};
use crate::capture::{delivery::DeliveryRoute, PendingCapture};

const TRAY_ID: &str = "glyphio-tray";
const CAPTURE_STATUS_ID: &str = "capture-status";
const FINISH_SETUP_ID: &str = "finish-setup";
const FEEDBACK_DURATION: std::time::Duration = std::time::Duration::from_millis(3200);

/// Capture feedback's platform-independent state plus the native menu row that exposes the
/// current result. The icon/title and tooltip are updated from the same presentation.
#[derive(Default)]
pub struct CaptureFeedback {
    orchestration: CaptureOrchestration<PendingCapture>,
    status_item: Option<MenuItem<tauri::Wry>>,
    /// A shortcut binding was refused or invalid. Shown only while nothing else is going on.
    shortcut_problem: bool,
    /// The menu and its "Finish setup" row, which sits at the top only while a permission is
    /// unresolved (menus can't hide a row, so it is inserted and removed).
    setup_row: Option<(Menu<tauri::Wry>, MenuItem<tauri::Wry>)>,
    setup_row_shown: bool,
}

/// Show the "Finish setup" row while a permission is unresolved, and remove it once both are
/// granted. Cheap to call repeatedly: it only touches the menu on a change.
pub fn set_setup_unresolved(app: &AppHandle, unresolved: bool) {
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        let state = app.state::<crate::AppState>();
        let mut feedback = state.capture_feedback.lock().unwrap();
        if feedback.setup_row_shown == unresolved {
            return;
        }
        let Some((menu, item)) = feedback.setup_row.clone() else {
            return;
        };
        let changed = if unresolved { menu.insert(&item, 0) } else { menu.remove(&item) };
        match changed {
            Ok(()) => feedback.setup_row_shown = unresolved,
            Err(e) => log::warn!("could not update the Finish setup row: {e}"),
        }
    });
}

fn presentation(app: &AppHandle) -> Presentation {
    let state = app.state::<crate::AppState>();
    let feedback = state.capture_feedback.lock().unwrap();
    with_shortcut_notice(feedback.orchestration.presentation(), feedback.shortcut_problem)
}

/// The resting presentation, carrying the shortcut notice if there is one. A capture in
/// progress, or its result, always takes precedence.
fn with_shortcut_notice(mut value: Presentation, problem: bool) -> Presentation {
    if problem && value.title.is_empty() {
        value.tooltip = "Glyphio — Some shortcuts could not be registered — open Settings".into();
        value.menu_text = "Some shortcuts could not be registered — open Settings".into();
    }
    value
}

/// Record whether any shortcut binding failed, and repaint the menu-bar item.
pub fn set_shortcut_problem(app: &AppHandle, problem: bool) {
    app.state::<crate::AppState>()
        .capture_feedback
        .lock()
        .unwrap()
        .shortcut_problem = problem;
    apply(app, presentation(app));
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
            .orchestration
            .expire(revision);
        if expired {
            apply(&app, presentation(&app));
        }
    });
}

/// Fail the capture `token` if it outlives its deadline. Sleeps until the deadline the
/// lifecycle reports, then asks again, so heartbeats that moved it are honoured. It ends as
/// soon as the token is stale, so a watchdog never touches a later capture.
fn watch_deadline(app: &AppHandle, token: CaptureToken) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            let check = app
                .state::<crate::AppState>()
                .capture_feedback
                .lock()
                .unwrap()
                .orchestration
                .check_deadline(token, std::time::Instant::now());
            match check {
                DeadlineCheck::Stale => return,
                DeadlineCheck::Pending(remaining) => tokio::time::sleep(remaining).await,
                DeadlineCheck::Expired {
                    revision,
                    mode,
                    phase,
                } => {
                    log::error!(
                        "capture ({}) timed out while {phase}; the lifecycle was reset",
                        mode.as_str()
                    );
                    apply(&app, presentation(&app));
                    schedule_reset(&app, revision);
                    return;
                }
            }
        }
    });
}

/// A scrolling capture produced another frame, so it is alive: extend its deadline.
pub fn capture_heartbeat(app: &AppHandle) {
    app.state::<crate::AppState>()
        .capture_feedback
        .lock()
        .unwrap()
        .orchestration
        .heartbeat();
}

/// Show a scrolling capture's progress in the menu bar.
pub fn capture_progress(app: &AppHandle, text: String) {
    app.state::<crate::AppState>()
        .capture_feedback
        .lock()
        .unwrap()
        .orchestration
        .set_progress(text);
    apply(app, presentation(app));
}

/// Record why a scrolling capture ended early, so its result says so.
pub fn capture_result_note(app: &AppHandle, text: String) {
    app.state::<crate::AppState>()
        .capture_feedback
        .lock()
        .unwrap()
        .orchestration
        .set_result_note(text);
}

/// Return the menu-bar capture state to idle, abandoning any stuck capture. Wired to Reload.
pub fn capture_reset(app: &AppHandle) {
    app.state::<crate::AppState>()
        .capture_feedback
        .lock()
        .unwrap()
        .orchestration
        .reset();
    apply(app, presentation(app));
}

/// Begin the one capture currently represented by the menu-bar icon.
pub fn capture_started(
    app: &AppHandle,
    mode: CaptureMode,
) -> Result<CaptureToken, CaptureInProgress> {
    let result = app
        .state::<crate::AppState>()
        .capture_feedback
        .lock()
        .unwrap()
        .orchestration
        .begin(mode);
    // `begin` also records a duplicate-request notice without replacing the active capture.
    // Apply it on both paths so the ignored hotkey is visible without activating Glyphio.
    apply(app, presentation(app));
    if let Ok(token) = &result {
        watch_deadline(app, *token);
    }
    result
}

pub fn active_capture(app: &AppHandle) -> Option<CaptureToken> {
    app.state::<crate::AppState>()
        .capture_feedback
        .lock()
        .unwrap()
        .orchestration
        .active_token()
}

/// Register a completed result and tie its exact-once session to the active icon state.
pub fn capture_completed(
    app: &AppHandle,
    token: CaptureToken,
    route: DeliveryRoute,
    pending: PendingCapture,
) -> anyhow::Result<crate::capture::delivery::DeliverySessionId> {
    let session = app
        .state::<crate::AppState>()
        .capture_feedback
        .lock()
        .unwrap()
        .orchestration
        .complete(token, route, pending)
        .ok_or_else(|| anyhow::anyhow!("capture completed after its lifecycle had already ended"))?;
    // Delivery has a much shorter budget than selection; the watchdog started at `begin` may be
    // asleep until the old deadline, so start one that wakes for the new one. Whichever finds
    // the capture expired fails it; the other sees a stale token.
    watch_deadline(app, token);
    Ok(session)
}

pub fn take_pending_capture(
    app: &AppHandle,
    session_id: &crate::capture::delivery::DeliverySessionId,
    route: DeliveryRoute,
) -> Option<PendingCapture> {
    app.state::<crate::AppState>()
        .capture_feedback
        .lock()
        .unwrap()
        .orchestration
        .consume(session_id, route)
}

pub fn capture_delivery_finished(
    app: &AppHandle,
    session_id: &str,
    silent: bool,
    history_id: Option<String>,
    error: Option<String>,
) {
    let result_route = if silent {
        history_id
            .clone()
            .map(ResultRoute::History)
            .unwrap_or(ResultRoute::Editor)
    } else {
        ResultRoute::Editor
    };
    let delivered = error.is_none();
    let outcome = match error {
        Some(message) => DeliveryOutcome::Failed {
            message,
            recovery: history_id.map(ResultRoute::History),
        },
        None => DeliveryOutcome::Succeeded {
            route: result_route,
        },
    };
    let revision = app
        .state::<crate::AppState>()
        .capture_feedback
        .lock()
        .unwrap()
        .orchestration
        .finish_delivery(session_id, outcome);
    if let Some(revision) = revision {
        // The setup window's "try it" step completes on this, and only this: a capture that
        // reached the editor or History, not a shortcut that merely fired.
        if delivered {
            let _ = app.emit("capture-delivered", ());
        }
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
        .orchestration
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
        .orchestration
        .fail_current(message);
    if let Some(revision) = revision {
        apply(app, presentation(app));
        schedule_reset(app, revision);
    }
}

/// The user backed out of capture `token` (Esc in the picker): clear the indicator quietly.
pub fn capture_cancelled_for(app: &AppHandle, token: CaptureToken) {
    let changed = app
        .state::<crate::AppState>()
        .capture_feedback
        .lock()
        .unwrap()
        .orchestration
        .cancel(token);
    if changed {
        apply(app, presentation(app));
    }
}

/// The scrolling selection overlay produced a rectangle and is about to close to start the
/// capture; its closing must not be mistaken for an abandoned selection.
pub fn capture_selection_made(app: &AppHandle, token: CaptureToken) {
    app.state::<crate::AppState>()
        .capture_feedback
        .lock()
        .unwrap()
        .orchestration
        .selection_made(token);
}

/// The scrolling selection overlay is gone without a rectangle. Cancels only a capture that
/// is still waiting on it.
pub fn capture_selection_abandoned(app: &AppHandle) {
    let changed = app
        .state::<crate::AppState>()
        .capture_feedback
        .lock()
        .unwrap()
        .orchestration
        .cancel_selection();
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
        .orchestration
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
        .orchestration
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

    let finish_setup = MenuItem::with_id(app, FINISH_SETUP_ID, "Finish setup…", true, None::<&str>)?;

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

    {
        let state = app.state::<crate::AppState>();
        let mut feedback = state.capture_feedback.lock().unwrap();
        feedback.setup_row = Some((menu.clone(), finish_setup));
        feedback.setup_row_shown = false;
    }

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .tooltip("Glyphio")
        .on_menu_event(|app, event| {
            let id = event.id().as_ref().to_string();
            let inner = app.clone();
            let _ = app.run_on_main_thread(move || match id.as_str() {
                CAPTURE_STATUS_ID => activate_capture_status(&inner),
                FINISH_SETUP_ID => {
                    if let Err(e) = crate::windows::open_setup(&inner) {
                        log::error!("could not open setup: {e:#}");
                    }
                }
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
