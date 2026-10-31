//! A running stream: encoder + optional audio + cursor poller → [`StreamQueue`] → `/stream`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::sync::Notify;

use super::{
    Abr, KIND_AUDIO, KIND_VIDEO, KIND_VIDEO_KEY, StreamClock, StreamQueue, cursor_frame,
    cursor_shape_frame, desktop_to_stream, encode_frame,
};
use crate::platform::{
    AudioStream, Platform, Rect, VideoInfo, VideoParams, VideoStream, blocking,
};
use crate::router::RpcError;
use crate::session::Session;

/// Cursor poll interval (~60 Hz).
const CURSOR_POLL: Duration = Duration::from_millis(16);
/// How often the source rectangle is re-read for window streams.
const RECT_REFRESH: Duration = Duration::from_millis(250);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

pub struct ActiveStream {
    pub id: u64,
    pub device_id: String,
    platform: Arc<dyn Platform>,
    session: Weak<Session>,
    queue: Mutex<StreamQueue>,
    notify: Notify,
    video: Mutex<Option<Box<dyn VideoStream>>>,
    audio: Mutex<Option<Box<dyn AudioStream>>>,
    params: Mutex<VideoParams>,
    info: Mutex<VideoInfo>,
    rect: Mutex<Option<Rect>>,
    clock: Mutex<StreamClock>,
    abr: Mutex<Abr>,
    started: Instant,
    attached: AtomicBool,
    stopped: AtomicBool,
    cursor_stop: Mutex<Option<std_mpsc::Sender<()>>>,
    /// Phase 5: a recording fed from this stream's encoder.
    record_tap: Mutex<Option<Arc<crate::p5::record::Recording>>>,
}

impl std::fmt::Debug for ActiveStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActiveStream").field("id", &self.id).finish()
    }
}

impl ActiveStream {
    /// Start capture + encoding (and audio when asked). Frames are discarded until a
    /// `/stream` socket attaches. Returns the stream and, when audio runs, its Opus pre-skip.
    pub async fn start(
        id: u64,
        platform: Arc<dyn Platform>,
        session: &Arc<Session>,
        params: VideoParams,
        want_audio: bool,
    ) -> Result<(Arc<ActiveStream>, Option<u32>), RpcError> {
        let now = Instant::now();
        let stream = Arc::new(ActiveStream {
            id,
            device_id: session.device_id.clone(),
            platform: platform.clone(),
            session: Arc::downgrade(session),
            queue: Mutex::new(StreamQueue::default()),
            notify: Notify::new(),
            video: Mutex::new(None),
            audio: Mutex::new(None),
            params: Mutex::new(params.clone()),
            info: Mutex::new(VideoInfo {
                width: 0,
                height: 0,
                fps: params.fps,
                hardware: false,
                encoder_name: String::new(),
            }),
            rect: Mutex::new(None),
            clock: Mutex::new(StreamClock::new(now)),
            abr: Mutex::new(Abr::new(params.bitrate_kbps, now)),
            started: now,
            attached: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            cursor_stop: Mutex::new(None),
            record_tap: Mutex::new(None),
        });
        stream.start_video(params).await?;
        let mut audio = None;
        if want_audio {
            let weak = Arc::downgrade(&stream);
            let p = platform.clone();
            let started = blocking(move || {
                p.start_audio(Box::new(move |packet: &[u8], pts: u64| {
                    if let Some(s) = weak.upgrade() {
                        s.on_audio(packet, pts);
                    }
                }))
            })
            .await;
            match started {
                Ok(a) => {
                    audio = Some(a.pre_skip());
                    *lock(&stream.audio) = Some(a);
                }
                Err(e) => tracing::info!("stream {id}: no audio ({})", e.message),
            }
        }
        Ok((stream, audio))
    }

    async fn start_video(self: &Arc<Self>, params: VideoParams) -> Result<(), RpcError> {
        let weak = Arc::downgrade(self);
        let p = self.platform.clone();
        let video = blocking(move || {
            p.start_video(
                params,
                Box::new(move |au: &[u8], key: bool, pts: u64| {
                    if let Some(s) = weak.upgrade() {
                        s.on_video(au, key, pts);
                    }
                }),
            )
        })
        .await?;
        *lock(&self.info) = video.info();
        *lock(&self.rect) = video.source_rect();
        if self.stopped.load(Ordering::SeqCst) {
            dispose(Some(video), None);
            return Err(RpcError::internal("stream stopped while starting"));
        }
        *lock(&self.video) = Some(video);
        Ok(())
    }

    pub fn info(&self) -> VideoInfo {
        lock(&self.info).clone()
    }

    pub fn bitrate_kbps(&self) -> u32 {
        lock(&self.abr).current
    }

