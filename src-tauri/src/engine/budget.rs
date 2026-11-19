//! How many times the supervisor restarts a crashing engine.
//!
//! A flat per-run budget meant five crashes spread over a week permanently disabled expansion
//! until the app restarted. The budget now refills once an engine has run healthily for
//! [`HEALTHY_RESET`], so only a real crash loop exhausts it.

use std::time::Duration;

pub const MAX_RESPAWNS: u32 = 5;
pub const HEALTHY_RESET: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Default)]
pub struct RespawnBudget {
    used: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Respawn; this is attempt `n` of [`MAX_RESPAWNS`].
    Respawn(u32),
    /// The budget is spent: leave the engine stopped and say so.
    Exhausted,
}

impl RespawnBudget {
    /// The engine died after running for `ran_for`.
    pub fn on_exit(&mut self, ran_for: Duration) -> Verdict {
        if ran_for >= HEALTHY_RESET {
            self.used = 0;
        }
        self.used += 1;
        if self.used > MAX_RESPAWNS {
            Verdict::Exhausted
        } else {
            Verdict::Respawn(self.used)
        }
    }

    /// A manual restart gives a fresh budget.
    pub fn reset(&mut self) {
        self.used = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHORT: Duration = Duration::from_secs(3);

    #[test]
    fn a_crash_loop_exhausts_the_budget() {
        let mut b = RespawnBudget::default();
        for n in 1..=MAX_RESPAWNS {
            assert_eq!(b.on_exit(SHORT), Verdict::Respawn(n));
        }
        assert_eq!(b.on_exit(SHORT), Verdict::Exhausted);
        assert_eq!(b.on_exit(SHORT), Verdict::Exhausted);
    }

    #[test]
    fn healthy_running_refills_the_budget() {
        let mut b = RespawnBudget::default();
        for _ in 0..MAX_RESPAWNS {
            b.on_exit(SHORT);
        }
        assert_eq!(b.on_exit(HEALTHY_RESET), Verdict::Respawn(1));
    }

    #[test]
    fn just_under_the_healthy_window_does_not_refill() {
        let mut b = RespawnBudget::default();
        for _ in 0..MAX_RESPAWNS {
            b.on_exit(SHORT);
        }
        assert_eq!(b.on_exit(HEALTHY_RESET - Duration::from_secs(1)), Verdict::Exhausted);
    }

    #[test]
    fn manual_restart_resets() {
        let mut b = RespawnBudget::default();
        for _ in 0..=MAX_RESPAWNS {
            b.on_exit(SHORT);
        }
        b.reset();
        assert_eq!(b.on_exit(SHORT), Verdict::Respawn(1));
    }
}
