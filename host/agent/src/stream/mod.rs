//! Phase 2 screen & audio streaming (PROTOCOL §10).
//!
//! The pure building blocks live here (wire framing, the send queue with its backpressure
//! rules, adaptive bitrate, the shared stream clock, single-use tokens, coordinate mapping);
//! [`active`] runs a stream and [`socket`] serves the `/stream` WebSocket.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use serde_json::json;
use subtle::ConstantTimeEq;

use crate::platform::Rect;

pub mod active;
pub mod socket;

pub use active::ActiveStream;

/// `stream.start` token lifetime (§10: single use, 30 s).
pub const TOKEN_LIFETIME: Duration = Duration::from_secs(30);

// ---- wire framing: [kind u8][pts_us u64 BE][payload] ----

pub const KIND_VIDEO: u8 = 0x01;
pub const KIND_VIDEO_KEY: u8 = 0x02;
pub const KIND_AUDIO: u8 = 0x03;
pub const KIND_CURSOR: u8 = 0x04;
pub const KIND_CURSOR_SHAPE: u8 = 0x05;
pub const HEADER_LEN: usize = 9;

pub fn encode_frame(kind: u8, pts_us: u64, payload: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(HEADER_LEN + payload.len());
    f.push(kind);
    f.extend_from_slice(&pts_us.to_be_bytes());
    f.extend_from_slice(payload);
    f
}

/// `0x04`: JSON `{x,y,visible,shape_id}` in stream pixel coordinates.
pub fn cursor_frame(pts_us: u64, x: i32, y: i32, visible: bool, shape_id: u32) -> Vec<u8> {
    let body = json!({"x": x, "y": y, "visible": visible, "shape_id": shape_id}).to_string();
    encode_frame(KIND_CURSOR, pts_us, body.as_bytes())
}

/// `0x05`: `[u32 shape_id][u16 w][u16 h][u16 hot_x][u16 hot_y]` (big-endian, like the pts) + PNG.
pub fn cursor_shape_frame(
    pts_us: u64,
    shape_id: u32,
    w: u16,
    h: u16,
    hot_x: u16,
    hot_y: u16,
    png: &[u8],
) -> Vec<u8> {
    let mut p = Vec::with_capacity(12 + png.len());
    p.extend_from_slice(&shape_id.to_be_bytes());
    for v in [w, h, hot_x, hot_y] {
        p.extend_from_slice(&v.to_be_bytes());
    }
    p.extend_from_slice(png);
    encode_frame(KIND_CURSOR_SHAPE, pts_us, &p)
}

// ---- send queue ----

/// Video frames allowed to wait for the socket (§10 backpressure).
pub const MAX_QUEUED_VIDEO: usize = 3;
/// Audio packets allowed to wait (0.5 s); the oldest is dropped beyond that.
pub const MAX_QUEUED_AUDIO: usize = 25;

/// One frame taken from the queue for the socket.
#[derive(Debug, PartialEq, Eq)]
pub struct Popped {
    pub frame: Vec<u8>,
    /// The video queue just drained after drops: ask the encoder for an IDR now.
    pub request_idr: bool,
}

/// Everything waiting to be written to the stream socket.
///
/// Video rules: at most [`MAX_QUEUED_VIDEO`] frames. A non-key frame that does not fit is
/// dropped, and so is every later non-key frame until a keyframe arrives (they reference the
/// lost picture). A keyframe always gets in: it replaces queued frames when the queue is full.
/// When the queue drains after a drop, [`Popped::request_idr`] asks for a keyframe (once).
/// Pop order: cursor shapes, audio, cursor position (latest only), video.
#[derive(Debug, Default)]
pub struct StreamQueue {
    video: VecDeque<Vec<u8>>,
    audio: VecDeque<Vec<u8>>,
    shapes: VecDeque<Vec<u8>>,
    cursor: Option<Vec<u8>>,
    waiting_key: bool,
    idr_wanted: bool,
    pub dropped_video: u64,
    pub dropped_audio: u64,
}

