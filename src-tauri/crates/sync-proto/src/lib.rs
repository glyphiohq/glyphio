//! Glyphio sync wire protocol **v1** — the shared vocabulary between any Glyphio client and any
//! compatible sync backend. Documented for third-party implementers in `docs/SYNC-PROTOCOL.md`;
//! this crate is the executable form of that contract (Apache-2.0 so anyone can build against it).
//!
//! Endpoints (all bearer-authenticated, JSON over HTTPS):
//! * `GET  /v1/me`                                   → [`Me`]
//! * `GET  /v1/teams/{team}/changes?since=N&limit=N` → [`Changes`]
//! * `POST /v1/teams/{team}/changes` [`Push`]        → [`PushAck`]
//!
//! Ordering is by a **server-assigned monotonic sequence** (`since` cursor) — client clocks never
//! order the stream. Conflict resolution is last-write-wins on `(updated_at, version)` per record
//! (see [`lww_wins`]); deletes are tombstones (`deleted_at` set), never row removals.

use serde::{Deserialize, Serialize};

/// Protocol version segment used in URL paths (`/v1/...`).
pub const VERSION: &str = "v1";

fn default_kind() -> String {
    "text".to_string()
}

#[allow(clippy::ptr_arg)]
fn is_default_kind(k: &String) -> bool {
    k == "text"
}

/// A snippet record on the wire. Field semantics match the client store: `updated_at` is
/// RFC3339 UTC with milliseconds (lexicographically ordered), `version` increments per edit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnippetRec {
    pub id: String,
    pub trigger: String,
    pub replacement: String,
    pub format: String,
    /// Snippet kind (additive in v1): `text` (default when absent) | `form` | `popup`.
    /// `command` is NOT a valid wire value — command snippets never sync, and compliant
    /// servers reject pushes carrying it (or `shell`/`script` variables).
    #[serde(default = "default_kind", skip_serializing_if = "is_default_kind")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variables: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_scope: Option<String>,
    /// Identity (subject/email) of the last writer. Informational; the server sets/overrides it
    /// from the authenticated identity on push — clients must not trust peer-supplied values.
    pub owner: String,
    /// Team this record belongs to. Must equal the `{team}` path segment on push.
    pub team: String,
    pub updated_at: String,
    pub version: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<String>,
}

/// A snippet group (folder) on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupRec {
    pub id: String,
    pub name: String,
    pub sort_order: i64,
    pub team: String,
    /// Restricted groups (additive; server-managed) sync only to identities holding a grant —
    /// see the restricted-groups section of `docs/SYNC-PROTOCOL.md`. Clients treat this as
    /// informational; filtering is server-side.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub restricted: bool,
    pub updated_at: String,
    pub version: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<String>,
}

/// Per-team role, strictly ordered. Servers enforce; clients only reflect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Reader,
    Writer,
    Manager,
    Admin,
    Owner,
}

/// Org policy knobs the client must honor (additive; enforced/audited server-side too where
/// applicable). Servers without org governance omit it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrgPolicy {
    /// Who may export **team-shared** groups from the app: `open` | `managers` | `disabled`.
    /// Personal snippets are always the user's own and stay exportable.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub export_team_groups: String,
}

/// `GET /v1/me` — the authenticated identity and the teams it may sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Me {
    pub sub: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    pub teams: Vec<String>,
    /// Role per team (additive in v1; servers without RBAC omit it and clients
    /// assume `writer`). Enforcement is server-side regardless.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub roles: std::collections::HashMap<String, Role>,
    /// Org policy for this identity (additive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<OrgPolicy>,
}

/// `GET /v1/teams/{team}/changes` — everything after the `since` cursor, oldest first.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Changes {
    #[serde(default)]
    pub snippets: Vec<SnippetRec>,
    #[serde(default)]
    pub groups: Vec<GroupRec>,
    /// Pass as `since` on the next pull.
    pub next_cursor: u64,
    /// True when `limit` truncated the page — pull again immediately.
    pub more: bool,
}

/// `POST /v1/teams/{team}/changes` request — a batch of locally-changed records.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Push {
    #[serde(default)]
    pub snippets: Vec<SnippetRec>,
    #[serde(default)]
    pub groups: Vec<GroupRec>,
}

/// Per-record outcome of a push.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushOutcome<T> {
    pub id: String,
    /// `accepted` — the pushed record won LWW and is now authoritative.
    /// `superseded` — the server already holds a newer record; it is returned in `server_record`
    /// and the client must apply it locally.
    pub status: OutcomeStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_record: Option<T>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutcomeStatus {
    Accepted,
    Superseded,
}

