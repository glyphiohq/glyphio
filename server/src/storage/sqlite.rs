// SPDX-License-Identifier: Apache-2.0
//! SQLite storage — the self-hosting default. One file, zero external services.
//!
//! Records are stored as their canonical wire JSON (`body`) plus the columns needed for
//! ordering and LWW comparison. Sequence numbers are per-team, allocated transactionally.

use std::path::Path;
use std::sync::Mutex;

use async_trait::async_trait;
use rusqlite::{Connection, OptionalExtension};
use sync_proto::{
    lww_wins, Changes, GroupRec, Member, OutcomeStatus, Push, PushAck, PushOutcome, Role,
    SnippetRec,
};

use super::{Storage, StorageError};

pub struct SqliteStorage {
    conn: Mutex<Connection>,
}

const KIND_SNIPPET: &str = "snip";
const KIND_GROUP: &str = "grp";

impl SqliteStorage {
    pub fn open(path: &str) -> Result<Self, StorageError> {
        if let Some(parent) = Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS records (
                 team       TEXT NOT NULL,
                 kind       TEXT NOT NULL,
                 id         TEXT NOT NULL,
                 seq        INTEGER NOT NULL,
                 updated_at TEXT NOT NULL,
                 version    INTEGER NOT NULL,
                 body       TEXT NOT NULL,
                 PRIMARY KEY (team, kind, id)
             );
             CREATE INDEX IF NOT EXISTS idx_records_team_seq ON records(team, seq);
             CREATE TABLE IF NOT EXISTS counters (team TEXT PRIMARY KEY, seq INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS members (
                 team      TEXT NOT NULL,
                 sub       TEXT NOT NULL,
                 email     TEXT,
                 last_seen TEXT NOT NULL,
                 PRIMARY KEY (team, sub)
             );
             CREATE TABLE IF NOT EXISTS roles (
                 team TEXT NOT NULL,
                 sub  TEXT NOT NULL,
                 role TEXT NOT NULL,
                 PRIMARY KEY (team, sub)
             );
             CREATE INDEX IF NOT EXISTS idx_roles_sub ON roles(sub);
             CREATE TABLE IF NOT EXISTS org_settings (
                 id   INTEGER PRIMARY KEY CHECK (id = 1),
                 body TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS teams (
                 team       TEXT PRIMARY KEY,
                 archived   INTEGER NOT NULL DEFAULT 0,
                 created_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS audit (
                 id     INTEGER PRIMARY KEY AUTOINCREMENT,
                 ts     TEXT NOT NULL,
                 actor  TEXT NOT NULL,
                 action TEXT NOT NULL,
                 team   TEXT,
                 target TEXT,
                 detail TEXT
             );
             CREATE INDEX IF NOT EXISTS idx_audit_ts ON audit(ts);
             CREATE INDEX IF NOT EXISTS idx_audit_team ON audit(team);
             CREATE TABLE IF NOT EXISTS tokens (
                 token_sha256 TEXT PRIMARY KEY,
                 sub          TEXT NOT NULL,
                 email        TEXT,
                 teams        TEXT NOT NULL,
                 role         TEXT,
                 created_by   TEXT NOT NULL,
                 created_at   TEXT NOT NULL,
                 expires_at   TEXT,
                 revoked_at   TEXT
             );
             CREATE INDEX IF NOT EXISTS idx_tokens_sub ON tokens(sub);
             CREATE TABLE IF NOT EXISTS group_flags (
                 team       TEXT NOT NULL,
                 group_id   TEXT NOT NULL,
                 restricted INTEGER NOT NULL DEFAULT 0,
                 PRIMARY KEY (team, group_id)
             );
             CREATE TABLE IF NOT EXISTS group_acl (
                 team     TEXT NOT NULL,
                 group_id TEXT NOT NULL,
                 sub      TEXT NOT NULL,
                 level    TEXT NOT NULL,
                 PRIMARY KEY (team, group_id, sub)
             );",
        )?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    fn next_seq(conn: &Connection, team: &str) -> Result<u64, StorageError> {
        conn.execute(
            "INSERT INTO counters (team, seq) VALUES (?1, 1)
             ON CONFLICT(team) DO UPDATE SET seq = seq + 1",
            [team],
        )?;
        let seq: i64 = conn.query_row("SELECT seq FROM counters WHERE team=?1", [team], |r| r.get(0))?;
        Ok(seq as u64)
    }

    /// Generic LWW upsert used by both record kinds. Returns the outcome for the ack.
    fn merge_one<T: serde::Serialize + serde::de::DeserializeOwned>(
        conn: &Connection,
        team: &str,
        kind: &str,
        id: &str,
        updated_at: &str,
        version: i64,
        record: &T,
    ) -> Result<PushOutcome<T>, StorageError> {
        let existing: Option<(String, i64, String)> = conn
            .query_row(
                "SELECT updated_at, version, body FROM records WHERE team=?1 AND kind=?2 AND id=?3",
                rusqlite::params![team, kind, id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let wins = match &existing {
            Some((u, v, _)) => lww_wins(updated_at, version, u, *v),
            None => true,
        };
        if !wins {
            let (_, _, body) = existing.unwrap();
            return Ok(PushOutcome {
                id: id.to_string(),
                status: OutcomeStatus::Superseded,
                server_record: Some(serde_json::from_str(&body)?),
            });
        }
        let seq = Self::next_seq(conn, team)?;
        conn.execute(
            "INSERT INTO records (team, kind, id, seq, updated_at, version, body)
             VALUES (?1,?2,?3,?4,?5,?6,?7)
             ON CONFLICT(team, kind, id) DO UPDATE SET
                 seq=excluded.seq, updated_at=excluded.updated_at,
                 version=excluded.version, body=excluded.body",
            rusqlite::params![team, kind, id, seq as i64, updated_at, version, serde_json::to_string(record)?],
        )?;
        Ok(PushOutcome { id: id.to_string(), status: OutcomeStatus::Accepted, server_record: None })
    }
}

#[async_trait]
impl Storage for SqliteStorage {
    async fn changes(&self, team: &str, since: u64, limit: usize) -> Result<Changes, StorageError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT kind, body, seq FROM records WHERE team=?1 AND seq>?2 ORDER BY seq LIMIT ?3",
        )?;
        // Fetch one extra row to detect truncation.
        let rows = stmt.query_map(
            rusqlite::params![team, since as i64, (limit + 1) as i64],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)),
        )?;
        let mut all: Vec<(String, String, i64)> = rows.collect::<Result<_, _>>()?;
        let more = all.len() > limit;
        if more {
            all.truncate(limit);
        }
        let mut out = Changes { snippets: vec![], groups: vec![], next_cursor: since, more };
        for (kind, body, seq) in all {
            match kind.as_str() {
                KIND_SNIPPET => out.snippets.push(serde_json::from_str::<SnippetRec>(&body)?),
                KIND_GROUP => out.groups.push(serde_json::from_str::<GroupRec>(&body)?),
                _ => {}
            }
            out.next_cursor = out.next_cursor.max(seq as u64);
        }
        Ok(out)
    }

    async fn merge(&self, team: &str, push: &Push) -> Result<PushAck, StorageError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let mut ack = PushAck { snippets: vec![], groups: vec![], cursor: 0 };
        for g in &push.groups {
            ack.groups.push(Self::merge_one(&tx, team, KIND_GROUP, &g.id, &g.updated_at, g.version, g)?);
        }
        for s in &push.snippets {
            ack.snippets
                .push(Self::merge_one(&tx, team, KIND_SNIPPET, &s.id, &s.updated_at, s.version, s)?);
        }
        let cursor: i64 = tx
            .query_row("SELECT seq FROM counters WHERE team=?1", [team], |r| r.get(0))
            .optional()?
            .unwrap_or(0);
        ack.cursor = cursor as u64;
        tx.commit()?;
        Ok(ack)
    }

