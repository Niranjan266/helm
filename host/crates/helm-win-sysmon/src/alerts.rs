//! Pure threshold alerts over [`Stats`] snapshots.
//!
//! Feed every sample to [`Alerts::evaluate`] with a monotonic timestamp in
//! milliseconds. Each rule is edge-triggered: one [`AlertEvent`] with
//! `firing: true` when its condition starts holding (for CPU: has held for
//! `for_s` seconds), and one with `firing: false` when it stops.

use serde::{Deserialize, Serialize};

use crate::stats::Stats;

/// A threshold condition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Condition {
    /// Total CPU above `pct` continuously for at least `for_s` seconds.
    CpuAbove {
        /// Threshold, 0–100.
        pct: f32,
        /// Required duration in seconds (0 = immediately).
        for_s: u32,
    },
    /// RAM in use above `pct` of total.
    RamAbove {
        /// Threshold, 0–100.
        pct: f32,
    },
    /// Free space on a disk below `pct` of its size. `mount: None` checks
    /// every disk and reports the fullest one.
    DiskFreeBelowPct {
        /// Mount point such as `C:\`; case-insensitive.
        mount: Option<String>,
        /// Threshold, 0–100.
        pct: f32,
    },
    /// Free space on a disk below `bytes`. `mount: None` checks every disk.
    DiskFreeBelowBytes {
        /// Mount point such as `C:\`; case-insensitive.
        mount: Option<String>,
        /// Threshold in bytes.
        bytes: u64,
    },
}

/// A named rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Rule {
    /// Caller-chosen id, echoed in events.
    pub id: String,
    /// What to check.
    pub condition: Condition,
}

/// A rule changed state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AlertEvent {
    /// The rule's id.
    pub rule_id: String,
    /// `true` when the alert starts, `false` when it clears.
    pub firing: bool,
    /// The measured value (percent, or free bytes for `DiskFreeBelowBytes`).
    pub value: f64,
    /// The disk the event refers to, for disk rules.
    pub mount: Option<String>,
    /// Short human readable text, e.g. "CPU 97% for 30 s".
    pub message: String,
    /// Timestamp passed to `evaluate`.
    pub at_ms: u64,
}

#[derive(Debug, Clone, Default)]
struct RuleState {
    /// When the condition started holding (CPU duration tracking).
    since_ms: Option<u64>,
    firing: bool,
}

/// Evaluates a set of [`Rule`]s against successive samples.
#[derive(Debug, Clone, Default)]
pub struct Alerts {
    rules: Vec<(Rule, RuleState)>,
}

impl Alerts {
    /// New evaluator with no alert firing.
    pub fn new(rules: Vec<Rule>) -> Self {
        Self { rules: rules.into_iter().map(|r| (r, RuleState::default())).collect() }
    }

    /// The configured rules.
    pub fn rules(&self) -> impl Iterator<Item = &Rule> {
        self.rules.iter().map(|(r, _)| r)
    }

    /// Ids of rules currently firing.
    pub fn firing(&self) -> Vec<&str> {
        self.rules.iter().filter(|(_, s)| s.firing).map(|(r, _)| r.id.as_str()).collect()
    }

    /// Evaluate one sample taken at monotonic time `now_ms`; returns state
    /// changes only.
    pub fn evaluate(&mut self, stats: &Stats, now_ms: u64) -> Vec<AlertEvent> {
        let mut out = Vec::new();
        for (rule, state) in &mut self.rules {
            let (holds, value, mount, what) = check(&rule.condition, stats);
            let active = match &rule.condition {
                Condition::CpuAbove { for_s, .. } => {
                    if holds {
                        let since = *state.since_ms.get_or_insert(now_ms);
                        now_ms.saturating_sub(since) >= u64::from(*for_s) * 1000
                    } else {
                        state.since_ms = None;
                        false
                    }
                }
                _ => holds,
            };
            if active != state.firing {
                state.firing = active;
                let message = if active { what } else { format!("{what} (cleared)") };
                out.push(AlertEvent {
                    rule_id: rule.id.clone(),
                    firing: active,
                    value,
                    mount,
                    message,
                    at_ms: now_ms,
                });
            }
        }
        out
    }
}

fn pct(used: u64, total: u64) -> f64 {
    if total == 0 { 0.0 } else { used as f64 * 100.0 / total as f64 }
}

fn mount_matches(want: &Option<String>, mount: &str) -> bool {
    want.as_ref().is_none_or(|w| w.eq_ignore_ascii_case(mount))
}

