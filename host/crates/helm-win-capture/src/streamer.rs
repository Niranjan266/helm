//! The live-streaming pipeline: capture → NV12 (GPU) → H.264 → callback.
//!
//! Threads:
//! * **caller** — `Streamer::start` spawns the pipeline thread and waits until it is either
//!   running or has failed, so configuration errors are returned synchronously.
//! * **capture callbacks** (system thread pool, free-threaded frame pool) — pace frames to the
//!   target fps, convert BGRA → NV12 into a free slot of a small GPU texture ring and post
//!   `Frame{slot}` to the pipeline thread. They never block on the encoder: when no slot is
//!   free the frame is dropped.
//! * **pipeline thread** (COM MTA + Media Foundation) — owns the encoder MFT. Waits on one
//!   channel for frames, MFT events (async hardware encoders), keyframe requests and stop.
//!   Always encodes the *newest* frame (older queued frames are dropped), repeats the last frame
//!   when the screen is idle, and calls `on_access_unit` for every encoded access unit.
//! * **MF work-queue threads** — run the `IMFAsyncCallback` that forwards encoder events.

use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::Win32::Graphics::Direct3D11::{ID3D11Texture2D, ID3D11VideoProcessorOutputView};
use windows::Win32::Media::MediaFoundation::{
    IMFAsyncCallback, METransformHaveOutput, METransformNeedInput,
};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::core::HRESULT;

use crate::capture::{self, CaptureSession, CapturedFrame};
use crate::config::{StreamConfig, fit_output_size};
use crate::convert::Nv12Converter;
use crate::d3d::D3dDevice;
use crate::encoder::{
    EVENT_ERROR, EncodedFrame, EncoderSettings, H264Encoder, MfRuntime, OutputResult, create_device_manager,
};
use crate::error::{Error, Result};
use crate::stats::{StatsAccumulator, StreamStats};

/// Number of NV12 textures in the ring between capture and encoder.
const SLOTS: usize = 6;
/// Upper bound on frames the encoder may hold before we consider them lost.
const MAX_IN_FLIGHT: usize = 16;

/// Callback for encoded access units: `(annex_b_bytes, is_keyframe, pts_us)`.
/// Runs on the pipeline thread; keep it short (copy the bytes into a queue / socket buffer).
pub type AccessUnitCallback = Box<dyn FnMut(&[u8], bool, u64) + Send + 'static>;

/// Facts about the running stream, fixed at start.
#[derive(Debug, Clone)]
pub struct StreamInfo {
    pub encoder_name: String,
    pub hardware: bool,
    /// Frames reach the encoder as D3D11 textures (no CPU copy).
    pub gpu_input: bool,
    /// The encoder is an asynchronous (event-driven) MFT.
    pub async_mft: bool,
    pub profile: &'static str,
    pub adapter: String,
    /// Encoded picture size.
    pub width: u32,
    pub height: u32,
    /// Capture source size at start.
    pub source_width: u32,
    pub source_height: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
    /// The yellow capture border is off (Windows 11).
    pub border_disabled: bool,
    /// Windows-side frame rate cap (`MinUpdateInterval`, Windows 11 24H2+) is active.
    pub min_update_interval_set: bool,
    pub codec_settings: Vec<String>,
}

enum Msg {
    Frame { slot: usize, capture_time: i64 },
    Mft { event: u32, status: HRESULT },
    Keyframe,
    Bitrate(u32),
    SourceClosed,
    Stop,
}

fn qpc_freq() -> i64 {
    static F: OnceLock<i64> = OnceLock::new();
    *F.get_or_init(|| {
        let mut f = 0i64;
        // SAFETY: out-pointer to a local.
        let _ = unsafe { QueryPerformanceFrequency(&mut f) };
        f.max(1)
    })
}

/// Current QueryPerformanceCounter time in 100 ns units (same base as `SystemRelativeTime`).
pub(crate) fn qpc_100ns() -> i64 {
    let mut c = 0i64;
    // SAFETY: out-pointer to a local.
    let _ = unsafe { QueryPerformanceCounter(&mut c) };
    (c as i128 * 10_000_000 / qpc_freq() as i128) as i64
}

