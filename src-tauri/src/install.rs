//! Where this copy of Glyphio runs from. A copy opened straight from a disk image, or one macOS
//! translocated (run from a randomized read-only path because it was never moved after
//! download), loses its permissions and updates on the next launch; a second installed copy
//! with the same bundle identifier splits the permission grants between two entries. Setup
//! offers to move the first and warns once about the second.
//!
//! The classification is pure; the file-system and launch calls go through
//! [`crate::platform::Installation`].

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::autostart::BUNDLE_IDENTIFIER;
use crate::platform::Installation;
use crate::AppState;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Location {
    Applications,
    UserApplications,
    /// Run by macOS from a randomized read-only path (Gatekeeper path randomization).
    Translocated,
    /// A disk image or other read-only volume.
    ReadOnlyVolume,
    /// Anywhere else, such as a development build. Left alone.
    Elsewhere,
}

/// The `.app` bundle that contains `exe`.
pub fn bundle_of(exe: &Path) -> Option<PathBuf> {
    exe.ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .map(Path::to_path_buf)
}

pub fn classify(bundle: &Path, home: &Path, read_only: bool) -> Location {
    if bundle.to_string_lossy().contains("/AppTranslocation/") {
        return Location::Translocated;
    }
    if read_only {
        return Location::ReadOnlyVolume;
    }
    match bundle.parent() {
        Some(p) if p == Path::new("/Applications") => Location::Applications,
        Some(p) if p == home.join("Applications") => Location::UserApplications,
        _ => Location::Elsewhere,
    }
}

/// Whether setup should offer to move this copy.
pub fn offers_move(location: Location) -> bool {
    matches!(location, Location::Translocated | Location::ReadOnlyVolume)
}

/// The folder to move into: Applications, or the user's own when that is not writable.
pub fn move_target(system_writable: bool, home: &Path) -> PathBuf {
    if system_writable {
        PathBuf::from("/Applications")
    } else {
        home.join("Applications")
    }
}

/// Installed copies other than `ours` that carry Glyphio's bundle identifier. `candidates` are
/// app bundles found in the Applications folders; only names mentioning Glyphio are read, so
/// launch never walks every app's Info.plist.
pub fn duplicates(
    ours: &Path,
    candidates: impl IntoIterator<Item = PathBuf>,
    identifier: impl Fn(&Path) -> Option<String>,
) -> Vec<PathBuf> {
    candidates
        .into_iter()
        .filter(|p| p != ours)
        .filter(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().to_lowercase().contains("glyphio"))
                .unwrap_or(false)
        })
        .filter(|p| identifier(p).as_deref() == Some(BUNDLE_IDENTIFIER))
        .collect()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallStatus {
    pub location: Location,
    pub bundle_path: Option<String>,
    pub offer_move: bool,
    /// Where a move would put the app.
    pub move_target: String,
    /// Other installed copies not yet warned about.
    pub duplicates: Vec<String>,
}

impl InstallStatus {
    /// Something setup should show at launch.
    pub fn needs_attention(&self) -> bool {
        self.offer_move || !self.duplicates.is_empty()
    }
}

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

fn app_bundles_in(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|e| e == "app"))
                .collect()
        })
        .unwrap_or_default()
}

/// Read where this copy runs from and which other copies exist.
pub fn status(os: &dyn Installation, warned: &[String]) -> InstallStatus {
    let home = home();
    let bundle = std::env::current_exe().ok().and_then(|exe| bundle_of(&exe));
    let location = match &bundle {
        Some(b) => classify(b, &home, os.read_only(b)),
        None => Location::Elsewhere,
    };
    let duplicates = match &bundle {
        // A translocated or disk-image copy is itself the transient one: the move resolves it.
        Some(b) if !offers_move(location) => {
            let candidates = app_bundles_in(Path::new("/Applications"))
                .into_iter()
                .chain(app_bundles_in(&home.join("Applications")));
            duplicates(b, candidates, |p| os.bundle_identifier(p))
                .into_iter()
                .map(|p| p.to_string_lossy().into_owned())
                .filter(|p| !warned.contains(p))
                .collect()
        }
        _ => Vec::new(),
    };
    InstallStatus {
        location,
        bundle_path: bundle.map(|b| b.to_string_lossy().into_owned()),
        offer_move: offers_move(location),
        move_target: move_target(os.dir_writable(Path::new("/Applications")), &home)
            .to_string_lossy()
            .into_owned(),
        duplicates,
    }
}

/// Current status for this launch.
pub fn current(app: &AppHandle) -> InstallStatus {
    let warned = app.state::<AppState>().settings.lock().unwrap().install_duplicates_warned.clone();
    status(&*crate::platform::current().install, &warned)
}

#[tauri::command]
pub fn install_status(app: AppHandle) -> InstallStatus {
    current(&app)
}

