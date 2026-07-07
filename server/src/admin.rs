// SPDX-License-Identifier: Apache-2.0
//! Admin API (`/admin/v1/...`, same bearer auth) + the bundled admin console (`GET /admin`).
//!
//! Role-change rules (capability matrix):
//! * **manager** may set/remove roles ≤ writer (add members to their team), on targets ≤ writer;
//! * **admin** may set/remove roles ≤ manager, and only on targets currently ≤ manager;
//! * **owner** may set/remove any role, including granting `owner` (ownership transfer =
//!   an owner crowning a second owner; both hold `owner` until one demotes the other).
//!
//! Self-demotion is allowed — if a team loses its last owner, the next successful toucher
//! re-bootstraps as owner.

use axum::extract::{Path, State};
use rand::RngCore;
use axum::response::Html;
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use sync_proto::Role;

use crate::auth::Identity;
use crate::error::ApiError;
use crate::rbac;
use crate::storage::{AuditEntry, OrgSettings};
use crate::AppState;

#[derive(Serialize)]
pub struct TeamRole {
    pub team: String,
    pub role: Role,
}

#[derive(Serialize)]
pub struct RoleRow {
    pub sub: String,
    pub role: Role,
}

#[derive(Deserialize)]
pub struct SetRoleBody {
    pub role: String,
}

/// Teams the caller can administer in some capacity (role ≥ manager), from role rows ∪
/// identity claim. Archived teams are excluded. In bootstrap team-creation mode, a team with
/// no owner row yet is shown as `owner` (the caller would bootstrap it on first touch).
pub async fn teams(
    State(state): State<AppState>,
    Extension(id): Extension<Identity>,
) -> Result<Json<Vec<TeamRole>>, ApiError> {
    let org = rbac::org(&state).await?;
    let mut teams = id.teams.clone();
    for (team, _) in state.storage.roles_for_sub(&id.sub).await.map_err(ApiError::Storage)? {
        if !teams.contains(&team) {
            teams.push(team);
        }
    }
    teams.sort();
    let mut out = Vec::new();
    for team in teams {
        if state.storage.archived(&team).await.map_err(ApiError::Storage)? {
            continue;
        }
        let Some(mut role) = rbac::resolve_role_with_org(&state, &org, &team, &id).await? else {
            continue;
        };
        if org.team_creation == "bootstrap" && id.teams.iter().any(|t| t == &team) {
            let has_owner = state
                .storage
                .roles(&team)
                .await
                .map_err(ApiError::Storage)?
                .iter()
                .any(|(_, r)| *r == Role::Owner);
            if !has_owner {
                role = Role::Owner; // virtual until a team-scoped request records it
            }
        }
        if role >= Role::Manager {
            out.push(TeamRole { team, role });
        }
    }
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct CreateTeamBody {
    pub team: String,
}

/// Explicit team creation, gated by the org `team_creation` policy. The creator becomes the
/// team's owner. Re-POSTing an archived team's name as one of its owners un-archives it.
pub async fn create_team(
    State(state): State<AppState>,
    Extension(id): Extension<Identity>,
    Json(body): Json<CreateTeamBody>,
) -> Result<Json<TeamRole>, ApiError> {
    let team = body.team.trim().to_string();
    if team.is_empty()
        || team.len() > 64
        || !team.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(ApiError::Validation(
            "team must be 1-64 chars of [A-Za-z0-9._-]".into(),
        ));
    }
    let org = rbac::org(&state).await?;

    // Policy gate. "owners": any existing team owner; "admins": admin+ somewhere;
    // "bootstrap": open to any authenticated identity (legacy behavior, made explicit).
    let mut best = Role::Reader;
    let mut my_teams = id.teams.clone();
    for (t, _) in state.storage.roles_for_sub(&id.sub).await.map_err(ApiError::Storage)? {
        if !my_teams.contains(&t) {
            my_teams.push(t);
        }
    }
    for t in &my_teams {
        if let Some(r) = rbac::resolve_role_with_org(&state, &org, t, &id).await? {
            best = best.max(r);
        }
    }
    let allowed = match org.team_creation.as_str() {
        "owners" => best >= Role::Owner,
        "admins" => best >= Role::Admin,
        _ => true, // bootstrap
    };
    if !allowed {
        return Err(ApiError::Forbidden);
    }

    let created = state.storage.create_team(&team).await.map_err(ApiError::Storage)?;
    if !created {
        // Existing name: an owner of the archived team may revive it; otherwise conflict.
        let archived = state.storage.archived(&team).await.map_err(ApiError::Storage)?;
        let my_role = rbac::resolve_role_with_org(&state, &org, &team, &id).await?;
        if archived && my_role == Some(Role::Owner) {
            state.storage.set_archived(&team, false).await.map_err(ApiError::Storage)?;
            rbac::audit_log(&state, AuditEntry::new(&id.sub, "team.unarchive").team(&team)).await;
            return Ok(Json(TeamRole { team, role: Role::Owner }));
        }
        return Err(ApiError::Validation(format!("team {team:?} already exists")));
    }
    state.storage.set_role(&team, &id.sub, Role::Owner).await.map_err(ApiError::Storage)?;
    rbac::audit_log(&state, AuditEntry::new(&id.sub, "team.create").team(&team)).await;
    Ok(Json(TeamRole { team, role: Role::Owner }))
}

