//! g9: view and control the *phone* from the PC (Docs/protocol/phone-control.md).
//!
//! The phone stays in charge: it is asked for every session, and the PC only ever receives what
//! the phone chose to send. This module is the PC half: the wire codecs (kept pure and tested
//! here), the [`PhoneHub`] state machine with its audit trail, and the lib API the desktop app
//! uses ([`PhoneHub::request`], [`PhoneHub::subscribe`], [`PhoneHub::next_frame`],
//! [`PhoneHub::input`], [`PhoneHub::stop`]). [`socket`] serves the phone's half of `/stream`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{Notify, broadcast, mpsc};

use crate::proto;
use crate::server::AppState;
use crate::session::Session;
use crate::stream::{HEADER_LEN, KIND_VIDEO, KIND_VIDEO_KEY, StreamQueue, TokenError, TokenStore};

pub mod socket;

/// How long the person at the phone has to answer: consent screen, then the Android prompt.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
/// Fragment of an access unit too large for one 1 MiB WebSocket message; more follow.
pub const KIND_VIDEO_PART: u8 = 0x06;
/// One access unit may not reassemble beyond this (same limit as agent → phone `/stream`).
pub const MAX_AU: usize = 16 * 1024 * 1024;
/// `text` input: at most this many UTF-8 bytes per event.
pub const MAX_TEXT_BYTES: usize = 8 * 1024;
/// Input events allowed to wait for the `/stream` socket; beyond that they are dropped.
const INPUT_QUEUE: usize = 256;

// ---------------------------------------------------------------- frames

/// `[kind u8][pts_us u64 BE][payload]` (the layout of PROTOCOL §10). `None` when too short.
pub fn parse_frame(frame: &[u8]) -> Option<(u8, u64, &[u8])> {
    if frame.len() < HEADER_LEN {
        return None;
    }
    let mut pts = [0u8; 8];
    pts.copy_from_slice(&frame[1..HEADER_LEN]);
    Some((frame[0], u64::from_be_bytes(pts), &frame[HEADER_LEN..]))
}

/// Undoes the phone's fragmenting: `0x06` parts + a final `0x01` / `0x02` with the same pts make
/// one access unit. A frame that was never split passes through untouched.
#[derive(Debug, Default)]
pub struct Reassembler {
    pts: u64,
    parts: Vec<u8>,
    dropped_pts: Option<u64>,
}

/// What [`Reassembler::push`] made of one frame.
#[derive(Debug, PartialEq, Eq)]
pub enum Assembled {
    /// Nothing complete yet (a part, or an unknown kind, which is ignored).
    Pending,
    /// A whole access unit in §10 layout.
    Frame { frame: Vec<u8>, key: bool },
    /// An access unit was too large and is gone: the next pictures need a keyframe.
    Lost,
}

impl Reassembler {
    pub fn push(&mut self, frame: Vec<u8>) -> Assembled {
        let Some((kind, pts, payload)) = parse_frame(&frame) else {
            return Assembled::Pending;
        };
        match kind {
            KIND_VIDEO_PART => {
                if self.dropped_pts == Some(pts) {
                    return Assembled::Pending;
                }
                if self.pts != pts {
                    self.parts.clear();
                }
                self.pts = pts;
                if self.parts.len() + payload.len() > MAX_AU {
                    self.parts = Vec::new();
                    self.dropped_pts = Some(pts);
                    return Assembled::Pending;
                }
                self.parts.extend_from_slice(payload);
                Assembled::Pending
            }
            KIND_VIDEO | KIND_VIDEO_KEY => {
                let key = kind == KIND_VIDEO_KEY;
                if self.dropped_pts == Some(pts) {
                    self.dropped_pts = None;
                    return Assembled::Lost;
                }
                if payload.is_empty() {
                    return Assembled::Pending;
                }
                if self.parts.is_empty() || self.pts != pts {
                    self.parts.clear();
                    return Assembled::Frame { frame, key };
                }
                let mut whole = Vec::with_capacity(HEADER_LEN + self.parts.len() + payload.len());
                whole.push(kind);
                whole.extend_from_slice(&pts.to_be_bytes());
                whole.append(&mut self.parts);
                whole.extend_from_slice(payload);
                Assembled::Frame { frame: whole, key }
            }
            _ => Assembled::Pending,
        }
    }
}

