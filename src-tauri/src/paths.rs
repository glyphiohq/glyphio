//! Resolved on-disk locations for Glyphio's data. Everything lives under one app-data root:
//! `~/Library/Application Support/Glyphio/` on macOS, `~/.local/share/Glyphio/` on Linux.
//!
//! A **portable copy** keeps that root in a [`PORTABLE_DIR_NAME`] folder next to the app
//! instead (beside `Glyphio.app`, the AppImage, or the unpacked portable folder), so it runs from
//! a USB stick or any folder without touching the user's profile. The folder's presence is the
//! switch: create it and that copy is portable. See [`DataLocation`].

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::Context;

/// The folder next to the app that turns a copy portable.
pub const PORTABLE_DIR_NAME: &str = "Glyphio Data";

/// Where this copy keeps its data, settled once per process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataLocation {
    pub root: PathBuf,
    /// The data lives beside the app rather than in the user's profile. A portable copy never
    /// registers a login item and is never offered a move into Applications.
    pub portable: bool,
    /// Where the log goes: inside the root when portable, the OS's log folder otherwise.
    pub logs: PathBuf,
}

/// Decide the data location. `app_dir` is the folder the user sees the app in, `data_dir` the
/// OS's per-user data folder and `log_dir` its log folder for Glyphio. Pure, so it is tested
/// without touching the real profile.
pub fn choose_location(
    app_dir: Option<&Path>,
    data_dir: Option<PathBuf>,
    log_dir: Option<PathBuf>,
) -> anyhow::Result<DataLocation> {
    if let Some(portable) = app_dir.map(|d| d.join(PORTABLE_DIR_NAME)).filter(|p| p.is_dir()) {
        return Ok(DataLocation { logs: portable.join("logs"), root: portable, portable: true });
    }
    let root = data_dir.context("could not resolve platform data dir")?.join("Glyphio");
    let logs = log_dir.unwrap_or_else(|| root.join("logs"));
    Ok(DataLocation { root, portable: false, logs })
}

static LOCATION: OnceLock<DataLocation> = OnceLock::new();

/// This process's data location. Resolved on first use and fixed for the life of the process,
/// so the log, the stores and Settings never disagree about where they are.
pub fn location() -> &'static DataLocation {
    LOCATION.get_or_init(|| {
        let platform = crate::platform::folders();
        choose_location(platform.app_dir.as_deref(), dirs::data_dir(), platform.log_dir).unwrap_or_else(|e| {
            // Without a home there is nowhere sensible; a temp root keeps the app diagnosable.
            eprintln!("glyphio: {e:#}; keeping data in the temp folder");
            let root = std::env::temp_dir().join("Glyphio");
            DataLocation { logs: root.join("logs"), root, portable: false }
        })
    })
}

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub root: PathBuf,
    /// SQLite source-of-truth for snippets.
    pub snippets_db: PathBuf,
    /// Expansion-engine config dir (holds `config/`, `match/`, `packages/`, `runtime/`). A
    /// generated artifact — regenerated from `snippets_db` on every change.
    pub engine_config: PathBuf,
    /// SQLite metadata store for capture history.
    pub history_db: PathBuf,
    /// On-disk PNG blobs for capture history (never synced).
    pub history_images: PathBuf,
    /// SQLite store for clipboard history (never synced — see `docs/SECURITY.md`).
    pub clipboard_db: PathBuf,
    /// On-disk PNG blobs for copied images.
    pub clipboard_images: PathBuf,
    /// JSON settings file (replaces Checkpoint's chrome.storage.sync).
    pub settings_json: PathBuf,
}

impl AppPaths {
    pub fn resolve() -> anyhow::Result<Self> {
        let root = location().root.clone();
        let engine_config = root.join("espanso"); // on-disk name kept for existing installs
        let history = root.join("history");

        // The engine refuses to start unless these exist.
        for sub in ["config", "match", "packages", "runtime"] {
            std::fs::create_dir_all(engine_config.join(sub))?;
        }
        let runtime = runtime_dir_for(&engine_config.join("runtime"), &root, short_runtime_base());
        if !runtime.starts_with(&engine_config) {
            create_private_dir(&runtime)?;
        }
        let history_images = history.join("images");
        std::fs::create_dir_all(&history_images)?;
        let clipboard = root.join("clipboard");
        let clipboard_images = clipboard.join("images");
        std::fs::create_dir_all(&clipboard_images)?;

        Ok(Self {
            snippets_db: root.join("snippets.db"),
            engine_config,
            history_db: history.join("history.db"),
            history_images,
            clipboard_db: clipboard.join("clipboard.db"),
            clipboard_images,
            settings_json: root.join("settings.json"),
            root,
        })
    }

    /// The engine's runtime folder: its lock, pid file and sockets. Normally inside the engine
    /// config; see [`runtime_dir_for`] for when it moves.
    pub fn engine_runtime(&self) -> PathBuf {
        runtime_dir_for(&self.engine_config.join("runtime"), &self.root, short_runtime_base())
    }
    /// Unix socket the engine's `glyphio` render extension calls back on (popup/form kinds).
    pub fn bridge_socket(&self) -> PathBuf {
        self.engine_runtime().join("bridge.sock")
    }
    pub fn engine_packages(&self) -> PathBuf {
        self.engine_config.join("packages")
    }
    pub fn image_path(&self, id: &str, kind: &str) -> PathBuf {
        self.history_images.join(format!("{id}-{kind}.png"))
    }
}