    async fn record_seen(
        &self,
        team: &str,
        sub: &str,
        email: Option<&str>,
    ) -> Result<bool, StorageError> {
        let conn = self.conn.lock().unwrap();
        let existed: bool = conn
            .query_row(
                "SELECT 1 FROM members WHERE team=?1 AND sub=?2",
                rusqlite::params![team, sub],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        conn.execute(
            "INSERT INTO members (team, sub, email, last_seen) VALUES (?1,?2,?3,?4)
             ON CONFLICT(team, sub) DO UPDATE SET
                 email = COALESCE(excluded.email, email), last_seen = excluded.last_seen",
            rusqlite::params![team, sub, email, super::now_rfc3339()],
        )?;
        Ok(!existed)
    }

    async fn members(&self, team: &str) -> Result<Vec<Member>, StorageError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT sub, email, last_seen FROM members WHERE team=?1")?;
        let rows = stmt.query_map([team], |r| {
            Ok(Member { sub: r.get(0)?, email: r.get(1)?, last_seen: r.get(2)? })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    async fn snippets_by_ids(
        &self,
        team: &str,
        ids: &[String],
    ) -> Result<Vec<SnippetRec>, StorageError> {
        let conn = self.conn.lock().unwrap();
        let mut out = Vec::new();
        let mut stmt = conn
            .prepare("SELECT body FROM records WHERE team=?1 AND kind=?2 AND id=?3")?;
        for id in ids {
            if let Some(body) = stmt
                .query_row(rusqlite::params![team, KIND_SNIPPET, id], |r| {
                    r.get::<_, String>(0)
                })
                .optional()?
            {
                out.push(serde_json::from_str::<SnippetRec>(&body)?);
            }
        }
        Ok(out)
    }

    async fn role(&self, team: &str, sub: &str) -> Result<Option<Role>, StorageError> {
        let conn = self.conn.lock().unwrap();
        let s: Option<String> = conn
            .query_row(
                "SELECT role FROM roles WHERE team=?1 AND sub=?2",
                rusqlite::params![team, sub],
                |r| r.get(0),
            )
            .optional()?;
        Ok(s.and_then(|v| super::role_from_str(&v)))
    }

    async fn set_role(&self, team: &str, sub: &str, role: Role) -> Result<(), StorageError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO roles (team, sub, role) VALUES (?1,?2,?3)
             ON CONFLICT(team, sub) DO UPDATE SET role = excluded.role",
            rusqlite::params![team, sub, super::role_to_str(role)],
        )?;
        Ok(())
    }

    async fn roles(&self, team: &str) -> Result<Vec<(String, Role)>, StorageError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT sub, role FROM roles WHERE team=?1")?;
        let rows = stmt.query_map([team], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        Ok(rows
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter_map(|(s, r)| super::role_from_str(&r).map(|role| (s, role)))
            .collect())
    }

    async fn remove_role(&self, team: &str, sub: &str) -> Result<(), StorageError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM roles WHERE team=?1 AND sub=?2", rusqlite::params![team, sub])?;
        Ok(())
    }

