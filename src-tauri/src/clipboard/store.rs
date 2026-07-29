//! Clipboard history: SQLite metadata, PNG blobs on disk for images.
//!
//! Deliberately the same shape as `history::HistoryStore` — one row per entry, blobs beside
//! the database, retention enforced on write — because it is the same problem and a second
//! shape would be a second set of bugs.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{params, Connection, Row};
use serde::Serialize;

use crate::paths::AppPaths;

/// How much of a copied text is worth keeping in the list. Longer entries are stored in full
/// — you pasted them, you may want them back — but the preview is what the picker draws.
const PREVIEW_CHARS: usize = 400;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipEntry {
    pub id: String,
    pub copied_at: String,
    /// `text` or `image`.
    pub kind: String,
    /// First [`PREVIEW_CHARS`] of the text, or a short description for an image.
    pub preview: String,
    /// Full text, for `text` entries. Absent in list responses to keep them small.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Where the PNG lives, for `image` entries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_path: Option<String>,
    pub image_width_px: Option<u32>,
    pub image_height_px: Option<u32>,
    /// The app that had focus when this was copied. Best-effort; may be empty.
    pub source_app: String,
    pub size_bytes: i64,
    /// Pinned entries survive retention and sort to the top.
    pub pinned: bool,
}

pub struct ClipStore {
    conn: Mutex<Connection>,
    images_dir: PathBuf,
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS clips (
    id              TEXT PRIMARY KEY,
    copied_at       TEXT NOT NULL,
    kind            TEXT NOT NULL,
    preview         TEXT NOT NULL,
    body            TEXT,
    image_path      TEXT,
    image_width_px  INTEGER,
    image_height_px INTEGER,
    source_app      TEXT NOT NULL DEFAULT '',
    size_bytes      INTEGER NOT NULL DEFAULT 0,
    pinned          INTEGER NOT NULL DEFAULT 0,
    digest          TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_clips_copied_at ON clips(copied_at DESC);
CREATE INDEX IF NOT EXISTS idx_clips_digest ON clips(digest);
"#;

const COLUMNS: &str = "id, copied_at, kind, preview, image_path, image_width_px, \
                       image_height_px, source_app, size_bytes, pinned";

/// What the watcher hands over once it has decided an entry is worth keeping.
pub enum NewClip {
    Text(String),
    /// PNG bytes and the dimensions to show beside them.
    Image { png: Vec<u8>, width: u32, height: u32 },
}

impl ClipStore {
    pub fn open(paths: &AppPaths) -> anyhow::Result<Self> {
        let conn = Connection::open(&paths.clipboard_db)?;
        conn.execute_batch(SCHEMA)?;
        restrict(&paths.clipboard_db);
        Ok(Self {
            conn: Mutex::new(conn),
            images_dir: paths.clipboard_images.clone(),
        })
    }

    /// Record a clipboard entry, unless we already have it.
    ///
    /// Re-copying anything already in the history moves that entry back to the top rather than
    /// adding a second copy of it — matched on content across the whole history, not just
    /// against the newest row. People re-copy the same handful of things all day; without this
    /// the list becomes a wall of the same string and the search stops helping.
    ///
    /// Returns the entry when a new one was stored, `None` when an existing one was promoted.
    pub fn record(
        &self,
        clip: NewClip,
        source_app: &str,
        max_items: u32,
        max_bytes: u64,
    ) -> anyhow::Result<Option<ClipEntry>> {
        let id = uuid::Uuid::new_v4().to_string();
        let copied_at = chrono::Utc::now().to_rfc3339();
        let digest = digest_of(&clip);

        let (kind, preview, body, image_path, dims, size) = match clip {
            NewClip::Text(text) => {
                let preview = preview_of(&text);
                let size = text.len() as i64;
                ("text", preview, Some(text), None, (None, None), size)
            }
            NewClip::Image { png, width, height } => {
                let path = self.images_dir.join(format!("{id}.png"));
                std::fs::write(&path, &png)?;
                restrict(&path);
                (
                    "image",
                    format!("{width} × {height} image"),
                    None,
                    Some(crate::paths::s(&path)),
                    (Some(width), Some(height)),
                    png.len() as i64,
                )
            }
        };

        {
            let conn = self.conn.lock().unwrap();
            // Already have this exact content? Promote it instead of storing it twice.
            let existing: Option<String> = conn
                .query_row(
                    "SELECT id FROM clips WHERE digest = ?1 LIMIT 1",
                    params![digest],
                    |r| r.get(0),
                )
                .ok();
            if let Some(existing_id) = existing {
                conn.execute(
                    "UPDATE clips SET copied_at = ?1 WHERE id = ?2",
                    params![copied_at, existing_id],
                )?;
                if let Some(p) = image_path {
                    let _ = std::fs::remove_file(p); // the copy we just wrote is redundant
                }
                return Ok(None);
            }
            conn.execute(
                "INSERT INTO clips (id, copied_at, kind, preview, body, image_path, \
                 image_width_px, image_height_px, source_app, size_bytes, pinned, digest) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0, ?11)",
                params![
                    id,
                    copied_at,
                    kind,
                    preview,
                    body,
                    image_path,
                    dims.0,
                    dims.1,
                    source_app,
                    size,
                    digest
                ],
            )?;
        }
        self.enforce_retention(max_items, max_bytes)?;
        self.get(&id)
    }

    pub fn list(&self) -> anyhow::Result<Vec<ClipEntry>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM clips ORDER BY pinned DESC, copied_at DESC"
        ))?;
        let rows = stmt.query_map([], row_to_entry)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn get(&self, id: &str) -> anyhow::Result<Option<ClipEntry>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare(&format!("SELECT {COLUMNS}, body FROM clips WHERE id = ?1"))?;
        let mut rows = stmt.query_map(params![id], |r| {
            let mut e = row_to_entry(r)?;
            e.text = r.get::<_, Option<String>>(10)?;
            Ok(e)
        })?;
        Ok(rows.next().transpose()?)
    }

    /// Move an entry to the top of the history. Used when the user picks one from the picker:
    /// re-using something is a fresh act of copying it, and it belongs where it would be if
    /// they had copied it again by hand.
    pub fn touch(&self, id: &str) -> anyhow::Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE clips SET copied_at = ?1 WHERE id = ?2",
            params![chrono::Utc::now().to_rfc3339(), id],
        )?;
        Ok(())
    }

    pub fn set_pinned(&self, id: &str, pinned: bool) -> anyhow::Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE clips SET pinned = ?1 WHERE id = ?2",
            params![pinned as i32, id],
        )?;
        Ok(())
    }

    pub fn delete(&self, id: &str) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        if let Ok(Some(path)) = conn.query_row(
            "SELECT image_path FROM clips WHERE id = ?1",
            params![id],
            |r| r.get::<_, Option<String>>(0),
        ) {
            let _ = std::fs::remove_file(path);
        }
        conn.execute("DELETE FROM clips WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Forget everything. Pinned entries go too — "clear" that leaves things behind is the
    /// kind of surprise this feature can least afford.
    pub fn clear(&self) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT image_path FROM clips WHERE image_path IS NOT NULL")?;
        let paths: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .filter_map(Result::ok)
            .collect();
        drop(stmt);
        for p in paths {
            let _ = std::fs::remove_file(p);
        }
        conn.execute("DELETE FROM clips", [])?;
        Ok(())
    }

    /// Drop the oldest unpinned entries until the history is within both caps.
    pub fn enforce_retention(&self, max_items: u32, max_bytes: u64) -> anyhow::Result<()> {
        let doomed: Vec<(String, Option<String>)> = {
            let conn = self.conn.lock().unwrap();
            let mut stmt = conn.prepare(
                "SELECT id, image_path, size_bytes FROM clips WHERE pinned = 0 \
                 ORDER BY copied_at DESC",
            )?;
            let rows: Vec<(String, Option<String>, i64)> = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            let mut running = 0i64;
            rows.into_iter()
                .enumerate()
                .filter(|(i, (_, _, size))| {
                    running += size;
                    *i as u32 >= max_items || running as u64 > max_bytes
                })
                .map(|(_, (id, path, _))| (id, path))
                .collect()
        };
        for (id, path) in doomed {
            if let Some(p) = path {
                let _ = std::fs::remove_file(p);
            }
            self.conn
                .lock()
                .unwrap()
                .execute("DELETE FROM clips WHERE id = ?1", params![id])?;
        }
        Ok(())
    }
}