// ---------------------------------------------------------------- input

/// Back, Home, Recents, Notifications (`global` input).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GlobalAction {
    Back,
    Home,
    Recents,
    Notifications,
}

/// Editing keys that are not text (`key` input).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EditKey {
    Backspace,
    Enter,
}

/// One PC → phone input event. Coordinates are normalised 0..1 to the picture the phone sends, so
/// they follow its rotation by themselves.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "k", rename_all = "lowercase")]
pub enum PhoneInput {
    Tap { x: f64, y: f64 },
    Long { x: f64, y: f64 },
    Swipe { x1: f64, y1: f64, x2: f64, y2: f64, ms: u32 },
    Down { x: f64, y: f64 },
    Move { x: f64, y: f64 },
    Up { x: f64, y: f64 },
    /// `dy` / `dx` in wheel notches; positive = on to what is below / to the right.
    Scroll { x: f64, y: f64, dy: f64, dx: f64 },
    Global { action: GlobalAction },
    Text { text: String },
    Key { key: EditKey },
}

fn unit(v: f64) -> Result<f64, &'static str> {
    if v.is_finite() {
        Ok(v.clamp(0.0, 1.0))
    } else {
        Err("coordinates must be numbers")
    }
}

impl PhoneInput {
    /// Clamp to the documented ranges; refuse what cannot be meant (NaN, empty or huge text).
    pub fn sanitized(self) -> Result<PhoneInput, &'static str> {
        Ok(match self {
            PhoneInput::Tap { x, y } => PhoneInput::Tap { x: unit(x)?, y: unit(y)? },
            PhoneInput::Long { x, y } => PhoneInput::Long { x: unit(x)?, y: unit(y)? },
            PhoneInput::Down { x, y } => PhoneInput::Down { x: unit(x)?, y: unit(y)? },
            PhoneInput::Move { x, y } => PhoneInput::Move { x: unit(x)?, y: unit(y)? },
            PhoneInput::Up { x, y } => PhoneInput::Up { x: unit(x)?, y: unit(y)? },
            PhoneInput::Swipe { x1, y1, x2, y2, ms } => PhoneInput::Swipe {
                x1: unit(x1)?,
                y1: unit(y1)?,
                x2: unit(x2)?,
                y2: unit(y2)?,
                ms: ms.clamp(1, 5_000),
            },
            PhoneInput::Scroll { x, y, dy, dx } => {
                if !dy.is_finite() || !dx.is_finite() {
                    return Err("scroll amounts must be numbers");
                }
                PhoneInput::Scroll {
                    x: unit(x)?,
                    y: unit(y)?,
                    dy: dy.clamp(-20.0, 20.0),
                    dx: dx.clamp(-20.0, 20.0),
                }
            }
            PhoneInput::Text { text } => {
                if text.is_empty() || text.len() > MAX_TEXT_BYTES {
                    return Err("text must be 1..=8192 bytes");
                }
                PhoneInput::Text { text }
            }
            other @ (PhoneInput::Global { .. } | PhoneInput::Key { .. }) => other,
        })
    }
}

// ---------------------------------------------------------------- lib API types

/// What the PC would like; the phone may send less. Over the relay the agent lowers it further.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestOptions {
    pub max_long: u32,
    pub max_short: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
}

impl Default for RequestOptions {
    fn default() -> Self {
        RequestOptions {
            max_long: 1920,
            max_short: 1080,
            fps: 30,
            bitrate_kbps: 6_000,
        }
    }
}

impl RequestOptions {
    /// Valid ranges, plus the relay limits (its traffic is slow and metered).
    pub fn effective(self, relayed: bool) -> RequestOptions {
        let o = RequestOptions {
            max_long: self.max_long.clamp(320, 4096),
            max_short: self.max_short.clamp(240, 4096),
            fps: self.fps.clamp(1, 60),
            bitrate_kbps: self.bitrate_kbps.clamp(200, 50_000),
        };
        if !relayed {
            return o;
        }
        RequestOptions {
            max_long: o.max_long.min(1280),
            max_short: o.max_short.min(720),
            fps: o.fps.min(15),
            bitrate_kbps: o.bitrate_kbps.min(1_500),
        }
    }
}

