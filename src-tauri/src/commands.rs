//! Tauri command surface exposed to the webview frontend.

use base64::Engine;
use tauri::{AppHandle, Manager, State};

use snippet_store::{Group, GroupUpdate, NewGroup, NewSnippet, Snippet, SnippetUpdate};
use tauri::Emitter;

use crate::history::{CaptureMeta, NewCapture};
use crate::settings::Settings;
use crate::AppState;

type CmdResult<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

// ---- snippets --------------------------------------------------------------
// Every mutation regenerates the engine config; its file-watcher hot-reloads it.

fn regen_yaml(state: &AppState) -> CmdResult<()> {
    state
        .snippets
        .render_yaml(&state.paths.engine_config)
        .map_err(err)
}

#[tauri::command]
pub fn list_snippets(state: State<AppState>) -> CmdResult<Vec<Snippet>> {
    state.snippets.list().map_err(err)
}

#[tauri::command]
pub fn create_snippet(state: State<AppState>, snippet: NewSnippet) -> CmdResult<Snippet> {
    let s = state.snippets.create(snippet).map_err(err)?;
    regen_yaml(&state)?;
    Ok(s)
}

#[tauri::command]
pub fn update_snippet(
    state: State<AppState>,
    id: String,
    patch: SnippetUpdate,
) -> CmdResult<Snippet> {
    let s = state.snippets.update(&id, patch).map_err(err)?;
    regen_yaml(&state)?;
    Ok(s)
}

#[tauri::command]
pub fn delete_snippet(state: State<AppState>, id: String) -> CmdResult<()> {
    state.snippets.soft_delete(&id).map_err(err)?;
    regen_yaml(&state)
}

// ---- groups ----------------------------------------------------------------
// Groups are organizational (UI only) — they don't change the generated YAML, so no regen needed.

#[tauri::command]
pub fn list_groups(state: State<AppState>) -> CmdResult<Vec<Group>> {
    state.snippets.list_groups().map_err(err)
}

#[tauri::command]
pub fn create_group(app: AppHandle, state: State<AppState>, group: NewGroup) -> CmdResult<Group> {
    let g = state.snippets.create_group(group).map_err(err)?;
    let _ = app.emit("groups-changed", ());
    Ok(g)
}

#[tauri::command]
pub fn update_group(app: AppHandle, state: State<AppState>, id: String, patch: GroupUpdate) -> CmdResult<()> {
    state.snippets.update_group(&id, patch).map_err(err)?;
    let _ = app.emit("groups-changed", ());
    Ok(())
}

#[tauri::command]
pub fn delete_group(app: AppHandle, state: State<AppState>, id: String) -> CmdResult<()> {
    state.snippets.soft_delete_group(&id).map_err(err)?;
    let _ = app.emit("groups-changed", ());
    Ok(())
}

// ---- portability -------------------------------------------------------------

/// Export snippets (all, or one group) to a Glyphio JSON file at a user-chosen path.
///
/// Org export policy (server-attested via `/v1/me`, see docs/PHASE4-PLAN §E3) gates
/// **team-shared** content: `open` (anyone), `managers` (manager+ of that team), `disabled`.
/// Personal snippets always export. A full export silently excludes unpermitted team groups;
/// exporting a specific unpermitted group errors so the user learns why.
#[tauri::command]
pub fn export_snippets(
    state: State<AppState>,
    path: String,
    group_id: Option<String>,
) -> CmdResult<()> {
    use sync_proto::Role;
    let status = state.sync.status();
    let (roles, policy) = match &status.identity {
        Some(me) => (
            me.roles.clone(),
            me.policy.as_ref().map(|p| p.export_team_groups.clone()).unwrap_or_default(),
        ),
        None => (Default::default(), String::new()),
    };
    let allow_team = move |team: &str| match policy.as_str() {
        "" | "open" => true,
        "disabled" => false,
        _ /* "managers" */ => roles.get(team).is_some_and(|r| *r >= Role::Manager),
    };

    if let Some(gid) = &group_id {
        if let Some(team) = state.snippets.get_group(gid).map_err(err)?.and_then(|g| g.team) {
            if !allow_team(&team) {
                return Err(format!(
                    "Export of team-shared groups is restricted by your organization \
                     (team “{team}”). Ask a team manager or your org owner."
                ));
            }
        }
    }
    let json = state.snippets.export_json(group_id.as_deref(), &allow_team).map_err(err)?;
    std::fs::write(&path, json).map_err(err)
}

