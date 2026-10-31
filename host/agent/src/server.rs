//! TLS WebSocket server: handshake (pair / auth), then the request loop.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context;
use futures_util::{SinkExt, StreamExt};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, WebSocketConfig};

use crate::audit::AuditLog;
use crate::devices::{Device, DeviceStore};
use crate::identity::{self, Identity};
use crate::mdns::Advertiser;
use crate::pairing::{self, Pairing, PairingState, TokenCheck};
use crate::proto::{self, Handshake, codes};
use crate::router::{RequestContext, Router, Tier};
use crate::{modules, net, util};

pub const DEFAULT_PORT: u16 = 47820;
pub const WS_PATH: &str = "/ws";

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
        }
    }
}

/// Default data folder: `%APPDATA%\Helm` (or `./helm-data` if APPDATA is not set).
pub fn default_data_dir() -> PathBuf {
    match std::env::var_os("APPDATA") {
        Some(appdata) => PathBuf::from(appdata).join("Helm"),
        None => PathBuf::from("helm-data"),
    }
}

enum Outgoing {
    Text(String),
    Close(CloseCode, &'static str),
}

struct SessionEntry {
    device_id: String,
    tx: mpsc::UnboundedSender<Outgoing>,
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
    pairing: Mutex<PairingState>,
    sessions: Mutex<HashMap<u64, SessionEntry>>,
    next_session: AtomicU64,
    router: Router,
    handshake_timeout: Duration,
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
        Ok(AppState {
            identity,
            devices: DeviceStore::new(&config.data_dir),
            audit: AuditLog::new(&config.data_dir),
            started: Instant::now(),
            port,
            addresses,
            data_dir: config.data_dir.clone(),
            pairing: Mutex::new(PairingState::new(config.token_lifetime, config.lockout)),
            sessions: Mutex::new(HashMap::new()),
            next_session: AtomicU64::new(1),
            router: Router::new(modules::all()),
            handshake_timeout: config.handshake_timeout,
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
        let sessions = self.sessions.lock().unwrap_or_else(|p| p.into_inner());
        for (id, s) in sessions.iter() {
            if s.device_id == device_id && Some(*id) != except {
                let _ =
                    s.tx.send(Outgoing::Close(CloseCode::Policy, "device revoked"));
            }
        }
    }

    pub fn router(&self) -> &Router {
        &self.router
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

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
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
    })
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
                        state.clone(), acceptor.clone(), tcp, peer, shutdown.clone(),
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

async fn handle_connection(
    state: Arc<AppState>,
    acceptor: TlsAcceptor,
    tcp: TcpStream,
    peer: SocketAddr,
    shutdown: watch::Receiver<bool>,
) {
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
    let ws = match tokio::time::timeout(
        limit,
        tokio_tungstenite::accept_hdr_async_with_config(tls, check_path, Some(ws_config)),
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
    run_session(state, ws, peer.to_string(), shutdown).await;
}

/// WebSocket upgrade callback: only `/ws` is served.
#[allow(clippy::result_large_err)] // the error type is fixed by tungstenite
fn check_path(req: &Request, resp: Response) -> Result<Response, ErrorResponse> {
    if req.uri().path() == WS_PATH {
        Ok(resp)
    } else {
        let mut err = ErrorResponse::new(Some("not found".to_string()));
        *err.status_mut() = http::StatusCode::NOT_FOUND;
        Err(err)
    }
}

/// Send an error, close the socket, and wait briefly for the peer's close.
async fn fail<S>(mut ws: WebSocketStream<S>, code: &str, message: &str)
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

async fn send_json<S>(ws: &mut WebSocketStream<S>, value: &Value) -> bool
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    ws.send(Message::text(value.to_string())).await.is_ok()
}

/// Read until the first text message, skipping control frames. `None` = connection gone.
enum First {
    Text(String),
    Binary,
    TimedOut,
    Gone,
}

async fn read_first<S>(ws: &mut WebSocketStream<S>, deadline: tokio::time::Instant) -> First
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

async fn run_session<S>(
    state: Arc<AppState>,
    mut ws: WebSocketStream<S>,
    peer: String,
    mut shutdown: watch::Receiver<bool>,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    // ---- handshake ----
    let nonce = util::b64_encode(&util::random_bytes(32));
    let hello = proto::hello(&state.identity.host_id, &state.identity.host_name, &nonce);
    if !send_json(&mut ws, &hello).await {
        return;
    }
    let deadline = tokio::time::Instant::now() + state.handshake_timeout;
    let text = match read_first(&mut ws, deadline).await {
        First::Text(t) => t,
        First::Binary => return fail(ws, codes::BAD_MESSAGE, "expected a JSON text message").await,
        First::TimedOut => {
            state
                .audit
                .record("handshake_timeout", None, Some(&peer), "");
            return fail(ws, codes::TIMEOUT, "no pair or auth within the time limit").await;
        }
        First::Gone => return,
    };
    let outcome = match proto::parse_handshake(&text) {
        Err(why) => Err((codes::BAD_MESSAGE, why)),
        Ok(Handshake::Pair {
            token,
            device_id,
            device_name,
            public_key,
            signature,
        }) => do_pair(
            &state,
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
        }) => do_auth(&state, &peer, &nonce, &device_id, &signature),
    };
    let (device_id, tier) = match outcome {
        Ok(ok) => ok,
        Err((code, message)) => return fail(ws, code, &message).await,
    };
    let ready = proto::ready(
        &device_id,
        tier.as_str(),
        &state.identity.host_id,
        &state.identity.host_name,
    );
    if !send_json(&mut ws, &ready).await {
        return;
    }

    // ---- session ----
    let session_id = state.next_session.fetch_add(1, Ordering::Relaxed);
    let (tx, mut rx) = mpsc::unbounded_channel::<Outgoing>();
    state
        .sessions
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(
            session_id,
            SessionEntry {
                device_id: device_id.clone(),
                tx: tx.clone(),
            },
        );
    let ctx = RequestContext {
        device_id: device_id.clone(),
        tier,
        session_id,
        peer: peer.clone(),
        state: state.clone(),
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
                Some(Ok(Message::Binary(_))) => {
                    let err = proto::error(codes::BAD_MESSAGE, "expected a JSON text message");
                    let _ = tx.send(Outgoing::Text(err.to_string()));
                }
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
    if !writer_done {
        let _ = tx.send(Outgoing::Close(CloseCode::Normal, ""));
        drop(tx);
        let _ = tokio::time::timeout(Duration::from_secs(2), writer).await;
    }
    tracing::debug!("session {session_id} ({device_id}) ended");
}

/// Parse one request and answer it on its own task (responses may come out of order).
fn handle_text(ctx: &RequestContext, tx: &mpsc::UnboundedSender<Outgoing>, text: &str) {
    let req = match proto::parse_request(text) {
        Ok(r) => r,
        Err(why) => {
            let _ = tx.send(Outgoing::Text(
                proto::error(codes::BAD_MESSAGE, &why).to_string(),
            ));
            return;
        }
    };
    let ctx = ctx.clone();
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
        Arc::new(AppState::new(&config, 1234, vec![Ipv4Addr::new(192, 168, 0, 2)]).unwrap())
    }

    fn ctx(state: &Arc<AppState>, tier: Tier) -> RequestContext {
        RequestContext {
            device_id: "dev".into(),
            tier,
            session_id: 1,
            peer: "test".into(),
            state: state.clone(),
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
