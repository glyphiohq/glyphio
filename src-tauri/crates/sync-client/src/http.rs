//! Backend transport. [`SyncProvider`] is the seam: the engine only knows these three calls,
//! so a file/S3/WebDAV backend later is a new impl here, never an engine change.

use async_trait::async_trait;
use sync_proto::{Changes, Me, Members, Push, PushAck};

use crate::{Result, SyncError};

#[async_trait]
pub trait SyncProvider: Send + Sync {
    async fn me(&self, bearer: &str) -> Result<Me>;
    async fn changes(&self, bearer: &str, team: &str, since: u64, limit: usize) -> Result<Changes>;
    async fn push(&self, bearer: &str, team: &str, batch: &Push) -> Result<PushAck>;
    /// Team roster (additive in protocol v1) — default empty so providers without a roster
    /// concept (files, object stores) stay valid implementations.
    async fn members(&self, _bearer: &str, _team: &str) -> Result<Members> {
        Ok(Members::default())
    }

    /// Redeem an invite for the signed-in identity, adding its team to whatever they already
    /// have. Returns the refreshed identity. Backends without a membership concept say so.
    async fn redeem_invite(&self, _bearer: &str, _code: &str) -> Result<Me> {
        Err(SyncError::Other("this backend does not support joining teams".into()))
    }

    /// Give up membership of one team.
    async fn leave_team(&self, _bearer: &str, _team: &str) -> Result<()> {
        Err(SyncError::Other("this backend does not support leaving teams".into()))
    }
}

/// HTTPS implementation of the v1 wire protocol (`docs/SYNC-PROTOCOL.md`).
pub struct HttpSync {
    base: url::Url,
    client: reqwest::Client,
}

impl HttpSync {
    pub fn new(backend_url: &str) -> Result<Self> {
        let mut base = url::Url::parse(backend_url)
            .map_err(|_| SyncError::Other("invalid backend URL".into()))?;
        // `Url::join` treats a path without a trailing slash as a file and replaces it —
        // normalize so `https://host/api` + `v1/me` = `https://host/api/v1/me`.
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent(concat!("glyphio-sync/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| SyncError::Network(e.to_string()))?;
        Ok(Self { base, client })
    }

    fn url(&self, path: &str) -> Result<url::Url> {
        self.base.join(path).map_err(|e| SyncError::Other(e.to_string()))
    }

    async fn handle<T: serde::de::DeserializeOwned>(resp: reqwest::Response) -> Result<T> {
        let status = resp.status();
        if status.is_success() {
            return resp.json::<T>().await.map_err(|e| SyncError::Network(e.to_string()));
        }
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(SyncError::Unauthorized);
        }
        // Try the RFC 7807 problem body; fall back to the bare status.
        let detail = resp
            .json::<sync_proto::Problem>()
            .await
            .ok()
            .and_then(|p| p.detail.or(Some(p.title)))
            .unwrap_or_else(|| status.canonical_reason().unwrap_or("request failed").to_string());
        Err(SyncError::Server { status: status.as_u16(), detail })
    }
}

#[async_trait]
impl SyncProvider for HttpSync {
    async fn me(&self, bearer: &str) -> Result<Me> {
        let resp = self
            .client
            .get(self.url("v1/me")?)
            .bearer_auth(bearer)
            .send()
            .await
            .map_err(|e| SyncError::Network(e.to_string()))?;
        Self::handle(resp).await
    }

    async fn changes(&self, bearer: &str, team: &str, since: u64, limit: usize) -> Result<Changes> {
        let mut url = self.url(&format!("v1/teams/{}/changes", urlencode(team)))?;
        url.query_pairs_mut()
            .append_pair("since", &since.to_string())
            .append_pair("limit", &limit.to_string());
        let resp = self
            .client
            .get(url)
            .bearer_auth(bearer)
            .send()
            .await
            .map_err(|e| SyncError::Network(e.to_string()))?;
        Self::handle(resp).await
    }

    async fn push(&self, bearer: &str, team: &str, batch: &Push) -> Result<PushAck> {
        let resp = self
            .client
            .post(self.url(&format!("v1/teams/{}/changes", urlencode(team)))?)
            .bearer_auth(bearer)
            .json(batch)
            .send()
            .await
            .map_err(|e| SyncError::Network(e.to_string()))?;
        Self::handle(resp).await
    }

    async fn members(&self, bearer: &str, team: &str) -> Result<Members> {
        let resp = self
            .client
            .get(self.url(&format!("v1/teams/{}/members", urlencode(team)))?)
            .bearer_auth(bearer)
            .send()
            .await
            .map_err(|e| SyncError::Network(e.to_string()))?;
        Self::handle(resp).await
    }

    async fn redeem_invite(&self, bearer: &str, code: &str) -> Result<Me> {
        let resp = self
            .client
            .post(self.url("v1/invites/redeem")?)
            .bearer_auth(bearer)
            .json(&serde_json::json!({ "code": code }))
            .send()
            .await
            .map_err(|e| SyncError::Network(e.to_string()))?;
        Self::handle(resp).await
    }

    async fn leave_team(&self, bearer: &str, team: &str) -> Result<()> {
        let resp = self
            .client
            .delete(self.url(&format!("v1/teams/{}/membership", urlencode(team)))?)
            .bearer_auth(bearer)
            .send()
            .await
            .map_err(|e| SyncError::Network(e.to_string()))?;
        Self::handle::<serde_json::Value>(resp).await.map(|_| ())
    }
}

fn urlencode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}
