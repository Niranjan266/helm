//! Deterministic trigger engine.
//!
//! Triggers map a condition to a deck action:
//!
//! ```json
//! {"id":"morning","when":{"time":"07:30 mon-fri"},"action":{"launch":"Outlook"}}
//! {"id":"obs","when":{"app_start":{"exe":"obs64.exe"}},"action":{"remote":"obs.scene:Live"}}
//! {"id":"phone","when":{"phone_connected":{"device_id":null}},"action":{"media":"pause"}}
//! {"id":"home","when":{"wifi":{"ssid":"HomeNet"}},"action":{"volume_step":-0.2}}
//! ```
//!
//! The engine never reads the real clock or the OS: time comes from a
//! [`Clock`] and everything else from [`Event`]s, so it is fully testable.
//! It returns [`Firing`]s; the caller runs the actions.
//!
//! Semantics:
//! - **Edge-triggered** (dedupe): `app_start` fires when the first instance of
//!   an exe appears (needs `AppExited` to re-arm), `phone_connected` when a
//!   device goes from disconnected to connected, `wifi` when the current SSID
//!   changes to the trigger's SSID. Repeated identical events do nothing.
//! - **Debounce**: a trigger never fires twice within its `debounce_ms`
//!   (default [`DEFAULT_DEBOUNCE_MS`]) on the monotonic clock.
//! - **Time**: `HH:MM [days]`, checked by [`TriggerEngine::tick`]. Each minute
//!   fires at most once. If ticks are late, minutes up to [`MAX_CATCHUP_MIN`]
//!   back are caught up; older ones are skipped. If local time moves backwards
//!   by ≤ 2 h (DST) nothing re-fires; a larger jump resets the schedule.
//! - Within one call, identical actions are only returned once.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::deck;

pub const DEFAULT_DEBOUNCE_MS: u64 = 2_000;
pub const MAX_CATCHUP_MIN: i64 = 5;
const BACKWARD_RESET_MIN: i64 = 120;
const MINUTES_PER_DAY: i64 = 1_440;

// ---------------------------------------------------------------- clock

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Weekday {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

impl Weekday {
    pub const ALL: [Weekday; 7] = [
        Self::Mon,
        Self::Tue,
        Self::Wed,
        Self::Thu,
        Self::Fri,
        Self::Sat,
        Self::Sun,
    ];

    pub fn index(self) -> u8 {
        self as u8
    }

    fn bit(self) -> u8 {
        1 << self.index()
    }

    fn parse(s: &str) -> Option<Self> {
        const NAMES: [&str; 7] = [
            "monday",
            "tuesday",
            "wednesday",
            "thursday",
            "friday",
            "saturday",
            "sunday",
        ];
        let s = s.to_ascii_lowercase();
        NAMES
            .iter()
            .position(|n| s == *n || (s.len() == 3 && n.starts_with(&s)))
            .map(|i| Self::ALL[i])
    }
}

/// A point in time as the engine sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Now {
    /// Monotonic milliseconds (debounce).
    pub mono_ms: u64,
    /// Minutes since 1970-01-01 00:00 *local* time (schedule).
    pub local_minute: i64,
}

impl Now {
    pub fn weekday(&self) -> Weekday {
        // 1970-01-01 was a Thursday.
        let day = self.local_minute.div_euclid(MINUTES_PER_DAY);
        Weekday::ALL[(day + 3).rem_euclid(7) as usize]
    }

    pub fn hour(&self) -> u8 {
        (self.local_minute.rem_euclid(MINUTES_PER_DAY) / 60) as u8
    }

    pub fn minute(&self) -> u8 {
        (self.local_minute.rem_euclid(60)) as u8
    }
}

pub trait Clock {
    fn now(&self) -> Now;
}

/// Real clock with a fixed UTC offset supplied by the agent (which knows the
/// Windows time zone); refresh it by building a new one when the zone changes.
#[derive(Debug, Clone)]
pub struct SystemClock {
    pub utc_offset_minutes: i32,
    start: Instant,
}

