//! Glyphio snippet store.
//!
//! The **source of truth** for text-expansion snippets is this local SQLite store — never the
//! expansion engine's config files. The engine's config directory (the engine is a GPL fork,
//! see NOTICES.md) is a generated, disposable artifact: [`SnippetStore::render_yaml`] rewrites
//! it from the SQLite rows and the engine's own file-watcher hot-reloads it.
//!
//! Designed for scale from the ground up:
//! * A **migration runner** (`PRAGMA user_version`) so the schema can evolve without data loss.
//! * First-class **groups** (folders) with their own sync columns.
//! * A **`format`** column (`plain` | `markdown` | `html`) — the engine natively injects all three
//!   (markdown/html paste as rich text via the clipboard), so rich snippets are just a different
//!   generated YAML key, not a bespoke mechanism.
//! * `owner`/`team`/`version`/`updated_at`/`deleted_at` on both snippets and groups so Phase 2
//!   team sync is additive (another consumer of the same store + [`ChangeEvent`] stream), not a
//!   migration that risks existing data.

use std::path::Path;
use std::sync::Mutex;

use chrono::Utc;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

mod portability;
mod yaml;

pub use portability::{ExportDoc, ImportReport};
pub use yaml::render_matches_yaml;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("yaml error: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not found: {0}")]
    NotFound(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// A single text-expansion snippet. Mirrors the SQLite `snippets` table.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Snippet {
    pub id: String,
    pub trigger: String,
    /// The body, interpreted per `format`: plain text, Markdown source, or HTML.
    pub replacement: String,
    /// `plain` | `markdown` | `html`. Drives which match-file body key is generated.
    pub format: String,
    /// What the trigger does:
    /// * `text` — classic expansion (the body is pasted).
    /// * `form` — a Tauri form window collects input, the filled body template is pasted.
    /// * `popup` — a Tauri popup window shows the body (cheatsheet); nothing is pasted.
    /// * `command` — runs a shell command, pastes its output. **Never syncs** (local-only,
    ///   `team` is forced NULL) — a synced command would be remote code execution.
    #[serde(default = "default_kind")]
    pub kind: String,
    /// Disabled snippets stay in the store/UI but are not rendered into the engine config.
    /// Used for quarantine: imported or pulled records with executable variables arrive
    /// disabled until the user reviews and enables them.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// engine `vars` array, stored as JSON. `None` = no variables.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variables: Option<serde_json::Value>,
    /// Owning group (folder). `None` = ungrouped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    /// `None`/`"all"` = active everywhere; otherwise an app scope (not yet enforced in YAML).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_scope: Option<String>,
    pub owner: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,
    pub updated_at: String,
    pub version: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSnippet {
    pub trigger: String,
    pub replacement: String,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    pub variables: Option<serde_json::Value>,
    pub group_id: Option<String>,
    pub app_scope: Option<String>,
    pub owner: Option<String>,
    pub team: Option<String>,
}

/// Full-record edit (a Save from the UI form): every editable field is provided.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnippetUpdate {
    pub trigger: String,
    pub replacement: String,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    pub variables: Option<serde_json::Value>,
    pub group_id: Option<String>,
    pub app_scope: Option<String>,
    pub team: Option<String>,
}

