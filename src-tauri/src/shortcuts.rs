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

/// Validate shortcuts at the same boundary that persists them. The recorder catches these
/// mistakes immediately, while this guard keeps another caller of `save_settings` from writing
/// a binding that the global-shortcut plugin will later ignore or dispatch ambiguously.
pub fn validate_settings(settings: &crate::settings::Settings) -> anyhow::Result<()> {
    let capture_labels = [
        "Visible area",
        "Region (snip)",
        "Full window",
        "Frontmost window",
        "Browser page",
        "Scrolling area",
        "Scrolling page",
        "Visible area (clipboard)",
        "Region (clipboard)",
        "Full window (clipboard)",
        "Frontmost window (clipboard)",
        "Browser page (clipboard)",
        "Scrolling area (clipboard)",
        "Scrolling page (clipboard)",
    ];
    let captures = settings.capture_shortcuts();
    let mut configured: Vec<(&str, Shortcut)> = Vec::new();

    for ((accelerator, _, _), label) in captures.iter().zip(capture_labels) {
        validate_one(accelerator, label, true, &mut configured)?;
    }
    for (accelerator, label) in [
        (&settings.shortcut_open_history, "Open capture history"),
        (&settings.shortcut_open_palette, "Snippet search"),
        (&settings.shortcut_open_clipboard, "Open clipboard history"),
    ] {
        validate_one(accelerator, label, false, &mut configured)?;
    }
    Ok(())
}

fn validate_one<'a>(
    accelerator: &'a str,
    label: &'a str,
    is_capture: bool,
    configured: &mut Vec<(&'a str, Shortcut)>,
) -> anyhow::Result<()> {
    if accelerator.trim().is_empty() {
        return Ok(());
    }
    let shortcut = Shortcut::from_str(accelerator)
        .map_err(|error| anyhow::anyhow!("{label}: invalid shortcut {accelerator:?}: {error}"))?;
    if is_capture {
        for (reserved, reason) in [
            ("Command+Space", "reserved for Spotlight"),
            ("Command+Tab", "reserved for switching applications"),
            ("Command+Shift+3", "reserved for macOS screenshots"),
            ("Command+Shift+4", "reserved for macOS screenshots"),
            (
                "Command+Shift+5",
                "reserved for macOS screenshots and recording",
            ),
            ("Control+Command+Q", "reserved for locking your Mac"),
            ("Alt+Command+Escape", "reserved for Force Quit"),
        ] {
            if Shortcut::from_str(reserved).is_ok_and(|value| value == shortcut) {
                anyhow::bail!("{label}: {accelerator} is {reason}");
            }
        }
    }
    if let Some((other, _)) = configured.iter().find(|(_, value)| *value == shortcut) {
        anyhow::bail!("{label}: {accelerator} is already used by {other}");
    }
    configured.push((label, shortcut));
    Ok(())
}

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

#[cfg(test)]
mod tests {
    use super::validate_settings;
    use crate::settings::Settings;

    #[test]
    fn accepts_a_recorded_command_shift_chord() {
        let mut settings = Settings::default();
        settings.shortcut_capture_full = "Command+Shift+S".into();
        validate_settings(&settings).unwrap();
    }

    #[test]
    fn rejects_invalid_reserved_and_colliding_capture_shortcuts() {
        let mut invalid = Settings::default();
        invalid.shortcut_capture_full = "Command+DefinitelyNotAKey".into();
        assert!(validate_settings(&invalid)
            .unwrap_err()
            .to_string()
            .contains("invalid"));

        let mut reserved = Settings::default();
        reserved.shortcut_capture_full = "Shift+Command+4".into();
        assert!(validate_settings(&reserved)
            .unwrap_err()
            .to_string()
            .contains("macOS screenshots"));

        let mut colliding = Settings::default();
        colliding.shortcut_capture_visible = "Shift+Option+KeyS".into();
        assert!(validate_settings(&colliding)
            .unwrap_err()
            .to_string()
            .contains("already used by Visible area"));
    }

    #[test]
    fn an_empty_capture_shortcut_is_a_deliberate_clear() {
        let mut settings = Settings::default();
        settings.shortcut_capture_full.clear();
        validate_settings(&settings).unwrap();
    }
}
