//! Phase 3 shared state and background work (PROTOCOL §11): alert rules (`alerts.json`),
//! profiles (`profiles.json`) and the active profile, per-session `stats.tick` streams, and
//! one ticker that evaluates alerts and pushes deck `deck.state` / `deck.widget` updates.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use helm_win_sysmon::alerts::{Alerts, Condition, Rule};

use crate::platform::{ForegroundInfo, StatsSampler, blocking};
use crate::router::RpcError;
use crate::server::AppState;
use crate::session::Session;
use crate::store::{Bind, DeckButton, Widget, read_json};
use crate::util;

pub const ALERTS_FILE: &str = "alerts.json";
pub const PROFILES_FILE: &str = "profiles.json";
pub const MIN_STATS_INTERVAL_MS: u64 = 500;
pub const MAX_STATS_INTERVAL_MS: u64 = 60_000;
const MAX_RULES: usize = 32;
const MAX_PROFILES: usize = 64;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

// ---------------------------------------------------------------- alerts

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertKind {
    CpuAbove,
    RamAbove,
    DiskFreeBelow,
}

/// Wire form of an alert rule.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlertRule {
    pub id: String,
    pub kind: AlertKind,
    /// Percent, 0–100 (disk: free space percent).
    pub threshold: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub for_s: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mount: Option<String>,
}

impl AlertRule {
    fn to_rule(&self) -> Rule {
        let condition = match self.kind {
            AlertKind::CpuAbove => Condition::CpuAbove {
                pct: self.threshold,
                for_s: self.for_s.unwrap_or(0),
            },
            AlertKind::RamAbove => Condition::RamAbove {
                pct: self.threshold,
            },
            AlertKind::DiskFreeBelow => Condition::DiskFreeBelowPct {
                mount: self.mount.clone(),
                pct: self.threshold,
            },
        };
        Rule {
            id: self.id.clone(),
            condition,
        }
    }
}

fn check_text(what: &str, s: &str, max: usize) -> Result<(), String> {
    if s.trim().is_empty() || s.len() > max || s.chars().any(char::is_control) {
        return Err(format!("{what} must be 1-{max} printable characters"));
    }
    Ok(())
}

pub fn validate_rules(rules: &[AlertRule]) -> Result<(), String> {
    if rules.len() > MAX_RULES {
        return Err(format!("at most {MAX_RULES} rules"));
    }
    let mut ids = HashSet::new();
    for r in rules {
        check_text("rule id", &r.id, 128)?;
        if !ids.insert(r.id.as_str()) {
            return Err(format!("duplicate rule id {:?}", r.id));
        }
        if !r.threshold.is_finite() || !(0.0..=100.0).contains(&r.threshold) {
            return Err("threshold must be 0-100".into());
        }
        if r.for_s.is_some_and(|s| s > 3600) {
            return Err("for_s must be at most 3600".into());
        }
        if let Some(m) = &r.mount {
            check_text("mount", m, 260)?;
        }
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct AlertsFile {
    version: u32,
    rules: Vec<AlertRule>,
}

// ---------------------------------------------------------------- profiles

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_exe: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deck_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dock_page: Option<u32>,
}

pub fn validate_profiles(profiles: &[Profile]) -> Result<(), String> {
    if profiles.len() > MAX_PROFILES {
        return Err(format!("at most {MAX_PROFILES} profiles"));
    }
    let mut ids = HashSet::new();
    for p in profiles {
        check_text("profile id", &p.id, 128)?;
        if !ids.insert(p.id.as_str()) {
            return Err(format!("duplicate profile id {:?}", p.id));
        }
        check_text("name", &p.name, 200)?;
        if let Some(m) = &p.match_exe {
            check_text("match_exe", m, 260)?;
        }
        if let Some(d) = &p.deck_id {
            check_text("deck_id", d, 128)?;
        }
        if p.dock_page.is_some_and(|d| d > 1000) {
            return Err("dock_page out of range".into());
        }
    }
    Ok(())
}

/// The first profile whose `match_exe` equals `exe_name` (case-insensitive).
pub fn match_profile<'a>(profiles: &'a [Profile], exe_name: &str) -> Option<&'a Profile> {
    profiles.iter().find(|p| {
        p.match_exe
            .as_deref()
            .is_some_and(|m| !exe_name.is_empty() && m.eq_ignore_ascii_case(exe_name))
    })
}

#[derive(Serialize, Deserialize)]
struct ProfilesFile {
    version: u32,
    profiles: Vec<Profile>,
}

// ---------------------------------------------------------------- state

pub struct P3State {
    dir: PathBuf,
    file_lock: Mutex<()>,
    alerts: Mutex<Alerts>,
    active_profile: Mutex<Option<String>>,
    last_foreground: Mutex<Option<String>>,
    stats_subs: Mutex<HashMap<u64, JoinHandle<()>>>,
    tick_ms: AtomicU64,
}

