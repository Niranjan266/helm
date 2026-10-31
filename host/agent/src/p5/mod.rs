//! Phase 5 shared state and background work (PROTOCOL §13): recordings, terminals, plugins,
//! triggers and Home Assistant. Trigger inputs: the foreground watcher (`app_start`), phone
//! sessions (`phone_connected`), a Wi-Fi SSID poll (`wifi`) and a clock tick (`time`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use helm_plugins::trigger::Event;
use serde_json::Value;
use tokio::task::JoinHandle;

use crate::platform::blocking;
use crate::router::RpcError;
use crate::server::AppState;
use crate::store::{DeckAction, HaCall};

pub mod ha;
pub mod plugins;
pub mod record;
pub mod term;
pub mod triggers;

pub(crate) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Defaults: time triggers are checked every 30 s, the Wi-Fi SSID every 5 s.
pub const TRIGGER_TICK: Duration = Duration::from_secs(30);
pub const WIFI_POLL: Duration = Duration::from_secs(5);

pub struct P5State {
    data_dir: PathBuf,
    pub videos_dir: PathBuf,
    pub(crate) recordings: Mutex<HashMap<u64, Arc<record::Recording>>>,
    pub terms: term::Terms,
    pub triggers: triggers::Triggers,
    pub plugins: plugins::Plugins,
    ha: Mutex<Option<ha::HaConfig>>,
    /// Open control sessions per device (a device may connect twice).
    phones: Mutex<HashMap<String, usize>>,
    foreground: Mutex<Option<String>>,
    wifi: Mutex<Option<Option<String>>>,
    tick_ms: AtomicU64,
    wifi_ms: AtomicU64,
}

impl P5State {
    pub fn new(data_dir: &Path, videos_dir: PathBuf, tick: Duration, wifi_poll: Duration) -> P5State {
        let ha = match ha::load(data_dir) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("{} unreadable: {e:#}", ha::HA_FILE);
                None
            }
        };
        P5State {
            data_dir: data_dir.to_path_buf(),
            videos_dir,
            recordings: Mutex::new(HashMap::new()),
            terms: term::Terms::default(),
            triggers: triggers::Triggers::load(data_dir, triggers::AgentClock::system()),
            plugins: plugins::Plugins::new(data_dir),
            ha: Mutex::new(ha),
            phones: Mutex::new(HashMap::new()),
            foreground: Mutex::new(None),
            wifi: Mutex::new(None),
            tick_ms: AtomicU64::new(tick.as_millis().max(10) as u64),
            wifi_ms: AtomicU64::new(wifi_poll.as_millis().max(10) as u64),
        }
    }

    pub fn recording_count(&self) -> usize {
        lock(&self.recordings).len()
    }

    pub fn ha_config(&self) -> Option<ha::HaConfig> {
        lock(&self.ha).clone()
    }

    /// Validate, store (token DPAPI-encrypted) and use a new Home Assistant config.
    pub fn set_ha_config(&self, config: ha::HaConfig) -> anyhow::Result<()> {
        ha::save(&self.data_dir, &config)?;
        *lock(&self.ha) = Some(config);
        Ok(())
    }

    fn tick(&self) -> Duration {
        Duration::from_millis(self.tick_ms.load(Ordering::Relaxed))
    }

    fn wifi_poll(&self) -> Duration {
        Duration::from_millis(self.wifi_ms.load(Ordering::Relaxed))
    }
}

// ---------------------------------------------------------------- triggers + plugins

fn event_json(event: &Event) -> Value {
    serde_json::to_value(event).unwrap_or(Value::Null)
}

/// Feed one event to the trigger engine (running what fires) and to the plugins.
pub fn feed(state: &Arc<AppState>, event: Event) {
    let fired = state.p5.triggers.handle(&event);
    run_fired(state, fired);
    if state.p5.plugins.has_running() {
        let st = state.clone();
        let data = event_json(&event);
        tokio::task::spawn_blocking(move || st.p5.plugins.dispatch(&data));
    }
}

fn run_fired(state: &Arc<AppState>, fired: Vec<(String, DeckAction)>) {
    for (id, action) in fired {
        let st = state.clone();
        tokio::spawn(async move {
            tracing::info!("trigger {id} fired");
            if let Err(e) = crate::modules::layout::run_action(&st, action).await {
                tracing::info!("trigger {id}: action failed: {}", e.message);
            }
        });
    }
}

/// The foreground app changed (from the Phase 3 watcher): `app_start` triggers.
pub fn on_foreground(state: &Arc<AppState>, exe_name: &str) {
    let exe = exe_name.trim().to_string();
    let previous = {
        let mut fg = lock(&state.p5.foreground);
        if fg.as_deref().is_some_and(|p| p.eq_ignore_ascii_case(&exe)) {
            return;
        }
        std::mem::replace(&mut *fg, (!exe.is_empty()).then(|| exe.clone()))
    };
    if let Some(prev) = previous {
        feed(state, Event::AppExited { exe: prev });
    }
    if !exe.is_empty() {
        feed(state, Event::AppStarted { exe });
    }
}