/// Why a [`PhoneHub`] call was refused. `code` is stable; the desktop app turns it into a state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhoneError {
    pub code: &'static str,
    pub message: String,
}

impl PhoneError {
    fn new(code: &'static str, message: impl Into<String>) -> PhoneError {
        PhoneError { code, message: message.into() }
    }
}

impl std::fmt::Display for PhoneError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// A connected phone that speaks g9, as last reported by `phone.caps`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PhoneTarget {
    pub session_id: u64,
    pub device_id: String,
    /// "Let PC see this phone" is on.
    pub enabled: bool,
    /// The Helm accessibility service is on (without it, sessions are view-only).
    pub accessibility: bool,
    pub relayed: bool,
}

/// The session as it stands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PhoneInfo {
    pub id: u64,
    pub device_id: String,
    pub relayed: bool,
    pub width: u32,
    pub height: u32,
    pub rotation: u32,
    /// The phone granted control and its accessibility service is on.
    pub control: bool,
    pub accessibility: bool,
    /// What the phone refuses to act on right now (`"password"`), if anything.
    pub secure: Option<String>,
}

/// Session lifecycle, for the desktop app.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum PhoneEvent {
    /// [`PhoneHub::targets`] changed.
    Targets,
    /// Asked; waiting for the person at the phone.
    Waiting { id: u64, device_id: String, relayed: bool },
    /// The phone accepted and is opening its stream socket.
    Accepted { info: PhoneInfo },
    /// Frames flow.
    Active { info: PhoneInfo },
    /// Size, rotation, control or secure changed.
    State { info: PhoneInfo },
    /// Over. `reason` is one of the codes in the spec (`declined`, `disconnected`, ...).
    Ended { id: u64, device_id: String, reason: String, was_active: bool },
}

/// Reasons a phone may give (`phone.decline`, `phone.stop`). Anything else is recorded as the
/// generic [`REASON_PHONE`], so a phone cannot write arbitrary words into the audit log.
pub const PHONE_REASONS: &[&str] = &[
    "declined",
    "capture_denied",
    "disabled",
    "busy",
    "stopped_on_phone",
    "screen_locked",
    "idle",
    "capture_stopped",
    "encoder_failed",
    "unsupported",
];
pub const REASON_PHONE: &str = "stopped_on_phone";

// ---------------------------------------------------------------- frame queue

/// Frames waiting for the (single) desktop consumer. Same rules as the screen stream
/// ([`StreamQueue`]): at most 3 frames, non-key frames that do not fit are dropped until the next
/// keyframe, and a keyframe is requested once the queue has drained.
#[derive(Default)]
struct FrameQueue {
    q: Mutex<StreamQueue>,
    ready: Notify,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

// ---------------------------------------------------------------- hub

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Asking,
    Accepted,
    Active,
}

struct Current {
    id: u64,
    session: Arc<Session>,
    peer: String,
    phase: Phase,
    info: PhoneInfo,
    started: Option<Instant>,
    stream_tx: Option<mpsc::Sender<String>>,
}

#[derive(Default)]
struct Hub {
    /// By control session: what its phone last said in `phone.caps`, and its peer address.
    targets: HashMap<u64, (PhoneTarget, String)>,
    current: Option<Current>,
}

/// The agent's single phone session (one at a time: there is one person at the PC).
pub struct PhoneHub {
    hub: Mutex<Hub>,
    next_id: AtomicU64,
    tokens: Mutex<TokenStore<u64>>,
    token_lifetime: Duration,
    request_timeout: Duration,
    events: broadcast::Sender<PhoneEvent>,
    frames: FrameQueue,
}

impl PhoneHub {
    pub fn new(request_timeout: Duration, token_lifetime: Duration) -> PhoneHub {
        PhoneHub {
            hub: Mutex::new(Hub::default()),
            next_id: AtomicU64::new(1),
            tokens: Mutex::new(TokenStore::new(token_lifetime)),
            token_lifetime,
            request_timeout,
            events: broadcast::channel(64).0,
            frames: FrameQueue::default(),
        }
    }

    // ------------------------------------------------------------ lib API (desktop app)