impl P3State {
    pub fn new(data_dir: &Path) -> P3State {
        let st = P3State {
            dir: data_dir.to_path_buf(),
            file_lock: Mutex::new(()),
            alerts: Mutex::new(Alerts::default()),
            active_profile: Mutex::new(None),
            last_foreground: Mutex::new(None),
            stats_subs: Mutex::new(HashMap::new()),
            tick_ms: AtomicU64::new(1000),
        };
        match st.alert_rules() {
            Ok(rules) => {
                *lock(&st.alerts) = Alerts::new(rules.iter().map(AlertRule::to_rule).collect())
            }
            Err(e) => tracing::warn!("alerts.json unreadable: {e:#}"),
        }
        st
    }

    /// How often the ticker runs (tests shorten it).
    pub fn set_tick(&self, every: Duration) {
        self.tick_ms
            .store(every.as_millis().max(10) as u64, Ordering::Relaxed);
    }

    fn tick(&self) -> Duration {
        Duration::from_millis(self.tick_ms.load(Ordering::Relaxed))
    }

    pub fn alert_rules(&self) -> anyhow::Result<Vec<AlertRule>> {
        let _g = lock(&self.file_lock);
        Ok(read_json::<AlertsFile>(&self.dir.join(ALERTS_FILE))?
            .map(|f| f.rules)
            .unwrap_or_default())
    }

    pub fn set_alert_rules(&self, rules: Vec<AlertRule>) -> anyhow::Result<()> {
        {
            let _g = lock(&self.file_lock);
            let file = AlertsFile {
                version: 1,
                rules: rules.clone(),
            };
            util::atomic_write(
                &self.dir.join(ALERTS_FILE),
                serde_json::to_string_pretty(&file)?.as_bytes(),
            )?;
        }
        *lock(&self.alerts) = Alerts::new(rules.iter().map(AlertRule::to_rule).collect());
        Ok(())
    }

    pub fn profiles(&self) -> anyhow::Result<Vec<Profile>> {
        let _g = lock(&self.file_lock);
        Ok(read_json::<ProfilesFile>(&self.dir.join(PROFILES_FILE))?
            .map(|f| f.profiles)
            .unwrap_or_default())
    }

    pub fn set_profiles(&self, profiles: Vec<Profile>) -> anyhow::Result<()> {
        let _g = lock(&self.file_lock);
        let file = ProfilesFile {
            version: 1,
            profiles,
        };
        util::atomic_write(
            &self.dir.join(PROFILES_FILE),
            serde_json::to_string_pretty(&file)?.as_bytes(),
        )?;
        Ok(())
    }

    pub fn active_profile(&self) -> Option<String> {
        lock(&self.active_profile).clone()
    }

    /// Re-match the last foreground app (after `profiles.set` or a foreground change);
    /// returns the new active id if it changed.
    fn rematch(&self) -> Option<Option<String>> {
        let exe = lock(&self.last_foreground).clone().unwrap_or_default();
        let profiles = self.profiles().unwrap_or_default();
        let id = match_profile(&profiles, &exe).map(|p| p.id.clone());
        let mut active = lock(&self.active_profile);
        if *active != id {
            *active = id.clone();
            Some(id)
        } else {
            None
        }
    }

    /// Start (or restart) `stats.tick` events for one session.
    pub fn subscribe_stats(
        &self,
        state: &Arc<AppState>,
        session: Arc<Session>,
        interval: Duration,
    ) {
        let sampler = state.platform.new_stats_sampler();
        let id = session.id;
        let task = tokio::spawn(stats_stream(sampler, session, interval));
        if let Some(old) = lock(&self.stats_subs).insert(id, task) {
            old.abort();
        }
    }

    pub fn unsubscribe_stats(&self, session_id: u64) -> bool {
        match lock(&self.stats_subs).remove(&session_id) {
            Some(t) => {
                t.abort();
                true
            }
            None => false,
        }
    }

    /// Stop streams of sessions that are gone.
    fn prune_stats(&self, alive: &HashSet<u64>) {
        lock(&self.stats_subs).retain(|id, t| {
            let keep = alive.contains(id) && !t.is_finished();
            if !keep {
                t.abort();
            }
            keep
        });
    }

    pub fn stats_subscribed(&self, session_id: u64) -> bool {
        lock(&self.stats_subs).contains_key(&session_id)
    }
}

async fn stats_stream(sampler: Arc<dyn StatsSampler>, session: Arc<Session>, interval: Duration) {
    let mut every = tokio::time::interval(interval);
    every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        every.tick().await;
        let s = sampler.clone();
        match blocking(move || s.sample()).await {
            Ok(stats) => {
                let data = serde_json::to_value(&stats).unwrap_or(Value::Null);
                if !session.send_event("stats.tick", data) {
                    return;
                }
            }
            Err(e) => tracing::debug!("stats sample failed: {}", e.message),
        }
    }
}

