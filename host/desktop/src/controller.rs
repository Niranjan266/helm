//! Owns the in-process agent (`helm_agent::ServerHandle`): start / pause / resume / stop,
//! the current pairing code, live connections, and the views the window asks for.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use helm_agent::audit::AuditLog;
use helm_agent::desktop::InstanceGuard;
use helm_agent::devices::{Device, DeviceStore};
use helm_agent::relay::{self, Relay};
use helm_agent::router::Tier;
use helm_agent::{AgentEvent, AppState, Config, Pairing, ServerHandle};
use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter};
use tokio::sync::broadcast::error::RecvError;

use crate::activity::{self, Entry};
use crate::settings::Settings;
use crate::status::{self, Headline, RunState};

/// Window event: something changed; payload `{"kind": "status"|"phones"|"activity"|"pairing"}`.
pub const CHANGED_EVENT: &str = "helm://changed";
/// Window event: switch page; payload is the page name.
pub const NAVIGATE_EVENT: &str = "helm://navigate";
/// How the desktop app is named in audit lines it writes.
const ACTOR: &str = "desktop";

#[derive(Clone, Debug)]
struct Conn {
    device_id: String,
    peer: String,
}

#[derive(Clone, Debug)]
struct IdentityInfo {
    host_name: String,
    host_id: String,
    fingerprint: String,
}

struct Inner {
    run: RunState,
    error: Option<String>,
    conns: BTreeMap<u64, Conn>,
    pairing: Option<Pairing>,
    started: Option<Instant>,
    identity: Option<IdentityInfo>,
    guard: Option<InstanceGuard>,
    pump: Option<tauri::async_runtime::JoinHandle<()>>,
    state: Option<Arc<AppState>>,
}

pub struct Controller {
    pub data_dir: PathBuf,
    app: AppHandle,
    server: tokio::sync::Mutex<Option<ServerHandle>>,
    inner: Mutex<Inner>,
}

// ---------------------------------------------------------------- views

#[derive(Serialize)]
pub struct RelayView {
    pub enabled: bool,
    pub url: String,
    pub connected: bool,
    pub last_error: Option<String>,
}

#[derive(Serialize)]
pub struct StatusView {
    pub run: RunState,
    pub headline: Headline,
    pub host_name: String,
    pub host_first: String,
    pub host_rest: String,
    pub host_id: Option<String>,
    pub fingerprint: Option<String>,
    pub version: &'static str,
    pub port: Option<u16>,
    pub configured_port: u16,
    pub addresses: Vec<String>,
    pub uptime_s: Option<u64>,
    pub phones_online: usize,
    pub phones_total: usize,
    pub phones_summary: String,
    pub relay: RelayView,
    pub error: Option<String>,
    pub data_dir: String,
}

#[derive(Serialize)]
pub struct PhoneView {
    pub device_id: String,
    pub name: String,
    pub tier: Tier,
    pub paired_at_ms: i64,
    pub last_seen_ms: i64,
    pub online: bool,
    /// "lan" or "relay" while connected.
    pub via: Option<&'static str>,
    pub peer: Option<String>,
}

#[derive(Serialize)]
pub struct PairingView {
    pub uri: String,
    pub svg: String,
    pub expires_in_s: u64,
}

#[derive(Serialize)]
pub struct SettingsView {
    pub autostart: bool,
    pub autostart_key: String,
    pub port: u16,
    pub relay_enabled: bool,
    pub relay_url: String,
    pub data_dir: String,
    pub version: &'static str,
}

