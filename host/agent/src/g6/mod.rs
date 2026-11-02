//! Everyday features (feature `g6`, Docs/protocol/everyday.md): clipboard history, quick file
//! drop, find my phone, PC notifications on the phone, scheduled actions and the health card.
//! Methods are in `modules/everyday.rs`; every PC effect goes through `Platform`.

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::task::JoinHandle;

use crate::platform::{NotifyAccess, PcNotification, blocking};
use crate::router::RpcError;
use crate::server::AppState;

pub mod clip;
pub mod schedule;
pub mod thumb;

pub(crate) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

pub const NOTIFY_POLL: Duration = Duration::from_secs(2);
pub const SCHEDULE_TICK: Duration = Duration::from_secs(5);
/// App names of Helm's own toasts (never forwarded).
const OWN_APPS: &[&str] = &["Helm", "Helm.Agent"];

pub struct G6State {
    pub clipboard: clip::ClipHistory,
    pub schedules: schedule::Schedules,
    /// Sessions that called `pc.notify.watch{on:true}` with access allowed.
    notify_watchers: Mutex<HashSet<u64>>,
    /// Notification ids already known (baseline taken when watching starts).
    notify_seen: Mutex<Option<HashSet<String>>>,
    notify_poll_ms: AtomicU64,
}

impl G6State {
    pub fn new(data_dir: &Path) -> G6State {
        G6State {
            clipboard: clip::ClipHistory::load(data_dir),
            schedules: schedule::Schedules::load(data_dir),
            notify_watchers: Mutex::new(HashSet::new()),
            notify_seen: Mutex::new(None),
            notify_poll_ms: AtomicU64::new(NOTIFY_POLL.as_millis() as u64),
        }
    }

    /// Change the notification poll interval (tests).
    pub fn set_notify_poll(&self, d: Duration) {
        self.notify_poll_ms
            .store(d.as_millis().max(10) as u64, Ordering::Relaxed);
    }

