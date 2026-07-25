//! Supervises the bundled expansion-engine binary (a GPL fork — see NOTICES.md) as a managed sidecar.
//!
//! We run the engine's hidden foreground `daemon` subcommand ("start the daemon
//! without spawning a new process") — as a child of the Tauri app, pointed at Glyphio's isolated
//! config dir via its `ESPANSO_*_DIR` env vars (the engine's own interface — renaming them would grow
//! the fork diff for cosmetics). It file-watches that dir and hot-reloads, so
//! regenerating config from the snippet store is all it takes to update live expansions.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_shell::process::{CommandChild, CommandEvent};
use tauri_plugin_shell::ShellExt;

use crate::paths::{s, AppPaths};

/// Give up auto-respawning after this many engine deaths in one app run (crash loop).
const MAX_RESPAWNS: u32 = 5;

#[derive(Default)]
pub struct Supervisor {
    child: Arc<Mutex<Option<CommandChild>>>,
    /// Tracks whether the daemon reported macOS Accessibility as granted (parsed from its logs).
    /// `None` until the daemon reports; the UI shows a "grant Accessibility" banner while false.
    accessibility_ok: Arc<AtomicBool>,
    accessibility_reported: Arc<AtomicBool>,
    /// App currently holding macOS Secure Input (parsed from worker logs), `None` when free.
    /// While ANY app holds it, no event tap on the system sees keystrokes — typed triggers
    /// silently do nothing while hotkeys/captures still work, which users read as "expansion
    /// is broken". Tracked so the UI can say so instead of looking broken.
    secure_input_holder: Arc<Mutex<Option<String>>>,
    /// Set while we are deliberately stopping (exit/restart) so the exit watcher doesn't respawn.
    stopping: Arc<AtomicBool>,
    respawns: Arc<AtomicU32>,
}

