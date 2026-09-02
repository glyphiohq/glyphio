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
pub fn update_group(
    app: AppHandle,
    state: State<AppState>,
    id: String,
    patch: GroupUpdate,
) -> CmdResult<()> {
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
/// Org export policy (server-attested via `/v1/me`, see `docs/ARCHITECTURE.md`) gates
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
            me.policy
                .as_ref()
                .map(|p| p.export_team_groups.clone())
                .unwrap_or_default(),
        ),
        None => (Default::default(), String::new()),
    };
    let allow_team = move |team: &str| match policy.as_str() {
        "" | "open" => true,
        "disabled" => false,
        _ /* "managers" */ => roles.get(team).is_some_and(|r| *r >= Role::Manager),
    };

    if let Some(gid) = &group_id {
        if let Some(team) = state
            .snippets
            .get_group(gid)
            .map_err(err)?
            .and_then(|g| g.team)
        {
            if !allow_team(&team) {
                return Err(format!(
                    "Export of team-shared groups is restricted by your organization \
                     (team “{team}”). Ask a team manager or your org owner."
                ));
            }
        }
    }
    let json = state
        .snippets
        .export_json(group_id.as_deref(), &allow_team)
        .map_err(err)?;
    std::fs::write(&path, json).map_err(err)
}

/// Read a Glyphio JSON export or a `matches:`-style YAML file (chosen by extension, with a
/// content-sniff fallback).
fn read_import(path: &str) -> CmdResult<snippet_store::ParsedImport> {
    let text = std::fs::read_to_string(path).map_err(err)?;
    let looks_json = path.ends_with(".json") || text.trim_start().starts_with('{');
    if looks_json {
        snippet_store::parse_json(&text).map_err(err)
    } else {
        snippet_store::parse_matches_yaml(&text).map_err(err)
    }
}

/// What importing this file would do, without writing anything. The import dialog needs the
/// answer up front: which snippets are new, which are already here byte-for-byte, and which
/// triggers collide with different content and so need the user's decision.
#[tauri::command]
pub fn preview_import(
    state: State<AppState>,
    path: String,
) -> CmdResult<snippet_store::ImportPlan> {
    state
        .snippets
        .plan_import(&read_import(&path)?)
        .map_err(err)
}

/// Import snippets. `options` carries the destination group (all snippets land there,
/// inheriting its team) and the triggers the user chose to overwrite; anything else that
/// collides is left exactly as it is.
#[tauri::command]
pub fn import_snippets(
    state: State<AppState>,
    path: String,
    options: Option<snippet_store::ImportOptions>,
) -> CmdResult<snippet_store::ImportReport> {
    let parsed = read_import(&path)?;
    let report = state
        .snippets
        .apply_import(&parsed, &options.unwrap_or_default())
        .map_err(err)?;
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
    crate::autostart::set_enabled(&app, settings.launch_at_login).map_err(err)?;
    settings.save(&state.paths.settings_json).map_err(err)?;
    let wants_worker = settings.wants_silent_worker();
    *state.settings.lock().unwrap() = settings;
    // Re-register global hotkeys so shortcut edits take effect immediately.
    crate::shortcuts::register(&app).map_err(err)?;
    // Park (or dismiss) the silent-capture worker now, while the user is looking at this
    // window — creating it during a capture would pull Glyphio in front of what they're
    // capturing. See `windows::ensure_silent_editor`.
    if wants_worker {
        if let Err(e) = crate::windows::ensure_silent_editor(&app) {
            log::warn!("could not park the silent capture worker: {e}");
        }
    } else {
        crate::windows::close_silent_editor(&app);
    }
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
    let full = engine
        .decode(strip_data_url(&full_png_base64))
        .map_err(err)?;
    let thumb = engine
        .decode(strip_data_url(&thumb_png_base64))
        .map_err(err)?;
    let (max_count, max_bytes) = {
        let s = state.settings.lock().unwrap();
        (s.history_max_count, s.history_max_bytes)
    };
    state
        .history
        .save(meta, &full, &thumb, max_count, max_bytes)
        .map_err(err)
}

