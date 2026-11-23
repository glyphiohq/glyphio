// SPDX-License-Identifier: Apache-2.0
//! First-run setup in the browser. While no owner exists, the server prints a one-time code
//! to its log at startup, and `/setup` creates the first team and owner for whoever presents
//! it: reading the log already proves control of the deployment. Once an owner exists, the
//! page and endpoint are gone for good (404), and the code is forgotten.

use std::sync::Mutex;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;

use crate::error::ApiError;
use crate::ratelimit::RateLimiter;
use crate::AppState;

/// Attempts per minute across all callers. Global on purpose: the code is the only secret,
/// so the limit has to bound guesses in total, not per address.
const ATTEMPTS_PER_MIN: u32 = 10;

pub struct Setup {
    code: Mutex<Option<String>>,
    limiter: RateLimiter,
}

impl Setup {
    /// No code: setup is unavailable (an owner exists, or tests that don't exercise it).
    pub fn disabled() -> Self {
        Self { code: Mutex::new(None), limiter: RateLimiter::new(ATTEMPTS_PER_MIN) }
    }

    pub fn with_code(code: String) -> Self {
        Self { code: Mutex::new(Some(code)), limiter: RateLimiter::new(ATTEMPTS_PER_MIN) }
    }

    /// A fresh code: 10 random bytes as five groups of four hex digits.
    pub fn new_code() -> String {
        use rand::RngCore;
        let mut bytes = [0u8; 10];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        hex::encode(bytes).as_bytes().chunks(4).map(|c| std::str::from_utf8(c).unwrap()).collect::<Vec<_>>().join("-")
    }

    fn matches(&self, presented: &str) -> bool {
        let guard = self.code.lock().unwrap();
        let Some(code) = guard.as_deref() else { return false };
        let normalize = |s: &str| s.trim().to_ascii_lowercase().replace([' ', '-'], "");
        normalize(presented).as_bytes().ct_eq(normalize(code).as_bytes()).into()
    }

    fn forget(&self) {
        *self.code.lock().unwrap() = None;
    }

    fn active(&self) -> bool {
        self.code.lock().unwrap().is_some()
    }
}

async fn available(state: &AppState) -> Result<bool, ApiError> {
    if !state.setup.active() {
        return Ok(false);
    }
    if state.storage.owner_exists().await.map_err(ApiError::Storage)? {
        state.setup.forget(); // someone ran `init` meanwhile
        return Ok(false);
    }
    Ok(true)
}

fn gone() -> Response {
    (StatusCode::NOT_FOUND, "Setup is finished. Sign in at /admin.").into_response()
}

pub async fn page(State(state): State<AppState>) -> Result<Response, ApiError> {
    if !available(&state).await? {
        return Ok(gone());
    }
    Ok(Html(include_str!("setup.html")).into_response())
}

#[derive(Deserialize)]
pub struct SetupBody {
    pub code: String,
    pub team: String,
    #[serde(default)]
    pub sub: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
}

#[derive(Serialize)]
pub struct SetupResponse {
    pub team: String,
    pub sub: String,
    /// The owner's token, shown once.
    pub token: String,
}

pub async fn create(
    State(state): State<AppState>,
    Json(body): Json<SetupBody>,
) -> Result<Response, ApiError> {
    state.setup.limiter.check([0u8; 32])?;
    if !available(&state).await? {
        return Ok(gone());
    }
    if !state.setup.matches(&body.code) {
        tracing::warn!("setup: wrong code presented");
        return Err(ApiError::Unauthorized);
    }
    let args = crate::cli::InitArgs {
        team: body.team.trim().to_string(),
        sub: body.sub.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or("owner").to_string(),
        email: body.email.map(|e| e.trim().to_string()).filter(|e| !e.is_empty()),
        public_url: None,
        force: false,
    };
    let minted = crate::cli::init(state.storage.as_ref(), &args).await.map_err(ApiError::Validation)?;
    state.setup.forget();
    tracing::info!("setup: created team {} with owner {}", args.team, args.sub);
    Ok(Json(SetupResponse { team: args.team, sub: args.sub, token: minted.token }).into_response())
}