struct Slot {
    texture: ID3D11Texture2D,
    view: ID3D11VideoProcessorOutputView,
}

#[derive(Default)]
struct SlotTable {
    /// References per slot: capture writer / encoder in-flight samples.
    refs: Vec<u32>,
    /// Slot of the last submitted frame; capture never overwrites it (it is the repeat source).
    last: Option<usize>,
    next: usize,
}

impl SlotTable {
    fn acquire_for_write(&mut self) -> Option<usize> {
        let n = self.refs.len();
        for k in 0..n {
            let i = (self.next + k) % n;
            if self.refs[i] == 0 && self.last != Some(i) {
                self.refs[i] = 1;
                self.next = (i + 1) % n;
                return Some(i);
            }
        }
        None
    }

    fn release(&mut self, i: usize) {
        if let Some(r) = self.refs.get_mut(i) {
            *r = r.saturating_sub(1);
        }
    }
}

/// State shared by capture callbacks and the pipeline thread.
struct Shared {
    slots: Vec<Slot>,
    table: Mutex<SlotTable>,
    converter: Mutex<Nv12Converter>,
    stats: Arc<Mutex<StatsAccumulator>>,
}

// SAFETY: the D3D11 objects live on a multithread-protected device and are free-threaded;
// mutable state is behind Mutexes.
unsafe impl Send for Shared {}
unsafe impl Sync for Shared {}

impl Shared {
    fn release(&self, i: usize) {
        self.table.lock().unwrap_or_else(|p| p.into_inner()).release(i);
    }
}

/// A running H.264 screen stream. Stop it with [`Streamer::stop`] (or drop it).
pub struct Streamer {
    tx: Sender<Msg>,
    thread: Option<JoinHandle<()>>,
    stats: Arc<Mutex<StatsAccumulator>>,
    info: StreamInfo,
}

impl Streamer {
    /// Start streaming. Returns once the encoder is configured and capture has started.
    /// `on_access_unit(bytes, is_keyframe, pts_us)` is called on the pipeline thread.
    pub fn start<F>(config: StreamConfig, on_access_unit: F) -> Result<Streamer>
    where
        F: FnMut(&[u8], bool, u64) + Send + 'static,
    {
        config.validate()?;
        let (tx, rx) = mpsc::channel::<Msg>();
        let (init_tx, init_rx) = mpsc::sync_channel::<Result<StreamInfo>>(1);
        let stats = Arc::new(Mutex::new(StatsAccumulator::default()));
        let thread_tx = tx.clone();
        let thread_stats = stats.clone();
        let cb: AccessUnitCallback = Box::new(on_access_unit);
        let thread = std::thread::Builder::new()
            .name("helm-stream".into())
            .spawn(move || pipeline_thread(config, thread_tx, rx, init_tx, thread_stats, cb))
            .map_err(|e| Error::Stopped(format!("cannot spawn pipeline thread: {e}")))?;
        match init_rx.recv() {
            Ok(Ok(info)) => Ok(Streamer { tx, thread: Some(thread), stats, info }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                let _ = thread.join();
                Err(Error::Stopped("pipeline thread exited during start".into()))
            }
        }
    }

    pub fn info(&self) -> &StreamInfo {
        &self.info
    }

    /// Ask for an IDR frame (with SPS/PPS) as soon as possible, e.g. when a client joins or
    /// reports packet loss.
    pub fn request_keyframe(&self) {
        let _ = self.tx.send(Msg::Keyframe);
    }

    /// Change the CBR bitrate of the running encoder (adaptive bitrate). Clamped to the
    /// `StreamConfig` limits; applied on the pipeline thread before the next frame. Encoders
    /// that reject the dynamic change keep their old rate (see `stats().bitrate_kbps`).
    pub fn set_bitrate(&self, kbps: u32) {
        let kbps = kbps.clamp(crate::config::MIN_BITRATE_KBPS, crate::config::MAX_BITRATE_KBPS);
        let _ = self.tx.send(Msg::Bitrate(kbps));
    }