    /// Session events from now on (slow receivers get `RecvError::Lagged`).
    pub fn subscribe(&self) -> broadcast::Receiver<PhoneEvent> {
        self.events.subscribe()
    }

    /// Connected phones that speak g9.
    pub fn targets(&self) -> Vec<PhoneTarget> {
        let mut out: Vec<PhoneTarget> =
            lock(&self.hub).targets.values().map(|(t, _)| t.clone()).collect();
        out.sort_by_key(|t| t.session_id);
        out
    }

    pub fn current(&self) -> Option<PhoneInfo> {
        lock(&self.hub).current.as_ref().map(|c| c.info.clone())
    }

    /// Ask a connected phone. The phone shows its consent screen; the outcome arrives as
    /// [`PhoneEvent`]s. Must run inside the Tokio runtime (it arms the answer timeout).
    pub fn request(
        &self,
        state: &Arc<AppState>,
        device_id: &str,
        options: RequestOptions,
    ) -> Result<PhoneInfo, PhoneError> {
        let session = state
            .sessions()
            .into_iter()
            .filter(|s| s.device_id == device_id)
            .max_by_key(|s| s.id)
            .ok_or_else(|| {
                PhoneError::new("not_connected", "That phone is not connected right now.")
            })?;
        let (info, peer, options) = {
            let mut hub = lock(&self.hub);
            if hub.current.is_some() {
                return Err(PhoneError::new(
                    "busy",
                    "A phone session is already running. Stop it first.",
                ));
            }
            let Some((target, peer)) = hub.targets.get(&session.id).cloned() else {
                return Err(PhoneError::new(
                    "unsupported",
                    "Helm on that phone is too old for this. Update the app on the phone.",
                ));
            };
            if !target.enabled {
                return Err(PhoneError::new(
                    "disabled",
                    "\"Let PC see this phone\" is off. Turn it on in Helm on the phone.",
                ));
            }
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            let info = PhoneInfo {
                id,
                device_id: device_id.to_string(),
                relayed: target.relayed,
                width: 0,
                height: 0,
                rotation: 0,
                control: false,
                accessibility: target.accessibility,
                secure: None,
            };
            hub.current = Some(Current {
                id,
                session: session.clone(),
                peer: peer.clone(),
                phase: Phase::Asking,
                info: info.clone(),
                started: None,
                stream_tx: None,
            });
            (info, peer, options.effective(target.relayed))
        };
        lock(&self.frames.q).reset_for_viewer();
        state.audit.record(
            "phone_request",
            Some(device_id),
            Some(&peer),
            &format!("id={} relayed={}", info.id, info.relayed),
        );
        session.send_event(
            "phone.request",
            json!({
                "request_id": info.id,
                "pc_name": state.identity.host_name,
                "relayed": info.relayed,
                "max_long": options.max_long,
                "max_short": options.max_short,
                "fps": options.fps,
                "bitrate_kbps": options.bitrate_kbps,
            }),
        );
        let _ = self.events.send(PhoneEvent::Waiting {
            id: info.id,
            device_id: device_id.to_string(),
            relayed: info.relayed,
        });
        self.expire_after(state, info.id, Phase::Asking, self.request_timeout, "timeout");
        Ok(info)
    }

    /// End the session from the PC. Safe when nothing runs.
    pub fn stop(&self, state: &AppState) {
        if let Some(id) = self.current().map(|i| i.id) {
            self.end(state, id, "stopped_by_pc", true);
        }
    }

    /// Next frame for the desktop app (§10 layout: `0x01` / `0x02`). Waits; cancel-safe.
    pub async fn next_frame(&self) -> Vec<u8> {
        loop {
            let waited = self.frames.ready.notified();
            let popped = lock(&self.frames.q).pop();
            if let Some(p) = popped {
                if p.request_idr {
                    self.request_keyframe();
                }
                return p.frame;
            }
            waited.await;
        }
    }

    /// Ask the phone for a keyframe (decoder reset, frames dropped further on).
    pub fn request_keyframe(&self) {
        if let Some(c) = lock(&self.hub).current.as_ref()
            && c.phase != Phase::Asking
        {
            c.session.send_event("phone.idr", json!({}));
        }
    }

