//! Phase 4 relay tunnel client (`relay/README.md`, "Tunnel protocol v1"; PROTOCOL.md §12).
//!
//! While `relay.enabled`, the agent keeps one outgoing WebSocket to `<url>/host/<host_id>`,
//! answers the relay's challenge with its identity key, and then serves multiplexed phone
//! channels. Each channel is an in-memory duplex pipe fed into the **same** TLS acceptor and
//! WebSocket handler as LAN connections, so the pinned TLS session is end-to-end and the
//! relay only sees ciphertext. Settings live in `relay.json` in the data folder.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use anyhow::Context;
use futures_util::{SinkExt, StreamExt};
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use p256::pkcs8::{DecodePrivateKey, EncodePublicKey};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;
use tokio_rustls::{TlsAcceptor, TlsConnector};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::http::Uri;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use crate::identity::Identity;
use crate::server::AppState;
use crate::util;

pub const RELAY_FILE: &str = "relay.json";
/// `peer` label of relayed sessions (also what `audit.log` shows): `via=relay ch=<n>`.
pub const RELAY_PEER_PREFIX: &str = "via=relay";

pub const FRAME_OPEN: u8 = 0x01;
pub const FRAME_DATA: u8 = 0x02;
pub const FRAME_CLOSE: u8 = 0x03;
/// Max tunnel payload per frame.
pub const MAX_PAYLOAD: usize = 65_536;
const HEADER: usize = 5;
/// Our own cap on concurrent channels (the relay allows 4 by default).
const MAX_CHANNELS: usize = 16;
/// Close a channel if the phone outruns the TLS stack by more than this.
const MAX_PENDING: usize = 1024 * 1024;
const PIPE_BUFFER: usize = 256 * 1024;
const PING_EVERY: Duration = Duration::from_secs(30);
const SILENT_LIMIT: Duration = Duration::from_secs(90);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// After `4403 key_mismatch` retrying soon will not help.
const KEY_MISMATCH_BACKOFF: Duration = Duration::from_secs(600);
const MAX_URL: usize = 512;
/// The relay address offered to users (the project owner's own deployment; editable).
pub const DEFAULT_RELAY_URL: &str = "wss://helm-relay.supportdangle-ae0.workers.dev";

/// True for the `peer` label of a relayed channel.
pub fn is_relayed_peer(peer: &str) -> bool {
    peer.starts_with(RELAY_PEER_PREFIX)
}

// ---------------------------------------------------------------- settings

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelaySettings {
    #[serde(default)]
    pub enabled: bool,
    /// Base URL, e.g. `wss://relay.example.com` (no trailing slash).
    #[serde(default)]
    pub url: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct RelayStatus {
    pub connected: bool,
    pub last_error: Option<String>,
}

/// Normalise whatever a person typed as the relay address and validate it.
///
/// Accepts `wss://host`, `https://host`, a bare `host`, and the easy mistake of stacking schemes
/// (`wss://https://host`); all are stored as `wss://host`. Plain `ws://` / `http://` only to
/// loopback (local testing). Empty input stays empty (= no relay configured).
pub fn normalize_url(url: &str) -> Result<String, String> {
    let url = url.trim().trim_end_matches('/');
    if url.is_empty() {
        return Ok(String::new());
    }
    if url.len() > MAX_URL {
        return Err(format!("url is longer than {MAX_URL} characters"));
    }
    if url.contains(['?', '#', '@']) || url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("url must not contain a query, fragment, user info or spaces".into());
    }
    // Peel every leading scheme; the outermost one says whether the user asked for plain text.
    let mut rest = url;
    let mut plain = None;
    while let Some((scheme, tail)) = rest.split_once("://") {
        match scheme.to_ascii_lowercase().as_str() {
            "wss" | "https" => plain = plain.or(Some(false)),
            "ws" | "http" => plain = plain.or(Some(true)),
            _ => return Err("use a wss:// or https:// address".into()),
        }
        rest = tail.trim_start_matches('/');
    }
    let plain = plain.unwrap_or(false);
    let normal = format!("{}://{rest}", if plain { "ws" } else { "wss" });
    let uri: Uri = normal
        .parse()
        .map_err(|_| "url is not a valid address".to_string())?;
    let host = uri.host().ok_or("url has no host")?.to_ascii_lowercase();
    let host_bare = host.trim_start_matches('[').trim_end_matches(']');
    if host_bare.is_empty() || matches!(host_bare, "wss" | "ws" | "https" | "http") {
        return Err("url has no host".into());
    }
    let loopback = host_bare == "localhost"
        || host_bare
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
    if plain && !loopback {
        return Err("plain ws:// or http:// is only allowed to localhost; use wss://".into());
    }
    Ok(normal)
}

