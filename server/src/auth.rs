// SPDX-License-Identifier: Apache-2.0
//! Bearer authentication: static API tokens (SHA-256, constant-time compare) and/or generic
//! OIDC JWT validation against the issuer's JWKS. Both are pure environment configuration —
//! no IdP-specific code paths.
//!
//! Security invariants:
//! * Tokens are NEVER logged (not even at trace level) and never stored — static tokens exist
//!   only as SHA-256 digests in config.
//! * JWTs validate signature (RS256/ES256 via JWKS), `iss`, `aud`, `exp`.
//! * Identity/teams are derived exclusively from the validated credential; nothing in the
//!   request body influences authorization.

use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use serde::Deserialize;
use sha2::Digest;
use subtle::ConstantTimeEq;

use crate::error::ApiError;
use crate::AppState;

/// The authenticated caller, inserted into request extensions by [`auth_middleware`].
#[derive(Debug, Clone)]
pub struct Identity {
    pub sub: String,
    pub email: Option<String>,
    pub teams: Vec<String>,
    /// Role pinned in static-token config (applies to all the token's teams unless an explicit
    /// role row overrides). Always `None` for OIDC identities.
    pub pinned_role: Option<sync_proto::Role>,
    /// SHA-256 (hex) of the *stored* invite token this caller presented, when they presented
    /// one. Lets a handler recognise its own credential — redeeming the invite you're already
    /// authenticated with must not revoke it out from under the session.
    pub token_sha: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StaticToken {
    token_sha256: String,
    sub: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    teams: Vec<String>,
    /// Optional pinned role (lowercase, e.g. "reader"); default resolution applies when absent.
    #[serde(default)]
    role: Option<String>,
}

pub struct Authenticator {
    static_tokens: Vec<StaticToken>,
    oidc: Option<OidcValidator>,
}

impl Authenticator {
    pub fn from_env() -> Result<Self, String> {
        let static_tokens = match (std::env::var("STATIC_TOKENS"), std::env::var("STATIC_TOKENS_FILE")) {
            (Ok(json), _) => parse_tokens(&json)?,
            (_, Ok(path)) => {
                let json = std::fs::read_to_string(&path)
                    .map_err(|e| format!("cannot read STATIC_TOKENS_FILE {path}: {e}"))?;
                parse_tokens(&json)?
            }
            _ => Vec::new(),
        };
        let oidc = match std::env::var("OIDC_ISSUER") {
            Ok(issuer) if !issuer.is_empty() => {
                let audience = std::env::var("OIDC_AUDIENCE")
                    .map_err(|_| "OIDC_ISSUER set but OIDC_AUDIENCE missing")?;
                let teams_claim =
                    std::env::var("TEAMS_CLAIM").unwrap_or_else(|_| "groups".to_string());
                Some(OidcValidator::new(issuer, audience, teams_claim))
            }
            _ => None,
        };
        if static_tokens.is_empty() && oidc.is_none() {
            return Err(
                "no auth configured: set OIDC_ISSUER+OIDC_AUDIENCE and/or STATIC_TOKENS(_FILE)"
                    .into(),
            );
        }
        tracing::info!(
            "auth: {} static token(s){}",
            static_tokens.len(),
            if oidc.is_some() { ", OIDC enabled" } else { "" }
        );
        Ok(Self { static_tokens, oidc })
    }

    #[cfg(test)]
    pub fn for_tests(static_tokens_json: &str) -> Self {
        Self { static_tokens: parse_tokens(static_tokens_json).unwrap(), oidc: None }
    }

    /// Configured (static-token) members of `team` — the "roster" half of the members list.
    /// OIDC identities are only known once seen (the IdP owns the authoritative directory).
    pub fn static_members(&self, team: &str) -> Vec<sync_proto::Member> {
        self.static_tokens
            .iter()
            .filter(|t| t.teams.iter().any(|x| x == team))
            .map(|t| sync_proto::Member { sub: t.sub.clone(), email: t.email.clone(), last_seen: None })
            .collect()
    }

