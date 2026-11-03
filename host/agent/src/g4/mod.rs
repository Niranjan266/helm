//! G4 — pairing security (hello feature `g4`, `Docs/protocol/pairing-security.md`):
//! short-code pairing, "ask before a new phone pairs", and host events for PC-side UIs.
//!
//! PC-side UIs (console, tray, the desktop app) use [`G4State`] through `AppState::g4`:
//! [`G4State::subscribe`] for [`HostEvent`]s, [`G4State::new_code`] to show a code,
//! [`G4State::resolve`] to Allow / Deny, [`G4State::pending`] and the settings calls.

pub mod approval;
pub mod code;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures_util::StreamExt;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::broadcast;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

use crate::platform::{Toast, ToastAction, ToastEvent};
use crate::proto::{self, PairCode, codes};
use crate::router::{MethodSpec, Module, RequestContext, RpcError, Tier};
use crate::server::{AppState, First, read_first, send_json};
use crate::util;

pub use approval::{Approvals, Outcome, PairingSettings, PendingPairing};
pub use code::{CodeState, Role, Take};

/// Event for PC-side UIs (not sent to phones). Serialized with `"type"`, snake_case.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostEvent {
    /// Show this code and PC name until it expires or [`HostEvent::PairingCodeEnded`].
    PairingCode {
        code: String,
        host_name: String,
        expires_unix_ms: i64,
    },
    /// The shown code is gone: `used` (an attempt started), `locked`, or `replaced`.
    PairingCodeEnded { reason: String },
    /// A phone waits for Allow / Deny ([`G4State::resolve`]).
    PairingRequest(PendingPairing),
    PairingResolved { request_id: u64, outcome: Outcome },
    /// A pairing attempt failed (`bad_code`, `bad_token`, `denied`, `timeout`, …).
    PairingFailed {
        method: String,
        peer: String,
        reason: String,
    },
    /// Code pairing is locked after repeated wrong codes.
    PairingLocked { until_unix_ms: i64 },
    /// A phone was stored as paired.
    Paired {
        device_id: String,
        device_name: String,
        method: String,
    },
}

/// A code to show on the PC.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CodePairing {
    /// 6 digits, e.g. `"012345"` (show as `"012 345"`, see [`code::display_code`]).
    pub code: String,
    pub host_name: String,
    #[serde(skip)]
    pub expires_at: Instant,
    pub expires_unix_ms: i64,
}

pub struct G4State {
    codes: Mutex<CodeState>,
    pub approvals: Arc<Approvals>,
    events: broadcast::Sender<HostEvent>,
}

impl G4State {
    pub fn new(data_dir: &std::path::Path, approval_timeout: Duration) -> G4State {
        G4State {
            codes: Mutex::new(CodeState::default()),
            approvals: Arc::new(Approvals::new(data_dir, approval_timeout)),
            events: broadcast::channel(64).0,
        }
    }

    /// Host events from now on (lagging receivers lose the oldest).
    pub fn subscribe(&self) -> broadcast::Receiver<HostEvent> {
        self.events.subscribe()
    }

    pub fn emit(&self, event: HostEvent) {
        let _ = self.events.send(event);
    }

    /// Show a fresh pairing code (the previous one stops working). `Err` = locked for that long.
    pub fn new_code(&self, host_name: &str) -> Result<CodePairing, Duration> {
        let (code, expires_at) = self.lock_codes().issue()?;
        let expires_unix_ms =
            util::now_ms() + expires_at.saturating_duration_since(Instant::now()).as_millis() as i64;
        let p = CodePairing {
            code,
            host_name: host_name.to_string(),
            expires_at,
            expires_unix_ms,
        };
        self.emit(HostEvent::PairingCode {
            code: p.code.clone(),
            host_name: p.host_name.clone(),
            expires_unix_ms,
        });
        Ok(p)
    }

    /// The code shown now, if any.
    pub fn current_code(&self) -> Option<String> {
        self.lock_codes().current().map(|(c, _)| c)
    }

    /// Stop showing the code.
    pub fn cancel_code(&self) {
        self.lock_codes().clear();
        self.emit(HostEvent::PairingCodeEnded {
            reason: "replaced".into(),
        });
    }

    pub fn code_locked_for(&self) -> Option<Duration> {
        self.lock_codes().locked_for()
    }

    pub fn settings(&self) -> PairingSettings {
        self.approvals.settings()
    }