/// Relay settings + live status, shared by the client task and the `relay.*` methods.
pub struct Relay {
    path: PathBuf,
    settings: watch::Sender<RelaySettings>,
    status: Mutex<RelayStatus>,
    write: Mutex<()>,
}

impl Relay {
    /// Read `relay.json` (missing or unreadable = disabled).
    pub fn load(data_dir: &Path) -> Relay {
        let path = data_dir.join(RELAY_FILE);
        let settings = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str::<RelaySettings>(&text)
                .ok()
                .and_then(|mut s| {
                    s.url = normalize_url(&s.url).ok()?;
                    Some(s)
                })
                .unwrap_or_else(|| {
                    tracing::warn!("ignoring invalid {}", path.display());
                    RelaySettings::default()
                }),
            Err(_) => RelaySettings::default(),
        };
        Relay {
            path,
            settings: watch::channel(settings).0,
            status: Mutex::new(RelayStatus::default()),
            write: Mutex::new(()),
        }
    }

    pub fn settings(&self) -> RelaySettings {
        self.settings.borrow().clone()
    }

    pub fn status(&self) -> RelayStatus {
        self.status
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<RelaySettings> {
        self.settings.subscribe()
    }

    /// Change, validate, persist and apply. `None` keeps the current value.
    pub fn set(&self, enabled: Option<bool>, url: Option<&str>) -> Result<RelaySettings, String> {
        let _guard = self.write.lock().unwrap_or_else(|p| p.into_inner());
        let mut next = self.settings();
        if let Some(u) = url {
            next.url = normalize_url(u)?;
        }
        if let Some(e) = enabled {
            next.enabled = e;
        }
        if next.enabled && next.url.is_empty() {
            return Err("a relay url is required to enable the relay".into());
        }
        let text = serde_json::to_string_pretty(&next).map_err(|e| e.to_string())?;
        util::atomic_write(&self.path, text.as_bytes())
            .map_err(|e| format!("could not save {RELAY_FILE}: {e}"))?;
        self.settings.send_replace(next.clone());
        Ok(next)
    }

    /// The relay address to put in a pairing QR (`r=`), when the relay is on.
    pub fn pairing_url(&self) -> Option<String> {
        let s = self.settings();
        (s.enabled && !s.url.is_empty()).then_some(s.url)
    }

    /// `system.info` `relay` field: `{enabled,url}`.
    pub fn info(&self) -> Value {
        let s = self.settings();
        json!({"enabled": s.enabled, "url": s.url})
    }

    /// `relay.get` result: settings plus live status.
    pub fn describe(&self) -> Value {
        let s = self.settings();
        let st = self.status();
        json!({
            "enabled": s.enabled,
            "url": s.url,
            "connected": st.connected,
            "last_error": st.last_error,
        })
    }

    fn set_status(&self, connected: bool, last_error: Option<String>) {
        *self.status.lock().unwrap_or_else(|p| p.into_inner()) = RelayStatus {
            connected,
            last_error,
        };
    }
}

// ---------------------------------------------------------------- frames

/// One mux frame: `[type u8][channel u32 BE][payload]`.
pub fn encode_frame(kind: u8, channel: u32, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(HEADER + payload.len());
    v.push(kind);
    v.extend_from_slice(&channel.to_be_bytes());
    v.extend_from_slice(payload);
    v
}

/// Split a mux frame into `(type, channel, payload)`.
pub fn decode_frame(frame: &[u8]) -> Result<(u8, u32, &[u8]), String> {
    if frame.len() < HEADER {
        return Err(format!("mux frame of {} bytes is too short", frame.len()));
    }
    if frame.len() - HEADER > MAX_PAYLOAD {
        return Err("mux payload larger than 65536 bytes".into());
    }
    let ch = u32::from_be_bytes([frame[1], frame[2], frame[3], frame[4]]);
    Ok((frame[0], ch, &frame[HEADER..]))
}