/// Returns (condition holds, value, mount, message).
fn check(c: &Condition, s: &Stats) -> (bool, f64, Option<String>, String) {
    match c {
        Condition::CpuAbove { pct: t, for_s } => {
            let v = f64::from(s.cpu_total);
            (v > f64::from(*t), v, None, format!("CPU {v:.0}% above {t}% for {for_s} s"))
        }
        Condition::RamAbove { pct: t } => {
            let v = pct(s.ram_used, s.ram_total);
            (v > f64::from(*t), v, None, format!("RAM {v:.0}% above {t}%"))
        }
        Condition::DiskFreeBelowPct { mount, pct: t } => {
            let worst = s
                .disks
                .iter()
                .filter(|d| d.total > 0 && mount_matches(mount, &d.mount))
                .map(|d| (pct(d.total.saturating_sub(d.used), d.total), d))
                .min_by(|a, b| a.0.total_cmp(&b.0));
            match worst {
                Some((v, d)) => (
                    v < f64::from(*t),
                    v,
                    Some(d.mount.clone()),
                    format!("{} free {v:.1}% below {t}%", d.mount),
                ),
                None => (false, 0.0, mount.clone(), "disk not found".into()),
            }
        }
        Condition::DiskFreeBelowBytes { mount, bytes } => {
            let worst = s
                .disks
                .iter()
                .filter(|d| d.total > 0 && mount_matches(mount, &d.mount))
                .map(|d| (d.total.saturating_sub(d.used), d))
                .min_by_key(|(free, _)| *free);
            match worst {
                Some((free, d)) => (
                    free < *bytes,
                    free as f64,
                    Some(d.mount.clone()),
                    format!("{} free {} MB below {} MB", d.mount, free >> 20, bytes >> 20),
                ),
                None => (false, 0.0, mount.clone(), "disk not found".into()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::DiskStats;

    fn stats(cpu: f32, ram_used: u64, disks: Vec<(&str, u64, u64)>) -> Stats {
        Stats {
            cpu_total: cpu,
            ram_used,
            ram_total: 100,
            disks: disks
                .into_iter()
                .map(|(m, used, total)| DiskStats {
                    mount: m.into(),
                    used,
                    total,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    fn cpu_rule(pct: f32, for_s: u32) -> Rule {
        Rule { id: "cpu".into(), condition: Condition::CpuAbove { pct, for_s } }
    }

    #[test]
    fn cpu_needs_duration_then_clears_once() {
        let mut a = Alerts::new(vec![cpu_rule(90.0, 10)]);
        let hot = stats(95.0, 0, vec![]);
        assert!(a.evaluate(&hot, 0).is_empty());
        assert!(a.evaluate(&hot, 9_999).is_empty());
        let ev = a.evaluate(&hot, 10_000);
        assert_eq!(ev.len(), 1);
        assert!(ev[0].firing);
        assert_eq!(ev[0].rule_id, "cpu");
        // Still hot: no repeat.
        assert!(a.evaluate(&hot, 11_000).is_empty());
        assert_eq!(a.firing(), vec!["cpu"]);
        let ev = a.evaluate(&stats(10.0, 0, vec![]), 12_000);
        assert_eq!(ev.len(), 1);
        assert!(!ev[0].firing);
        assert!(a.firing().is_empty());
    }

    #[test]
    fn cpu_dip_resets_timer() {
        let mut a = Alerts::new(vec![cpu_rule(90.0, 5)]);
        let hot = stats(95.0, 0, vec![]);
        a.evaluate(&hot, 0);
        a.evaluate(&stats(50.0, 0, vec![]), 4_000);
        assert!(a.evaluate(&hot, 6_000).is_empty());
        assert!(a.evaluate(&hot, 10_999).is_empty());
        assert_eq!(a.evaluate(&hot, 11_000).len(), 1);
    }

    #[test]
    fn cpu_equal_to_threshold_does_not_fire() {
        let mut a = Alerts::new(vec![cpu_rule(90.0, 0)]);
        assert!(a.evaluate(&stats(90.0, 0, vec![]), 0).is_empty());
        assert_eq!(a.evaluate(&stats(90.5, 0, vec![]), 1).len(), 1);
    }

    #[test]
    fn ram_threshold() {
        let mut a =
            Alerts::new(vec![Rule { id: "ram".into(), condition: Condition::RamAbove { pct: 80.0 } }]);
        assert!(a.evaluate(&stats(0.0, 70, vec![]), 0).is_empty());
        let ev = a.evaluate(&stats(0.0, 85, vec![]), 1);
        assert!(ev[0].firing);
        assert!((ev[0].value - 85.0).abs() < 1e-9);
    }

    #[test]
    fn disk_pct_specific_and_any() {
        let s = stats(0.0, 0, vec![("C:\\", 95, 100), ("D:\\", 10, 100)]);
        let mut a = Alerts::new(vec![
            Rule {
                id: "d".into(),
                condition: Condition::DiskFreeBelowPct { mount: Some("d:\\".into()), pct: 10.0 },
            },
            Rule { id: "any".into(), condition: Condition::DiskFreeBelowPct { mount: None, pct: 10.0 } },
        ]);
        let ev = a.evaluate(&s, 0);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].rule_id, "any");
        assert_eq!(ev[0].mount.as_deref(), Some("C:\\"));
    }

    #[test]
    fn disk_bytes_and_missing_disk() {
        let s = stats(0.0, 0, vec![("C:\\", 90, 100)]);
        let mut a = Alerts::new(vec![
            Rule {
                id: "b".into(),
                condition: Condition::DiskFreeBelowBytes { mount: None, bytes: 11 },
            },
            Rule {
                id: "missing".into(),
                condition: Condition::DiskFreeBelowBytes { mount: Some("Z:\\".into()), bytes: 1 << 40 },
            },
        ]);
        let ev = a.evaluate(&s, 0);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].rule_id, "b");
        assert_eq!(ev[0].value, 10.0);
    }

    #[test]
    fn rules_serde_snake_case() {
        let r = Rule { id: "x".into(), condition: Condition::CpuAbove { pct: 90.0, for_s: 30 } };
        let j = serde_json::to_string(&r).unwrap();
        assert_eq!(j, r#"{"id":"x","condition":{"kind":"cpu_above","pct":90.0,"for_s":30}}"#);
        let back: Rule = serde_json::from_str(&j).unwrap();
        assert_eq!(back, r);
    }
}