    pub fn set_require_approval(&self, on: bool) -> anyhow::Result<PairingSettings> {
        self.approvals.set_require_approval(on)
    }

    /// Allow (`true`) or deny a waiting pairing. False if it is no longer waiting.
    pub fn resolve(&self, request_id: u64, allow: bool) -> bool {
        self.approvals.resolve(request_id, allow)
    }

    pub fn pending(&self) -> Vec<PendingPairing> {
        self.approvals.pending()
    }

    fn lock_codes(&self) -> std::sync::MutexGuard<'_, CodeState> {
        self.codes.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// Handshake failure: `(error code, message)`.
pub(crate) type Failure = (&'static str, String);

/// If "ask before a new phone pairs" is on, wait (≤ timeout) for the PC owner. The phone gets
/// `pair_pending` first. A phone already paired with the same key is not asked about again.
pub(crate) async fn await_approval<S>(
    state: &Arc<AppState>,
    ws: &mut WebSocketStream<S>,
    peer: &str,
    device_id: &str,
    device_name: &str,
    public_key: &str,
    method: &str,
) -> Result<(), Failure>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let g4 = &state.g4;
    if !g4.approvals.require_approval() {
        return Ok(());
    }
    if let Ok(Some(d)) = state.devices.get(device_id)
        && d.public_key == public_key.trim()
    {
        return Ok(());
    }
    let (info, rx) = g4.approvals.begin(device_id, device_name, peer, method);
    let id = info.request_id;
    g4.emit(HostEvent::PairingRequest(info));
    show_approval_toast(state, id, device_name, peer).await;
    let timeout = g4.approvals.timeout;
    let pending = json!({"type": "pair_pending", "timeout_s": timeout.as_secs()});
    let outcome = if !send_json(ws, &pending).await {
        Outcome::Cancelled
    } else {
        let gone = async {
            loop {
                match ws.next().await {
                    None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return,
                    Some(Ok(_)) => continue,
                }
            }
        };
        tokio::select! {
            r = rx => match r {
                Ok(true) => Outcome::Allowed,
                _ => Outcome::Denied,
            },
            _ = tokio::time::sleep(timeout) => Outcome::Timeout,
            _ = gone => Outcome::Cancelled,
        }
    };
    g4.approvals.forget(id);
    g4.emit(HostEvent::PairingResolved {
        request_id: id,
        outcome,
    });
    state.audit.record(
        "pair_approval",
        Some(device_id),
        Some(peer),
        &format!("method={method} outcome={}", outcome.as_str()),
    );
    match outcome {
        Outcome::Allowed => Ok(()),
        Outcome::Timeout => Err((
            codes::DENIED,
            "nobody allowed this phone on the PC in time".into(),
        )),
        _ => Err((codes::DENIED, "the PC did not allow this phone".into())),
    }
}

/// Windows toast with Allow / Deny (through the platform, so tests use the fake).
async fn show_approval_toast(state: &Arc<AppState>, id: u64, device_name: &str, peer: &str) {
    let from = if crate::relay::is_relayed_peer(peer) {
        "coming through the relay, not on this network,".to_string()
    } else {
        let ip = peer.rsplit_once(':').map(|(ip, _)| ip).unwrap_or(peer);
        format!("at {ip}")
    };
    let toast = Toast {
        title: format!("Allow \u{201c}{device_name}\u{201d} to pair with this PC?"),
        body: format!(
            "A phone {from} wants to control this PC. It is denied automatically in {} s.",
            state.g4.approvals.timeout.as_secs()
        ),
        actions: vec![
            ToastAction {
                id: "allow".into(),
                label: "Allow".into(),
            },
            ToastAction {
                id: "deny".into(),
                label: "Deny".into(),
            },
        ],
        tag: Some(format!("pair-{id}")),
        group: Some("helm-pair".into()),
        ..Toast::default()
    };
    let approvals = Arc::downgrade(&state.g4.approvals);
    let on_event: crate::platform::ToastCallback = Box::new(move |ev: ToastEvent| {
        if let ToastEvent::Activated {
            action_id: Some(a), ..
        } = ev
            && let Some(ap) = approvals.upgrade()
        {
            ap.resolve(id, a == "allow");
        }
    });
    let platform = state.platform.clone();
    if let Err(e) = crate::platform::blocking(move || platform.show_toast(&toast, on_event)).await
    {
        tracing::warn!("pairing approval toast unavailable: {}", e.message);
    }
}

fn b64_exact(text: &str, len: usize) -> Option<Vec<u8>> {
    util::b64_decode(text).filter(|b| b.len() == len)
}

/// Next `code_open` for `round`; `Err` with the failure to send.
async fn read_open<S>(
    ws: &mut WebSocketStream<S>,
    round: usize,
    limit: Duration,
) -> Result<Vec<u8>, Failure>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let deadline = tokio::time::Instant::now() + limit;
    let text = match read_first(ws, deadline).await {
        First::Text(t) => t,
        First::TimedOut => return Err((codes::TIMEOUT, "pairing code exchange timed out".into())),
        First::Binary => return Err((codes::BAD_MESSAGE, "expected a JSON text message".into())),
        First::Gone => return Err((codes::BAD_CODE, "the phone left during pairing".into())),
    };
    let v: Value = serde_json::from_str(&text)
        .map_err(|_| (codes::BAD_MESSAGE, "message is not JSON".to_string()))?;
    match v.get("type").and_then(Value::as_str) {
        Some("code_open") => {}
        // The phone saw a wrong opening from us: wrong code or someone in the middle.
        Some("error") => {
            return Err((
                codes::BAD_CODE,
                "the phone rejected the PC's code proof".into(),
            ));
        }
        _ => return Err((codes::BAD_MESSAGE, "expected code_open".into())),
    }
    if v.get("i").and_then(Value::as_u64) != Some(round as u64) {
        return Err((codes::BAD_MESSAGE, "code_open for the wrong digit".into()));
    }
    v.get("r")
        .and_then(Value::as_str)
        .and_then(|r| b64_exact(r, code::R_LEN))
        .ok_or_else(|| (codes::BAD_MESSAGE, "code_open.r must be 32 bytes".to_string()))
}

