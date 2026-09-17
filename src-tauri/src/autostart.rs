//! Native adapter for the macOS login item.
//!
//! On macOS the autostart plugin has two backends.  A `LaunchAgent` creates a plist in
//! `~/Library/LaunchAgents`, which is indistinguishable from common persistence malware to EDR
//! products.  Glyphio deliberately uses the user-visible Login Items backend instead.

use std::{env, fs};

use tauri::AppHandle;
use tauri_plugin_autostart::ManagerExt;

const LEGACY_LAUNCH_AGENT_NAMES: &[&str] = &["glyphio", "Glyphio"];

/// Make the system login item match the persisted user preference.
pub fn set_enabled(app: &AppHandle, enabled: bool) -> anyhow::Result<()> {
    remove_legacy_launch_agent()?;

    let manager = app.autolaunch();
    if manager.is_enabled()? == enabled {
        return Ok(());
    }
    if enabled {
        manager.enable()?;
    } else {
        manager.disable()?;
    }
    Ok(())
}

/// Remove only plist entries written by Glyphio's former LaunchAgent backend.
///
/// Earlier versions created `~/Library/LaunchAgents/{glyphio,Glyphio}.plist`. Leaving that entry
/// behind would keep the old persistence mechanism active after an upgrade. We verify that the
/// plist targets this exact executable before removing it, so similarly named user entries stay
/// untouched.
fn remove_legacy_launch_agent() -> anyhow::Result<()> {
    let executable = env::current_exe()?;
    let executable = executable.to_string_lossy();
    let Some(home) = dirs::home_dir() else {
        return Ok(());
    };

    let directory = home.join("Library").join("LaunchAgents");
    for name in LEGACY_LAUNCH_AGENT_NAMES {
        let plist = directory.join(format!("{name}.plist"));
        if !plist.is_file() {
            continue;
        }

        let contents = fs::read_to_string(&plist)?;
        if targets_executable(&contents, executable.as_ref()) {
            fs::remove_file(&plist)?;
            log::info!("removed legacy Glyphio LaunchAgent: {}", plist.display());
        } else {
            log::warn!(
                "not removing unexpected LaunchAgent with a Glyphio name: {}",
                plist.display()
            );
        }
    }
    Ok(())
}

fn targets_executable(plist: &str, executable: &str) -> bool {
    plist.contains(&format!("<string>{executable}</string>"))
}

#[cfg(test)]
mod tests {
    use super::targets_executable;

    #[test]
    fn only_migrates_a_launch_agent_that_targets_glyphio() {
        assert!(targets_executable(
            "<array><string>/Applications/Glyphio.app/Contents/MacOS/glyphio</string></array>",
            "/Applications/Glyphio.app/Contents/MacOS/glyphio",
        ));
        assert!(!targets_executable(
            "<string>/Applications/Other.app/Contents/MacOS/other</string>",
            "/Applications/Glyphio.app/Contents/MacOS/glyphio",
        ));
    }
}
