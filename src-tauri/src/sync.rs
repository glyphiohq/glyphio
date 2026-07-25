//! Sync wiring: owns the engine lifecycle and exposes the Tauri command surface for the
//! Settings → Sync UI. All heavy lifting lives in the `sync-client` crate; this module only
//! builds/rebuilds the engine when configuration or session state changes, and forwards
//! status transitions to the webview as `sync-status` events.
//!
//! Guardrail: with `enabled = false` (the shipped default) no engine is ever constructed, so
//! the app makes zero sync-related network calls until the user configures a backend.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use sync_client::auth::{AuthProvider, OidcAuth, StaticTokenAuth};
use sync_client::engine::SyncEngine;
use sync_client::http::HttpSync;
use sync_client::{SyncConfig, SyncStatus};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::AppState;

/// System-wide managed configuration (deployed by IT/MDM, root-owned). When present, its
/// sync settings are authoritative: the user's `sync.toml` is ignored for connection fields
/// and the app refuses user edits — the enterprise anti-exfiltration control that stops
/// team content being redirected to a rogue backend. Absent on self-hosted/personal installs.
#[cfg(target_os = "macos")]
const MANAGED_CONFIG: &str = "/Library/Application Support/Glyphio/managed.toml";
#[cfg(not(target_os = "macos"))]
const MANAGED_CONFIG: &str = "/etc/glyphio/managed.toml";

pub struct SyncState {
    config_path: PathBuf,
    engine: Mutex<Option<Arc<SyncEngine>>>,
    task: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}

/// What the UI gets: the effective config plus whether it is org-managed (locked).
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncConfigView {
    #[serde(flatten)]
    pub config: SyncConfig,
    pub managed: bool,
}

impl SyncState {
    pub fn new(config_path: PathBuf) -> Self {
        Self { config_path, engine: Mutex::new(None), task: Mutex::new(None) }
    }

    fn managed_config() -> Option<SyncConfig> {
        let path = std::path::Path::new(MANAGED_CONFIG);
        if !path.exists() {
            return None;
        }
        let cfg = SyncConfig::load(path);
        // A managed file that fails validation is ignored loudly rather than silently
        // unlocking the client.
        match cfg.validate() {
            Ok(()) => Some(cfg),
            Err(e) => {
                log::error!("managed config at {MANAGED_CONFIG} is invalid ({e}) — sync disabled");
                Some(SyncConfig::default()) // managed-but-broken = locked and off
            }
        }
    }

    pub fn is_managed(&self) -> bool {
        std::path::Path::new(MANAGED_CONFIG).exists()
    }

    pub fn config(&self) -> SyncConfig {
        Self::managed_config().unwrap_or_else(|| SyncConfig::load(&self.config_path))
    }

    pub(crate) fn status(&self) -> SyncStatus {
        if let Some(e) = self.engine.lock().unwrap().as_ref() {
            return e.status();
        }
        let cfg = self.config();
        SyncStatus {
            state: if cfg.enabled { "signedOut" } else { "disabled" }.to_string(),
            ..Default::default()
        }
    }

    /// Tear down any running engine and, if configured + authenticated, start a fresh one.
    /// `auth_override` carries a just-completed interactive sign-in.
    fn rebuild(&self, app: &AppHandle, auth_override: Option<Box<dyn AuthProvider>>) {
        // Stop the old loop. Aborting mid-cycle is safe: every store apply is idempotent LWW
        // and the dirty flags are the push queue, so nothing is lost.
        if let Some(t) = self.task.lock().unwrap().take() {
            t.abort();
        }
        *self.engine.lock().unwrap() = None;

        let cfg = self.config();
        if cfg.validate().is_err() {
            emit_status(app, &self.status());
            return;
        }
        let auth: Box<dyn AuthProvider> = match auth_override {
            Some(a) => a,
            None => match cfg.auth_mode.as_str() {
                "oidc" if OidcAuth::has_session() => Box::new(OidcAuth::resume(&cfg)),
                "token" if StaticTokenAuth::has_token() => Box::new(StaticTokenAuth),
                mode => {
                    // Configured but no stored credential (or the keychain denied access —
                    // e.g. an unsigned dev binary whose identity changed since the grant).
                    log::info!("sync configured (mode: {mode}) but no usable credential — waiting for sign-in");
                    emit_status(app, &self.status());
                    return;
                }
            },
        };
        let provider = match HttpSync::new(&cfg.backend_url) {
            Ok(p) => Box::new(p),
            Err(e) => {
                log::warn!("sync disabled: {e}");
                return;
            }
        };
        let handle = app.clone();
        let engine = SyncEngine::new(
            self.store(app),
            provider,
            auth,
            cfg.interval(),
            Box::new(move |s| emit_status(&handle, s)),
        );
        let task = tauri::async_runtime::spawn(engine.clone().run());
        engine.kick();
        *self.engine.lock().unwrap() = Some(engine);
        *self.task.lock().unwrap() = Some(task);
    }