/// Parse `stats.subscribe{interval_ms}`.
pub fn stats_interval(params: &Value) -> Result<Duration, RpcError> {
    let ms = params
        .get("interval_ms")
        .and_then(Value::as_u64)
        .ok_or_else(|| RpcError::bad_params("interval_ms (integer >= 500) is required"))?;
    if !(MIN_STATS_INTERVAL_MS..=MAX_STATS_INTERVAL_MS).contains(&ms) {
        return Err(RpcError::bad_params(format!(
            "interval_ms must be {MIN_STATS_INTERVAL_MS}..={MAX_STATS_INTERVAL_MS}"
        )));
    }
    Ok(Duration::from_millis(ms))
}

/// Announce a profile change to every phone.
pub fn announce_rematch(state: &AppState) {
    if let Some(id) = state.p3.rematch() {
        state.broadcast("profile.active", json!({"id": id}));
    }
}

// ---------------------------------------------------------------- background

/// Start the foreground watcher pump and the ticker. Abort the handles to stop them.
pub fn spawn(state: &Arc<AppState>) -> Vec<JoinHandle<()>> {
    vec![
        tokio::spawn(foreground_pump(state.clone())),
        tokio::spawn(ticker(state.clone())),
    ]
}

async fn foreground_pump(state: Arc<AppState>) {
    let (tx, mut rx) = mpsc::unbounded_channel::<ForegroundInfo>();
    let platform = state.platform.clone();
    // The guard lives as long as this task; aborting the task stops the watcher.
    let _guard =
        match tokio::task::spawn_blocking(move || platform.start_foreground_watcher(tx)).await {
            Ok(g) => g,
            Err(_) => return,
        };
    while let Some(info) = rx.recv().await {
        *lock(&state.p3.last_foreground) = Some(info.exe_name.clone());
        let st = state.clone();
        let _ = tokio::task::spawn_blocking(move || announce_rematch(&st)).await;
    }
}

fn widget_text(
    widget: Widget,
    stats: Option<&crate::platform::Stats>,
    clock: &str,
    now_playing: &str,
) -> Option<String> {
    match widget {
        Widget::Cpu => stats.map(|s| format!("CPU {:.0}%", s.cpu_total)),
        Widget::Ram => stats.map(|s| format!("RAM {:.0}%", s.ram_pct())),
        Widget::Clock => Some(clock.to_string()),
        Widget::NowPlaying => Some(now_playing.to_string()),
    }
}

/// Live values the deck and alerts need, read once per tick.
struct Snapshot {
    stats: Option<crate::platform::Stats>,
    mic_muted: Option<bool>,
    audio_muted: Option<bool>,
    media_playing: Option<bool>,
    now_playing: String,
    clock: String,
}

