//! Capture history: native replacement for Checkpoint's IndexedDB store.
//! Metadata in SQLite, PNG blobs on disk. Preserves Checkpoint's behavior exactly, including
//! oldest-first eviction at 50 captures OR 200 MB (whichever fills first). History is device-local
//! and NEVER synced.

use std::path::PathBuf;
use std::sync::Mutex;

use chrono::Utc;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::paths::AppPaths;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureMeta {
    pub id: String,
    /// Original capture time — immutable. Banner add/remove later never changes it; the
    /// banner timestamp is always rendered from this value.
    pub captured_at: String,
    pub url: String, // window/app title natively
    pub title: String,
    /// What the browser said about the page, when the capture targeted one window and the
    /// user asked for it. Kept per row so the banner renders the same months later.
    #[serde(default)]
    pub page_title: String,
    #[serde(default)]
    pub page_url: String,
    #[serde(default)]
    pub profile: String,
    pub mode: String, // "visible" | "snip" | "fullWindow"
    pub image_width_px: i64,
    pub image_height_px: i64,
    pub dpr: f64,
    pub size_bytes: i64,
    /// Absolute on-disk paths (frontend loads them via convertFileSrc).
    pub full_path: String,
    pub thumb_path: String,
    /// Banner note (editable after the fact — the stored PNG is content-only).
    pub note: String,
    /// Whether the banner is composited on view/copy/export of this capture.
    pub banner_enabled: bool,
    /// Legacy rows (pre-separation): the stored PNG has the banner baked in, so banner/note
    /// edits are locked. New saves always store content-only and set this false.
    pub banner_baked: bool,
}

/// What the frontend passes when persisting a capture (post-edit). The PNG is content-only
/// (crop/redact/draw baked; no banner) — the banner is composited at view/export time.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewCapture {
    /// Capture completion time supplied by the artifact. Empty for callers restoring older data.
    #[serde(default)]
    pub captured_at: String,
    pub url: String,
    pub title: String,
    #[serde(default)]
    pub page_title: String,
    #[serde(default)]
    pub page_url: String,
    #[serde(default)]
    pub profile: String,
    pub mode: String,
    pub image_width_px: i64,
    pub image_height_px: i64,
    pub dpr: f64,
    #[serde(default)]
    pub note: String,
    #[serde(default = "default_true")]
    pub banner_enabled: bool,
}

fn default_true() -> bool {
    true
}

/// Post-save edits to a stored capture. `None` = leave unchanged. A new content PNG
/// (re-crop, redact…) comes with its dimensions; `captured_at` is never updatable.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureUpdate {
    pub note: Option<String>,
    pub banner_enabled: Option<bool>,
    pub image_width_px: Option<i64>,
    pub image_height_px: Option<i64>,
}