    /// One input event. **Dropped (`not_allowed`) unless a session is active and the phone
    /// granted control**; the phone checks the same again on its side.
    pub fn input(&self, event: PhoneInput) -> Result<(), PhoneError> {
        let event = event
            .sanitized()
            .map_err(|why| PhoneError::new("bad_input", why))?;
        let hub = lock(&self.hub);
        let current = hub
            .current
            .as_ref()
            .filter(|c| c.phase == Phase::Active && c.info.control)
            .ok_or_else(|| PhoneError::new("not_allowed", "The phone has not granted control."))?;
        let data = serde_json::to_value(&event)
            .map_err(|e| PhoneError::new("bad_input", e.to_string()))?;
        let text = proto::event("phone.in", data).to_string();
        match &current.stream_tx {
            Some(tx) => tx
                .try_send(text)
                .map_err(|_| PhoneError::new("busy", "Input is queueing up; dropped one event.")),
            None => Err(PhoneError::new("not_allowed", "The phone's stream is not open.")),
        }
    }

    /// Frames dropped so far because the desktop consumer was behind.
    pub fn dropped_frames(&self) -> u64 {
        lock(&self.frames.q).dropped_video
    }

    // ------------------------------------------------------------ from the phone (`phone.*`)

    /// `phone.caps`: what this connected phone currently allows. `peer` tells LAN from relay.
    pub fn on_caps(&self, session: &Session, peer: &str, enabled: bool, accessibility: bool) {
        let target = PhoneTarget {
            session_id: session.id,
            device_id: session.device_id.clone(),
            enabled,
            accessibility,
            relayed: crate::relay::is_relayed_peer(peer),
        };
        lock(&self.hub)
            .targets
            .insert(session.id, (target, peer.to_string()));
        let _ = self.events.send(PhoneEvent::Targets);
    }

    /// `phone.decline`: the person said no (or the phone cannot do it).
    pub fn on_decline(
        &self,
        state: &AppState,
        session_id: u64,
        request_id: u64,
        reason: &str,
    ) -> Result<(), PhoneError> {
        self.check_owner(session_id, request_id, &[Phase::Asking])?;
        self.end(state, request_id, phone_reason(reason), false);
        Ok(())
    }

    /// `phone.accept`: consent + Android's capture prompt done. Returns the `/stream` token.
    #[allow(clippy::too_many_arguments)]
    pub fn on_accept(
        &self,
        state: &Arc<AppState>,
        session: &Session,
        request_id: u64,
        width: u32,
        height: u32,
        rotation: u32,
        control: bool,
    ) -> Result<String, PhoneError> {
        self.check_owner(session.id, request_id, &[Phase::Asking])?;
        let info = {
            let mut hub = lock(&self.hub);
            let current = hub
                .current
                .as_mut()
                .ok_or_else(|| PhoneError::new("no_session", "No such request."))?;
            current.phase = Phase::Accepted;
            current.info.width = width;
            current.info.height = height;
            current.info.rotation = rotation;
            current.info.control = control;
            current.info.clone()
        };
        let token = lock(&self.tokens).issue(&session.device_id, request_id, Instant::now());
        let _ = self.events.send(PhoneEvent::Accepted { info });
        self.expire_after(state, request_id, Phase::Accepted, self.token_lifetime, "open_timeout");
        Ok(token)
    }

    /// `phone.state`: anything the phone wants the PC to know mid-session.
    pub fn on_state(
        &self,
        session_id: u64,
        request_id: u64,
        change: StateChange,
    ) -> Result<(), PhoneError> {
        self.check_owner(session_id, request_id, &[Phase::Accepted, Phase::Active])?;
        let info = {
            let mut hub = lock(&self.hub);
            let current = hub
                .current
                .as_mut()
                .ok_or_else(|| PhoneError::new("no_session", "No such session."))?;
            let info = &mut current.info;
            if let Some(v) = change.width {
                info.width = v;
            }
            if let Some(v) = change.height {
                info.height = v;
            }
            if let Some(v) = change.rotation {
                info.rotation = v;
            }
            if let Some(v) = change.control {
                info.control = v;
            }
            if let Some(v) = change.accessibility {
                info.accessibility = v;
            }
            if let Some(v) = change.secure {
                info.secure = v;
            }
            info.clone()
        };
        let _ = self.events.send(PhoneEvent::State { info });
        Ok(())
    }