/// Copy this bundle into the Applications folder and relaunch from there. A translocated copy
/// is a read-only view of the downloaded app, so the original is left where it is.
#[tauri::command]
pub fn install_move(app: AppHandle) -> Result<(), String> {
    let status = current(&app);
    if !status.offer_move {
        return Err("Glyphio is already installed.".into());
    }
    let from = PathBuf::from(status.bundle_path.ok_or("cannot find this copy of Glyphio")?);
    let target_dir = PathBuf::from(&status.move_target);
    let to = target_dir.join(from.file_name().ok_or("unexpected app location")?);
    if to.exists() {
        return Err(format!(
            "{} already exists. Quit this copy and open that one, or move it to the Trash first.",
            to.display()
        ));
    }
    let os = &crate::platform::current().install;
    std::fs::create_dir_all(&target_dir).map_err(|e| format!("cannot create {}: {e}", target_dir.display()))?;
    os.copy_bundle(&from, &to).map_err(|e| format!("{e:#}"))?;

    // Stop the engine and give up the single-instance lock before the new copy starts, or it
    // would hand its launch back to this one and quit.
    {
        let state = app.state::<AppState>();
        state.supervisor.stop(&state.paths);
    }
    tauri_plugin_single_instance::destroy(&app);
    os.open_bundle(&to).map_err(|e| format!("{e:#}"))?;
    app.exit(0);
    Ok(())
}

/// Remember the duplicate copies the user has been told about, so the warning shows once.
#[tauri::command]
pub fn install_dismiss_duplicates(state: State<AppState>, paths: Vec<String>) -> Result<(), String> {
    let mut s = state.settings.lock().unwrap();
    for p in paths {
        if !s.install_duplicates_warned.contains(&p) {
            s.install_duplicates_warned.push(p);
        }
    }
    s.save(&state.paths.settings_json).map_err(|e| format!("{e:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/Users/a";

    #[test]
    fn finds_the_bundle_around_the_executable() {
        assert_eq!(
            bundle_of(Path::new("/Applications/Glyphio.app/Contents/MacOS/glyphio")),
            Some(PathBuf::from("/Applications/Glyphio.app"))
        );
        assert_eq!(bundle_of(Path::new("/usr/local/bin/glyphio")), None);
    }

    #[test]
    fn classifies_install_locations() {
        let home = Path::new(HOME);
        let c = |p: &str, ro| classify(Path::new(p), home, ro);
        assert_eq!(c("/Applications/Glyphio.app", false), Location::Applications);
        assert_eq!(c("/Users/a/Applications/Glyphio.app", false), Location::UserApplications);
        assert_eq!(
            c("/private/var/folders/xy/T/AppTranslocation/1A2B/d/Glyphio.app", true),
            Location::Translocated
        );
        assert_eq!(c("/Volumes/Glyphio 1.2.2/Glyphio.app", true), Location::ReadOnlyVolume);
        assert_eq!(c("/Users/a/src/glyphio/target/release/bundle/macos/Glyphio.app", false), Location::Elsewhere);
    }

    #[test]
    fn only_a_disk_image_or_translocated_copy_is_offered_a_move() {
        assert!(offers_move(Location::Translocated));
        assert!(offers_move(Location::ReadOnlyVolume));
        for l in [Location::Applications, Location::UserApplications, Location::Elsewhere] {
            assert!(!offers_move(l), "{l:?}");
        }
    }

    #[test]
    fn moves_to_user_applications_when_the_system_folder_is_not_writable() {
        let home = Path::new(HOME);
        assert_eq!(move_target(true, home), PathBuf::from("/Applications"));
        assert_eq!(move_target(false, home), PathBuf::from("/Users/a/Applications"));
    }

    #[test]
    fn duplicates_are_other_copies_with_our_identifier() {
        let ours = PathBuf::from("/Applications/Glyphio.app");
        let candidates = vec![
            ours.clone(),
            PathBuf::from("/Users/a/Applications/Glyphio.app"),
            PathBuf::from("/Applications/Glyphio 2.app"),
            PathBuf::from("/Applications/Glyphio Helper Unrelated.app"),
            PathBuf::from("/Applications/Safari.app"),
        ];
        let id = |p: &Path| match p.to_str().unwrap() {
            "/Applications/Glyphio Helper Unrelated.app" => Some("com.example.other".into()),
            "/Applications/Safari.app" => panic!("unrelated apps are never read"),
            _ => Some(BUNDLE_IDENTIFIER.to_string()),
        };
        assert_eq!(
            duplicates(&ours, candidates, id),
            vec![PathBuf::from("/Users/a/Applications/Glyphio.app"), PathBuf::from("/Applications/Glyphio 2.app")]
        );
    }
}