pub struct HistoryStore {
    conn: Mutex<Connection>,
    images_dir: PathBuf,
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS captures (
    id              TEXT PRIMARY KEY,
    captured_at     TEXT NOT NULL,
    url             TEXT,
    title           TEXT,
    mode            TEXT,
    image_width_px  INTEGER,
    image_height_px INTEGER,
    dpr             REAL,
    full_path       TEXT NOT NULL,
    thumb_path      TEXT NOT NULL,
    size_bytes      INTEGER NOT NULL,
    note            TEXT NOT NULL DEFAULT '',
    banner_enabled  INTEGER NOT NULL DEFAULT 1,
    banner_baked    INTEGER NOT NULL DEFAULT 0,
    page_title      TEXT NOT NULL DEFAULT '',
    page_url        TEXT NOT NULL DEFAULT '',
    profile         TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_captures_captured_at ON captures(captured_at DESC);
"#;

/// Columns added after the initial release. Rows that predate the split stored the banner
/// baked into the PNG, so `banner_baked` defaults to 1 here (vs 0 in SCHEMA for new tables).
const MIGRATIONS: &[&str] = &[
    "ALTER TABLE captures ADD COLUMN note TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE captures ADD COLUMN banner_enabled INTEGER NOT NULL DEFAULT 1",
    "ALTER TABLE captures ADD COLUMN banner_baked INTEGER NOT NULL DEFAULT 1",
    "ALTER TABLE captures ADD COLUMN page_title TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE captures ADD COLUMN page_url TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE captures ADD COLUMN profile TEXT NOT NULL DEFAULT ''",
];

const COLUMNS: &str = "id, captured_at, url, title, mode, image_width_px, image_height_px, \
                       dpr, size_bytes, full_path, thumb_path, note, banner_enabled, \
                       banner_baked, page_title, page_url, profile";

impl HistoryStore {
    pub fn open(paths: &AppPaths) -> anyhow::Result<Self> {
        let conn = Connection::open(&paths.history_db)?;
        conn.execute_batch(SCHEMA)?;
        for m in MIGRATIONS {
            // Fresh tables already have the column (SCHEMA); ignore "duplicate column".
            if let Err(e) = conn.execute(m, []) {
                let msg = e.to_string();
                if !msg.contains("duplicate column") {
                    return Err(e.into());
                }
            }
        }
        Ok(Self {
            conn: Mutex::new(conn),
            images_dir: paths.history_images.clone(),
        })
    }

    fn full_path(&self, id: &str) -> PathBuf {
        self.images_dir.join(format!("{id}-full.png"))
    }
    fn thumb_path(&self, id: &str) -> PathBuf {
        self.images_dir.join(format!("{id}-thumb.png"))
    }

    /// Persist a capture: write both PNGs, insert the row, then enforce retention.
    pub fn save(
        &self,
        meta: NewCapture,
        full_png: &[u8],
        thumb_png: &[u8],
        max_count: u32,
        max_bytes: u64,
    ) -> anyhow::Result<CaptureMeta> {
        let id = Uuid::new_v4().to_string();
        let full_path = self.full_path(&id);
        let thumb_path = self.thumb_path(&id);
        std::fs::write(&full_path, full_png)?;
        std::fs::write(&thumb_path, thumb_png)?;
        let size_bytes = (full_png.len() + thumb_png.len()) as i64;
        let captured_at = if meta.captured_at.is_empty() {
            Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        } else {
            meta.captured_at
        };

        let row = CaptureMeta {
            id,
            captured_at,
            url: meta.url,
            title: meta.title,
            page_title: meta.page_title,
            page_url: meta.page_url,
            profile: meta.profile,
            mode: meta.mode,
            image_width_px: meta.image_width_px,
            image_height_px: meta.image_height_px,
            dpr: meta.dpr,
            size_bytes,
            full_path: full_path.to_string_lossy().to_string(),
            thumb_path: thumb_path.to_string_lossy().to_string(),
            note: meta.note,
            banner_enabled: meta.banner_enabled,
            banner_baked: false,
        };
        {
            let conn = self.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO captures (id, captured_at, url, title, mode, image_width_px,
                    image_height_px, dpr, full_path, thumb_path, size_bytes,
                    note, banner_enabled, banner_baked, page_title, page_url, profile)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,0,?14,?15,?16)",
                rusqlite::params![
                    row.id,
                    row.captured_at,
                    row.url,
                    row.title,
                    row.mode,
                    row.image_width_px,
                    row.image_height_px,
                    row.dpr,
                    row.full_path,
                    row.thumb_path,
                    row.size_bytes,
                    row.note,
                    row.banner_enabled,
                    row.page_title,
                    row.page_url,
                    row.profile,
                ],
            )?;
        }
        self.enforce_retention(max_count, max_bytes)?;
        Ok(row)
    }

    /// Edit a stored capture: note / banner toggle, and optionally replace the content PNG
    /// (+ thumb) after a re-crop or redact. Legacy banner-baked rows are immutable.
    pub fn update(
        &self,
        id: &str,
        patch: CaptureUpdate,
        full_png: Option<&[u8]>,
        thumb_png: Option<&[u8]>,
    ) -> anyhow::Result<CaptureMeta> {
        let mut row = self
            .get(id)?
            .ok_or_else(|| anyhow::anyhow!("capture not found"))?;
        if row.banner_baked {
            anyhow::bail!("this capture predates editable history and is read-only");
        }
        if let Some(n) = patch.note {
            row.note = n;
        }
        if let Some(b) = patch.banner_enabled {
            row.banner_enabled = b;
        }
        if let Some(full) = full_png {
            std::fs::write(self.full_path(id), full)?;
            if let Some(thumb) = thumb_png {
                std::fs::write(self.thumb_path(id), thumb)?;
            }
            let thumb_len = std::fs::metadata(self.thumb_path(id))
                .map(|m| m.len())
                .unwrap_or(0);
            row.size_bytes = full.len() as i64 + thumb_len as i64;
            row.image_width_px = patch.image_width_px.unwrap_or(row.image_width_px);
            row.image_height_px = patch.image_height_px.unwrap_or(row.image_height_px);
        }
        self.conn.lock().unwrap().execute(
            "UPDATE captures SET note=?2, banner_enabled=?3, size_bytes=?4,
                image_width_px=?5, image_height_px=?6 WHERE id=?1",
            rusqlite::params![
                id,
                row.note,
                row.banner_enabled,
                row.size_bytes,
                row.image_width_px,
                row.image_height_px,
            ],
        )?;
        Ok(row)
    }