fn row_to_entry(r: &Row) -> rusqlite::Result<ClipEntry> {
    Ok(ClipEntry {
        id: r.get(0)?,
        copied_at: r.get(1)?,
        kind: r.get(2)?,
        preview: r.get(3)?,
        text: None,
        image_path: r.get(4)?,
        image_width_px: r.get(5)?,
        image_height_px: r.get(6)?,
        source_app: r.get(7)?,
        size_bytes: r.get(8)?,
        pinned: r.get::<_, i64>(9)? != 0,
    })
}

/// First line-ish of a copied text, with runs of whitespace flattened so a pasted block of
/// code or a wrapped paragraph still reads as one row in the list.
fn preview_of(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match flat.char_indices().nth(PREVIEW_CHARS) {
        Some((cut, _)) => format!("{}…", &flat[..cut]),
        None => flat,
    }
}

/// Content identity, for "is this the same thing I already have?". Hashed rather than stored
/// so the comparison never needs the body — and so an image compares by pixels, not by path.
fn digest_of(clip: &NewClip) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    match clip {
        NewClip::Text(t) => {
            "t".hash(&mut h);
            t.hash(&mut h);
        }
        NewClip::Image { png, .. } => {
            "i".hash(&mut h);
            png.hash(&mut h);
        }
    }
    format!("{:016x}", h.finish())
}

