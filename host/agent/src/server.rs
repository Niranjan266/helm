//! TLS WebSocket server: handshake (pair / auth), then the request loop.

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use anyhow::Context;
use futures_util::{SinkExt, StreamExt};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, WebSocketConfig};

use helm_win_system::power::{DryRunBackend, PowerBackend, PowerScheduler, SystemPowerBackend};

use crate::audit::AuditLog;
use crate::devices::{Device, DeviceStore};
use crate::identity::{self, Identity};
use crate::mdns::Advertiser;
use crate::pairing::{self, Pairing, PairingState, TokenCheck};
use crate::platform::{Platform, PlatformEvent, WatchGuard, real::RealPlatform};
use crate::proto::{self, Handshake, codes};
use crate::router::{RequestContext, Router, Tier};
use crate::session::{Outgoing, Session};
use crate::store::LayoutStore;
use crate::stream::{ActiveStream, TokenStore};
use crate::{files, modules, net, paths, util};

pub const DEFAULT_PORT: u16 = 47820;
pub const WS_PATH: &str = "/ws";
/// Second socket for screen / audio frames (PROTOCOL §10).
pub const STREAM_PATH: &str = "/stream";

/// Settings for an in-process server.
#[derive(Clone, Debug)]
pub struct Config {
    /// Where identity.json, devices.json, audit.log and pair-qr.svg live.
    pub data_dir: PathBuf,
    /// TCP port. 0 = any free port. If the port is busy a free port is used instead.
    pub port: u16,
    pub enable_mdns: bool,
    /// Time allowed between `hello` and `pair`/`auth` (10 s by protocol).
    pub handshake_timeout: Duration,
    pub token_lifetime: Duration,
    pub lockout: Duration,
    /// Override the computer name (defaults to the Windows computer name).
    pub host_name: Option<String>,
    /// Override the addresses put in the pairing link / mDNS (defaults to LAN IPv4s).
    pub addresses: Option<Vec<Ipv4Addr>>,
    /// What the agent controls. `None` = this PC. Tests pass a fake.
    pub platform: Option<Arc<dyn Platform>>,
    /// Record power actions instead of performing them (tests).
    pub power_dry_run: bool,
    /// Where uploads are saved (default `<Downloads>\Helm`).
    pub downloads_dir: Option<PathBuf>,
    /// Only files under this folder can be downloaded (default: the user's profile folder).
    pub download_root: Option<PathBuf>,
    /// How often the window list is checked for `windows.changed` while phones are connected.
    pub windows_poll: Duration,
    /// Lifetime of `stream.start` tokens (30 s by protocol; tests shorten it).
    pub stream_token_lifetime: Duration,
    /// Where recordings are saved (default `<Videos>\Helm`).
    pub videos_dir: Option<PathBuf>,
    /// How often time triggers are checked (30 s; tests shorten it).
    pub trigger_tick: Duration,
    /// How often the Wi-Fi SSID is read for `wifi` triggers (5 s).
    pub wifi_poll: Duration,
}

impl Config {
    pub fn new(data_dir: impl Into<PathBuf>) -> Config {
        Config {
            data_dir: data_dir.into(),
            port: DEFAULT_PORT,
            enable_mdns: true,
            handshake_timeout: Duration::from_secs(10),
            token_lifetime: pairing::TOKEN_LIFETIME,
            lockout: pairing::LOCKOUT,
            host_name: None,
            addresses: None,
            platform: None,
            power_dry_run: false,
            downloads_dir: None,
            download_root: None,
            windows_poll: Duration::from_secs(2),
            stream_token_lifetime: crate::stream::TOKEN_LIFETIME,
            videos_dir: None,
            trigger_tick: crate::p5::TRIGGER_TICK,
            wifi_poll: crate::p5::WIFI_POLL,
        }
    }
}

/// Default data folder: the signed-in user's `%APPDATA%\Helm` (also when started by
/// `helm-service`, which passes the user's environment), else `./helm-data`.
pub fn default_data_dir() -> PathBuf {
    match paths::roaming_app_data() {
        Some(appdata) => appdata.join("Helm"),
        None => PathBuf::from("helm-data"),
    }
}

