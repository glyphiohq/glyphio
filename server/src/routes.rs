// SPDX-License-Identifier: Apache-2.0
//! Protocol handlers. Authorization (team membership) and validation happen here — storage
//! only ever sees vetted, owner-stamped batches.

use std::collections::{BTreeMap, HashMap};

use axum::extract::{Path, Query, State};
use axum::{Extension, Json};
use serde::Deserialize;
use sync_proto::{
    limits, Changes, Me, Members, OutcomeStatus, Push, PushAck, PushOutcome, Role, SnippetRec,
};

use crate::auth::Identity;
use crate::error::ApiError;
use crate::rbac;
use crate::AppState;

/// Identity, effective teams (claim ∪ explicit role rows), and the resolved role per team.
/// No bootstrap side effects — that only happens on team-scoped requests.
pub async fn me(
    State(state): State<AppState>,
    Extension(id): Extension<Identity>,
) -> Result<Json<Me>, ApiError> {
    let mut teams = id.teams.clone();
    for (team, _) in state.storage.roles_for_sub(&id.sub).await.map_err(ApiError::Storage)? {
        if !teams.contains(&team) {
            teams.push(team);
        }
    }
    teams.sort();
    let org = rbac::org(&state).await?;
    let mut roles = HashMap::new();
    for team in &teams {
        if let Some(role) = rbac::resolve_role_with_org(&state, &org, team, &id).await? {
            roles.insert(team.clone(), role);
        }
    }
    let policy = Some(sync_proto::OrgPolicy { export_team_groups: org.export_team_groups.clone() });
    Ok(Json(Me { sub: id.sub.clone(), email: id.email.clone(), teams, roles, policy }))
}

