//! Global capture hotkeys (Tauri replacement for Checkpoint's `chrome.commands`).
//!
//! The plugin is built once in `lib.rs` with [`handler`]; accelerators are (re)registered from
//! the current settings by [`register`], so editing a shortcut in Settings takes effect live.

use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutEvent, ShortcutState};

use crate::AppState;

/// The one key a running scrolling capture borrows.
const STOP_KEY: &str = "Escape";
/// Whether [`STOP_KEY`] is currently ours. Escape belongs to whatever is in front, so it is
/// only registered while a capture is scrolling — and this flag makes sure a user who has bound
/// Escape to something of their own still gets their binding the rest of the time.
static STOP_KEY_ARMED: AtomicBool = AtomicBool::new(false);

/// Borrow Escape for the duration of a scrolling capture — the one way to stop one early.
///
/// Best-effort, and worth logging when it fails: if the system won't hand Escape over, the
/// capture can only run to the bottom of the content or to the frame cap.
pub fn arm_stop_key(app: &AppHandle) {
    match Shortcut::from_str(STOP_KEY) {
        Ok(sc) => match app.global_shortcut().register(sc) {
            Ok(()) => STOP_KEY_ARMED.store(true, Ordering::SeqCst),
            Err(e) => log::warn!("could not take {STOP_KEY} for the scrolling capture: {e}"),
        },
        Err(e) => log::warn!("invalid stop accelerator {STOP_KEY:?}: {e}"),
    }
}

/// Give Escape back.
pub fn release_stop_key(app: &AppHandle) {
    if !STOP_KEY_ARMED.swap(false, Ordering::SeqCst) {
        return;
    }
    if let Ok(sc) = Shortcut::from_str(STOP_KEY) {
        if let Err(e) = app.global_shortcut().unregister(sc) {
            log::warn!("could not give {STOP_KEY} back: {e}");
        }
    }
}

/// (Re)register all configured accelerators.
pub fn register(app: &AppHandle) -> anyhow::Result<()> {
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    let s = app.state::<AppState>().settings.lock().unwrap().clone();
    let captures = s.capture_shortcuts();
    for acc in captures
        .iter()
        .map(|(acc, _, _)| *acc)
        .chain([
            s.shortcut_open_history.as_str(),
            s.shortcut_open_palette.as_str(),
            s.shortcut_open_clipboard.as_str(),
        ])
    {
        if acc.is_empty() {
            continue;
        }
        match Shortcut::from_str(acc) {
            Ok(sc) => {
                if let Err(e) = gs.register(sc) {
                    log::warn!("could not register shortcut {acc}: {e}");
                }
            }
            Err(e) => log::warn!("invalid shortcut accelerator {acc:?}: {e}"),
        }
    }
    // `unregister_all` above dropped Escape too, if a scrolling capture had borrowed it —
    // saving settings while one is running would otherwise leave that capture with no way to
    // stop, and `STOP_KEY_ARMED` still claiming otherwise.
    if STOP_KEY_ARMED.load(Ordering::SeqCst) {
        if let Ok(sc) = Shortcut::from_str(STOP_KEY) {
            if let Err(e) = gs.register(sc) {
                log::warn!("could not take {STOP_KEY} back for the running capture: {e}");
            }
        }
    }
    Ok(())
}

fn matches(pressed: &Shortcut, acc: &str) -> bool {
    Shortcut::from_str(acc).map(|s| &s == pressed).unwrap_or(false)
}

/// Global handler installed on the plugin; dispatches a fired shortcut to its action.
pub fn handler(app: &AppHandle, shortcut: &Shortcut, event: ShortcutEvent) {
    if event.state() != ShortcutState::Pressed {
        return;
    }
    // Checked before anything configured: while a scrolling capture is running, Escape means
    // "stop now and keep the frames" and nothing else.
    if STOP_KEY_ARMED.load(Ordering::SeqCst) && matches(shortcut, STOP_KEY) {
        crate::capture::scroll::request_stop();
        return;
    }
    let s = app.state::<AppState>().settings.lock().unwrap().clone();
    let app = app.clone();
    // A silent twin is only a different delivery for the same mode, so one table answers
    // both. First match wins: a key configured twice does the thing listed first.
    let fired = s
        .capture_shortcuts()
        .iter()
        .find(|(acc, _, _)| !acc.is_empty() && matches(shortcut, acc))
        .map(|(_, mode, silent)| (*mode, *silent));
    if let Some((mode, silent)) = fired {
        dispatch_capture(app, mode, silent);
    } else if matches(shortcut, &s.shortcut_open_history) {
        let inner = app.clone();
        let _ = app.run_on_main_thread(move || {
            let _ = crate::commands::open_history_view(inner.clone());
        });
    } else if matches(shortcut, &s.shortcut_open_palette) {
        let inner = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Err(e) = crate::windows::toggle_palette(&inner, None) {
                log::error!("palette failed: {e}");
            }
        });
    } else if matches(shortcut, &s.shortcut_open_clipboard) {
        let inner = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Err(e) = crate::windows::toggle_palette(&inner, Some("clipboard")) {
                log::error!("palette failed: {e}");
            }
        });
    }
}

fn dispatch_capture(app: AppHandle, mode: &'static str, silent: bool) {
    let inner = app.clone();
    let _ = app.run_on_main_thread(move || {
        let delivery = silent.then_some(crate::capture::Delivery::Silent);
        crate::capture::trigger_or_report(&inner, mode, delivery);
    });
}