/// Shared state of a running agent.
pub struct AppState {
    pub identity: Identity,
    pub devices: DeviceStore,
    pub audit: AuditLog,
    pub started: Instant,
    pub port: u16,
    pub addresses: Vec<Ipv4Addr>,
    pub data_dir: PathBuf,
    pub platform: Arc<dyn Platform>,
    pub layouts: LayoutStore,
    pub power: PowerScheduler,
    /// Performs immediate power actions (the scheduler has its own handle to it).
    pub power_backend: Arc<dyn PowerBackend>,
    /// Set when `Config::power_dry_run`: what would have run.
    pub power_dry_run: Option<Arc<DryRunBackend>>,
    pub downloads_dir: PathBuf,
    pub download_root: PathBuf,
    /// Phase 4 relay settings and status (`relay.json`).
    pub relay: crate::relay::Relay,
    /// Phase 3: alerts, profiles, stats streams.
    pub p3: crate::p3::P3State,
    /// Phase 5: recordings, terminals, plugins, triggers, Home Assistant.
    pub p5: crate::p5::P5State,
    windows_poll: Duration,
    pairing: Mutex<PairingState>,
    sessions: Mutex<HashMap<u64, Arc<Session>>>,
    next_session: AtomicU64,
    router: Router,
    handshake_timeout: Duration,
    /// Pending `stream.start` tokens → the stream they open.
    stream_tokens: Mutex<TokenStore<Weak<ActiveStream>>>,
    pub(crate) stream_token_lifetime: Duration,
    next_stream: AtomicU64,
}

impl AppState {
    pub fn new(config: &Config, port: u16, addresses: Vec<Ipv4Addr>) -> anyhow::Result<AppState> {
        std::fs::create_dir_all(&config.data_dir)
            .with_context(|| format!("creating {}", config.data_dir.display()))?;
        let host_name = config
            .host_name
            .clone()
            .unwrap_or_else(identity::computer_name);
        let identity = Identity::load_or_create(&config.data_dir, host_name)?;
        let platform: Arc<dyn Platform> = match &config.platform {
            Some(p) => p.clone(),
            None => Arc::new(RealPlatform::new(&config.data_dir.join("icon-cache"))),
        };
        let power_dry_run = config.power_dry_run.then(|| Arc::new(DryRunBackend::new()));
        let power_backend: Arc<dyn PowerBackend> = match &power_dry_run {
            Some(d) => d.clone(),
            None => Arc::new(SystemPowerBackend),
        };
        Ok(AppState {
            identity,
            devices: DeviceStore::new(&config.data_dir),
            audit: AuditLog::new(&config.data_dir),
            started: Instant::now(),
            port,
            addresses,
            data_dir: config.data_dir.clone(),
            platform,
            layouts: LayoutStore::new(&config.data_dir),
            power: PowerScheduler::new(power_backend.clone()),
            power_backend,
            power_dry_run,
            downloads_dir: config
                .downloads_dir
                .clone()
                .unwrap_or_else(paths::helm_downloads_dir),
            download_root: config
                .download_root
                .clone()
                .unwrap_or_else(paths::profile_dir),
            relay: crate::relay::Relay::load(&config.data_dir),
            p3: crate::p3::P3State::new(&config.data_dir),
            p5: crate::p5::P5State::new(
                &config.data_dir,
                config
                    .videos_dir
                    .clone()
                    .unwrap_or_else(paths::helm_videos_dir),
                config.trigger_tick,
                config.wifi_poll,
            ),
            windows_poll: config.windows_poll,
            pairing: Mutex::new(PairingState::new(config.token_lifetime, config.lockout)),
            sessions: Mutex::new(HashMap::new()),
            next_session: AtomicU64::new(1),
            router: Router::new(modules::all()),
            handshake_timeout: config.handshake_timeout,
            stream_tokens: Mutex::new(TokenStore::new(config.stream_token_lifetime)),
            stream_token_lifetime: config.stream_token_lifetime,
            next_stream: AtomicU64::new(1),
        })
    }

    /// Issue a fresh one-time pairing token (the previous one stops working).
    pub fn new_pairing(&self) -> Pairing {
        let (token, expires_at) = self.lock_pairing().issue();
        let uri = pairing::build_uri(
            &self.identity.host_id,
            &self.identity.host_name,
            &self.addresses,
            self.port,
            &self.identity.fingerprint,
            &token,
        );
        Pairing {
            uri,
            token,
            expires_at,
        }
    }

    /// Close every open session of `device_id` except `except`.
    pub fn kick_device(&self, device_id: &str, except: Option<u64>) {
        for s in self.sessions() {
            if s.device_id == device_id && Some(s.id) != except {
                s.close(CloseCode::Policy, "device revoked");
            }
        }
    }