/// A group (folder) snippets can belong to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    pub id: String,
    pub name: String,
    pub sort_order: i64,
    /// `Some(team)` = this group syncs with that team; `None` = local-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,
    pub updated_at: String,
    pub version: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewGroup {
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupUpdate {
    pub name: String,
    pub sort_order: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeKind {
    Created,
    Updated,
    Deleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeEntity {
    Snippet,
    Group,
}

/// Where a mutation came from. The sync engine pushes `Local` changes and ignores `Remote` ones
/// (they were just applied *from* the server — re-pushing them would loop).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeOrigin {
    Local,
    Remote,
}

/// Emitted on every mutation. The Tauri app subscribes to regenerate YAML + refresh the UI;
/// the sync client subscribes to push changes upstream. Both are just listeners.
#[derive(Debug, Clone, Serialize)]
pub struct ChangeEvent {
    pub kind: ChangeKind,
    pub entity: ChangeEntity,
    pub origin: ChangeOrigin,
    pub id: String,
}

type Listener = Box<dyn Fn(&ChangeEvent) + Send + Sync + 'static>;

pub struct SnippetStore {
    conn: Mutex<Connection>,
    listeners: Mutex<Vec<Listener>>,
}

/// Ordered schema migrations. Index + 1 == the `user_version` they bring the DB to.
const MIGRATIONS: &[&str] = &[
    // v1 — snippets.
    r#"
    CREATE TABLE IF NOT EXISTS snippets (
        id          TEXT PRIMARY KEY,
        trigger     TEXT NOT NULL,
        replacement TEXT NOT NULL,
        variables   TEXT,
        app_scope   TEXT,
        owner       TEXT NOT NULL DEFAULT 'personal',
        team        TEXT,
        updated_at  TEXT NOT NULL,
        version     INTEGER NOT NULL DEFAULT 1,
        deleted_at  TEXT
    );
    CREATE INDEX IF NOT EXISTS idx_snippets_updated_at ON snippets(updated_at);
    CREATE INDEX IF NOT EXISTS idx_snippets_live ON snippets(deleted_at) WHERE deleted_at IS NULL;
    "#,
    // v2 — rich formats + groups.
    r#"
    ALTER TABLE snippets ADD COLUMN format TEXT NOT NULL DEFAULT 'plain';
    ALTER TABLE snippets ADD COLUMN group_id TEXT;
    CREATE TABLE IF NOT EXISTS groups (
        id         TEXT PRIMARY KEY,
        name       TEXT NOT NULL,
        sort_order INTEGER NOT NULL DEFAULT 0,
        updated_at TEXT NOT NULL,
        version    INTEGER NOT NULL DEFAULT 1,
        deleted_at TEXT
    );
    CREATE INDEX IF NOT EXISTS idx_groups_live ON groups(deleted_at) WHERE deleted_at IS NULL;
    CREATE INDEX IF NOT EXISTS idx_snippets_group ON snippets(group_id);
    "#,
    // v3 — sync bookkeeping (additive): team-scoped groups, per-team pull cursors, and
    // per-record pushed-version tracking (dirty = version > pushed version).
    r#"
    ALTER TABLE groups ADD COLUMN team TEXT;
    CREATE TABLE IF NOT EXISTS sync_cursors (
        team   TEXT PRIMARY KEY,
        cursor INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE IF NOT EXISTS sync_pushed (
        kind    TEXT NOT NULL,
        id      TEXT NOT NULL,
        version INTEGER NOT NULL,
        PRIMARY KEY (kind, id)
    );
    CREATE INDEX IF NOT EXISTS idx_snippets_team ON snippets(team) WHERE team IS NOT NULL;
    "#,
    // v4 — interactive snippet kinds (text | form | popup | command) + enable/quarantine flag.
    r#"
    ALTER TABLE snippets ADD COLUMN kind TEXT NOT NULL DEFAULT 'text';
    ALTER TABLE snippets ADD COLUMN enabled INTEGER NOT NULL DEFAULT 1;
    "#,
];

impl SnippetStore {
    pub fn open(db_path: impl AsRef<Path>) -> Result<Self> {
        if let Some(parent) = db_path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        Self::init(Connection::open(db_path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        Self::migrate(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            listeners: Mutex::new(Vec::new()),
        })
    }

    /// Apply any pending migrations, tracked via `PRAGMA user_version`.
    ///
    /// Robust against **mixed binary versions on one machine** (e.g. a packaged build and a
    /// dev build sharing the data dir): an older binary must never *downgrade* the stamp of a
    /// newer schema, and a re-run `ALTER TABLE … ADD COLUMN` that finds its column already
    /// present is treated as already-applied rather than fatal (the stamp then self-heals).
    fn migrate(conn: &Connection) -> Result<()> {
        let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        let latest = MIGRATIONS.len() as i64;
        for (i, sql) in MIGRATIONS.iter().enumerate() {
            let target = (i + 1) as i64;
            if current < target {
                match conn.execute_batch(sql) {
                    Ok(()) => {}
                    Err(e) if e.to_string().contains("duplicate column name") => {
                        // Schema is already at (at least) this migration; only the stamp was
                        // stale — see the mixed-version note above.
                    }
                    Err(e) => return Err(e.into()),
                }
            }
        }
        if current < latest {
            conn.execute_batch(&format!("PRAGMA user_version = {latest}"))?;
        }
        Ok(())
    }

    pub fn add_change_listener<F>(&self, f: F)
    where
        F: Fn(&ChangeEvent) + Send + Sync + 'static,
    {
        self.listeners.lock().unwrap().push(Box::new(f));
    }

    fn notify(&self, kind: ChangeKind, entity: ChangeEntity, origin: ChangeOrigin, id: &str) {
        let event = ChangeEvent { kind, entity, origin, id: id.to_string() };
        for l in self.listeners.lock().unwrap().iter() {
            l(&event);
        }
    }

    // ---- snippets ---------------------------------------------------------

    pub fn list(&self) -> Result<Vec<Snippet>> {
        self.query("WHERE deleted_at IS NULL ORDER BY updated_at DESC", [])
    }

    pub fn list_all(&self) -> Result<Vec<Snippet>> {
        self.query("ORDER BY updated_at DESC", [])
    }

    pub fn get(&self, id: &str) -> Result<Option<Snippet>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            &format!("SELECT {COLUMNS} FROM snippets WHERE id = ?1"),
            [id],
            row_to_snippet,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn create(&self, new: NewSnippet) -> Result<Snippet> {
        let kind = normalize_kind(new.kind);
        // A snippet created inside a team-shared group inherits that team — otherwise it
        // silently neither syncs nor appears under the team view. Command snippets NEVER
        // carry a team (they must not sync), even inside a team-shared group.
        let team = if kind == "command" {
            None
        } else {
            match (&new.team, &new.group_id) {
                (Some(t), _) => Some(t.clone()),
                (None, Some(g)) => self.get_group(g)?.and_then(|grp| grp.team),
                (None, None) => None,
            }
        };
        let snippet = Snippet {
            id: Uuid::new_v4().to_string(),
            trigger: new.trigger,
            replacement: new.replacement,
            format: normalize_format(new.format),
            kind,
            enabled: new.enabled.unwrap_or(true),
            variables: new.variables,
            group_id: new.group_id,
            app_scope: new.app_scope,
            owner: new.owner.unwrap_or_else(|| "personal".to_string()),
            team,
            updated_at: now(),
            version: 1,
            deleted_at: None,
        };
        {
            let conn = self.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO snippets (id, trigger, replacement, variables, app_scope, owner, team,
                    updated_at, version, deleted_at, format, group_id, kind, enabled)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
                rusqlite::params![
                    snippet.id, snippet.trigger, snippet.replacement,
                    vars_to_text(&snippet.variables)?, snippet.app_scope, snippet.owner,
                    snippet.team, snippet.updated_at, snippet.version, snippet.deleted_at,
                    snippet.format, snippet.group_id, snippet.kind, snippet.enabled,
                ],
            )?;
        }
        self.notify(ChangeKind::Created, ChangeEntity::Snippet, ChangeOrigin::Local, &snippet.id);
        Ok(snippet)
    }

    pub fn update(&self, id: &str, patch: SnippetUpdate) -> Result<Snippet> {
        let mut snippet = self.get(id)?.ok_or_else(|| StoreError::NotFound(id.to_string()))?;
        snippet.trigger = patch.trigger;
        snippet.replacement = patch.replacement;
        snippet.format = normalize_format(patch.format);
        snippet.kind = patch.kind.map(|k| normalize_kind(Some(k))).unwrap_or(snippet.kind);
        snippet.enabled = patch.enabled.unwrap_or(snippet.enabled);
        snippet.variables = patch.variables;
        snippet.group_id = patch.group_id;
        snippet.app_scope = patch.app_scope;
        // Command snippets never carry a team — they must not sync.
        snippet.team = if snippet.kind == "command" {
            None
        } else {
            match (&patch.team, &snippet.group_id) {
                (Some(t), _) => Some(t.clone()),
                (None, Some(g)) => self.get_group(g)?.and_then(|grp| grp.team),
                (None, None) => None,
            }
        };
        snippet.updated_at = now();
        snippet.version += 1;
        {
            let conn = self.conn.lock().unwrap();
            conn.execute(
                "UPDATE snippets SET trigger=?2, replacement=?3, variables=?4, app_scope=?5,
                   team=?6, updated_at=?7, version=?8, format=?9, group_id=?10, kind=?11,
                   enabled=?12 WHERE id=?1",
                rusqlite::params![
                    snippet.id, snippet.trigger, snippet.replacement,
                    vars_to_text(&snippet.variables)?, snippet.app_scope, snippet.team,
                    snippet.updated_at, snippet.version, snippet.format, snippet.group_id,
                    snippet.kind, snippet.enabled,
                ],
            )?;
        }
        self.notify(ChangeKind::Updated, ChangeEntity::Snippet, ChangeOrigin::Local, id);
        Ok(snippet)
    }

    pub fn soft_delete(&self, id: &str) -> Result<()> {
        let affected = {
            let conn = self.conn.lock().unwrap();
            conn.execute(
                "UPDATE snippets SET deleted_at=?2, updated_at=?2, version=version+1
                 WHERE id=?1 AND deleted_at IS NULL",
                rusqlite::params![id, now()],
            )?
        };
        if affected == 0 {
            return Err(StoreError::NotFound(id.to_string()));
        }
        self.notify(ChangeKind::Deleted, ChangeEntity::Snippet, ChangeOrigin::Local, id);
        Ok(())
    }

    // ---- groups -----------------------------------------------------------

    pub fn list_groups(&self) -> Result<Vec<Group>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {GROUP_COLUMNS} FROM groups WHERE deleted_at IS NULL ORDER BY sort_order, name"
        ))?;
        let rows = stmt.query_map([], row_to_group)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn get_group(&self, id: &str) -> Result<Option<Group>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            &format!("SELECT {GROUP_COLUMNS} FROM groups WHERE id = ?1"),
            [id],
            row_to_group,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn create_group(&self, new: NewGroup) -> Result<Group> {
        let group = {
            let conn = self.conn.lock().unwrap();
            let next_order: i64 = conn
                .query_row("SELECT COALESCE(MAX(sort_order), -1) + 1 FROM groups", [], |r| r.get(0))
                .unwrap_or(0);
            let group = Group {
                id: Uuid::new_v4().to_string(),
                name: new.name,
                sort_order: next_order,
                team: None,
                updated_at: now(),
                version: 1,
                deleted_at: None,
            };
            conn.execute(
                "INSERT INTO groups (id, name, sort_order, team, updated_at, version, deleted_at)
                 VALUES (?1,?2,?3,?4,?5,?6,NULL)",
                rusqlite::params![
                    group.id, group.name, group.sort_order, group.team, group.updated_at,
                    group.version
                ],
            )?;
            group
        };
        self.notify(ChangeKind::Created, ChangeEntity::Group, ChangeOrigin::Local, &group.id);
        Ok(group)
    }

    pub fn update_group(&self, id: &str, patch: GroupUpdate) -> Result<()> {
        let affected = {
            let conn = self.conn.lock().unwrap();
            conn.execute(
                "UPDATE groups SET name=?2, sort_order=COALESCE(?3, sort_order),
                   updated_at=?4, version=version+1 WHERE id=?1 AND deleted_at IS NULL",
                rusqlite::params![id, patch.name, patch.sort_order, now()],
            )?
        };
        if affected == 0 {
            return Err(StoreError::NotFound(id.to_string()));
        }
        self.notify(ChangeKind::Updated, ChangeEntity::Group, ChangeOrigin::Local, id);
        Ok(())
    }

    /// Set (or clear) the team a group syncs with. Its member snippets follow the group's scope.
    pub fn set_group_team(&self, id: &str, team: Option<&str>) -> Result<()> {
        let (affected, member_ids) = {
            let conn = self.conn.lock().unwrap();
            let affected = conn.execute(
                "UPDATE groups SET team=?2, updated_at=?3, version=version+1
                 WHERE id=?1 AND deleted_at IS NULL",
                rusqlite::params![id, team, now()],
            )?;
            // Command snippets never follow a group into a team — they must not sync.
            conn.execute(
                "UPDATE snippets SET team=?2, updated_at=?3, version=version+1
                 WHERE group_id=?1 AND deleted_at IS NULL AND kind != 'command'",
                rusqlite::params![id, team, now()],
            )?;
            let mut stmt = conn.prepare(
                "SELECT id FROM snippets
                 WHERE group_id=?1 AND deleted_at IS NULL AND kind != 'command'",
            )?;
            let ids = stmt
                .query_map([id], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            (affected, ids)
        };
        if affected == 0 {
            return Err(StoreError::NotFound(id.to_string()));
        }
        self.notify(ChangeKind::Updated, ChangeEntity::Group, ChangeOrigin::Local, id);
        for sid in member_ids {
            self.notify(ChangeKind::Updated, ChangeEntity::Snippet, ChangeOrigin::Local, &sid);
        }
        Ok(())
    }

    /// Soft-delete a group; its snippets become ungrouped (never deleted with the group).
    pub fn soft_delete_group(&self, id: &str) -> Result<()> {
        let affected = {
            let conn = self.conn.lock().unwrap();
            conn.execute("UPDATE snippets SET group_id=NULL WHERE group_id=?1", [id])?;
            conn.execute(
                "UPDATE groups SET deleted_at=?2, updated_at=?2, version=version+1
                 WHERE id=?1 AND deleted_at IS NULL",
                rusqlite::params![id, now()],
            )?
        };
        if affected == 0 {
            return Err(StoreError::NotFound(id.to_string()));
        }
        self.notify(ChangeKind::Deleted, ChangeEntity::Group, ChangeOrigin::Local, id);
        Ok(())
    }

    // ---- sync bookkeeping ---------------------------------------------------
    // Consumed by the sync engine (`sync-client` crate). "Dirty" = the record's `version` is
    // newer than the last version acknowledged by the server (`sync_pushed`). Tombstones are
    // included — deletes must reach the server too.

    /// Team-scoped snippets not yet acknowledged by the server (includes tombstones).
    pub fn dirty_snippets(&self, team: &str) -> Result<Vec<Snippet>> {
        let cols = prefixed_columns("s", COLUMNS);
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {cols} FROM snippets s
             LEFT JOIN sync_pushed p ON p.kind='snippet' AND p.id = s.id
             WHERE s.team = ?1 AND s.version > COALESCE(p.version, 0)
             ORDER BY s.updated_at"
        ))?;
        let rows = stmt.query_map([team], row_to_snippet)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Team-scoped groups not yet acknowledged by the server (includes tombstones).
    pub fn dirty_groups(&self, team: &str) -> Result<Vec<Group>> {
        let cols = prefixed_columns("g", GROUP_COLUMNS);
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {cols} FROM groups g
             LEFT JOIN sync_pushed p ON p.kind='group' AND p.id = g.id
             WHERE g.team = ?1 AND g.version > COALESCE(p.version, 0)
             ORDER BY g.updated_at"
        ))?;
        let rows = stmt.query_map([team], row_to_group)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Record that the server acknowledged this record at `version` (it is no longer dirty,
    /// unless edited again since — a later local edit bumps `version` past this).
    pub fn mark_pushed(&self, kind: ChangeEntity, id: &str, version: i64) -> Result<()> {
        let kind = match kind {
            ChangeEntity::Snippet => "snippet",
            ChangeEntity::Group => "group",
        };
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO sync_pushed (kind, id, version) VALUES (?1,?2,?3)
             ON CONFLICT(kind, id) DO UPDATE SET version = excluded.version",
            rusqlite::params![kind, id, version],
        )?;
        Ok(())
    }

    /// The per-team pull cursor (0 = never pulled).
    pub fn sync_cursor(&self, team: &str) -> Result<u64> {
        let conn = self.conn.lock().unwrap();
        let v: Option<i64> = conn
            .query_row("SELECT cursor FROM sync_cursors WHERE team=?1", [team], |r| r.get(0))
            .optional()?;
        Ok(v.unwrap_or(0) as u64)
    }

    pub fn set_sync_cursor(&self, team: &str, cursor: u64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO sync_cursors (team, cursor) VALUES (?1,?2)
             ON CONFLICT(team) DO UPDATE SET cursor = excluded.cursor",
            rusqlite::params![team, cursor as i64],
        )?;
        Ok(())
    }

    /// Apply a server-side snippet record. Last-write-wins on `(updated_at, version)`: the
    /// remote record is written verbatim only if strictly newer than the local row (a local
    /// winner stays put — it is still dirty and will push). Applying also acknowledges the
    /// record as pushed (client and server now agree). Returns whether it was applied.
    pub fn apply_remote_snippet(&self, s: &Snippet) -> Result<bool> {
        let (applied, kind) = {
            let conn = self.conn.lock().unwrap();
            let existing: Option<(String, i64)> = conn
                .query_row(
                    "SELECT updated_at, version FROM snippets WHERE id=?1",
                    [&s.id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let wins = match &existing {
                Some((u, v)) => (s.updated_at.as_str(), s.version) > (u.as_str(), *v),
                None => true,
            };
            if !wins {
                return Ok(false);
            }
            conn.execute(
                "INSERT INTO snippets (id, trigger, replacement, variables, app_scope, owner,
                    team, updated_at, version, deleted_at, format, group_id, kind, enabled)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)
                 ON CONFLICT(id) DO UPDATE SET trigger=?2, replacement=?3, variables=?4,
                    app_scope=?5, owner=?6, team=?7, updated_at=?8, version=?9, deleted_at=?10,
                    format=?11, group_id=?12, kind=?13, enabled=?14",
                rusqlite::params![
                    s.id, s.trigger, s.replacement, vars_to_text(&s.variables)?, s.app_scope,
                    s.owner, s.team, s.updated_at, s.version, s.deleted_at, s.format, s.group_id,
                    s.kind, s.enabled,
                ],
            )?;
            conn.execute(
                "INSERT INTO sync_pushed (kind, id, version) VALUES ('snippet',?1,?2)
                 ON CONFLICT(kind, id) DO UPDATE SET version = excluded.version",
                rusqlite::params![s.id, s.version],
            )?;
            let kind = if s.deleted_at.is_some() {
                ChangeKind::Deleted
            } else if existing.is_some() {
                ChangeKind::Updated
            } else {
                ChangeKind::Created
            };
            (true, kind)
        };
        if applied {
            self.notify(kind, ChangeEntity::Snippet, ChangeOrigin::Remote, &s.id);
        }
        Ok(applied)
    }

    /// Apply a server-side group record — same LWW + acknowledge semantics as snippets.
    pub fn apply_remote_group(&self, g: &Group) -> Result<bool> {
        let (applied, kind) = {
            let conn = self.conn.lock().unwrap();
            let existing: Option<(String, i64)> = conn
                .query_row(
                    "SELECT updated_at, version FROM groups WHERE id=?1",
                    [&g.id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let wins = match &existing {
                Some((u, v)) => (g.updated_at.as_str(), g.version) > (u.as_str(), *v),
                None => true,
            };
            if !wins {
                return Ok(false);
            }
            conn.execute(
                "INSERT INTO groups (id, name, sort_order, team, updated_at, version, deleted_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)
                 ON CONFLICT(id) DO UPDATE SET name=?2, sort_order=?3, team=?4, updated_at=?5,
                    version=?6, deleted_at=?7",
                rusqlite::params![
                    g.id, g.name, g.sort_order, g.team, g.updated_at, g.version, g.deleted_at
                ],
            )?;
            if g.deleted_at.is_some() {
                conn.execute("UPDATE snippets SET group_id=NULL WHERE group_id=?1", [&g.id])?;
            }
            conn.execute(
                "INSERT INTO sync_pushed (kind, id, version) VALUES ('group',?1,?2)
                 ON CONFLICT(kind, id) DO UPDATE SET version = excluded.version",
                rusqlite::params![g.id, g.version],
            )?;
            let kind = if g.deleted_at.is_some() {
                ChangeKind::Deleted
            } else if existing.is_some() {
                ChangeKind::Updated
            } else {
                ChangeKind::Created
            };
            (true, kind)
        };
        if applied {
            self.notify(kind, ChangeEntity::Group, ChangeOrigin::Remote, &g.id);
        }
        Ok(applied)
    }

    // ---- YAML generation --------------------------------------------------

    /// Regenerate the engine's config directory from the current live snippets. See [`yaml`].
    ///
    /// Unscoped snippets go to `match/glyphio.yml` (picked up by the engine's default include glob).
    /// App-scoped snippets go to underscore-prefixed `match/_scoped_<n>.yml` files — which the
    /// default glob (`[!_]*.yml`) skips — each activated by a generated `config/app_<n>.yml`
    /// carrying the scope's `filter_exec`/`filter_title` plus an `extra_includes` for its file.
    pub fn render_yaml(&self, config_dir: impl AsRef<Path>) -> Result<()> {
        let config_dir = config_dir.as_ref();
        let match_dir = config_dir.join("match");
        let cfg_dir = config_dir.join("config");
        std::fs::create_dir_all(&match_dir)?;
        std::fs::create_dir_all(&cfg_dir)?;

        let default_cfg = cfg_dir.join("default.yml");
        if !default_cfg.exists() {
            atomic_write(&default_cfg, DEFAULT_ENGINE_CONFIG.as_bytes())?;
        }

        // Partition live snippets by scope. `None`/`"all"`/blank = active everywhere.
        let all = self.list()?;
        let (unscoped, scoped): (Vec<_>, Vec<_>) = all.into_iter().partition(|s| {
            matches!(s.app_scope.as_deref(), None | Some("") | Some("all"))
        });

        // Stable ordering: distinct scopes sorted, so generated filenames are deterministic.
        let mut scopes: Vec<String> =
            scoped.iter().filter_map(|s| s.app_scope.clone()).collect();
        scopes.sort();
        scopes.dedup();

        // Remove previously generated scoped artifacts (the whole dir is a generated artifact,
        // so anything matching our naming is ours to clean).
        remove_generated(&match_dir, "_scoped_")?;
        remove_generated(&cfg_dir, "app_")?;

        atomic_write(
            &match_dir.join("glyphio.yml"),
            render_matches_yaml(&unscoped)?.as_bytes(),
        )?;
        for (i, scope) in scopes.iter().enumerate() {
            let matches: Vec<Snippet> = scoped
                .iter()
                .filter(|s| s.app_scope.as_deref() == Some(scope.as_str()))
                .cloned()
                .collect();
            let match_file = format!("_scoped_{i}.yml");
            atomic_write(
                &match_dir.join(&match_file),
                render_matches_yaml(&matches)?.as_bytes(),
            )?;
            atomic_write(
                &cfg_dir.join(format!("app_{i}.yml")),
                yaml::render_app_config_yaml(scope, &match_file)?.as_bytes(),
            )?;
        }
        Ok(())
    }

    // ---- helpers ----------------------------------------------------------

    fn query<P: rusqlite::Params>(&self, tail: &str, params: P) -> Result<Vec<Snippet>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!("SELECT {COLUMNS} FROM snippets {tail}"))?;
        let rows = stmt.query_map(params, row_to_snippet)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }
}