impl StreamQueue {
    /// Returns false if the frame was dropped.
    pub fn push_video(&mut self, frame: Vec<u8>, key: bool) -> bool {
        if key {
            if self.video.len() >= MAX_QUEUED_VIDEO {
                self.dropped_video += self.video.len() as u64;
                self.video.clear();
            }
            self.video.push_back(frame);
            self.waiting_key = false;
            self.idr_wanted = false;
            return true;
        }
        if self.waiting_key || self.video.len() >= MAX_QUEUED_VIDEO {
            self.dropped_video += 1;
            if !self.waiting_key {
                self.waiting_key = true;
                self.idr_wanted = true;
            }
            return false;
        }
        self.video.push_back(frame);
        true
    }

    pub fn push_audio(&mut self, frame: Vec<u8>) {
        if self.audio.len() >= MAX_QUEUED_AUDIO {
            self.audio.pop_front();
            self.dropped_audio += 1;
        }
        self.audio.push_back(frame);
    }

    pub fn push_cursor_shape(&mut self, frame: Vec<u8>) {
        self.shapes.push_back(frame);
    }

    /// Position updates coalesce: only the newest one is sent.
    pub fn push_cursor(&mut self, frame: Vec<u8>) {
        self.cursor = Some(frame);
    }

    /// A new viewer: forget queued video and wait for the keyframe the caller requests.
    pub fn reset_for_viewer(&mut self) {
        self.video.clear();
        self.waiting_key = true;
        self.idr_wanted = false;
    }

    pub fn video_len(&self) -> usize {
        self.video.len()
    }

    pub fn pop(&mut self) -> Option<Popped> {
        let plain = |frame| Some(Popped { frame, request_idr: false });
        if let Some(f) = self.shapes.pop_front() {
            return plain(f);
        }
        if let Some(f) = self.audio.pop_front() {
            return plain(f);
        }
        if let Some(f) = self.cursor.take() {
            return plain(f);
        }
        let frame = self.video.pop_front()?;
        let request_idr = self.video.is_empty() && std::mem::take(&mut self.idr_wanted);
        Some(Popped { frame, request_idr })
    }
}

// ---- adaptive bitrate ----

/// Bitrate never adapts below this (or the requested rate, if lower).
pub const ABR_FLOOR_KBPS: u32 = 500;
pub const ABR_RAISE_EVERY: Duration = Duration::from_secs(5);

/// §10: drop 20 % when the phone reports dropped frames or the agent's backlog grows
/// (queue longer than at the last report, or the agent dropped frames since then);
/// raise 10 % (up to the requested rate) after 5 s without trouble.
#[derive(Debug, Clone)]
pub struct Abr {
    pub current: u32,
    pub ceiling: u32,
    floor: u32,
    last_change: Instant,
    last_backlog: usize,
    last_agent_drops: u64,
}

impl Abr {
    pub fn new(kbps: u32, now: Instant) -> Abr {
        Abr {
            current: kbps,
            ceiling: kbps,
            floor: ABR_FLOOR_KBPS.min(kbps),
            last_change: now,
            last_backlog: 0,
            last_agent_drops: 0,
        }
    }

    /// `stream.config{bitrate_kbps}`: new ceiling, applied now.
    pub fn set_target(&mut self, kbps: u32, now: Instant) {
        self.current = kbps;
        self.ceiling = kbps;
        self.floor = ABR_FLOOR_KBPS.min(kbps);
        self.last_change = now;
    }

    /// One `stream.stats` report. `dropped` = frames the phone dropped in the last interval,
    /// `backlog` = video frames queued now, `agent_drops` = total frames dropped by the agent.
    /// Returns the new bitrate when it changes.
    pub fn on_stats(
        &mut self,
        dropped: u64,
        backlog: usize,
        agent_drops: u64,
        now: Instant,
    ) -> Option<u32> {
        let backlog_grew = backlog > self.last_backlog && backlog >= 2;
        let agent_dropped = agent_drops > self.last_agent_drops;
        self.last_backlog = backlog;
        self.last_agent_drops = agent_drops;
        let next = if dropped > 0 || backlog_grew || agent_dropped {
            self.last_change = now;
            ((self.current as u64 * 4 / 5) as u32).max(self.floor)
        } else if now.saturating_duration_since(self.last_change) >= ABR_RAISE_EVERY {
            self.last_change = now;
            (self.current as u64 * 11).div_ceil(10).min(self.ceiling as u64) as u32
        } else {
            self.current
        };
        if next != self.current {
            self.current = next;
            Some(next)
        } else {
            None
        }
    }
}

// ---- one clock for video, audio and cursor ----

