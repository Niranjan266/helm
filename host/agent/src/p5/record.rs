//! `record.start` / `record.stop` (PROTOCOL §13): MP4 of the screen, optionally with system
//! audio. Taps the session's running stream encoder, or starts a private encoder when the
//! phone is not streaming.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Instant;

use crate::platform::{
    AudioStream, RecordConfig, RecordedFile, Recorder, VideoParams, VideoSource, VideoStream,
    blocking,
};
use crate::router::RpcError;
use crate::server::AppState;
use crate::session::Session;
use crate::stream::{ActiveStream, StreamClock};

use super::lock;

/// Private encoder settings when nothing is streaming.
pub const OWN_MAX_WIDTH: u32 = 1920;
pub const OWN_MAX_HEIGHT: u32 = 1080;
pub const OWN_FPS: u32 = 30;
pub const OWN_BITRATE_KBPS: u32 = 8_000;

struct Clock {
    clock: StreamClock,
    last_video_src: Option<u64>,
}

/// One running recording (at most one per control session).
pub struct Recording {
    pub session_id: u64,
    pub path: PathBuf,
    recorder: Arc<dyn Recorder>,
    clock: Mutex<Clock>,
    own_video: Mutex<Option<Box<dyn VideoStream>>>,
    own_audio: Mutex<Option<Box<dyn AudioStream>>>,
    tapped: Mutex<Option<Weak<ActiveStream>>>,
    /// Audio comes from the tapped stream (not from `own_audio`).
    audio_from_stream: AtomicBool,
    stopped: AtomicBool,
}

impl std::fmt::Debug for Recording {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Recording")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl Recording {
    /// An encoded access unit (source timestamps; mapped onto one clock with the audio).
    pub fn on_video(&self, au: &[u8], key: bool, src_pts: u64) {
        if self.stopped.load(Ordering::SeqCst) {
            return;
        }
        let pts = {
            let mut c = lock(&self.clock);
            // The encoder restarted (stream.config fps change): re-anchor.
            if c.last_video_src.is_some_and(|last| src_pts < last) {
                c.clock.reset_video();
            }
            c.last_video_src = Some(src_pts);
            c.clock.video_pts(src_pts, Instant::now())
        };
        self.recorder.push_video(au, key, pts);
    }

    /// An Opus packet from the system-audio capture.
    pub fn on_audio(&self, packet: &[u8], src_pts: u64) {
        if self.stopped.load(Ordering::SeqCst) {
            return;
        }
        let pts = lock(&self.clock).clock.audio_pts(src_pts, Instant::now());
        self.recorder.push_audio_opus(packet, pts);
    }

    /// Audio packets of the tapped stream are wanted only when the recording has no own
    /// audio capture.
    pub fn wants_stream_audio(&self) -> bool {
        self.audio_from_stream.load(Ordering::SeqCst)
    }
}

/// `Videos\Helm\helm-YYYYMMDD-HHMMSS.mp4` in local time (with `-N` when the name is taken).
pub fn next_path(dir: &std::path::Path, utc_offset_minutes: i32) -> PathBuf {
    let offset = time::UtcOffset::from_whole_seconds(utc_offset_minutes * 60)
        .unwrap_or(time::UtcOffset::UTC);
    let t = time::OffsetDateTime::now_utc().to_offset(offset);
    let stamp = format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second()
    );
    let mut path = dir.join(format!("helm-{stamp}.mp4"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("helm-{stamp}-{n}.mp4"));
        n += 1;
    }
    path
}

