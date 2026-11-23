//! The permission model: what the system allows, why not, and what to do about it.
//!
//! Platform-neutral, over [`crate::platform::Permissions`]. The decisions are pure
//! ([`grant`], [`import_legacy_flags`], [`repair`]) and tested against the fake; the watcher and
//! commands at the bottom only gather observations and emit changes.
//!
//! Accessibility is read from the app's own trust check, never from the engine: TCC attributes
//! the bundled engine to Glyphio, so one grant covers both. Whether expansion is actually running
//! is a separate line (`expansion_live`), reported by the engine.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::platform::{PermissionKind as Kind, Permissions};
use crate::settings::Settings;
use crate::AppState;

/// How long after opening System Settings we stop assuming the user is still toggling it.
pub const SETTLE_AFTER_SETTINGS: Duration = Duration::from_secs(20);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Grant {
    Granted,
    /// Never asked.
    NotDetermined,
    /// Asked and refused (or not yet switched on).
    Denied,
    /// The user says it is on, but the system still refuses: the entry belongs to another
    /// build or signature and needs repairing.
    Stale,
    /// Granted in System Settings, but this process can only use it after a relaunch.
    NeedsRelaunch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub accessibility: Grant,
    pub screen_recording: Grant,
    /// The engine confirmed its event tap, i.e. text expansion is running.
    pub expansion_live: bool,
}

/// Everything [`grant`] needs to know about one permission.
#[derive(Clone, Copy, Debug, Default)]
pub struct Observation {
    /// The system lets this process use the capability right now.
    pub usable: bool,
    /// The system prompt for it has been shown before.
    pub prompted: bool,
    /// The user returned from System Settings after we opened it.
    pub returned_from_settings: bool,
    /// The user pressed "I've switched it on", or 20 s passed since opening Settings.
    pub says_enabled: bool,
    /// The grant was last seen on a different bundle path.
    pub identity_changed: bool,
}

/// The one rule that maps an observation to a state. Order matters: a usable capability is
/// granted whatever else is true; otherwise a changed identity is stale before anything else.
pub fn grant(kind: Kind, o: Observation) -> Grant {
    if o.usable {
        return Grant::Granted;
    }
    if o.identity_changed {
        return Grant::Stale;
    }
    if kind == Kind::ScreenRecording && o.returned_from_settings && !o.says_enabled {
        return Grant::NeedsRelaunch;
    }
    if o.says_enabled {
        return Grant::Stale;
    }
    if o.prompted {
        Grant::Denied
    } else {
        Grant::NotDetermined
    }
}

/// The old UI kept "already prompted" in web storage. Take those flags once; never un-set one
/// that settings already has.
pub fn import_legacy_flags(settings: &mut Settings, accessibility: bool, screen_recording: bool) -> bool {
    let before = (
        settings.permission_prompted_accessibility,
        settings.permission_prompted_screen_recording,
    );
    settings.permission_prompted_accessibility |= accessibility;
    settings.permission_prompted_screen_recording |= screen_recording;
    before
        != (
            settings.permission_prompted_accessibility,
            settings.permission_prompted_screen_recording,
        )
}

pub fn set_prompted(settings: &mut Settings, kind: Kind, value: bool) {
    match kind {
        Kind::Accessibility => settings.permission_prompted_accessibility = value,
        Kind::ScreenRecording => settings.permission_prompted_screen_recording = value,
    }
}

pub fn prompted(settings: &Settings, kind: Kind) -> bool {
    match kind {
        Kind::Accessibility => settings.permission_prompted_accessibility,
        Kind::ScreenRecording => settings.permission_prompted_screen_recording,
    }
}

/// Reset only this app's entry, then ask again so one fresh prompt appears. Returns whether the
/// request says the capability is already usable.
pub fn repair(perms: &dyn Permissions, kind: Kind) -> anyhow::Result<bool> {
    perms.reset(kind)?;
    Ok(match kind {
        Kind::Accessibility => perms.request_accessibility(),
        Kind::ScreenRecording => perms.request_screen_recording(),
    })
}

// ---- gathering ---------------------------------------------------------------------------

/// What happened in this run that the system cannot tell us.
#[derive(Default)]
pub struct Session {
    opened_settings_at: [Option<Instant>; 2],
    returned: [bool; 2],
    confirmed: [bool; 2],
}

