//! Resolved on-disk locations for Glyphio's data. Everything lives under one app-data root:
//! `~/Library/Application Support/Glyphio/` on macOS.

use std::path::{Path, PathBuf};

use anyhow::Context;

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
    /// JSON settings file (replaces Checkpoint's chrome.storage.sync).
    pub settings_json: PathBuf,
}

impl AppPaths {
    pub fn resolve() -> anyhow::Result<Self> {
        let root = dirs::data_dir()
            .context("could not resolve platform data dir")?
            .join("Glyphio");
        let engine_config = root.join("espanso"); // on-disk name kept for existing installs
        let history = root.join("history");

        // The engine refuses to start unless these exist.
        for sub in ["config", "match", "packages", "runtime"] {
            std::fs::create_dir_all(engine_config.join(sub))?;
        }
        let history_images = history.join("images");
        std::fs::create_dir_all(&history_images)?;

        Ok(Self {
            snippets_db: root.join("snippets.db"),
            engine_config,
            history_db: history.join("history.db"),
            history_images,
            settings_json: root.join("settings.json"),
            root,
        })
    }

    pub fn engine_runtime(&self) -> PathBuf {
        self.engine_config.join("runtime")
    }
    pub fn engine_packages(&self) -> PathBuf {
        self.engine_config.join("packages")
    }
    pub fn image_path(&self, id: &str, kind: &str) -> PathBuf {
        self.history_images.join(format!("{id}-{kind}.png"))
    }
}

/// Best-effort string form of a path for passing as an env var.
pub fn s(p: &Path) -> String {
    p.to_string_lossy().to_string()
}