impl SystemClock {
    pub fn new(utc_offset_minutes: i32) -> Self {
        Self {
            utc_offset_minutes,
            start: Instant::now(),
        }
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Now {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Now {
            mono_ms: self.start.elapsed().as_millis() as u64,
            local_minute: secs.div_euclid(60) + i64::from(self.utc_offset_minutes),
        }
    }
}

/// Manually driven clock for tests. Clones share state.
#[derive(Debug, Clone)]
pub struct FakeClock(Arc<Mutex<(u64, i64)>>); // (mono_ms, local_ms)

impl FakeClock {
    /// Starts on the first `weekday` after 1970-01-05 (a Monday) at `hh:mm`.
    pub fn at(weekday: Weekday, hour: u8, minute: u8) -> Self {
        let c = Self(Arc::new(Mutex::new((0, 0))));
        c.set_local(weekday, hour, minute);
        c
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, (u64, i64)> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Advance both clocks.
    pub fn advance_ms(&self, ms: u64) {
        let mut g = self.lock();
        g.0 += ms;
        g.1 += ms as i64;
    }

    pub fn advance_min(&self, minutes: u64) {
        self.advance_ms(minutes * 60_000);
    }

    /// Jump wall-clock time (e.g. DST or user change) without touching the
    /// monotonic clock.
    pub fn shift_local_min(&self, delta: i64) {
        self.lock().1 += delta * 60_000;
    }

    pub fn set_local(&self, weekday: Weekday, hour: u8, minute: u8) {
        let day = 4 + i64::from(weekday.index());
        let m = day * MINUTES_PER_DAY + i64::from(hour) * 60 + i64::from(minute);
        self.lock().1 = m * 60_000;
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Now {
        let g = self.lock();
        Now {
            mono_ms: g.0,
            local_minute: g.1.div_euclid(60_000),
        }
    }
}

// ---------------------------------------------------------------- spec

/// `HH:MM [days]`. Days: `daily`/`*`, `weekdays`, `weekends`, names
/// (`mon` or `monday`), ranges (`mon-fri`, wraps: `fri-mon`), lists with
/// commas or spaces. No days means every day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeSpec {
    pub hour: u8,
    pub minute: u8,
    /// Bit `i` = [`Weekday::ALL`]`[i]`.
    pub days: u8,
}

impl TimeSpec {
    pub fn parse(s: &str) -> Result<Self, TriggerError> {
        let bad = |why: &str| TriggerError::BadTime(format!("{why}: `{s}`"));
        let mut parts = s.split_whitespace();
        let hm = parts.next().ok_or_else(|| bad("empty"))?;
        let (h, m) = hm.split_once(':').ok_or_else(|| bad("expected HH:MM"))?;
        let digits = |x: &str| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit());
        if !digits(h) || h.len() > 2 || !digits(m) || m.len() != 2 {
            return Err(bad("expected HH:MM"));
        }
        let hour: u8 = h.parse().map_err(|_| bad("bad hour"))?;
        let minute: u8 = m.parse().map_err(|_| bad("bad minute"))?;
        if hour > 23 || minute > 59 {
            return Err(bad("time out of range"));
        }
        let rest: Vec<&str> = parts.collect();
        let days = if rest.is_empty() {
            0x7f
        } else {
            let mut mask = 0u8;
            for tok in rest
                .join(",")
                .split(',')
                .map(str::trim)
                .filter(|t| !t.is_empty())
            {
                mask |= match tok.to_ascii_lowercase().as_str() {
                    "daily" | "everyday" | "*" | "all" => 0x7f,
                    "weekdays" => 0x1f,
                    "weekends" => 0x60,
                    t => match t.split_once('-') {
                        Some((a, b)) => {
                            let a = Weekday::parse(a).ok_or_else(|| bad("bad day"))?.index();
                            let b = Weekday::parse(b).ok_or_else(|| bad("bad day"))?.index();
                            let mut m = 0u8;
                            let mut i = a;
                            loop {
                                m |= 1 << i;
                                if i == b {
                                    break;
                                }
                                i = (i + 1) % 7;
                            }
                            m
                        }
                        None => Weekday::parse(t).ok_or_else(|| bad("bad day"))?.bit(),
                    },
                };
            }
            mask
        };
        Ok(Self { hour, minute, days })
    }

