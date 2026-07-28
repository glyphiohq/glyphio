//! Pluggable authentication.
//!
//! [`AuthProvider`] yields a bearer credential for the sync backend; identity/teams always come
//! from the backend's `/v1/me` (derived server-side from the validated credential), so the
//! client never trusts its own parsing of a token for authorization.
//!
//! * [`OidcAuth`] — generic OIDC **Authorization Code + PKCE (S256)** for native apps
//!   (RFC 8252): system browser + one-shot loopback redirect on `127.0.0.1`, `state` checked,
//!   `nonce` verified, ID token signature/`iss`/`aud`/`exp` validated via the IdP's JWKS
//!   (all through the `openidconnect` crate — no hand-rolled JWT code). The **ID token** is the
//!   bearer sent to the backend (portable across IdPs: every OIDC IdP issues signed JWT ID
//!   tokens, while access-token formats vary); it is refreshed silently via the refresh token.
//! * [`StaticTokenAuth`] — a pasted API token for self-hosted backends with no IdP.
//!
//! Secret storage: **OS keychain only** (`keyring` crate — macOS Keychain / Windows Credential
//! Manager / Secret Service). Nothing token-like ever touches disk, the SQLite DB, or logs.

use std::sync::Mutex;

use async_trait::async_trait;
use base64::Engine as _;
use chrono::{DateTime, Duration, Utc};
use openidconnect::core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata};
use openidconnect::reqwest::async_http_client;
use openidconnect::{
    AuthorizationCode, ClientId, CsrfToken, IssuerUrl, Nonce, OAuth2TokenResponse,
    PkceCodeChallenge, RedirectUrl, RefreshToken, Scope, TokenResponse,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::{Result, SyncConfig, SyncError};

/// Keychain service name for all Glyphio sync secrets.
const KEYCHAIN_SERVICE: &str = "io.glyphio.sync";
const KEY_REFRESH_TOKEN: &str = "oidc-refresh-token";
const KEY_STATIC_TOKEN: &str = "static-token";

/// How long before expiry we proactively refresh.
const REFRESH_SKEW_SECS: i64 = 120;
/// How long we wait for the user to complete the browser flow.
const SIGN_IN_TIMEOUT_SECS: u64 = 300;

#[async_trait]
pub trait AuthProvider: Send + Sync {
    /// A currently-valid bearer credential, silently refreshed when possible.
    async fn bearer(&self) -> Result<String>;
    /// Drop local session state and stored secrets.
    fn sign_out(&self);
    fn kind(&self) -> &'static str;
}

fn keychain(key: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(KEYCHAIN_SERVICE, key).map_err(|e| SyncError::Keychain(e.to_string()))
}

fn keychain_get(key: &str) -> Result<Option<String>> {
    match keychain(key)?.get_password() {
        Ok(v) => Ok(Some(v)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(SyncError::Keychain(e.to_string())),
    }
}

fn keychain_set(key: &str, value: &str) -> Result<()> {
    keychain(key)?.set_password(value).map_err(|e| SyncError::Keychain(e.to_string()))
}

fn keychain_delete(key: &str) {
    if let Ok(e) = keychain(key) {
        let _ = e.delete_credential();
    }
}

// ---- static token ------------------------------------------------------------

pub struct StaticTokenAuth;

impl StaticTokenAuth {
    /// Store a pasted token in the keychain (replacing any previous one).
    pub fn set_token(token: &str) -> Result<()> {
        let token = token.trim();
        if token.is_empty() {
            return Err(SyncError::Auth("token is empty".into()));
        }
        keychain_set(KEY_STATIC_TOKEN, token)
    }

    pub fn has_token() -> bool {
        matches!(keychain_get(KEY_STATIC_TOKEN), Ok(Some(_)))
    }
}

#[async_trait]
impl AuthProvider for StaticTokenAuth {
    async fn bearer(&self) -> Result<String> {
        keychain_get(KEY_STATIC_TOKEN)?.ok_or(SyncError::SignedOut)
    }

    fn sign_out(&self) {
        keychain_delete(KEY_STATIC_TOKEN);
    }

    fn kind(&self) -> &'static str {
        "token"
    }
}

// ---- OIDC PKCE ---------------------------------------------------------------

struct OidcSession {
    id_token: String,
    expires_at: DateTime<Utc>,
}

pub struct OidcAuth {
    cfg: SyncConfig,
    session: Mutex<Option<OidcSession>>,
}

impl OidcAuth {
    /// Provider that resumes silently from a stored refresh token (if any). Cheap to construct;
    /// the first `bearer()` call performs the actual refresh.
    pub fn resume(cfg: &SyncConfig) -> Self {
        Self { cfg: cfg.clone(), session: Mutex::new(None) }
    }

    /// Whether a refresh token exists in the keychain (i.e. a previous sign-in to resume).
    pub fn has_session() -> bool {
        matches!(keychain_get(KEY_REFRESH_TOKEN), Ok(Some(_)))
    }

    async fn oidc_client(cfg: &SyncConfig, redirect: Option<String>) -> Result<CoreClient> {
        let issuer =
            IssuerUrl::new(cfg.issuer.clone()).map_err(|e| SyncError::Auth(e.to_string()))?;
        let meta = CoreProviderMetadata::discover_async(issuer, async_http_client)
            .await
            .map_err(|e| SyncError::Auth(format!("OIDC discovery failed: {e}")))?;
        let mut client =
            CoreClient::from_provider_metadata(meta, ClientId::new(cfg.client_id.clone()), None);
        if let Some(r) = redirect {
            client = client.set_redirect_uri(
                RedirectUrl::new(r).map_err(|e| SyncError::Auth(e.to_string()))?,
            );
        }
        Ok(client)
    }

    /// Run the full interactive sign-in: browser + loopback redirect. `open_url` is called with
    /// the authorization URL (the app opens the system browser — this crate stays UI-free).
    pub async fn sign_in(cfg: &SyncConfig, open_url: impl FnOnce(String)) -> Result<Self> {
        // One-shot loopback listener (RFC 8252 §7.3). Port 0 = OS-assigned ephemeral port;
        // a fixed `redirect_port` supports IdPs that require an exact, pre-registered URI.
        let listener = TcpListener::bind(("127.0.0.1", cfg.redirect_port))
            .await
            .map_err(|e| SyncError::Auth(format!("cannot bind loopback listener: {e}")))?;
        let port = listener.local_addr().map_err(|e| SyncError::Auth(e.to_string()))?.port();
        let redirect = format!("http://127.0.0.1:{port}/callback");

        let client = Self::oidc_client(cfg, Some(redirect)).await?;
        let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
        let mut auth = client
            .authorize_url(
                CoreAuthenticationFlow::AuthorizationCode,
                CsrfToken::new_random,
                Nonce::new_random,
            )
            .set_pkce_challenge(pkce_challenge);
        for s in &cfg.scopes {
            auth = auth.add_scope(Scope::new(s.clone()));
        }
        if !cfg.audience.is_empty() {
            auth = auth.add_extra_param("audience", cfg.audience.clone());
        }
        let (auth_url, csrf, nonce) = auth.url();

        open_url(auth_url.to_string());

        // Wait (bounded) for the browser to hit the loopback redirect.
        let (code, returned_state) = tokio::time::timeout(
            std::time::Duration::from_secs(SIGN_IN_TIMEOUT_SECS),
            wait_for_callback(listener),
        )
        .await
        .map_err(|_| SyncError::Auth("sign-in timed out (5 minutes)".into()))??;

        // CSRF check: the `state` round-trip must match exactly.
        if returned_state != *csrf.secret() {
            return Err(SyncError::Auth("state mismatch — possible CSRF, aborting".into()));
        }

        let tokens = client
            .exchange_code(AuthorizationCode::new(code))
            .set_pkce_verifier(pkce_verifier)
            .request_async(async_http_client)
            .await
            .map_err(|e| SyncError::Auth(format!("code exchange failed: {e}")))?;

        // Full ID-token validation: signature (JWKS), iss, aud, exp — and the nonce we sent.
        let id_token =
            tokens.id_token().ok_or_else(|| SyncError::Auth("IdP returned no ID token".into()))?;
        let claims = id_token
            .claims(&client.id_token_verifier(), &nonce)
            .map_err(|e| SyncError::Auth(format!("ID token validation failed: {e}")))?;
        let expires_at = claims.expiration();

        if let Some(rt) = tokens.refresh_token() {
            keychain_set(KEY_REFRESH_TOKEN, rt.secret())?;
        } else {
            log::warn!(
                "IdP returned no refresh token (add the offline_access scope?) — \
                 sync will require sign-in again when the session expires"
            );
        }

        Ok(Self {
            cfg: cfg.clone(),
            session: Mutex::new(Some(OidcSession {
                id_token: id_token.to_string(),
                expires_at,
            })),
        })
    }

    async fn refresh(&self) -> Result<OidcSession> {
        let refresh_token = keychain_get(KEY_REFRESH_TOKEN)?.ok_or(SyncError::SignedOut)?;
        let client = Self::oidc_client(&self.cfg, None).await?;
        let tokens = client
            .exchange_refresh_token(&RefreshToken::new(refresh_token))
            .request_async(async_http_client)
            .await
            .map_err(|e| {
                // A rejected refresh token means the session was revoked → signed out.
                log::info!("token refresh rejected; dropping to signed-out");
                let _ = e; // error details may embed request context; don't log them verbatim
                SyncError::SignedOut
            })?;
        let id_token =
            tokens.id_token().ok_or_else(|| SyncError::Auth("refresh returned no ID token".into()))?;
        // No nonce on a refresh grant (there was no authorization request to carry one);
        // signature/iss/aud/exp still fully verify against the JWKS.
        let claims = id_token
            .claims(&client.id_token_verifier(), |_: Option<&Nonce>| Ok(()))
            .map_err(|e| SyncError::Auth(format!("refreshed ID token invalid: {e}")))?;
        let expires_at = claims.expiration();
        // IdPs that rotate refresh tokens return a new one — persist it.
        if let Some(rt) = tokens.refresh_token() {
            keychain_set(KEY_REFRESH_TOKEN, rt.secret())?;
        }
        Ok(OidcSession { id_token: id_token.to_string(), expires_at })
    }
}

#[async_trait]
impl AuthProvider for OidcAuth {
    async fn bearer(&self) -> Result<String> {
        {
            let s = self.session.lock().unwrap();
            if let Some(sess) = s.as_ref() {
                if sess.expires_at - Duration::seconds(REFRESH_SKEW_SECS) > Utc::now() {
                    return Ok(sess.id_token.clone());
                }
            }
        }
        let fresh = self.refresh().await?;
        let token = fresh.id_token.clone();
        *self.session.lock().unwrap() = Some(fresh);
        Ok(token)
    }

    fn sign_out(&self) {
        *self.session.lock().unwrap() = None;
        keychain_delete(KEY_REFRESH_TOKEN);
    }

    fn kind(&self) -> &'static str {
        "oidc"
    }
}