/// Start recording for `session`. Returns the file path.
pub async fn start(
    state: &Arc<AppState>,
    session: &Arc<Session>,
    want_audio: bool,
) -> Result<String, RpcError> {
    if lock(&state.p5.recordings).contains_key(&session.id) {
        return Err(RpcError::bad_params(
            "a recording is already running (call record.stop)",
        ));
    }
    let dir = state.p5.videos_dir.clone();
    std::fs::create_dir_all(&dir)
        .map_err(|e| RpcError::internal(format!("cannot create {}: {e}", dir.display())))?;
    let platform = state.platform.clone();
    let path = next_path(&dir, platform.utc_offset_minutes());
    // Frames that arrive before the recording exists are dropped (it starts at a keyframe
    // that is requested once everything is wired up).
    let slot: Arc<OnceLock<Weak<Recording>>> = Arc::new(OnceLock::new());

    let stream = session.stream().filter(|s| !s.is_stopped());
    let (info, own_video) = match &stream {
        Some(s) => (s.info(), None),
        None => {
            let params = VideoParams {
                source: VideoSource::Monitor(None),
                max_width: OWN_MAX_WIDTH,
                max_height: OWN_MAX_HEIGHT,
                fps: OWN_FPS,
                bitrate_kbps: OWN_BITRATE_KBPS,
            };
            let sink_slot = slot.clone();
            let p = platform.clone();
            let video = blocking(move || {
                p.start_video(
                    params,
                    Box::new(move |au: &[u8], key: bool, pts: u64| {
                        if let Some(r) = sink_slot.get().and_then(Weak::upgrade) {
                            r.on_video(au, key, pts);
                        }
                    }),
                )
            })
            .await?;
            (video.info(), Some(video))
        }
    };

    let stream_audio = want_audio && stream.as_ref().is_some_and(|s| s.has_audio());
    let mut own_audio = None;
    if want_audio && !stream_audio {
        let sink_slot = slot.clone();
        let p = platform.clone();
        match blocking(move || {
            p.start_audio(Box::new(move |packet: &[u8], pts: u64| {
                if let Some(r) = sink_slot.get().and_then(Weak::upgrade) {
                    r.on_audio(packet, pts);
                }
            }))
        })
        .await
        {
            Ok(a) => own_audio = Some(a),
            Err(e) => tracing::info!("recording without audio: {}", e.message),
        }
    }
    let audio = stream_audio || own_audio.is_some();

    let config = RecordConfig {
        width: info.width,
        height: info.height,
        fps: info.fps,
        audio,
    };
    let p = platform.clone();
    let file = path.clone();
    let recorder = match blocking(move || p.start_recorder(&file, config)).await {
        Ok(r) => r,
        Err(e) => {
            dispose(own_video, own_audio).await;
            return Err(e);
        }
    };
    let rec = Arc::new(Recording {
        session_id: session.id,
        path: path.clone(),
        recorder,
        clock: Mutex::new(Clock {
            clock: StreamClock::new(Instant::now()),
            last_video_src: None,
        }),
        own_video: Mutex::new(None),
        own_audio: Mutex::new(own_audio),
        tapped: Mutex::new(stream.as_ref().map(Arc::downgrade)),
        audio_from_stream: AtomicBool::new(stream_audio),
        stopped: AtomicBool::new(false),
    });
    let _ = slot.set(Arc::downgrade(&rec));
    match (&stream, own_video) {
        (Some(s), _) => {
            s.set_record_tap(Some(rec.clone()));
            s.request_keyframe();
        }
        (None, Some(v)) => {
            v.request_keyframe();
            *lock(&rec.own_video) = Some(v);
        }
        (None, None) => {}
    }
    let raced = {
        match lock(&state.p5.recordings).entry(session.id) {
            std::collections::hash_map::Entry::Occupied(_) => Some(rec),
            std::collections::hash_map::Entry::Vacant(v) => {
                v.insert(rec);
                None
            }
        }
    };
    if let Some(rec) = raced {
        finish(rec).await.ok();
        return Err(RpcError::bad_params("a recording is already running"));
    }
    tracing::info!("recording to {}", path.display());
    Ok(path.to_string_lossy().into_owned())
}

/// Stop the session's recording and finalize the file.
pub async fn stop(state: &Arc<AppState>, session_id: u64) -> Result<RecordedFile, RpcError> {
    let rec = lock(&state.p5.recordings)
        .remove(&session_id)
        .ok_or_else(|| RpcError::bad_params("no recording is running (call record.start)"))?;
    finish(rec).await
}

async fn finish(rec: Arc<Recording>) -> Result<RecordedFile, RpcError> {
    rec.stopped.store(true, Ordering::SeqCst);
    if let Some(s) = lock(&rec.tapped).take().and_then(|w| w.upgrade()) {
        s.clear_record_tap(&rec);
    }
    let video = lock(&rec.own_video).take();
    let audio = lock(&rec.own_audio).take();
    dispose(video, audio).await;
    let recorder = rec.recorder.clone();
    let done = blocking(move || recorder.stop()).await;
    match &done {
        Ok(f) => tracing::info!("recording saved: {} ({} ms)", f.path, f.duration_ms),
        Err(e) => tracing::warn!("recording failed: {}", e.message),
    }
    done
}

async fn dispose(video: Option<Box<dyn VideoStream>>, audio: Option<Box<dyn AudioStream>>) {
    if video.is_none() && audio.is_none() {
        return;
    }
    let _ = tokio::task::spawn_blocking(move || {
        drop(video);
        drop(audio);
    })
    .await;
}