/// Stream time in µs since `stream.start`. Video and audio sources each count from their own
/// zero; the first frame of each is placed at the time it was delivered, and later frames
/// keep their source spacing (both sources run on QPC, so they do not drift apart).
/// Video pts stay strictly increasing across an encoder restart (`stream.config{fps}`).
#[derive(Debug, Clone)]
pub struct StreamClock {
    start: Instant,
    video_offset: Option<i64>,
    audio_offset: Option<i64>,
    last_video: Option<u64>,
}

impl StreamClock {
    pub fn new(start: Instant) -> StreamClock {
        StreamClock {
            start,
            video_offset: None,
            audio_offset: None,
            last_video: None,
        }
    }

    pub fn now_us(&self, now: Instant) -> u64 {
        now.saturating_duration_since(self.start).as_micros() as u64
    }

    /// The video source restarted from pts 0: re-anchor at the next frame.
    pub fn reset_video(&mut self) {
        self.video_offset = None;
    }

    pub fn video_pts(&mut self, src_pts: u64, now: Instant) -> u64 {
        let now_us = self.now_us(now) as i64;
        let off = *self.video_offset.get_or_insert(now_us - src_pts as i64);
        let mut pts = (src_pts as i64 + off).max(0) as u64;
        if let Some(last) = self.last_video
            && pts <= last
        {
            pts = last + 1;
        }
        self.last_video = Some(pts);
        pts
    }

    pub fn audio_pts(&mut self, src_pts: u64, now: Instant) -> u64 {
        let now_us = self.now_us(now) as i64;
        let off = *self.audio_offset.get_or_insert(now_us - src_pts as i64);
        (src_pts as i64 + off).max(0) as u64
    }
}

// ---- single-use tokens ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenError {
    /// Unknown, already used, or issued to another device.
    Invalid,
    Expired,
}

struct Pending<T> {
    token: String,
    device_id: String,
    expires: Instant,
    value: T,
}

/// Tokens handed out by `stream.start`, redeemed once by `stream.open` from the same device.
pub struct TokenStore<T> {
    lifetime: Duration,
    pending: Vec<Pending<T>>,
}

impl<T> TokenStore<T> {
    pub fn new(lifetime: Duration) -> TokenStore<T> {
        TokenStore {
            lifetime,
            pending: Vec::new(),
        }
    }

    /// New random token (32 bytes, base64url) for `device_id`.
    pub fn issue(&mut self, device_id: &str, value: T, now: Instant) -> String {
        self.pending.retain(|p| p.expires > now);
        let token = crate::util::b64url_encode(&crate::util::random_bytes(32));
        self.pending.push(Pending {
            token: token.clone(),
            device_id: device_id.to_string(),
            expires: now + self.lifetime,
            value,
        });
        token
    }

    /// Redeem: the token is removed whatever the outcome (single use).
    pub fn take(&mut self, token: &str, device_id: &str, now: Instant) -> Result<T, TokenError> {
        let found = self
            .pending
            .iter()
            .position(|p| bool::from(p.token.as_bytes().ct_eq(token.as_bytes())));
        let Some(i) = found else {
            return Err(TokenError::Invalid);
        };
        let p = self.pending.swap_remove(i);
        if p.device_id != device_id {
            return Err(TokenError::Invalid);
        }
        if p.expires <= now {
            return Err(TokenError::Expired);
        }
        Ok(p.value)
    }