/// Archive a team (owner only): data stays, listings hide it, sync answers 403.
pub async fn archive_team(
    State(state): State<AppState>,
    Extension(id): Extension<Identity>,
    Path(team): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let caller = rbac::ensure_access(&state, &team, &id, Role::Owner).await?;
    debug_assert!(caller >= Role::Owner);
    state.storage.set_archived(&team, true).await.map_err(ApiError::Storage)?;
    rbac::audit_log(&state, AuditEntry::new(&id.sub, "team.archive").team(&team)).await;
    Ok(Json(serde_json::json!({ "archived": team })))
}

pub async fn list_roles(
    State(state): State<AppState>,
    Extension(id): Extension<Identity>,
    Path(team): Path<String>,
) -> Result<Json<Vec<RoleRow>>, ApiError> {
    rbac::ensure_access(&state, &team, &id, Role::Manager).await?;
    let mut rows: Vec<RoleRow> = state
        .storage
        .roles(&team)
        .await
        .map_err(ApiError::Storage)?
        .into_iter()
        .map(|(sub, role)| RoleRow { sub, role })
        .collect();
    rows.sort_by(|a, b| a.sub.cmp(&b.sub));
    Ok(Json(rows))
}

/// May `caller` assign/remove `target_new` on a target currently holding `target_current`?
fn may_administer(caller: Role, target_current: Role, target_new: Role) -> bool {
    match caller {
        Role::Owner => true,
        Role::Admin => target_current <= Role::Manager && target_new <= Role::Manager,
        Role::Manager => target_current <= Role::Writer && target_new <= Role::Writer,
        _ => false,
    }
}

pub async fn set_role(
    State(state): State<AppState>,
    Extension(id): Extension<Identity>,
    Path((team, sub)): Path<(String, String)>,
    Json(body): Json<SetRoleBody>,
) -> Result<Json<RoleRow>, ApiError> {
    let caller = rbac::ensure_access(&state, &team, &id, Role::Manager).await?;
    let Some(new_role) = crate::storage::role_from_str(&body.role) else {
        return Err(ApiError::Validation(format!(
            "invalid role {:?} (reader|writer|manager|admin|owner)",
            body.role
        )));
    };
    let current = state
        .storage
        .role(&team, &sub)
        .await
        .map_err(ApiError::Storage)?
        .unwrap_or(Role::Reader);
    if !may_administer(caller, current, new_role) {
        return Err(ApiError::Forbidden);
    }
    state.storage.set_role(&team, &sub, new_role).await.map_err(ApiError::Storage)?;
    rbac::audit_log(
        &state,
        AuditEntry::new(&id.sub, "role.set")
            .team(&team)
            .target(&sub)
            .detail(crate::storage::role_to_str(new_role)),
    )
    .await;
    Ok(Json(RoleRow { sub, role: new_role }))
}

