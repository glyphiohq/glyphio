//! Glyphio Tauri application: wires the snippet store + engine sidecar, capture/edit/history,
//! settings, the tray, and global hotkeys into one shell.

mod bridge;
pub mod capture;
mod commands;
mod engine;
mod history;
mod logging;
mod paths;
mod settings;
mod shortcuts;
mod sync;
mod tray;
mod updates;
mod windows;

use std::sync::{Arc, Mutex};

use tauri::{Emitter, Manager};

use crate::capture::PendingCapture;
use crate::engine::Supervisor;
use crate::history::HistoryStore;
use crate::paths::AppPaths;
use crate::settings::Settings;
use snippet_store::{ChangeEntity, ChangeOrigin, SnippetStore};

/// Everything the command handlers and background threads share.
pub struct AppState {
    pub paths: AppPaths,
    pub snippets: Arc<SnippetStore>,
    pub history: HistoryStore,
    pub supervisor: Supervisor,
    pub settings: Mutex<Settings>,
    pub pending_capture: Mutex<Option<PendingCapture>>,
    /// Payloads stashed for bridge-driven windows (`popup` / `form`), keyed by window label;
    /// the window pulls its payload once via `take_pending_payload` on load.
    pub pending_payloads: Mutex<std::collections::HashMap<String, serde_json::Value>>,
    /// Form requests from the engine bridge awaiting user input.
    pub bridge: bridge::BridgeState,
    pub sync: sync::SyncState,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    logging::init();

    let paths = AppPaths::resolve().expect("resolve app paths");
    let snippets = Arc::new(SnippetStore::open(&paths.snippets_db).expect("open snippet store"));
    let history = HistoryStore::open(&paths).expect("open history store");
    let settings = Settings::load(&paths.settings_json);
    let sync = sync::SyncState::new(paths.root.join("sync.toml"));

    // Reflect current snippets into the engine config before the daemon starts.
    snippets
        .render_yaml(&paths.engine_config)
        .expect("initial engine config render");

