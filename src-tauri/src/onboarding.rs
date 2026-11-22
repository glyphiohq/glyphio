//! First-run setup state: which step the window is on, what was skipped, and whether it is done.

use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager, State};

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

/// The setup window's Launch at Login switch. This is the only place a fresh install registers
/// the item, so its effect has been explained first. Returns a note when macOS wants the user
/// to approve it in System Settings.
#[tauri::command]
pub fn onboarding_set_login_item(
    app: AppHandle,
    state: State<AppState>,
    want: bool,
) -> Result<Option<String>, String> {
    crate::commands::set_login_item(&app, &state, want)?;
    {
        let mut s = state.settings.lock().unwrap();
        s.launch_at_login = want;
        s.save(&state.paths.settings_json).map_err(|e| format!("{e:#}"))?;
    }
    Ok(crate::commands::login_item_notice(state))
}

/// Hand the team choice to Settings: show its Sync page, or confirm a pasted invite there. The
/// invite is never applied from here — Settings asks first, as it does for a link opened by
/// the system.
#[tauri::command]
pub fn onboarding_open_team(app: AppHandle, invite: Option<String>) -> Result<(), String> {
    crate::windows::open(&app, "settings").map_err(|e| format!("{e:#}"))?;
    // A window opened just now has not attached its listeners yet.
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(700)).await;
        let _ = match invite {
            Some(url) => app.emit("invite-link", url),
            None => app.emit("show-sync", ()),
        };
    });
    Ok(())
}