/// Owner-only, like the log and the engine socket. This file holds whatever the user has
/// copied today; the default umask is not a strong enough opinion about that.
fn restrict(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (ClipStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let images = dir.path().join("images");
        std::fs::create_dir_all(&images).unwrap();
        let conn = Connection::open(dir.path().join("c.db")).unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        (
            ClipStore { conn: Mutex::new(conn), images_dir: images },
            dir,
        )
    }

    #[test]
    fn copying_the_same_text_twice_does_not_add_a_second_row() {
        let (s, _d) = store();
        assert!(s.record(NewClip::Text("hello".into()), "", 100, 1 << 20).unwrap().is_some());
        assert!(s.record(NewClip::Text("hello".into()), "", 100, 1 << 20).unwrap().is_none());
        assert_eq!(s.list().unwrap().len(), 1);
    }

    #[test]
    fn re_copying_something_older_promotes_it_instead_of_duplicating_it() {
        let (s, _d) = store();
        s.record(NewClip::Text("first".into()), "", 100, 1 << 20).unwrap();
        s.record(NewClip::Text("second".into()), "", 100, 1 << 20).unwrap();
        // No new row — the one we already had comes back to the top.
        assert!(s.record(NewClip::Text("first".into()), "", 100, 1 << 20).unwrap().is_none());
        let list = s.list().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].preview, "first");
    }

    #[test]
    fn touching_an_entry_moves_it_to_the_top() {
        let (s, _d) = store();
        let first = s.record(NewClip::Text("older".into()), "", 100, 1 << 20).unwrap().unwrap();
        s.record(NewClip::Text("newer".into()), "", 100, 1 << 20).unwrap();
        assert_eq!(s.list().unwrap()[0].preview, "newer");
        s.touch(&first.id).unwrap();
        assert_eq!(s.list().unwrap()[0].preview, "older");
    }

    #[test]
    fn the_oldest_entries_fall_off_the_end() {
        let (s, _d) = store();
        for i in 0..5 {
            s.record(NewClip::Text(format!("entry {i}")), "", 3, 1 << 20).unwrap();
        }
        let list = s.list().unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].preview, "entry 4");
    }

    #[test]
    fn a_pinned_entry_outlives_the_cap_and_sorts_first() {
        let (s, _d) = store();
        let keep = s.record(NewClip::Text("keep me".into()), "", 2, 1 << 20).unwrap().unwrap();
        s.set_pinned(&keep.id, true).unwrap();
        for i in 0..6 {
            s.record(NewClip::Text(format!("noise {i}")), "", 2, 1 << 20).unwrap();
        }
        let list = s.list().unwrap();
        assert_eq!(list[0].preview, "keep me");
        assert!(list.iter().any(|e| e.pinned));
        // The cap applies to the unpinned remainder, not to the total.
        assert_eq!(list.iter().filter(|e| !e.pinned).count(), 2);
    }

    #[test]
    fn a_preview_is_one_flat_line() {
        assert_eq!(preview_of("  line one\n\n\tline two  "), "line one line two");
        let long = "x".repeat(PREVIEW_CHARS + 50);
        assert!(preview_of(&long).ends_with('…'));
        assert_eq!(preview_of(&long).chars().count(), PREVIEW_CHARS + 1);
    }

    #[test]
    fn an_image_is_stored_beside_the_database_and_deleted_with_its_row() {
        let (s, _d) = store();
        let png = vec![0u8, 1, 2, 3, 4];
        let e = s
            .record(NewClip::Image { png, width: 4, height: 2 }, "", 100, 1 << 20)
            .unwrap()
            .unwrap();
        let path = e.image_path.clone().unwrap();
        assert!(std::path::Path::new(&path).exists());
        assert_eq!(e.preview, "4 × 2 image");
        s.delete(&e.id).unwrap();
        assert!(!std::path::Path::new(&path).exists());
    }
}