pub async fn remove_role(
    State(state): State<AppState>,
    Extension(id): Extension<Identity>,
    Path((team, sub)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let caller = rbac::ensure_access(&state, &team, &id, Role::Manager).await?;
    let current = state
        .storage
        .role(&team, &sub)
        .await
        .map_err(ApiError::Storage)?
        .unwrap_or(Role::Reader);
    if !may_administer(caller, current, Role::Reader) {
        return Err(ApiError::Forbidden);
    }
    state.storage.remove_role(&team, &sub).await.map_err(ApiError::Storage)?;
    rbac::audit_log(&state, AuditEntry::new(&id.sub, "role.remove").team(&team).target(&sub))
        .await;
    Ok(Json(serde_json::json!({ "removed": sub })))
}

// ---- org settings ---------------------------------------------------------------

/// Effective role per team for the caller (claim ∪ rows), used by org/audit authz.
async fn caller_roles(
    state: &AppState,
    id: &Identity,
    org: &OrgSettings,
) -> Result<Vec<(String, Role)>, ApiError> {
    let mut teams = id.teams.clone();
    for (t, _) in state.storage.roles_for_sub(&id.sub).await.map_err(ApiError::Storage)? {
        if !teams.contains(&t) {
            teams.push(t);
        }
    }
    let mut out = Vec::new();
    for t in teams {
        if let Some(r) = rbac::resolve_role_with_org(state, org, &t, id).await? {
            out.push((t, r));
        }
    }
    Ok(out)
}

/// GET /admin/v1/org — visible to any role-holder (settings shape team behavior for everyone).
pub async fn get_org(
    State(state): State<AppState>,
    Extension(id): Extension<Identity>,
) -> Result<Json<OrgSettings>, ApiError> {
    let org = rbac::org(&state).await?;
    if caller_roles(&state, &id, &org).await?.is_empty() {
        return Err(ApiError::Forbidden);
    }
    Ok(Json(org))
}

/// PUT /admin/v1/org — single-org server: org owners = team owners, so any identity owning
/// at least one team may change org settings (documented in README).
pub async fn put_org(
    State(state): State<AppState>,
    Extension(id): Extension<Identity>,
    Json(new): Json<OrgSettings>,
) -> Result<Json<OrgSettings>, ApiError> {
    let org = rbac::org(&state).await?;
    let is_owner =
        caller_roles(&state, &id, &org).await?.iter().any(|(_, r)| *r == Role::Owner);
    if !is_owner {
        return Err(ApiError::Forbidden);
    }
    // Validate enum-ish knobs.
    if let Some(dr) = &new.default_role {
        if crate::storage::role_from_str(dr).is_none() {
            return Err(ApiError::Validation(format!("invalid defaultRole {dr:?}")));
        }
    }
    if !matches!(new.team_creation.as_str(), "owners" | "admins" | "bootstrap") {
        return Err(ApiError::Validation("teamCreation must be owners|admins|bootstrap".into()));
    }
    if !matches!(new.export_team_groups.as_str(), "open" | "managers" | "disabled") {
        return Err(ApiError::Validation(
            "exportTeamGroups must be open|managers|disabled".into(),
        ));
    }
    if new.audit_retention_days == 0 || new.audit_retention_days > 3650 {
        return Err(ApiError::Validation("auditRetentionDays must be 1..=3650".into()));
    }
    state.storage.set_org_settings(&new).await.map_err(ApiError::Storage)?;
    rbac::audit_log(
        &state,
        AuditEntry::new(&id.sub, "org.settings").detail(
            serde_json::to_string(&new).unwrap_or_default(),
        ),
    )
    .await;
    Ok(Json(new))
}

// ---- console config (unauthenticated) --------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsoleConfig {
    /// Whether the console can offer browser OIDC sign-in (issuer + client id configured).
    pub oidc_enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issuer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// Space-separated scopes for the console's auth request (`ADMIN_OIDC_SCOPES`,
    /// default `openid profile email`; include your groups scope if the IdP needs it).
    pub scopes: String,
}