    /// Resolve a bearer credential to an identity. Order: static env tokens (bootstrap) →
    /// stored invite tokens (rejecting expired/revoked) → OIDC JWT validation.
    pub async fn authenticate(
        &self,
        bearer: &str,
        storage: &dyn crate::storage::Storage,
    ) -> Result<Identity, ApiError> {
        let digest = sha2::Sha256::digest(bearer.as_bytes());
        for t in &self.static_tokens {
            if let Ok(expected) = hex::decode(&t.token_sha256) {
                if expected.len() == 32 && digest.as_slice().ct_eq(&expected).into() {
                    return Ok(Identity {
                        sub: t.sub.clone(),
                        email: t.email.clone(),
                        teams: t.teams.clone(),
                        pinned_role: t.role.as_deref().and_then(crate::storage::role_from_str),
                        token_sha: None, // env-configured, not a stored invite
                    });
                }
            }
        }
        // Stored invite tokens: keyed directly by digest (constant lookup; the digest IS the
        // key, so no oracle beyond what any keyed store implies).
        let sha_hex = hex::encode(digest);
        if let Some(t) = storage.token_by_sha(&sha_hex).await.map_err(ApiError::Storage)? {
            if t.revoked_at.is_some() {
                return Err(ApiError::Unauthorized);
            }
            if let Some(exp) = &t.expires_at {
                if exp.as_str() < crate::storage::now_rfc3339().as_str() {
                    return Err(ApiError::Unauthorized);
                }
            }
            return Ok(Identity {
                sub: t.sub.clone(),
                email: t.email.clone(),
                teams: t.teams.clone(),
                pinned_role: t.role.as_deref().and_then(crate::storage::role_from_str),
                token_sha: Some(sha_hex),
            });
        }
        if let Some(oidc) = &self.oidc {
            return oidc.validate(bearer).await;
        }
        Err(ApiError::Unauthorized)
    }
}

fn parse_tokens(json: &str) -> Result<Vec<StaticToken>, String> {
    let tokens: Vec<StaticToken> =
        serde_json::from_str(json).map_err(|e| format!("invalid STATIC_TOKENS JSON: {e}"))?;
    for t in &tokens {
        if t.token_sha256.len() != 64 || hex::decode(&t.token_sha256).is_err() {
            return Err(format!("tokenSha256 for sub {:?} is not 64 hex chars", t.sub));
        }
        if let Some(r) = &t.role {
            if crate::storage::role_from_str(r).is_none() {
                return Err(format!(
                    "invalid role {r:?} for sub {:?} (reader|writer|manager|admin|owner)",
                    t.sub
                ));
            }
        }
    }
    Ok(tokens)
}

// ---- OIDC ---------------------------------------------------------------------

struct Jwks {
    keys: HashMap<String, jsonwebtoken::DecodingKey>,
    algs: HashMap<String, jsonwebtoken::Algorithm>,
    fetched: Instant,
}

pub struct OidcValidator {
    issuer: String,
    audience: String,
    teams_claim: String,
    http: reqwest::Client,
    jwks: RwLock<Option<Jwks>>,
}

const JWKS_TTL: Duration = Duration::from_secs(3600);

impl OidcValidator {
    fn new(issuer: String, audience: String, teams_claim: String) -> Self {
        Self {
            issuer: issuer.trim_end_matches('/').to_string(),
            audience,
            teams_claim,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("http client"),
            jwks: RwLock::new(None),
        }
    }

    async fn fetch_jwks(&self) -> Result<(), ApiError> {
        #[derive(Deserialize)]
        struct Discovery {
            jwks_uri: String,
        }
        let disc: Discovery = self
            .http
            .get(format!("{}/.well-known/openid-configuration", self.issuer))
            .send()
            .await
            .map_err(|_| ApiError::Unauthorized)?
            .json()
            .await
            .map_err(|_| ApiError::Unauthorized)?;
        let set: jsonwebtoken::jwk::JwkSet = self
            .http
            .get(&disc.jwks_uri)
            .send()
            .await
            .map_err(|_| ApiError::Unauthorized)?
            .json()
            .await
            .map_err(|_| ApiError::Unauthorized)?;
        let mut keys = HashMap::new();
        let mut algs = HashMap::new();
        for jwk in &set.keys {
            if let (Some(kid), Ok(key)) =
                (jwk.common.key_id.clone(), jsonwebtoken::DecodingKey::from_jwk(jwk))
            {
                let alg = match &jwk.algorithm {
                    jsonwebtoken::jwk::AlgorithmParameters::RSA(_) => jsonwebtoken::Algorithm::RS256,
                    jsonwebtoken::jwk::AlgorithmParameters::EllipticCurve(_) => {
                        jsonwebtoken::Algorithm::ES256
                    }
                    _ => continue,
                };
                keys.insert(kid.clone(), key);
                algs.insert(kid, alg);
            }
        }
        tracing::debug!("fetched {} JWKS key(s)", keys.len());
        *self.jwks.write().unwrap() = Some(Jwks { keys, algs, fetched: Instant::now() });
        Ok(())
    }