    /// Every ready session.
    pub fn sessions(&self) -> Vec<Arc<Session>> {
        self.sessions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .cloned()
            .collect()
    }

    /// Send an event to every ready session.
    pub fn broadcast(&self, name: &str, data: Value) {
        let text = proto::event(name, data).to_string();
        for s in self.sessions() {
            s.send_text(text.clone());
        }
    }

    pub fn router(&self) -> &Router {
        &self.router
    }

    pub(crate) fn next_stream_id(&self) -> u64 {
        self.next_stream.fetch_add(1, Ordering::Relaxed)
    }

    pub(crate) fn lock_stream_tokens(
        &self,
    ) -> std::sync::MutexGuard<'_, TokenStore<Weak<ActiveStream>>> {
        self.stream_tokens.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn lock_pairing(&self) -> std::sync::MutexGuard<'_, PairingState> {
        self.pairing.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// A running server. Dropping it does not stop it; call [`ServerHandle::shutdown`].
pub struct ServerHandle {
    state: Arc<AppState>,
    shutdown_tx: watch::Sender<bool>,
    task: JoinHandle<()>,
    mdns: Option<Advertiser>,
    background: Vec<JoinHandle<()>>,
    watchers: Option<WatchGuard>,
}

impl ServerHandle {
    pub fn port(&self) -> u16 {
        self.state.port
    }
    pub fn fingerprint(&self) -> &str {
        &self.state.identity.fingerprint
    }
    pub fn host_id(&self) -> &str {
        &self.state.identity.host_id
    }
    pub fn host_name(&self) -> &str {
        &self.state.identity.host_name
    }
    pub fn addresses(&self) -> &[Ipv4Addr] {
        &self.state.addresses
    }
    pub fn data_dir(&self) -> &std::path::Path {
        &self.state.data_dir
    }
    pub fn state(&self) -> &Arc<AppState> {
        &self.state
    }
    /// Issue a fresh pairing token and return the `helm://pair` link for it.
    pub fn new_pairing(&self) -> Pairing {
        self.state.new_pairing()
    }
    pub fn devices(&self) -> anyhow::Result<Vec<Device>> {
        self.state.devices.list()
    }

    /// Stop accepting, close open sessions, stop mDNS.
    pub async fn shutdown(self) {
        let _ = self.shutdown_tx.send(true);
        let _ = self.task.await;
        for t in self.background {
            t.abort();
        }
        self.state.power.cancel();
        if let Some(w) = self.watchers {
            let _ = tokio::task::spawn_blocking(move || drop(w)).await;
        }
        if let Some(m) = self.mdns {
            let _ = tokio::task::spawn_blocking(move || m.stop()).await;
        }
    }
}

fn tls_acceptor(identity: &Identity) -> anyhow::Result<TlsAcceptor> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(identity.cert_der.clone())],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(identity.key_pkcs8.clone())),
        )?;
    Ok(TlsAcceptor::from(Arc::new(config)))
}

async fn bind(port: u16) -> anyhow::Result<TcpListener> {
    match TcpListener::bind((Ipv4Addr::UNSPECIFIED, port)).await {
        Ok(l) => Ok(l),
        Err(e) if port != 0 => {
            tracing::warn!("port {port} is not available ({e}); using a free port instead");
            Ok(TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)).await?)
        }
        Err(e) => Err(e.into()),
    }
}