    pub fn list(&self) -> anyhow::Result<Vec<CaptureMeta>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM captures ORDER BY captured_at DESC"
        ))?;
        let rows = stmt.query_map([], row_to_meta)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn get(&self, id: &str) -> anyhow::Result<Option<CaptureMeta>> {
        let conn = self.conn.lock().unwrap();
        Ok(conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM captures WHERE id = ?1"),
                [id],
                row_to_meta,
            )
            .optional()?)
    }

    pub fn delete(&self, id: &str) -> anyhow::Result<()> {
        let _ = std::fs::remove_file(self.full_path(id));
        let _ = std::fs::remove_file(self.thumb_path(id));
        self.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM captures WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn clear(&self) -> anyhow::Result<()> {
        let ids: Vec<String> = {
            let conn = self.conn.lock().unwrap();
            let mut stmt = conn.prepare("SELECT id FROM captures")?;
            let ids = stmt.query_map([], |r| r.get::<_, String>(0))?;
            ids.collect::<Result<Vec<_>, _>>()?
        };
        for id in &ids {
            let _ = std::fs::remove_file(self.full_path(id));
            let _ = std::fs::remove_file(self.thumb_path(id));
        }
        self.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM captures", [])?;
        Ok(())
    }

    /// Oldest-first eviction until BOTH caps are satisfied (Checkpoint's `enforceRetention`).
    pub fn enforce_retention(&self, max_count: u32, max_bytes: u64) -> anyhow::Result<()> {
        // (id, size) oldest-first.
        let entries: Vec<(String, i64)> = {
            let conn = self.conn.lock().unwrap();
            let mut stmt =
                conn.prepare("SELECT id, size_bytes FROM captures ORDER BY captured_at ASC")?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let mut count = entries.len() as u64;
        let mut total: i64 = entries.iter().map(|(_, s)| *s).sum();
        for (id, size) in entries {
            if count <= u64::from(max_count) && (total as u64) <= max_bytes {
                break;
            }
            self.delete(&id)?;
            count -= 1;
            total -= size;
        }
        Ok(())
    }
}

fn row_to_meta(row: &rusqlite::Row) -> rusqlite::Result<CaptureMeta> {
    Ok(CaptureMeta {
        id: row.get(0)?,
        captured_at: row.get(1)?,
        url: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
        title: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
        mode: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
        image_width_px: row.get::<_, Option<i64>>(5)?.unwrap_or(0),
        image_height_px: row.get::<_, Option<i64>>(6)?.unwrap_or(0),
        dpr: row.get::<_, Option<f64>>(7)?.unwrap_or(1.0),
        size_bytes: row.get(8)?,
        full_path: row.get(9)?,
        thumb_path: row.get(10)?,
        note: row.get::<_, Option<String>>(11)?.unwrap_or_default(),
        banner_enabled: row.get::<_, Option<bool>>(12)?.unwrap_or(true),
        banner_baked: row.get::<_, Option<bool>>(13)?.unwrap_or(false),
        page_title: row.get::<_, Option<String>>(14)?.unwrap_or_default(),
        page_url: row.get::<_, Option<String>>(15)?.unwrap_or_default(),
        profile: row.get::<_, Option<String>>(16)?.unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::{HistoryStore, NewCapture};
    use crate::paths::AppPaths;

    fn paths(root: std::path::PathBuf) -> AppPaths {
        let history = root.join("history");
        let history_images = history.join("images");
        std::fs::create_dir_all(&history_images).unwrap();
        AppPaths {
            snippets_db: root.join("snippets.db"),
            engine_config: root.join("espanso"),
            history_db: history.join("history.db"),
            history_images,
            clipboard_db: root.join("clipboard.db"),
            clipboard_images: root.join("clipboard/images"),
            settings_json: root.join("settings.json"),
            root,
        }
    }

    fn capture(captured_at: &str) -> NewCapture {
        NewCapture {
            captured_at: captured_at.into(),
            url: "Safari".into(),
            title: "Glyphio".into(),
            page_title: String::new(),
            page_url: String::new(),
            profile: String::new(),
            mode: "visible".into(),
            image_width_px: 2,
            image_height_px: 1,
            dpr: 2.0,
            note: "note".into(),
            banner_enabled: true,
        }
    }

    #[test]
    fn reopens_the_artifact_with_its_original_capture_time() {
        let temp = tempfile::tempdir().unwrap();
        let store = HistoryStore::open(&paths(temp.path().to_path_buf())).unwrap();
        let captured_at = "2026-09-02T12:00:00.000Z";

        let saved = store.save(capture(captured_at), &[1], &[2], 50, 200).unwrap();
        let reopened = store.get(&saved.id).unwrap().unwrap();

        assert_eq!(reopened.captured_at, captured_at);
    }
}