/// GET /admin/v1/config — public: the console needs the issuer/client id BEFORE sign-in,
/// and both are public knowledge (they appear in every auth redirect). No secrets here.
/// `ADMIN_OIDC_CLIENT_ID` lets operators register a dedicated public SPA client for the
/// console; it falls back to `OIDC_AUDIENCE` when the same client is used for both.
pub async fn console_config() -> Json<ConsoleConfig> {
    let non_empty = |v: std::result::Result<String, std::env::VarError>| {
        v.ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    };
    let issuer = non_empty(std::env::var("OIDC_ISSUER"));
    let client_id = non_empty(std::env::var("ADMIN_OIDC_CLIENT_ID"))
        .or_else(|| non_empty(std::env::var("OIDC_AUDIENCE")));
    let scopes = non_empty(std::env::var("ADMIN_OIDC_SCOPES"))
        .unwrap_or_else(|| "openid profile email".to_string());
    Json(ConsoleConfig {
        oidc_enabled: issuer.is_some() && client_id.is_some(),
        issuer,
        client_id,
        scopes,
    })
}

// ---- stats (dashboard overview) ---------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamStat {
    pub team: String,
    pub role: Role,
    pub members: usize,
    pub archived: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DayActivity {
    pub day: String, // YYYY-MM-DD (UTC)
    pub pushes: u32,
    pub roles: u32,
    pub invites: u32,
    pub other: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub teams: Vec<TeamStat>,
    /// Last 30 days of audit activity, oldest first. Empty for callers without audit access
    /// (managers see team counters only).
    pub activity: Vec<DayActivity>,
}

/// GET /admin/v1/stats — Overview counters + activity sparkline data. Team counters cover
/// the teams the caller administers; activity is bucketed from the audit log under the same
/// visibility rules as `GET /admin/v1/audit`.
pub async fn stats(
    State(state): State<AppState>,
    Extension(id): Extension<Identity>,
) -> Result<Json<Stats>, ApiError> {
    let org = rbac::org(&state).await?;
    let mine = caller_roles(&state, &id, &org).await?;
    let admin_or_up: Vec<&String> =
        mine.iter().filter(|(_, r)| *r >= Role::Admin).map(|(t, _)| t).collect();
    let is_owner = mine.iter().any(|(_, r)| *r == Role::Owner);
    if mine.iter().all(|(_, r)| *r < Role::Manager) {
        return Err(ApiError::Forbidden);
    }

    let mut teams = Vec::new();
    for (team, role) in mine.iter().filter(|(_, r)| *r >= Role::Manager) {
        let mut subs: std::collections::HashSet<String> = state
            .storage
            .roles(team)
            .await
            .map_err(ApiError::Storage)?
            .into_iter()
            .map(|(sub, _)| sub)
            .collect();
        for m in state.storage.members(team).await.map_err(ApiError::Storage)? {
            subs.insert(m.sub);
        }
        teams.push(TeamStat {
            team: team.clone(),
            role: *role,
            members: subs.len(),
            archived: state.storage.archived(team).await.map_err(ApiError::Storage)?,
        });
    }
    teams.sort_by(|a, b| a.team.cmp(&b.team));

    // Activity buckets, mirroring get_audit visibility (owner: everything; admins: their teams).
    let mut activity: Vec<DayActivity> = Vec::new();
    if is_owner || !admin_or_up.is_empty() {
        let mut entries = state.storage.audit(None, 2000).await.map_err(ApiError::Storage)?;
        if !is_owner {
            entries.retain(|e| e.team.as_ref().is_some_and(|t| admin_or_up.contains(&t)));
        }
        let cutoff = (chrono::Utc::now() - chrono::Duration::days(30))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let mut buckets: std::collections::BTreeMap<String, DayActivity> = Default::default();
        for e in entries.iter().filter(|e| e.ts.as_str() >= cutoff.as_str()) {
            let day = e.ts.get(0..10).unwrap_or("").to_string();
            if day.is_empty() {
                continue;
            }
            let b = buckets.entry(day.clone()).or_insert_with(|| DayActivity {
                day,
                pushes: 0,
                roles: 0,
                invites: 0,
                other: 0,
            });
            match e.action.as_str() {
                "push" => b.pushes += 1,
                a if a.starts_with("role.") || a == "access.revoke" => b.roles += 1,
                a if a.starts_with("invite.") => b.invites += 1,
                _ => b.other += 1,
            }
        }
        activity = buckets.into_values().collect();
    }

    Ok(Json(Stats { teams, activity }))
}

