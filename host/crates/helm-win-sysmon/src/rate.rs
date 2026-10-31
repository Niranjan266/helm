//! Pure helpers that turn cumulative byte counters into per-second rates.

use std::collections::HashMap;
use std::time::Duration;

/// Bytes per second between two readings of a monotonically increasing
/// counter.
///
/// Returns 0 when `elapsed` is zero or when the counter went backwards
/// (adapter reset, disk re-attached, 32-bit wrap): a reset is reported as "no
/// traffic" for one interval rather than as a huge spike.
pub fn per_second(prev: u64, cur: u64, elapsed: Duration) -> u64 {
    let secs = elapsed.as_secs_f64();
    if secs <= 0.0 || cur < prev {
        return 0;
    }
    ((cur - prev) as f64 / secs).round() as u64
}

/// Remembers the last value of named cumulative counters and yields rates.
#[derive(Debug, Default, Clone)]
pub struct RateTracker {
    last: HashMap<String, u64>,
}

impl RateTracker {
    /// Empty tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record `cur` for `key` and return its rate since the previous call.
    /// The first reading of a key returns 0.
    pub fn update(&mut self, key: &str, cur: u64, elapsed: Duration) -> u64 {
        match self.last.insert(key.to_string(), cur) {
            Some(prev) => per_second(prev, cur, elapsed),
            None => 0,
        }
    }

    /// Forget keys that are not in `alive` (removed disks / adapters).
    pub fn retain(&mut self, alive: &[&str]) {
        self.last.retain(|k, _| alive.contains(&k.as_str()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_rate() {
        assert_eq!(per_second(1000, 3000, Duration::from_secs(2)), 1000);
        assert_eq!(per_second(0, 500, Duration::from_millis(500)), 1000);
    }

    #[test]
    fn zero_elapsed_and_reset_are_zero() {
        assert_eq!(per_second(0, 500, Duration::ZERO), 0);
        assert_eq!(per_second(5000, 10, Duration::from_secs(1)), 0);
    }

    #[test]
    fn tracker_first_reading_is_zero_then_rates() {
        let mut t = RateTracker::new();
        assert_eq!(t.update("eth", 100, Duration::from_secs(1)), 0);
        assert_eq!(t.update("eth", 1100, Duration::from_secs(1)), 1000);
        assert_eq!(t.update("wifi", 7, Duration::from_secs(1)), 0);
        t.retain(&["wifi"]);
        assert_eq!(t.update("eth", 5000, Duration::from_secs(1)), 0);
    }
}