/// Edit a stored capture after the fact: note, banner on/off, and optionally a replacement
/// content PNG (re-crop / redact). The original `captured_at` timestamp is never touched.
#[tauri::command]
pub fn update_capture(
    state: State<AppState>,
    id: String,
    patch: crate::history::CaptureUpdate,
    full_png_base64: Option<String>,
    thumb_png_base64: Option<String>,
) -> CmdResult<CaptureMeta> {
    let engine = base64::engine::general_purpose::STANDARD;
    let full = full_png_base64
        .map(|b| engine.decode(strip_data_url(&b)))
        .transpose()
        .map_err(err)?;
    let thumb = thumb_png_base64
        .map(|b| engine.decode(strip_data_url(&b)))
        .transpose()
        .map_err(err)?;
    state
        .history
        .update(&id, patch, full.as_deref(), thumb.as_deref())
        .map_err(err)
}

#[tauri::command]
pub fn list_captures(state: State<AppState>) -> CmdResult<Vec<CaptureMeta>> {
    state.history.list().map_err(err)
}

/// Returns the full PNG as a base64 data URL (used by history Open/Copy/Download).
#[tauri::command]
pub fn read_capture_data_url(state: State<AppState>, id: String) -> CmdResult<String> {
    let meta = state
        .history
        .get(&id)
        .map_err(err)?
        .ok_or("capture not found")?;
    let bytes = std::fs::read(&meta.full_path).map_err(err)?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(format!("data:image/png;base64,{b64}"))
}

/// Put a base64 PNG on the system clipboard.
///
/// Copying goes through the OS rather than `navigator.clipboard`, whose write is gated on a
/// *transient user activation* — a real click in the page. Copy-on-open has no click behind
/// it, so the web API refused it ("not allowed by the user agent") and every capture opened
/// with an apologetic error instead of the image on the clipboard.
#[tauri::command]
pub fn copy_image_to_clipboard(app: AppHandle, png_base64: String) -> CmdResult<()> {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(strip_data_url(&png_base64))
        .map_err(err)?;
    // The clipboard wants raw RGBA + dimensions, not an encoded PNG.
    let img = image::load_from_memory(&bytes).map_err(err)?.to_rgba8();
    let (w, h) = img.dimensions();
    app.clipboard()
        .write_image(&tauri::image::Image::new(&img.into_raw(), w, h))
        .map_err(err)
    // Deliberately NOT suppressed from clipboard history. It was, to keep a screenshot from
    // appearing twice in the merged History — and that traded a small redundancy for the
    // clipboard list misreporting what is on the clipboard. Captures auto-copy when the editor
    // opens, so the effect was that you took a screenshot, it went to the clipboard, and the
    // one place you'd look to confirm that showed nothing. A duplicate row is a much smaller
    // problem than a list that lies.
}

/// Write a base64 PNG (optionally a data URL) to a user-chosen path (editor Download).
#[tauri::command]
pub fn save_file(path: String, png_base64: String) -> CmdResult<()> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(strip_data_url(&png_base64))
        .map_err(err)?;
    std::fs::write(&path, bytes).map_err(err)
}

/// Recognize text in an image — on-device via the Vision-framework sidecar (`glyphio-ocr`).
/// Nothing leaves the machine; OCR runs on demand, never automatically. Takes the editor's
/// current content pixels (so it reflects crops/redactions) and returns recognized lines
/// with normalized bounding boxes, for the selectable text overlay.
#[tauri::command]
pub async fn ocr_image(app: AppHandle, png_base64: String) -> CmdResult<serde_json::Value> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(strip_data_url(&png_base64))
        .map_err(err)?;
    let tmp = std::env::temp_dir().join(format!("glyphio-ocr-{}.png", uuid::Uuid::new_v4()));
    std::fs::write(&tmp, &bytes).map_err(err)?;
    use tauri_plugin_shell::ShellExt;
    let output = app
        .shell()
        .sidecar("glyphio-ocr")
        .map_err(err)?
        .arg(&tmp)
        .output()
        .await;
    let _ = std::fs::remove_file(&tmp);
    let output = output.map_err(err)?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let result: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(|e| format!("bad OCR output: {e}"))?;
    Ok(result)
}

#[tauri::command]
pub fn delete_capture(state: State<AppState>, id: String) -> CmdResult<()> {
    state.history.delete(&id).map_err(err)
}