/// Longest runtime folder that still fits the engine's socket names (`/espansoworkerv2.sock`,
/// 21 bytes) inside a Unix socket address, which macOS caps at 103 bytes.
const MAX_RUNTIME_DIR: usize = 80;

/// The engine's runtime folder. `default` unless its sockets would not fit a socket address,
/// which happens when the data root is deep (a portable copy on a stick with a long name).
/// Then a short folder under `short_base`, named by a stable hash of `root` so two portable
/// copies never share one and every launch of the same copy finds the same one.
pub fn runtime_dir_for(default: &Path, root: &Path, short_base: Option<PathBuf>) -> PathBuf {
    if default.as_os_str().len() <= MAX_RUNTIME_DIR {
        return default.to_path_buf();
    }
    match short_base {
        Some(base) => base.join(format!("glyphio-{:016x}", fnv1a(root.as_os_str().as_encoded_bytes()))),
        None => default.to_path_buf(),
    }
}

/// The per-user runtime folder where there is one (`$XDG_RUNTIME_DIR`), the temp folder otherwise.
fn short_runtime_base() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| Some(std::env::temp_dir()))
}

/// Create `dir` for this user only. An existing folder someone else made is refused: the engine's
/// sockets would be reachable through it.
fn create_private_dir(dir: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    crate::platform::files::restrict_dir_to_owner(dir)
        .with_context(|| format!("{} belongs to someone else", dir.display()))?;
    if crate::platform::files::is_owner_only(dir) == Some(false) {
        anyhow::bail!("{} is readable by other users", dir.display());
    }
    Ok(())
}

/// 64-bit FNV-1a: tiny, and stable across Rust releases (unlike `DefaultHasher`), so a copy's
/// runtime folder doesn't move when Glyphio is rebuilt.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3))
}

/// Where Settings and setup learn whether this copy is portable.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppMode {
    pub portable: bool,
    pub data_dir: String,
}

#[tauri::command]
pub fn app_mode() -> AppMode {
    let l = location();
    AppMode { portable: l.portable, data_dir: s(&l.root) }
}

/// Best-effort string form of a path for passing as an env var.
pub fn s(p: &Path) -> String {
    p.to_string_lossy().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_data_folder_beside_the_app_makes_it_portable() {
        let app = tempfile::tempdir().unwrap();
        std::fs::create_dir(app.path().join(PORTABLE_DIR_NAME)).unwrap();
        let l = choose_location(Some(app.path()), Some("/home/a/.local/share".into()), None).unwrap();
        assert!(l.portable);
        assert_eq!(l.root, app.path().join(PORTABLE_DIR_NAME));
        assert_eq!(l.logs, app.path().join(PORTABLE_DIR_NAME).join("logs"));
    }

    #[test]
    fn without_one_the_data_stays_in_the_profile() {
        let app = tempfile::tempdir().unwrap();
        let l = choose_location(
            Some(app.path()),
            Some("/home/a/.local/share".into()),
            Some("/home/a/.local/state/Glyphio".into()),
        )
        .unwrap();
        assert_eq!(
            l,
            DataLocation {
                root: "/home/a/.local/share/Glyphio".into(),
                portable: false,
                logs: "/home/a/.local/state/Glyphio".into(),
            }
        );
    }

    #[test]
    fn a_file_with_the_folder_name_is_not_a_switch() {
        let app = tempfile::tempdir().unwrap();
        std::fs::write(app.path().join(PORTABLE_DIR_NAME), "").unwrap();
        let l = choose_location(Some(app.path()), Some("/d".into()), None).unwrap();
        assert!(!l.portable);
        assert_eq!(l.logs, PathBuf::from("/d/Glyphio/logs"));
    }

    #[test]
    fn a_deep_root_moves_the_runtime_somewhere_its_sockets_fit() {
        let short = Path::new("/home/a/.local/share/Glyphio/espanso/runtime");
        assert_eq!(runtime_dir_for(short, Path::new("/r"), Some("/run/user/1000".into())), short);

        let root = PathBuf::from(format!("/media/a/{}/Glyphio Data", "A Very Long Stick Name ".repeat(3)));
        let deep = root.join("espanso/runtime");
        let moved = runtime_dir_for(&deep, &root, Some("/run/user/1000".into()));
        assert!(moved.to_string_lossy().starts_with("/run/user/1000/glyphio-"), "{}", moved.display());
        assert!(moved.join("espansoworkerv2.sock").as_os_str().len() < 104);
        // Stable for the same copy, different for another.
        assert_eq!(moved, runtime_dir_for(&deep, &root, Some("/run/user/1000".into())));
        let other = root.with_file_name("Other Data");
        assert_ne!(moved, runtime_dir_for(&other.join("espanso/runtime"), &other, Some("/run/user/1000".into())));
        // With nowhere shorter to go, it stays put rather than failing.
        assert_eq!(runtime_dir_for(&deep, &root, None), deep);
    }

    #[test]
    fn the_hash_is_fixed_fnv1a() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn no_profile_and_no_portable_folder_is_an_error() {
        assert!(choose_location(None, None, None).is_err());
    }
}
