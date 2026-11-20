//! Supervises the bundled expansion-engine binary (a GPL fork — see NOTICES.md) as a managed sidecar.
//!
//! We run the engine's hidden foreground `daemon` subcommand ("start the daemon
//! without spawning a new process") — as a child of the Tauri app, pointed at Glyphio's isolated
//! config dir via its `ESPANSO_*_DIR` env vars (the engine's own interface — renaming them would grow
//! the fork diff for cosmetics). It file-watches that dir and hot-reloads, so
//! regenerating config from the snippet store is all it takes to update live expansions.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_shell::process::{CommandChild, CommandEvent};
use tauri_plugin_shell::ShellExt;

use crate::paths::{s, AppPaths};
use budget::{RespawnBudget, Verdict, MAX_RESPAWNS};
use reaper::{reap, wait_until, Reaped};

mod budget;
mod os;
mod reaper;

/// How long to wait for a killed daemon's lock to clear before starting the next one.
const LOCK_WAIT: Duration = Duration::from_secs(3);

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
    budget: Arc<Mutex<RespawnBudget>>,
    /// When the current daemon was spawned, so a long healthy run refills the respawn budget.
    started_at: Arc<Mutex<Option<Instant>>>,
    /// The respawn budget is spent and the engine is left stopped (shown in tray and Settings).
    exhausted: Arc<AtomicBool>,
}

impl Supervisor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_running(&self) -> bool {
        self.child.lock().unwrap().is_some()
    }

    /// True once the engine crashed too often to be restarted automatically.
    pub fn exhausted(&self) -> bool {
        self.exhausted.load(Ordering::SeqCst)
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
        // Clear an orphaned engine from a previous run (a tray-quit or crash that bypassed our
        // cleanup). A lingering daemon holds the engine's lock, so our fresh daemon would exit with
        // "daemon is already running!" and never report Accessibility. Only the pid we recorded,
        // and only if it is this bundle's engine, is ever terminated.
        clear_stale(&paths.engine_runtime());

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
        let budget = self.budget.clone();
        let started_at = self.started_at.clone();
        let exhausted = self.exhausted.clone();
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
                        let ran_for = started_at
                            .lock()
                            .unwrap()
                            .map_or(Duration::ZERO, |t| t.elapsed());
                        let verdict = budget.lock().unwrap().on_exit(ran_for);
                        let n = match verdict {
                            Verdict::Respawn(n) => n,
                            Verdict::Exhausted => {
                                log::error!(
                                    "[engine] daemon exited ({payload:?}) — giving up after {MAX_RESPAWNS} respawns"
                                );
                                exhausted.store(true, Ordering::SeqCst);
                                let _ = app_ev.emit("engine-status", true);
                                update_tray_tooltip(&app_ev, None, true);
                                continue;
                            }
                        };
                        log::warn!("[engine] daemon exited ({payload:?}) — respawning ({n}/{MAX_RESPAWNS})");
                        tokio::time::sleep(Duration::from_secs(2)).await;
                        let app_re = app_ev.clone();
                        let result = tauri::async_runtime::spawn_blocking(move || {
                            let state = app_re.state::<crate::AppState>();
                            state.supervisor.start(&app_re, &state.paths)
                        })
                        .await;
                        match result {
                            Ok(Ok(())) => {}
                            Ok(Err(e)) => log::error!("[engine] respawn failed: {e}"),
                            Err(e) => log::error!("[engine] respawn task failed: {e}"),
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
                    update_tray_tooltip(&app_ev, Some(&holder), false);
                } else if line.contains("secure input has been disabled") {
                    *secure_holder.lock().unwrap() = None;
                    let _ = app_ev.emit("secure-input-status", Option::<String>::None);
                    update_tray_tooltip(&app_ev, None, false);
                }
                // Redacted: this is the only text in the log we didn't write ourselves, so it
                // is the only place a URL could arrive without anyone having decided to log one.
                let safe = crate::logging::redact(&line);
                if is_err {
                    log::warn!("[engine] {safe}");
                } else {
                    log::info!("[engine] {safe}");
                }
            }
        });
        os::write_pid(&paths.engine_runtime(), child.pid());
        *self.started_at.lock().unwrap() = Some(Instant::now());
        *self.child.lock().unwrap() = Some(child);
        log::info!("engine daemon started (config: {})", s(&paths.engine_config));
        Ok(())
    }

    /// Kill the sidecar (called on app exit): our tracked daemon and its worker (the daemon
    /// spawns a separate `worker` that must not be left orphaned), then wait for the lock.
    pub fn stop(&self, paths: &AppPaths) {
        self.stopping.store(true, Ordering::SeqCst); // deliberate — exit watcher must not respawn
        if let Some(child) = self.child.lock().unwrap().take() {
            reaper::ProcessControl::kill_tree(&os::Os, child.pid());
            if let Err(e) = child.kill() {
                log::debug!("engine daemon already gone: {e}");
            }
            wait_for_lock(&paths.engine_runtime());
        }
        os::clear_pid(&paths.engine_runtime());
    }

    /// Restart to force a full config reload (rarely needed — file-watch handles most changes).
    pub fn restart(&self, app: &AppHandle, paths: &AppPaths) -> anyhow::Result<()> {
        self.stop(paths);
        // Clear the last-known Accessibility state so a stale value can't linger across the
        // restart; the fresh daemon (or its periodic re-check) reports the current status.
        self.accessibility_ok.store(false, Ordering::SeqCst);
        self.accessibility_reported.store(false, Ordering::SeqCst);
        self.budget.lock().unwrap().reset(); // manual restart resets the crash-loop budget
        if self.exhausted.swap(false, Ordering::SeqCst) {
            let _ = app.emit("engine-status", false);
            update_tray_tooltip(app, None, false);
        }
        self.start(app, paths)
    }
}

/// Reflect engine state in the menu-bar tooltip — the one Glyphio surface that's always
/// reachable while the user wonders why typing a trigger did nothing.
fn update_tray_tooltip(app: &AppHandle, holder: Option<&str>, exhausted: bool) {
    if let Some(tray) = app.tray_by_id("glyphio-tray") {
        let tip = match (exhausted, holder) {
            (true, _) => "Glyphio — expansion engine stopped; choose Reload to restart it".to_string(),
            (false, Some(h)) => format!("Glyphio — expansion paused: {h} holds macOS Secure Input"),
            (false, None) => "Glyphio".to_string(),
        };
        let _ = tray.set_tooltip(Some(tip));
    }
}

/// Terminate the engine recorded by a previous run, if it is this bundle's, and wait for its
/// lock only when something was actually killed.
fn clear_stale(runtime: &std::path::Path) {
    let Some(engine_exe) = os::bundled_engine_exe() else {
        log::warn!("[engine] cannot resolve the bundled engine path; not clearing stale engines");
        return;
    };
    match reap(&os::Os, os::read_pid(runtime), &engine_exe) {
        Reaped::Killed(pid) => {
            log::info!("[engine] terminated stale daemon {pid}");
            wait_for_lock(runtime);
        }
        Reaped::Foreign(pid) => {
            log::info!("[engine] recorded pid {pid} is not this bundle's engine; left running");
        }
        Reaped::Gone(_) | Reaped::NothingRecorded => {}
    }
    os::clear_pid(runtime);
}

fn wait_for_lock(runtime: &std::path::Path) {
    let freed = wait_until(
        || os::daemon_lock_free(runtime),
        LOCK_WAIT,
        Duration::from_millis(20),
        std::thread::sleep,
    );
    if !freed {
        log::warn!("[engine] daemon lock still held after {LOCK_WAIT:?}");
    }
}