/// The agent side of code pairing (after `hello`, the phone sent `pair_code`). On success the
/// device is approved (if asked) and stored; the caller sends `ready`.
pub(crate) async fn run_code_pairing<S>(
    state: &Arc<AppState>,
    ws: &mut WebSocketStream<S>,
    peer: &str,
    nonce_a: &str,
    msg: PairCode,
) -> Result<(String, Tier), Failure>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let host_id = state.identity.host_id.clone();
    // 1. Proof of the device key (does not touch the code).
    let signed = proto::signed_string("pair_code", &host_id, nonce_a);
    let valid = proto::parse_public_key(&msg.public_key)
        .is_some_and(|key| proto::verify_signature(&key, &signed, &msg.signature));
    if !valid {
        state
            .audit
            .record("pair_fail", Some(&msg.device_id), Some(peer), "code bad_signature");
        return Err((codes::BAD_SIGNATURE, "public key or signature is invalid".into()));
    }
    let commits_p: Vec<Vec<u8>> = msg
        .commits
        .iter()
        .filter_map(|c| b64_exact(c, 32))
        .collect();
    if commits_p.len() != code::DIGITS || msg.commits.len() != code::DIGITS {
        return Err((codes::BAD_MESSAGE, "commits must be 6 × 32 bytes".into()));
    }
    if b64_exact(&msg.nonce, 32).is_none() {
        return Err((codes::BAD_MESSAGE, "nonce must be 32 bytes".into()));
    }

    // 2. Use up the code: one attempt per code, whatever happens next.
    let taken = state.g4.lock_codes().take();
    let digits = match taken {
        Take::Locked => {
            state
                .audit
                .record("pair_fail", Some(&msg.device_id), Some(peer), "code pairing_locked");
            return Err((
                codes::PAIRING_LOCKED,
                "too many wrong pairing codes; try again later".into(),
            ));
        }
        Take::NoCode => {
            state
                .audit
                .record("pair_fail", Some(&msg.device_id), Some(peer), "code no_code");
            return Err((
                codes::BAD_CODE,
                "no pairing code is shown on the PC (it expired or was used)".into(),
            ));
        }
        Take::Ok(d) => d,
    };
    state.g4.emit(HostEvent::PairingCodeEnded {
        reason: "used".into(),
    });

    match exchange(state, ws, nonce_a, &msg, &commits_p, &digits).await {
        Ok(()) => state.g4.lock_codes().record_success(),
        Err(failure) => {
            let locked = state.g4.lock_codes().record_failure();
            state.audit.record(
                "pair_fail",
                Some(&msg.device_id),
                Some(peer),
                &format!("code {}", failure.0),
            );
            state.g4.emit(HostEvent::PairingFailed {
                method: "code".into(),
                peer: peer.to_string(),
                reason: failure.0.to_string(),
            });
            if locked {
                let left = state.g4.code_locked_for().unwrap_or_default();
                state.g4.emit(HostEvent::PairingLocked {
                    until_unix_ms: util::now_ms() + left.as_millis() as i64,
                });
                state.audit.record("pair_locked", None, Some(peer), "code");
            }
            return Err(failure);
        }
    }

    // 3. Owner approval, then store.
    if let Err(f) = await_approval(
        state,
        ws,
        peer,
        &msg.device_id,
        &msg.device_name,
        &msg.public_key,
        "code",
    )
    .await
    {
        state.g4.emit(HostEvent::PairingFailed {
            method: "code".into(),
            peer: peer.to_string(),
            reason: f.0.to_string(),
        });
        return Err(f);
    }
    crate::server::store_paired(
        state,
        peer,
        &msg.device_id,
        &msg.device_name,
        &msg.public_key,
        "code",
    )
}