    /// `phone.stop`: ended on the phone (Stop button, screen lock, idle, ...).
    pub fn on_stop(
        &self,
        state: &AppState,
        session_id: u64,
        request_id: u64,
        reason: &str,
    ) -> Result<(), PhoneError> {
        self.check_owner(
            session_id,
            request_id,
            &[Phase::Asking, Phase::Accepted, Phase::Active],
        )?;
        self.end(state, request_id, phone_reason(reason), false);
        Ok(())
    }

    /// A control session ended: forget its phone, and end its phone session if it had one.
    pub fn on_session_closed(&self, state: &AppState, session_id: u64) {
        let (forgot, owned) = {
            let mut hub = lock(&self.hub);
            let forgot = hub.targets.remove(&session_id).is_some();
            let owned = hub
                .current
                .as_ref()
                .filter(|c| c.session.id == session_id)
                .map(|c| c.id);
            (forgot, owned)
        };
        if let Some(id) = owned {
            self.end(state, id, "disconnected", false);
        }
        if forgot {
            let _ = self.events.send(PhoneEvent::Targets);
        }
    }

    // ------------------------------------------------------------ the `/stream` side

    /// `phone.open`: the token becomes the live session. Returns its id and what to send down.
    pub(crate) fn open(
        &self,
        state: &AppState,
        token: &str,
        device_id: &str,
    ) -> Result<(u64, mpsc::Receiver<String>), TokenError> {
        let id = lock(&self.tokens).take(token, device_id, Instant::now())?;
        let (tx, rx) = mpsc::channel(INPUT_QUEUE);
        let (info, peer, session) = {
            let mut hub = lock(&self.hub);
            let current = hub
                .current
                .as_mut()
                .filter(|c| c.id == id && c.phase == Phase::Accepted)
                .ok_or(TokenError::Invalid)?;
            current.phase = Phase::Active;
            current.started = Some(Instant::now());
            current.stream_tx = Some(tx);
            (current.info.clone(), current.peer.clone(), current.session.clone())
        };
        state.audit.record(
            "phone_session_start",
            Some(device_id),
            Some(&peer),
            &format!(
                "id={id} control={} relayed={} size={}x{}",
                info.control, info.relayed, info.width, info.height
            ),
        );
        lock(&self.frames.q).reset_for_viewer();
        // The first picture may predate this socket: ask for a fresh keyframe.
        session.send_event("phone.idr", json!({}));
        let _ = self.events.send(PhoneEvent::Active { info });
        Ok((id, rx))
    }

    /// One whole access unit from the phone. Only accepted for the live session.
    pub(crate) fn on_frame(&self, id: u64, frame: Vec<u8>, key: bool) {
        let live = lock(&self.hub)
            .current
            .as_ref()
            .is_some_and(|c| c.id == id && c.phase == Phase::Active);
        if live {
            lock(&self.frames.q).push_video(frame, key);
            self.frames.ready.notify_one();
        }
    }

    /// The phone's `/stream` socket closed.
    pub(crate) fn on_stream_closed(&self, state: &AppState, id: u64) {
        self.end(state, id, "disconnected", true);
    }

    pub(crate) fn end_for_shutdown(&self, state: &AppState, id: u64) {
        self.end(state, id, "agent_stopping", true);
    }

    // ------------------------------------------------------------ internals

    fn check_owner(
        &self,
        session_id: u64,
        request_id: u64,
        phases: &[Phase],
    ) -> Result<(), PhoneError> {
        let hub = lock(&self.hub);
        match hub.current.as_ref() {
            Some(c)
                if c.id == request_id
                    && c.session.id == session_id
                    && phases.contains(&c.phase) =>
            {
                Ok(())
            }
            _ => Err(PhoneError::new("no_session", "No such request or session.")),
        }
    }

    fn expire_after(
        &self,
        state: &Arc<AppState>,
        id: u64,
        phase: Phase,
        after: Duration,
        reason: &'static str,
    ) {
        let weak = Arc::downgrade(state);
        tokio::spawn(async move {
            tokio::time::sleep(after).await;
            let Some(st) = weak.upgrade() else { return };
            let still = lock(&st.phone.hub)
                .current
                .as_ref()
                .is_some_and(|c| c.id == id && c.phase == phase);
            if still {
                st.phone.end(&st, id, reason, true);
            }
        });
    }