// ---- audit ----------------------------------------------------------------------

#[derive(Deserialize)]
pub struct AuditParams {
    pub team: Option<String>,
    pub limit: Option<usize>,
    /// Substring filter on the action name (e.g. `role`, `push`, `org.settings`).
    pub action: Option<String>,
    /// Substring filter on the actor sub/email.
    pub actor: Option<String>,
}

/// GET /admin/v1/audit — owners (of any team) see everything; admins see entries for teams
/// they administer.
pub async fn get_audit(
    State(state): State<AppState>,
    Extension(id): Extension<Identity>,
    axum::extract::Query(params): axum::extract::Query<AuditParams>,
) -> Result<Json<Vec<AuditEntry>>, ApiError> {
    let org = rbac::org(&state).await?;
    let mine = caller_roles(&state, &id, &org).await?;
    let is_owner = mine.iter().any(|(_, r)| *r == Role::Owner);
    let admin_teams: Vec<&String> =
        mine.iter().filter(|(_, r)| *r >= Role::Admin).map(|(t, _)| t).collect();
    if !is_owner && admin_teams.is_empty() {
        return Err(ApiError::Forbidden);
    }
    let limit = params.limit.unwrap_or(100).clamp(1, 500);

    // action/actor are substring filters, applied within the fetched window.
    let post_filter = |mut entries: Vec<AuditEntry>| {
        if let Some(a) = params.action.as_deref().map(str::to_lowercase).filter(|a| !a.is_empty()) {
            entries.retain(|e| e.action.to_lowercase().contains(&a));
        }
        if let Some(a) = params.actor.as_deref().map(str::to_lowercase).filter(|a| !a.is_empty()) {
            entries.retain(|e| e.actor.to_lowercase().contains(&a));
        }
        entries
    };

    if let Some(team) = &params.team {
        let may = is_owner || admin_teams.contains(&team);
        if !may {
            return Err(ApiError::Forbidden);
        }
        let entries =
            state.storage.audit(Some(team), limit).await.map_err(ApiError::Storage)?;
        return Ok(Json(post_filter(entries)));
    }
    let mut entries = state.storage.audit(None, limit).await.map_err(ApiError::Storage)?;
    if !is_owner {
        // Admins: only their teams' entries (org-wide entries like org.settings are owner-only).
        entries.retain(|e| e.team.as_ref().is_some_and(|t| admin_teams.contains(&t)));
    }
    Ok(Json(post_filter(entries)))
}

/// The bundled single-file admin console. Static, no build step, no external resources.
pub async fn console() -> Html<&'static str> {
    Html(include_str!("admin.html"))
}