    fn store(&self, app: &AppHandle) -> Arc<snippet_store::SnippetStore> {
        app.state::<AppState>().snippets.clone()
    }
}

fn emit_status(app: &AppHandle, status: &SyncStatus) {
    let _ = app.emit("sync-status", status.clone());
}

/// Called once at startup: resume a previous session silently if one exists.
pub fn init(app: &AppHandle) {
    let state = app.state::<AppState>();
    state.sync.rebuild(app, None);
}

// ---- commands ----------------------------------------------------------------

type CmdResult<T> = Result<T, String>;

#[tauri::command]
pub fn get_sync_config(state: State<AppState>) -> SyncConfigView {
    SyncConfigView { config: state.sync.config(), managed: state.sync.is_managed() }
    // contains no secrets by design
}

#[tauri::command]
pub fn save_sync_config(app: AppHandle, state: State<AppState>, config: SyncConfig) -> CmdResult<()> {
    if state.sync.is_managed() {
        return Err("Sync connection settings are managed by your organization.".into());
    }
    if config.enabled {
        config.validate().map_err(|e| e.to_string())?;
    }
    config.save(&state.sync.config_path).map_err(|e| e.to_string())?;
    state.sync.rebuild(&app, None);
    Ok(())
}

#[tauri::command]
pub fn sync_status(state: State<AppState>) -> SyncStatus {
    state.sync.status()
}

#[tauri::command]
pub fn sync_now(state: State<AppState>) -> CmdResult<()> {
    match state.sync.engine.lock().unwrap().as_ref() {
        Some(e) => {
            e.kick();
            Ok(())
        }
        None => Err("sync is not configured or signed in".into()),
    }
}

/// Interactive OIDC sign-in: opens the system browser, waits for the loopback callback,
/// then starts the engine with the fresh session.
#[tauri::command]
pub async fn sync_sign_in(app: AppHandle) -> CmdResult<()> {
    let cfg = app.state::<AppState>().sync.config();
    cfg.validate().map_err(|e| e.to_string())?;
    if cfg.auth_mode != "oidc" {
        return Err("authMode is not \"oidc\" — paste a token instead".into());
    }
    let opener = app.clone();
    let auth = OidcAuth::sign_in(&cfg, move |url| {
        use tauri_plugin_shell::ShellExt;
        if let Err(e) = opener.shell().open(url, None) {
            log::error!("could not open browser for sign-in: {e}");
        }
    })
    .await
    .map_err(|e| e.to_string())?;
    app.state::<AppState>().sync.rebuild(&app, Some(Box::new(auth)));
    Ok(())
}

/// Static-token mode: store the pasted token in the OS keychain and start the engine.
/// The token never appears in config files, the DB, or logs.
#[tauri::command]
pub fn sync_set_token(app: AppHandle, state: State<AppState>, token: String) -> CmdResult<()> {
    let cfg = state.sync.config();
    cfg.validate().map_err(|e| e.to_string())?;
    if cfg.auth_mode != "token" {
        return Err("authMode is not \"token\"".into());
    }
    StaticTokenAuth::set_token(&token).map_err(|e| e.to_string())?;
    state.sync.rebuild(&app, Some(Box::new(StaticTokenAuth)));
    Ok(())
}

/// Roster of one of the signed-in identity's teams (server-attested; static-token servers
/// return the configured token list, OIDC servers return seen-members — the IdP stays the
/// authoritative source, which is also why "add member" is guidance, not an API call).
#[tauri::command]
pub async fn sync_team_members(app: AppHandle, team: String) -> CmdResult<Vec<sync_proto::Member>> {
    let engine = app
        .state::<AppState>()
        .sync
        .engine
        .lock()
        .unwrap()
        .clone()
        .ok_or("sync is not configured or signed in")?;
    engine.members(&team).await.map_err(|e| e.to_string())
}

/// Join another team on the backend already connected, by redeeming an invite with the
/// credential that's already signed in. This is what makes membership additive: a second
/// invite adds a team instead of replacing the connection (which is what applying the whole
/// invite link does). Works on managed installs too — the server stays fixed, only teams change.
#[tauri::command]
pub async fn sync_join_team(app: AppHandle, code: String) -> CmdResult<Vec<String>> {
    let engine = app
        .state::<AppState>()
        .sync
        .engine
        .lock()
        .unwrap()
        .clone()
        .ok_or("Sign in to your team backend first, then join with an invite.")?;
    // An invite arrives either as a bare code or as a whole glyphio://join link; accept both,
    // since what an admin sends depends on how they copied it.
    let code = invite_code(&code).ok_or("That doesn't look like an invite code or link.")?;
    let me = engine.redeem_invite(&code).await.map_err(|e| e.to_string())?;
    Ok(me.teams)
}