/// Close the capture editor after its artifact has been discarded.
#[tauri::command]
pub fn close_editor(app: AppHandle) {
    if let Some(window) = app.get_webview_window("editor") {
        let _ = window.close();
    }
}

#[tauri::command]
pub fn clear_captures(state: State<AppState>) -> CmdResult<()> {
    state.history.clear().map_err(err)
}

// ---- capture ---------------------------------------------------------------

/// Trigger a capture (`visible` | `snip` | `fullWindow` | `scrolling`). Runs the native
/// capture (or opens the scrolling selection overlay), then the editor with the result.
/// `silent` overrides the configured delivery for this one capture.
#[tauri::command]
pub fn trigger_capture(app: AppHandle, mode: String, silent: Option<bool>) -> CmdResult<()> {
    crate::capture::trigger(&app, &mode, delivery(silent)).map_err(err)
}

/// A frontend's `silent` flag as a delivery choice; `None` means "as configured".
fn delivery(silent: Option<bool>) -> Option<crate::capture::Delivery> {
    use crate::capture::Delivery;
    silent.map(|s| {
        if s {
            Delivery::Silent
        } else {
            Delivery::Editor
        }
    })
}

/// The scrolling-capture overlay reports the selected rect (window-logical px == points,
/// relative to the overlay). We translate to global coords, dismiss the overlay so it's in
/// neither the frames nor the scroll path, then run the capture off the main thread.
#[tauri::command]
pub async fn scroll_capture_run(
    app: AppHandle,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    silent: Option<bool>,
) -> CmdResult<()> {
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

    // Everything from here on has to report itself. The page that invoked this command is the
    // selection overlay, and it was closed four lines ago — a returned `Err` resolves into a
    // dead webview, so a capture that failed simply looked like a capture that never happened.
    let outcome = match crate::capture::run_scrolling(&app, (gx, gy, w, h)).await {
        Ok(shot) => {
            let delivery = crate::capture::Delivery::resolve_for(&app, delivery(silent));
            crate::capture::finish(&app, shot, "scrolling", delivery)
        }
        Err(e) => Err(e),
    };
    if let Err(e) = outcome {
        crate::capture::report_failure(&app, "capture (scrolling)", &e);
    }
    Ok(())
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

// ---- snippet palette ---------------------------------------------------------

/// Dismiss the snippet palette (Esc, focus loss, or after an expansion).
///
/// Destroyed rather than hidden: it is rebuilt on the Space the user is on next time it is
/// summoned, which is the only way it can appear over a full-screen app (see
/// `windows::toggle_palette`).
#[tauri::command]
pub fn palette_hide(app: AppHandle) {
    if let Some(win) = app.get_webview_window("palette") {
        let _ = win.destroy();
    }
}

/// Expand a snippet from the palette into the previously focused app. Hides the palette,
/// gives macOS a beat to hand focus back, then asks the engine worker (over its IPC) to run
/// the full match pipeline — exactly as if the trigger had been typed, so variables, forms,
/// popups, and command snippets all behave normally.
#[tauri::command]
pub async fn palette_exec(app: AppHandle, trigger: String) -> CmdResult<()> {
    if let Some(win) = app.get_webview_window("palette") {
        let _ = win.destroy();
    }
    // Deactivate the whole app, not just the palette: if another Glyphio window is open,
    // macOS would hand key focus to it and the expansion would land inside Glyphio instead
    // of the app the user came from.
    #[cfg(target_os = "macos")]
    let _ = app.hide();
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    let paths = app.state::<AppState>().paths.clone();
    use crate::paths::s;
    use tauri_plugin_shell::ShellExt;
    let output = app
        .shell()
        .sidecar("glyphio-engine")
        .map_err(err)?
        .args(["match", "exec", "--trigger", &trigger])
        .env("ESPANSO_CONFIG_DIR", s(&paths.engine_config))
        .env("ESPANSO_RUNTIME_DIR", s(&paths.engine_runtime()))
        .env("ESPANSO_PACKAGE_DIR", s(&paths.engine_packages()))
        .output()
        .await
        .map_err(err)?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(())
}

/// Run a capture mode from the palette. Hides the palette and deactivates the app first so
/// focus (and the "frontmost window") returns to where the user was, then triggers the same
/// capture pipeline the tray and hotkeys use. `silent` is the palette's ⌘↩.
#[tauri::command]
pub async fn palette_capture(app: AppHandle, mode: String, silent: Option<bool>) -> CmdResult<()> {
    if let Some(win) = app.get_webview_window("palette") {
        let _ = win.destroy();
    }
    #[cfg(target_os = "macos")]
    let _ = app.hide();
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    let inner = app.clone();
    app.run_on_main_thread(move || {
        if let Err(e) = crate::capture::trigger(&inner, &mode, delivery(silent)) {
            log::error!("palette capture ({mode}) failed: {e}");
        }
    })
    .map_err(err)
}

// ---- reload ------------------------------------------------------------------

/// Full reload: re-read settings from disk, regenerate the engine config from the snippet
/// store, re-register hotkeys, restart the engine, and refresh open windows. Wired to the
/// tray's "Reload" item as a one-click recovery/refresh.
pub fn do_reload(app: &AppHandle) -> anyhow::Result<()> {
    let state = app.state::<AppState>();
    *state.settings.lock().unwrap() = Settings::load(&state.paths.settings_json);
    state.snippets.render_yaml(&state.paths.engine_config)?;
    crate::shortcuts::register(app)?;
    state.supervisor.restart(app, &state.paths)?;
    let _ = app.emit("snippets-changed", ());
    let _ = app.emit("groups-changed", ());
    let _ = app.emit("settings-changed", ());
    Ok(())
}

#[tauri::command]
pub fn reload_all(app: AppHandle) -> CmdResult<()> {
    do_reload(&app).map_err(err)
}

// ---- engine / permissions -------------------------------------------------

/// Whether the engine daemon last reported macOS Accessibility as granted.
#[tauri::command]
pub fn accessibility_status(state: State<AppState>) -> bool {
    state.supervisor.accessibility_ok()
}

/// The app currently holding macOS Secure Input (`None` when free). While held, typed
/// triggers cannot expand — the settings banner uses this to explain the pause.
#[tauri::command]
pub fn secure_input_status(state: State<AppState>) -> Option<String> {
    state.supervisor.secure_input_holder()
}

/// Open System Settings at Privacy & Security › Accessibility.
#[tauri::command]
pub fn open_accessibility_settings(app: AppHandle) -> CmdResult<()> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_url(
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
            None::<&str>,
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
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_url(
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
            None::<&str>,
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
// ---- clipboard history ------------------------------------------------------

#[tauri::command]
pub fn list_clips(state: State<AppState>) -> CmdResult<Vec<crate::clipboard::ClipEntry>> {
    state.clips.list().map_err(err)
}

/// Which list the palette should open on, asked for once by the page on load.
#[tauri::command]
pub fn palette_view(state: State<AppState>) -> String {
    state.palette_view.lock().unwrap().clone()
}

/// Remember the list the user just switched to, so the next ⌥Space opens on it.
///
/// The palette window is destroyed when it is dismissed (see `windows::toggle_palette`), so
/// this can't be state in the page — the page is gone.
#[tauri::command]
pub fn palette_view_set(state: State<AppState>, view: String) {
    *state.palette_view.lock().unwrap() = view;
}

/// Put an entry back on the clipboard and paste it where the user was.
///
/// The picker had focus a moment ago, so the same rule as `form_submit` applies: close and
/// step aside *first*, then paste — otherwise ⌘V arrives while Glyphio is still frontmost and
/// nothing appears to happen. `paste: false` just loads the clipboard and leaves it there.
#[tauri::command]
pub async fn clipboard_use(app: AppHandle, id: String, paste: bool) -> CmdResult<()> {
    let entry = app
        .state::<AppState>()
        .clips
        .get(&id)
        .map_err(err)?
        .ok_or_else(|| "that clipboard entry is gone".to_string())?;
    crate::clipboard::put_back(&app, &entry).map_err(err)?;

    if let Some(win) = app.get_webview_window("palette") {
        let _ = win.destroy();
    }
    // Picking an entry always succeeds at the part that matters — it is on the clipboard, ready
    // for ⌘V. Pasting for the user is the bonus, and it needs the Accessibility grant to
    // synthesise the keystroke. Reporting the missing grant as an *error* made a working copy
    // look like a failed one, so acknowledge it the way a silent capture does and stop there.
    if !paste || !crate::clipboard::can_send_paste() {
        crate::tray::flash_ack(&app);
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    let _ = app.hide();
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    if let Err(e) = crate::clipboard::send_paste() {
        log::warn!("paste keystroke failed: {e}"); // still on the clipboard
        crate::tray::flash_ack(&app);
    }
    Ok(())
}

#[tauri::command]
pub fn clip_set_pinned(state: State<AppState>, id: String, pinned: bool) -> CmdResult<()> {
    state.clips.set_pinned(&id, pinned).map_err(err)
}

#[tauri::command]
pub fn delete_clip(state: State<AppState>, id: String) -> CmdResult<()> {
    state.clips.delete(&id).map_err(err)
}

#[tauri::command]
pub fn clear_clips(state: State<AppState>) -> CmdResult<()> {
    state.clips.clear().map_err(err)
}

/// One-shot payload pull for bridge-driven windows (`popup` / `form`) — same pattern as
/// `take_pending_capture`, keyed by window label.
#[tauri::command]
pub fn take_pending_payload(state: State<AppState>, label: String) -> Option<serde_json::Value> {
    state.pending_payloads.lock().unwrap().remove(&label)
}

/// The form window submits its filled body — completes the engine's blocked expansion.
///
/// Order matters, and it is the opposite of the obvious one. Resolving unblocks the engine,
/// which injects into whatever is frontmost within milliseconds — so the form window has to
/// be gone, and Glyphio deactivated, *before* the answer goes back. Answer first and the
/// expansion lands in the form that asked for it, which looks exactly like nothing happening.
/// Same reasoning and the same beat as [`palette_exec`].
#[tauri::command]
pub async fn form_submit(app: AppHandle, request_id: String, text: String) {
    if let Some(win) = app.get_webview_window("form") {
        let _ = win.close();
    }
    // Not just the window: another open Glyphio window would take key focus instead.
    #[cfg(target_os = "macos")]
    let _ = app.hide();
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    app.state::<AppState>()
        .bridge
        .resolve(&request_id, crate::bridge::FormReply::Submitted(text));
}

/// The form window was cancelled (Esc / closed) — the expansion aborts cleanly.
#[tauri::command]
pub fn form_cancel(app: AppHandle, state: State<AppState>, request_id: String) {
    state
        .bridge
        .resolve(&request_id, crate::bridge::FormReply::Cancelled);
    if let Some(win) = app.get_webview_window("form") {
        let _ = win.close();
    }
}

#[tauri::command]
pub fn take_pending_capture(
    app: AppHandle,
    session_id: String,
    silent: bool,
) -> Option<crate::capture::PendingCapture> {
    let session_id = crate::capture::delivery::DeliverySessionId::parse(session_id)?;
    app.state::<AppState>()
        .capture_deliveries
        .lock()
        .unwrap()
        .consume(
            &session_id,
            crate::capture::delivery::DeliveryRoute::from_silent(silent),
        )
}

/// A silent capture is finished with. Tell the user something happened — a capture with no
/// window of its own is otherwise indistinguishable from a hotkey that didn't fire. Failures
/// get the same dialog as any other capture; there is no editor left open to notice them in.
///
/// The worker window stays parked for the next one (see `windows::ensure_silent_editor`).
#[tauri::command]
pub fn capture_done_silently(app: AppHandle, error: Option<String>) {
    match error {
        Some(message) => {
            crate::capture::report_failure(&app, "silent capture", &anyhow::anyhow!(message))
        }
        None => crate::tray::flash_ack(&app),
    }
}

/// Ask GitHub whether a newer Glyphio exists. Read-only — nothing is downloaded.
#[tauri::command]
pub async fn check_for_update(app: AppHandle) -> crate::updates::Status {
    crate::updates::check(&app).await
}

/// Install the update the user just agreed to, then relaunch into it.
#[tauri::command]
pub async fn install_update(app: AppHandle) -> CmdResult<()> {
    crate::updates::install(&app)
        .await
        .map_err(|e| format!("{e:#}"))
}

fn strip_data_url(s: &str) -> &str {
    s.split_once(",").map_or(s, |(_, rest)| rest)
}