// ---- invite tokens ---------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteBody {
    pub sub: String,
    #[serde(default)]
    pub email: Option<String>,
    /// Pinned role for the invited identity (≤ the caller's grant ceiling); default = org default.
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub expires_days: Option<u32>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteResponse {
    /// The plaintext token — returned exactly once, never stored or logged.
    pub token: String,
    pub sub: String,
    pub teams: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

/// The highest role a caller may pin on an invite: manager→writer, admin→manager, owner→admin
/// (nobody mints owner tokens — ownership moves only via the explicit role API).
fn invite_ceiling(caller: Role) -> Role {
    match caller {
        Role::Owner => Role::Admin,
        Role::Admin => Role::Manager,
        _ => Role::Writer,
    }
}

pub async fn create_invite(
    State(state): State<AppState>,
    Extension(id): Extension<Identity>,
    Path(team): Path<String>,
    Json(body): Json<InviteBody>,
) -> Result<Json<InviteResponse>, ApiError> {
    let caller = rbac::ensure_access(&state, &team, &id, Role::Manager).await?;
    let sub = body.sub.trim().to_string();
    if sub.is_empty() || sub.len() > 256 {
        return Err(ApiError::Validation("sub must be 1-256 chars".into()));
    }
    let pinned = match &body.role {
        None => None,
        Some(r) => {
            let Some(role) = crate::storage::role_from_str(r) else {
                return Err(ApiError::Validation(format!("invalid role {r:?}")));
            };
            if role > invite_ceiling(caller) {
                return Err(ApiError::Forbidden);
            }
            Some(role)
        }
    };

    // 32 random bytes from the OS CSPRNG; only the SHA-256 is persisted.
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let token = hex::encode(bytes);
    let sha = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(token.as_bytes()));

    let expires_at = body.expires_days.map(|d| {
        (chrono::Utc::now() + chrono::Duration::days(d.clamp(1, 3650) as i64))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    });
    let stored = crate::storage::StoredToken {
        token_sha256: sha,
        sub: sub.clone(),
        email: body.email.clone(),
        teams: vec![team.clone()],
        role: pinned.map(|r| crate::storage::role_to_str(r).to_string()),
        created_by: id.sub.clone(),
        created_at: crate::storage::now_rfc3339(),
        expires_at: expires_at.clone(),
        revoked_at: None,
    };
    state.storage.store_token(&stored).await.map_err(ApiError::Storage)?;
    rbac::audit_log(
        &state,
        AuditEntry::new(&id.sub, "invite.create").team(&team).target(&sub).detail(format!(
            "role={} expires={}",
            stored.role.as_deref().unwrap_or("default"),
            expires_at.as_deref().unwrap_or("never")
        )),
    )
    .await;
    Ok(Json(InviteResponse {
        token,
        sub,
        teams: vec![team],
        role: stored.role,
        expires_at,
    }))
}

/// Revoke a member's access to the team: their stored tokens lose this team (revoked outright
/// when it was their only team) and their role row is removed. Grant ceilings apply.
pub async fn revoke_access(
    State(state): State<AppState>,
    Extension(id): Extension<Identity>,
    Path((team, sub)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let caller = rbac::ensure_access(&state, &team, &id, Role::Manager).await?;
    let target_role = state
        .storage
        .role(&team, &sub)
        .await
        .map_err(ApiError::Storage)?
        .unwrap_or(Role::Reader);
    if !may_administer(caller, target_role, Role::Reader) {
        return Err(ApiError::Forbidden);
    }
    let mut tokens_touched = 0u32;
    for t in state.storage.tokens_for_sub(&sub).await.map_err(ApiError::Storage)? {
        if !t.teams.iter().any(|x| x == &team) || t.revoked_at.is_some() {
            continue;
        }
        if t.teams.len() == 1 {
            state.storage.revoke_token(&t.token_sha256).await.map_err(ApiError::Storage)?;
        } else {
            let remaining: Vec<String> =
                t.teams.iter().filter(|x| *x != &team).cloned().collect();
            state
                .storage
                .update_token_teams(&t.token_sha256, &remaining)
                .await
                .map_err(ApiError::Storage)?;
        }
        tokens_touched += 1;
    }
    state.storage.remove_role(&team, &sub).await.map_err(ApiError::Storage)?;
    rbac::audit_log(
        &state,
        AuditEntry::new(&id.sub, "access.revoke")
            .team(&team)
            .target(&sub)
            .detail(format!("tokens={tokens_touched}")),
    )
    .await;
    Ok(Json(serde_json::json!({ "revoked": sub, "tokens": tokens_touched })))
}

// ---- restricted groups + grants ----------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupInfo {
    pub id: String,
    pub name: String,
    pub restricted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<String>,
}