/// The string signed in the host auth: `helm-relay-v1|host|<host_id>|<nonce>`.
pub fn auth_string(host_id: &str, nonce: &str) -> String {
    format!(
        "helm-relay-v1|host|{}|{nonce}",
        host_id.to_ascii_lowercase()
    )
}

/// The `auth` reply to a relay challenge, signed with the identity (certificate) key.
pub fn auth_message(identity: &Identity, nonce: &str) -> anyhow::Result<Value> {
    let key = SigningKey::from_pkcs8_der(&identity.key_pkcs8)
        .map_err(|e| anyhow::anyhow!("identity key is not P-256 PKCS#8: {e}"))?;
    let spki = key
        .verifying_key()
        .to_public_key_der()
        .map_err(|e| anyhow::anyhow!("encoding public key: {e}"))?;
    let sig: Signature = key.sign(auth_string(&identity.host_id, nonce).as_bytes());
    Ok(json!({
        "type": "auth",
        "public_key": util::b64_encode(spki.as_bytes()),
        "signature": util::b64_encode(sig.to_der().as_bytes()),
    }))
}

// ---------------------------------------------------------------- backoff

/// Exponential backoff 1 s → 60 s with ±25 % jitter.
#[derive(Debug)]
pub struct Backoff {
    next: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Backoff { next: BACKOFF_MIN }
    }
}

impl Backoff {
    pub fn reset(&mut self) {
        self.next = BACKOFF_MIN;
    }
    /// The base delay that the next call would use (before jitter).
    pub fn peek(&self) -> Duration {
        self.next
    }
    pub fn next_delay(&mut self) -> Duration {
        use rand::Rng;
        let base = self.next;
        self.next = (self.next * 2).min(BACKOFF_MAX);
        let factor: f64 = rand::thread_rng().gen_range(0.75..1.25);
        base.mul_f64(factor).min(BACKOFF_MAX)
    }
}

// ---------------------------------------------------------------- client

trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}
type RelayWs = WebSocketStream<Box<dyn Io>>;

fn tls_client_config() -> Arc<rustls::ClientConfig> {
    static CONFIG: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let mut roots = rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            let config = rustls::ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .expect("ring supports the default TLS versions")
                .with_root_certificates(roots)
                .with_no_client_auth();
            Arc::new(config)
        })
        .clone()
}

async fn connect_ws(base_url: &str, host_id: &str) -> anyhow::Result<RelayWs> {
    let full = format!("{base_url}/host/{}", host_id.to_ascii_lowercase());
    let uri: Uri = full.parse().context("bad relay url")?;
    let tls = uri.scheme_str() == Some("wss");
    let host = uri
        .host()
        .context("relay url has no host")?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    let port = uri.port_u16().unwrap_or(if tls { 443 } else { 80 });
    let tcp = TcpStream::connect((host.as_str(), port))
        .await
        .with_context(|| format!("connecting to {host}:{port}"))?;
    let _ = tcp.set_nodelay(true);
    let io: Box<dyn Io> = if tls {
        let name =
            rustls::pki_types::ServerName::try_from(host.clone()).context("bad relay host name")?;
        Box::new(
            TlsConnector::from(tls_client_config())
                .connect(name, tcp)
                .await
                .context("TLS handshake with the relay")?,
        )
    } else {
        Box::new(tcp)
    };
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_PAYLOAD + HEADER + 1024))
        .max_frame_size(Some(MAX_PAYLOAD + HEADER + 1024));
    let (ws, _) = tokio_tungstenite::client_async_with_config(full, io, Some(config))
        .await
        .map_err(|e| anyhow::anyhow!(upgrade_error(&e)))?;
    Ok(ws)
}

/// Why the relay would not upgrade to a WebSocket, in words for the Settings page.
fn upgrade_error(e: &tokio_tungstenite::tungstenite::Error) -> String {
    use tokio_tungstenite::tungstenite::Error;
    match e {
        Error::Http(resp) => match resp.status().as_u16() {
            429 => "the relay is rate limiting this connection; retrying".to_string(),
            code => format!("this address does not answer like a Helm relay (HTTP {code})"),
        },
        other => format!("WebSocket upgrade with the relay failed: {other}"),
    }
}

/// Words for a relay `error` code (relay/README.md section 1.6).
fn refusal_text(code: &str) -> String {
    match code {
        "key_mismatch" => "the relay knows a different key for this PC".to_string(),
        "bad_signature" => "the relay did not accept this PC's signature".to_string(),
        other => format!("the relay refused this PC ({other})"),
    }
}