/// Start the agent in-process.
pub async fn start(config: Config) -> anyhow::Result<ServerHandle> {
    let listener = bind(config.port).await?;
    let port = listener.local_addr()?.port();
    let addresses = config
        .addresses
        .clone()
        .unwrap_or_else(net::lan_ipv4_addresses);
    if addresses.is_empty() {
        tracing::warn!("no LAN IPv4 address found; the pairing link will have no addresses");
    }
    let state = Arc::new(AppState::new(&config, port, addresses)?);
    let acceptor = tls_acceptor(&state.identity)?;

    let mdns = if config.enable_mdns {
        match Advertiser::start(
            &state.identity.host_id,
            &state.identity.host_name,
            &state.addresses,
            port,
        ) {
            Ok(a) => Some(a),
            Err(e) => {
                tracing::warn!("mDNS advertisement failed: {e:#}");
                None
            }
        }
    } else {
        None
    };

    // Scheduled power actions announce when they ran (or failed).
    let weak = Arc::downgrade(&state);
    state.power.set_listener(move |action, result| {
        if let Some(st) = weak.upgrade() {
            let detail = match result {
                Ok(()) => format!("action={action:?} ran"),
                Err(e) => format!("action={action:?} failed: {e}"),
            };
            st.audit.record("power_run", None, None, &detail);
            st.broadcast("power.scheduled", serde_json::json!({"scheduled": null}));
        }
    });

    let (events_tx, events_rx) = mpsc::unbounded_channel();
    let watchers = {
        let platform = state.platform.clone();
        tokio::task::spawn_blocking(move || platform.start_watchers(events_tx)).await?
    };
    let mut background = vec![
        tokio::spawn(pump_events(state.clone(), events_rx)),
        tokio::spawn(poll_windows(state.clone())),
    ];
    background.extend(crate::p3::spawn(&state));
    background.extend(crate::p5::spawn(&state));

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    background.push(tokio::spawn(crate::relay::run(
        state.clone(),
        acceptor.clone(),
        shutdown_rx.clone(),
    )));
    let task = tokio::spawn(accept_loop(listener, acceptor, state.clone(), shutdown_rx));
    tracing::info!(
        "listening on wss://0.0.0.0:{port}{WS_PATH} (fingerprint {})",
        state.identity.fingerprint
    );
    Ok(ServerHandle {
        state,
        shutdown_tx,
        task,
        mdns,
        background,
        watchers: Some(watchers),
    })
}

/// Clipboard text larger than this is left out of `clipboard.changed` (use `clipboard.get`).
const MAX_EVENT_TEXT: usize = 256 * 1024;

/// Turn watcher callbacks into events for the phones.
async fn pump_events(state: Arc<AppState>, mut rx: mpsc::UnboundedReceiver<PlatformEvent>) {
    use serde_json::json;
    while let Some(ev) = rx.recv().await {
        match ev {
            PlatformEvent::Media(m) => state.broadcast("media.changed", json!({"state": m})),
            PlatformEvent::Volume(v) => {
                state.broadcast("audio.changed", json!({"level": v.level, "muted": v.muted}))
            }
            PlatformEvent::TextFocus(f) => {
                state.broadcast("input.textFocus", json!({"focused": f}))
            }
            PlatformEvent::Clipboard {
                has_text,
                has_image,
            } => {
                let watching: Vec<_> = state
                    .sessions()
                    .into_iter()
                    .filter(|s| s.clipboard_watch())
                    .collect();
                if watching.is_empty() {
                    continue;
                }
                let mut data = json!({"has_image": has_image});
                if has_text {
                    let platform = state.platform.clone();
                    if let Ok(Some(text)) =
                        crate::platform::blocking(move || platform.clipboard_text()).await
                        && text.len() <= MAX_EVENT_TEXT
                    {
                        data["text"] = Value::String(text);
                    }
                }
                let text = proto::event("clipboard.changed", data).to_string();
                for s in watching {
                    s.send_text(text.clone());
                }
            }
        }
    }
}

/// Window identity used to detect changes: (hwnd, title, minimized, foreground).
type WindowKey = (u64, String, bool, bool);

/// Poll the window list while phones are connected; announce `windows.changed` on change.
async fn poll_windows(state: Arc<AppState>) {
    let mut interval = tokio::time::interval(state.windows_poll);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last: Option<Vec<WindowKey>> = None;
    loop {
        interval.tick().await;
        if state.sessions().is_empty() {
            last = None;
            continue;
        }
        let platform = state.platform.clone();
        let Ok(list) = crate::platform::blocking(move || platform.list_windows()).await else {
            continue;
        };
        let snapshot: Vec<WindowKey> = list
            .into_iter()
            .map(|w| (w.hwnd, w.title, w.is_minimized, w.is_foreground))
            .collect();
        if last.as_ref().is_some_and(|l| *l != snapshot) {
            state.broadcast("windows.changed", serde_json::json!({}));
        }
        last = Some(snapshot);
    }
}

async fn accept_loop(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    state: Arc<AppState>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut connections = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((tcp, peer)) => {
                    let _ = tcp.set_nodelay(true);
                    connections.spawn(handle_connection(
                        state.clone(), acceptor.clone(), tcp, peer.to_string(), shutdown.clone(),
                    ));
                }
                Err(e) => {
                    tracing::warn!("accept failed: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            },
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
            _ = shutdown.changed() => break,
        }
    }
    drop(listener);
    // Give open sessions a moment to send their close frames.
    let _ = tokio::time::timeout(Duration::from_secs(3), async {
        while connections.join_next().await.is_some() {}
    })
    .await;
    connections.abort_all();
}

