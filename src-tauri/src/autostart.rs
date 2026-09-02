//! Native adapter for the macOS login item.

use tauri::AppHandle;
use tauri_plugin_autostart::ManagerExt;

/// Make the system login item match the persisted user preference.
pub fn set_enabled(app: &AppHandle, enabled: bool) -> anyhow::Result<()> {
    let manager = app.autolaunch();
    if enabled {
        manager.enable()?;
    } else {
        manager.disable()?;
    }
    Ok(())
}
