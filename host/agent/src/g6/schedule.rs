//! Scheduled actions (everyday.md §5): deck or power actions at a local time, once or on
//! repeating ISO weekdays, saved in `schedules.json`. Time maths is pure (unix ms + the
//! PC's UTC offset) so tests never wait for the clock.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use helm_win_system::power::PowerAction;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::{Date, Month, OffsetDateTime};

use super::lock;
use crate::store::{DeckAction, validate_deck_action};
use crate::util;

pub const SCHEDULES_FILE: &str = "schedules.json";
pub const MAX_SCHEDULES: usize = 64;
/// Runs later than this (PC asleep, agent stopped) are skipped.
pub const MAX_LATE_MS: u64 = 10 * 60 * 1000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PowerStep {
    pub power: PowerAction,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SchedAction {
    Power(PowerStep),
    Deck(DeckAction),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Schedule {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Local `HH:MM`.
    pub time: String,
    /// ISO weekdays 1 (Mon) ..= 7 (Sun); empty = one-off.
    #[serde(default)]
    pub days: Vec<u8>,
    /// One-off date `YYYY-MM-DD` (filled in when saved).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    pub action: SchedAction,
    /// Agent-owned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_ms: Option<u64>,
}

fn yes() -> bool {
    true
}

impl Schedule {
    pub fn is_power(&self) -> bool {
        matches!(self.action, SchedAction::Power(_))
    }
    pub fn one_off(&self) -> bool {
        self.days.is_empty()
    }
}

pub fn parse_hhmm(s: &str) -> Option<(u8, u8)> {
    let (h, m) = s.split_once(':')?;
    if h.len() != 2 || m.len() != 2 {
        return None;
    }
    let (h, m): (u8, u8) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some((h, m))
}

pub fn parse_date(s: &str) -> Option<Date> {
    let mut it = s.split('-');
    let (y, m, d) = (it.next()?, it.next()?, it.next()?);
    if it.next().is_some() || y.len() != 4 || m.len() != 2 || d.len() != 2 {
        return None;
    }
    let month = Month::try_from(m.parse::<u8>().ok()?).ok()?;
    Date::from_calendar_date(y.parse().ok()?, month, d.parse().ok()?).ok()
}

fn fmt_date(d: Date) -> String {
    format!("{:04}-{:02}-{:02}", d.year(), u8::from(d.month()), d.day())
}

/// Local date of `unix_ms` with UTC offset `offset_min`.
fn local_date(unix_ms: u64, offset_min: i32) -> Date {
    let local_s = (unix_ms as i64 + i64::from(offset_min) * 60_000).div_euclid(1000);
    OffsetDateTime::from_unix_timestamp(local_s)
        .map(|t| t.date())
        .unwrap_or(Date::MIN)
}

/// Unix ms of local `date` at `hh:mm`.
fn at(date: Date, (h, m): (u8, u8), offset_min: i32) -> i64 {
    let midnight = date.midnight().assume_utc().unix_timestamp() * 1000;
    midnight + (i64::from(h) * 60 + i64::from(m)) * 60_000 - i64::from(offset_min) * 60_000
}

/// The first run at or after `from_ms` (`None`: one-off in the past, or invalid).
pub fn next_run(s: &Schedule, from_ms: u64, offset_min: i32) -> Option<u64> {
    let hm = parse_hhmm(&s.time)?;
    if s.one_off() {
        let t = at(parse_date(s.date.as_deref()?)?, hm, offset_min);
        return (t >= from_ms as i64).then_some(t.max(0) as u64);
    }
    let start = local_date(from_ms, offset_min);
    for k in 0..=7 {
        let d = start.checked_add(time::Duration::days(k))?;
        if !s.days.contains(&d.weekday().number_from_monday()) {
            continue;
        }
        let t = at(d, hm, offset_min);
        if t >= from_ms as i64 {
            return Some(t as u64);
        }
    }
    None
}

/// Check and normalise a list from `schedule.set` (`now_ms` fills in one-off dates).
pub fn validate(list: &mut [Schedule], now_ms: u64, offset_min: i32) -> Result<(), String> {
    if list.len() > MAX_SCHEDULES {
        return Err(format!("at most {MAX_SCHEDULES} schedules"));
    }
    let mut ids = std::collections::HashSet::new();
    for s in list.iter_mut() {
        if s.id.trim().is_empty() || s.id.chars().count() > 64 || s.id.chars().any(char::is_control)
        {
            return Err("schedule id must be 1-64 printable characters".into());
        }
        if !ids.insert(s.id.clone()) {
            return Err(format!("duplicate schedule id {:?}", s.id));
        }
        if let Some(n) = &s.name
            && (n.chars().count() > 100 || n.chars().any(char::is_control))
        {
            return Err("name must be at most 100 printable characters".into());
        }
        let hm = parse_hhmm(&s.time).ok_or_else(|| format!("time {:?} must be HH:MM", s.time))?;
        if s.days.iter().any(|d| !(1..=7).contains(d)) {
            return Err("days are ISO weekdays 1 (Monday) to 7 (Sunday)".into());
        }
        s.days.sort_unstable();
        s.days.dedup();
        if s.one_off() {
            match &s.date {
                Some(d) => {
                    parse_date(d).ok_or_else(|| format!("date {d:?} must be YYYY-MM-DD"))?;
                }
                None => {
                    let today = local_date(now_ms, offset_min);
                    let date = if at(today, hm, offset_min) >= now_ms as i64 {
                        today
                    } else {
                        today.next_day().unwrap_or(today)
                    };
                    s.date = Some(fmt_date(date));
                }
            }
        } else {
            s.date = None;
        }
        if let SchedAction::Deck(a) = &s.action {
            validate_deck_action(a)?;
        }
    }
    Ok(())
}

/// What a tick decided.
#[derive(Debug, Default, PartialEq)]
pub struct TickResult {
    /// Schedules to run now.
    pub run: Vec<Schedule>,
    /// Something was saved (one-off disabled, `last_run_ms` set).
    pub changed: bool,
    /// One-offs disabled (the list must be re-sent to phones).
    pub disabled: bool,
}

/// Pure tick: which enabled schedules have a run in `(last_check, now]` (at most
/// [`MAX_LATE_MS`] late, and after their `last_run_ms`). Updates `list` in place.
pub fn due(list: &mut [Schedule], last_check: Option<u64>, now_ms: u64, offset_min: i32) -> TickResult {
    let mut out = TickResult::default();
    let floor = now_ms.saturating_sub(MAX_LATE_MS);
    let from = last_check.map_or(floor, |c| c.max(floor)) + 1;
    for s in list.iter_mut().filter(|s| s.enabled) {
        if s.one_off() {
            let Some(t) = next_run(s, 0, offset_min) else {
                s.enabled = false;
                out.changed = true;
                out.disabled = true;
                continue;
            };
            if t <= now_ms {
                s.enabled = false;
                out.changed = true;
                out.disabled = true;
                if t >= floor && s.last_run_ms.is_none_or(|r| r < t) {
                    s.last_run_ms = Some(now_ms);
                    out.run.push(s.clone());
                }
            }
            continue;
        }
        if let Some(t) = next_run(s, from, offset_min)
            && t <= now_ms
            && s.last_run_ms.is_none_or(|r| r < t)
        {
            s.last_run_ms = Some(now_ms);
            out.changed = true;
            out.run.push(s.clone());
        }
    }
    out
}

#[derive(Serialize, Deserialize)]
struct File {
    version: u32,
    schedules: Vec<Schedule>,
}

pub struct Schedules {
    path: PathBuf,
    list: Mutex<Vec<Schedule>>,
    last_check: Mutex<Option<u64>>,
}

impl Schedules {
    pub fn load(dir: &Path) -> Schedules {
        let path = dir.join(SCHEDULES_FILE);
        let list = match std::fs::read(&path) {
            Ok(b) => serde_json::from_slice::<File>(&b)
                .map(|f| f.schedules)
                .unwrap_or_else(|e| {
                    tracing::warn!("{SCHEDULES_FILE} unreadable: {e}");
                    Vec::new()
                }),
            Err(_) => Vec::new(),
        };
        Schedules {
            path,
            list: Mutex::new(list),
            last_check: Mutex::new(None),
        }
    }

    pub fn get(&self) -> Vec<Schedule> {
        lock(&self.list).clone()
    }

    fn save(&self, list: &[Schedule]) -> anyhow::Result<()> {
        let f = File {
            version: 1,
            schedules: list.to_vec(),
        };
        util::atomic_write(&self.path, serde_json::to_string_pretty(&f)?.as_bytes())?;
        Ok(())
    }

    /// Replace the list (already validated); keeps `last_run_ms` of ids that stay.
    pub fn set(&self, mut new: Vec<Schedule>) -> anyhow::Result<()> {
        let mut g = lock(&self.list);
        for s in &mut new {
            s.last_run_ms = g.iter().find(|o| o.id == s.id).and_then(|o| o.last_run_ms);
        }
        self.save(&new)?;
        *g = new;
        Ok(())
    }

    pub fn tick(&self, now_ms: u64, offset_min: i32) -> TickResult {
        let mut g = lock(&self.list);
        let mut last = lock(&self.last_check);
        // Clock went backwards (or a test ticked the future earlier): start over.
        let prev = last.filter(|&c| c <= now_ms);
        let r = due(&mut g, prev, now_ms, offset_min);
        *last = Some(now_ms.max(last.unwrap_or(0)));
        if r.changed
            && let Err(e) = self.save(&g)
        {
            tracing::warn!("saving {SCHEDULES_FILE}: {e:#}");
        }
        r
    }
}

/// A schedule as sent to phones (adds `next_ms`).
pub fn to_json(list: &[Schedule], now_ms: u64, offset_min: i32) -> Value {
    Value::Array(
        list.iter()
            .map(|s| {
                let mut v = serde_json::to_value(s).unwrap_or(Value::Null);
                let next = s.enabled.then(|| next_run(s, now_ms, offset_min)).flatten();
                v["next_ms"] = json!(next);
                v["last_run_ms"] = json!(s.last_run_ms);
                v
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-11-02 is a Monday. Returns unix ms of that local date at hh:mm.
    fn ms(day: u8, h: u8, m: u8, off: i32) -> u64 {
        at(Date::from_calendar_date(2026, Month::November, day).unwrap(), (h, m), off) as u64
    }

    fn sched(id: &str, time: &str, days: &[u8], date: Option<&str>) -> Schedule {
        serde_json::from_value(json!({
            "id": id, "time": time, "days": days, "date": date,
            "action": {"media": "toggle_play_pause"}
        }))
        .unwrap()
    }

    #[test]
    fn parse_and_codec() {
        assert_eq!(parse_hhmm("07:05"), Some((7, 5)));
        for bad in ["7:05", "24:00", "12:60", "ab:cd", "1200"] {
            assert_eq!(parse_hhmm(bad), None, "{bad}");
        }
        assert!(parse_date("2026-02-29").is_none());
        assert!(parse_date("2028-02-29").is_some());
        let s: Schedule = serde_json::from_value(json!({
            "id":"a","time":"22:00","days":[],"action":{"power":"shutdown"}
        }))
        .unwrap();
        assert!(s.is_power() && s.enabled);
        let d: Schedule = serde_json::from_value(json!({
            "id":"b","time":"22:00","action":{"hotkey":"ctrl+c"}
        }))
        .unwrap();
        assert_eq!(d.action, SchedAction::Deck(DeckAction::Hotkey("ctrl+c".into())));
        assert!(serde_json::from_value::<Schedule>(json!({
            "id":"c","time":"22:00","action":{"power":"explode"}
        }))
        .is_err());
        let back = serde_json::to_value(&s).unwrap();
        assert_eq!(back["action"], json!({"power":"shutdown"}));
    }

    #[test]
    fn next_run_repeating_and_offset() {
        let off = 330; // UTC+05:30
        let s = sched("w", "08:30", &[1, 3], None); // Mon, Wed
        // Monday 08:00 local → Monday 08:30.
        assert_eq!(next_run(&s, ms(2, 8, 0, off), off), Some(ms(2, 8, 30, off)));
        // Monday 09:00 → Wednesday.
        assert_eq!(next_run(&s, ms(2, 9, 0, off), off), Some(ms(4, 8, 30, off)));
        // Thursday → next Monday (Nov 9).
        assert_eq!(next_run(&s, ms(5, 0, 0, off), off), Some(ms(9, 8, 30, off)));
        // Exactly on time counts.
        assert_eq!(next_run(&s, ms(2, 8, 30, off), off), Some(ms(2, 8, 30, off)));
        let neg = -300;
        let sun = sched("s", "23:59", &[7], None);
        assert_eq!(next_run(&sun, ms(2, 0, 0, neg), neg), Some(ms(8, 23, 59, neg)));
    }

    #[test]
    fn validate_fills_one_off_date_and_rejects_bad() {
        let off = 0;
        let mut l = vec![sched("a", "10:00", &[], None), sched("b", "06:00", &[], None)];
        validate(&mut l, ms(2, 9, 0, off), off).unwrap();
        assert_eq!(l[0].date.as_deref(), Some("2026-11-02"), "later today");
        assert_eq!(l[1].date.as_deref(), Some("2026-11-03"), "already past → tomorrow");
        let mut r = vec![sched("r", "10:00", &[3, 1, 3], Some("2026-01-01"))];
        validate(&mut r, 0, off).unwrap();
        assert_eq!(r[0].days, vec![1, 3]);
        assert_eq!(r[0].date, None, "repeating schedules have no date");
        for bad in [
            vec![sched("", "10:00", &[], None)],
            vec![sched("x", "10:00", &[], None), sched("x", "11:00", &[], None)],
            vec![sched("x", "25:00", &[], None)],
            vec![sched("x", "10:00", &[0], None)],
            vec![sched("x", "10:00", &[8], None)],
            vec![sched("x", "10:00", &[], Some("2026-13-01"))],
        ] {
            let mut b = bad;
            assert!(validate(&mut b, 0, off).is_err(), "{b:?}");
        }
        let mut hk = vec![sched("h", "10:00", &[], None)];
        hk[0].action = SchedAction::Deck(DeckAction::Hotkey("ctrl+nonsense".into()));
        assert!(validate(&mut hk, 0, off).is_err());
        let mut many: Vec<_> = (0..65).map(|i| sched(&i.to_string(), "10:00", &[1], None)).collect();
        assert!(validate(&mut many, 0, off).is_err());
    }

    #[test]
    fn due_runs_once_per_occurrence_and_skips_stale() {
        let off = 60;
        let mut l = vec![
            sched("rep", "08:00", &[1, 2, 3, 4, 5, 6, 7], None),
            sched("once", "08:05", &[], Some("2026-11-02")),
            sched("stale", "07:00", &[], Some("2026-11-02")),
        ];
        // 07:59: nothing yet, but the 07:00 one-off is more than 10 min late → disabled, not run.
        let r = due(&mut l, None, ms(2, 7, 59, off), off);
        assert!(r.run.is_empty());
        assert!(r.disabled && !l[2].enabled && l[2].last_run_ms.is_none());
        // 08:00:30 → the repeating one runs.
        let t1 = ms(2, 8, 0, off) + 30_000;
        let r = due(&mut l, Some(ms(2, 7, 59, off)), t1, off);
        assert_eq!(r.run.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["rep"]);
        // Same minute again → not twice.
        let r = due(&mut l, Some(t1), t1 + 10_000, off);
        assert!(r.run.is_empty());
        // Agent was stopped 08:01-08:09: the 08:05 one-off still runs (4 min late) and disables.
        let r = due(&mut l, None, ms(2, 8, 9, off), off);
        assert_eq!(r.run.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["once"]);
        assert!(!l[1].enabled && r.disabled);
        // The repeating schedule does not run again at 08:09 (last_run_ms after 08:00).
        assert!(due(&mut l, None, ms(2, 8, 9, off), off).run.is_empty());
        // Next day at 08:00 it runs again.
        let r = due(&mut l, Some(ms(3, 7, 59, off)), ms(3, 8, 0, off), off);
        assert_eq!(r.run.len(), 1);
        // Disabled schedules never run.
        l[0].enabled = false;
        assert!(due(&mut l, Some(ms(4, 7, 59, off)), ms(4, 8, 0, off), off).run.is_empty());
    }

    #[test]
    fn store_keeps_last_run_and_reports_next() {
        let dir = tempfile::tempdir().unwrap();
        let st = Schedules::load(dir.path());
        let mut l = vec![sched("rep", "08:00", &[1], None)];
        validate(&mut l, 0, 0).unwrap();
        st.set(l.clone()).unwrap();
        let r = st.tick(ms(2, 8, 0, 0), 0);
        assert_eq!(r.run.len(), 1);
        st.set(l).unwrap();
        assert_eq!(st.get()[0].last_run_ms, Some(ms(2, 8, 0, 0)), "kept across set");
        let again = Schedules::load(dir.path());
        assert_eq!(again.get().len(), 1);
        let v = to_json(&again.get(), ms(2, 9, 0, 0), 0);
        assert_eq!(v[0]["next_ms"], ms(9, 8, 0, 0));
        assert_eq!(v[0]["last_run_ms"], ms(2, 8, 0, 0));
    }
}