    /// The one way a session ends: audit, tell the phone (when it does not know yet), tell the
    /// desktop app, and stop accepting input and frames at once.
    fn end(&self, state: &AppState, id: u64, reason: &str, tell_phone: bool) {
        let taken = {
            let mut hub = lock(&self.hub);
            if hub.current.as_ref().is_some_and(|c| c.id == id) {
                hub.current.take()
            } else {
                None
            }
        };
        let Some(current) = taken else { return };
        lock(&self.tokens).revoke_where(|v| *v == id);
        let device_id = current.info.device_id.clone();
        let was_active = current.phase == Phase::Active;
        if was_active {
            let secs = current.started.map(|t| t.elapsed().as_secs()).unwrap_or(0);
            state.audit.record(
                "phone_session_stop",
                Some(&device_id),
                Some(&current.peer),
                &format!("id={id} reason={reason} secs={secs}"),
            );
        } else {
            state.audit.record(
                "phone_request_end",
                Some(&device_id),
                Some(&current.peer),
                &format!("id={id} reason={reason}"),
            );
        }
        if tell_phone {
            current
                .session
                .send_event("phone.cancel", json!({"request_id": id, "reason": reason}));
        }
        let _ = self.events.send(PhoneEvent::Ended {
            id,
            device_id,
            reason: reason.to_string(),
            was_active,
        });
    }
}

fn phone_reason(reason: &str) -> &str {
    if PHONE_REASONS.contains(&reason) { reason } else { REASON_PHONE }
}

/// Parsed `phone.state` params. `secure: Some(None)` clears it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StateChange {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub rotation: Option<u32>,
    pub control: Option<bool>,
    pub accessibility: Option<bool>,
    pub secure: Option<Option<String>>,
}