/// Serve one connection: a LAN TCP socket, or a relay channel (`peer` = `via=relay ch=<n>`).
pub(crate) async fn handle_connection<T>(
    state: Arc<AppState>,
    acceptor: TlsAcceptor,
    tcp: T,
    peer: String,
    shutdown: watch::Receiver<bool>,
) where
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let limit = state.handshake_timeout;
    let tls = match tokio::time::timeout(limit, acceptor.accept(tcp)).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            tracing::debug!("TLS handshake with {peer} failed: {e}");
            return;
        }
        Err(_) => return,
    };
    let ws_config = WebSocketConfig::default()
        .max_message_size(Some(proto::MAX_MESSAGE))
        .max_frame_size(Some(proto::MAX_MESSAGE));
    let mut path = None;
    #[allow(clippy::result_large_err)] // the error type is fixed by tungstenite
    let callback = |req: &Request, resp: Response| {
        let r = check_path(req, resp);
        if r.is_ok() {
            path = Some(req.uri().path().to_string());
        }
        r
    };
    let ws = match tokio::time::timeout(
        limit,
        tokio_tungstenite::accept_hdr_async_with_config(tls, callback, Some(ws_config)),
    )
    .await
    {
        Ok(Ok(ws)) => ws,
        Ok(Err(e)) => {
            tracing::debug!("WebSocket upgrade from {peer} failed: {e}");
            return;
        }
        Err(_) => return,
    };
    if path.as_deref() == Some(STREAM_PATH) {
        crate::stream::socket::run(state, ws, peer, shutdown).await;
    } else {
        run_session(state, ws, peer, shutdown).await;
    }
}

/// WebSocket upgrade callback: only `/ws` and `/stream` are served.
#[allow(clippy::result_large_err)] // the error type is fixed by tungstenite
fn check_path(req: &Request, resp: Response) -> Result<Response, ErrorResponse> {
    let path = req.uri().path();
    if path == WS_PATH || path == STREAM_PATH {
        Ok(resp)
    } else {
        let mut err = ErrorResponse::new(Some("not found".to_string()));
        *err.status_mut() = http::StatusCode::NOT_FOUND;
        Err(err)
    }
}

/// Send an error, close the socket, and wait briefly for the peer's close.
pub(crate) async fn fail<S>(mut ws: WebSocketStream<S>, code: &str, message: &str)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let _ = ws
        .send(Message::text(proto::error(code, message).to_string()))
        .await;
    let _ = ws.close(None).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(Ok(_)) = ws.next().await {}
    })
    .await;
}

pub(crate) async fn send_json<S>(ws: &mut WebSocketStream<S>, value: &Value) -> bool
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    ws.send(Message::text(value.to_string())).await.is_ok()
}

/// Read until the first text message, skipping control frames. `None` = connection gone.
pub(crate) enum First {
    Text(String),
    Binary,
    TimedOut,
    Gone,
}

pub(crate) async fn read_first<S>(ws: &mut WebSocketStream<S>, deadline: tokio::time::Instant) -> First
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    loop {
        match tokio::time::timeout_at(deadline, ws.next()).await {
            Err(_) => return First::TimedOut,
            Ok(None) | Ok(Some(Err(_))) | Ok(Some(Ok(Message::Close(_)))) => return First::Gone,
            Ok(Some(Ok(Message::Text(t)))) => return First::Text(t.as_str().to_string()),
            Ok(Some(Ok(Message::Binary(_)))) => return First::Binary,
            Ok(Some(Ok(_))) => continue,
        }
    }
}