const COLUMNS: &str = "id, trigger, replacement, variables, app_scope, owner, team, \
                       updated_at, version, deleted_at, format, group_id, kind, enabled";

const GROUP_COLUMNS: &str = "id, name, sort_order, team, updated_at, version, deleted_at";

/// `"id, name"` + `"t"` → `"t.id, t.name"` (disambiguates columns in JOINed queries).
fn prefixed_columns(alias: &str, columns: &str) -> String {
    columns
        .split(", ")
        .map(|c| format!("{alias}.{}", c.trim()))
        .collect::<Vec<_>>()
        .join(", ")
}

const DEFAULT_ENGINE_CONFIG: &str = "# Generated by Glyphio. The engine runs headless as a sidecar;\n\
# the Glyphio app provides the user-facing tray, notifications, and About dialog.\n\
show_icon: false\n\
show_notifications: false\n";

fn normalize_format(f: Option<String>) -> String {
    match f.as_deref() {
        Some("markdown") => "markdown".to_string(),
        Some("html") => "html".to_string(),
        _ => "plain".to_string(),
    }
}

/// Whether an engine `vars` JSON array contains an executable variable (`shell` / `script`).
/// Used everywhere content crosses a trust boundary: YAML import, sync push exclusion, sync
/// pull quarantine, and the server's push validation mirrors it.
pub fn has_exec_vars(vars: &Option<serde_json::Value>) -> bool {
    let Some(serde_json::Value::Array(items)) = vars else {
        return false;
    };
    items.iter().any(|v| {
        matches!(
            v.get("type").and_then(|t| t.as_str()),
            Some("shell") | Some("script")
        )
    })
}