    /// Current statistics.
    pub fn stats(&self) -> StreamStats {
        self.stats.lock().unwrap_or_else(|p| p.into_inner()).snapshot()
    }

    /// False once the pipeline has stopped by itself (source closed or an error; see
    /// `stats().ended_reason`).
    pub fn is_running(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }

    /// Stop capture and encoding, wait for the pipeline thread, return final statistics.
    pub fn stop(mut self) -> StreamStats {
        self.shutdown();
        self.stats()
    }

    fn shutdown(&mut self) {
        let _ = self.tx.send(Msg::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Streamer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct InFlight {
    sample_time: i64,
    capture_time: i64,
    slot: Option<usize>,
    fresh: bool,
}

struct Pipeline {
    cfg: StreamConfig,
    shared: Arc<Shared>,
    encoder: H264Encoder,
    callback: AccessUnitCallback,
    stats: Arc<Mutex<StatsAccumulator>>,
    /// Frames waiting for `METransformNeedInput` (async encoders).
    pending: VecDeque<(usize, i64, bool)>,
    need_input: u32,
    in_flight: VecDeque<InFlight>,
    t0: Option<i64>,
    last_sample_time: i64,
    last_submit: Option<Instant>,
    last_was_repeat: bool,
    force_key: bool,
}

impl Pipeline {
    fn stats(&self) -> std::sync::MutexGuard<'_, StatsAccumulator> {
        self.stats.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn drop_frame(&mut self, slot: usize) {
        self.shared.release(slot);
        self.stats().frames_dropped += 1;
    }

    /// A frame (fresh or repeat) is ready in `slot` (we hold one reference).
    fn on_frame(&mut self, slot: usize, capture_time: i64, fresh: bool) -> Result<()> {
        if self.encoder.is_async() {
            // Keep only the newest waiting frame: latency over completeness.
            while let Some((old, _, was_fresh)) = self.pending.pop_front() {
                self.shared.release(old);
                if was_fresh {
                    self.stats().frames_dropped += 1;
                }
            }
            self.pending.push_back((slot, capture_time, fresh));
            self.pump()
        } else {
            self.submit(slot, capture_time, fresh)?;
            self.drain_sync()
        }
    }

    fn pump(&mut self) -> Result<()> {
        while self.need_input > 0 {
            let Some((slot, t, fresh)) = self.pending.pop_front() else { break };
            self.need_input -= 1;
            self.submit(slot, t, fresh)?;
        }
        Ok(())
    }

    fn submit(&mut self, slot: usize, capture_time: i64, fresh: bool) -> Result<()> {
        let t0 = *self.t0.get_or_insert(capture_time);
        let sample_time = (capture_time - t0).max(self.last_sample_time + 1);
        if self.force_key {
            self.encoder.force_keyframe();
            self.force_key = false;
        }
        let dur = self.cfg.frame_duration_100ns();
        let texture = self.shared.slots[slot].texture.clone();
        let sample = match self.encoder.make_sample(&texture, sample_time, dur) {
            Ok(s) => s,
            Err(e) => {
                self.shared.release(slot);
                return Err(e);
            }
        };
        // With CPU input the pixels were copied; the texture is free again.
        let held = if self.encoder.gpu_input {
            Some(slot)
        } else {
            self.shared.release(slot);
            None
        };
        let mut accepted = self.encoder.process_input(&sample)?;
        if !accepted && !self.encoder.is_async() {
            self.drain_sync()?;
            accepted = self.encoder.process_input(&sample)?;
        }
        if !accepted {
            if let Some(s) = held {
                self.shared.release(s);
            }
            self.stats().frames_dropped += 1;
            return Ok(());
        }
        self.last_sample_time = sample_time;
        self.last_submit = Some(Instant::now());
        self.last_was_repeat = !fresh;
        {
            let mut st = self.stats();
            st.started.get_or_insert_with(Instant::now);
            if fresh {
                st.frames_captured += 1;
            } else {
                st.frames_repeated += 1;
            }
        }
        self.in_flight.push_back(InFlight { sample_time, capture_time, slot: held, fresh });
        while self.in_flight.len() > MAX_IN_FLIGHT {
            if let Some(f) = self.in_flight.pop_front() {
                if let Some(s) = f.slot {
                    self.shared.release(s);
                }
            }
        }
        Ok(())
    }

    fn drain_sync(&mut self) -> Result<()> {
        loop {
            match self.encoder.process_output()? {
                OutputResult::Frame(f) => self.deliver(f),
                OutputResult::NeedMoreInput => return Ok(()),
                OutputResult::StreamChanged => continue,
            }
        }
    }

    fn deliver(&mut self, f: EncodedFrame) {
        let now = qpc_100ns();
        let mut latency = None;
        while let Some(front) = self.in_flight.front() {
            if front.sample_time > f.sample_time {
                break;
            }
            let e = self.in_flight.pop_front().expect("front exists");
            if let Some(s) = e.slot {
                self.shared.release(s);
            }
            if e.sample_time == f.sample_time && e.fresh {
                latency = Some(now - e.capture_time);
            }
        }
        {
            let mut st = self.stats();
            st.frames_encoded += 1;
            st.bytes += f.data.len() as u64;
            if f.keyframe {
                st.keyframes += 1;
            }
            if let Some(l) = latency {
                // 100 ns → µs; clamp nonsense (clock mismatch) out of the stats.
                if (0..10_000_000).contains(&l) {
                    st.push_latency_us((l / 10) as u32);
                }
            }
        }
        (self.callback)(&f.data, f.keyframe, (f.sample_time.max(0) / 10) as u64);
    }

    /// Re-submit the last frame when the screen is idle (see the timeout logic in `run_loop`).
    fn repeat_last(&mut self) -> Result<()> {
        if !self.pending.is_empty() {
            return Ok(());
        }
        let slot = {
            let mut t = self.shared.table.lock().unwrap_or_else(|p| p.into_inner());
            match t.last {
                Some(i) => {
                    t.refs[i] += 1;
                    i
                }
                None => return Ok(()),
            }
        };
        self.on_frame(slot, qpc_100ns(), false)
    }
}

fn pipeline_thread(
    cfg: StreamConfig,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    init_tx: mpsc::SyncSender<Result<StreamInfo>>,
    stats: Arc<Mutex<StatsAccumulator>>,
    callback: AccessUnitCallback,
) {
    let _rt = match MfRuntime::new() {
        Ok(r) => r,
        Err(e) => {
            let _ = init_tx.send(Err(e));
            return;
        }
    };
    let setup = (|| -> Result<(Pipeline, CaptureSession, Option<IMFAsyncCallback>, StreamInfo)> {
        let d3d = D3dDevice::new()?;
        let manager = create_device_manager(&d3d)?;
        let item = capture::create_item(&cfg.source)?;
        let (src_w, src_h) = capture::item_size(&item)?;
        let (w, h) = fit_output_size(src_w, src_h, cfg.max_width, cfg.max_height);
        let settings = EncoderSettings {
            width: w,
            height: h,
            fps: cfg.fps,
            bitrate_kbps: cfg.bitrate_kbps,
            gop_frames: cfg.gop_frames,
            quality_vs_speed: cfg.quality_vs_speed,
        };
        let encoder = H264Encoder::create(&d3d, &manager, settings, cfg.encoder)?;
        let converter = Nv12Converter::new(&d3d, src_w, src_h, w, h, cfg.fps)?;
        let mut slots = Vec::with_capacity(SLOTS);
        for _ in 0..SLOTS {
            let texture = converter.create_output_texture()?;
            let view = converter.output_view(&texture)?;
            slots.push(Slot { texture, view });
        }
        let shared = Arc::new(Shared {
            slots,
            table: Mutex::new(SlotTable { refs: vec![0; SLOTS], ..Default::default() }),
            converter: Mutex::new(converter),
            stats: stats.clone(),
        });

        // Async encoder events → pipeline messages.
        let event_cb = {
            let etx = tx.clone();
            encoder.event_forwarder(move |event, status| {
                let _ = etx.send(Msg::Mft { event, status });
            })
        };
        if let Some(cb) = &event_cb {
            encoder.request_event(cb)?;
        }

        // Capture callback: pace, convert into a free slot, notify the pipeline.
        let interval = cfg.frame_duration_100ns();
        let tolerance = interval / 4;
        let mut next_due: Option<i64> = None;
        let cshared = shared.clone();
        let ctx = tx.clone();
        let on_frame = move |frame: &CapturedFrame| {
            // Arrival time (QPC). `SystemRelativeTime` is DWM's target composition time, which
            // lies a few ms in the future, so it cannot be the start of a latency measurement.
            let arrived = qpc_100ns();
            // Pace on the composition clock: it is regular, arrival times jitter.
            let t = frame.system_relative_time_100ns;
            if let Some(due) = next_due {
                if t < due - tolerance {
                    cshared.stats.lock().unwrap_or_else(|p| p.into_inner()).frames_paced_out += 1;
                    return;
                }
            }
            next_due = Some(match next_due {
                Some(due) if t <= due + interval => due + interval,
                _ => t + interval,
            });
            let slot = cshared.table.lock().unwrap_or_else(|p| p.into_inner()).acquire_for_write();
            let Some(slot) = slot else {
                cshared.stats.lock().unwrap_or_else(|p| p.into_inner()).frames_dropped += 1;
                return;
            };
            let ok = {
                let mut conv = cshared.converter.lock().unwrap_or_else(|p| p.into_inner());
                let r = conv.convert(
                    frame.texture,
                    frame.content_width,
                    frame.content_height,
                    &cshared.slots[slot].view,
                );
                conv.flush();
                r.is_ok()
            };
            if !ok || ctx.send(Msg::Frame { slot, capture_time: arrived }).is_err() {
                cshared.release(slot);
            }
        };
        let closed_tx = tx.clone();
        let on_closed = move || {
            let _ = closed_tx.send(Msg::SourceClosed);
        };
        // Ask Windows for at most ~fps frames (a little faster, so its own jitter never costs us a
        // frame; our pacer above trims the rest).
        let capture = CaptureSession::start(&d3d, item, Some(interval * 3 / 4), on_frame, on_closed)?;

        let info = StreamInfo {
            encoder_name: encoder.info.name.clone(),
            hardware: encoder.info.hardware,
            gpu_input: encoder.gpu_input,
            async_mft: encoder.is_async(),
            profile: encoder.profile,
            adapter: d3d.adapter_name.clone(),
            width: w,
            height: h,
            source_width: src_w,
            source_height: src_h,
            fps: cfg.fps,
            bitrate_kbps: cfg.bitrate_kbps,
            border_disabled: capture.border_disabled,
            min_update_interval_set: capture.min_update_interval_set,
            codec_settings: encoder.codec_settings.clone(),
        };
        let pipeline = Pipeline {
            cfg: cfg.clone(),
            shared,
            encoder,
            callback,
            stats: stats.clone(),
            pending: VecDeque::new(),
            need_input: 0,
            in_flight: VecDeque::new(),
            t0: None,
            last_sample_time: -1,
            last_submit: None,
            last_was_repeat: false,
            force_key: false,
        };
        Ok((pipeline, capture, event_cb, info))
    })();

    let (mut p, mut capture, event_cb, info) = match setup {
        Ok(v) => v,
        Err(e) => {
            let _ = init_tx.send(Err(e));
            return;
        }
    };
    let _ = init_tx.send(Ok(info));

    let reason = run_loop(&mut p, &rx, event_cb.as_ref());
    capture.stop();
    p.encoder.shutdown();
    let mut st = p.stats();
    st.finished = Some(Instant::now());
    st.ended_reason = reason;
}

/// Returns `Some(reason)` if the stream ended by itself, `None` on a normal stop.
fn run_loop(p: &mut Pipeline, rx: &Receiver<Msg>, event_cb: Option<&IMFAsyncCallback>) -> Option<String> {
    let interval = Duration::from_nanos(1_000_000_000 / p.cfg.fps as u64);
    let mut backlog: VecDeque<Msg> = VecDeque::new();
    loop {
        let msg = if let Some(m) = backlog.pop_front() {
            m
        } else {
            // Idle detection: no fresh frame for 1.5 intervals → repeat at the frame rate.
            // (Waiting only one interval would insert repeats between slightly late frames.)
            let wait = if p.last_was_repeat { interval } else { interval * 3 / 2 };
            let timeout = match (p.cfg.repeat_idle_frames, p.last_submit) {
                (true, Some(t)) => (t + wait).saturating_duration_since(Instant::now()),
                _ => Duration::from_millis(200),
            };
            match rx.recv_timeout(timeout) {
                Ok(m) => m,
                Err(RecvTimeoutError::Timeout) => {
                    if p.cfg.repeat_idle_frames && p.last_submit.is_some() {
                        if let Err(e) = p.repeat_last() {
                            return Some(e.to_string());
                        }
                    }
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => return None,
            }
        };
        let result = match msg {
            Msg::Stop => return None,
            Msg::SourceClosed => return Some("capture source closed".into()),
            Msg::Keyframe => {
                p.force_key = true;
                Ok(())
            }
            Msg::Bitrate(kbps) => {
                // A rejected change keeps the previous rate; nothing else to do.
                let _ = p.encoder.set_bitrate(kbps);
                Ok(())
            }
            Msg::Frame { mut slot, mut capture_time } => {
                // Skip to the newest queued frame; keep other messages in order.
                while let Ok(m) = rx.try_recv() {
                    match m {
                        Msg::Frame { slot: s, capture_time: t } => {
                            p.drop_frame(slot);
                            slot = s;
                            capture_time = t;
                        }
                        other => backlog.push_back(other),
                    }
                }
                {
                    let mut t = p.shared.table.lock().unwrap_or_else(|e| e.into_inner());
                    t.last = Some(slot);
                }
                p.on_frame(slot, capture_time, true)
            }
            Msg::Mft { event, status } => {
                let r = if event == EVENT_ERROR || status.is_err() {
                    Err(Error::Encoder(format!("encoder event {event} failed: 0x{:08X}", status.0 as u32)))
                } else if event == METransformNeedInput.0 as u32 {
                    p.need_input += 1;
                    p.pump()
                } else if event == METransformHaveOutput.0 as u32 {
                    match p.encoder.process_output() {
                        Ok(OutputResult::Frame(f)) => {
                            p.deliver(f);
                            Ok(())
                        }
                        Ok(_) => Ok(()),
                        Err(e) => Err(e),
                    }
                } else {
                    Ok(())
                };
                match (r, event_cb) {
                    (Ok(()), Some(cb)) => p.encoder.request_event(cb),
                    (r, _) => r,
                }
            }
        };
        if let Err(e) = result {
            return Some(e.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn streamer_can_be_shared_across_threads() {
        // The agent keeps the Streamer in async state shared between tasks.
        assert_send_sync::<Streamer>();
        assert_send_sync::<StreamInfo>();
    }

    #[test]
    fn slot_table_never_hands_out_the_repeat_source() {
        let mut t = SlotTable { refs: vec![0; 3], ..Default::default() };
        let a = t.acquire_for_write().unwrap();
        t.last = Some(a);
        t.release(a);
        let b = t.acquire_for_write().unwrap();
        let c = t.acquire_for_write().unwrap();
        assert!(b != a && c != a && b != c);
        assert_eq!(t.acquire_for_write(), None, "slot `a` is protected, others busy");
        t.release(b);
        assert_eq!(t.acquire_for_write(), Some(b));
    }

    #[test]
    fn qpc_clock_is_monotonic_100ns() {
        let a = qpc_100ns();
        std::thread::sleep(Duration::from_millis(5));
        let b = qpc_100ns();
        assert!(b - a >= 40_000, "{a} {b}");
    }
}
