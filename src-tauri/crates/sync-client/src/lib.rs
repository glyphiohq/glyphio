//! Glyphio sync client.
//!
//! Three pluggable layers, per the Phase 2 design:
//! * [`auth`] — [`auth::AuthProvider`]: how we obtain a bearer credential. Implementations:
//!   generic OIDC Authorization Code + PKCE (any compliant IdP — Okta, Entra, Auth0, Keycloak,
//!   Google — pure config, no provider-specific code) and a static API token for self-hosted
//!   backends with no IdP. Secrets live in the OS keychain, never on disk or in logs.
//! * [`http`] — [`http::SyncProvider`]: how we reach a backend. The default [`http::HttpSync`]
//!   speaks the v1 wire protocol (`sync-proto`) over HTTPS; a future file/S3/WebDAV backend is
//!   a new impl, not an engine change.
//! * [`engine`] — [`engine::SyncEngine`]: the actual sync loop, a *consumer* of
//!   `snippet_store::SnippetStore` (change-listener → debounced push; pulls applied through the
//!   store so YAML regeneration and UI refresh happen for free).
//!
//! Hard scope rule, enforced here and not just by convention: only records whose `team` is one
//! of the authenticated identity's teams are ever serialized to the wire. `personal` snippets
//! and screenshot history have **no code path** into this crate.

pub mod auth;
pub mod engine;
pub mod http;

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("sync is not configured")]
    NotConfigured,
    #[error("not signed in")]
    SignedOut,
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("the server rejected the credential (sign in again)")]
    Unauthorized,
    #[error("network error: {0}")]
    Network(String),
    #[error("server error ({status}): {detail}")]
    Server { status: u16, detail: String },
    #[error("store error: {0}")]
    Store(#[from] snippet_store::StoreError),
    #[error("keychain error: {0}")]
    Keychain(String),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, SyncError>;

/// Runtime sync configuration. Loaded from `sync.toml` in the app data dir — **never** compiled
/// in, so any deployment (an enterprise Okta+AWS or a self-hosted Keycloak+VPS) is pure configuration.
/// Contains no secrets: tokens live in the OS keychain.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct SyncConfig {
    /// Master switch. Off (the default) = the app makes zero network calls.
    pub enabled: bool,
    /// Base URL of a backend speaking the v1 sync protocol, e.g. `https://sync.example.com`.
    pub backend_url: String,
    /// `oidc` or `token`.
    pub auth_mode: String,
    /// OIDC issuer URL (discovery is derived from it), e.g. `https://acme.okta.com`.
    pub issuer: String,
    /// OIDC public-client ID.
    pub client_id: String,
    /// Extra scopes beyond `openid` (e.g. `profile email offline_access groups`).
    pub scopes: Vec<String>,
    /// Optional `audience` parameter for IdPs that require it (e.g. Auth0).
    pub audience: String,
    /// Fixed loopback-redirect port for IdPs that require an exact redirect URI; 0 = ephemeral.
    pub redirect_port: u16,
    /// Background sync interval in seconds (min 60; default 300).
    pub interval_secs: u64,
}

impl SyncConfig {
    pub fn load(path: &std::path::Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        let text = toml::to_string_pretty(self).map_err(|e| SyncError::Other(e.to_string()))?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).map_err(|e| SyncError::Other(e.to_string()))?;
        std::fs::rename(&tmp, path).map_err(|e| SyncError::Other(e.to_string()))?;
        Ok(())
    }

    pub fn interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.interval_secs.clamp(60, 24 * 3600))
    }

    pub fn validate(&self) -> Result<()> {
        if !self.enabled {
            return Err(SyncError::NotConfigured);
        }
        let url = url::Url::parse(&self.backend_url)
            .map_err(|_| SyncError::Other("backendUrl is not a valid URL".into()))?;
        // TLS is non-negotiable except for local development/loopback testing.
        let loopback = matches!(url.host_str(), Some("127.0.0.1") | Some("localhost") | Some("[::1]"));
        if url.scheme() != "https" && !loopback {
            return Err(SyncError::Other("backendUrl must be https (plain http is only allowed for 127.0.0.1/localhost testing)".into()));
        }
        match self.auth_mode.as_str() {
            "oidc" => {
                if self.issuer.is_empty() || self.client_id.is_empty() {
                    return Err(SyncError::Other("OIDC mode requires issuer and clientId".into()));
                }
                let iss = url::Url::parse(&self.issuer)
                    .map_err(|_| SyncError::Other("issuer is not a valid URL".into()))?;
                if iss.scheme() != "https" {
                    return Err(SyncError::Other("issuer must be https".into()));
                }
            }
            "token" => {}
            other => {
                return Err(SyncError::Other(format!(
                    "authMode must be \"oidc\" or \"token\" (got {other:?})"
                )))
            }
        }
        Ok(())
    }
}

/// Sync state surfaced to the UI. Never contains a token.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    /// `disabled` | `signedOut` | `idle` | `syncing` | `error`
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<sync_proto::Me>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_sync: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_validation_enforces_https_and_modes() {
        let mut c = SyncConfig { enabled: true, backend_url: "http://example.com".into(), auth_mode: "token".into(), ..Default::default() };
        assert!(c.validate().is_err(), "plain http to a real host must be rejected");
        c.backend_url = "http://127.0.0.1:8787".into();
        assert!(c.validate().is_ok(), "loopback http is allowed for local testing");
        c.backend_url = "https://sync.example.com".into();
        assert!(c.validate().is_ok());
        c.auth_mode = "oidc".into();
        assert!(c.validate().is_err(), "oidc requires issuer/clientId");
        c.issuer = "https://acme.okta.com".into();
        c.client_id = "abc".into();
        assert!(c.validate().is_ok());
        c.auth_mode = "bogus".into();
        assert!(c.validate().is_err());
    }

    #[test]
    fn config_roundtrips_toml_without_secrets() {
        let c = SyncConfig {
            enabled: true,
            backend_url: "https://sync.example.com".into(),
            auth_mode: "oidc".into(),
            issuer: "https://acme.okta.com".into(),
            client_id: "abc".into(),
            scopes: vec!["profile".into(), "offline_access".into()],
            ..Default::default()
        };
        let t = toml::to_string_pretty(&c).unwrap();
        assert_eq!(toml::from_str::<SyncConfig>(&t).unwrap(), c);
    }
}
