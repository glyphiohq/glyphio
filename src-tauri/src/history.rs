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
    pub captured_at: String,
    pub url: String,   // window/app title natively
    pub title: String,
    pub mode: String,  // "visible" | "snip" | "fullWindow"
    pub image_width_px: i64,
    pub image_height_px: i64,
    pub dpr: f64,
    pub size_bytes: i64,
    /// Absolute on-disk paths (frontend loads them via convertFileSrc).
    pub full_path: String,
    pub thumb_path: String,
}

/// What the frontend passes when persisting a capture (post-edit).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewCapture {
    pub url: String,
    pub title: String,
    pub mode: String,
    pub image_width_px: i64,
    pub image_height_px: i64,
    pub dpr: f64,
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
    size_bytes      INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_captures_captured_at ON captures(captured_at DESC);
"#;

impl HistoryStore {
    pub fn open(paths: &AppPaths) -> anyhow::Result<Self> {
        let conn = Connection::open(&paths.history_db)?;
        conn.execute_batch(SCHEMA)?;
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
        let captured_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

        let row = CaptureMeta {
            id,
            captured_at,
            url: meta.url,
            title: meta.title,
            mode: meta.mode,
            image_width_px: meta.image_width_px,
            image_height_px: meta.image_height_px,
            dpr: meta.dpr,
            size_bytes,
            full_path: full_path.to_string_lossy().to_string(),
            thumb_path: thumb_path.to_string_lossy().to_string(),
        };
        {
            let conn = self.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO captures (id, captured_at, url, title, mode, image_width_px,
                    image_height_px, dpr, full_path, thumb_path, size_bytes)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                rusqlite::params![
                    row.id, row.captured_at, row.url, row.title, row.mode,
                    row.image_width_px, row.image_height_px, row.dpr,
                    row.full_path, row.thumb_path, row.size_bytes,
                ],
            )?;
        }
        self.enforce_retention(max_count, max_bytes)?;
        Ok(row)
    }

    pub fn list(&self) -> anyhow::Result<Vec<CaptureMeta>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, captured_at, url, title, mode, image_width_px, image_height_px, dpr,
                    size_bytes, full_path, thumb_path
             FROM captures ORDER BY captured_at DESC",
        )?;
        let rows = stmt.query_map([], row_to_meta)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn get(&self, id: &str) -> anyhow::Result<Option<CaptureMeta>> {
        let conn = self.conn.lock().unwrap();
        Ok(conn
            .query_row(
                "SELECT id, captured_at, url, title, mode, image_width_px, image_height_px, dpr,
                        size_bytes, full_path, thumb_path
                 FROM captures WHERE id = ?1",
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
        self.conn.lock().unwrap().execute("DELETE FROM captures", [])?;
        Ok(())
    }

    /// Oldest-first eviction until BOTH caps are satisfied (Checkpoint's `enforceRetention`).
    pub fn enforce_retention(&self, max_count: u32, max_bytes: u64) -> anyhow::Result<()> {
        // (id, size) oldest-first.
        let entries: Vec<(String, i64)> = {
            let conn = self.conn.lock().unwrap();
            let mut stmt = conn
                .prepare("SELECT id, size_bytes FROM captures ORDER BY captured_at ASC")?;
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
    })
}
