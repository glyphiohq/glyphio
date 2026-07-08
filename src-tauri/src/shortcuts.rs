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
    for acc in [
        &s.shortcut_capture_visible,
        &s.shortcut_capture_snip,
        &s.shortcut_capture_full,
        &s.shortcut_capture_front_window,
        &s.shortcut_capture_scroll,
        &s.shortcut_capture_scroll_page,
        &s.shortcut_open_history,
        &s.shortcut_open_palette,
    ] {
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
    if matches(shortcut, &s.shortcut_capture_visible) {
        dispatch_capture(app, "visible");
    } else if matches(shortcut, &s.shortcut_capture_snip) {
        dispatch_capture(app, "snip");
    } else if matches(shortcut, &s.shortcut_capture_full) {
        dispatch_capture(app, "fullWindow");
    } else if matches(shortcut, &s.shortcut_capture_front_window) {
        dispatch_capture(app, "frontWindow");
    } else if matches(shortcut, &s.shortcut_capture_scroll) {
        dispatch_capture(app, "scrolling");
    } else if matches(shortcut, &s.shortcut_capture_scroll_page) {
        dispatch_capture(app, "scrollingPage");
    } else if matches(shortcut, &s.shortcut_open_history) {
        let inner = app.clone();
        let _ = app.run_on_main_thread(move || {
            let _ = crate::commands::open_history_view(inner.clone());
        });
    } else if matches(shortcut, &s.shortcut_open_palette) {
        let inner = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Err(e) = crate::windows::toggle_palette(&inner) {
                log::error!("snippet palette failed: {e}");
            }
        });
    }
}

fn dispatch_capture(app: AppHandle, mode: &'static str) {
    let inner = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Err(e) = crate::capture::trigger(&inner, mode) {
            log::error!("capture ({mode}) failed: {e}");
        }
    });
}