/// Outcome of the handshake: who the device is, or which error to send.
type HandshakeResult = Result<(String, Tier), (&'static str, String)>;

#[allow(clippy::too_many_arguments)]
fn do_pair(
    state: &AppState,
    peer: &str,
    nonce: &str,
    token: &str,
    device_id: &str,
    device_name: &str,
    public_key: &str,
    signature: &str,
) -> HandshakeResult {
    let check = state.lock_pairing().check_and_take(token);
    let expires_at = match check {
        TokenCheck::Locked => {
            state
                .audit
                .record("pair_fail", Some(device_id), Some(peer), "pairing_locked");
            return Err((
                codes::PAIRING_LOCKED,
                "too many wrong pairing codes; try again in a minute".into(),
            ));
        }
        TokenCheck::Bad => {
            state
                .audit
                .record("pair_fail", Some(device_id), Some(peer), "bad_token");
            return Err((
                codes::BAD_TOKEN,
                "pairing code is wrong, expired or already used".into(),
            ));
        }
        TokenCheck::Ok(expires_at) => expires_at,
    };
    let message = proto::signed_string("pair", &state.identity.host_id, nonce);
    let valid = proto::parse_public_key(public_key)
        .is_some_and(|key| proto::verify_signature(&key, &message, signature));
    if !valid {
        // The token was right; let the real phone retry with it.
        state.lock_pairing().restore(token, expires_at);
        state
            .audit
            .record("pair_fail", Some(device_id), Some(peer), "bad_signature");
        return Err((
            codes::BAD_SIGNATURE,
            "public key or signature is invalid".into(),
        ));
    }
    let now = util::now_ms();
    let device = Device {
        device_id: device_id.to_string(),
        device_name: device_name.to_string(),
        public_key: public_key.trim().to_string(),
        tier: Tier::Admin,
        paired_at_ms: now,
        last_seen_ms: now,
    };
    if let Err(e) = state.devices.upsert(device) {
        tracing::error!("could not save devices.json: {e:#}");
        state.lock_pairing().restore(token, expires_at);
        return Err((
            codes::BAD_MESSAGE,
            "agent could not store the device".into(),
        ));
    }
    state.audit.record(
        "pair_ok",
        Some(device_id),
        Some(peer),
        &format!("name={device_name}"),
    );
    tracing::info!("paired new device \"{device_name}\" ({device_id}) from {peer}");
    Ok((device_id.to_string(), Tier::Admin))
}

fn do_auth(
    state: &AppState,
    peer: &str,
    nonce: &str,
    device_id: &str,
    signature: &str,
) -> HandshakeResult {
    let device = match state.devices.get(device_id) {
        Ok(Some(d)) => d,
        Ok(None) => {
            state
                .audit
                .record("auth_fail", Some(device_id), Some(peer), "unknown_device");
            return Err((codes::UNKNOWN_DEVICE, "this device is not paired".into()));
        }
        Err(e) => {
            tracing::error!("could not read devices.json: {e:#}");
            return Err((codes::UNKNOWN_DEVICE, "device list unavailable".into()));
        }
    };
    let message = proto::signed_string("auth", &state.identity.host_id, nonce);
    let valid = proto::parse_public_key(&device.public_key)
        .is_some_and(|key| proto::verify_signature(&key, &message, signature));
    if !valid {
        state
            .audit
            .record("auth_fail", Some(device_id), Some(peer), "bad_signature");
        return Err((codes::BAD_SIGNATURE, "signature is invalid".into()));
    }
    if let Err(e) = state.devices.touch(device_id, util::now_ms()) {
        tracing::warn!("could not update last_seen: {e:#}");
    }
    state
        .audit
        .record("auth_ok", Some(device_id), Some(peer), "");
    tracing::info!(
        "device \"{}\" ({device_id}) connected from {peer}",
        device.device_name
    );
    Ok((device_id.to_string(), device.tier))
}

/// `hello` -> `pair` / `auth` -> `ready`. Returns the socket and who it is, or `None` after
/// the error was sent. `allow_pair = false` on `/stream` (§10).
pub(crate) async fn handshake<S>(
    state: &Arc<AppState>,
    mut ws: WebSocketStream<S>,
    peer: &str,
    allow_pair: bool,
) -> Option<(WebSocketStream<S>, String, Tier)>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let peer = peer.to_string();
    let nonce = util::b64_encode(&util::random_bytes(32));
    let hello = proto::hello(&state.identity.host_id, &state.identity.host_name, &nonce);
    if !send_json(&mut ws, &hello).await {
        return None;
    }
    let deadline = tokio::time::Instant::now() + state.handshake_timeout;
    let text = match read_first(&mut ws, deadline).await {
        First::Text(t) => t,
        First::Binary => {
            fail(ws, codes::BAD_MESSAGE, "expected a JSON text message").await;
            return None;
        }
        First::TimedOut => {
            state
                .audit
                .record("handshake_timeout", None, Some(&peer), "");
            fail(ws, codes::TIMEOUT, "no pair or auth within the time limit").await;
            return None;
        }
        First::Gone => return None,
    };
    let outcome = match proto::parse_handshake(&text) {
        Err(why) => Err((codes::BAD_MESSAGE, why)),
        // Pairing needs the QR code, a LAN/proximity action (PROTOCOL §12).
        Ok(Handshake::Pair { device_id, .. }) if crate::relay::is_relayed_peer(&peer) => {
            state
                .audit
                .record("pair_fail", Some(&device_id), Some(&peer), "relayed");
            Err((
                codes::BAD_MESSAGE,
                "pairing is only allowed on the local network".to_string(),
            ))
        }
        Ok(Handshake::Pair { .. }) if !allow_pair => Err((
            codes::BAD_MESSAGE,
            "pairing is only allowed on the control socket".to_string(),
        )),
        Ok(Handshake::Pair {
            token,
            device_id,
            device_name,
            public_key,
            signature,
        }) => do_pair(
            state,
            &peer,
            &nonce,
            &token,
            &device_id,
            &device_name,
            &public_key,
            &signature,
        ),
        Ok(Handshake::Auth {
            device_id,
            signature,
        }) => do_auth(state, &peer, &nonce, &device_id, &signature),
    };
    let (device_id, tier) = match outcome {
        Ok(ok) => ok,
        Err((code, message)) => {
            fail(ws, code, &message).await;
            return None;
        }
    };
    let ready = proto::ready(
        &device_id,
        tier.as_str(),
        &state.identity.host_id,
        &state.identity.host_name,
    );
    if !send_json(&mut ws, &ready).await {
        return None;
    }
    Some((ws, device_id, tier))
}

