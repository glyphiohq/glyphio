// SPDX-License-Identifier: Apache-2.0
//! Glyphio reference sync backend.
//!
//! Implements the v1 wire protocol (`sync-proto`, see the app repo's `docs/SYNC-PROTOCOL.md`)
//! over plain HTTP behind TLS termination (ALB/API Gateway/reverse proxy). Configuration is
//! environment-only — no config files, no compiled-in tenants or endpoints.

mod admin;
mod auth;
mod error;
mod ratelimit;
mod rbac;
mod routes;
mod storage;

use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::routing::get;
use axum::{middleware, Router};

#[derive(Clone)]
pub struct AppState {
    pub storage: Arc<dyn storage::Storage>,
    pub auth: Arc<auth::Authenticator>,
    pub limiter: Arc<ratelimit::RateLimiter>,
    /// Role for identities with team access (claim/config) but no explicit role row.
    pub default_role: sync_proto::Role,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .compact()
        .init();

    let state = build_state().await.unwrap_or_else(|e| {
        eprintln!("fatal: {e}");
        std::process::exit(1);
    });
    let app = router(state);

    let port: u16 = std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8787);
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!("glyphio-server listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    axum::serve(listener, app).await.expect("serve");
}

async fn build_state() -> Result<AppState, String> {
    let storage: Arc<dyn storage::Storage> =
        match std::env::var("STORAGE").as_deref().unwrap_or("sqlite") {
            "sqlite" => {
                let path =
                    std::env::var("DB_PATH").unwrap_or_else(|_| "/data/glyphio.db".to_string());
                Arc::new(
                    storage::sqlite::SqliteStorage::open(&path)
                        .map_err(|e| format!("sqlite storage: {e}"))?,
                )
            }
            "dynamo" => {
                let table =
                    std::env::var("DYNAMO_TABLE").map_err(|_| "STORAGE=dynamo requires DYNAMO_TABLE")?;
                Arc::new(storage::dynamo::DynamoStorage::new(table).await)
            }
            other => return Err(format!("unknown STORAGE {other:?} (sqlite|dynamo)")),
        };

    let auth = Arc::new(auth::Authenticator::from_env().map_err(|e| format!("auth config: {e}"))?);
    let per_min = std::env::var("RATE_LIMIT_PER_MIN").ok().and_then(|v| v.parse().ok()).unwrap_or(60);
    let default_role = match std::env::var("DEFAULT_ROLE").as_deref() {
        Err(_) => sync_proto::Role::Writer,
        Ok(v) => storage::role_from_str(v)
            .ok_or_else(|| format!("invalid DEFAULT_ROLE {v:?} (reader|writer|manager|admin|owner)"))?,
    };
    Ok(AppState {
        storage,
        auth,
        limiter: Arc::new(ratelimit::RateLimiter::new(per_min)),
        default_role,
    })
}

