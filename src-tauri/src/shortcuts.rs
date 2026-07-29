//! Global capture hotkeys (Tauri replacement for Checkpoint's `chrome.commands`).
//!
//! The plugin is built once in `lib.rs` with [`handler`]; accelerators are (re)registered from
//! the current settings by [`register`], so editing a shortcut in Settings takes effect live.

use std::str::FromStr;

use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutEvent, ShortcutState};

use crate::AppState;

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