/// Commit to every digit, then open them one at a time, checking the phone's openings.
async fn exchange<S>(
    state: &Arc<AppState>,
    ws: &mut WebSocketStream<S>,
    nonce_a: &str,
    msg: &PairCode,
    commits_p: &[Vec<u8>],
    digits: &[u8; code::DIGITS],
) -> Result<(), Failure>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let h = code::transcript_hash(&code::Transcript {
        host_id: &state.identity.host_id,
        nonce_a,
        nonce_p: &msg.nonce,
        fingerprint: &state.identity.fingerprint,
        device_id: &msg.device_id,
        public_key: &msg.public_key,
    });
    let r_a: Vec<Vec<u8>> = (0..code::DIGITS)
        .map(|_| util::random_bytes(code::R_LEN))
        .collect();
    let commits_a: Vec<String> = (0..code::DIGITS)
        .map(|i| {
            util::b64_encode(&code::commitment(
                Role::Agent,
                i + 1,
                digits[i],
                &h,
                &r_a[i],
            ))
        })
        .collect();
    let lost = || (codes::BAD_CODE, "the phone left during pairing".to_string());
    if !send_json(ws, &json!({"type": "code_commit", "commits": commits_a})).await {
        return Err(lost());
    }
    let limit = state.handshake_timeout();
    for i in 0..code::DIGITS {
        let round = i + 1;
        let open = json!({"type": "code_open", "i": round, "r": util::b64_encode(&r_a[i])});
        if !send_json(ws, &open).await {
            return Err(lost());
        }
        let r_p = read_open(ws, round, limit).await?;
        if !code::opening_matches(&commits_p[i], Role::Phone, round, digits[i], &h, &r_p) {
            return Err((
                codes::BAD_CODE,
                "the pairing code does not match (wrong code, or someone in the middle)".into(),
            ));
        }
    }
    Ok(())
}

/// `pairing.settings.get` (admin) / `pairing.settings.set{require_approval}` (admin).
pub struct PairingModule;

const METHODS: &[MethodSpec] = &[
    MethodSpec {
        name: "pairing.settings.get",
        min_tier: Tier::Admin,
    },
    MethodSpec {
        name: "pairing.settings.set",
        min_tier: Tier::Admin,
    },
];

#[async_trait]
impl Module for PairingModule {
    fn methods(&self) -> &'static [MethodSpec] {
        METHODS
    }

    async fn call(
        &self,
        method: &str,
        params: Value,
        ctx: &RequestContext,
    ) -> Result<Value, RpcError> {
        let g4 = &ctx.state.g4;
        match method {
            "pairing.settings.get" => Ok(json!(g4.settings())),
            "pairing.settings.set" => {
                let on = params
                    .get("require_approval")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| RpcError::bad_params("require_approval (bool) is required"))?;
                let s = g4
                    .set_require_approval(on)
                    .map_err(|e| RpcError::internal(e.to_string()))?;
                ctx.state.audit.record(
                    "pairing_settings",
                    Some(&ctx.device_id),
                    Some(&ctx.peer),
                    &format!("require_approval={on}"),
                );
                Ok(json!(s))
            }
            _ => Err(RpcError::unknown_method(method)),
        }
    }
}