async fn run_session<S>(
    state: Arc<AppState>,
    ws: WebSocketStream<S>,
    peer: String,
    mut shutdown: watch::Receiver<bool>,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let Some((ws, device_id, tier)) = handshake(&state, ws, &peer, true).await else {
        return;
    };

    // ---- session ----
    let session_id = state.next_session.fetch_add(1, Ordering::Relaxed);
    let (tx, mut rx) = mpsc::unbounded_channel::<Outgoing>();
    let session = Arc::new(Session::new(
        session_id,
        device_id.clone(),
        tier,
        tx.clone(),
    ));
    state
        .sessions
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(session_id, session.clone());
    crate::p5::session_started(&state, &device_id);
    let ctx = RequestContext {
        device_id: device_id.clone(),
        tier,
        session_id,
        req_id: 0,
        peer: peer.clone(),
        state: state.clone(),
        session: session.clone(),
    };

    let (mut sink, mut stream) = ws.split();
    let mut writer = tokio::spawn(async move {
        while let Some(out) = rx.recv().await {
            match out {
                Outgoing::Text(t) => {
                    if sink.send(Message::text(t)).await.is_err() {
                        return;
                    }
                }
                Outgoing::Binary(b, permit) => {
                    let sent = sink.send(Message::binary(b)).await;
                    drop(permit);
                    if sent.is_err() {
                        return;
                    }
                }
                Outgoing::Close(code, reason) => {
                    let frame = CloseFrame {
                        code,
                        reason: reason.into(),
                    };
                    let _ = sink.send(Message::Close(Some(frame))).await;
                    let _ = sink.flush().await;
                    return;
                }
            }
        }
    });

    let mut writer_done = false;
    let mut shutting_down = false;
    loop {
        tokio::select! {
            msg = stream.next() => match msg {
                Some(Ok(Message::Text(t))) => handle_text(&ctx, &tx, t.as_str()),
                Some(Ok(Message::Binary(b))) => match files::decode_frame(&b) {
                    Ok((id, offset, data)) => session.write_upload(&id, offset, data).await,
                    Err(why) => {
                        let err = proto::error(codes::BAD_MESSAGE, &why);
                        let _ = tx.send(Outgoing::Text(err.to_string()));
                    }
                },
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            },
            _ = &mut writer, if !writer_done => {
                writer_done = true;
                // We sent a close frame (or the socket broke). Wait for the peer's close
                // reply, but not forever.
                let _ = tokio::time::timeout(Duration::from_secs(2), async {
                    while let Some(Ok(_)) = stream.next().await {}
                }).await;
                break;
            }
            _ = shutdown.changed(), if !shutting_down => {
                shutting_down = true;
                let _ = tx.send(Outgoing::Close(CloseCode::Away, "agent shutting down"));
            }
        }
    }

    state
        .sessions
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(&session_id);
    crate::p5::session_ended(&state, session_id, &device_id).await;
    state.platform.presenter().session_ended(session_id);
    session.finish().await;
    if !writer_done {
        let _ = tx.send(Outgoing::Close(CloseCode::Normal, ""));
        drop(tx);
        let _ = tokio::time::timeout(Duration::from_secs(2), writer).await;
    }
    tracing::debug!("session {session_id} ({device_id}) ended");
}

