// SPDX-License-Identifier: Apache-2.0
//! Storage behind the protocol: LWW merge + a per-team monotonic change sequence, seen-member
//! tracking, and per-team role rows (RBAC).

use async_trait::async_trait;
use sync_proto::{Changes, Member, Push, PushAck, Role, SnippetRec};

pub mod dynamo;
pub mod sqlite;

pub type StorageError = Box<dyn std::error::Error + Send + Sync>;

#[async_trait]
pub trait Storage: Send + Sync {
    /// All records for `team` with sequence > `since`, oldest first, at most `limit` of each
    /// kind. `next_cursor` advances past everything returned; `more` signals truncation.
    async fn changes(&self, team: &str, since: u64, limit: usize) -> Result<Changes, StorageError>;

    /// LWW-merge a validated batch (owner already stamped by the caller). Accepted records get
    /// a fresh sequence number; superseded ones return the currently-winning server record.
    async fn merge(&self, team: &str, push: &Push) -> Result<PushAck, StorageError>;

    /// Current server copies of the given snippet ids (missing ids are simply absent).
    /// Used by push enforcement: authorship preservation + ownership checks.
    async fn snippets_by_ids(
        &self,
        team: &str,
        ids: &[String],
    ) -> Result<Vec<SnippetRec>, StorageError>;

    /// Upsert a seen-member record: `sub` authenticated against `team` just now
    /// (`last_seen` = now, RFC3339). Returns `true` on the FIRST sighting (drives the
    /// `member.first_seen` audit entry).
    async fn record_seen(
        &self,
        team: &str,
        sub: &str,
        email: Option<&str>,
    ) -> Result<bool, StorageError>;

    /// Members previously seen on `team`, each with their `last_seen`. Unsorted.
    async fn members(&self, team: &str) -> Result<Vec<Member>, StorageError>;

    // ---- RBAC role rows ----------------------------------------------------

    /// Explicit role row for (team, sub), if any.
    async fn role(&self, team: &str, sub: &str) -> Result<Option<Role>, StorageError>;

    /// Upsert a role row.
    async fn set_role(&self, team: &str, sub: &str, role: Role) -> Result<(), StorageError>;

    /// All role rows for `team`, unsorted.
    async fn roles(&self, team: &str) -> Result<Vec<(String, Role)>, StorageError>;

    /// Remove a role row (the identity falls back to default resolution).
    async fn remove_role(&self, team: &str, sub: &str) -> Result<(), StorageError>;

    /// All role rows for `sub` across teams — grants access to teams outside the identity's
    /// claim. Unsorted.
    async fn roles_for_sub(&self, sub: &str) -> Result<Vec<(String, Role)>, StorageError>;

    // ---- org settings (single-org server: one settings row) -----------------

    async fn org_settings(&self) -> Result<Option<OrgSettings>, StorageError>;
    async fn set_org_settings(&self, settings: &OrgSettings) -> Result<(), StorageError>;

    // ---- team registry / lifecycle -------------------------------------------

    /// Register a team. Returns `false` if it already exists (idempotent create).
    async fn create_team(&self, team: &str) -> Result<bool, StorageError>;

    /// Whether the team is archived (kept on disk, hidden from listings, sync blocked).
    async fn archived(&self, team: &str) -> Result<bool, StorageError>;

    async fn set_archived(&self, team: &str, archived: bool) -> Result<(), StorageError>;

    // ---- audit log (append-only; counts and ids, NEVER snippet content) ------

    /// Append an entry; best-effort purge of entries older than `retention_days` piggybacks
    /// on writes so no background job is needed.
    async fn audit_append(
        &self,
        entry: &AuditEntry,
        retention_days: u32,
    ) -> Result<(), StorageError>;

    /// Newest-first entries, optionally filtered to one team, at most `limit`.
    async fn audit(
        &self,
        team: Option<&str>,
        limit: usize,
    ) -> Result<Vec<AuditEntry>, StorageError>;

    // ---- invite tokens (server-generated; only the SHA-256 is ever stored) ----

    async fn store_token(&self, token: &StoredToken) -> Result<(), StorageError>;

    /// Look up a presented credential by its SHA-256 hex. Returns revoked/expired rows too —
    /// the auth layer decides (and can distinguish "revoked" from "unknown" if needed).
    async fn token_by_sha(&self, sha: &str) -> Result<Option<StoredToken>, StorageError>;