    async fn key_for(
        &self,
        kid: &str,
    ) -> Result<(jsonwebtoken::DecodingKey, jsonwebtoken::Algorithm), ApiError> {
        // Serve from cache when fresh and the kid is known; otherwise (re)fetch —
        // this also covers IdP key rotation (unknown kid → refetch once).
        let need_fetch = {
            let guard = self.jwks.read().unwrap();
            match guard.as_ref() {
                Some(j) => j.fetched.elapsed() > JWKS_TTL || !j.keys.contains_key(kid),
                None => true,
            }
        };
        if need_fetch {
            self.fetch_jwks().await?;
        }
        let guard = self.jwks.read().unwrap();
        let jwks = guard.as_ref().ok_or(ApiError::Unauthorized)?;
        match (jwks.keys.get(kid), jwks.algs.get(kid)) {
            (Some(k), Some(a)) => Ok((k.clone(), *a)),
            _ => Err(ApiError::Unauthorized),
        }
    }

    async fn validate(&self, jwt: &str) -> Result<Identity, ApiError> {
        let header = jsonwebtoken::decode_header(jwt).map_err(|_| ApiError::Unauthorized)?;
        let kid = header.kid.ok_or(ApiError::Unauthorized)?;
        let (key, alg) = self.key_for(&kid).await?;

        let mut validation = jsonwebtoken::Validation::new(alg);
        validation.set_issuer(&[&self.issuer]);
        validation.set_audience(&[&self.audience]);
        validation.validate_exp = true;

        let data = jsonwebtoken::decode::<serde_json::Value>(jwt, &key, &validation)
            .map_err(|e| {
                tracing::debug!("JWT rejected: {}", e.kind_str());
                ApiError::Unauthorized
            })?;
        let claims = data.claims;
        let sub = claims
            .get("sub")
            .and_then(|v| v.as_str())
            .ok_or(ApiError::Unauthorized)?
            .to_string();
        let email = claims.get("email").and_then(|v| v.as_str()).map(String::from);
        let teams = claims
            .get(&self.teams_claim)
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default();
        Ok(Identity { sub, email, teams, pinned_role: None, token_sha: None })
    }
}

/// Small extension so JWT rejection reasons can be logged without the token or claims.
trait KindStr {
    fn kind_str(&self) -> &'static str;
}
impl KindStr for jsonwebtoken::errors::Error {
    fn kind_str(&self) -> &'static str {
        use jsonwebtoken::errors::ErrorKind::*;
        match self.kind() {
            InvalidToken => "malformed",
            InvalidSignature => "bad signature",
            ExpiredSignature => "expired",
            InvalidIssuer => "wrong issuer",
            InvalidAudience => "wrong audience",
            _ => "invalid",
        }
    }
}

// ---- middleware -----------------------------------------------------------------

/// Rate-limits by credential hash, authenticates, and injects [`Identity`].
pub async fn auth_middleware(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let bearer = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(ApiError::Unauthorized)?
        .to_string();

    // Rate limit before signature verification: cheap hash, and it also shields the
    // JWKS/verify path from being hammered with garbage credentials.
    let digest: [u8; 32] = sha2::Sha256::digest(bearer.as_bytes()).into();
    state.limiter.check(digest)?;

    let identity = state.auth.authenticate(&bearer, state.storage.as_ref()).await?;
    req.extensions_mut().insert(identity);
    Ok(next.run(req).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn static_token_auth_matches_sha_and_rejects_unknown() {
        let token = "s3cret-token";
        let sha = hex::encode(sha2::Sha256::digest(token.as_bytes()));
        let auth = Authenticator::for_tests(&format!(
            r#"[{{"tokenSha256":"{sha}","sub":"u1","teams":["a","b"]}}]"#
        ));
        let dir = tempfile::tempdir().unwrap();
        let store = crate::storage::sqlite::SqliteStorage::open(
            dir.path().join("t.db").to_str().unwrap(),
        )
        .unwrap();
        let id = auth.authenticate(token, &store).await.unwrap();
        assert_eq!(id.sub, "u1");
        assert_eq!(id.teams, vec!["a", "b"]);
        assert!(auth.authenticate("wrong", &store).await.is_err());
    }

    #[test]
    fn token_config_requires_64_hex() {
        assert!(parse_tokens(r#"[{"tokenSha256":"zz","sub":"u"}]"#).is_err());
    }
}