/// Why a relay connection ended.
#[derive(Debug)]
enum Ended {
    Shutdown,
    /// `ready` was reached before it ended (resets the backoff).
    Lost {
        was_ready: bool,
        error: String,
        key_mismatch: bool,
    },
}

fn lost(was_ready: bool, error: impl Into<String>) -> Ended {
    Ended::Lost {
        was_ready,
        error: error.into(),
        key_mismatch: false,
    }
}

async fn next_text(ws: &mut RelayWs) -> Result<String, Ended> {
    loop {
        match tokio::time::timeout(HANDSHAKE_TIMEOUT, ws.next()).await {
            Err(_) => return Err(lost(false, "relay handshake timed out")),
            Ok(Some(Ok(Message::Text(t)))) => return Ok(t.as_str().to_string()),
            Ok(Some(Ok(Message::Close(f)))) => {
                let code = f.as_ref().map(|f| u16::from(f.code)).unwrap_or(0);
                return Err(Ended::Lost {
                    was_ready: false,
                    error: format!("relay closed the connection ({code})"),
                    key_mismatch: code == 4403,
                });
            }
            Ok(Some(Ok(Message::Binary(_)))) => {
                return Err(lost(false, "relay sent binary data during the handshake"));
            }
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(e))) => return Err(lost(false, format!("relay connection error: {e}"))),
            Ok(None) => return Err(lost(false, "relay closed the connection")),
        }
    }
}

async fn handshake(ws: &mut RelayWs, identity: &Identity) -> Result<(), Ended> {
    let text = next_text(ws).await?;
    let challenge: Value =
        serde_json::from_str(&text).map_err(|_| lost(false, "relay challenge is not JSON"))?;
    let nonce = match (challenge["type"].as_str(), challenge["nonce"].as_str()) {
        (Some("challenge"), Some(n)) if !n.is_empty() && n.len() <= 256 => n.to_string(),
        _ => return Err(lost(false, "unexpected first message from the relay")),
    };
    let auth = auth_message(identity, &nonce).map_err(|e| lost(false, format!("{e:#}")))?;
    ws.send(Message::text(auth.to_string()))
        .await
        .map_err(|e| lost(false, format!("sending auth: {e}")))?;
    let text = next_text(ws).await?;
    let reply: Value =
        serde_json::from_str(&text).map_err(|_| lost(false, "relay reply is not JSON"))?;
    match reply["type"].as_str() {
        Some("ready") => Ok(()),
        Some("error") => {
            let code = reply["code"].as_str().unwrap_or("unknown").to_string();
            Err(Ended::Lost {
                was_ready: false,
                key_mismatch: code == "key_mismatch",
                error: refusal_text(&code),
            })
        }
        _ => Err(lost(false, "unexpected reply from the relay")),
    }
}

/// One live phone channel.
struct Channel {
    to_tls: mpsc::UnboundedSender<Vec<u8>>,
    pending: Arc<AtomicUsize>,
}

/// Start the per-channel tasks: the agent's normal TLS/WebSocket handler on one end of a
/// duplex pipe, and two pumps between the other end and the relay socket.
fn open_channel(
    ch: u32,
    state: &Arc<AppState>,
    acceptor: &TlsAcceptor,
    shutdown: &watch::Receiver<bool>,
    out_tx: &mpsc::Sender<Message>,
    ended_tx: &mpsc::UnboundedSender<u32>,
    tasks: &mut JoinSet<()>,
) -> Channel {
    let (agent_io, relay_io) = tokio::io::duplex(PIPE_BUFFER);
    let (mut rd, mut wr) = tokio::io::split(relay_io);
    let peer = format!("{RELAY_PEER_PREFIX} ch={ch}");
    tasks.spawn(crate::server::handle_connection(
        state.clone(),
        acceptor.clone(),
        agent_io,
        peer,
        shutdown.clone(),
    ));
    // agent → phone
    let out = out_tx.clone();
    let ended = ended_tx.clone();
    tasks.spawn(async move {
        let mut buf = vec![0u8; MAX_PAYLOAD];
        loop {
            match rd.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let frame = encode_frame(FRAME_DATA, ch, &buf[..n]);
                    if out.send(Message::binary(frame)).await.is_err() {
                        break;
                    }
                }
            }
        }
        let _ = ended.send(ch);
    });
    // phone → agent
    let (to_tls, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let pending = Arc::new(AtomicUsize::new(0));
    let pend = pending.clone();
    tasks.spawn(async move {
        while let Some(chunk) = rx.recv().await {
            let n = chunk.len();
            if wr.write_all(&chunk).await.is_err() {
                break;
            }
            pend.fetch_sub(n, Ordering::SeqCst);
        }
        let _ = wr.shutdown().await;
    });
    Channel { to_tls, pending }
}