/// Leave a team: drop the membership server-side, then un-share any local group that pointed
/// at it. The group and its snippets stay — they simply become personal again, which is the
/// only non-destructive reading of "leave".
#[tauri::command]
pub async fn sync_leave_team(app: AppHandle, team: String) -> CmdResult<()> {
    let engine = app
        .state::<AppState>()
        .sync
        .engine
        .lock()
        .unwrap()
        .clone()
        .ok_or("sync is not configured or signed in")?;
    engine.leave_team(&team).await.map_err(|e| e.to_string())?;
    let store = app.state::<AppState>().snippets.clone();
    let groups = store.list_groups().map_err(|e| e.to_string())?;
    for g in groups.into_iter().filter(|g| g.team.as_deref() == Some(team.as_str())) {
        if let Err(e) = store.set_group_team(&g.id, None) {
            log::warn!("could not un-share group {} after leaving {team}: {e}", g.id);
        }
    }
    let _ = app.emit("groups-changed", ());
    Ok(())
}

/// The invite code inside whatever the user pasted: a bare code, or a `glyphio://join?...`
/// link with a `token` parameter.
fn invite_code(input: &str) -> Option<String> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(u) = url::Url::parse(s) {
        if u.scheme() == "glyphio" {
            return u
                .query_pairs()
                .find(|(k, _)| k == "token")
                .map(|(_, v)| v.into_owned())
                .filter(|t| !t.is_empty());
        }
    }
    if s.contains("://") || s.contains(char::is_whitespace) {
        return None;
    }
    Some(s.to_string())
}

#[tauri::command]
pub fn sync_sign_out(app: AppHandle, state: State<AppState>) -> CmdResult<()> {
    if let Some(t) = state.sync.task.lock().unwrap().take() {
        t.abort();
    }
    if let Some(e) = state.sync.engine.lock().unwrap().take() {
        e.sign_out(); // drops keychain secrets for the active mode
    } else {
        // No engine (e.g. mid-configuration) — clear both modes' secrets anyway.
        OidcAuth::resume(&state.sync.config()).sign_out();
        StaticTokenAuth.sign_out();
    }
    emit_status(
        &app,
        &SyncStatus { state: "signedOut".into(), ..Default::default() },
    );
    Ok(())
}

/// What an invite link/code proposes. Returned by [`parse_invite`] so the UI can show a
/// human-readable confirmation BEFORE anything is applied — a `glyphio://` link can be
/// triggered by any webpage, so silent application would be a sync-redirection attack.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteInfo {
    pub server: String,
    pub auth_mode: String,
    pub has_token: bool,
    pub issuer: Option<String>,
    pub client_id: Option<String>,
    /// The invite is for the backend this install already uses, so applying it only adds a
    /// team — no connection change. That's what lets a managed (locked) install join more
    /// teams within its own organization, and what stops a second invite from displacing the
    /// first on a personal one.
    pub join_only: bool,
}

/// Whether two backend URLs address the same server (ignoring a trailing slash and case).
fn same_backend(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.trim().trim_end_matches('/').to_ascii_lowercase();
    !a.trim().is_empty() && norm(a) == norm(b)
}

fn parse_invite_url(url: &str) -> Result<(InviteInfo, Option<String>), String> {
    let u = url::Url::parse(url.trim()).map_err(|_| "not a valid invite link".to_string())?;
    if u.scheme() != "glyphio" || u.host_str() != Some("join") {
        return Err("not a Glyphio invite link (expected glyphio://join?...)".into());
    }
    let q: std::collections::HashMap<_, _> = u.query_pairs().into_owned().collect();
    let server = q.get("server").cloned().ok_or("invite link has no server")?;
    let mode = q.get("mode").cloned().unwrap_or_else(|| "token".into());
    let token = q.get("token").cloned().filter(|t| !t.is_empty());
    let info = InviteInfo {
        server: server.clone(),
        auth_mode: mode.clone(),
        has_token: token.is_some(),
        issuer: q.get("issuer").cloned(),
        client_id: q.get("clientId").cloned(),
        join_only: false, // decided by the caller, which knows the current connection
    };
    // Validate the resulting config up front so a bad link fails at parse, not after apply.
    let cfg = SyncConfig {
        enabled: true,
        backend_url: server,
        auth_mode: mode,
        issuer: info.issuer.clone().unwrap_or_default(),
        client_id: info.client_id.clone().unwrap_or_default(),
        ..Default::default()
    };
    cfg.validate().map_err(|e| e.to_string())?;
    Ok((info, token))
}