async fn ticker(state: Arc<AppState>) {
    let sampler = state.platform.new_stats_sampler();
    let mut last_state: HashMap<String, bool> = HashMap::new();
    let mut last_widget: HashMap<String, String> = HashMap::new();
    let mut known_sessions: HashSet<u64> = HashSet::new();
    let started = std::time::Instant::now();
    loop {
        tokio::time::sleep(state.p3.tick()).await;
        let alive: HashSet<u64> = state.sessions().iter().map(|s| s.id).collect();
        state.p3.prune_stats(&alive);
        if !alive.is_subset(&known_sessions) {
            // A phone connected: send it everything again.
            last_state.clear();
            last_widget.clear();
        }
        known_sessions = alive.clone();

        let has_rules = lock(&state.p3.alerts).rules().next().is_some();
        let buttons: Vec<DeckButton> = if alive.is_empty() {
            Vec::new()
        } else {
            let st = state.clone();
            blocking(move || {
                st.layouts
                    .deck()
                    .map_err(|e| RpcError::internal(e.to_string()))
            })
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|b| b.state.is_some() || b.widget.is_some())
            .collect()
        };
        if !has_rules && buttons.is_empty() {
            continue;
        }
        let need_stats = has_rules
            || buttons
                .iter()
                .any(|b| matches!(b.widget, Some(Widget::Cpu | Widget::Ram)));
        let binds: HashSet<Bind> = buttons
            .iter()
            .filter_map(|b| b.state.map(|s| s.bind))
            .collect();
        let widgets: HashSet<Widget> = buttons.iter().filter_map(|b| b.widget).collect();
        let platform = state.platform.clone();
        let s = sampler.clone();
        let snap = blocking(move || {
            let stats = if need_stats { s.sample().ok() } else { None };
            let mic_muted = if binds.contains(&Bind::MicMuted) {
                platform.mic_muted().ok().flatten()
            } else {
                None
            };
            let audio_muted = if binds.contains(&Bind::AudioMuted) {
                platform.volume().ok().map(|v| v.muted)
            } else {
                None
            };
            let media =
                if binds.contains(&Bind::MediaPlaying) || widgets.contains(&Widget::NowPlaying) {
                    platform.media_state().ok().flatten()
                } else {
                    None
                };
            let media_playing = media
                .as_ref()
                .map(|m| matches!(m.status, helm_win_system::media::PlaybackStatus::Playing));
            let now_playing = match &media {
                Some(m) if m.artist.is_empty() => m.title.clone(),
                Some(m) => format!("{} \u{2014} {}", m.title, m.artist),
                None => String::new(),
            };
            let clock = if widgets.contains(&Widget::Clock) {
                platform.local_clock().unwrap_or_default()
            } else {
                String::new()
            };
            Ok(Snapshot {
                stats,
                mic_muted,
                audio_muted,
                media_playing: media_playing.or(Some(false)),
                now_playing,
                clock,
            })
        })
        .await;
        let Ok(snap) = snap else { continue };

        if has_rules && let Some(stats) = &snap.stats {
            let now = started.elapsed().as_millis() as u64;
            let events = lock(&state.p3.alerts).evaluate(stats, now);
            for e in events {
                state.broadcast(
                    "alert",
                    json!({"id": e.rule_id, "firing": e.firing, "value": e.value,
                           "mount": e.mount, "message": e.message}),
                );
            }
        }

        for b in &buttons {
            if let Some(binding) = b.state {
                let on = match binding.bind {
                    Bind::MicMuted => snap.mic_muted,
                    Bind::AudioMuted => snap.audio_muted,
                    Bind::MediaPlaying => snap.media_playing,
                };
                if let Some(on) = on
                    && last_state.get(&b.id) != Some(&on)
                {
                    last_state.insert(b.id.clone(), on);
                    state.broadcast("deck.state", json!({"id": b.id, "on": on}));
                }
            }
            if let Some(w) = b.widget
                && let Some(text) =
                    widget_text(w, snap.stats.as_ref(), &snap.clock, &snap.now_playing)
                && last_widget.get(&b.id) != Some(&text)
            {
                last_widget.insert(b.id.clone(), text.clone());
                state.broadcast("deck.widget", json!({"id": b.id, "text": text}));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(id: &str, kind: AlertKind, threshold: f32) -> AlertRule {
        AlertRule {
            id: id.into(),
            kind,
            threshold,
            for_s: None,
            mount: None,
        }
    }

    #[test]
    fn rules_validate_and_map() {
        assert!(validate_rules(&[rule("a", AlertKind::CpuAbove, 90.0)]).is_ok());
        assert!(validate_rules(&[rule("a", AlertKind::CpuAbove, 190.0)]).is_err());
        assert!(
            validate_rules(&[
                rule("a", AlertKind::RamAbove, 1.0),
                rule("a", AlertKind::RamAbove, 2.0)
            ])
            .is_err()
        );
        let r: AlertRule = serde_json::from_value(
            json!({"id":"d","kind":"disk_free_below","threshold":10,"mount":"C:\\"}),
        )
        .unwrap();
        assert_eq!(
            r.to_rule().condition,
            Condition::DiskFreeBelowPct {
                mount: Some("C:\\".into()),
                pct: 10.0
            }
        );
        assert!(
            serde_json::from_value::<AlertRule>(json!({"id":"x","kind":"gpu_hot","threshold":1}))
                .is_err()
        );
    }

    #[test]
    fn profiles_match_case_insensitively() {
        let p = |id: &str, exe: Option<&str>| Profile {
            id: id.into(),
            name: id.into(),
            match_exe: exe.map(Into::into),
            deck_id: None,
            dock_page: None,
        };
        let list = vec![
            p("none", None),
            p("code", Some("Code.exe")),
            p("code2", Some("code.exe")),
        ];
        assert!(validate_profiles(&list).is_ok());
        assert_eq!(match_profile(&list, "CODE.EXE").unwrap().id, "code");
        assert!(match_profile(&list, "").is_none());
        assert!(match_profile(&list, "notepad.exe").is_none());
        assert!(validate_profiles(&[p("a", None), p("a", None)]).is_err());
    }

    #[test]
    fn store_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let st = P3State::new(dir.path());
        assert!(st.alert_rules().unwrap().is_empty());
        st.set_alert_rules(vec![rule("c", AlertKind::CpuAbove, 50.0)])
            .unwrap();
        assert_eq!(P3State::new(dir.path()).alert_rules().unwrap().len(), 1);
        assert_eq!(lock(&st.alerts).rules().count(), 1);
        assert!(stats_interval(&json!({"interval_ms": 499})).is_err());
        assert!(stats_interval(&json!({"interval_ms": 1000})).is_ok());
    }
}