/// A control session became ready: `phone_connected` triggers.
pub fn session_started(state: &Arc<AppState>, device_id: &str) {
    let first = {
        let mut phones = lock(&state.p5.phones);
        let n = phones.entry(device_id.to_string()).or_insert(0);
        *n += 1;
        *n == 1
    };
    if first {
        feed(
            state,
            Event::PhoneConnected {
                device_id: device_id.to_string(),
            },
        );
    }
}

/// A control session ended: stop its recording, kill its shells.
pub async fn session_ended(state: &Arc<AppState>, session_id: u64, device_id: &str) {
    if lock(&state.p5.recordings).contains_key(&session_id)
        && let Err(e) = record::stop(state, session_id).await
    {
        tracing::info!("recording of session {session_id}: {}", e.message);
    }
    state.p5.terms.close_session(session_id).await;
    let last = {
        let mut phones = lock(&state.p5.phones);
        match phones.get_mut(device_id) {
            Some(n) if *n > 1 => {
                *n -= 1;
                false
            }
            Some(_) => {
                phones.remove(device_id);
                true
            }
            None => false,
        }
    };
    if last {
        feed(
            state,
            Event::PhoneDisconnected {
                device_id: device_id.to_string(),
            },
        );
    }
}

/// Poll the Wi-Fi SSID once; feed a change to the triggers.
pub async fn poll_wifi(state: &Arc<AppState>) {
    let p = state.platform.clone();
    let ssid = blocking(move || p.wifi_ssid()).await.ok().flatten();
    let changed = {
        let mut last = lock(&state.p5.wifi);
        if last.as_ref() == Some(&ssid) {
            false
        } else {
            *last = Some(ssid.clone());
            true
        }
    };
    if changed {
        match ssid {
            Some(ssid) => feed(state, Event::WifiConnected { ssid }),
            None => feed(state, Event::WifiDisconnected),
        }
    }
}

/// Check time triggers now.
pub fn clock_tick(state: &Arc<AppState>) {
    let p5 = &state.p5;
    p5.triggers
        .clock()
        .set_offset(state.platform.utc_offset_minutes());
    let fired = p5.triggers.tick();
    run_fired(state, fired);
}

/// Start the Phase 5 background work. Abort the handles to stop it.
pub fn spawn(state: &Arc<AppState>) -> Vec<JoinHandle<()>> {
    let loader = {
        let st = state.clone();
        tokio::spawn(async move {
            let s = st.clone();
            let _ = tokio::task::spawn_blocking(move || s.p5.plugins.load_all(&s)).await;
        })
    };
    let ticker = {
        let st = state.clone();
        tokio::spawn(async move {
            let mut last_tick: Option<Instant> = None;
            let mut last_wifi: Option<Instant> = None;
            loop {
                let now = Instant::now();
                if last_wifi.is_none_or(|t| now.duration_since(t) >= st.p5.wifi_poll()) {
                    last_wifi = Some(now);
                    poll_wifi(&st).await;
                }
                if last_tick.is_none_or(|t| now.duration_since(t) >= st.p5.tick()) {
                    last_tick = Some(now);
                    clock_tick(&st);
                }
                let nap = st.p5.wifi_poll().min(st.p5.tick()).min(Duration::from_secs(1));
                tokio::time::sleep(nap).await;
            }
        })
    };
    vec![loader, ticker]
}

// ---------------------------------------------------------------- Home Assistant

fn ha_client(state: &AppState) -> Result<helm_plugins::ha::HaClient, RpcError> {
    let config = state
        .p5
        .ha_config()
        .ok_or_else(|| RpcError::bad_params("Home Assistant is not set up (ha.config.set)"))?;
    ha::client(&config, state.platform.http())
}

/// Deck / trigger action `{"ha":{"service","entity_id","data"}}`.
pub async fn ha_call(state: &Arc<AppState>, call: &HaCall) -> Result<(), RpcError> {
    let (domain, service, data) = call.parts().map_err(RpcError::bad_params)?;
    let client = ha_client(state)?;
    blocking(move || {
        client
            .call_service(&domain, &service, &data)
            .map(|_| ())
            .map_err(ha::ha_err)
    })
    .await
}

/// `ha.states`.
pub async fn ha_states(state: &Arc<AppState>) -> Result<Value, RpcError> {
    let client = ha_client(state)?;
    let states = blocking(move || client.states().map_err(ha::ha_err)).await?;
    Ok(serde_json::to_value(states).unwrap_or(Value::Array(Vec::new())))
}