fn slot(kind: Kind) -> usize {
    match kind {
        Kind::Accessibility => 0,
        Kind::ScreenRecording => 1,
    }
}

impl Session {
    pub fn opened_settings(&mut self, kind: Kind) {
        self.opened_settings_at[slot(kind)] = Some(Instant::now());
        self.returned[slot(kind)] = false;
    }
    /// The app became active again after Settings was opened.
    pub fn returned_to_app(&mut self) {
        for i in 0..2 {
            if self.opened_settings_at[i].is_some() {
                self.returned[i] = true;
            }
        }
    }
    pub fn confirm_enabled(&mut self, kind: Kind) {
        self.confirmed[slot(kind)] = true;
    }
    pub fn clear(&mut self, kind: Kind) {
        let i = slot(kind);
        self.opened_settings_at[i] = None;
        self.returned[i] = false;
        self.confirmed[i] = false;
    }
    fn says_enabled(&self, kind: Kind, now: Instant) -> bool {
        let i = slot(kind);
        self.confirmed[i]
            || (self.returned[i]
                && self.opened_settings_at[i]
                    .is_some_and(|t| now.duration_since(t) >= SETTLE_AFTER_SETTINGS))
    }
    fn returned(&self, kind: Kind) -> bool {
        self.returned[slot(kind)]
    }
}

/// This app's bundle path (`…/Glyphio.app`), or the executable's folder outside a bundle.
pub fn current_identity() -> String {
    let Ok(exe) = std::env::current_exe() else { return String::new() };
    exe.ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .or_else(|| exe.parent())
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}

fn observe(
    perms: &dyn Permissions,
    kind: Kind,
    settings: &Settings,
    session: &Session,
    now: Instant,
) -> Observation {
    let was_prompted = prompted(settings, kind);
    let usable = match kind {
        Kind::Accessibility => perms.accessibility_trusted(),
        // The probe can itself prompt on a fresh system, so it only runs once the user has
        // been asked: then it tells "granted, relaunch not needed" from "refused".
        Kind::ScreenRecording => {
            perms.screen_recording_granted() || (was_prompted && perms.screen_recording_probe())
        }
    };
    let identity = &settings.permission_grant_identity;
    Observation {
        usable,
        prompted: was_prompted,
        returned_from_settings: session.returned(kind),
        says_enabled: session.says_enabled(kind, now),
        identity_changed: !identity.is_empty() && *identity != current_identity(),
    }
}

pub fn status(
    perms: &dyn Permissions,
    settings: &Settings,
    session: &Session,
    expansion_live: bool,
    now: Instant,
) -> Status {
    let g = |k| grant(k, observe(perms, k, settings, session, now));
    Status {
        accessibility: g(Kind::Accessibility),
        screen_recording: g(Kind::ScreenRecording),
        expansion_live,
    }
}

// ---- shared state, watcher and commands --------------------------------------------------

pub struct PermissionState {
    pub session: Mutex<Session>,
    pub last: Mutex<Option<Status>>,
    /// Polling runs only while setup or General settings is on screen.
    pub watching: AtomicBool,
}

impl Default for PermissionState {
    fn default() -> Self {
        Self { session: Mutex::new(Session::default()), last: Mutex::new(None), watching: AtomicBool::new(false) }
    }
}

fn read_status(app: &AppHandle) -> Status {
    let state = app.state::<AppState>();
    let settings = state.settings.lock().unwrap().clone();
    let session = state.permissions.session.lock().unwrap();
    status(
        &*crate::platform::current().permissions,
        &settings,
        &session,
        state.supervisor.accessibility_ok(),
        Instant::now(),
    )
}

impl Status {
    /// Something still needs the user: either permission is not granted.
    pub fn unresolved(&self) -> bool {
        self.accessibility != Grant::Granted || self.screen_recording != Grant::Granted
    }
}

/// Read the current status and show or hide the menu-bar "Finish setup" row to match. Called
/// once at launch, after the menu exists; [`refresh`] keeps it current after that.
pub fn sync_tray(app: &AppHandle) {
    let now = read_status(app);
    crate::tray::set_setup_unresolved(app, now.unresolved());
}

