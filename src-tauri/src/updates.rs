//! Staying current. Checks GitHub Releases for a newer Glyphio and, with the user's say-so,
//! installs it.
//!
//! # Two signatures, two different jobs
//!
//! An update is verified by a **minisign** key that belongs to this project (its public half is
//! in `tauri.conf.json`, its private half never leaves the maintainer's machine). That is what
//! makes an update authentic, and it is deliberately independent of Apple: the updater works
//! identically on a self-signed build and a notarized one, so the project can ship fixes
//! without a paid Developer ID.
//!
//! Apple's signature answers a different question — whether *Gatekeeper* will open the app at
//! all. See `scripts/release.sh`.
//!
//! # Installs we must not touch
//!
//! A Homebrew cask is owned by Homebrew: it tracks the installed version, and an app that
//! replaces itself underneath leaves `brew` convinced it has an older build than it does, so
//! the next `brew upgrade` happily reinstalls the version the user already moved past. When we
//! detect that kind of install we tell the user the command to run instead of self-updating.

use tauri::{AppHandle, Manager};
use tauri_plugin_updater::UpdaterExt;

/// How the running copy of Glyphio got here, which decides who is allowed to replace it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Install {
    /// A DMG drag-install (or a build run from source) — ours to update.
    SelfManaged,
    /// Installed by Homebrew, which owns the version it thinks is present.
    Homebrew,
}

impl Install {
    /// Homebrew stages a cask under its Caskroom and links the result into `/Applications`, so
    /// the bundle is either inside the Caskroom or a symlink pointing into it.
    pub fn detect(app: &AppHandle) -> Self {
        let Ok(exe) = app.path().resource_dir().or_else(|_| std::env::current_exe()) else {
            return Install::SelfManaged;
        };
        let real = std::fs::canonicalize(&exe).unwrap_or(exe);
        if real.components().any(|c| c.as_os_str() == "Caskroom") {
            Install::Homebrew
        } else {
            Install::SelfManaged
        }
    }
}

/// What a check found, for the settings pane to render.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase", tag = "state")]
pub enum Status {
    UpToDate { version: String },
    Available { version: String, notes: String, date: String },
    /// A newer version exists but Homebrew owns this install — hand over the command.
    ManagedElsewhere { version: String, command: String },
    Failed { error: String },
}

/// Ask the endpoint whether there is anything newer. Never installs.
pub async fn check(app: &AppHandle) -> Status {
    let current = app.package_info().version.to_string();
    let updater = match app.updater() {
        Ok(u) => u,
        Err(e) => return Status::Failed { error: e.to_string() },
    };
    match updater.check().await {
        Ok(Some(update)) => {
            if Install::detect(app) == Install::Homebrew {
                Status::ManagedElsewhere {
                    version: update.version.clone(),
                    command: "brew upgrade --cask glyphio".into(),
                }
            } else {
                Status::Available {
                    version: update.version.clone(),
                    notes: update.body.clone().unwrap_or_default(),
                    date: update.date.map(|d| d.to_string()).unwrap_or_default(),
                }
            }
        }
        Ok(None) => Status::UpToDate { version: current },
        Err(e) => {
            // Offline, or GitHub is having a day. Not worth a dialog — the user did not ask
            // for this if it came from the startup check.
            log::warn!("update check failed: {e}");
            Status::Failed { error: e.to_string() }
        }
    }
}

/// Download and install the pending update, then restart into it.
///
/// The bytes are verified against the bundled public key before anything is written; a payload
/// that fails that check is an error, not a slower install.
pub async fn install(app: &AppHandle) -> anyhow::Result<()> {
    if Install::detect(app) == Install::Homebrew {
        anyhow::bail!("this copy is managed by Homebrew — run `brew upgrade --cask glyphio`");
    }
    let update = app
        .updater()?
        .check()
        .await?
        .ok_or_else(|| anyhow::anyhow!("no update available"))?;

    let mut downloaded = 0usize;
    update
        .download_and_install(
            |chunk, total| {
                downloaded += chunk;
                if let Some(total) = total {
                    log::info!("update: {downloaded}/{total} bytes");
                }
            },
            || log::info!("update downloaded — installing"),
        )
        .await?;

    // The engine sidecar must not outlive the bundle it came from.
    app.state::<crate::AppState>().supervisor.stop();
    app.restart();
}

/// The quiet check that runs a little after launch.
///
/// Deliberately does nothing but log unless something is waiting: an app that opens a window on
/// startup to talk about itself is an app people quit. The settings pane is where a user goes
/// to ask, and [`Status`] is what it shows them.
pub fn check_in_background(app: &AppHandle) {
    // The only network call an otherwise-unconfigured Glyphio makes, so it is the user's to
    // decline. Declining does not disable the button in Settings → About — that is them asking.
    if !app.state::<crate::AppState>().settings.lock().unwrap().check_for_updates {
        log::info!("automatic update checks are off");
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // Let launch settle — the engine and the tray matter more than this does.
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        match check(&app).await {
            Status::Available { version, .. } => {
                use tauri::Emitter;
                log::info!("update available: {version}");
                let _ = app.emit("update-available", version);
            }
            Status::ManagedElsewhere { version, .. } => {
                log::info!("update available: {version} (Homebrew-managed install)");
            }
            Status::UpToDate { version } => log::info!("up to date ({version})"),
            Status::Failed { .. } => {}
        }
    });
}
