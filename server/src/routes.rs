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
        check_short(&s.updated_at, "updatedAt")?;
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