    pub fn is_attached(&self) -> bool {
        self.attached.load(Ordering::SeqCst)
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    pub fn age(&self) -> Duration {
        self.started.elapsed()
    }

    /// Where the streamed source is on the desktop (for `ma` and the cursor).
    pub fn source_rect(&self) -> Option<Rect> {
        *lock(&self.rect)
    }

    /// `(running, ended_reason)` of the encoder.
    pub fn video_state(&self) -> (bool, Option<String>) {
        match lock(&self.video).as_ref() {
            Some(v) => (v.is_running(), v.ended_reason()),
            None => (true, None), // between an fps restart's stop and start
        }
    }

    fn on_video(&self, au: &[u8], key: bool, pts: u64) {
        let tap = lock(&self.record_tap).clone();
        if let Some(rec) = tap {
            rec.on_video(au, key, pts);
        }
        if !self.is_attached() || self.is_stopped() {
            return;
        }
        let pts = lock(&self.clock).video_pts(pts, Instant::now());
        let kind = if key { KIND_VIDEO_KEY } else { KIND_VIDEO };
        if lock(&self.queue).push_video(encode_frame(kind, pts, au), key) {
            self.notify.notify_one();
        }
    }

    fn on_audio(&self, packet: &[u8], pts: u64) {
        let tap = lock(&self.record_tap).clone();
        if let Some(rec) = tap.filter(|r| r.wants_stream_audio()) {
            rec.on_audio(packet, pts);
        }
        if !self.is_attached() || self.is_stopped() {
            return;
        }
        let pts = lock(&self.clock).audio_pts(pts, Instant::now());
        lock(&self.queue).push_audio(encode_frame(KIND_AUDIO, pts, packet));
        self.notify.notify_one();
    }

    /// Feed encoded video (and audio) to a recording too.
    pub fn set_record_tap(&self, rec: Option<Arc<crate::p5::record::Recording>>) {
        *lock(&self.record_tap) = rec;
    }

    /// Remove `rec` if it is the current tap.
    pub fn clear_record_tap(&self, rec: &Arc<crate::p5::record::Recording>) {
        let mut tap = lock(&self.record_tap);
        if tap.as_ref().is_some_and(|t| Arc::ptr_eq(t, rec)) {
            *tap = None;
        }
    }

    /// System audio runs with this stream.
    pub fn has_audio(&self) -> bool {
        lock(&self.audio).is_some()
    }

    pub fn request_keyframe(&self) {
        if let Some(v) = lock(&self.video).as_ref() {
            v.request_keyframe();
        }
    }

    /// The `/stream` socket is open: start sending (from a keyframe) and poll the cursor.
    pub fn attach(self: &Arc<Self>) {
        if self.attached.swap(true, Ordering::SeqCst) {
            return;
        }
        lock(&self.queue).reset_for_viewer();
        self.request_keyframe();
        let (tx, rx) = std_mpsc::channel::<()>();
        *lock(&self.cursor_stop) = Some(tx);
        let weak = Arc::downgrade(self);
        let spawned = std::thread::Builder::new()
            .name(format!("helm-cursor-{}", self.id))
            .spawn(move || cursor_loop(weak, rx));
        if let Err(e) = spawned {
            tracing::warn!("cursor thread failed to start: {e}");
        }
    }

    /// Next frame for the socket; `None` once the stream is stopped.
    pub async fn next_frame(&self) -> Option<Vec<u8>> {
        loop {
            let popped = lock(&self.queue).pop();
            if let Some(p) = popped {
                if p.request_idr {
                    self.request_keyframe();
                }
                return Some(p.frame);
            }
            if self.is_stopped() {
                return None;
            }
            self.notify.notified().await;
        }
    }

    /// Stop everything. Idempotent; never blocks (the encoder is joined on another thread).
    pub fn stop(&self) {
        if self.stopped.swap(true, Ordering::SeqCst) {
            return;
        }
        drop(lock(&self.cursor_stop).take());
        let video = lock(&self.video).take();
        let audio = lock(&self.audio).take();
        dispose(video, audio);
        self.notify.notify_one();
    }

    /// `stream.stats` from the phone: run the adaptive-bitrate rules.
    pub fn on_stats(&self, dropped: u64) -> Value {
        let (backlog, agent_drops) = {
            let q = lock(&self.queue);
            (q.video_len(), q.dropped_video)
        };
        let change = lock(&self.abr).on_stats(dropped, backlog, agent_drops, Instant::now());
        if let Some(kbps) = change
            && let Some(v) = lock(&self.video).as_ref()
        {
            v.set_bitrate(kbps);
        }
        json!({"bitrate_kbps": self.bitrate_kbps()})
    }

    /// `stream.config{bitrate_kbps?, fps?}`. An fps change restarts the encoder.
    pub async fn reconfigure(
        self: &Arc<Self>,
        bitrate_kbps: Option<u32>,
        fps: Option<u32>,
    ) -> Result<Value, RpcError> {
        if let Some(kbps) = bitrate_kbps {
            lock(&self.abr).set_target(kbps, Instant::now());
            lock(&self.params).bitrate_kbps = kbps;
            if let Some(v) = lock(&self.video).as_ref() {
                v.set_bitrate(kbps);
            }
        }
        if let Some(fps) = fps
            && fps != lock(&self.params).fps
        {
            let params = {
                let mut p = lock(&self.params);
                p.fps = fps;
                p.bitrate_kbps = lock(&self.abr).current;
                p.clone()
            };
            let old = lock(&self.video).take();
            if let Some(old) = old {
                let _ = tokio::task::spawn_blocking(move || drop(old)).await;
            }
            lock(&self.clock).reset_video();
            lock(&self.queue).reset_for_viewer();
            if let Err(e) = self.start_video(params).await {
                self.stop();
                return Err(e);
            }
        }
        let info = self.info();
        Ok(json!({
            "bitrate_kbps": self.bitrate_kbps(),
            "fps": info.fps,
            "width": info.width,
            "height": info.height,
        }))
    }

    /// The stream ended by itself: tell the phone and clean up.
    pub fn end(&self, reason: &str) {
        if self.is_stopped() {
            return;
        }
        tracing::info!("stream {} ended: {reason}", self.id);
        self.stop();
        if let Some(session) = self.session.upgrade() {
            session.clear_stream(self.id);
            session.send_event("stream.ended", json!({"reason": reason}));
        }
    }
}

impl Drop for ActiveStream {
    fn drop(&mut self) {
        // The last reference may be released on the encoder's own thread: never join here.
        let video = self.video.get_mut().ok().and_then(|v| v.take());
        let audio = self.audio.get_mut().ok().and_then(|a| a.take());
        if video.is_some() || audio.is_some() {
            dispose(video, audio);
        }
    }
}

/// Drop encoder / audio on a helper thread (their `Drop` joins worker threads).
fn dispose(video: Option<Box<dyn VideoStream>>, audio: Option<Box<dyn AudioStream>>) {
    if video.is_none() && audio.is_none() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("helm-stream-stop".into())
        .spawn(move || {
            drop(audio);
            drop(video);
        });
    if let Err(e) = spawned {
        tracing::warn!("could not stop stream cleanly: {e}");
    }
}

/// Poll the pointer at ~60 Hz; send its image once per new cursor and its position when it
/// changes, in stream pixel coordinates.
fn cursor_loop(weak: Weak<ActiveStream>, stop: std_mpsc::Receiver<()>) {
    let mut shapes: HashMap<u64, u32> = HashMap::new();
    let mut last: Option<(i32, i32, bool, u32)> = None;
    let mut last_rect_read = Instant::now();
    loop {
        match stop.recv_timeout(CURSOR_POLL) {
            Err(std_mpsc::RecvTimeoutError::Timeout) => {}
            _ => return,
        }
        let Some(s) = weak.upgrade() else { return };
        if s.is_stopped() {
            return;
        }
        let Ok(cur) = s.platform.cursor() else {
            continue;
        };
        if matches!(lock(&s.params).source, crate::platform::VideoSource::Window(_))
            && last_rect_read.elapsed() >= RECT_REFRESH
        {
            last_rect_read = Instant::now();
            let rect = lock(&s.video).as_ref().and_then(|v| v.source_rect());
            if rect.is_some() {
                *lock(&s.rect) = rect;
            }
        }
        let Some(rect) = s.source_rect() else {
            continue;
        };
        let info = s.info();
        let now = lock(&s.clock).now_us(Instant::now());
        let shape_id = if cur.handle == 0 {
            0
        } else if let Some(&id) = shapes.get(&cur.handle) {
            id
        } else {
            let id = shapes.len() as u32 + 1;
            shapes.insert(cur.handle, id);
            if let Ok(Some(shape)) = s.platform.cursor_shape(cur.handle) {
                lock(&s.queue).push_cursor_shape(cursor_shape_frame(
                    now,
                    id,
                    shape.width,
                    shape.height,
                    shape.hot_x,
                    shape.hot_y,
                    &shape.png,
                ));
            }
            id
        };
        let (x, y, inside) = desktop_to_stream(cur.x, cur.y, rect, info.width, info.height);
        let state = (x, y, cur.visible && inside, shape_id);
        if last != Some(state) {
            last = Some(state);
            lock(&s.queue).push_cursor(cursor_frame(now, x, y, state.2, shape_id));
            s.notify.notify_one();
        }
    }
}
