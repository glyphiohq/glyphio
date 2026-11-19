//! Launch timing marks (PERF-01): elapsed milliseconds since the process reached `run`, logged
//! at fixed labels. Labels are `'static` strings, so nothing user-supplied can reach the log.

use std::sync::OnceLock;
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();

/// Start the clock. Idempotent; the first call wins.
pub fn start() {
    let _ = START.set(Instant::now());
}

/// Milliseconds since [`start`], or `None` if it was never called.
pub fn elapsed_ms() -> Option<u128> {
    START.get().map(|t| t.elapsed().as_millis())
}

pub fn mark(label: &'static str) {
    if let Some(ms) = elapsed_ms() {
        log::info!("launch-timing {label}={ms}ms");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_is_available_once_started() {
        start();
        assert!(elapsed_ms().is_some());
        mark("test");
    }
}