    let state = AppState {
        paths,
        snippets,
        history,
        supervisor: Supervisor::new(),
        settings: Mutex::new(settings),
        pending_capture: Mutex::new(None),
        pending_payloads: Mutex::new(std::collections::HashMap::new()),
        bridge: bridge::BridgeState::default(),
        sync,
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(shortcuts::handler)
                .build(),
        )
        .manage(state)
        .setup(|app| {
            // Start as a menu-bar agent; opening a window promotes us to a regular app and
            // closing the last one demotes us again (windows::sync_activation_policy).
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            tray::build(app.handle())?;
            shortcuts::register(app.handle())?;

            // Notify open windows on store changes. Remote-origin changes (applied by the sync
            // engine, bypassing the command layer) must also regenerate the engine's config here.
            let handle = app.handle().clone();
            {
                let state = app.state::<AppState>();
                let store = state.snippets.clone();
                let engine_config = state.paths.engine_config.clone();
                state.snippets.add_change_listener(move |ev| {
                    if ev.origin == ChangeOrigin::Remote && ev.entity == ChangeEntity::Snippet {
                        if let Err(e) = store.render_yaml(&engine_config) {
                            log::error!("YAML regen after remote sync failed: {e}");
                        }
                    }
                    let event = match ev.entity {
                        ChangeEntity::Snippet => "snippets-changed",
                        ChangeEntity::Group => "groups-changed",
                    };
                    let _ = handle.emit(event, ev.clone());
                });
            }

            // The engine's callback socket must be listening before the daemon starts —
            // a popup/form trigger fired right after launch has to find it.
            bridge::start(app.handle());

            // Launch the supervised expansion-engine daemon.
            let handle = app.handle().clone();
            {
                let state = handle.state::<AppState>();
                if let Err(e) = state.supervisor.start(&handle, &state.paths) {
                    log::error!("failed to start engine daemon: {e}");
                }
            }

            // Resume a previous sync session silently, if one is configured.
            sync::init(app.handle());

            // Invite links (glyphio://join?...) — forward to the settings window, which shows
            // a confirmation before anything is applied (never silently: any webpage can fire
            // a URL scheme, and reconfiguring sync silently would be a redirection attack).
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                let handle = app.handle().clone();
                app.deep_link().on_open_url(move |event| {
                    for u in event.urls() {
                        let _ = windows::open(&handle, "settings");
                        let _ = handle.emit("invite-link", u.to_string());
                    }
                });
            }

            // Show the settings/snippets window on launch — which also flips us to a regular,
            // Dock-visible app for as long as it's open (see windows::sync_activation_policy).
            windows::open(app.handle(), "settings")?;

            // Park the silent-capture worker while we're launching anyway: creating its
            // window activates the app, and a capture is the wrong moment for that.
            if app.state::<AppState>().settings.lock().unwrap().wants_silent_worker() {
                if let Err(e) = windows::ensure_silent_editor(app.handle()) {
                    log::warn!("could not park the silent capture worker: {e}");
                }
            }

            // Look for a newer release, quietly. Nothing is downloaded and no window opens —
            // the answer waits in Settings → About until the user goes looking for it.
            updates::check_in_background(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_snippets,
            commands::create_snippet,
            commands::update_snippet,
            commands::delete_snippet,
            commands::list_groups,
            commands::create_group,
            commands::update_group,
            commands::delete_group,
            commands::export_snippets,
            commands::preview_import,
            commands::import_snippets,
            commands::get_settings,
            commands::save_settings,
            commands::save_capture,
            commands::update_capture,
            commands::list_captures,
            commands::read_capture_data_url,
            commands::save_file,
            commands::copy_image_to_clipboard,
            commands::ocr_image,
            commands::delete_capture,
            commands::clear_captures,
            commands::trigger_capture,
            commands::scroll_capture_run,
            commands::scroll_capture_cancel,
            commands::app_accessibility_status,
            commands::request_accessibility,
            commands::open_window,
            commands::open_history_view,
            commands::open_capture,
            commands::take_pending_capture,
            commands::check_for_update,
            commands::install_update,
            commands::capture_done_silently,
            commands::take_pending_payload,
            commands::form_submit,
            commands::form_cancel,
            commands::accessibility_status,
            commands::secure_input_status,
            commands::open_accessibility_settings,
            commands::restart_engine,
            commands::screen_recording_status,
            commands::request_screen_recording,
            commands::open_screen_recording_settings,
            commands::relaunch_app,
            commands::palette_hide,
            commands::palette_exec,
            commands::palette_capture,
            commands::reload_all,
            sync::get_sync_config,
            sync::save_sync_config,
            sync::sync_status,
            sync::sync_now,
            sync::sync_sign_in,
            sync::sync_set_token,
            sync::sync_sign_out,
            sync::sync_team_members,
            sync::sync_join_team,
            sync::sync_leave_team,
            sync::set_group_team,
            sync::parse_invite,
            sync::apply_invite,
        ])
        .build(tauri::generate_context!())
        .expect("error building Glyphio")
        .run(|app_handle, event| match event {
            // Closing the main window HIDES it instead of destroying it: Glyphio is a menu-bar
            // service, so the window is a surface that comes and goes while the app keeps
            // running (expansion, hotkeys, tray all stay live). Hiding (vs. destroying) also
            // keeps the app from ever dropping to zero windows — the state where macOS could
            // tear the process down — and makes reopening from the tray instant. Transient
            // windows (editor/capture/popup/form/palette) close normally.
            tauri::RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::CloseRequested { api, .. },
                ..
            } => {
                // Last chance to measure the frame — after this the window is hidden or gone.
                windows::save_geometry(app_handle, &label);
                if label == "settings" {
                    if let Some(win) = app_handle.get_webview_window(&label) {
                        let _ = win.hide();
                    }
                    api.prevent_close();
                }
                // With this window going away Glyphio may be back to a menu-bar-only service:
                // drop the Dock icon unless another real window is still up.
                windows::sync_activation_policy(app_handle, Some(&label));
            }
            // A destroyed window (editor, capture viewer) leaves the same question open —
            // CloseRequested doesn't fire when a window is closed programmatically.
            tauri::RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::Destroyed,
                ..
            } => {
                windows::sync_activation_policy(app_handle, Some(&label));
            }
            // Relaunching the app (`open -a Glyphio`, Launchpad, Finder) while it's already
            // running fires Reopen — surface the settings window frontmost, like clicking
            // a Dock icon would for a regular app.
            tauri::RunEvent::Reopen { .. } => {
                let _ = windows::open(app_handle, "settings");
            }
            // The settings window is opened during setup, but an accessory app can lose
            // the initial focus race (macOS hands focus back to the previously active app
            // while we're still launching). Re-assert once launch completes.
            tauri::RunEvent::Ready => {
                if let Some(win) = app_handle.get_webview_window("settings") {
                    let _ = win.set_focus();
                }
            }
            // Belt-and-suspenders: should every window still end up closed, keep the process
            // alive on a window-driven exit. Only an explicit quit (tray) carries a code.
            tauri::RunEvent::ExitRequested { code, api, .. } => {
                if code.is_none() {
                    api.prevent_exit();
                } else {
                    windows::save_all_geometry(app_handle);
                    capture::restore_browser_accessibility();
                    app_handle.state::<AppState>().supervisor.stop();
                }
            }
            // Tray "Quit" and ⌘Q (both predefined menu items) fire `Exit` — clean up the engine
            // here, else the daemon/worker are orphaned and block the next launch.
            tauri::RunEvent::Exit => {
                windows::save_all_geometry(app_handle);
                capture::restore_browser_accessibility();
                app_handle.state::<AppState>().supervisor.stop();
            }
            _ => {}
        });
}