/// Import snippets from a Glyphio JSON export or a `matches:`-style YAML file
/// (chosen by extension, with a content-sniff fallback). Additive — duplicates are skipped
/// and reported, never overwritten.
#[tauri::command]
pub fn import_snippets(
    state: State<AppState>,
    path: String,
) -> CmdResult<snippet_store::ImportReport> {
    let text = std::fs::read_to_string(&path).map_err(err)?;
    let looks_json =
        path.ends_with(".json") || text.trim_start().starts_with('{');
    let report = if looks_json {
        state.snippets.import_json(&text).map_err(err)?
    } else {
        state.snippets.import_matches_yaml(&text).map_err(err)?
    };
    regen_yaml(&state)?;
    Ok(report)
}

// ---- settings --------------------------------------------------------------

#[tauri::command]
pub fn get_settings(state: State<AppState>) -> Settings {
    state.settings.lock().unwrap().clone()
}

#[tauri::command]
pub fn save_settings(app: AppHandle, state: State<AppState>, settings: Settings) -> CmdResult<()> {
    settings.save(&state.paths.settings_json).map_err(err)?;
    *state.settings.lock().unwrap() = settings;
    // Re-register global hotkeys so shortcut edits take effect immediately.
    crate::shortcuts::register(&app).map_err(err)?;
    Ok(())
}

// ---- history ---------------------------------------------------------------

#[tauri::command]
pub fn save_capture(
    state: State<AppState>,
    meta: NewCapture,
    full_png_base64: String,
    thumb_png_base64: String,
) -> CmdResult<CaptureMeta> {
    let engine = base64::engine::general_purpose::STANDARD;
    let full = engine.decode(strip_data_url(&full_png_base64)).map_err(err)?;
    let thumb = engine.decode(strip_data_url(&thumb_png_base64)).map_err(err)?;
    let (max_count, max_bytes) = {
        let s = state.settings.lock().unwrap();
        (s.history_max_count, s.history_max_bytes)
    };
    state
        .history
        .save(meta, &full, &thumb, max_count, max_bytes)
        .map_err(err)
}

#[tauri::command]
pub fn list_captures(state: State<AppState>) -> CmdResult<Vec<CaptureMeta>> {
    state.history.list().map_err(err)
}

/// Returns the full PNG as a base64 data URL (used by history Open/Copy/Download).
#[tauri::command]
pub fn read_capture_data_url(state: State<AppState>, id: String) -> CmdResult<String> {
    let meta = state.history.get(&id).map_err(err)?.ok_or("capture not found")?;
    let bytes = std::fs::read(&meta.full_path).map_err(err)?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(format!("data:image/png;base64,{b64}"))
}

/// Write a base64 PNG (optionally a data URL) to a user-chosen path (editor Download).
#[tauri::command]
pub fn save_file(path: String, png_base64: String) -> CmdResult<()> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(strip_data_url(&png_base64))
        .map_err(err)?;
    std::fs::write(&path, bytes).map_err(err)
}

/// Recognize text in a stored capture — on-device via the Vision-framework sidecar
/// (`glyphio-ocr`). Nothing leaves the machine; OCR runs on demand, never automatically.
#[tauri::command]
pub async fn ocr_capture(app: AppHandle, id: String) -> CmdResult<String> {
    let path = {
        let state = app.state::<AppState>();
        let meta = state.history.get(&id).map_err(err)?.ok_or("capture not found")?;
        meta.full_path.clone()
    };
    use tauri_plugin_shell::ShellExt;
    let output = app
        .shell()
        .sidecar("glyphio-ocr")
        .map_err(err)?
        .arg(&path)
        .output()
        .await
        .map_err(err)?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        return Err("No text recognized in this capture.".into());
    }
    Ok(text)
}

#[tauri::command]
pub fn delete_capture(state: State<AppState>, id: String) -> CmdResult<()> {
    state.history.delete(&id).map_err(err)
}

#[tauri::command]
pub fn clear_captures(state: State<AppState>) -> CmdResult<()> {
    state.history.clear().map_err(err)
}

// ---- capture ---------------------------------------------------------------

/// Trigger a capture (`visible` | `snip` | `fullWindow` | `scrolling`). Runs the native
/// capture (or opens the scrolling selection overlay), then the editor with the result.
#[tauri::command]
pub fn trigger_capture(app: AppHandle, mode: String) -> CmdResult<()> {
    crate::capture::trigger(&app, &mode).map_err(err)
}