/// The team's groups with their restriction state (dashboard's group list).
pub async fn list_groups(
    State(state): State<AppState>,
    Extension(id): Extension<Identity>,
    Path(team): Path<String>,
) -> Result<Json<Vec<GroupInfo>>, ApiError> {
    rbac::ensure_access(&state, &team, &id, Role::Manager).await?;
    let restricted: std::collections::HashSet<String> = state
        .storage
        .restricted_groups(&team)
        .await
        .map_err(ApiError::Storage)?
        .into_iter()
        .collect();
    let mut out: Vec<GroupInfo> = state
        .storage
        .groups(&team)
        .await
        .map_err(ApiError::Storage)?
        .into_iter()
        .filter(|g| g.deleted_at.is_none())
        .map(|g| GroupInfo {
            restricted: restricted.contains(&g.id),
            id: g.id,
            name: g.name,
            deleted_at: g.deleted_at,
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct RestrictedBody {
    pub restricted: bool,
}

pub async fn set_restricted(
    State(state): State<AppState>,
    Extension(id): Extension<Identity>,
    Path((team, group_id)): Path<(String, String)>,
    Json(body): Json<RestrictedBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    rbac::ensure_access(&state, &team, &id, Role::Manager).await?;
    state
        .storage
        .set_group_restricted(&team, &group_id, body.restricted)
        .await
        .map_err(ApiError::Storage)?;
    rbac::audit_log(
        &state,
        AuditEntry::new(&id.sub, "group.restricted")
            .team(&team)
            .target(&group_id)
            .detail(body.restricted.to_string()),
    )
    .await;
    Ok(Json(serde_json::json!({ "id": group_id, "restricted": body.restricted })))
}

#[derive(Serialize, Deserialize)]
pub struct GrantRow {
    pub sub: String,
    pub level: String,
}

pub async fn list_grants(
    State(state): State<AppState>,
    Extension(id): Extension<Identity>,
    Path((team, group_id)): Path<(String, String)>,
) -> Result<Json<Vec<GrantRow>>, ApiError> {
    rbac::ensure_access(&state, &team, &id, Role::Manager).await?;
    let grants = state
        .storage
        .group_grants(&team, &group_id)
        .await
        .map_err(ApiError::Storage)?
        .into_iter()
        .map(|(sub, level)| GrantRow { sub, level })
        .collect();
    Ok(Json(grants))
}

#[derive(Deserialize)]
pub struct GrantBody {
    pub level: String,
}

pub async fn set_grant(
    State(state): State<AppState>,
    Extension(id): Extension<Identity>,
    Path((team, group_id, sub)): Path<(String, String, String)>,
    Json(body): Json<GrantBody>,
) -> Result<Json<GrantRow>, ApiError> {
    rbac::ensure_access(&state, &team, &id, Role::Manager).await?;
    if !matches!(body.level.as_str(), "read" | "write") {
        return Err(ApiError::Validation("level must be read|write".into()));
    }
    state
        .storage
        .set_group_grant(&team, &group_id, &sub, &body.level)
        .await
        .map_err(ApiError::Storage)?;
    rbac::audit_log(
        &state,
        AuditEntry::new(&id.sub, "group.grant")
            .team(&team)
            .target(&sub)
            .detail(format!("group={group_id} level={}", body.level)),
    )
    .await;
    Ok(Json(GrantRow { sub, level: body.level }))
}

pub async fn remove_grant(
    State(state): State<AppState>,
    Extension(id): Extension<Identity>,
    Path((team, group_id, sub)): Path<(String, String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    rbac::ensure_access(&state, &team, &id, Role::Manager).await?;
    state
        .storage
        .remove_group_grant(&team, &group_id, &sub)
        .await
        .map_err(ApiError::Storage)?;
    rbac::audit_log(
        &state,
        AuditEntry::new(&id.sub, "group.ungrant")
            .team(&team)
            .target(&sub)
            .detail(format!("group={group_id}")),
    )
    .await;
    Ok(Json(serde_json::json!({ "removed": sub })))
}