/// Parse one request and answer it on its own task (responses may come out of order).
fn handle_text(ctx: &RequestContext, tx: &mpsc::UnboundedSender<Outgoing>, text: &str) {
    // Input frames: no id, no reply, applied in order.
    if let Ok(v) = serde_json::from_str::<Value>(text)
        && v.get("type").and_then(Value::as_str) == Some("in")
    {
        ctx.session.handle_input(&ctx.state.platform, v);
        return;
    }
    let req = match proto::parse_request(text) {
        Ok(r) => r,
        Err(why) => {
            let _ = tx.send(Outgoing::Text(
                proto::error(codes::BAD_MESSAGE, &why).to_string(),
            ));
            return;
        }
    };
    let mut ctx = ctx.clone();
    ctx.req_id = req.id;
    let tx = tx.clone();
    tokio::spawn(async move {
        let id = req.id;
        let is_revoke = req.method == "devices.revoke";
        let call_ctx = ctx.clone();
        let result = tokio::spawn(async move {
            let state = call_ctx.state.clone();
            state
                .router()
                .dispatch(&req.method, req.params, &call_ctx)
                .await
        })
        .await;
        let response = match result {
            Ok(Ok(value)) => proto::res_ok(id, value),
            Ok(Err(e)) => proto::res_err(id, e.code, &e.message),
            Err(_) => proto::res_err(id, "internal", "the method failed unexpectedly"),
        };
        let _ = tx.send(Outgoing::Text(response.to_string()));
        if let Some(work) = ctx.session.take_after(id) {
            work.await;
        }
        if is_revoke && !ctx.state.devices.is_paired(&ctx.device_id) {
            // This device revoked itself: close after the response.
            let _ = tx.send(Outgoing::Close(CloseCode::Normal, "device revoked"));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn state(dir: &std::path::Path) -> Arc<AppState> {
        let mut config = Config::new(dir);
        config.host_name = Some("TEST-PC".into());
        config.platform = Some(crate::platform::fake::FakePlatform::new());
        config.power_dry_run = true;
        config.downloads_dir = Some(dir.join("downloads"));
        config.download_root = Some(dir.to_path_buf());
        Arc::new(AppState::new(&config, 1234, vec![Ipv4Addr::new(192, 168, 0, 2)]).unwrap())
    }

    fn ctx(state: &Arc<AppState>, tier: Tier) -> RequestContext {
        RequestContext {
            device_id: "dev".into(),
            tier,
            session_id: 1,
            req_id: 1,
            peer: "test".into(),
            state: state.clone(),
            session: Arc::new(Session::detached(1, "dev", tier)),
        }
    }

    #[tokio::test]
    async fn router_checks_tier_and_params() {
        let dir = tempfile::tempdir().unwrap();
        let st = state(dir.path());
        let viewer = ctx(&st, Tier::Viewer);
        let r = st
            .router()
            .dispatch("devices.list", json!({}), &viewer)
            .await;
        assert_eq!(r.unwrap_err().code, "forbidden");
        let r = st
            .router()
            .dispatch("system.ping", json!({}), &viewer)
            .await;
        assert_eq!(r.unwrap()["pong"], true);
        let r = st.router().dispatch("nope.nope", json!({}), &viewer).await;
        assert_eq!(r.unwrap_err().code, "unknown_method");
        let r = st
            .router()
            .dispatch("system.ping", json!([1]), &viewer)
            .await;
        assert_eq!(r.unwrap_err().code, "bad_params");
        let admin = ctx(&st, Tier::Admin);
        let r = st
            .router()
            .dispatch("devices.revoke", json!({}), &admin)
            .await;
        assert_eq!(r.unwrap_err().code, "bad_params");
    }

    #[test]
    fn pairing_uri_uses_state() {
        let dir = tempfile::tempdir().unwrap();
        let st = state(dir.path());
        let p = st.new_pairing();
        assert!(p.uri.starts_with(&format!(
            "helm://pair?v=1&id={}&n=TEST-PC&a=192.168.0.2&p=1234&fp={}&t=",
            st.identity.host_id, st.identity.fingerprint
        )));
        assert!(p.uri.ends_with(&p.token));
    }
}