/// Parse an invite link/code WITHOUT applying it (the UI confirms with the user first).
///
/// An invite for the backend we're already on is a *join*: it adds a team and leaves the
/// connection alone. That's the only kind a managed install accepts — the organization's
/// server stays fixed, but its people can still be added to more of its teams.
#[tauri::command]
pub fn parse_invite(state: State<AppState>, url: String) -> CmdResult<InviteInfo> {
    let (mut info, _) = parse_invite_url(&url)?;
    let cfg = state.sync.config();
    info.join_only = cfg.enabled && same_backend(&info.server, &cfg.backend_url);
    if state.sync.is_managed() && !info.join_only {
        return Err(
            "This install is managed by your organization — it can only join teams on your organization's server."
                .into(),
        );
    }
    Ok(info)
}

/// Apply a confirmed invite: write the sync config, store the token in the keychain, start
/// the engine. Only ever called after the user confirmed the parsed summary.
#[tauri::command]
pub async fn apply_invite(app: AppHandle, url: String) -> CmdResult<()> {
    let (info, token) = {
        let state = app.state::<AppState>();
        let (info, token) = parse_invite_url(&url)?;
        let cfg = state.sync.config();
        // Same backend → this is a join, not a reconnection: redeem the invite with the
        // credential already signed in so the teams we have are kept.
        if cfg.enabled && same_backend(&info.server, &cfg.backend_url) {
            let Some(code) = token else {
                // OIDC invite for the server we're on: membership comes from the identity
                // provider, so there is nothing to apply here.
                return Ok(());
            };
            drop(state);
            sync_join_team(app.clone(), code).await?;
            return Ok(());
        }
        if state.sync.is_managed() {
            return Err(
                "This install is managed by your organization — it can only join teams on your organization's server."
                    .into(),
            );
        }
        (info, token)
    };
    let state = app.state::<AppState>();
    let cfg = SyncConfig {
        enabled: true,
        backend_url: info.server,
        auth_mode: info.auth_mode.clone(),
        issuer: info.issuer.unwrap_or_default(),
        client_id: info.client_id.unwrap_or_default(),
        scopes: if info.auth_mode == "oidc" {
            vec!["profile".into(), "email".into(), "offline_access".into(), "groups".into()]
        } else {
            vec![]
        },
        ..Default::default()
    };
    cfg.save(&state.sync.config_path).map_err(|e| e.to_string())?;
    if let Some(t) = token {
        StaticTokenAuth::set_token(&t).map_err(|e| e.to_string())?;
        state.sync.rebuild(&app, Some(Box::new(StaticTokenAuth)));
    } else {
        state.sync.rebuild(&app, None); // OIDC invite: user clicks Sign in next
    }
    Ok(())
}

/// Assign (or clear) the team a group syncs with — the primary UX for sharing snippets:
/// the group's member snippets follow it.
#[tauri::command]
pub fn set_group_team(
    app: AppHandle,
    state: State<AppState>,
    id: String,
    team: Option<String>,
) -> CmdResult<()> {
    state
        .snippets
        .set_group_team(&id, team.as_deref())
        .map_err(|e| e.to_string())?;
    let _ = app.emit("groups-changed", ());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invite_codes_come_from_links_or_bare_codes() {
        let code = "a".repeat(64);
        // What an admin dashboard hands out, and what a link contains.
        assert_eq!(invite_code(&code).as_deref(), Some(code.as_str()));
        assert_eq!(invite_code(&format!("  {code}\n")).as_deref(), Some(code.as_str()));
        assert_eq!(
            invite_code(&format!("glyphio://join?server=https://s.example.com&token={code}")).as_deref(),
            Some(code.as_str())
        );
        // Nothing usable: an OIDC invite carries no token, and a stray URL isn't a code.
        assert!(invite_code("glyphio://join?server=https://s.example.com&mode=oidc").is_none());
        assert!(invite_code("https://example.com/join").is_none());
        assert!(invite_code("   ").is_none());
    }

    #[test]
    fn backend_comparison_ignores_trailing_slash_and_case() {
        assert!(same_backend("https://Sync.Example.com/", "https://sync.example.com"));
        assert!(!same_backend("https://sync.example.com", "https://other.example.com"));
        // An unset backend never matches — that would make every invite look like a join.
        assert!(!same_backend("", ""));
    }
}