/// Archived teams keep their data but refuse sync (403 + "team archived").
async fn reject_archived(state: &AppState, team: &str) -> Result<(), ApiError> {
    if state.storage.archived(team).await.map_err(ApiError::Storage)? {
        return Err(ApiError::Archived);
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct ChangesParams {
    #[serde(default)]
    since: u64,
    limit: Option<usize>,
}

/// Seen-member tracking: every successful team-scoped request refreshes (team, sub,
/// last_seen). Non-fatal — a bookkeeping failure must never break sync itself.
async fn note_seen(state: &AppState, team: &str, id: &Identity) {
    match state.storage.record_seen(team, &id.sub, id.email.as_deref()).await {
        Ok(true) => {
            rbac::audit_log(state, crate::storage::AuditEntry::new(&id.sub, "member.first_seen").team(team))
                .await;
        }
        Ok(false) => {}
        Err(e) => tracing::warn!("seen-member upsert failed: {e}"),
    }
}

pub async fn changes(
    State(state): State<AppState>,
    Extension(id): Extension<Identity>,
    Path(team): Path<String>,
    Query(params): Query<ChangesParams>,
) -> Result<Json<Changes>, ApiError> {
    let role = rbac::ensure_access(&state, &team, &id, Role::Reader).await?;
    reject_archived(&state, &team).await?;
    note_seen(&state, &team, &id).await;
    let limit = params.limit.unwrap_or(limits::DEFAULT_PAGE).clamp(1, limits::MAX_PAGE);
    let mut changes = state
        .storage
        .changes(&team, params.since, limit)
        .await
        .map_err(ApiError::Storage)?;

    // Restricted-group filtering at serialization time: the cursor advanced globally above;
    // what each identity SEES is decided here. Managers+ see everything; others need a grant.
    let restricted = state.storage.restricted_groups(&team).await.map_err(ApiError::Storage)?;
    if !restricted.is_empty() {
        let restricted: std::collections::HashSet<String> = restricted.into_iter().collect();
        let is_manager = role >= Role::Manager;
        let grants = if is_manager {
            std::collections::HashMap::new()
        } else {
            state.storage.grants_for_sub(&team, &id.sub).await.map_err(ApiError::Storage)?
        };
        let visible =
            |gid: &str| !restricted.contains(gid) || is_manager || grants.contains_key(gid);
        changes.groups.retain(|g| visible(&g.id));
        changes.snippets.retain(|s| s.group_id.as_deref().is_none_or(visible));
        // The server is the sole authority for the outgoing `restricted` flag.
        for g in &mut changes.groups {
            g.restricted = restricted.contains(&g.id);
        }
    }
    Ok(Json(changes))
}

pub async fn push(
    State(state): State<AppState>,
    Extension(id): Extension<Identity>,
    Path(team): Path<String>,
    Json(mut batch): Json<Push>,
) -> Result<Json<PushAck>, ApiError> {
    // Readers get a batch-level 403; writers proceed to per-record ownership checks below.
    let role = rbac::ensure_access(&state, &team, &id, Role::Writer).await?;
    reject_archived(&state, &team).await?;
    note_seen(&state, &team, &id).await;
    validate(&team, &batch)?;

    // Restricted groups: pushing a snippet into (or the group record of) a restricted group
    // requires a WRITE grant or manager+. Deliberately a generic batch-level 403 — a specific
    // error would confirm the group's existence to someone who can't see it.
    let restricted: std::collections::HashSet<String> = state
        .storage
        .restricted_groups(&team)
        .await
        .map_err(ApiError::Storage)?
        .into_iter()
        .collect();
    if !restricted.is_empty() && role < Role::Manager {
        let grants =
            state.storage.grants_for_sub(&team, &id.sub).await.map_err(ApiError::Storage)?;
        let may_write =
            |gid: &str| !restricted.contains(gid) || grants.get(gid).map(String::as_str) == Some("write");
        let blocked = batch
            .snippets
            .iter()
            .any(|s| s.group_id.as_deref().is_some_and(|g| !may_write(g)))
            || batch.groups.iter().any(|g| !may_write(&g.id));
        if blocked {
            return Err(ApiError::Denied);
        }
    }
    // Only the server sets `restricted` on stored/outgoing group records.
    for g in &mut batch.groups {
        g.restricted = false;
    }

    // The server, not the client, decides attribution: a NEW record's `owner` is the
    // authenticated pusher; an EXISTING record keeps its original author (so `owner` stays
    // meaningful for the ownership checks even after manager edits).
    let ids: Vec<String> = batch.snippets.iter().map(|s| s.id.clone()).collect();
    let existing = state.storage.snippets_by_ids(&team, &ids).await.map_err(ApiError::Storage)?;
    let by_id: HashMap<&str, &SnippetRec> = existing.iter().map(|s| (s.id.as_str(), s)).collect();

    // Capability matrix: writers may create records and edit their own; editing or
    // tombstoning someone else's record needs manager+. A writer's disallowed record is
    // reported as `superseded` with the server copy (the protocol's reconcile path — the
    // client applies the authoritative record; sync-proto has no per-record "forbidden").
    let mut rejected: Vec<PushOutcome<SnippetRec>> = Vec::new();
    let mut kept = Vec::with_capacity(batch.snippets.len());
    for mut s in batch.snippets {
        match by_id.get(s.id.as_str()) {
            Some(server) => {
                if server.owner != id.sub && role < Role::Manager {
                    rejected.push(PushOutcome {
                        id: s.id.clone(),
                        status: OutcomeStatus::Superseded,
                        server_record: Some((*server).clone()),
                    });
                    continue;
                }
                s.owner = server.owner.clone();
            }
            None => s.owner = id.sub.clone(),
        }
        kept.push(s);
    }
    batch.snippets = kept;

    let mut ack = state.storage.merge(&team, &batch).await.map_err(ApiError::Storage)?;
    ack.snippets.extend(rejected);
    // Audit counts only — snippet content must never reach the log.
    rbac::audit_log(
        &state,
        crate::storage::AuditEntry::new(&id.sub, "push").team(&team).detail(format!(
            "snippets={} groups={} rejected={}",
            batch.snippets.len(),
            batch.groups.len(),
            ack.snippets.iter().filter(|o| o.status == OutcomeStatus::Superseded).count()
        )),
    )
    .await;
    Ok(Json(ack))
}

/// Team roster = configured static-token members ∪ identities seen authenticating, deduped
/// by `sub` (seen data wins — it carries `last_seen` and a fresher email), sorted by `sub`.
pub async fn members(
    State(state): State<AppState>,
    Extension(id): Extension<Identity>,
    Path(team): Path<String>,
) -> Result<Json<Members>, ApiError> {
    rbac::ensure_access(&state, &team, &id, Role::Reader).await?;
    reject_archived(&state, &team).await?;
    note_seen(&state, &team, &id).await;
    let mut merged: BTreeMap<String, sync_proto::Member> = BTreeMap::new();
    for m in state.auth.static_members(&team) {
        merged.insert(m.sub.clone(), m);
    }
    for seen in state.storage.members(&team).await.map_err(ApiError::Storage)? {
        merged
            .entry(seen.sub.clone())
            .and_modify(|m| {
                m.last_seen = seen.last_seen.clone();
                if seen.email.is_some() {
                    m.email = seen.email.clone();
                }
            })
            .or_insert(seen);
    }
    Ok(Json(Members { members: merged.into_values().collect() }))
}

// ---- self-service membership -------------------------------------------------------
// An identity belongs to as many teams as it has access to; `/v1/me` unions the IdP claim
// with explicit role rows. These two endpoints let a *member* add and drop rows for
// themselves — joining by redeeming an invite, and leaving on their own — so multi-team
// membership doesn't require an admin round trip for every change.

#[derive(Deserialize)]
pub struct RedeemBody {
    /// The invite's plaintext token, as minted by `POST /admin/v1/teams/{team}/invites`.
    pub code: String,
}

/// Redeem an invite for the *caller's* identity, granting the invite's team(s) on top of
/// whatever they already have. Authenticated with the caller's existing credential, so a
/// second, third… team never displaces the first.
///
/// The invite is consumed (revoked) on success: it was minted for one person to join once,
/// and leaving it live would be a second standing credential for the team.
pub async fn redeem_invite(
    State(state): State<AppState>,
    Extension(id): Extension<Identity>,
    Json(body): Json<RedeemBody>,
) -> Result<Json<sync_proto::Me>, ApiError> {
    use sha2::Digest;
    let code = body.code.trim();
    if code.is_empty() || code.len() > 512 {
        return Err(ApiError::Validation("invite code missing or malformed".into()));
    }
    let sha = hex::encode(sha2::Sha256::digest(code.as_bytes()));
    let invite = state
        .storage
        .token_by_sha(&sha)
        .await
        .map_err(ApiError::Storage)?
        // One message for "no such invite" and "expired/revoked" alike: a distinguishable
        // error would turn this endpoint into an oracle for guessing invite codes.
        .filter(|t| t.revoked_at.is_none())
        .filter(|t| t.expires_at.as_deref().is_none_or(|e| e > crate::storage::now_rfc3339().as_str()))
        .ok_or_else(|| ApiError::Validation("this invite is not valid — it may have expired or already been used".into()))?;

    let org = rbac::org(&state).await?;
    let granted = invite.role.as_deref().and_then(crate::storage::role_from_str);
    let mut joined = Vec::new();
    for team in &invite.teams {
        if state.storage.archived(team).await.map_err(ApiError::Storage)? {
            continue;
        }
        let role = granted.unwrap_or(rbac::effective_default(&org, state.default_role));
        // Never demote: redeeming an invite can only ever add access. Someone already an
        // owner who redeems a reader invite stays an owner.
        let effective = match state.storage.role(team, &id.sub).await.map_err(ApiError::Storage)? {
            Some(existing) if existing >= role => existing,
            _ => {
                state.storage.set_role(team, &id.sub, role).await.map_err(ApiError::Storage)?;
                role
            }
        };
        joined.push(team.clone());
        rbac::audit_log(
            &state,
            crate::storage::AuditEntry::new(&id.sub, "invite.redeem")
                .team(team)
                .target(&id.sub)
                .detail(format!(
                    "role={} invited={}",
                    crate::storage::role_to_str(effective),
                    invite.sub
                )),
        )
        .await;
    }
    if joined.is_empty() {
        return Err(ApiError::Validation("this invite's team is no longer available".into()));
    }
    // Consume it — unless the caller is *authenticated with this very invite*, in which case
    // revoking would log them out of the session that just used it.
    if id.token_sha.as_deref() != Some(sha.as_str()) {
        state.storage.revoke_token(&sha).await.map_err(ApiError::Storage)?;
    }
    me(State(state), Extension(id)).await
}

/// Leave a team: drop the caller's own access. Refuses to strand a team without an owner,
/// and can't undo membership that comes from the identity provider — the IdP is the
/// authority there, so the honest answer is to say so rather than fail silently.
pub async fn leave_team(
    State(state): State<AppState>,
    Extension(id): Extension<Identity>,
    Path(team): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let org = rbac::org(&state).await?;
    let Some(role) = rbac::resolve_role_with_org(&state, &org, &team, &id).await? else {
        return Err(ApiError::Forbidden);
    };
    if role == Role::Owner {
        let owners = state
            .storage
            .roles(&team)
            .await
            .map_err(ApiError::Storage)?
            .into_iter()
            .filter(|(_, r)| *r == Role::Owner)
            .count();
        if owners <= 1 {
            return Err(ApiError::Validation(
                "you are this team's only owner — make someone else an owner first".into(),
            ));
        }
    }
    state.storage.remove_role(&team, &id.sub).await.map_err(ApiError::Storage)?;
    // Drop the team from any invite token of the caller's, so a stored credential can't keep
    // the membership alive behind the removed role row.
    let mut tokens_touched = 0u32;
    for t in state.storage.tokens_for_sub(&id.sub).await.map_err(ApiError::Storage)? {
        if t.revoked_at.is_some() || !t.teams.iter().any(|x| x == &team) {
            continue;
        }
        let remaining: Vec<String> = t.teams.iter().filter(|x| *x != &team).cloned().collect();
        if remaining.is_empty() {
            state.storage.revoke_token(&t.token_sha256).await.map_err(ApiError::Storage)?;
        } else {
            state
                .storage
                .update_token_teams(&t.token_sha256, &remaining)
                .await
                .map_err(ApiError::Storage)?;
        }
        tokens_touched += 1;
    }
    rbac::audit_log(
        &state,
        crate::storage::AuditEntry::new(&id.sub, "team.leave")
            .team(&team)
            .target(&id.sub)
            .detail(format!("tokens={tokens_touched}")),
    )
    .await;
    // Claim-granted membership survives this: say so plainly instead of pretending it worked.
    let still_a_member = id.teams.iter().any(|t| t == &team);
    Ok(Json(serde_json::json!({
        "left": team,
        "stillGrantedByIdentityProvider": still_a_member,
    })))
}

fn validate(team: &str, batch: &Push) -> Result<(), ApiError> {
    let total = batch.snippets.len() + batch.groups.len();
    if total == 0 {
        return Err(ApiError::Validation("empty batch".into()));
    }
    if total > limits::MAX_BATCH {
        return Err(ApiError::Validation(format!(
            "batch of {total} exceeds the {} record limit",
            limits::MAX_BATCH
        )));
    }
    for s in &batch.snippets {
        if s.team != team {
            return Err(ApiError::Validation(format!(
                "snippet {} team {:?} does not match path team {team:?}",
                s.id, s.team
            )));
        }
        check_short(&s.id, "id")?;
        check_short(&s.trigger, "trigger")?;
        check_short(&s.format, "format")?;
        check_short(&s.kind, "kind")?;
        check_short(&s.updated_at, "updatedAt")?;
        // Executable content never syncs: a shell command distributed to a team would be
        // remote code execution on every member's machine. Reject, don't sanitize — the
        // pusher must know their record was refused.
        if s.kind == "command" || sync_proto::has_exec_vars(&s.variables) {
            return Err(ApiError::Validation(format!(
                "snippet {} carries executable content (command kind or shell/script \
                 variables) — these are local-only and never sync",
                s.id
            )));
        }
        if s.replacement.len() > limits::MAX_REPLACEMENT {
            return Err(ApiError::Validation(format!("snippet {} replacement too large", s.id)));
        }
        if let Some(v) = &s.variables {
            let size = serde_json::to_string(v).map(|j| j.len()).unwrap_or(usize::MAX);
            if size > limits::MAX_VARIABLES {
                return Err(ApiError::Validation(format!("snippet {} variables too large", s.id)));
            }
        }
    }
    for g in &batch.groups {
        if g.team != team {
            return Err(ApiError::Validation(format!(
                "group {} team {:?} does not match path team {team:?}",
                g.id, g.team
            )));
        }
        check_short(&g.id, "id")?;
        check_short(&g.name, "name")?;
        check_short(&g.updated_at, "updatedAt")?;
    }
    Ok(())
}

fn check_short(value: &str, field: &str) -> Result<(), ApiError> {
    if value.is_empty() {
        return Err(ApiError::Validation(format!("{field} must not be empty")));
    }
    if value.len() > limits::MAX_SHORT_STRING {
        return Err(ApiError::Validation(format!("{field} exceeds {} bytes", limits::MAX_SHORT_STRING)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(kind: &str, variables: Option<serde_json::Value>) -> SnippetRec {
        SnippetRec {
            id: "s1".into(),
            trigger: ":t".into(),
            replacement: "body".into(),
            format: "plain".into(),
            kind: kind.into(),
            variables,
            group_id: None,
            app_scope: None,
            owner: "u1".into(),
            team: "sec".into(),
            updated_at: "2026-07-07T00:00:00.000Z".into(),
            version: 1,
            deleted_at: None,
        }
    }

    fn push_of(s: SnippetRec) -> Push {
        Push { snippets: vec![s], groups: vec![] }
    }

    #[test]
    fn validate_rejects_executable_content() {
        // Command kind: refused outright.
        assert!(validate("sec", &push_of(rec("command", None))).is_err());
        // shell / script variables on any kind: refused.
        for var_type in ["shell", "script"] {
            let vars = serde_json::json!([{ "name": "x", "type": var_type, "params": {} }]);
            assert!(validate("sec", &push_of(rec("text", Some(vars)))).is_err());
        }
        // Benign variables and kinds pass.
        let date = serde_json::json!([{ "name": "d", "type": "date", "params": {} }]);
        assert!(validate("sec", &push_of(rec("text", Some(date)))).is_ok());
        assert!(validate("sec", &push_of(rec("form", None))).is_ok());
        assert!(validate("sec", &push_of(rec("popup", None))).is_ok());
    }
}