/// Compare with the last reading, remember the grant identity, emit on change, and restart the
/// engine the moment Accessibility becomes granted so expansion starts without a relaunch.
fn refresh(app: &AppHandle) -> Status {
    let now = read_status(app);
    let state = app.state::<AppState>();
    let previous = state.permissions.last.lock().unwrap().replace(now);
    if now.accessibility == Grant::Granted || now.screen_recording == Grant::Granted {
        let identity = current_identity();
        let mut settings = state.settings.lock().unwrap();
        if settings.permission_grant_identity != identity {
            settings.permission_grant_identity = identity;
            if let Err(e) = settings.save(&state.paths.settings_json) {
                log::warn!("could not record the permission identity: {e}");
            }
        }
    }
    if previous != Some(now) {
        let _ = app.emit("permission-status", now);
        crate::tray::set_setup_unresolved(app, now.unresolved());
        let newly_granted = now.accessibility == Grant::Granted
            && previous.is_some_and(|p| p.accessibility != Grant::Granted);
        let newly_usable = newly_granted
            || (now.screen_recording == Grant::Granted
                && previous.is_some_and(|p| p.screen_recording != Grant::Granted));
        if newly_usable {
            crate::onboarding::bring_setup_forward(app);
        }
        if newly_granted {
            let app = app.clone();
            std::thread::spawn(move || {
                let state = app.state::<AppState>();
                if let Err(e) = state.supervisor.restart(&app, &state.paths) {
                    log::error!("could not restart the engine after Accessibility was granted: {e}");
                }
            });
        }
    }
    now
}

/// Start the once-a-second watcher. It does nothing unless [`set_watching`] turned it on.
pub fn start_watcher(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(1));
        if app.state::<AppState>().permissions.watching.load(Ordering::SeqCst) {
            refresh(&app);
        }
    });
}

fn parse_kind(kind: &str) -> Result<Kind, String> {
    match kind {
        "accessibility" => Ok(Kind::Accessibility),
        "screenRecording" => Ok(Kind::ScreenRecording),
        other => Err(format!("unknown permission kind: {other}")),
    }
}

fn mark_prompted(app: &AppHandle, kind: Kind, value: bool) {
    let state = app.state::<AppState>();
    let mut settings = state.settings.lock().unwrap();
    set_prompted(&mut settings, kind, value);
    if let Err(e) = settings.save(&state.paths.settings_json) {
        log::warn!("could not save the permission prompt flag: {e}");
    }
}

#[tauri::command]
pub fn permission_status(app: AppHandle) -> Status {
    app.state::<AppState>().permissions.session.lock().unwrap().returned_to_app();
    refresh(&app)
}

/// Turn the watcher on or off (setup or General settings shown or hidden).
#[tauri::command]
pub fn permission_watch(state: tauri::State<AppState>, active: bool) {
    state.permissions.watching.store(active, Ordering::SeqCst);
}

/// Show the system prompt (first time) and remember that it was shown.
#[tauri::command]
pub fn permission_request(app: AppHandle, kind: String) -> Result<Status, String> {
    let kind = parse_kind(&kind)?;
    let perms = &crate::platform::current().permissions;
    match kind {
        Kind::Accessibility => perms.request_accessibility(),
        Kind::ScreenRecording => perms.request_screen_recording(),
    };
    mark_prompted(&app, kind, true);
    Ok(refresh(&app))
}

#[tauri::command]
pub fn permission_open_settings(app: AppHandle, kind: String) -> Result<(), String> {
    let kind = parse_kind(&kind)?;
    app.state::<AppState>().permissions.session.lock().unwrap().opened_settings(kind);
    crate::commands::open_permission_settings(&app, kind)
}

/// "I've switched it on": the system still refuses, so the entry is stale.
#[tauri::command]
pub fn permission_confirm_enabled(app: AppHandle, kind: String) -> Result<Status, String> {
    let kind = parse_kind(&kind)?;
    app.state::<AppState>().permissions.session.lock().unwrap().confirm_enabled(kind);
    Ok(refresh(&app))
}

/// Reset Glyphio's own entry and ask again; one fresh prompt, one clean entry.
#[tauri::command]
pub fn permission_repair(app: AppHandle, kind: String) -> Result<Status, String> {
    let kind = parse_kind(&kind)?;
    repair(&*crate::platform::current().permissions, kind).map_err(|e| format!("{e:#}"))?;
    app.state::<AppState>().permissions.session.lock().unwrap().clear(kind);
    mark_prompted(&app, kind, true);
    Ok(refresh(&app))
}

