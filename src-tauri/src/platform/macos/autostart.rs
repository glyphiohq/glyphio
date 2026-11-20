//! The macOS login item: `SMAppService.mainApp`, plus clearing what older builds left behind.

use std::path::Path;

mod native {
    use objc2::rc::Retained;
    use objc2_service_management::{SMAppService, SMAppServiceStatus};

    use crate::autostart::{LoginItem, Status};

    /// `SMAppService.mainApp`.
    pub struct MainApp(Retained<SMAppService>);

    impl MainApp {
        pub fn new() -> Self {
            // SAFETY: a class method with no preconditions.
            Self(unsafe { SMAppService::mainAppService() })
        }
    }

    impl LoginItem for MainApp {
        fn status(&self) -> Status {
            // SAFETY: reads the registration state; no prompt, no subprocess.
            match unsafe { self.0.status() } {
                SMAppServiceStatus::Enabled => Status::Enabled,
                SMAppServiceStatus::RequiresApproval => Status::RequiresApproval,
                SMAppServiceStatus::NotFound => Status::NotFound,
                _ => Status::NotRegistered,
            }
        }
        fn register(&mut self) -> Result<(), String> {
            // SAFETY: plain message send; the error, if any, is returned to us.
            unsafe { self.0.registerAndReturnError() }
                .map_err(|e| e.localizedDescription().to_string())
        }
        fn unregister(&mut self) -> Result<(), String> {
            // SAFETY: as above.
            unsafe { self.0.unregisterAndReturnError() }
                .map_err(|e| e.localizedDescription().to_string())
        }
    }
}

use native::MainApp;

/// Bring the login item in line with a changed setting.
pub fn set_enabled(want: bool) -> crate::autostart::Outcome {
    crate::autostart::apply_setting(&mut MainApp::new(), want)
}

/// Launch-time reconciliation; reads the status and registers at most once.
pub fn reconcile(want: bool) -> crate::autostart::Outcome {
    crate::autostart::reconcile_at_launch(&mut MainApp::new(), want)
}

/// Delete the login item the old AppleScript backend added, and any LaunchAgent plist it or an
/// earlier build wrote. Runs once per install: the scripted call is the one remaining use of
/// `osascript`, and it is best-effort.
pub fn migrate_legacy(marker: &Path, existing_install: bool) -> crate::autostart::Migration {
    remove_legacy_launch_agents();
    let outcome = crate::autostart::migrate_scripted_item(marker.exists() || !existing_install, || {
        let out = std::process::Command::new("/usr/bin/osascript")
            .args([
                "-e",
                "tell application \"System Events\" to delete (every login item whose name is \"Glyphio\")",
            ])
            .output()
            .map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    });
    if !marker.exists() {
        let _ = std::fs::write(marker, "1"); // a fresh install counts as done too
    }
    outcome
}

fn remove_legacy_launch_agents() {
    let Some(home) = dirs::home_dir() else { return };
    let dir = home.join("Library").join("LaunchAgents");
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("plist") {
            continue;
        }
        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
        if !name.contains("glyphio") {
            continue;
        }
        let Ok(contents) = std::fs::read_to_string(&path) else { continue };
        if crate::autostart::is_legacy_glyphio_agent(&contents, bundle_identifier_of) {
            match std::fs::remove_file(&path) {
                Ok(()) => log::info!("removed legacy Glyphio LaunchAgent: {}", path.display()),
                Err(e) => log::warn!("could not remove {}: {e}", path.display()),
            }
        } else {
            log::warn!("not removing unexpected LaunchAgent with a Glyphio name: {}", path.display());
        }
    }
}

/// `CFBundleIdentifier` of the app bundle at `bundle`, read from its Info.plist.
fn bundle_identifier_of(bundle: &Path) -> Option<String> {
    let info = std::fs::read_to_string(bundle.join("Contents/Info.plist")).ok()?;
    let after = &info[info.find("<key>CFBundleIdentifier</key>")?..];
    let start = after.find("<string>")? + "<string>".len();
    let end = start + after[start..].find("</string>")?;
    Some(after[start..end].trim().to_string())
}

