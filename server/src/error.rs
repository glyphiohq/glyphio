// SPDX-License-Identifier: Apache-2.0
//! RFC 7807 problem+json error responses (`sync_proto::Problem`).

use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("missing or invalid credential")]
    Unauthorized,
    #[error("not a member of this team")]
    Forbidden,
    #[error("team archived")]
    Archived,
    /// Deliberately generic 403 — used where a specific reason would confirm the existence
    /// of something the caller isn't allowed to see (e.g. restricted groups).
    #[error("forbidden")]
    Denied,
    #[error("{0}")]
    Validation(String),
    #[error("rate limit exceeded")]
    RateLimited { retry_after_secs: u64 },
    #[error("storage error")]
    Storage(#[source] anyhow_like::Error),
}

/// Tiny boxed-error alias so storage impls can bubble anything without a dep on anyhow.
pub mod anyhow_like {
    pub type Error = Box<dyn std::error::Error + Send + Sync>;
}

impl ApiError {
    fn status(&self) -> StatusCode {
        match self {
            ApiError::Unauthorized => StatusCode::UNAUTHORIZED,
            ApiError::Forbidden => StatusCode::FORBIDDEN,
            ApiError::Archived => StatusCode::FORBIDDEN,
            ApiError::Denied => StatusCode::FORBIDDEN,
            ApiError::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
            ApiError::RateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
            ApiError::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();
        // Never leak internals: storage errors log server-side, clients get a generic title.
        if let ApiError::Storage(e) = &self {
            tracing::error!("storage error: {e}");
        }
        let problem = sync_proto::Problem {
            title: match &self {
                ApiError::Storage(_) => "internal error".to_string(),
                other => other.to_string(),
            },
            status: status.as_u16(),
            detail: match &self {
                ApiError::Validation(d) => Some(d.clone()),
                ApiError::Archived => Some("team archived".into()),
                _ => None,
            },
        };
        let mut resp = (
            status,
            [(header::CONTENT_TYPE, "application/problem+json")],
            serde_json::to_string(&problem).unwrap_or_else(|_| "{}".into()),
        )
            .into_response();
        if let ApiError::RateLimited { retry_after_secs } = self {
            if let Ok(v) = retry_after_secs.to_string().parse() {
                resp.headers_mut().insert(header::RETRY_AFTER, v);
            }
        }
        resp
    }
}