fn normalize_kind(k: Option<String>) -> String {
    match k.as_deref() {
        Some("form") => "form".to_string(),
        Some("popup") => "popup".to_string(),
        Some("command") => "command".to_string(),
        _ => "text".to_string(),
    }
}

fn default_kind() -> String {
    "text".to_string()
}

fn default_enabled() -> bool {
    true
}

fn row_to_snippet(row: &rusqlite::Row) -> rusqlite::Result<Snippet> {
    let variables_text: Option<String> = row.get(3)?;
    let variables = variables_text.and_then(|t| serde_json::from_str(&t).ok());
    Ok(Snippet {
        id: row.get(0)?,
        trigger: row.get(1)?,
        replacement: row.get(2)?,
        variables,
        app_scope: row.get(4)?,
        owner: row.get(5)?,
        team: row.get(6)?,
        updated_at: row.get(7)?,
        version: row.get(8)?,
        deleted_at: row.get(9)?,
        format: row.get(10)?,
        group_id: row.get(11)?,
        kind: row.get(12)?,
        enabled: row.get(13)?,
    })
}

fn row_to_group(row: &rusqlite::Row) -> rusqlite::Result<Group> {
    Ok(Group {
        id: row.get(0)?,
        name: row.get(1)?,
        sort_order: row.get(2)?,
        team: row.get(3)?,
        updated_at: row.get(4)?,
        version: row.get(5)?,
        deleted_at: row.get(6)?,
    })
}