async fn serve(
    ws: RelayWs,
    state: &Arc<AppState>,
    acceptor: &TlsAcceptor,
    shutdown: &mut watch::Receiver<bool>,
) -> Ended {
    let (mut sink, mut stream) = ws.split();
    let (out_tx, mut out_rx) = mpsc::channel::<Message>(64);
    let writer = tokio::spawn(async move {
        while let Some(m) = out_rx.recv().await {
            if sink.send(m).await.is_err() {
                return;
            }
        }
        let _ = sink.close().await;
    });
    let (ended_tx, mut ended_rx) = mpsc::unbounded_channel::<u32>();
    let mut channels: HashMap<u32, Channel> = HashMap::new();
    let mut tasks = JoinSet::new();
    let mut ping = tokio::time::interval(PING_EVERY);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ping.tick().await;
    let mut last_rx = tokio::time::Instant::now();

    let result = loop {
        tokio::select! {
            msg = stream.next() => match msg {
                Some(Ok(Message::Binary(b))) => {
                    last_rx = tokio::time::Instant::now();
                    let (kind, ch, payload) = match decode_frame(&b) {
                        Ok(f) => f,
                        Err(why) => {
                            tracing::debug!("relay: {why}");
                            continue;
                        }
                    };
                    match kind {
                        FRAME_OPEN => {
                            if channels.contains_key(&ch) || channels.len() >= MAX_CHANNELS {
                                let _ = out_tx.send(Message::binary(encode_frame(FRAME_CLOSE, ch, &[]))).await;
                                continue;
                            }
                            tracing::debug!("relay: channel {ch} opened");
                            let c = open_channel(ch, state, acceptor, shutdown, &out_tx, &ended_tx, &mut tasks);
                            channels.insert(ch, c);
                        }
                        FRAME_DATA if !payload.is_empty() => {
                            let Some(c) = channels.get(&ch) else { continue };
                            let n = payload.len();
                            let over = c.pending.load(Ordering::SeqCst) + n > MAX_PENDING;
                            if over || c.to_tls.send(payload.to_vec()).is_err() {
                                tracing::debug!("relay: channel {ch} overrun, closing");
                                channels.remove(&ch);
                                let _ = out_tx.send(Message::binary(encode_frame(FRAME_CLOSE, ch, &[]))).await;
                            } else {
                                c.pending.fetch_add(n, Ordering::SeqCst);
                            }
                        }
                        FRAME_CLOSE => {
                            // Dropping the sender shuts the pipe: the session sees EOF.
                            channels.remove(&ch);
                        }
                        _ => {} // unknown types are ignored (forward compatibility)
                    }
                }
                Some(Ok(Message::Close(f))) => {
                    let code = f.as_ref().map(|f| u16::from(f.code)).unwrap_or(0);
                    break lost(true, format!("relay closed the connection ({code})"));
                }
                Some(Ok(_)) => last_rx = tokio::time::Instant::now(),
                Some(Err(e)) => break lost(true, format!("relay connection error: {e}")),
                None => break lost(true, "relay connection lost"),
            },
            Some(ch) = ended_rx.recv() => {
                if channels.remove(&ch).is_some() {
                    let _ = out_tx.send(Message::binary(encode_frame(FRAME_CLOSE, ch, &[]))).await;
                }
            }
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
            _ = ping.tick() => {
                if last_rx.elapsed() > SILENT_LIMIT {
                    break lost(true, "relay stopped answering");
                }
                let _ = out_tx.send(Message::text("ping")).await;
            }
            _ = shutdown.changed() => break Ended::Shutdown,
        }
    };

    // Let sessions end cleanly (input released, uploads dropped): close their pipes, give
    // them a moment to send their close frames, then drop the rest.
    drop(channels);
    let _ = tokio::time::timeout(Duration::from_secs(3), async {
        while tasks.join_next().await.is_some() {}
    })
    .await;
    tasks.abort_all();
    drop(out_tx);
    let _ = tokio::time::timeout(Duration::from_secs(2), writer).await;
    result
}