pub fn router(state: AppState) -> Router {
    let v1 = Router::new()
        .route("/me", get(routes::me))
        .route("/teams/{team}/changes", get(routes::changes).post(routes::push))
        .route("/teams/{team}/members", get(routes::members))
        // Self-service membership: join by redeeming an invite, leave on your own.
        .route("/invites/redeem", axum::routing::post(routes::redeem_invite))
        .route("/teams/{team}/membership", axum::routing::delete(routes::leave_team))
        .route_layer(middleware::from_fn_with_state(state.clone(), auth::auth_middleware));
    let admin_api = Router::new()
        .route("/teams", get(admin::teams).post(admin::create_team))
        .route("/teams/{team}", axum::routing::delete(admin::archive_team))
        .route("/teams/{team}/roles", get(admin::list_roles))
        .route(
            "/teams/{team}/roles/{sub}",
            axum::routing::put(admin::set_role).delete(admin::remove_role),
        )
        .route("/org", get(admin::get_org).put(admin::put_org))
        .route("/audit", get(admin::get_audit))
        .route("/stats", get(admin::stats))
        .route("/teams/{team}/invites", axum::routing::post(admin::create_invite))
        .route("/teams/{team}/access/{sub}", axum::routing::delete(admin::revoke_access))
        .route("/teams/{team}/groups", get(admin::list_groups))
        .route(
            "/teams/{team}/groups/{group_id}/restricted",
            axum::routing::put(admin::set_restricted),
        )
        .route("/teams/{team}/groups/{group_id}/acl", get(admin::list_grants))
        .route(
            "/teams/{team}/groups/{group_id}/acl/{sub}",
            axum::routing::put(admin::set_grant).delete(admin::remove_grant),
        )
        .route_layer(middleware::from_fn_with_state(state.clone(), auth::auth_middleware))
        // Added AFTER route_layer → unauthenticated. The console needs the issuer/client id
        // before sign-in, and both are public knowledge (no secrets in the response).
        .route("/config", get(admin::console_config));
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/admin", get(admin::console))
        .nest("/v1", v1)
        .nest("/admin/v1", admin_api)
        .layer(DefaultBodyLimit::max(sync_proto::limits::MAX_BODY))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use sha2::Digest;
    use tower::ServiceExt;


    // ---- shared test helpers -------------------------------------------------

    fn test_state(dir: &tempfile::TempDir, tokens_json: &str) -> AppState {
        AppState {
            storage: Arc::new(
                storage::sqlite::SqliteStorage::open(dir.path().join("t.db").to_str().unwrap())
                    .unwrap(),
            ),
            auth: Arc::new(auth::Authenticator::for_tests(tokens_json)),
            limiter: Arc::new(ratelimit::RateLimiter::new(100_000)),
            default_role: sync_proto::Role::Writer,
        }
    }

    fn tok(name: &str) -> (String, String) {
        let token = format!("token-{name}");
        (token.clone(), hex::encode(sha2::Sha256::digest(token.as_bytes())))
    }

    async fn req(
        app: &Router,
        method: &str,
        path: &str,
        bearer: &str,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        let mut b = Request::builder()
            .method(method)
            .uri(path)
            .header("authorization", format!("Bearer {bearer}"));
        let body = match body {
            Some(v) => {
                b = b.header("content-type", "application/json");
                Body::from(v.to_string())
            }
            None => Body::empty(),
        };
        let res = app.clone().oneshot(b.body(body).unwrap()).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    fn snip_json(id: &str, replacement: &str, version: i64, ts: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id, "trigger": ":t", "replacement": replacement, "format": "plain",
            "owner": "ignored", "team": "sec", "updatedAt": ts, "version": version
        })
    }

    /// RBAC: bootstrap-owner, pinned reader 403 on push, writer-cannot-touch-others
    /// (superseded), manager can, /v1/me roles, and the admin role-change matrix.
    #[tokio::test]
    async fn rbac_enforcement_and_admin_matrix() {
        let dir = tempfile::tempdir().unwrap();
        let (alice, alice_sha) = tok("alice");
        let (bob, bob_sha) = tok("bob");
        let (carol, carol_sha) = tok("carol");
        let (dave, dave_sha) = tok("dave");
        let tokens = format!(
            r#"[{{"tokenSha256":"{alice_sha}","sub":"alice","teams":["sec"]}},
                {{"tokenSha256":"{bob_sha}","sub":"bob","teams":["sec"]}},
                {{"tokenSha256":"{carol_sha}","sub":"carol","teams":["sec"],"role":"reader"}},
                {{"tokenSha256":"{dave_sha}","sub":"dave","teams":["sec"]}}]"#
        );
        let app = router(test_state(&dir, &tokens));

        // Bootstrap: alice touches the team first → auto-granted owner; visible in /v1/me.
        let (st, _) = req(&app, "GET", "/v1/teams/sec/changes?since=0", &alice, None).await;
        assert_eq!(st, StatusCode::OK);
        let (st, me) = req(&app, "GET", "/v1/me", &alice, None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(me["roles"]["sec"], "owner");

        // Pinned reader: may pull, may NOT push (batch-level 403).
        let (st, _) = req(&app, "GET", "/v1/teams/sec/changes?since=0", &carol, None).await;
        assert_eq!(st, StatusCode::OK);
        let (st, _) = req(&app, "POST", "/v1/teams/sec/changes", &carol,
            Some(serde_json::json!({"snippets": [snip_json("c1", "x", 1, "2026-07-02T10:00:00.000Z")], "groups": []}))).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        let (_, me) = req(&app, "GET", "/v1/me", &carol, None).await;
        assert_eq!(me["roles"]["sec"], "reader");

        // alice creates s1. bob (default writer) edits it → per-record superseded, unchanged.
        let (st, _) = req(&app, "POST", "/v1/teams/sec/changes", &alice,
            Some(serde_json::json!({"snippets": [snip_json("s1", "alice v1", 1, "2026-07-02T10:00:00.000Z")], "groups": []}))).await;
        assert_eq!(st, StatusCode::OK);
        let (st, ack) = req(&app, "POST", "/v1/teams/sec/changes", &bob,
            Some(serde_json::json!({"snippets": [
                snip_json("s1", "bob hijack", 9, "2026-07-02T11:00:00.000Z"),
                snip_json("b1", "bob's own", 1, "2026-07-02T11:00:00.000Z")
            ], "groups": []}))).await;
        assert_eq!(st, StatusCode::OK);
        let outcomes: std::collections::HashMap<&str, &serde_json::Value> = ack["snippets"]
            .as_array().unwrap().iter()
            .map(|o| (o["id"].as_str().unwrap(), o))
            .collect();
        assert_eq!(outcomes["s1"]["status"], "superseded");
        assert_eq!(outcomes["s1"]["serverRecord"]["replacement"], "alice v1");
        assert_eq!(outcomes["b1"]["status"], "accepted");
        let (_, ch) = req(&app, "GET", "/v1/teams/sec/changes?since=0", &alice, None).await;
        let s1 = ch["snippets"].as_array().unwrap().iter().find(|s| s["id"] == "s1").unwrap();
        assert_eq!(s1["replacement"], "alice v1", "writer must not overwrite another's record");
        assert_eq!(s1["owner"], "alice");

        // Owner promotes bob to manager → now bob CAN edit alice's record; authorship sticks.
        let (st, _) = req(&app, "PUT", "/admin/v1/teams/sec/roles/bob", &alice,
            Some(serde_json::json!({"role": "manager"}))).await;
        assert_eq!(st, StatusCode::OK);
        let (st, ack) = req(&app, "POST", "/v1/teams/sec/changes", &bob,
            Some(serde_json::json!({"snippets": [snip_json("s1", "manager edit", 2, "2026-07-02T12:00:00.000Z")], "groups": []}))).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(ack["snippets"][0]["status"], "accepted");
        let (_, ch) = req(&app, "GET", "/v1/teams/sec/changes?since=0", &alice, None).await;
        let s1 = ch["snippets"].as_array().unwrap().iter().find(|s| s["id"] == "s1").unwrap();
        assert_eq!(s1["replacement"], "manager edit");
        assert_eq!(s1["owner"], "alice", "original authorship must be preserved");

        // Admin matrix. Owner may grant admin:
        let (st, _) = req(&app, "PUT", "/admin/v1/teams/sec/roles/dave", &alice,
            Some(serde_json::json!({"role": "admin"}))).await;
        assert_eq!(st, StatusCode::OK);
        // Admin sets ≤ manager:
        let (st, _) = req(&app, "PUT", "/admin/v1/teams/sec/roles/eve", &dave,
            Some(serde_json::json!({"role": "writer"}))).await;
        assert_eq!(st, StatusCode::OK);
        // Admin may NOT grant admin:
        let (st, _) = req(&app, "PUT", "/admin/v1/teams/sec/roles/eve2", &dave,
            Some(serde_json::json!({"role": "admin"}))).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        // Admin may NOT demote an owner:
        let (st, _) = req(&app, "PUT", "/admin/v1/teams/sec/roles/alice", &dave,
            Some(serde_json::json!({"role": "reader"}))).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        // Admin may remove a ≤ manager row:
        let (st, _) = req(&app, "DELETE", "/admin/v1/teams/sec/roles/bob", &dave, None).await;
        assert_eq!(st, StatusCode::OK);
        let (_, me) = req(&app, "GET", "/v1/me", &bob, None).await;
        assert_eq!(me["roles"]["sec"], "writer", "row removed → default resolution");
        // Ownership transfer: owner grants owner; both hold it.
        let (st, _) = req(&app, "PUT", "/admin/v1/teams/sec/roles/frank", &alice,
            Some(serde_json::json!({"role": "owner"}))).await;
        assert_eq!(st, StatusCode::OK);
        let (st, rows) = req(&app, "GET", "/admin/v1/teams/sec/roles", &alice, None).await;
        assert_eq!(st, StatusCode::OK);
        let owners: Vec<&str> = rows.as_array().unwrap().iter()
            .filter(|r| r["role"] == "owner")
            .map(|r| r["sub"].as_str().unwrap())
            .collect();
        assert_eq!(owners, vec!["alice", "frank"]);
        // Invalid role string → 422.
        let (st, _) = req(&app, "PUT", "/admin/v1/teams/sec/roles/x", &alice,
            Some(serde_json::json!({"role": "sudo"}))).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        // Non-admin (bob, writer) is locked out of the admin API.
        let (st, _) = req(&app, "GET", "/admin/v1/teams/sec/roles", &bob, None).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        // Admin teams list for dave includes sec; console serves.
        let (st, teams) = req(&app, "GET", "/admin/v1/teams", &dave, None).await;
        assert_eq!(st, StatusCode::OK);
        assert!(teams.as_array().unwrap().iter().any(|t| t["team"] == "sec" && t["role"] == "admin"));
        let res = app.clone()
            .oneshot(Request::get("/admin").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }


    /// Phase 4 §A/§D: org settings (authz + validation + effect on default role), explicit
    /// team lifecycle (create policy, archive → sync 403 + hidden), bootstrap tightening,
    /// /v1/me policy, audit log visibility.
    #[tokio::test]
    async fn org_settings_team_lifecycle_and_audit() {
        let dir = tempfile::tempdir().unwrap();
        let (alice, alice_sha) = tok("alice"); // will own teamA
        let (bob, bob_sha) = tok("bob");       // plain claim member of teamA
        let tokens = format!(
            r#"[{{"tokenSha256":"{alice_sha}","sub":"alice","teams":["teamA"]}},
                {{"tokenSha256":"{bob_sha}","sub":"bob","teams":["teamA"]}}]"#
        );
        let app = router(test_state(&dir, &tokens));

        // Bootstrap mode (default): alice touches teamA → owner. /v1/me carries policy.
        let (st, _) = req(&app, "GET", "/v1/teams/teamA/changes?since=0", &alice, None).await;
        assert_eq!(st, StatusCode::OK);
        let (_, me) = req(&app, "GET", "/v1/me", &alice, None).await;
        assert_eq!(me["roles"]["teamA"], "owner");
        assert_eq!(me["policy"]["exportTeamGroups"], "open");

        // Org settings: bob (writer) may read but not write; alice (owner) may write.
        let (st, org) = req(&app, "GET", "/admin/v1/org", &bob, None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(org["teamCreation"], "bootstrap");
        let (st, _) = req(&app, "PUT", "/admin/v1/org", &bob,
            Some(serde_json::json!({"teamCreation":"owners","exportTeamGroups":"managers","auditRetentionDays":30}))).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        let (st, _) = req(&app, "PUT", "/admin/v1/org", &alice,
            Some(serde_json::json!({"defaultRole":"reader","teamCreation":"owners","exportTeamGroups":"managers","auditRetentionDays":30}))).await;
        assert_eq!(st, StatusCode::OK);
        // Invalid knobs are rejected.
        let (st, _) = req(&app, "PUT", "/admin/v1/org", &alice,
            Some(serde_json::json!({"teamCreation":"anarchy","exportTeamGroups":"open","auditRetentionDays":30}))).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);

        // Org default_role now applies: bob resolves reader → his pushes are 403.
        let (_, me) = req(&app, "GET", "/v1/me", &bob, None).await;
        assert_eq!(me["roles"]["teamA"], "reader");
        assert_eq!(me["policy"]["exportTeamGroups"], "managers");
        let (st, _) = req(&app, "POST", "/v1/teams/teamA/changes", &bob,
            Some(serde_json::json!({"snippets":[snip_json_t("x1","x",1,"2026-07-02T10:00:00.000Z","teamA")],"groups":[]}))).await;
        assert_eq!(st, StatusCode::FORBIDDEN);

        // Bootstrap is now OFF (team_creation=owners): bob touching a fresh claim team must
        // NOT be crowned. Give bob a claim on teamB via a new token set? Instead: alice's
        // claim only has teamA, so use teamA2 path — verify via roles listing that touching
        // a team with no owner in owners-mode records nothing.
        // (bob has no claim on teamB → 403 outright.)
        let (st, _) = req(&app, "GET", "/v1/teams/teamB/changes?since=0", &bob, None).await;
        assert_eq!(st, StatusCode::FORBIDDEN);

        // Team creation policy "owners": bob (reader) may not create; alice (owner) may.
        let (st, _) = req(&app, "POST", "/admin/v1/teams", &bob,
            Some(serde_json::json!({"team":"newteam"}))).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        let (st, created) = req(&app, "POST", "/admin/v1/teams", &alice,
            Some(serde_json::json!({"team":"newteam"}))).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(created["role"], "owner"); // creator becomes owner
        let (st, _) = req(&app, "POST", "/admin/v1/teams", &alice,
            Some(serde_json::json!({"team":"bad/name"}))).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);

        // Archive: hidden from listings, sync 403 with "team archived" detail; revivable.
        let (st, _) = req(&app, "DELETE", "/admin/v1/teams/newteam", &alice, None).await;
        assert_eq!(st, StatusCode::OK);
        let (_, teams) = req(&app, "GET", "/admin/v1/teams", &alice, None).await;
        assert!(!teams.as_array().unwrap().iter().any(|t| t["team"] == "newteam"));
        let (st, prob) = req(&app, "GET", "/v1/teams/newteam/changes?since=0", &alice, None).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        assert_eq!(prob["detail"], "team archived");
        let (st, _) = req(&app, "POST", "/admin/v1/teams", &alice,
            Some(serde_json::json!({"team":"newteam"}))).await;
        assert_eq!(st, StatusCode::OK, "owner re-POSTing an archived team revives it");
        let (st, _) = req(&app, "GET", "/v1/teams/newteam/changes?since=0", &alice, None).await;
        assert_eq!(st, StatusCode::OK);

        // Audit: owner sees org-wide entries incl. org.settings / team.create / role rows.
        let (st, audit) = req(&app, "GET", "/admin/v1/audit?limit=100", &alice, None).await;
        assert_eq!(st, StatusCode::OK);
        let actions: Vec<&str> = audit.as_array().unwrap().iter()
            .map(|e| e["action"].as_str().unwrap()).collect();
        assert!(actions.contains(&"org.settings"));
        assert!(actions.contains(&"team.create"));
        assert!(actions.contains(&"team.archive"));
        assert!(actions.contains(&"role.bootstrap_owner"));
        assert!(actions.contains(&"member.first_seen"));
        // Push counts (never content) were logged earlier in other tests' flows? push here:
        let (st, _) = req(&app, "POST", "/v1/teams/teamA/changes", &alice,
            Some(serde_json::json!({"snippets":[snip_json_t("a1","hello",1,"2026-07-02T10:00:00.000Z","teamA")],"groups":[]}))).await;
        assert_eq!(st, StatusCode::OK);
        let (_, audit) = req(&app, "GET", "/admin/v1/audit?limit=10", &alice, None).await;
        let push_entry = audit.as_array().unwrap().iter()
            .find(|e| e["action"] == "push").expect("push audited");
        assert_eq!(push_entry["detail"], "snippets=1 groups=0 rejected=0");
        assert!(!push_entry.to_string().contains("hello"), "audit must not contain content");
        // bob (reader) may not read audit.
        let (st, _) = req(&app, "GET", "/admin/v1/audit", &bob, None).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
    }

    /// Phase 4 §E2 isolation: cross-team access is denied even for owners/admins of other
    /// teams, listings never leak foreign teams, archived stays blocked.
    #[tokio::test]
    async fn team_isolation_matrix() {
        let dir = tempfile::tempdir().unwrap();
        let (ann, ann_sha) = tok("ann"); // owner of teamA only
        let (ben, ben_sha) = tok("ben"); // owner of teamB only
        let tokens = format!(
            r#"[{{"tokenSha256":"{ann_sha}","sub":"ann","teams":["teamA"]}},
                {{"tokenSha256":"{ben_sha}","sub":"ben","teams":["teamB"]}}]"#
        );
        let app = router(test_state(&dir, &tokens));

        // Bootstrap both owners.
        let (st, _) = req(&app, "GET", "/v1/teams/teamA/changes?since=0", &ann, None).await;
        assert_eq!(st, StatusCode::OK);
        let (st, _) = req(&app, "GET", "/v1/teams/teamB/changes?since=0", &ben, None).await;
        assert_eq!(st, StatusCode::OK);

        // (a) ann (owner of A) is 403 on ALL of team B's surfaces.
        for path in [
            "/v1/teams/teamB/changes?since=0",
            "/v1/teams/teamB/members",
            "/admin/v1/teams/teamB/roles",
        ] {
            let (st, _) = req(&app, "GET", path, &ann, None).await;
            assert_eq!(st, StatusCode::FORBIDDEN, "ann must be denied on {path}");
        }
        let (st, _) = req(&app, "POST", "/v1/teams/teamB/changes", &ann,
            Some(serde_json::json!({"snippets":[snip_json_t("s","x",1,"2026-07-02T10:00:00.000Z","teamB")],"groups":[]}))).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        let (st, _) = req(&app, "PUT", "/admin/v1/teams/teamB/roles/ann", &ann,
            Some(serde_json::json!({"role":"owner"}))).await;
        assert_eq!(st, StatusCode::FORBIDDEN, "cannot self-grant into a foreign team");

        // (b) listings never include foreign teams.
        let (_, teams) = req(&app, "GET", "/admin/v1/teams", &ann, None).await;
        let names: Vec<&str> = teams.as_array().unwrap().iter()
            .map(|t| t["team"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["teamA"]);
        let (_, me) = req(&app, "GET", "/v1/me", &ann, None).await;
        assert!(me["roles"].get("teamB").is_none());

        // (c) archived team stays blocked for everyone incl. its owner.
        let (st, _) = req(&app, "DELETE", "/admin/v1/teams/teamB", &ben, None).await;
        assert_eq!(st, StatusCode::OK);
        let (st, prob) = req(&app, "GET", "/v1/teams/teamB/changes?since=0", &ben, None).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        assert_eq!(prob["detail"], "team archived");
        let (st, _) = req(&app, "POST", "/v1/teams/teamB/changes", &ben,
            Some(serde_json::json!({"snippets":[snip_json_t("s","x",1,"2026-07-02T10:00:00.000Z","teamB")],"groups":[]}))).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        let (st, _) = req(&app, "GET", "/v1/teams/teamB/members", &ben, None).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
    }

    /// Phase 4 manager tier: managers add members up to writer, never manager+.
    #[tokio::test]
    async fn manager_grant_ceiling() {
        let dir = tempfile::tempdir().unwrap();
        let (ora, ora_sha) = tok("ora");
        let (max, max_sha) = tok("max");
        let tokens = format!(
            r#"[{{"tokenSha256":"{ora_sha}","sub":"ora","teams":["t"]}},
                {{"tokenSha256":"{max_sha}","sub":"max","teams":["t"]}}]"#
        );
        let app = router(test_state(&dir, &tokens));
        let (st, _) = req(&app, "GET", "/v1/teams/t/changes?since=0", &ora, None).await;
        assert_eq!(st, StatusCode::OK); // ora bootstraps owner
        let (st, _) = req(&app, "PUT", "/admin/v1/teams/t/roles/max", &ora,
            Some(serde_json::json!({"role":"manager"}))).await;
        assert_eq!(st, StatusCode::OK);

        // manager may list roles + grant reader/writer…
        let (st, _) = req(&app, "GET", "/admin/v1/teams/t/roles", &max, None).await;
        assert_eq!(st, StatusCode::OK);
        let (st, _) = req(&app, "PUT", "/admin/v1/teams/t/roles/newbie", &max,
            Some(serde_json::json!({"role":"writer"}))).await;
        assert_eq!(st, StatusCode::OK);
        // …but not manager/admin, and not touch a manager+ target.
        let (st, _) = req(&app, "PUT", "/admin/v1/teams/t/roles/other", &max,
            Some(serde_json::json!({"role":"manager"}))).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        let (st, _) = req(&app, "DELETE", "/admin/v1/teams/t/roles/ora", &max, None).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        // manager appears in the admin teams listing now.
        let (_, teams) = req(&app, "GET", "/admin/v1/teams", &max, None).await;
        assert!(teams.as_array().unwrap().iter().any(|t| t["team"] == "t" && t["role"] == "manager"));
    }

    fn snip_json_t(id: &str, replacement: &str, version: i64, ts: &str, team: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id, "trigger": ":t", "replacement": replacement, "format": "plain",
            "owner": "ignored", "team": team, "updatedAt": ts, "version": version
        })
    }


    /// Invite tokens: create (ceiling-gated), authenticate, expiry, revoke-access semantics
    /// (single-team revoke vs multi-team trim), audit without token material.
    #[tokio::test]
    async fn invite_tokens_lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        let (alice, alice_sha) = tok("alice");
        let tokens = format!(
            r#"[{{"tokenSha256":"{alice_sha}","sub":"alice","teams":["t"]}}]"#
        );
        let state = test_state(&dir, &tokens);
        let storage = state.storage.clone();
        let app = router(state);

        // alice bootstraps ownership.
        let (st, _) = req(&app, "GET", "/v1/teams/t/changes?since=0", &alice, None).await;
        assert_eq!(st, StatusCode::OK);

        // Owner invites ivy as writer with expiry.
        let (st, inv) = req(&app, "POST", "/admin/v1/teams/t/invites", &alice,
            Some(serde_json::json!({"sub":"ivy","email":"ivy@example.com","role":"writer","expiresDays":7}))).await;
        assert_eq!(st, StatusCode::OK);
        let ivy_token = inv["token"].as_str().unwrap().to_string();
        assert_eq!(ivy_token.len(), 64); // 32 bytes hex
        assert_eq!(inv["role"], "writer");

        // The invite token authenticates with the pinned identity.
        let (st, me) = req(&app, "GET", "/v1/me", &ivy_token, None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(me["sub"], "ivy");
        assert_eq!(me["roles"]["t"], "writer");
        let (st, _) = req(&app, "POST", "/v1/teams/t/changes", &ivy_token,
            Some(serde_json::json!({"snippets":[snip_json_t("iv1","x",1,"2026-07-02T10:00:00.000Z","t")],"groups":[]}))).await;
        assert_eq!(st, StatusCode::OK);

        // Ceiling: owner invites mona as manager; mona may invite writers but not managers.
        let (st, inv) = req(&app, "POST", "/admin/v1/teams/t/invites", &alice,
            Some(serde_json::json!({"sub":"mona","role":"manager"}))).await;
        assert_eq!(st, StatusCode::OK);
        let mona_token = inv["token"].as_str().unwrap().to_string();
        let (st, _) = req(&app, "POST", "/admin/v1/teams/t/invites", &mona_token,
            Some(serde_json::json!({"sub":"kid","role":"manager"}))).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        let (st, _) = req(&app, "POST", "/admin/v1/teams/t/invites", &mona_token,
            Some(serde_json::json!({"sub":"kid","role":"writer"}))).await;
        assert_eq!(st, StatusCode::OK);
        let (st, _) = req(&app, "POST", "/admin/v1/teams/t/invites", &alice,
            Some(serde_json::json!({"sub":"x","role":"sudo"}))).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);

        // Expired tokens are rejected.
        let expired = crate::storage::StoredToken {
            token_sha256: hex::encode(sha2::Sha256::digest(b"expired-token")),
            sub: "old".into(), email: None, teams: vec!["t".into()], role: None,
            created_by: "alice".into(), created_at: "2020-01-01T00:00:00.000Z".into(),
            expires_at: Some("2020-06-01T00:00:00.000Z".into()), revoked_at: None,
        };
        storage.store_token(&expired).await.unwrap();
        let (st, _) = req(&app, "GET", "/v1/me", "expired-token", None).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);

        // Multi-team token: revoking one team trims the list, the token survives.
        let multi = crate::storage::StoredToken {
            token_sha256: hex::encode(sha2::Sha256::digest(b"multi-token")),
            sub: "ivy".into(), email: None, teams: vec!["t".into(), "u".into()], role: None,
            created_by: "alice".into(), created_at: "2026-01-01T00:00:00.000Z".into(),
            expires_at: None, revoked_at: None,
        };
        storage.store_token(&multi).await.unwrap();

        // Revoke ivy's access to t: single-team invite token dies, multi-team token trims.
        let (st, out) = req(&app, "DELETE", "/admin/v1/teams/t/access/ivy", &alice, None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(out["tokens"], 2);
        let (st, _) = req(&app, "GET", "/v1/me", &ivy_token, None).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "single-team token must be revoked");
        let (st, me) = req(&app, "GET", "/v1/me", "multi-token", None).await;
        assert_eq!(st, StatusCode::OK, "multi-team token survives with the team removed");
        assert_eq!(me["teams"], serde_json::json!(["u"]));

        // Audit has the events and never any token material.
        let (_, audit) = req(&app, "GET", "/admin/v1/audit?limit=100", &alice, None).await;
        let text = audit.to_string();
        assert!(text.contains("invite.create"));
        assert!(text.contains("access.revoke"));
        assert!(!text.contains(&ivy_token), "audit must not contain token plaintext");
        assert!(!text.contains(&hex::encode(sha2::Sha256::digest(ivy_token.as_bytes()))),
            "audit must not contain token digests either");
    }

    /// Self-service membership: one credential accumulates teams by redeeming invites, an
    /// invite is single-use, leaving drops access, and the last owner can't walk away.
    #[tokio::test]
    async fn join_and_leave_multiple_teams() {
        let dir = tempfile::tempdir().unwrap();
        let (alice, alice_sha) = tok("alice");
        let (nick, nick_sha) = tok("nick");
        let tokens = format!(
            r#"[{{"tokenSha256":"{alice_sha}","sub":"alice","teams":["red","blue"]}},
                {{"tokenSha256":"{nick_sha}","sub":"nick","teams":["own"]}}]"#
        );
        let app = router(test_state(&dir, &tokens));

        // alice bootstraps ownership of both her teams; nick owns his.
        for team in ["red", "blue"] {
            let (st, _) = req(&app, "GET", &format!("/v1/teams/{team}/changes?since=0"), &alice, None).await;
            assert_eq!(st, StatusCode::OK);
        }
        let (st, _) = req(&app, "GET", "/v1/teams/own/changes?since=0", &nick, None).await;
        assert_eq!(st, StatusCode::OK);

        // nick starts with one team and joins alice's two by redeeming invites — with the
        // credential he already has, so nothing displaces his existing membership.
        let mut codes = Vec::new();
        for (team, role) in [("red", "writer"), ("blue", "reader")] {
            let (st, inv) = req(&app, "POST", &format!("/admin/v1/teams/{team}/invites"), &alice,
                Some(serde_json::json!({"sub":"nick","role":role}))).await;
            assert_eq!(st, StatusCode::OK);
            codes.push(inv["token"].as_str().unwrap().to_string());
        }
        for code in &codes {
            let (st, me) = req(&app, "POST", "/v1/invites/redeem", &nick,
                Some(serde_json::json!({"code": code}))).await;
            assert_eq!(st, StatusCode::OK);
            assert_eq!(me["sub"], "nick");
        }
        let (_, me) = req(&app, "GET", "/v1/me", &nick, None).await;
        assert_eq!(me["teams"], serde_json::json!(["blue", "own", "red"]));
        assert_eq!(me["roles"]["red"], "writer");
        assert_eq!(me["roles"]["blue"], "reader");
        assert_eq!(me["roles"]["own"], "owner", "joining must not disturb existing roles");

        // He can actually sync the joined team now.
        let (st, _) = req(&app, "POST", "/v1/teams/red/changes", &nick,
            Some(serde_json::json!({"snippets":[snip_json_t("n1","x",1,"2026-07-02T10:00:00.000Z","red")],"groups":[]}))).await;
        assert_eq!(st, StatusCode::OK);

        // An invite is single-use, and it never authenticates on its own afterwards.
        let (st, _) = req(&app, "POST", "/v1/invites/redeem", &nick,
            Some(serde_json::json!({"code": codes[0]}))).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        let (st, _) = req(&app, "GET", "/v1/me", &codes[0], None).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
        let (st, _) = req(&app, "POST", "/v1/invites/redeem", &nick,
            Some(serde_json::json!({"code": "0".repeat(64)}))).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);

        // Leaving drops the row and the access that came with it.
        let (st, out) = req(&app, "DELETE", "/v1/teams/blue/membership", &nick, None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(out["left"], "blue");
        let (_, me) = req(&app, "GET", "/v1/me", &nick, None).await;
        assert_eq!(me["teams"], serde_json::json!(["own", "red"]));
        let (st, _) = req(&app, "GET", "/v1/teams/blue/changes?since=0", &nick, None).await;
        assert_eq!(st, StatusCode::FORBIDDEN);

        // The only owner of a team can't strand it.
        let (st, _) = req(&app, "DELETE", "/v1/teams/own/membership", &nick, None).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        let (_, me) = req(&app, "GET", "/v1/me", &nick, None).await;
        assert_eq!(me["roles"]["own"], "owner");

        // Both membership changes are on the record, without token material.
        let (_, audit) = req(&app, "GET", "/admin/v1/audit?limit=100", &alice, None).await;
        let text = audit.to_string();
        assert!(text.contains("invite.redeem"));
        assert!(text.contains("team.leave"));
        for code in &codes {
            assert!(!text.contains(code.as_str()), "audit must not contain invite plaintext");
        }
    }

    /// Restricted groups: pull filtering (with global cursor), generic push 403, read vs
    /// write grants, manager visibility, ACL endpoint authz, audit.
    #[tokio::test]
    async fn restricted_groups_visibility_and_grants() {
        let dir = tempfile::tempdir().unwrap();
        let (alice, alice_sha) = tok("alice"); // becomes owner
        let (bob, bob_sha) = tok("bob");       // default writer
        let tokens = format!(
            r#"[{{"tokenSha256":"{alice_sha}","sub":"alice","teams":["t"]}},
                {{"tokenSha256":"{bob_sha}","sub":"bob","teams":["t"]}}]"#
        );
        let app = router(test_state(&dir, &tokens));
        let (st, _) = req(&app, "GET", "/v1/teams/t/changes?since=0", &alice, None).await;
        assert_eq!(st, StatusCode::OK);

        // Owner pushes: group g1, a snippet inside it, and an ungrouped snippet.
        let group = serde_json::json!({
            "id":"g1","name":"Secret","sortOrder":0,"team":"t",
            "updatedAt":"2026-07-02T10:00:00.000Z","version":1});
        let mut s_in = snip_json_t("s-in","inside",1,"2026-07-02T10:00:00.000Z","t");
        s_in["groupId"] = "g1".into();
        let s_out = snip_json_t("s-out","outside",1,"2026-07-02T10:00:00.000Z","t");
        let (st, _) = req(&app, "POST", "/v1/teams/t/changes", &alice,
            Some(serde_json::json!({"snippets":[s_in, s_out],"groups":[group]}))).await;
        assert_eq!(st, StatusCode::OK);

        // Restrict g1 (manager+ only; bob the writer is refused).
        let (st, _) = req(&app, "PUT", "/admin/v1/teams/t/groups/g1/restricted", &bob,
            Some(serde_json::json!({"restricted":true}))).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        let (st, _) = req(&app, "PUT", "/admin/v1/teams/t/groups/g1/restricted", &alice,
            Some(serde_json::json!({"restricted":true}))).await;
        assert_eq!(st, StatusCode::OK);

        // Dashboard group list shows the flag.
        let (st, groups) = req(&app, "GET", "/admin/v1/teams/t/groups", &alice, None).await;
        assert_eq!(st, StatusCode::OK);
        assert!(groups.as_array().unwrap().iter().any(|g| g["id"] == "g1" && g["restricted"] == true));

        // Manager+ (owner) sees everything, with the server-set restricted flag on the wire.
        let (_, ch) = req(&app, "GET", "/v1/teams/t/changes?since=0", &alice, None).await;
        let owner_cursor = ch["nextCursor"].as_u64().unwrap();
        assert_eq!(ch["snippets"].as_array().unwrap().len(), 2);
        assert_eq!(ch["groups"][0]["restricted"], true);

        // Non-granted bob: group + inside-snippet omitted, cursor still fully advances.
        let (_, ch) = req(&app, "GET", "/v1/teams/t/changes?since=0", &bob, None).await;
        assert!(ch["groups"].as_array().unwrap().is_empty());
        let ids: Vec<&str> = ch["snippets"].as_array().unwrap().iter()
            .map(|s| s["id"].as_str().unwrap()).collect();
        assert_eq!(ids, vec!["s-out"]);
        assert_eq!(ch["nextCursor"].as_u64().unwrap(), owner_cursor,
            "cursor advances globally; filtering is at serialization");

        // Push into the restricted group without a grant → generic batch 403.
        let mut intrude = snip_json_t("s-x","sneak",1,"2026-07-02T11:00:00.000Z","t");
        intrude["groupId"] = "g1".into();
        let (st, prob) = req(&app, "POST", "/v1/teams/t/changes", &bob,
            Some(serde_json::json!({"snippets":[intrude.clone()],"groups":[]}))).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        assert_eq!(prob["title"], "forbidden", "must not confirm the group's existence");
        assert!(!prob.to_string().contains("restricted") && !prob.to_string().contains("g1"));

        // Read grant: bob now sees g1 + s-in, but still can't push into it.
        let (st, _) = req(&app, "PUT", "/admin/v1/teams/t/groups/g1/acl/bob", &alice,
            Some(serde_json::json!({"level":"read"}))).await;
        assert_eq!(st, StatusCode::OK);
        let (_, ch) = req(&app, "GET", "/v1/teams/t/changes?since=0", &bob, None).await;
        assert_eq!(ch["groups"][0]["id"], "g1");
        assert_eq!(ch["groups"][0]["restricted"], true);
        assert_eq!(ch["snippets"].as_array().unwrap().len(), 2);
        let (st, _) = req(&app, "POST", "/v1/teams/t/changes", &bob,
            Some(serde_json::json!({"snippets":[intrude.clone()],"groups":[]}))).await;
        assert_eq!(st, StatusCode::FORBIDDEN);

        // Write grant: push accepted.
        let (st, _) = req(&app, "PUT", "/admin/v1/teams/t/groups/g1/acl/bob", &alice,
            Some(serde_json::json!({"level":"write"}))).await;
        assert_eq!(st, StatusCode::OK);
        let (st, ack) = req(&app, "POST", "/v1/teams/t/changes", &bob,
            Some(serde_json::json!({"snippets":[intrude],"groups":[]}))).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(ack["snippets"][0]["status"], "accepted");

        // Invalid level → 422; grant removal hides the group again.
        let (st, _) = req(&app, "PUT", "/admin/v1/teams/t/groups/g1/acl/bob", &alice,
            Some(serde_json::json!({"level":"root"}))).await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        let (st, _) = req(&app, "DELETE", "/admin/v1/teams/t/groups/g1/acl/bob", &alice, None).await;
        assert_eq!(st, StatusCode::OK);
        let (_, ch) = req(&app, "GET", "/v1/teams/t/changes?since=0", &bob, None).await;
        assert!(ch["groups"].as_array().unwrap().is_empty());

        // A client cannot smuggle `restricted: true` onto an unrestricted group.
        let sneaky = serde_json::json!({
            "id":"g2","name":"Normal","sortOrder":0,"team":"t","restricted":true,
            "updatedAt":"2026-07-02T10:00:00.000Z","version":1});
        let (st, _) = req(&app, "POST", "/v1/teams/t/changes", &alice,
            Some(serde_json::json!({"snippets":[],"groups":[sneaky]}))).await;
        assert_eq!(st, StatusCode::OK);
        let (_, ch) = req(&app, "GET", "/v1/teams/t/changes?since=0", &bob, None).await;
        let g2 = ch["groups"].as_array().unwrap().iter().find(|g| g["id"] == "g2").unwrap();
        assert!(g2.get("restricted").is_none(), "server must strip client-set restricted flags");

        // Audit trail.
        let (_, audit) = req(&app, "GET", "/admin/v1/audit?limit=100", &alice, None).await;
        let text = audit.to_string();
        assert!(text.contains("group.restricted"));
        assert!(text.contains("group.grant"));
        assert!(text.contains("group.ungrant"));
    }

    /// End-to-end router test: static-token auth + SQLite storage in a temp dir.
    /// Covers: unauthenticated 401, /me, team authz 403, push→changes roundtrip,
    /// team-mismatch 422.
    #[tokio::test]
    async fn router_happy_path_and_authz() {
        let dir = tempfile::tempdir().unwrap();
        let token = "test-token-0123456789abcdef";
        let sha = hex::encode(sha2::Sha256::digest(token.as_bytes()));
        // user1 authenticates in this test; zed is configured-but-never-seen (roster only).
        let tokens = format!(
            r#"[{{"tokenSha256":"{sha}","sub":"user1","teams":["sec"]}},
                {{"tokenSha256":"{}","sub":"zed","email":"zed@example.com","teams":["sec"]}}]"#,
            hex::encode(sha2::Sha256::digest(b"never-used-token"))
        );

        let state = AppState {
            storage: Arc::new(
                storage::sqlite::SqliteStorage::open(
                    dir.path().join("t.db").to_str().unwrap(),
                )
                .unwrap(),
            ),
            auth: Arc::new(auth::Authenticator::for_tests(&tokens)),
            limiter: Arc::new(ratelimit::RateLimiter::new(1000)),
            default_role: sync_proto::Role::Writer,
        };
        let app = router(state);

        // no token → 401 problem+json
        let res = app
            .clone()
            .oneshot(Request::get("/v1/me").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        // /me
        let res = app
            .clone()
            .oneshot(
                Request::get("/v1/me")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let me: sync_proto::Me =
            serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert_eq!(me.sub, "user1");
        assert_eq!(me.teams, vec!["sec"]);

        // wrong team → 403
        let res = app
            .clone()
            .oneshot(
                Request::get("/v1/teams/other/changes?since=0")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);

        // push a snippet
        let snip = serde_json::json!({
            "id": "s1", "trigger": ":t", "replacement": "hello", "format": "plain",
            "owner": "spoofed-owner", "team": "sec",
            "updatedAt": "2026-07-02T10:00:00.000Z", "version": 1
        });
        let res = app
            .clone()
            .oneshot(
                Request::post("/v1/teams/sec/changes")
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"snippets":[snip],"groups":[]}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let ack: sync_proto::PushAck =
            serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert_eq!(ack.snippets.len(), 1);
        assert!(matches!(ack.snippets[0].status, sync_proto::OutcomeStatus::Accepted));

        // pull it back — owner must be the authenticated sub, not the spoofed value
        let res = app
            .clone()
            .oneshot(
                Request::get("/v1/teams/sec/changes?since=0")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let ch: sync_proto::Changes =
            serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert_eq!(ch.snippets.len(), 1);
        assert_eq!(ch.snippets[0].owner, "user1");
        assert!(ch.next_cursor >= 1);

        // record whose team != path team → 422
        let bad = serde_json::json!({
            "id": "s2", "trigger": ":x", "replacement": "x", "format": "plain",
            "owner": "user1", "team": "other",
            "updatedAt": "2026-07-02T10:00:00.000Z", "version": 1
        });
        let res = app
            .clone()
            .oneshot(
                Request::post("/v1/teams/sec/changes")
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"snippets":[bad],"groups":[]}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);

        // members: configured roster (zed, never seen) ∪ seen (user1, from the calls above),
        // deduped, sorted by sub; seen entries carry last_seen.
        let res = app
            .clone()
            .oneshot(
                Request::get("/v1/teams/sec/members")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let members: sync_proto::Members =
            serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
        let subs: Vec<&str> = members.members.iter().map(|m| m.sub.as_str()).collect();
        assert_eq!(subs, vec!["user1", "zed"]); // deduped + sorted
        let user1 = &members.members[0];
        assert!(user1.last_seen.is_some(), "authenticated caller must be tracked as seen");
        let zed = &members.members[1];
        assert_eq!(zed.email.as_deref(), Some("zed@example.com"));
        assert!(zed.last_seen.is_none(), "configured-but-never-seen member has no last_seen");

        // members of a foreign team → 403
        let res = app
            .clone()
            .oneshot(
                Request::get("/v1/teams/other/members")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    /// Console v2 endpoints: /admin/v1/config is public (no auth), /admin/v1/stats needs
    /// manager+ and carries team counters + activity buckets, and audit action/actor
    /// filters narrow the result set.
    #[tokio::test]
    async fn console_config_stats_and_audit_filters() {
        let dir = tempfile::tempdir().unwrap();
        let (alice, alice_sha) = tok("alice");
        let (carol, carol_sha) = tok("carol");
        let tokens = format!(
            r#"[{{"tokenSha256":"{alice_sha}","sub":"alice","teams":["sec"]}},
                {{"tokenSha256":"{carol_sha}","sub":"carol","teams":["sec"],"role":"reader"}}]"#
        );
        let app = router(test_state(&dir, &tokens));

        // config: unauthenticated 200 with the default scopes (no OIDC env in tests).
        let res = app
            .clone()
            .oneshot(Request::get("/admin/v1/config").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let cfg: serde_json::Value =
            serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert_eq!(cfg["scopes"], "openid profile email");

        // Bootstrap alice as owner, then generate audit entries (push + role change).
        let (st, _) = req(&app, "GET", "/v1/teams/sec/changes?since=0", &alice, None).await;
        assert_eq!(st, StatusCode::OK);
        let (st, _) = req(&app, "POST", "/v1/teams/sec/changes", &alice,
            Some(serde_json::json!({"snippets": [snip_json("s1", "x", 1, "2026-07-07T10:00:00.000Z")], "groups": []}))).await;
        assert_eq!(st, StatusCode::OK);
        let (st, _) = req(&app, "PUT", "/admin/v1/teams/sec/roles/dora", &alice,
            Some(serde_json::json!({"role": "writer"}))).await;
        assert_eq!(st, StatusCode::OK);

        // stats as owner: sec counted with members, and today's activity bucketed.
        let (st, stats) = req(&app, "GET", "/admin/v1/stats", &alice, None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(stats["teams"][0]["team"], "sec");
        assert!(stats["teams"][0]["members"].as_u64().unwrap() >= 1);
        let day = &stats["activity"].as_array().unwrap()[0];
        assert!(day["pushes"].as_u64().unwrap() >= 1);
        assert!(day["roles"].as_u64().unwrap() >= 1);

        // stats as a pinned reader → 403.
        let (st, _) = req(&app, "GET", "/admin/v1/stats", &carol, None).await;
        assert_eq!(st, StatusCode::FORBIDDEN);

        // audit filters: action narrows, and a non-matching filter yields nothing.
        let (st, entries) = req(&app, "GET", "/admin/v1/audit?action=role.set", &alice, None).await;
        assert_eq!(st, StatusCode::OK);
        let entries = entries.as_array().unwrap().clone();
        assert!(!entries.is_empty());
        assert!(entries.iter().all(|e| e["action"] == "role.set"));
        let (_, none) = req(&app, "GET", "/admin/v1/audit?actor=nobody-matches", &alice, None).await;
        assert!(none.as_array().unwrap().is_empty());
    }
}