    /// Drop every token whose value matches (stream stopped before it was opened).
    pub fn revoke_where(&mut self, mut f: impl FnMut(&T) -> bool) {
        self.pending.retain(|p| !f(&p.value));
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

// ---- coordinates ----

/// `ma{x,y}` (0..1, clamped) → virtual-desktop pixel inside `rect`.
pub fn normalized_to_desktop(nx: f64, ny: f64, rect: Rect) -> (i32, i32) {
    let axis = |n: f64, origin: i32, size: u32| -> i32 {
        let n = if n.is_finite() { n.clamp(0.0, 1.0) } else { 0.0 };
        origin + (n * size.saturating_sub(1) as f64).round() as i32
    };
    (axis(nx, rect.x, rect.width), axis(ny, rect.y, rect.height))
}

/// G3: a point normalised to the encoded **picture** (0..1, clamped) → virtual-desktop pixel,
/// undoing the letterbox (inverse of [`desktop_to_stream`]). Points on the black bars clamp
/// to the nearest source edge. Used for window streams, whose aspect ratio changes when the
/// window is resized while the encoded size stays fixed.
pub fn picture_to_desktop(nx: f64, ny: f64, rect: Rect, width: u32, height: u32) -> (i32, i32) {
    if rect.width == 0 || rect.height == 0 || width == 0 || height == 0 {
        return normalized_to_desktop(nx, ny, rect);
    }
    let (sw, sh, dw, dh) = (
        rect.width as f64,
        rect.height as f64,
        width as f64,
        height as f64,
    );
    let scale = f64::min(dw / sw, dh / sh);
    let (left, top) = ((dw - sw * scale) / 2.0, (dh - sh * scale) / 2.0);
    let axis = |n: f64, offset: f64, d: f64, s: f64, origin: i32| -> i32 {
        let n = if n.is_finite() { n.clamp(0.0, 1.0) } else { 0.0 };
        let src = ((n * d - offset) / scale).floor().clamp(0.0, s - 1.0);
        origin + src as i32
    };
    (
        axis(nx, left, dw, sw, rect.x),
        axis(ny, top, dh, sh, rect.y),
    )
}

/// Virtual-desktop pixel → stream picture pixel (`width × height`, source letterboxed and
/// centred like the encoder does). The bool says whether the point is on the source.
pub fn desktop_to_stream(x: i32, y: i32, rect: Rect, width: u32, height: u32) -> (i32, i32, bool) {
    if rect.width == 0 || rect.height == 0 {
        return (0, 0, false);
    }
    let (sw, sh, dw, dh) = (
        rect.width as f64,
        rect.height as f64,
        width as f64,
        height as f64,
    );
    let scale = f64::min(dw / sw, dh / sh);
    let (cw, ch) = (sw * scale, sh * scale);
    let (left, top) = ((dw - cw) / 2.0, (dh - ch) / 2.0);
    let (rx, ry) = ((x - rect.x) as f64, (y - rect.y) as f64);
    let inside = rx >= 0.0 && ry >= 0.0 && rx < sw && ry < sh;
    (
        (left + rx * scale).floor() as i32,
        (top + ry * scale).floor() as i32,
        inside,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_bytes_are_kind_pts_be_payload() {
        let f = encode_frame(KIND_VIDEO_KEY, 0x0102_0304_0506_0708, &[0xAA, 0xBB]);
        assert_eq!(f, vec![0x02, 1, 2, 3, 4, 5, 6, 7, 8, 0xAA, 0xBB]);
        let c = cursor_frame(5, 10, -3, true, 7);
        assert_eq!(c[0], KIND_CURSOR);
        assert_eq!(&c[1..9], &5u64.to_be_bytes());
        let v: serde_json::Value = serde_json::from_slice(&c[9..]).unwrap();
        assert_eq!(v, json!({"x":10,"y":-3,"visible":true,"shape_id":7}));
        let s = cursor_shape_frame(9, 0x0A0B0C0D, 32, 48, 3, 4, b"PNG");
        assert_eq!(
            s,
            vec![
                0x05, 0, 0, 0, 0, 0, 0, 0, 9, 0x0A, 0x0B, 0x0C, 0x0D, 0, 32, 0, 48, 0, 3, 0, 4,
                b'P', b'N', b'G'
            ]
        );
    }

    fn v(n: u8) -> Vec<u8> {
        vec![n]
    }

    fn drain(q: &mut StreamQueue) -> Vec<(Vec<u8>, bool)> {
        std::iter::from_fn(|| q.pop().map(|p| (p.frame, p.request_idr))).collect()
    }

    #[test]
    fn backpressure_drops_non_key_until_keyframe_and_requests_idr_once_drained() {
        let mut q = StreamQueue::default();
        assert!(q.push_video(v(0), true));
        assert!(q.push_video(v(1), false));
        assert!(q.push_video(v(2), false));
        // Full: this P frame is dropped, and so are the following ones (broken references).
        assert!(!q.push_video(v(3), false));
        assert_eq!(q.video_len(), 3);
        let first = q.pop().unwrap();
        assert_eq!(first, Popped { frame: v(0), request_idr: false });
        assert!(!q.push_video(v(4), false), "still waiting for a keyframe");
        assert_eq!(q.dropped_video, 2);
        // The queue drains: the last pop carries the IDR request, exactly once.
        assert_eq!(drain(&mut q), vec![(v(1), false), (v(2), true)]);
        assert!(q.pop().is_none());
        assert!(!q.push_video(v(5), false));
        assert!(q.pop().is_none());
        // The keyframe gets in and normal flow resumes.
        assert!(q.push_video(v(6), true));
        assert!(q.push_video(v(7), false));
        assert_eq!(drain(&mut q), vec![(v(6), false), (v(7), false)]);
    }

    #[test]
    fn keyframe_replaces_a_full_queue() {
        let mut q = StreamQueue::default();
        for i in 0..3 {
            q.push_video(v(i), i == 0);
        }
        assert!(q.push_video(v(9), true));
        assert_eq!(q.dropped_video, 3);
        assert_eq!(drain(&mut q), vec![(v(9), false)]);
    }

    #[test]
    fn pop_order_and_cursor_coalescing() {
        let mut q = StreamQueue::default();
        q.push_video(v(1), true);
        q.push_cursor(v(20));
        q.push_cursor(v(21));
        q.push_audio(v(30));
        q.push_cursor_shape(v(40));
        let order: Vec<_> = drain(&mut q).into_iter().map(|(f, _)| f[0]).collect();
        assert_eq!(order, vec![40, 30, 21, 1]);
        for i in 0..30 {
            q.push_audio(v(i));
        }
        assert_eq!(q.dropped_audio, 5);
        assert_eq!(q.pop().unwrap().frame, v(5), "oldest audio dropped");
    }

    #[test]
    fn viewer_reset_waits_for_keyframe() {
        let mut q = StreamQueue::default();
        q.push_video(v(1), true);
        q.reset_for_viewer();
        assert!(!q.push_video(v(2), false));
        assert!(q.pop().is_none());
        assert!(q.push_video(v(3), true));
        assert_eq!(q.pop().unwrap().frame, v(3));
    }

    #[test]
    fn abr_drops_20_percent_on_trouble_and_raises_10_percent_after_5s() {
        let t0 = Instant::now();
        let s = |n: u64| t0 + Duration::from_secs(n);
        let mut a = Abr::new(10_000, t0);
        assert_eq!(a.on_stats(0, 0, 0, s(1)), None, "healthy, at ceiling");
        assert_eq!(a.on_stats(3, 0, 0, s(2)), Some(8_000), "phone dropped frames");
        assert_eq!(a.on_stats(0, 2, 0, s(3)), Some(6_400), "backlog grew 0 -> 2");
        assert_eq!(a.on_stats(0, 2, 0, s(4)), None, "backlog steady");
        assert_eq!(a.on_stats(0, 0, 4, s(5)), Some(5_120), "agent dropped frames");
        assert_eq!(a.on_stats(0, 0, 4, s(9)), None, "only 4 s healthy");
        assert_eq!(a.on_stats(0, 0, 4, s(10)), Some(5_632), "5 s healthy: +10 %");
        assert_eq!(a.on_stats(0, 0, 4, s(12)), None);
        assert_eq!(a.on_stats(0, 0, 4, s(15)), Some(6_196));
        // Never above the requested rate, never below the floor.
        let mut b = Abr::new(1_000, t0);
        b.current = 990;
        assert_eq!(b.on_stats(0, 0, 0, s(6)), Some(1_000));
        assert_eq!(b.on_stats(0, 0, 0, s(12)), None);
        for i in 0..20 {
            b.on_stats(1, 0, 0, s(13 + i));
        }
        assert_eq!(b.current, 500);
        b.set_target(300, s(40));
        assert_eq!(b.on_stats(1, 0, 0, s(41)), None, "floor = requested rate when lower");
        assert_eq!(b.current, 300);
    }

    #[test]
    fn clock_aligns_sources_on_delivery_and_keeps_video_increasing() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut c = StreamClock::new(t0);
        assert_eq!(c.video_pts(0, ms(100)), 100_000);
        assert_eq!(c.video_pts(16_666, ms(130)), 116_666, "source spacing kept");
        assert_eq!(c.audio_pts(5_000_000, ms(120)), 120_000);
        assert_eq!(c.audio_pts(5_020_000, ms(200)), 140_000);
        c.reset_video(); // encoder restarted at pts 0
        assert_eq!(c.video_pts(0, ms(110)), 116_667, "never goes backwards");
        assert_eq!(c.video_pts(33_333, ms(150)), 143_333);
    }

    #[test]
    fn tokens_are_single_use_device_bound_and_expire() {
        let t0 = Instant::now();
        let mut s: TokenStore<u32> = TokenStore::new(Duration::from_secs(30));
        let a = s.issue("dev-a", 1, t0);
        assert_eq!(a.len(), 43);
        assert_eq!(s.take("nope", "dev-a", t0), Err(TokenError::Invalid));
        assert_eq!(s.take(&a, "dev-b", t0), Err(TokenError::Invalid), "other device");
        assert_eq!(s.take(&a, "dev-a", t0), Err(TokenError::Invalid), "burnt by the wrong device");
        let b = s.issue("dev-a", 2, t0);
        assert_eq!(s.take(&b, "dev-a", t0 + Duration::from_secs(29)), Ok(2));
        assert_eq!(s.take(&b, "dev-a", t0), Err(TokenError::Invalid), "single use");
        let c = s.issue("dev-a", 3, t0);
        assert_eq!(s.take(&c, "dev-a", t0 + Duration::from_secs(30)), Err(TokenError::Expired));
        let d = s.issue("dev-a", 4, t0);
        s.revoke_where(|v| *v == 4);
        assert_eq!(s.take(&d, "dev-a", t0), Err(TokenError::Invalid));
        assert!(s.is_empty());
    }

    #[test]
    fn coordinate_mapping() {
        let r = Rect {
            x: -1920,
            y: 0,
            width: 1920,
            height: 1080,
        };
        assert_eq!(normalized_to_desktop(0.0, 0.0, r), (-1920, 0));
        assert_eq!(normalized_to_desktop(1.0, 1.0, r), (-1, 1079));
        assert_eq!(normalized_to_desktop(0.5, 2.0, r), (-960, 1079));
        assert_eq!(normalized_to_desktop(f64::NAN, -1.0, r), (-1920, 0));
        // Same aspect, scaled 2/3.
        assert_eq!(desktop_to_stream(-960, 540, r, 1280, 720), (640, 360, true));
        assert!(!desktop_to_stream(10, 10, r, 1280, 720).2);
        // 16:9 source in a 4:3 picture: letterboxed top/bottom.
        let (x, y, inside) = desktop_to_stream(-1920, 0, r, 1024, 768);
        assert_eq!((x, y, inside), (0, 96, true));
    }

    #[test]
    fn picture_to_desktop_undoes_the_letterbox() {
        // Window 800x800 shown in a 1280x720 picture: pillarbox 280 px each side, scale 0.9.
        let r = Rect { x: 100, y: 50, width: 800, height: 800 };
        assert_eq!(picture_to_desktop(0.5, 0.5, r, 1280, 720), (500, 450));
        assert_eq!(picture_to_desktop(0.0, 0.0, r, 1280, 720), (100, 50)); // on the bar → edge
        assert_eq!(picture_to_desktop(1.0, 1.0, r, 1280, 720), (899, 849));
        // Left edge of the picture content (x = 280 px) is the window's left edge.
        assert_eq!(picture_to_desktop(280.0 / 1280.0, 0.0, r, 1280, 720).0, 100);
        // Round trip with desktop_to_stream for points on the source.
        for (x, y) in [(100, 50), (333, 777), (899, 849), (500, 450)] {
            let (px, py, inside) = desktop_to_stream(x, y, r, 1280, 720);
            assert!(inside);
            let (bx, by) =
                picture_to_desktop((px as f64 + 0.5) / 1280.0, (py as f64 + 0.5) / 720.0, r, 1280, 720);
            assert!((bx - x).abs() <= 1 && (by - y).abs() <= 1, "({x},{y}) -> ({px},{py}) -> ({bx},{by})");
        }
        // Same aspect = plain mapping; degenerate sizes fall back; NaN clamps to 0.
        let m = Rect { x: -1920, y: 0, width: 1920, height: 1080 };
        assert_eq!(picture_to_desktop(0.5, 0.5, m, 1280, 720), (-960, 540));
        assert_eq!(picture_to_desktop(f64::NAN, 2.0, m, 1280, 720), (-1920, 1079));
        assert_eq!(picture_to_desktop(0.5, 0.5, m, 0, 0), normalized_to_desktop(0.5, 0.5, m));
    }
}
