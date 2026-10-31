//! Restart backoff for the supervised agent process.
//!
//! Every exit that happens soon after a launch counts as a failure and
//! doubles the wait before the next launch (base, 2x base, 4x base, ...,
//! capped at `max`). A run that lasted at least `stable_after` proves the
//! agent was healthy, so the failure streak is forgotten.

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackoffPolicy {
    pub base: Duration,
    pub max: Duration,
    pub stable_after: Duration,
}

impl Default for BackoffPolicy {
    fn default() -> Self {
        Self {
            base: Duration::from_secs(1),
            max: Duration::from_secs(60),
            stable_after: Duration::from_secs(30),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Backoff {
    policy: BackoffPolicy,
    failures: u32,
}

impl Backoff {
    pub fn new(policy: BackoffPolicy) -> Self {
        Self {
            policy,
            failures: 0,
        }
    }

    /// Consecutive quick failures so far.
    pub fn failures(&self) -> u32 {
        self.failures
    }

    /// Records an exit after the process ran for `ran_for` and returns how
    /// long to wait before relaunching.
    pub fn on_exit(&mut self, ran_for: Duration) -> Duration {
        if ran_for >= self.policy.stable_after {
            self.failures = 0;
        }
        self.failures = self.failures.saturating_add(1);
        self.delay_for(self.failures)
    }

    /// A launch that failed outright (no process) counts as a quick exit.
    pub fn on_launch_failure(&mut self) -> Duration {
        self.on_exit(Duration::ZERO)
    }

    /// Forget the streak (new session, new user).
    pub fn reset(&mut self) {
        self.failures = 0;
    }

    fn delay_for(&self, failures: u32) -> Duration {
        let shift = failures.saturating_sub(1).min(31);
        let factor = 1u32 << shift;
        self.policy
            .base
            .checked_mul(factor)
            .map_or(self.policy.max, |d| d.min(self.policy.max))
    }
}