async fn connect_and_serve(
    state: &Arc<AppState>,
    acceptor: &TlsAcceptor,
    url: &str,
    shutdown: &mut watch::Receiver<bool>,
) -> Ended {
    let mut ws =
        match tokio::time::timeout(CONNECT_TIMEOUT, connect_ws(url, &state.identity.host_id)).await
        {
            Ok(Ok(ws)) => ws,
            Ok(Err(e)) => return lost(false, format!("{e:#}")),
            Err(_) => return lost(false, "connecting to the relay timed out"),
        };
    if let Err(e) = handshake(&mut ws, &state.identity).await {
        let _ = ws.close(None).await;
        return e;
    }
    state.relay.set_status(true, None);
    state.audit.record("relay_up", None, None, url);
    tracing::info!("relay connected: {url}");
    serve(ws, state, acceptor, shutdown).await
}

/// The relay client task (runs for the agent's lifetime; idle while disabled).
pub(crate) async fn run(
    state: Arc<AppState>,
    acceptor: TlsAcceptor,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut settings_rx = state.relay.subscribe();
    let mut backoff = Backoff::default();
    loop {
        if *shutdown.borrow() {
            return;
        }
        let settings = settings_rx.borrow_and_update().clone();
        if !settings.enabled || settings.url.is_empty() {
            state.relay.set_status(false, None);
            tokio::select! {
                r = settings_rx.changed() => if r.is_err() { return },
                _ = shutdown.changed() => return,
            }
            backoff.reset();
            continue;
        }
        let ended = tokio::select! {
            e = connect_and_serve(&state, &acceptor, &settings.url, &mut shutdown) => e,
            r = settings_rx.changed() => {
                if r.is_err() { return }
                // The old connection is gone with the old settings: start from "connecting".
                state.relay.set_status(false, None);
                backoff.reset();
                continue;
            }
        };
        let (delay, error) = match ended {
            Ended::Shutdown => return,
            Ended::Lost {
                was_ready,
                error,
                key_mismatch,
            } => {
                if was_ready {
                    backoff.reset();
                    state.audit.record("relay_down", None, None, &error);
                }
                let delay = if key_mismatch {
                    KEY_MISMATCH_BACKOFF
                } else {
                    backoff.next_delay()
                };
                (delay, error)
            }
        };
        tracing::warn!(
            "relay: {error}; retrying in {} s",
            delay.as_secs_f32().ceil()
        );
        state.relay.set_status(false, Some(error));
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            r = settings_rx.changed() => {
                if r.is_err() { return }
                state.relay.set_status(false, None);
                backoff.reset();
            }
            _ = shutdown.changed() => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let f = encode_frame(FRAME_DATA, 0x0102_0304, b"hi");
        assert_eq!(f, [2, 1, 2, 3, 4, b'h', b'i']);
        assert_eq!(
            decode_frame(&f).unwrap(),
            (FRAME_DATA, 0x0102_0304, &b"hi"[..])
        );
        assert_eq!(
            decode_frame(&encode_frame(FRAME_OPEN, 7, &[])).unwrap().1,
            7
        );
        assert!(decode_frame(&[1, 0, 0]).is_err());
        assert!(decode_frame(&vec![2; HEADER + MAX_PAYLOAD + 1]).is_err());
        assert!(decode_frame(&vec![2; HEADER + MAX_PAYLOAD]).is_ok());
    }

    #[test]
    fn urls_are_checked() {
        assert_eq!(
            normalize_url(" wss://r.example.com/ ").unwrap(),
            "wss://r.example.com"
        );
        assert_eq!(
            normalize_url("WSS://R.example.com:8443/x").unwrap(),
            "wss://R.example.com:8443/x"
        );
        assert_eq!(
            normalize_url("ws://127.0.0.1:9000").unwrap(),
            "ws://127.0.0.1:9000"
        );
        assert_eq!(normalize_url("ws://localhost").unwrap(), "ws://localhost");
        assert_eq!(normalize_url("").unwrap(), "");
        for bad in [
            "ws://relay.example.com",
            "http://relay.example.com",
            "ws://https://relay.example.com",
            "ftp://relay.example.com",
            "wss://",
            "wss://https://",
            "https:",
            "wss://https:",
            "wss://a.com/?x=1",
            "wss://u@a.com",
            "wss://a b",
        ] {
            assert!(normalize_url(bad).is_err(), "{bad}");
        }
        assert!(normalize_url(&format!("wss://{}.com", "a".repeat(600))).is_err());
    }

    #[test]
    fn people_can_type_the_relay_address_any_way() {
        let want = "wss://helm-relay.example.workers.dev";
        for typed in [
            "wss://helm-relay.example.workers.dev",
            "https://helm-relay.example.workers.dev",
            "helm-relay.example.workers.dev",
            "wss://https://helm-relay.example.workers.dev",
            "https://wss://helm-relay.example.workers.dev/",
            "HTTPS://helm-relay.example.workers.dev//",
            "  wss://wss://helm-relay.example.workers.dev  ",
        ] {
            assert_eq!(normalize_url(typed).unwrap(), want, "{typed}");
        }
        assert_eq!(
            normalize_url("http://localhost:8787").unwrap(),
            "ws://localhost:8787"
        );
        assert_eq!(
            normalize_url("localhost:8787").unwrap(),
            "wss://localhost:8787"
        );
        assert_eq!(normalize_url(DEFAULT_RELAY_URL).unwrap(), DEFAULT_RELAY_URL);
    }

    #[test]
    fn a_mistaken_saved_address_heals_on_load_and_reaches_the_qr() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(RELAY_FILE),
            r#"{"enabled":true,"url":"wss://https://r.example.com/"}"#,
        )
        .unwrap();
        let r = Relay::load(dir.path());
        assert_eq!(r.settings().url, "wss://r.example.com");
        assert_eq!(r.pairing_url().as_deref(), Some("wss://r.example.com"));
        r.set(Some(false), None).unwrap();
        assert_eq!(r.pairing_url(), None);
    }

    #[test]
    fn settings_persist_and_validate() {
        let dir = tempfile::tempdir().unwrap();
        let r = Relay::load(dir.path());
        assert_eq!(r.settings(), RelaySettings::default());
        assert!(r.set(Some(true), None).is_err(), "no url yet");
        assert!(r.set(None, Some("http://x")).is_err());
        let s = r.set(Some(true), Some("wss://relay.example.com/")).unwrap();
        assert_eq!(s.url, "wss://relay.example.com");
        let again = Relay::load(dir.path());
        assert_eq!(again.settings(), s);
        assert_eq!(
            again.info(),
            json!({"enabled": true, "url": "wss://relay.example.com"})
        );
        assert_eq!(again.describe()["connected"], false);
        std::fs::write(dir.path().join(RELAY_FILE), "not json").unwrap();
        assert_eq!(Relay::load(dir.path()).settings(), RelaySettings::default());
    }

    #[test]
    fn backoff_grows_to_cap() {
        let mut b = Backoff::default();
        let mut bases = Vec::new();
        for _ in 0..9 {
            bases.push(b.peek().as_secs());
            let d = b.next_delay();
            assert!(d <= BACKOFF_MAX);
        }
        assert_eq!(bases, [1, 2, 4, 8, 16, 32, 60, 60, 60]);
        b.reset();
        assert_eq!(b.peek(), BACKOFF_MIN);
        let d = Backoff::default().next_delay();
        assert!(d >= Duration::from_millis(750) && d <= Duration::from_millis(1250));
    }

    #[test]
    fn auth_signature_verifies_with_identity_key() {
        let dir = tempfile::tempdir().unwrap();
        let id = Identity::load_or_create(dir.path(), "PC".into()).unwrap();
        let msg = auth_message(&id, "bm9uY2U=").unwrap();
        assert_eq!(msg["type"], "auth");
        let key = crate::proto::parse_public_key(msg["public_key"].as_str().unwrap()).unwrap();
        let signed = auth_string(&id.host_id, "bm9uY2U=");
        assert!(signed.starts_with("helm-relay-v1|host|"));
        assert!(crate::proto::verify_signature(
            &key,
            &signed,
            msg["signature"].as_str().unwrap()
        ));
        assert!(is_relayed_peer("via=relay ch=3"));
        assert!(!is_relayed_peer("127.0.0.1:5000"));
    }
}
