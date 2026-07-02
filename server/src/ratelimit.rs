// SPDX-License-Identifier: Apache-2.0
//! Fixed-window in-memory rate limiter keyed by credential hash. Deliberately simple: the AWS
//! reference deployment also throttles at API Gateway; this protects self-hosted instances.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::ApiError;

pub struct RateLimiter {
    per_min: u32,
    windows: Mutex<HashMap<[u8; 32], (u64, u32)>>,
}

impl RateLimiter {
    pub fn new(per_min: u32) -> Self {
        Self { per_min, windows: Mutex::new(HashMap::new()) }
    }

    pub fn check(&self, key: [u8; 32]) -> Result<(), ApiError> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        let minute = now / 60;
        let mut w = self.windows.lock().unwrap();
        // Opportunistic cleanup so the map can't grow unboundedly under churn.
        if w.len() > 10_000 {
            w.retain(|_, (m, _)| *m == minute);
        }
        let entry = w.entry(key).or_insert((minute, 0));
        if entry.0 != minute {
            *entry = (minute, 0);
        }
        entry.1 += 1;
        if entry.1 > self.per_min {
            return Err(ApiError::RateLimited { retry_after_secs: 60 - (now % 60) });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_after_n_requests() {
        let rl = RateLimiter::new(3);
        let k = [1u8; 32];
        assert!(rl.check(k).is_ok());
        assert!(rl.check(k).is_ok());
        assert!(rl.check(k).is_ok());
        assert!(rl.check(k).is_err());
        // A different credential is unaffected.
        assert!(rl.check([2u8; 32]).is_ok());
    }
}