/// The scrolling-capture overlay reports the selected rect (window-logical px == points,
/// relative to the overlay). We translate to global coords, dismiss the overlay so it's in
/// neither the frames nor the scroll path, then run the capture off the main thread.
#[tauri::command]
pub async fn scroll_capture_run(app: AppHandle, x: f64, y: f64, w: f64, h: f64) -> CmdResult<()> {
    let (gx, gy) = {
        let win = app
            .get_webview_window("scroll-overlay")
            .ok_or("selection overlay is gone")?;
        let pos = win.outer_position().map_err(err)?;
        let scale = win.scale_factor().map_err(err)?;
        (pos.x as f64 / scale + x, pos.y as f64 / scale + y)
    };
    crate::windows::close_scroll_overlay(&app);
    // Give the compositor a beat to remove the overlay before the first frame.
    tokio::time::sleep(std::time::Duration::from_millis(180)).await;

    let shot = tauri::async_runtime::spawn_blocking(move || {
        crate::capture::scroll::capture(gx, gy, w, h)
    })
    .await
    .map_err(err)?
    .map_err(err)?;
    crate::capture::finish(&app, shot, "scrolling").map_err(err)
}

#[tauri::command]
pub fn scroll_capture_cancel(app: AppHandle) {
    crate::windows::close_scroll_overlay(&app);
}

/// Whether the APP holds Accessibility. One grant covers both expansion (the engine is a
/// child of the app, so TCC attributes it here) and scrolling capture.
#[tauri::command]
pub fn app_accessibility_status() -> bool {
    crate::capture::scroll::app_accessibility_trusted()
}

/// Show the system Accessibility dialog — macOS adds the "Glyphio" row itself, the user only
/// toggles it. The engine notices within ~2s (its periodic re-check) and expansion turns on.
#[tauri::command]
pub fn request_accessibility() -> bool {
    crate::capture::scroll::request_accessibility()
}

// ---- windows ---------------------------------------------------------------

#[tauri::command]
pub fn open_window(app: AppHandle, name: String) -> CmdResult<()> {
    crate::windows::open(&app, &name).map_err(err)
}

/// History now lives inside the main window: open (or focus) it and switch the view.
#[tauri::command]
pub fn open_history_view(app: AppHandle) -> CmdResult<()> {
    crate::windows::open(&app, "settings").map_err(err)?;
    let _ = app.emit("show-history", ());
    Ok(())
}

#[tauri::command]
pub fn open_capture(app: AppHandle, id: String) -> CmdResult<()> {
    crate::windows::open_capture(&app, &id).map_err(err)
}

// ---- engine / permissions -------------------------------------------------

/// Whether the engine daemon last reported macOS Accessibility as granted.
#[tauri::command]
pub fn accessibility_status(state: State<AppState>) -> bool {
    state.supervisor.accessibility_ok()
}

/// Open System Settings at Privacy & Security › Accessibility.
#[tauri::command]
pub fn open_accessibility_settings(app: AppHandle) -> CmdResult<()> {
    use tauri_plugin_shell::ShellExt;
    app.shell()
        .open(
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
            None,
        )
        .map_err(err)
}

/// Restart the expansion-engine daemon (e.g. after granting Accessibility).
#[tauri::command]
pub fn restart_engine(app: AppHandle, state: State<AppState>) -> CmdResult<()> {
    state.supervisor.restart(&app, &state.paths).map_err(err)
}

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

/// Whether the Screen Recording TCC permission is granted (checked without prompting).
#[tauri::command]
pub fn screen_recording_status() -> bool {
    #[cfg(target_os = "macos")]
    unsafe {
        CGPreflightScreenCaptureAccess()
    }
    #[cfg(not(target_os = "macos"))]
    true
}

/// Trigger the OS Screen Recording prompt (also lists Glyphio in System Settings). Returns the
/// possibly-still-false grant state — macOS applies the grant on the app's next launch.
#[tauri::command]
pub fn request_screen_recording() -> bool {
    #[cfg(target_os = "macos")]
    unsafe {
        CGRequestScreenCaptureAccess()
    }
    #[cfg(not(target_os = "macos"))]
    true
}

/// Open System Settings at Privacy & Security › Screen Recording.
#[tauri::command]
pub fn open_screen_recording_settings(app: AppHandle) -> CmdResult<()> {
    use tauri_plugin_shell::ShellExt;
    app.shell()
        .open(
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
            None,
        )
        .map_err(err)
}

/// Relaunch the app — macOS only applies a fresh Screen Recording grant on the next launch,
/// so the banner offers this instead of asking the user to quit + reopen manually.
#[tauri::command]
pub fn relaunch_app(app: AppHandle) {
    app.state::<AppState>().supervisor.stop(); // don't orphan the engine daemon/worker
    app.restart();
}

/// The editor requests its pending capture payload (set by the capture flow).
#[tauri::command]
pub fn take_pending_capture(app: AppHandle) -> Option<crate::capture::PendingCapture> {
    app.state::<AppState>().pending_capture.lock().unwrap().take()
}

fn strip_data_url(s: &str) -> &str {
    s.split_once(",").map_or(s, |(_, rest)| rest)
}