/// JSON for the desktop app: the events are exactly their serde form.
pub fn event_json(ev: &PhoneEvent) -> Value {
    serde_json::to_value(ev).unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(kind: u8, pts: u64, payload: &[u8]) -> Vec<u8> {
        crate::stream::encode_frame(kind, pts, payload)
    }

    #[test]
    fn frame_layout_matches_section_10() {
        let f = frame(KIND_VIDEO_KEY, 0x0102_0304_0506_0708, &[0xAA, 0xBB]);
        assert_eq!(f, vec![0x02, 1, 2, 3, 4, 5, 6, 7, 8, 0xAA, 0xBB]);
        assert_eq!(parse_frame(&f), Some((2, 0x0102_0304_0506_0708, &[0xAA, 0xBB][..])));
        assert_eq!(parse_frame(&[1, 2, 3]), None);
        assert_eq!(parse_frame(&frame(1, 7, &[])), Some((1, 7, &[][..])));
    }

    #[test]
    fn reassembler_passes_whole_frames_and_joins_parts() {
        let mut r = Reassembler::default();
        let whole = frame(KIND_VIDEO, 5, &[1, 2]);
        assert_eq!(r.push(whole.clone()), Assembled::Frame { frame: whole, key: false });
        assert_eq!(r.push(frame(KIND_VIDEO_PART, 9, &[1, 2])), Assembled::Pending);
        assert_eq!(r.push(frame(KIND_VIDEO_PART, 9, &[3])), Assembled::Pending);
        assert_eq!(
            r.push(frame(KIND_VIDEO_KEY, 9, &[4])),
            Assembled::Frame { frame: frame(KIND_VIDEO_KEY, 9, &[1, 2, 3, 4]), key: true }
        );
        // Parts of an AU that never finished do not leak into the next one.
        assert_eq!(r.push(frame(KIND_VIDEO_PART, 10, &[7])), Assembled::Pending);
        assert_eq!(
            r.push(frame(KIND_VIDEO, 11, &[8])),
            Assembled::Frame { frame: frame(KIND_VIDEO, 11, &[8]), key: false }
        );
        assert_eq!(r.push(frame(KIND_VIDEO_PART, 12, &[1])), Assembled::Pending);
        assert_eq!(r.push(frame(KIND_VIDEO_PART, 13, &[2])), Assembled::Pending);
        assert_eq!(
            r.push(frame(KIND_VIDEO, 13, &[3])),
            Assembled::Frame { frame: frame(KIND_VIDEO, 13, &[2, 3]), key: false }
        );
        // Unknown kinds and junk are ignored; an empty AU is not a frame.
        assert_eq!(r.push(frame(0x7f, 1, &[1])), Assembled::Pending);
        assert_eq!(r.push(vec![1, 2]), Assembled::Pending);
        assert_eq!(r.push(frame(KIND_VIDEO, 14, &[])), Assembled::Pending);
    }

    #[test]
    fn reassembler_refuses_oversized_access_units() {
        let mut r = Reassembler::default();
        let chunk = vec![0u8; 900 * 1024];
        let mut parts = 0;
        while parts * chunk.len() <= MAX_AU {
            assert_eq!(r.push(frame(KIND_VIDEO_PART, 3, &chunk)), Assembled::Pending);
            parts += 1;
        }
        assert_eq!(r.push(frame(KIND_VIDEO_PART, 3, &chunk)), Assembled::Pending);
        assert_eq!(r.push(frame(KIND_VIDEO, 3, &[1])), Assembled::Lost);
        // The next AU is fine.
        let next = frame(KIND_VIDEO_KEY, 4, &[1]);
        assert_eq!(r.push(next.clone()), Assembled::Frame { frame: next, key: true });
    }

    #[test]
    fn input_wire_form_and_clamping() {
        let e = PhoneInput::Tap { x: 0.25, y: 1.5 }.sanitized().unwrap();
        assert_eq!(e, PhoneInput::Tap { x: 0.25, y: 1.0 });
        assert_eq!(serde_json::to_value(&e).unwrap(), json!({"k":"tap","x":0.25,"y":1.0}));
        let g: PhoneInput =
            serde_json::from_value(json!({"k":"global","action":"recents"})).unwrap();
        assert_eq!(g, PhoneInput::Global { action: GlobalAction::Recents });
        let k: PhoneInput = serde_json::from_value(json!({"k":"key","key":"backspace"})).unwrap();
        assert_eq!(k, PhoneInput::Key { key: EditKey::Backspace });
        assert!(PhoneInput::Tap { x: f64::NAN, y: 0.0 }.sanitized().is_err());
        assert!(PhoneInput::Text { text: String::new() }.sanitized().is_err());
        assert!(
            PhoneInput::Text { text: "x".repeat(MAX_TEXT_BYTES + 1) }
                .sanitized()
                .is_err()
        );
        assert!(
            PhoneInput::Scroll { x: 0.0, y: 0.0, dy: f64::INFINITY, dx: 0.0 }
                .sanitized()
                .is_err()
        );
        let s = PhoneInput::Swipe { x1: -1.0, y1: 0.0, x2: 2.0, y2: 0.5, ms: 0 }
            .sanitized()
            .unwrap();
        assert_eq!(s, PhoneInput::Swipe { x1: 0.0, y1: 0.0, x2: 1.0, y2: 0.5, ms: 1 });
        let s = PhoneInput::Scroll { x: 0.5, y: 0.5, dy: 100.0, dx: -100.0 }
            .sanitized()
            .unwrap();
        assert_eq!(s, PhoneInput::Scroll { x: 0.5, y: 0.5, dy: 20.0, dx: -20.0 });
    }

    #[test]
    fn relay_lowers_the_request() {
        let o = RequestOptions::default();
        assert_eq!(o.effective(false), o);
        assert_eq!(
            o.effective(true),
            RequestOptions { max_long: 1280, max_short: 720, fps: 15, bitrate_kbps: 1500 }
        );
        let big = RequestOptions { max_long: 9999, max_short: 1, fps: 500, bitrate_kbps: 1 };
        assert_eq!(
            big.effective(false),
            RequestOptions { max_long: 4096, max_short: 240, fps: 60, bitrate_kbps: 200 }
        );
    }

    #[test]
    fn phone_cannot_invent_reasons() {
        assert_eq!(phone_reason("declined"), "declined");
        assert_eq!(phone_reason("evil\tvalue"), REASON_PHONE);
    }
}