/// Accept exactly one loopback HTTP request and extract `code` + `state` from its query string.
/// Serves a tiny "you can close this tab" page. Deliberately minimal: it binds to 127.0.0.1,
/// reads a single request line, and never interprets anything beyond the two query params.
async fn wait_for_callback(listener: TcpListener) -> Result<(String, String)> {
    loop {
        let (mut stream, peer) =
            listener.accept().await.map_err(|e| SyncError::Auth(e.to_string()))?;
        if !peer.ip().is_loopback() {
            continue; // paranoid: bound to 127.0.0.1, but verify anyway
        }
        let mut buf = vec![0u8; 8192];
        let n = stream.read(&mut buf).await.unwrap_or(0);
        let req = String::from_utf8_lossy(&buf[..n]);
        let path = req.lines().next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or("");
        // Browsers also request /favicon.ico — only handle the callback path.
        if !path.starts_with("/callback") {
            let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n").await;
            continue;
        }
        let query = path.split_once('?').map(|x| x.1).unwrap_or("");
        let mut code = None;
        let mut state = None;
        let mut error = None;
        for pair in query.split('&') {
            let mut it = pair.splitn(2, '=');
            let k = it.next().unwrap_or("");
            let v = url_decode(it.next().unwrap_or(""));
            match k {
                "code" => code = Some(v),
                "state" => state = Some(v),
                "error" => error = Some(v),
                _ => {}
            }
        }
        let body = if error.is_none() && code.is_some() {
            "<html><body style=\"font-family:system-ui;background:#0f172a;color:#e2e8f0;\
             display:grid;place-items:center;height:100vh;margin:0\"><div style=\"text-align:center\">\
             <h2>Signed in to Glyphio</h2><p>You can close this tab and return to the app.</p>\
             </div></body></html>"
        } else {
            "<html><body style=\"font-family:system-ui;background:#0f172a;color:#e2e8f0;\
             display:grid;place-items:center;height:100vh;margin:0\"><div style=\"text-align:center\">\
             <h2>Sign-in failed</h2><p>Return to Glyphio and try again.</p></div></body></html>"
        };
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = stream.write_all(resp.as_bytes()).await;
        let _ = stream.shutdown().await;

        if let Some(e) = error {
            return Err(SyncError::Auth(format!("IdP returned error: {e}")));
        }
        match (code, state) {
            (Some(c), Some(s)) => return Ok((c, s)),
            _ => return Err(SyncError::Auth("callback missing code/state".into())),
        }
    }
}

fn url_decode(s: &str) -> String {
    // Minimal percent-decoding (auth codes/state are URL-safe; this covers %XX and '+').
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Best-effort peek at a JWT's payload for **display only** (e.g. showing which account is
/// connected before `/v1/me` responds). Never used for authorization decisions.
pub fn peek_claims(jwt: &str) -> Option<serde_json::Value> {
    let payload = jwt.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload).ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_decode_handles_percent_and_plus() {
        assert_eq!(url_decode("a%2Fb+c"), "a/b c");
        assert_eq!(url_decode("plain"), "plain");
        assert_eq!(url_decode("bad%2"), "bad%2");
    }

    #[test]
    fn peek_claims_reads_payload() {
        // header.payload.sig with payload {"sub":"u1"}
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"{\"sub\":\"u1\"}");
        let jwt = format!("x.{payload}.y");
        assert_eq!(peek_claims(&jwt).unwrap()["sub"], "u1");
    }
}