impl Supervisor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_running(&self) -> bool {
        self.child.lock().unwrap().is_some()
    }

    /// Whether the daemon last reported Accessibility as granted (false until reported).
    pub fn accessibility_ok(&self) -> bool {
        self.accessibility_ok.load(Ordering::SeqCst)
    }

    /// The app currently holding macOS Secure Input, if any (expansion is paused while held).
    pub fn secure_input_holder(&self) -> Option<String> {
        self.secure_input_holder.lock().unwrap().clone()
    }

    /// Spawn the engine daemon sidecar (idempotent — no-op if already running).
    pub fn start(&self, app: &AppHandle, paths: &AppPaths) -> anyhow::Result<()> {
        if self.is_running() {
            return Ok(());
        }
        // Clear any orphaned engine from a previous run (a tray-quit or crash that bypassed our
        // cleanup). A lingering daemon holds the engine's lock, so our fresh daemon would exit with
        // "daemon is already running!" and never report Accessibility — leaving the UI banner
        // stuck at "not granted" no matter what the user granted. Kill stale ones first.
        kill_stale_engines();

        let cmd = app
            .shell()
            .sidecar("glyphio-engine")?
            .args(["daemon"])
            .env("ESPANSO_CONFIG_DIR", s(&paths.engine_config))
            .env("ESPANSO_RUNTIME_DIR", s(&paths.engine_runtime()))
            .env("ESPANSO_PACKAGE_DIR", s(&paths.engine_packages()))
            // The fork's `glyphio` render extension calls back here for popup/form kinds.
            .env("GLYPHIO_IPC_SOCKET", s(&paths.bridge_socket()));

        self.stopping.store(false, Ordering::SeqCst);
        let (mut rx, child) = cmd.spawn()?;
        let app_ev = app.clone();
        let ax_ok = self.accessibility_ok.clone();
        let ax_reported = self.accessibility_reported.clone();
        let secure_holder = self.secure_input_holder.clone();
        let child_slot = self.child.clone();
        let stopping = self.stopping.clone();
        let respawns = self.respawns.clone();
        tauri::async_runtime::spawn(async move {
            while let Some(event) = rx.recv().await {
                let (line, is_err) = match event {
                    CommandEvent::Stdout(l) => (String::from_utf8_lossy(&l).trim_end().to_string(), false),
                    CommandEvent::Stderr(l) => (String::from_utf8_lossy(&l).trim_end().to_string(), true),
                    CommandEvent::Terminated(payload) => {
                        // An engine that dies unexpectedly must be respawned, or the app keeps
                        // believing it's running: expansion silently stops and the user
                        // (reasonably) blames permissions. Bounded to avoid crash-looping.
                        child_slot.lock().unwrap().take();
                        if stopping.load(Ordering::SeqCst) {
                            log::info!("[engine] daemon stopped (requested)");
                            continue;
                        }
                        let n = respawns.fetch_add(1, Ordering::SeqCst) + 1;
                        if n > MAX_RESPAWNS {
                            log::error!(
                                "[engine] daemon exited ({payload:?}) — giving up after {MAX_RESPAWNS} respawns"
                            );
                            continue;
                        }
                        log::warn!("[engine] daemon exited ({payload:?}) — respawning ({n}/{MAX_RESPAWNS})");
                        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                        let state = app_ev.state::<crate::AppState>();
                        if let Err(e) = state.supervisor.start(&app_ev, &state.paths) {
                            log::error!("[engine] respawn failed: {e}");
                        }
                        continue;
                    }
                    _ => continue,
                };
                // Parse the daemon's Accessibility report (see the fork's daemon gate).
                if line.contains("Accessibility permission") {
                    let granted = line.contains("is granted");
                    ax_ok.store(granted, Ordering::SeqCst);
                    ax_reported.store(true, Ordering::SeqCst);
                    let _ = app_ev.emit("accessibility-status", granted);
                }
                // Parse Secure Input transitions. While held, typed triggers can't work
                // (no event tap on the system sees keystrokes) — surface WHO holds it so
                // the settings banner + tray can explain the pause instead of looking broken.
                if line.contains("secure input has been acquired") {
                    let holder = line
                        .split("caused by '")
                        .nth(1)
                        .and_then(|r| r.split('\'').next())
                        .unwrap_or("another app")
                        .trim_end_matches(".app")
                        .to_string();
                    *secure_holder.lock().unwrap() = Some(holder.clone());
                    let _ = app_ev.emit("secure-input-status", Some(holder.clone()));
                    update_tray_tooltip(&app_ev, Some(&holder));
                } else if line.contains("secure input has been disabled") {
                    *secure_holder.lock().unwrap() = None;
                    let _ = app_ev.emit("secure-input-status", Option::<String>::None);
                    update_tray_tooltip(&app_ev, None);
                }
                if is_err {
                    log::warn!("[engine] {line}");
                } else {
                    log::info!("[engine] {line}");
                }
            }
        });
        *self.child.lock().unwrap() = Some(child);
        log::info!("engine daemon started (config: {})", s(&paths.engine_config));
        Ok(())
    }

    /// Kill the sidecar (called on app exit). Kills our tracked daemon child *and* sweeps any
    /// engine processes (the daemon spawns a separate `worker` that must not be left orphaned).
    pub fn stop(&self) {
        self.stopping.store(true, Ordering::SeqCst); // deliberate — exit watcher must not respawn
        if let Some(child) = self.child.lock().unwrap().take() {
            if let Err(e) = child.kill() {
                log::warn!("failed to kill engine daemon: {e}");
            }
        }
        kill_stale_engines();
    }

    /// Restart to force a full config reload (rarely needed — file-watch handles most changes).
    pub fn restart(&self, app: &AppHandle, paths: &AppPaths) -> anyhow::Result<()> {
        self.stop();
        // Clear the last-known Accessibility state so a stale value can't linger across the
        // restart; the fresh daemon (or its periodic re-check) reports the current status.
        self.accessibility_ok.store(false, Ordering::SeqCst);
        self.accessibility_reported.store(false, Ordering::SeqCst);
        self.respawns.store(0, Ordering::SeqCst); // manual restart resets the crash-loop budget
        self.start(app, paths)
    }
}

/// Reflect Secure Input state in the menu-bar tooltip — the one Glyphio surface that's
/// always reachable while the user wonders why typing a trigger did nothing.
fn update_tray_tooltip(app: &AppHandle, holder: Option<&str>) {
    if let Some(tray) = app.tray_by_id("glyphio-tray") {
        let tip = match holder {
            Some(h) => format!("Glyphio — expansion paused: {h} holds macOS Secure Input"),
            None => "Glyphio".to_string(),
        };
        let _ = tray.set_tooltip(Some(tip));
    }
}

/// SIGKILL any lingering `glyphio-engine` processes (daemon + worker) not under our control.
/// the engine's daemon spawns the worker as a separate process and guards startup with a lock file,
/// so an orphan from a prior run must be cleared or the next daemon can't start. macOS-only helper;
/// matches on the unique sidecar name so it can't hit unrelated processes.
fn kill_stale_engines() {
    let _ = std::process::Command::new("pkill")
        .args(["-9", "-f", "glyphio-engine"])
        .status();
    // Give the OS a moment to release the engine's lock file before we (re)spawn.
    std::thread::sleep(std::time::Duration::from_millis(300));
}