    async fn roles_for_sub(&self, sub: &str) -> Result<Vec<(String, Role)>, StorageError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT team, role FROM roles WHERE sub=?1")?;
        let rows = stmt.query_map([sub], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        Ok(rows
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter_map(|(t, r)| super::role_from_str(&r).map(|role| (t, role)))
            .collect())
    }

    async fn org_settings(&self) -> Result<Option<super::OrgSettings>, StorageError> {
        let conn = self.conn.lock().unwrap();
        let body: Option<String> = conn
            .query_row("SELECT body FROM org_settings WHERE id=1", [], |r| r.get(0))
            .optional()?;
        Ok(match body {
            Some(b) => Some(serde_json::from_str(&b)?),
            None => None,
        })
    }

    async fn set_org_settings(&self, settings: &super::OrgSettings) -> Result<(), StorageError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO org_settings (id, body) VALUES (1, ?1)
             ON CONFLICT(id) DO UPDATE SET body = excluded.body",
            [serde_json::to_string(settings)?],
        )?;
        Ok(())
    }

    async fn create_team(&self, team: &str) -> Result<bool, StorageError> {
        let conn = self.conn.lock().unwrap();
        let inserted = conn.execute(
            "INSERT OR IGNORE INTO teams (team, archived, created_at) VALUES (?1, 0, ?2)",
            rusqlite::params![team, super::now_rfc3339()],
        )?;
        Ok(inserted > 0)
    }

    async fn archived(&self, team: &str) -> Result<bool, StorageError> {
        let conn = self.conn.lock().unwrap();
        let v: Option<i64> = conn
            .query_row("SELECT archived FROM teams WHERE team=?1", [team], |r| r.get(0))
            .optional()?;
        Ok(v.unwrap_or(0) != 0)
    }

    async fn set_archived(&self, team: &str, archived: bool) -> Result<(), StorageError> {
        let conn = self.conn.lock().unwrap();
        // Register-on-archive so pre-registry (bootstrap-era) teams can be archived too.
        conn.execute(
            "INSERT INTO teams (team, archived, created_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(team) DO UPDATE SET archived = excluded.archived",
            rusqlite::params![team, archived as i64, super::now_rfc3339()],
        )?;
        Ok(())
    }

    async fn audit_append(
        &self,
        entry: &super::AuditEntry,
        retention_days: u32,
    ) -> Result<(), StorageError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO audit (ts, actor, action, team, target, detail)
             VALUES (?1,?2,?3,?4,?5,?6)",
            rusqlite::params![
                entry.ts, entry.actor, entry.action, entry.team, entry.target, entry.detail
            ],
        )?;
        // Best-effort retention purge (RFC3339 strings order chronologically).
        let cutoff = (chrono::Utc::now() - chrono::Duration::days(retention_days as i64))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        conn.execute("DELETE FROM audit WHERE ts < ?1", [cutoff])?;
        Ok(())
    }

    async fn store_token(&self, t: &super::StoredToken) -> Result<(), StorageError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO tokens (token_sha256, sub, email, teams, role, created_by, created_at,
                expires_at, revoked_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            rusqlite::params![
                t.token_sha256, t.sub, t.email, serde_json::to_string(&t.teams)?, t.role,
                t.created_by, t.created_at, t.expires_at, t.revoked_at
            ],
        )?;
        Ok(())
    }

    async fn token_by_sha(&self, sha: &str) -> Result<Option<super::StoredToken>, StorageError> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT token_sha256, sub, email, teams, role, created_by, created_at,
                        expires_at, revoked_at FROM tokens WHERE token_sha256=?1",
                [sha],
                row_to_token,
            )
            .optional()?;
        Ok(row)
    }

    async fn tokens_for_sub(&self, sub: &str) -> Result<Vec<super::StoredToken>, StorageError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT token_sha256, sub, email, teams, role, created_by, created_at,
                    expires_at, revoked_at FROM tokens WHERE sub=?1",
        )?;
        let rows = stmt.query_map([sub], row_to_token)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    async fn update_token_teams(&self, sha: &str, teams: &[String]) -> Result<(), StorageError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE tokens SET teams=?2 WHERE token_sha256=?1",
            rusqlite::params![sha, serde_json::to_string(teams)?],
        )?;
        Ok(())
    }

    async fn revoke_token(&self, sha: &str) -> Result<(), StorageError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE tokens SET revoked_at=?2 WHERE token_sha256=?1 AND revoked_at IS NULL",
            rusqlite::params![sha, super::now_rfc3339()],
        )?;
        Ok(())
    }

    async fn set_group_restricted(
        &self,
        team: &str,
        group_id: &str,
        restricted: bool,
    ) -> Result<(), StorageError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO group_flags (team, group_id, restricted) VALUES (?1,?2,?3)
             ON CONFLICT(team, group_id) DO UPDATE SET restricted = excluded.restricted",
            rusqlite::params![team, group_id, restricted as i64],
        )?;
        Ok(())
    }

    async fn restricted_groups(&self, team: &str) -> Result<Vec<String>, StorageError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT group_id FROM group_flags WHERE team=?1 AND restricted=1")?;
        let rows = stmt.query_map([team], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    async fn set_group_grant(
        &self,
        team: &str,
        group_id: &str,
        sub: &str,
        level: &str,
    ) -> Result<(), StorageError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO group_acl (team, group_id, sub, level) VALUES (?1,?2,?3,?4)
             ON CONFLICT(team, group_id, sub) DO UPDATE SET level = excluded.level",
            rusqlite::params![team, group_id, sub, level],
        )?;
        Ok(())
    }

    async fn remove_group_grant(
        &self,
        team: &str,
        group_id: &str,
        sub: &str,
    ) -> Result<(), StorageError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM group_acl WHERE team=?1 AND group_id=?2 AND sub=?3",
            rusqlite::params![team, group_id, sub],
        )?;
        Ok(())
    }

    async fn group_grants(
        &self,
        team: &str,
        group_id: &str,
    ) -> Result<Vec<(String, String)>, StorageError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT sub, level FROM group_acl WHERE team=?1 AND group_id=?2 ORDER BY sub")?;
        let rows = stmt.query_map(rusqlite::params![team, group_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    async fn grants_for_sub(
        &self,
        team: &str,
        sub: &str,
    ) -> Result<std::collections::HashMap<String, String>, StorageError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT group_id, level FROM group_acl WHERE team=?1 AND sub=?2")?;
        let rows = stmt.query_map(rusqlite::params![team, sub], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    async fn groups(&self, team: &str) -> Result<Vec<GroupRec>, StorageError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT body FROM records WHERE team=?1 AND kind=?2 ORDER BY seq")?;
        let rows = stmt.query_map(rusqlite::params![team, KIND_GROUP], |r| {
            r.get::<_, String>(0)
        })?;
        let mut out = Vec::new();
        for body in rows {
            out.push(serde_json::from_str::<GroupRec>(&body?)?);
        }
        Ok(out)
    }

    async fn audit(
        &self,
        team: Option<&str>,
        limit: usize,
    ) -> Result<Vec<super::AuditEntry>, StorageError> {
        let conn = self.conn.lock().unwrap();
        let map = |r: &rusqlite::Row| {
            Ok(super::AuditEntry {
                ts: r.get(0)?,
                actor: r.get(1)?,
                action: r.get(2)?,
                team: r.get(3)?,
                target: r.get(4)?,
                detail: r.get(5)?,
            })
        };
        let rows = match team {
            Some(t) => {
                let mut stmt = conn.prepare(
                    "SELECT ts, actor, action, team, target, detail FROM audit
                     WHERE team=?1 ORDER BY id DESC LIMIT ?2",
                )?;
                let it = stmt.query_map(rusqlite::params![t, limit as i64], map)?;
                it.collect::<Result<Vec<_>, _>>()?
            }
            None => {
                let mut stmt = conn.prepare(
                    "SELECT ts, actor, action, team, target, detail FROM audit
                     ORDER BY id DESC LIMIT ?1",
                )?;
                let it = stmt.query_map([limit as i64], map)?;
                it.collect::<Result<Vec<_>, _>>()?
            }
        };
        Ok(rows)
    }
}

fn row_to_token(r: &rusqlite::Row) -> rusqlite::Result<super::StoredToken> {
    let teams_json: String = r.get(3)?;
    Ok(super::StoredToken {
        token_sha256: r.get(0)?,
        sub: r.get(1)?,
        email: r.get(2)?,
        teams: serde_json::from_str(&teams_json).unwrap_or_default(),
        role: r.get(4)?,
        created_by: r.get(5)?,
        created_at: r.get(6)?,
        expires_at: r.get(7)?,
        revoked_at: r.get(8)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snip(id: &str, updated_at: &str, version: i64, body: &str) -> SnippetRec {
        SnippetRec {
            id: id.into(),
            trigger: ":t".into(),
            replacement: body.into(),
            format: "plain".into(),
            kind: "text".into(),
            variables: None,
            group_id: None,
            app_scope: None,
            owner: "u1".into(),
            team: "sec".into(),
            updated_at: updated_at.into(),
            version,
            deleted_at: None,
        }
    }

    #[tokio::test]
    async fn lww_merge_accept_supersede_and_pagination() {
        let dir = tempfile::tempdir().unwrap();
        let s = SqliteStorage::open(dir.path().join("t.db").to_str().unwrap()).unwrap();

        // Accept a new record.
        let ack = s
            .merge("sec", &Push { snippets: vec![snip("a", "2026-07-02T10:00:00.000Z", 1, "v1")], groups: vec![] })
            .await
            .unwrap();
        assert!(matches!(ack.snippets[0].status, OutcomeStatus::Accepted));
        assert_eq!(ack.cursor, 1);

        // Older push → superseded, server record returned.
        let ack = s
            .merge("sec", &Push { snippets: vec![snip("a", "2026-07-02T09:00:00.000Z", 5, "stale")], groups: vec![] })
            .await
            .unwrap();
        assert!(matches!(ack.snippets[0].status, OutcomeStatus::Superseded));
        assert_eq!(ack.snippets[0].server_record.as_ref().unwrap().replacement, "v1");

        // Newer push (same ts, higher version) → accepted.
        let ack = s
            .merge("sec", &Push { snippets: vec![snip("a", "2026-07-02T10:00:00.000Z", 2, "v2")], groups: vec![] })
            .await
            .unwrap();
        assert!(matches!(ack.snippets[0].status, OutcomeStatus::Accepted));

        // Changes: only after `since`; pagination flags truncation.
        for i in 0..5 {
            s.merge(
                "sec",
                &Push {
                    snippets: vec![snip(&format!("p{i}"), "2026-07-02T11:00:00.000Z", 1, "x")],
                    groups: vec![],
                },
            )
            .await
            .unwrap();
        }
        let page = s.changes("sec", 0, 3).await.unwrap();
        assert_eq!(page.snippets.len(), 3);
        assert!(page.more);
        let rest = s.changes("sec", page.next_cursor, 100).await.unwrap();
        assert!(!rest.more);
        assert_eq!(page.snippets.len() + rest.snippets.len(), 6);

        // Team isolation.
        assert!(s.changes("other", 0, 100).await.unwrap().snippets.is_empty());
    }

    #[tokio::test]
    async fn seen_members_upsert_and_isolate_by_team() {
        let dir = tempfile::tempdir().unwrap();
        let s = SqliteStorage::open(dir.path().join("t.db").to_str().unwrap()).unwrap();

        s.record_seen("sec", "alice", Some("alice@example.com")).await.unwrap();
        let first = s.members("sec").await.unwrap();
        assert_eq!(first.len(), 1);
        let t0 = first[0].last_seen.clone().unwrap();

        // Upsert: same member again → still one row, last_seen advances, email kept when
        // the new sighting has none (COALESCE).
        std::thread::sleep(std::time::Duration::from_millis(5));
        s.record_seen("sec", "alice", None).await.unwrap();
        let again = s.members("sec").await.unwrap();
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].email.as_deref(), Some("alice@example.com"));
        assert!(again[0].last_seen.clone().unwrap() > t0);

        // Different team → separate list.
        s.record_seen("other", "bob", None).await.unwrap();
        assert_eq!(s.members("sec").await.unwrap().len(), 1);
        assert_eq!(s.members("other").await.unwrap().len(), 1);
    }


    #[tokio::test]
    async fn role_rows_crud_and_lookup_by_sub() {
        let dir = tempfile::tempdir().unwrap();
        let s = SqliteStorage::open(dir.path().join("t.db").to_str().unwrap()).unwrap();

        assert!(s.role("sec", "alice").await.unwrap().is_none());
        s.set_role("sec", "alice", Role::Owner).await.unwrap();
        s.set_role("sec", "bob", Role::Manager).await.unwrap();
        s.set_role("ops", "alice", Role::Reader).await.unwrap();
        assert_eq!(s.role("sec", "alice").await.unwrap(), Some(Role::Owner));

        // Upsert overwrites.
        s.set_role("sec", "bob", Role::Writer).await.unwrap();
        assert_eq!(s.role("sec", "bob").await.unwrap(), Some(Role::Writer));

        let mut team_roles = s.roles("sec").await.unwrap();
        team_roles.sort();
        assert_eq!(team_roles, vec![("alice".into(), Role::Owner), ("bob".into(), Role::Writer)]);

        let mut mine = s.roles_for_sub("alice").await.unwrap();
        mine.sort();
        assert_eq!(mine, vec![("ops".into(), Role::Reader), ("sec".into(), Role::Owner)]);

        s.remove_role("sec", "bob").await.unwrap();
        assert!(s.role("sec", "bob").await.unwrap().is_none());
        assert_eq!(s.roles("sec").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn tombstones_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let s = SqliteStorage::open(dir.path().join("t.db").to_str().unwrap()).unwrap();
        let mut rec = snip("a", "2026-07-02T10:00:00.000Z", 1, "v1");
        s.merge("sec", &Push { snippets: vec![rec.clone()], groups: vec![] }).await.unwrap();
        rec.deleted_at = Some("2026-07-02T10:05:00.000Z".into());
        rec.updated_at = "2026-07-02T10:05:00.000Z".into();
        rec.version = 2;
        s.merge("sec", &Push { snippets: vec![rec], groups: vec![] }).await.unwrap();
        let page = s.changes("sec", 0, 10).await.unwrap();
        assert_eq!(page.snippets.len(), 1);
        assert!(page.snippets[0].deleted_at.is_some());
    }
}
