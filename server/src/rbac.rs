// SPDX-License-Identifier: Apache-2.0
//! Role resolution + access checks (capability matrix in the app repo's PHASE3 plan).
//!
//! Resolution precedence, per identity and team:
//! 1. an explicit role row (`roles` storage) always wins;
//! 2. else, if the team is in the identity's claim/config, the token-pinned role (static-token
//!    `"role"` field) or the server-wide `DEFAULT_ROLE` (default `writer`);
//! 3. else no access.
//!
//! **Bootstrap**: the first identity to successfully touch a team while it has no owner row is
//! auto-granted `owner` (recorded as a real role row). This only happens on team-scoped
//! requests — `/v1/me` reads roles without side effects.

use sync_proto::Role;

use crate::auth::Identity;
use crate::error::ApiError;
use crate::storage::{AuditEntry, OrgSettings};
use crate::AppState;

/// Current org settings (defaults when none were ever saved).
pub async fn org(state: &AppState) -> Result<OrgSettings, ApiError> {
    Ok(state.storage.org_settings().await.map_err(ApiError::Storage)?.unwrap_or_default())
}

/// Org default role, falling back to the DEFAULT_ROLE env value.
pub fn effective_default(org: &OrgSettings, env_fallback: Role) -> Role {
    org.default_role
        .as_deref()
        .and_then(crate::storage::role_from_str)
        .unwrap_or(env_fallback)
}

/// Append an audit entry (best-effort — auditing must never fail the request). Retention
/// comes from org settings.
pub async fn audit_log(state: &AppState, entry: AuditEntry) {
    let retention = match state.storage.org_settings().await {
        Ok(s) => s.unwrap_or_default().audit_retention_days,
        Err(_) => OrgSettings::default().audit_retention_days,
    };
    if let Err(e) = state.storage.audit_append(&entry, retention).await {
        tracing::warn!("audit append failed: {e}");
    }
}

/// Side-effect-free resolution with org settings in hand (fetch once for N teams).
pub async fn resolve_role_with_org(
    state: &AppState,
    org: &OrgSettings,
    team: &str,
    id: &Identity,
) -> Result<Option<Role>, ApiError> {
    if let Some(row) = state.storage.role(team, &id.sub).await.map_err(ApiError::Storage)? {
        return Ok(Some(row));
    }
    if id.teams.iter().any(|t| t == team) {
        return Ok(Some(id.pinned_role.unwrap_or(effective_default(org, state.default_role))));
    }
    Ok(None)
}

/// Resolve + bootstrap + minimum-role gate for team-scoped endpoints. Returns the effective
/// role so handlers can make finer-grained per-record decisions.
pub async fn ensure_access(
    state: &AppState,
    team: &str,
    id: &Identity,
    min: Role,
) -> Result<Role, ApiError> {
    let org = org(state).await?;
    let Some(mut role) = resolve_role_with_org(state, &org, team, id).await? else {
        return Err(ApiError::Forbidden);
    };
    // Bootstrap (compat mode only): a team with no owner row crowns its first successful
    // toucher — and ONLY an identity that carries the team in its IdP claim / token config.
    // In "owners"/"admins" team-creation modes, ownership comes exclusively from explicit
    // team creation (POST /admin/v1/teams).
    if org.team_creation == "bootstrap" && id.teams.iter().any(|t| t == team) {
        let has_owner = state
            .storage
            .roles(team)
            .await
            .map_err(ApiError::Storage)?
            .iter()
            .any(|(_, r)| *r == Role::Owner);
        if !has_owner {
            state
                .storage
                .set_role(team, &id.sub, Role::Owner)
                .await
                .map_err(ApiError::Storage)?;
            tracing::info!("bootstrap: granted owner of team {team:?} to first toucher");
            audit_log(state, AuditEntry::new(&id.sub, "role.bootstrap_owner").team(team)).await;
            role = Role::Owner;
        }
    }
    if role < min {
        return Err(ApiError::Forbidden);
    }
    Ok(role)
}