/// `POST /v1/teams/{team}/changes` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushAck {
    pub snippets: Vec<PushOutcome<SnippetRec>>,
    pub groups: Vec<PushOutcome<GroupRec>>,
    /// Cursor after these writes; a client that was already at the pre-push cursor may skip
    /// re-pulling its own accepted records by adopting this.
    pub cursor: u64,
}

/// A member of a team, as known to the server. In static-token mode this is the configured
/// token list; in OIDC mode the server records identities as it sees them authenticate
/// ("seen members" — the IdP owns the authoritative list).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Member {
    pub sub: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// RFC3339; absent for configured-but-never-seen members.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<String>,
}

/// `GET /v1/teams/{team}/members` (additive in v1).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Members {
    pub members: Vec<Member>,
}

/// RFC 7807 problem document — the error body for all non-2xx responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Problem {
    pub title: String,
    pub status: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Whether an engine `variables` JSON array contains an executable variable (`shell` /
/// `script`). **Compliant servers MUST reject** pushed snippets where this returns true or
/// where `kind == "command"`: executable content never syncs — a synced shell command would
/// be remote code execution on every teammate's machine. Clients exclude such records from
/// push and quarantine them on pull as defense in depth.
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

/// Last-write-wins: does `(a_updated_at, a_version)` beat `(b_updated_at, b_version)`?
/// `updated_at` is RFC3339 UTC-with-millis, so string comparison is chronological; `version`
/// breaks same-millisecond ties. Exact ties do NOT win (the holder keeps the record), which
/// makes the rule deterministic on both sides of the wire.
pub fn lww_wins(a_updated_at: &str, a_version: i64, b_updated_at: &str, b_version: i64) -> bool {
    (a_updated_at, a_version) > (b_updated_at, b_version)
}

/// Validation limits enforced by compliant servers (and pre-checked by clients).
pub mod limits {
    /// Max records (snippets + groups) per push batch.
    pub const MAX_BATCH: usize = 500;
    /// Max bytes for a snippet `replacement` body. Sized for rich-HTML snippets carrying
    /// inline data-URI images (the editor downscales inserts to stay well under this).
    pub const MAX_REPLACEMENT: usize = 1024 * 1024;
    /// Max bytes for the serialized `variables` array.
    pub const MAX_VARIABLES: usize = 16 * 1024;
    /// Max bytes for `trigger`, `name`, and other short string fields.
    pub const MAX_SHORT_STRING: usize = 512;
    /// Max request body size servers must accept (a push batch may hold several
    /// image-bearing snippets).
    pub const MAX_BODY: usize = 8 * 1024 * 1024;
    /// Default / max page size for `changes`.
    pub const DEFAULT_PAGE: usize = 200;
    pub const MAX_PAGE: usize = 1000;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lww_orders_by_timestamp_then_version_and_ties_lose() {
        assert!(lww_wins("2026-07-02T10:00:00.001Z", 1, "2026-07-02T10:00:00.000Z", 9));
        assert!(lww_wins("2026-07-02T10:00:00.000Z", 2, "2026-07-02T10:00:00.000Z", 1));
        assert!(!lww_wins("2026-07-02T10:00:00.000Z", 1, "2026-07-02T10:00:00.000Z", 1));
        assert!(!lww_wins("2026-07-01T10:00:00.000Z", 9, "2026-07-02T10:00:00.000Z", 1));
    }

    #[test]
    fn wire_shapes_roundtrip_camel_case() {
        let s = SnippetRec {
            id: "i".into(), trigger: ":t".into(), replacement: "r".into(), format: "plain".into(),
            kind: "text".into(), variables: None, group_id: None, app_scope: None,
            owner: "me".into(), team: "sec".into(),
            updated_at: "2026-07-02T00:00:00.000Z".into(), version: 1, deleted_at: None,
        };
        let j = serde_json::to_string(&s).unwrap();
        assert!(j.contains("updatedAt") && !j.contains("groupId")); // camelCase + skipped None
        // Default kind is omitted on the wire (additive for old servers), and absent kind
        // deserializes back to "text" — the round trip below covers both directions.
        assert!(!j.contains("kind"));
        assert_eq!(serde_json::from_str::<SnippetRec>(&j).unwrap(), s);
    }
}