fn vars_to_text(v: &Option<serde_json::Value>) -> Result<Option<String>> {
    match v {
        Some(val) => Ok(Some(serde_json::to_string(val)?)),
        None => Ok(None),
    }
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Delete previously generated files in `dir` whose name starts with `prefix` and ends `.yml`.
fn remove_generated(dir: &Path, prefix: &str) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with(prefix) && name.ends_with(".yml") {
            std::fs::remove_file(&path)?;
        }
    }
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("yml.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_bring_fresh_db_to_latest() {
        let store = SnippetStore::open_in_memory().unwrap();
        let v: i64 = store
            .conn
            .lock()
            .unwrap()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, MIGRATIONS.len() as i64);
    }

    #[test]
    fn snippet_roundtrip_with_format_and_group() {
        let store = SnippetStore::open_in_memory().unwrap();
        let g = store.create_group(NewGroup { name: "Support".into() }).unwrap();
        let s = store
            .create(NewSnippet {
                trigger: ":tbl".into(),
                replacement: "<b>hi</b>".into(),
                format: Some("html".into()),
                group_id: Some(g.id.clone()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(s.format, "html");
        assert_eq!(s.group_id.as_deref(), Some(g.id.as_str()));
        assert_eq!(store.list().unwrap().len(), 1);
        assert_eq!(store.list_groups().unwrap().len(), 1);

        // Deleting the group orphans (does not delete) its snippet.
        store.soft_delete_group(&g.id).unwrap();
        assert_eq!(store.list_groups().unwrap().len(), 0);
        assert_eq!(store.list().unwrap().len(), 1);
        assert!(store.get(&s.id).unwrap().unwrap().group_id.is_none());
    }

    #[test]
    fn render_yaml_partitions_scoped_snippets_and_cleans_stale_files() {
        let store = SnippetStore::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        store
            .create(NewSnippet { trigger: ":a".into(), replacement: "x".into(), ..Default::default() })
            .unwrap();
        let scoped = store
            .create(NewSnippet {
                trigger: ":s".into(),
                replacement: "y".into(),
                app_scope: Some("Slack".into()),
                ..Default::default()
            })
            .unwrap();
        store.render_yaml(dir.path()).unwrap();

        let main = std::fs::read_to_string(dir.path().join("match/glyphio.yml")).unwrap();
        assert!(main.contains(":a") && !main.contains(":s"));
        let scoped_yml = std::fs::read_to_string(dir.path().join("match/_scoped_0.yml")).unwrap();
        assert!(scoped_yml.contains(":s"));
        let app_cfg = std::fs::read_to_string(dir.path().join("config/app_0.yml")).unwrap();
        assert!(app_cfg.contains("filter_exec"));
        assert!(app_cfg.contains("(?i)Slack"));
        assert!(app_cfg.contains("../match/_scoped_0.yml"));

        // Un-scoping the snippet removes the now-stale generated files on the next render.
        store
            .update(&scoped.id, SnippetUpdate {
                trigger: ":s".into(),
                replacement: "y".into(),
                ..Default::default()
            })
            .unwrap();
        store.render_yaml(dir.path()).unwrap();
        assert!(!dir.path().join("match/_scoped_0.yml").exists());
        assert!(!dir.path().join("config/app_0.yml").exists());
        assert!(std::fs::read_to_string(dir.path().join("match/glyphio.yml")).unwrap().contains(":s"));
    }

    #[test]
    fn migrate_survives_mixed_binary_versions() {
        // Simulate: new binary migrates to v3, then an OLD binary (fewer migrations) opens the
        // DB and stamps its lower version, then the new binary opens it again. The re-run v3
        // ALTER must be tolerated and the stamp must self-heal (and never downgrade).
        let store = SnippetStore::open_in_memory().unwrap();
        {
            let conn = store.conn.lock().unwrap();
            conn.execute_batch("PRAGMA user_version = 2").unwrap(); // old binary's downgrade
            SnippetStore::migrate(&conn).unwrap(); // must not fail on duplicate column
            let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
            assert_eq!(v, MIGRATIONS.len() as i64);
        }
        // And a future-schema DB is left alone (no downgrade).
        {
            let conn = store.conn.lock().unwrap();
            conn.execute_batch("PRAGMA user_version = 99").unwrap();
            SnippetStore::migrate(&conn).unwrap();
            let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
            assert_eq!(v, 99);
        }
    }

    #[test]
    fn sync_dirty_tracking_and_remote_apply_lww() {
        let store = SnippetStore::open_in_memory().unwrap();
        // Personal snippets are never dirty for a team.
        store
            .create(NewSnippet { trigger: ":p".into(), replacement: "x".into(), ..Default::default() })
            .unwrap();
        let t = store
            .create(NewSnippet {
                trigger: ":t".into(),
                replacement: "team".into(),
                team: Some("sec".into()),
                ..Default::default()
            })
            .unwrap();
        let dirty = store.dirty_snippets("sec").unwrap();
        assert_eq!(dirty.len(), 1);
        assert_eq!(dirty[0].id, t.id);

        // Acknowledge → clean; a later edit → dirty again.
        store.mark_pushed(ChangeEntity::Snippet, &t.id, t.version).unwrap();
        assert!(store.dirty_snippets("sec").unwrap().is_empty());
        store
            .update(&t.id, SnippetUpdate {
                trigger: ":t".into(),
                replacement: "team v2".into(),
                team: Some("sec".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(store.dirty_snippets("sec").unwrap().len(), 1);

        // A strictly-newer remote record applies (and acknowledges); an older one is rejected.
        let local = store.get(&t.id).unwrap().unwrap();
        let mut remote = local.clone();
        remote.replacement = "remote wins".into();
        remote.updated_at = "2999-01-01T00:00:00.000Z".into();
        remote.version = local.version + 5;
        assert!(store.apply_remote_snippet(&remote).unwrap());
        assert_eq!(store.get(&t.id).unwrap().unwrap().replacement, "remote wins");
        assert!(store.dirty_snippets("sec").unwrap().is_empty());
        let mut stale = local.clone();
        stale.updated_at = "2000-01-01T00:00:00.000Z".into();
        assert!(!store.apply_remote_snippet(&stale).unwrap());
        assert_eq!(store.get(&t.id).unwrap().unwrap().replacement, "remote wins");

        // Tombstones count as dirty; remote tombstones apply.
        store.soft_delete(&t.id).unwrap();
        assert_eq!(store.dirty_snippets("sec").unwrap().len(), 1);

        // Groups: set_group_team cascades to member snippets and marks both dirty.
        let g = store.create_group(NewGroup { name: "Shared".into() }).unwrap();
        let m = store
            .create(NewSnippet {
                trigger: ":m".into(),
                replacement: "member".into(),
                group_id: Some(g.id.clone()),
                ..Default::default()
            })
            .unwrap();
        store.set_group_team(&g.id, Some("sec")).unwrap();
        assert_eq!(store.dirty_groups("sec").unwrap().len(), 1);
        assert!(store.dirty_snippets("sec").unwrap().iter().any(|s| s.id == m.id));
        assert_eq!(store.get(&m.id).unwrap().unwrap().team.as_deref(), Some("sec"));

        // Change events carry entity + origin.
        let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        store.add_change_listener(move |e| seen2.lock().unwrap().push((e.entity, e.origin)));
        let mut rg = store.get_group(&g.id).unwrap().unwrap();
        rg.name = "Shared v2".into();
        rg.version += 1;
        rg.updated_at = "2999-01-01T00:00:00.000Z".into();
        assert!(store.apply_remote_group(&rg).unwrap());
        assert_eq!(
            seen.lock().unwrap().as_slice(),
            &[(ChangeEntity::Group, ChangeOrigin::Remote)]
        );
    }

    #[test]
    fn scope_filter_parses_prefixes_and_escapes_bare_values() {
        assert_eq!(yaml::scope_filter("exec:/App/Slack"), ("filter_exec", "/App/Slack".into()));
        assert_eq!(yaml::scope_filter("title:.*Jira.*"), ("filter_title", ".*Jira.*".into()));
        assert_eq!(yaml::scope_filter("C++ IDE"), ("filter_exec", "(?i)C\\+\\+ IDE".into()));
    }

    #[test]
    fn default_format_is_plain() {
        let store = SnippetStore::open_in_memory().unwrap();
        let s = store
            .create(NewSnippet { trigger: ":a".into(), replacement: "b".into(), ..Default::default() })
            .unwrap();
        assert_eq!(s.format, "plain");
    }
}
