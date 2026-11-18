//! Global capture hotkeys (Tauri replacement for Checkpoint's `chrome.commands`).
//!
//! The plugin is built once in `lib.rs` with [`handler`]; accelerators are (re)registered from
//! the current settings by [`register`], so editing a shortcut in Settings takes effect live.

pub mod registry;

use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutEvent, ShortcutState};

use crate::AppState;
use registry::{Action, Binding, Registrar, Registration, ShortcutStatus};

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

/// Settings fields holding the capture chords, in the order of `Settings::capture_shortcuts`.
const CAPTURE_KEYS: [&str; 14] = [
    "shortcutCaptureVisible",
    "shortcutCaptureSnip",
    "shortcutCaptureFull",
    "shortcutCaptureFrontWindow",
    "shortcutCapturePage",
    "shortcutCaptureScroll",
    "shortcutCaptureScrollPage",
    "shortcutCaptureVisibleSilent",
    "shortcutCaptureSnipSilent",
    "shortcutCaptureFullSilent",
    "shortcutCaptureFrontWindowSilent",
    "shortcutCapturePageSilent",
    "shortcutCaptureScrollSilent",
    "shortcutCaptureScrollPageSilent",
];

/// Every binding the settings describe, captures first: that order is what "first listed wins"
/// means when two actions share a chord.
fn bindings(s: &crate::settings::Settings) -> Vec<Binding> {
    let mut out: Vec<Binding> = s
        .capture_shortcuts()
        .iter()
        .zip(CAPTURE_KEYS)
        .map(|((acc, mode, silent), key)| Binding {
            key,
            action: Action::Capture {
                mode,
                silent: *silent,
            },
            accelerator: acc.to_string(),
        })
        .collect();
    for (key, action, acc) in [
        ("shortcutOpenHistory", Action::OpenHistory, &s.shortcut_open_history),
        ("shortcutOpenPalette", Action::OpenPalette, &s.shortcut_open_palette),
        ("shortcutOpenClipboard", Action::OpenClipboard, &s.shortcut_open_clipboard),
    ] {
        out.push(Binding {
            key,
            action,
            accelerator: acc.clone(),
        });
    }
    out
}

/// The real registrar. It never lets go of Escape while a scrolling capture has borrowed it,
/// even if the user has also bound Escape to something.
struct PluginRegistrar<'a>(&'a AppHandle);

impl Registrar for PluginRegistrar<'_> {
    fn register(&mut self, chord: Shortcut) -> Result<(), String> {
        self.0
            .global_shortcut()
            .register(chord)
            .map_err(|e| e.to_string())
    }
    fn unregister(&mut self, chord: Shortcut) -> Result<(), String> {
        if STOP_KEY_ARMED.load(Ordering::SeqCst)
            && Shortcut::from_str(STOP_KEY).is_ok_and(|stop| stop == chord)
        {
            return Ok(());
        }
        self.0
            .global_shortcut()
            .unregister(chord)
            .map_err(|e| e.to_string())
    }
}

/// Bring the system's registrations in line with the current settings and say what happened to
/// each binding. Only chords that changed are touched, so saving an unrelated setting makes no
/// register or unregister call and the borrowed Escape is never involved.
pub fn register(app: &AppHandle) -> Vec<ShortcutStatus> {
    let state = app.state::<AppState>();
    let wanted = bindings(&state.settings.lock().unwrap());
    let status = state
        .shortcuts
        .lock()
        .unwrap()
        .apply(&wanted, &mut PluginRegistrar(app));
    crate::tray::set_shortcut_problem(app, problem_summary(&status));
    status
}

/// The last results, without registering anything.
pub fn status(app: &AppHandle) -> Vec<ShortcutStatus> {
    app.state::<AppState>().shortcuts.lock().unwrap().status().to_vec()
}

/// The notice for the tray when any binding was refused or invalid.
fn problem_summary(status: &[ShortcutStatus]) -> bool {
    status.iter().any(|s| {
        matches!(
            s.registration,
            Registration::Refused(_) | Registration::Invalid(_)
        )
    })
}

/// Global handler installed on the plugin; dispatches a fired shortcut to its action.
pub fn handler(app: &AppHandle, shortcut: &Shortcut, event: ShortcutEvent) {
    if event.state() != ShortcutState::Pressed {
        return;
    }
    // Checked before anything configured: while a scrolling capture is running, Escape means
    // "stop now and keep the frames" and nothing else.
    if STOP_KEY_ARMED.load(Ordering::SeqCst)
        && Shortcut::from_str(STOP_KEY).is_ok_and(|stop| &stop == shortcut)
    {
        crate::capture::scroll::request_stop();
        return;
    }
    let action = {
        let state = app.state::<AppState>();
        let mut registry = state.shortcuts.lock().unwrap();
        match registry.lookup(shortcut) {
            Some(action) if registry.should_fire(action, Instant::now()) => action,
            _ => return,
        }
    };
    let app = app.clone();
    match action {
        Action::Capture { mode, silent } => dispatch_capture(app, mode, silent),
        Action::OpenHistory => {
            let inner = app.clone();
            let _ = app.run_on_main_thread(move || {
                let _ = crate::commands::open_history_view(inner.clone());
            });
        }
        Action::OpenPalette => {
            let inner = app.clone();
            let _ = app.run_on_main_thread(move || {
                if let Err(e) = crate::windows::toggle_palette(&inner, None) {
                    log::error!("palette failed: {e}");
                }
            });
        }
        Action::OpenClipboard => {
            let inner = app.clone();
            let _ = app.run_on_main_thread(move || {
                if let Err(e) = crate::windows::toggle_palette(&inner, Some("clipboard")) {
                    log::error!("palette failed: {e}");
                }
            });
        }
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
    use super::{bindings, CAPTURE_KEYS};
    use super::validate_settings;
    use crate::settings::Settings;

    #[test]
    fn accepts_a_recorded_command_shift_chord() {
        let settings = Settings {
            shortcut_capture_full: "Command+Shift+S".into(),
            ..Settings::default()
        };
        validate_settings(&settings).unwrap();
    }

    #[test]
    fn rejects_invalid_reserved_and_colliding_capture_shortcuts() {
        let invalid = Settings {
            shortcut_capture_full: "Command+DefinitelyNotAKey".into(),
            ..Settings::default()
        };
        assert!(validate_settings(&invalid)
            .unwrap_err()
            .to_string()
            .contains("invalid"));

        let reserved = Settings {
            shortcut_capture_full: "Shift+Command+4".into(),
            ..Settings::default()
        };
        assert!(validate_settings(&reserved)
            .unwrap_err()
            .to_string()
            .contains("macOS screenshots"));

        let colliding = Settings {
            shortcut_capture_visible: "Shift+Option+KeyS".into(),
            ..Settings::default()
        };
        assert!(validate_settings(&colliding)
            .unwrap_err()
            .to_string()
            .contains("already used by Visible area"));
    }

    #[test]
    fn an_empty_capture_shortcut_is_a_deliberate_clear() {
        let settings = Settings {
            shortcut_capture_full: String::new(),
            ..Settings::default()
        };
        validate_settings(&settings).unwrap();
    }

    #[test]
    fn every_binding_key_is_a_settings_field() {
        let json = serde_json::to_value(Settings::default()).unwrap();
        for b in bindings(&Settings::default()) {
            assert!(json.get(b.key).is_some(), "{} is not a settings field", b.key);
        }
        assert_eq!(CAPTURE_KEYS.len(), Settings::default().capture_shortcuts().len());
    }
}