    pub fn matches(&self, now: &Now) -> bool {
        self.hour == now.hour()
            && self.minute == now.minute()
            && self.days & now.weekday().bit() != 0
    }
}

// ---------------------------------------------------------------- model

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum When {
    Time(String),
    AppStart {
        exe: String,
    },
    PhoneConnected {
        #[serde(default)]
        device_id: Option<String>,
    },
    Wifi {
        ssid: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Trigger {
    pub id: String,
    pub when: When,
    pub action: Value,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debounce_ms: Option<u64>,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    AppStarted { exe: String },
    AppExited { exe: String },
    PhoneConnected { device_id: String },
    PhoneDisconnected { device_id: String },
    WifiConnected { ssid: String },
    WifiDisconnected,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Firing {
    pub trigger_id: String,
    pub action: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TriggerError {
    #[error("trigger id must be 1-64 printable chars")]
    BadId,
    #[error("duplicate trigger id `{0}`")]
    DuplicateId(String),
    #[error("bad time spec {0}")]
    BadTime(String),
    #[error("trigger `{id}`: {reason}")]
    BadCondition { id: String, reason: &'static str },
    #[error("trigger `{id}`: {source}")]
    BadAction {
        id: String,
        #[source]
        source: deck::ActionError,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Cond {
    Time(TimeSpec),
    App(String),
    Phone(Option<String>),
    Wifi(String),
}

#[derive(Debug, Clone)]
struct Compiled {
    def: Trigger,
    cond: Cond,
}

/// Lower-case file name without directories and a trailing `.exe`.
pub fn normalize_exe(exe: &str) -> String {
    let base = exe.rsplit(['\\', '/']).next().unwrap_or(exe).trim();
    let lower = base.to_ascii_lowercase();
    lower.strip_suffix(".exe").unwrap_or(&lower).to_string()
}

fn compile(t: &Trigger) -> Result<Compiled, TriggerError> {
    if t.id.is_empty() || t.id.len() > 64 || t.id.chars().any(char::is_control) {
        return Err(TriggerError::BadId);
    }
    let cond_err = |reason| TriggerError::BadCondition {
        id: t.id.clone(),
        reason,
    };
    let cond = match &t.when {
        When::Time(s) => Cond::Time(TimeSpec::parse(s)?),
        When::AppStart { exe } => {
            let n = normalize_exe(exe);
            if n.is_empty() {
                return Err(cond_err("exe must not be empty"));
            }
            Cond::App(n)
        }
        When::PhoneConnected { device_id } => {
            if device_id.as_deref().is_some_and(str::is_empty) {
                return Err(cond_err("device_id must not be empty"));
            }
            Cond::Phone(device_id.clone())
        }
        When::Wifi { ssid } => {
            if ssid.is_empty() || ssid.len() > 32 {
                return Err(cond_err("ssid must be 1-32 bytes"));
            }
            Cond::Wifi(ssid.clone())
        }
    };
    deck::validate_action(&t.action).map_err(|source| TriggerError::BadAction {
        id: t.id.clone(),
        source,
    })?;
    Ok(Compiled {
        def: t.clone(),
        cond,
    })
}

// ---------------------------------------------------------------- engine

#[derive(Debug)]
pub struct TriggerEngine<C: Clock> {
    clock: C,
    triggers: Vec<Compiled>,
    last_fired: HashMap<String, u64>,
    last_minute: Option<i64>,
    running: HashMap<String, u32>,
    phones: HashSet<String>,
    wifi: Option<String>,
}

impl<C: Clock> TriggerEngine<C> {
    pub fn new(clock: C, triggers: Vec<Trigger>) -> Result<Self, TriggerError> {
        let mut e = Self {
            clock,
            triggers: Vec::new(),
            last_fired: HashMap::new(),
            last_minute: None,
            running: HashMap::new(),
            phones: HashSet::new(),
            wifi: None,
        };
        e.set_triggers(triggers)?;
        Ok(e)
    }

    /// Replace the trigger list (all-or-nothing). Observed state (running
    /// apps, connected phones, Wi-Fi) is kept.
    pub fn set_triggers(&mut self, triggers: Vec<Trigger>) -> Result<(), TriggerError> {
        let mut seen = HashSet::new();
        let mut compiled = Vec::with_capacity(triggers.len());
        for t in &triggers {
            let c = compile(t)?;
            if !seen.insert(t.id.clone()) {
                return Err(TriggerError::DuplicateId(t.id.clone()));
            }
            compiled.push(c);
        }
        self.last_fired.retain(|id, _| seen.contains(id));
        self.triggers = compiled;
        Ok(())
    }

    pub fn triggers(&self) -> impl Iterator<Item = &Trigger> {
        self.triggers.iter().map(|c| &c.def)
    }

    pub fn clock(&self) -> &C {
        &self.clock
    }

    /// Check time triggers. Call at least every 30 s.
    pub fn tick(&mut self) -> Vec<Firing> {
        let now = self.clock.now();
        let cur = now.local_minute;
        let from = match self.last_minute {
            None => cur,
            Some(last) if cur > last => (last + 1).max(cur - MAX_CATCHUP_MIN + 1),
            Some(last) if last - cur > BACKWARD_RESET_MIN => cur,
            Some(_) => {
                // Same minute, or a small backward step: nothing new.
                return Vec::new();
            }
        };
        self.last_minute = Some(cur);
        let mut out = Vec::new();
        for m in from..=cur {
            let at = Now {
                mono_ms: now.mono_ms,
                local_minute: m,
            };
            out.extend(fire(
                &self.triggers,
                &mut self.last_fired,
                &now,
                |c| matches!(c, Cond::Time(spec) if spec.matches(&at)),
            ));
        }
        dedupe(out)
    }

    /// Feed one OS/agent event.
    pub fn handle(&mut self, event: &Event) -> Vec<Firing> {
        let now = self.clock.now();
        let out = match event {
            Event::AppStarted { exe } => {
                let k = normalize_exe(exe);
                let n = self.running.entry(k.clone()).or_insert(0);
                *n += 1;
                if *n != 1 {
                    return Vec::new();
                }
                fire(
                    &self.triggers,
                    &mut self.last_fired,
                    &now,
                    |c| matches!(c, Cond::App(e) if *e == k),
                )
            }
            Event::AppExited { exe } => {
                let k = normalize_exe(exe);
                if let Some(n) = self.running.get_mut(&k) {
                    *n = n.saturating_sub(1);
                    if *n == 0 {
                        self.running.remove(&k);
                    }
                }
                Vec::new()
            }
            Event::PhoneConnected { device_id } => {
                if !self.phones.insert(device_id.clone()) {
                    return Vec::new();
                }
                fire(&self.triggers, &mut self.last_fired, &now, |c| match c {
                    Cond::Phone(None) => true,
                    Cond::Phone(Some(d)) => d == device_id,
                    _ => false,
                })
            }
            Event::PhoneDisconnected { device_id } => {
                self.phones.remove(device_id);
                Vec::new()
            }
            Event::WifiConnected { ssid } => {
                if self.wifi.as_deref() == Some(ssid.as_str()) {
                    return Vec::new();
                }
                self.wifi = Some(ssid.clone());
                fire(
                    &self.triggers,
                    &mut self.last_fired,
                    &now,
                    |c| matches!(c, Cond::Wifi(s) if s == ssid),
                )
            }
            Event::WifiDisconnected => {
                self.wifi = None;
                Vec::new()
            }
        };
        dedupe(out)
    }
}

fn fire(
    triggers: &[Compiled],
    last_fired: &mut HashMap<String, u64>,
    now: &Now,
    pred: impl Fn(&Cond) -> bool,
) -> Vec<Firing> {
    let mut out = Vec::new();
    for t in triggers.iter().filter(|t| t.def.enabled && pred(&t.cond)) {
        let debounce = t.def.debounce_ms.unwrap_or(DEFAULT_DEBOUNCE_MS);
        if let Some(&at) = last_fired.get(&t.def.id)
            && now.mono_ms.saturating_sub(at) < debounce
        {
            continue;
        }
        last_fired.insert(t.def.id.clone(), now.mono_ms);
        out.push(Firing {
            trigger_id: t.def.id.clone(),
            action: t.def.action.clone(),
        });
    }
    out
}

fn dedupe(firings: Vec<Firing>) -> Vec<Firing> {
    let mut out: Vec<Firing> = Vec::with_capacity(firings.len());
    for f in firings {
        if !out.iter().any(|o| o.action == f.action) {
            out.push(f);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weekday_math() {
        let n = Now {
            mono_ms: 0,
            local_minute: 0,
        };
        assert_eq!(n.weekday(), Weekday::Thu);
        let c = FakeClock::at(Weekday::Sun, 23, 59);
        let n = c.now();
        assert_eq!((n.weekday(), n.hour(), n.minute()), (Weekday::Sun, 23, 59));
        c.advance_min(1);
        let n = c.now();
        assert_eq!((n.weekday(), n.hour(), n.minute()), (Weekday::Mon, 0, 0));
    }

    #[test]
    fn time_spec_parsing() {
        let t = TimeSpec::parse("7:05").unwrap();
        assert_eq!((t.hour, t.minute, t.days), (7, 5, 0x7f));
        assert_eq!(TimeSpec::parse("07:30 mon-fri").unwrap().days, 0x1f);
        assert_eq!(TimeSpec::parse("07:30 weekdays").unwrap().days, 0x1f);
        assert_eq!(TimeSpec::parse("10:00 sat,sun").unwrap().days, 0x60);
        assert_eq!(TimeSpec::parse("10:00 Saturday Sunday").unwrap().days, 0x60);
        assert_eq!(TimeSpec::parse("10:00 fri-mon").unwrap().days, 0b111_0001);
        assert_eq!(TimeSpec::parse("10:00 daily").unwrap().days, 0x7f);
        for bad in [
            "",
            "24:00",
            "12:60",
            "12",
            "1:2",
            "12:00 funday",
            "a:bc",
            "123:00",
        ] {
            assert!(TimeSpec::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn exe_normalization() {
        assert_eq!(normalize_exe(r"C:\Program Files\OBS\OBS64.EXE"), "obs64");
        assert_eq!(normalize_exe("obs64"), "obs64");
        assert_eq!(normalize_exe("/usr/bin/x.exe"), "x");
    }
}