    /// All stored tokens for a sub (for access revocation).
    async fn tokens_for_sub(&self, sub: &str) -> Result<Vec<StoredToken>, StorageError>;

    /// Replace a token's team list (used when revoking one team from a multi-team token).
    async fn update_token_teams(
        &self,
        sha: &str,
        teams: &[String],
    ) -> Result<(), StorageError>;

    async fn revoke_token(&self, sha: &str) -> Result<(), StorageError>;

    // ---- restricted groups + per-identity grants ------------------------------

    async fn set_group_restricted(
        &self,
        team: &str,
        group_id: &str,
        restricted: bool,
    ) -> Result<(), StorageError>;

    /// Ids of the team's currently-restricted groups.
    async fn restricted_groups(&self, team: &str) -> Result<Vec<String>, StorageError>;

    /// Upsert a grant. `level` is `"read"` or `"write"` (validated by the caller).
    async fn set_group_grant(
        &self,
        team: &str,
        group_id: &str,
        sub: &str,
        level: &str,
    ) -> Result<(), StorageError>;

    async fn remove_group_grant(
        &self,
        team: &str,
        group_id: &str,
        sub: &str,
    ) -> Result<(), StorageError>;

    /// All grants on one group: `(sub, level)`.
    async fn group_grants(
        &self,
        team: &str,
        group_id: &str,
    ) -> Result<Vec<(String, String)>, StorageError>;

    /// One identity's grants across the team: `group_id -> level`.
    async fn grants_for_sub(
        &self,
        team: &str,
        sub: &str,
    ) -> Result<std::collections::HashMap<String, String>, StorageError>;

    /// The team's group records (for the dashboard's group list).
    async fn groups(&self, team: &str) -> Result<Vec<sync_proto::GroupRec>, StorageError>;
}

/// A server-generated invite token (day-to-day membership without STATIC_TOKENS env edits).
/// The plaintext is returned exactly once at creation; only this SHA-256 row persists.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredToken {
    pub token_sha256: String,
    pub sub: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    pub teams: Vec<String>,
    /// Pinned role (lowercase), applied to all the token's teams unless a role row overrides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    pub created_by: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<String>,
}

/// Org-wide settings (single row; this server is single-org by design — an enterprise deploys
/// its own instance). Stored as one JSON document so adding knobs never needs a migration.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OrgSettings {
    /// Org default role (falls back to the DEFAULT_ROLE env when unset).
    pub default_role: Option<String>,
    /// Who may create teams: "owners" | "admins" | "bootstrap" (legacy touch-to-own).
    pub team_creation: String,
    /// Who may export team-shared groups in the app: "open" | "managers" | "disabled".
    pub export_team_groups: String,
    /// Audit entries older than this are purged (best-effort, on write).
    pub audit_retention_days: u32,
}

impl Default for OrgSettings {
    fn default() -> Self {
        Self {
            default_role: None,
            team_creation: "bootstrap".into(), // back-compat with pre-org deployments
            export_team_groups: "open".into(),
            audit_retention_days: 365,
        }
    }
}

/// One audit record. `detail` is small structured text (counts, role names) — the log must
/// never contain snippet bodies or tokens.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntry {
    pub ts: String,
    pub actor: String,
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl AuditEntry {
    pub fn new(actor: &str, action: &str) -> Self {
        Self {
            ts: now_rfc3339(),
            actor: actor.to_string(),
            action: action.to_string(),
            team: None,
            target: None,
            detail: None,
        }
    }
    pub fn team(mut self, team: &str) -> Self {
        self.team = Some(team.to_string());
        self
    }
    pub fn target(mut self, target: &str) -> Self {
        self.target = Some(target.to_string());
        self
    }
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// RFC3339 UTC timestamp with milliseconds — the same shape the wire protocol uses everywhere.
pub(crate) fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Wire-stable role <-> string mapping (matches sync-proto's lowercase serde).
pub(crate) fn role_to_str(r: Role) -> &'static str {
    match r {
        Role::Reader => "reader",
        Role::Writer => "writer",
        Role::Manager => "manager",
        Role::Admin => "admin",
        Role::Owner => "owner",
    }
}

pub(crate) fn role_from_str(s: &str) -> Option<Role> {
    match s {
        "reader" => Some(Role::Reader),
        "writer" => Some(Role::Writer),
        "manager" => Some(Role::Manager),
        "admin" => Some(Role::Admin),
        "owner" => Some(Role::Owner),
        _ => None,
    }
}