impl Controller {
    pub fn new(app: AppHandle, data_dir: PathBuf) -> Controller {
        Controller {
            data_dir,
            app,
            server: tokio::sync::Mutex::new(None),
            inner: Mutex::new(Inner {
                run: RunState::Starting,
                error: None,
                conns: BTreeMap::new(),
                pairing: None,
                started: None,
                identity: None,
                guard: None,
                pump: None,
                state: None,
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn agent(&self) -> Option<Arc<AppState>> {
        self.lock().state.clone()
    }

    pub fn run_state(&self) -> RunState {
        self.lock().run
    }

    /// Tell the window (and the tray) that `kind` changed.
    pub fn notify(&self, kind: &str) {
        let _ = self.app.emit(CHANGED_EVENT, json!({ "kind": kind }));
        if kind == "status" || kind == "phones" {
            let (run, online) = {
                let inner = self.lock();
                (inner.run, online_devices(&inner.conns).len())
            };
            crate::tray::refresh(&self.app, run, online);
        }
    }

    fn set_run(&self, run: RunState, error: Option<String>) {
        {
            let mut inner = self.lock();
            inner.run = run;
            inner.error = error;
        }
        self.notify("status");
    }

    // ------------------------------------------------------------ lifecycle

    /// Start the agent (no-op if it runs). Errors end up in the status view.
    pub async fn start(self: &Arc<Self>) {
        let mut server = self.server.lock().await;
        if server.is_some() {
            return;
        }
        self.set_run(RunState::Starting, None);
        let have_guard = {
            let mut inner = self.lock();
            if inner.guard.is_none() {
                inner.guard = helm_agent::desktop::single_instance(&self.data_dir);
            }
            inner.guard.is_some()
        };
        if !have_guard {
            tracing::warn!("helm-agent already serves {}", self.data_dir.display());
            self.set_run(RunState::Conflict, None);
            return;
        }
        let mut config = Config::new(&self.data_dir);
        config.port = Settings::load(&self.data_dir).port;
        config.enable_mdns = true;
        // Boxed: the agent's start future is deep enough to overflow rustc's stack when
        // checked inline inside the Tauri async runtime's Send bounds.
        match Box::pin(helm_agent::start(config)).await {
            Ok(handle) => {
                let events = handle.subscribe();
                let audit = handle.state().audit.subscribe();
                let pump = tauri::async_runtime::spawn(pump(self.clone(), events, audit));
                {
                    let mut inner = self.lock();
                    inner.state = Some(handle.state().clone());
                    inner.identity = Some(IdentityInfo {
                        host_name: handle.host_name().to_string(),
                        host_id: handle.host_id().to_string(),
                        fingerprint: handle.fingerprint().to_string(),
                    });
                    inner.started = Some(Instant::now());
                    inner.conns.clear();
                    inner.pump = Some(pump);
                }
                tracing::info!("agent running on port {}", handle.port());
                *server = Some(handle);
                self.set_run(RunState::Running, None);
            }
            Err(e) => {
                tracing::error!("agent failed to start: {e:#}");
                self.set_run(RunState::Failed, Some(format!("{e:#}")));
            }
        }
    }

    /// Stop the agent and end in `next` (`Paused`, or `Stopping` when quitting).
    pub async fn stop(&self, next: RunState) {
        let mut server = self.server.lock().await;
        let Some(handle) = server.take() else {
            if self.run_state() != RunState::Conflict || next == RunState::Stopping {
                self.set_run(next, None);
            }
            return;
        };
        self.set_run(RunState::Stopping, None);
        Box::pin(handle.shutdown()).await;
        {
            let mut inner = self.lock();
            if let Some(p) = inner.pump.take() {
                p.abort();
            }
            inner.state = None;
            inner.conns.clear();
            inner.pairing = None;
            inner.started = None;
        }
        self.set_run(next, None);
        self.notify("phones");
        self.notify("pairing");
    }

    pub async fn set_paused(self: &Arc<Self>, paused: bool) {
        if paused {
            self.stop(RunState::Paused).await;
        } else {
            self.start().await;
        }
    }

    /// Restart so a new port takes effect.
    pub async fn restart(self: &Arc<Self>) {
        self.stop(RunState::Paused).await;
        self.start().await;
    }

    // ------------------------------------------------------------ pairing

    fn pairing_view(p: &Pairing) -> Result<PairingView, String> {
        let svg = helm_agent::qr::to_svg(&p.uri).map_err(|e| format!("{e:#}"))?;
        Ok(PairingView {
            uri: p.uri.clone(),
            svg,
            expires_in_s: p
                .expires_at
                .saturating_duration_since(Instant::now())
                .as_secs(),
        })
    }

    /// The active (unexpired) pairing code, if any.
    pub fn current_pairing(&self) -> Result<Option<PairingView>, String> {
        let mut inner = self.lock();
        if inner
            .pairing
            .as_ref()
            .is_some_and(|p| p.expires_at <= Instant::now())
        {
            inner.pairing = None;
        }
        inner.pairing.as_ref().map(Self::pairing_view).transpose()
    }

    /// Issue a fresh one-time code (the previous one stops working).
    pub fn new_pairing(&self) -> Result<PairingView, String> {
        let state = self
            .agent()
            .ok_or("Helm is not running. Resume connections first.")?;
        let p = state.new_pairing();
        let view = Self::pairing_view(&p)?;
        self.lock().pairing = Some(p);
        self.notify("pairing");
        Ok(view)
    }

    /// Keep an active code, or issue one.
    pub fn ensure_pairing(&self) -> Result<PairingView, String> {
        match self.current_pairing()? {
            Some(v) => Ok(v),
            None => self.new_pairing(),
        }
    }

    // ------------------------------------------------------------ views

    fn device_store_list(&self) -> Vec<Device> {
        let list = match self.agent() {
            Some(st) => st.devices.list(),
            None => DeviceStore::new(&self.data_dir).list(),
        };
        list.unwrap_or_else(|e| {
            tracing::warn!("could not read devices: {e:#}");
            Vec::new()
        })
    }

    fn relay_view(&self) -> RelayView {
        match self.agent() {
            Some(st) => {
                let s = st.relay.settings();
                let live = st.relay.status();
                RelayView {
                    enabled: s.enabled,
                    url: s.url,
                    connected: live.connected,
                    last_error: live.last_error,
                }
            }
            None => {
                let s = Relay::load(&self.data_dir).settings();
                RelayView {
                    enabled: s.enabled,
                    url: s.url,
                    connected: false,
                    last_error: None,
                }
            }
        }
    }

    pub fn status(&self) -> StatusView {
        let devices = self.device_store_list();
        let relay = self.relay_view();
        let state = self.agent();
        let inner = self.lock();
        let online_ids = online_devices(&inner.conns);
        let names: HashMap<&str, &str> = devices
            .iter()
            .map(|d| (d.device_id.as_str(), d.device_name.as_str()))
            .collect();
        let online_names: Vec<String> = online_ids
            .iter()
            .map(|id| names.get(id.as_str()).copied().unwrap_or("A phone").to_string())
            .collect();
        let headline = status::headline(
            inner.run,
            &online_names,
            devices.len(),
            inner.error.as_deref(),
        );
        let host_name = inner
            .identity
            .as_ref()
            .map(|i| i.host_name.clone())
            .unwrap_or_else(helm_agent::identity::computer_name);
        let (host_first, host_rest) = status::split_host_name(&host_name);
        StatusView {
            run: inner.run,
            headline,
            host_first,
            host_rest,
            host_name,
            host_id: inner.identity.as_ref().map(|i| i.host_id.clone()),
            fingerprint: inner.identity.as_ref().map(|i| i.fingerprint.clone()),
            version: env!("CARGO_PKG_VERSION"),
            port: state.as_ref().map(|s| s.port),
            configured_port: Settings::load(&self.data_dir).port,
            addresses: state
                .as_ref()
                .map(|s| s.addresses.iter().map(|a| a.to_string()).collect())
                .unwrap_or_default(),
            uptime_s: inner.started.map(|t| t.elapsed().as_secs()),
            phones_online: online_ids.len(),
            phones_total: devices.len(),
            phones_summary: status::phones_summary(online_ids.len(), devices.len()),
            relay,
            error: inner.error.clone(),
            data_dir: self.data_dir.display().to_string(),
        }
    }

    pub fn phones(&self) -> Vec<PhoneView> {
        let devices = self.device_store_list();
        let inner = self.lock();
        let mut out: Vec<PhoneView> = devices
            .into_iter()
            .map(|d| {
                let conn = inner.conns.values().find(|c| c.device_id == d.device_id);
                PhoneView {
                    online: conn.is_some(),
                    via: conn.map(|c| {
                        if relay::is_relayed_peer(&c.peer) {
                            "relay"
                        } else {
                            "lan"
                        }
                    }),
                    peer: conn.map(|c| c.peer.clone()),
                    device_id: d.device_id,
                    name: d.device_name,
                    tier: d.tier,
                    paired_at_ms: d.paired_at_ms,
                    last_seen_ms: d.last_seen_ms,
                }
            })
            .collect();
        out.sort_by(|a, b| {
            b.online
                .cmp(&a.online)
                .then(b.last_seen_ms.cmp(&a.last_seen_ms))
        });
        out
    }

    fn audit(&self) -> (Option<Arc<AppState>>, AuditLog) {
        (self.agent(), AuditLog::new(&self.data_dir))
    }

    fn record(&self, event: &str, device_id: &str, detail: &str) {
        match self.audit() {
            (Some(st), _) => st.audit.record(event, Some(device_id), Some(ACTOR), detail),
            (None, log) => {
                log.record(event, Some(device_id), Some(ACTOR), detail);
                self.notify("activity");
            }
        }
    }

    pub fn activity(&self, limit: usize) -> Result<Vec<Entry>, String> {
        let names: HashMap<String, String> = self
            .device_store_list()
            .into_iter()
            .map(|d| (d.device_id, d.device_name))
            .collect();
        let lines = match self.audit() {
            (Some(st), _) => st.audit.tail(limit),
            (None, log) => log.tail(limit),
        }
        .map_err(|e| e.to_string())?;
        Ok(activity::parse_lines(&lines, &names))
    }

    pub fn revoke(&self, device_id: &str) -> Result<bool, String> {
        let removed = match self.agent() {
            Some(st) => st.devices.revoke(device_id),
            None => DeviceStore::new(&self.data_dir).revoke(device_id),
        }
        .map_err(|e| format!("{e:#}"))?;
        if removed {
            self.record("revoke", device_id, "by=desktop");
            if let Some(st) = self.agent() {
                st.kick_device(device_id, None);
            }
            tracing::info!("device {device_id} revoked from the desktop app");
        }
        self.notify("phones");
        self.notify("status");
        Ok(removed)
    }

    pub fn set_tier(&self, device_id: &str, tier: Tier) -> Result<(), String> {
        let found = match self.agent() {
            Some(st) => st.devices.set_tier(device_id, tier),
            None => DeviceStore::new(&self.data_dir).set_tier(device_id, tier),
        }
        .map_err(|e| format!("{e:#}"))?;
        if !found {
            return Err("That phone is no longer paired.".into());
        }
        self.record(
            "set_tier",
            device_id,
            &format!("tier={} by=desktop", tier.as_str()),
        );
        // Open sessions keep the tier they signed in with; make them reconnect.
        if let Some(st) = self.agent() {
            st.kick_device(device_id, None);
        }
        self.notify("phones");
        Ok(())
    }

    pub fn settings(&self) -> SettingsView {
        let relay = self.relay_view();
        SettingsView {
            autostart: crate::autostart::is_enabled(),
            autostart_key: crate::autostart::registry_path(),
            port: Settings::load(&self.data_dir).port,
            relay_enabled: relay.enabled,
            relay_url: relay.url,
            data_dir: self.data_dir.display().to_string(),
            version: env!("CARGO_PKG_VERSION"),
        }
    }

    pub fn set_relay(&self, enabled: bool, url: &str) -> Result<(), String> {
        let saved = match self.agent() {
            Some(st) => st.relay.set(Some(enabled), Some(url))?,
            None => Relay::load(&self.data_dir).set(Some(enabled), Some(url))?,
        };
        self.record(
            "relay_set",
            "-",
            &format!("enabled={} url={}", saved.enabled, saved.url),
        );
        self.notify("status");
        Ok(())
    }
}

/// Distinct connected device ids, in session order.
fn online_devices(conns: &BTreeMap<u64, Conn>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in conns.values() {
        if !out.contains(&c.device_id) {
            out.push(c.device_id.clone());
        }
    }
    out
}

/// Forward agent events to the window until the agent stops.
async fn pump(
    ctl: Arc<Controller>,
    mut events: tokio::sync::broadcast::Receiver<AgentEvent>,
    mut audit: tokio::sync::broadcast::Receiver<String>,
) {
    loop {
        tokio::select! {
            ev = events.recv() => match ev {
                Ok(AgentEvent::Connected { session_id, device_id, peer, .. }) => {
                    ctl.lock().conns.insert(session_id, Conn { device_id, peer });
                    ctl.notify("phones");
                    ctl.notify("status");
                }
                Ok(AgentEvent::Disconnected { session_id, .. }) => {
                    ctl.lock().conns.remove(&session_id);
                    ctl.notify("phones");
                    ctl.notify("status");
                }
                Err(RecvError::Lagged(_)) => resync(&ctl),
                Err(RecvError::Closed) => break,
            },
            line = audit.recv() => match line {
                Ok(line) => {
                    if line.split('\t').nth(1) == Some("pair_ok") {
                        // The code was used up; a new phone appeared.
                        ctl.lock().pairing = None;
                        ctl.notify("pairing");
                        ctl.notify("phones");
                    }
                    ctl.notify("activity");
                }
                Err(RecvError::Lagged(_)) => ctl.notify("activity"),
                Err(RecvError::Closed) => break,
            },
        }
    }
}

/// After missing events: rebuild the connection list from the agent's sessions.
fn resync(ctl: &Controller) {
    let Some(st) = ctl.agent() else { return };
    let sessions = st.sessions();
    {
        let mut inner = ctl.lock();
        let old = std::mem::take(&mut inner.conns);
        for s in sessions {
            let peer = old.get(&s.id).map(|c| c.peer.clone()).unwrap_or_default();
            inner.conns.insert(
                s.id,
                Conn {
                    device_id: s.device_id.clone(),
                    peer,
                },
            );
        }
    }
    ctl.notify("phones");
    ctl.notify("status");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn online_devices_are_distinct_and_ordered() {
        let mut conns = BTreeMap::new();
        let c = |d: &str| Conn {
            device_id: d.into(),
            peer: String::new(),
        };
        conns.insert(3, c("b"));
        conns.insert(1, c("a"));
        conns.insert(2, c("b"));
        assert_eq!(online_devices(&conns), vec!["a", "b"]);
        assert!(online_devices(&BTreeMap::new()).is_empty());
    }
}