/// One-time import of the old web-storage "already prompted" flags.
#[tauri::command]
pub fn permission_import_legacy_flags(app: AppHandle, accessibility: bool, screen_recording: bool) {
    let state = app.state::<AppState>();
    let mut settings = state.settings.lock().unwrap();
    if import_legacy_flags(&mut settings, accessibility, screen_recording) {
        if let Err(e) = settings.save(&state.paths.settings_json) {
            log::warn!("could not save imported permission flags: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::fake::Fake;

    fn obs() -> Observation {
        Observation::default()
    }

    #[test]
    fn usable_is_granted_whatever_else_is_true() {
        let o = Observation { usable: true, identity_changed: true, says_enabled: true, ..obs() };
        assert_eq!(grant(Kind::Accessibility, o), Grant::Granted);
    }

    #[test]
    fn never_asked_is_not_determined_and_asked_is_denied() {
        assert_eq!(grant(Kind::Accessibility, obs()), Grant::NotDetermined);
        assert_eq!(grant(Kind::Accessibility, Observation { prompted: true, ..obs() }), Grant::Denied);
    }

    #[test]
    fn says_enabled_but_refused_is_stale() {
        let o = Observation { prompted: true, says_enabled: true, ..obs() };
        assert_eq!(grant(Kind::Accessibility, o), Grant::Stale);
    }

    #[test]
    fn a_different_bundle_is_stale_even_before_the_user_says_anything() {
        let o = Observation { prompted: true, identity_changed: true, ..obs() };
        assert_eq!(grant(Kind::ScreenRecording, o), Grant::Stale);
    }

    #[test]
    fn screen_recording_back_from_settings_still_refused_needs_a_relaunch() {
        let o = Observation { prompted: true, returned_from_settings: true, ..obs() };
        assert_eq!(grant(Kind::ScreenRecording, o), Grant::NeedsRelaunch);
        // Accessibility has no relaunch state: it is simply still denied.
        assert_eq!(grant(Kind::Accessibility, o), Grant::Denied);
    }

    #[test]
    fn legacy_flags_are_imported_once_and_never_unset() {
        let mut s = Settings::default();
        assert!(import_legacy_flags(&mut s, true, false));
        assert!(s.permission_prompted_accessibility && !s.permission_prompted_screen_recording);
        assert!(!import_legacy_flags(&mut s, true, false), "second import changes nothing");
        assert!(!import_legacy_flags(&mut s, false, false), "a false flag never unsets");
        assert!(s.permission_prompted_accessibility);
    }

    #[test]
    fn repair_resets_then_requests_once() {
        let f = Fake::default();
        repair(&f, Kind::Accessibility).unwrap();
        assert_eq!(
            *f.calls.lock().unwrap(),
            vec!["reset Accessibility".to_string(), "request Accessibility".to_string()]
        );
    }

    #[test]
    fn status_reflects_the_system_and_the_probe_only_runs_after_a_prompt() {
        let f = Fake { accessibility: true, ..Default::default() };
        let mut settings = Settings::default();
        let session = Session::default();
        let s = status(&f, &settings, &session, true, Instant::now());
        assert_eq!(s.accessibility, Grant::Granted);
        assert_eq!(s.screen_recording, Grant::NotDetermined);
        assert!(!f.calls.lock().unwrap().contains(&"probe".to_string()), "no probe before a prompt");
        settings.permission_prompted_screen_recording = true;
        let s = status(&f, &settings, &session, true, Instant::now());
        assert_eq!(s.screen_recording, Grant::Denied);
        assert!(f.calls.lock().unwrap().contains(&"probe".to_string()));
    }

    #[test]
    fn twenty_seconds_after_opening_settings_and_returning_means_the_user_says_it_is_on() {
        let mut session = Session::default();
        let t0 = Instant::now();
        session.opened_settings(Kind::Accessibility);
        session.returned_to_app();
        assert!(!session.says_enabled(Kind::Accessibility, t0));
        assert!(session.says_enabled(Kind::Accessibility, t0 + SETTLE_AFTER_SETTINGS + Duration::from_secs(1)));
        session.clear(Kind::Accessibility);
        assert!(!session.says_enabled(Kind::Accessibility, t0 + Duration::from_secs(60)));
    }
}