    pub fn notify_watching(&self) -> usize {
        lock(&self.notify_watchers).len()
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------- clipboard history

/// The clipboard changed (watcher event): record it when the history is on.
pub fn on_clipboard(state: &Arc<AppState>, has_text: bool, has_image: bool) {
    if !state.g6.clipboard.settings().enabled || !(has_text || has_image) {
        return;
    }
    let st = state.clone();
    tokio::spawn(async move {
        let p = st.platform.clone();
        let content = blocking(move || {
            if !p.clipboard_history_allowed()? {
                return Ok(None);
            }
            if has_image && let Some(png) = p.clipboard_png()? {
                return Ok(Some(clip::Content::Image(png)));
            }
            Ok(p.clipboard_text()?.map(clip::Content::Text))
        })
        .await;
        match content {
            Ok(Some(c)) => {
                if st.g6.clipboard.push(c) {
                    st.broadcast("clipboard.history.changed", json!({}));
                }
            }
            Ok(None) => {}
            Err(e) => tracing::debug!("clipboard history: {}", e.message),
        }
    });
}

// ---------------------------------------------------------------- find my phone

/// Send `phone.ring` to `target`'s sessions, or to every device except `except` when
/// `target` is `None`. Returns how many sessions were told.
pub fn ring_phones(
    state: &AppState,
    target: Option<&str>,
    except: Option<&str>,
    from: Value,
    stop: bool,
) -> usize {
    let data = json!({"from": from, "stop": stop});
    let mut n = 0;
    for s in state.sessions() {
        let hit = match target {
            Some(t) => s.device_id == t,
            None => except != Some(s.device_id.as_str()),
        };
        if hit {
            s.send_event("phone.ring", data.clone());
            n += 1;
        }
    }
    n
}

/// Tray "Ring my phone": every connected phone.
pub fn ring_from_pc(state: &AppState) -> usize {
    let n = ring_phones(
        state,
        None,
        None,
        json!({"name": state.identity.host_name}),
        false,
    );
    state
        .audit
        .record("phone_ring", None, None, &format!("from=tray sessions={n}"));
    n
}

// ---------------------------------------------------------------- PC notifications

/// `pc.notify.watch{on}` for one session.
pub async fn notify_watch(
    state: &Arc<AppState>,
    session_id: u64,
    on: bool,
) -> Result<NotifyAccess, RpcError> {
    if !on {
        let empty = {
            let mut w = lock(&state.g6.notify_watchers);
            w.remove(&session_id);
            w.is_empty()
        };
        if empty {
            *lock(&state.g6.notify_seen) = None;
        }
        return Ok(NotifyAccess::Allowed);
    }
    let p = state.platform.clone();
    let access = blocking(move || p.pc_notify_access()).await?;
    if access != NotifyAccess::Allowed {
        lock(&state.g6.notify_watchers).remove(&session_id);
        return Ok(access);
    }
    let first = lock(&state.g6.notify_seen).is_none();
    if first {
        let p = state.platform.clone();
        // A failed baseline must not make everything in Action Center look new later.
        let now = blocking(move || p.pc_notifications()).await?;
        *lock(&state.g6.notify_seen) = Some(now.into_iter().map(|n| n.id).collect());
    }
    lock(&state.g6.notify_watchers).insert(session_id);
    Ok(access)
}

fn clip_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// One poll: forward notifications that are new since the last poll.
pub async fn poll_notifications(state: &Arc<AppState>) {
    let live: HashSet<u64> = state.sessions().iter().map(|s| s.id).collect();
    let none_left = {
        let mut w = lock(&state.g6.notify_watchers);
        w.retain(|id| live.contains(id));
        w.is_empty()
    };
    if none_left {
        *lock(&state.g6.notify_seen) = None;
        return;
    }
    let p = state.platform.clone();
    let Ok(list) = blocking(move || p.pc_notifications()).await else {
        return;
    };
    let fresh: Vec<PcNotification> = {
        let mut seen = lock(&state.g6.notify_seen);
        // Watching stopped while the list was being read: there is no baseline to compare
        // with, and treating everything as new would resend all of Action Center.
        let Some(known) = seen.as_mut() else {
            return;
        };
        let fresh = list
            .iter()
            .filter(|n| !known.contains(&n.id))
            .filter(|n| !OWN_APPS.iter().any(|a| a.eq_ignore_ascii_case(n.app.trim())))
            .cloned()
            .collect();
        *known = list.into_iter().map(|n| n.id).collect();
        fresh
    };
    if fresh.is_empty() {
        return;
    }
    // Who is watching now, not when the poll began.
    let watchers = lock(&state.g6.notify_watchers).clone();
    for s in state.sessions().into_iter().filter(|s| watchers.contains(&s.id)) {
        for n in &fresh {
            s.send_event(
                "pc.notification",
                json!({
                    "id": n.id, "app": clip_chars(&n.app, 100),
                    "title": clip_chars(&n.title, 500), "body": clip_chars(&n.body, 4000),
                    "time_ms": n.time_ms,
                }),
            );
        }
    }
}

// ---------------------------------------------------------------- schedules

/// Check schedules at `now_ms` (the background ticker passes the real time; tests pass any).
pub fn schedule_tick_at(state: &Arc<AppState>, now_ms: u64) {
    let offset = state.platform.utc_offset_minutes();
    let r = state.g6.schedules.tick(now_ms, offset);
    if r.disabled {
        broadcast_schedules(state);
    }
    for s in r.run {
        let st = state.clone();
        tokio::spawn(async move { run_schedule(&st, s).await });
    }
}

pub fn schedules_json(state: &AppState) -> Value {
    schedule::to_json(
        &state.g6.schedules.get(),
        now_ms(),
        state.platform.utc_offset_minutes(),
    )
}

pub fn broadcast_schedules(state: &AppState) {
    state.broadcast("schedule.changed", json!({"schedules": schedules_json(state)}));
}

async fn run_schedule(state: &Arc<AppState>, s: schedule::Schedule) {
    let r = match &s.action {
        schedule::SchedAction::Power(p) => {
            let backend = state.power_backend.clone();
            let action = p.power;
            match tokio::task::spawn_blocking(move || backend.execute(action)).await {
                Ok(Ok(())) => Ok(()),
                Ok(Err(e)) => Err(RpcError::internal(e.to_string())),
                Err(e) => Err(RpcError::internal(e.to_string())),
            }
        }
        schedule::SchedAction::Deck(a) => crate::modules::layout::run_action(state, a.clone()).await,
    };
    let detail = match &r {
        Ok(()) => format!("id={} ran", s.id),
        Err(e) => format!("id={} failed: {}", s.id, e.message),
    };
    state.audit.record("schedule_run", None, None, &detail);
    let mut data = json!({"id": s.id, "ok": r.is_ok()});
    if let Err(e) = r {
        data["error"] = Value::String(e.message);
    }
    state.broadcast("schedule.ran", data);
}

// ---------------------------------------------------------------- health

/// `system.health`.
pub async fn health(state: &Arc<AppState>) -> Result<Value, RpcError> {
    let p = state.platform.clone();
    let agent_uptime_s = state.started.elapsed().as_secs();
    let (battery, temp, disks, uptime) = blocking(move || {
        let battery = p.battery().ok().flatten();
        let temp = p.cpu_temp_c().ok().flatten();
        let disks = p
            .new_stats_sampler()
            .sample()
            .map(|s| s.disks)
            .unwrap_or_default();
        let uptime = p.system_uptime_s().ok();
        Ok((battery, temp, disks, uptime))
    })
    .await?;
    let battery = battery.map(|b| {
        json!({"percent": b.percent, "charging": b.charging, "ac_online": b.ac_online})
    });
    let disks: Vec<Value> = disks
        .iter()
        .map(|d| {
            json!({"mount": d.mount, "label": d.label,
                   "free_bytes": d.total.saturating_sub(d.used), "total_bytes": d.total})
        })
        .collect();
    Ok(json!({
        "battery": battery,
        "cpu_temp_c": temp.filter(|t| t.is_finite()),
        "disks": disks,
        "uptime_s": uptime,
        "agent_uptime_s": agent_uptime_s,
    }))
}

// ---------------------------------------------------------------- background

/// Notification poller and schedule ticker. Abort the handles to stop.
pub fn spawn(state: &Arc<AppState>) -> Vec<JoinHandle<()>> {
    let notify = {
        let st = state.clone();
        tokio::spawn(async move {
            loop {
                let nap = Duration::from_millis(st.g6.notify_poll_ms.load(Ordering::Relaxed));
                tokio::time::sleep(nap).await;
                if st.g6.notify_watching() > 0 {
                    poll_notifications(&st).await;
                }
            }
        })
    };
    let ticker = {
        let st = state.clone();
        tokio::spawn(async move {
            loop {
                schedule_tick_at(&st, now_ms());
                tokio::time::sleep(SCHEDULE_TICK).await;
            }
        })
    };
    vec![notify, ticker]
}
