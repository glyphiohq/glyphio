//! First-run setup state: which step the window is on, what was skipped, and whether it is done.

use tauri::{AppHandle, Manager, State};

use crate::AppState;

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingState {
    pub completed: bool,
    pub step: String,
    pub skipped: Vec<String>,
}

#[tauri::command]
pub fn onboarding_state(state: State<AppState>) -> OnboardingState {
    let s = state.settings.lock().unwrap();
    OnboardingState {
        completed: s.onboarding_completed,
        step: s.onboarding_step.clone(),
        skipped: s.onboarding_skipped.clone(),
    }
}

/// Persist progress. Any argument left out is unchanged.
#[tauri::command]
pub fn onboarding_update(
    state: State<AppState>,
    step: Option<String>,
    skipped: Option<Vec<String>>,
    completed: Option<bool>,
) -> Result<(), String> {
    let mut s = state.settings.lock().unwrap();
    if let Some(step) = step {
        s.onboarding_step = step;
    }
    if let Some(skipped) = skipped {
        s.onboarding_skipped = skipped;
    }
    if let Some(completed) = completed {
        s.onboarding_completed = completed;
    }
    s.save(&state.paths.settings_json).map_err(|e| format!("{e:#}"))
}

/// Reopen setup (Settings → "Run setup again", and later the tray row).
#[tauri::command]
pub fn open_setup(app: AppHandle) -> Result<(), String> {
    crate::windows::open_setup(&app).map_err(|e| format!("{e:#}"))
}

/// Bring the setup window to the front, if open: the user is in System Settings when a grant
/// lands, and the window should be waiting when they come back.
pub fn bring_setup_forward(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("setup") {
        let _ = win.unminimize();
        let _ = win.set_focus();
    }
}
