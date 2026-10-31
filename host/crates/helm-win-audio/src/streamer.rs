//! `AudioStreamer`: WASAPI loopback → mix → resample → frame → Opus → callback, on one thread.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Media::Audio::IMMNotificationClient;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};

use crate::config::AudioConfig;
use crate::error::{Ctx, Error, Result};
use crate::format::to_f32;
use crate::framer::{Adjust, Framer, Timeline, pts_us, samples_to_us};
use crate::mix::ChannelMixer;
use crate::opus::{Encoder, MAX_PACKET};
use crate::resample::Resampler;
use crate::stats::{AudioStats, StatsAccumulator};
use crate::wasapi::{DeviceWatcher, LoopbackCapture, default_device_id, enumerator, qpc_now_us};

/// Callback for encoded packets: `(opus_packet, pts_us)`.
pub type PacketCallback = Box<dyn FnMut(&[u8], u64) + Send + 'static>;

/// Gaps longer than this are skipped (whole packets are not sent; pts jumps) instead of being
/// filled with silence.
const MAX_SILENCE_FILL_US: i64 = 1_000_000;
/// Retry interval while no device can be opened.
const REOPEN_RETRY: Duration = Duration::from_millis(500);

struct Shared {
    stop: AtomicBool,
    running: AtomicBool,
    stats: Mutex<StatsAccumulator>,
    /// Auto-reset event: WASAPI data ready, device change, or stop.
    wake: isize,
}

/// A running system-audio stream. `Send + Sync`; dropping it stops the stream.
pub struct AudioStreamer {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    config: AudioConfig,
}

impl AudioStreamer {
    /// Start capturing the default playback device. Blocks until the device and encoder are
    /// running (a few ms) or returns the error. `on_packet(opus_packet, pts_us)` is called on the
    /// internal thread `helm-audio` every `frame_ms`; keep it fast (copy into a bounded channel).
    pub fn start<F>(config: AudioConfig, on_packet: F) -> Result<AudioStreamer>
    where
        F: FnMut(&[u8], u64) + Send + 'static,
    {
        config.validate()?;
        let event = unsafe { CreateEventW(None, false, false, None) }.ctx("CreateEventW")?;
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            running: AtomicBool::new(true),
            stats: Mutex::new(StatsAccumulator::default()),
            wake: event.0 as isize,
        });
        let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();
        let sh = shared.clone();
        let cfg = config.clone();
        let cb: PacketCallback = Box::new(on_packet);
        let thread = std::thread::Builder::new()
            .name("helm-audio".into())
            .spawn(move || pipeline_thread(cfg, sh, cb, ready_tx))
            .map_err(|e| Error::Stopped(format!("spawn: {e}")))?;
        let streamer = AudioStreamer {
            shared,
            thread: Some(thread),
            config,
        };
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(streamer),
            Ok(Err(e)) => Err(e), // Drop joins the (finished) thread and closes the event.
            Err(_) => Err(Error::Stopped("audio thread exited during start".into())),
        }
    }

    pub fn config(&self) -> &AudioConfig {
        &self.config
    }

    pub fn stats(&self) -> AudioStats {
        self.shared.stats.lock().map(|s| s.snapshot()).unwrap_or_default()
    }

    /// False after a fatal error (`stats().ended_reason`) or `stop()`.
    pub fn is_running(&self) -> bool {
        self.shared.running.load(Ordering::SeqCst)
    }

    /// Stop, join the thread and return the final statistics.
    pub fn stop(mut self) -> AudioStats {
        self.shutdown();
        self.stats()
    }

    fn shutdown(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        unsafe {
            let _ = SetEvent(HANDLE(self.shared.wake as *mut _));
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
            unsafe {
                let _ = CloseHandle(HANDLE(self.shared.wake as *mut _));
            }
        }
    }
}

impl Drop for AudioStreamer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Per-device conversion chain.
struct Chain {
    cap: LoopbackCapture,
    mixer: ChannelMixer,
    resampler: Resampler,
}

impl Chain {
    fn new(cap: LoopbackCapture, cfg: &AudioConfig) -> Chain {
        let mixer = ChannelMixer::new(cap.format.channels, cap.format.channel_mask, cfg.channels);
        let resampler = Resampler::new(cap.format.sample_rate, cfg.sample_rate, cfg.channels as usize);
        Chain { cap, mixer, resampler }
    }
}

/// Everything after the device: timeline, framer, encoder, callback, stats.
struct Sink {
    cfg: AudioConfig,
    ch: usize,
    timeline: Timeline,
    framer: Framer,
    encoder: Encoder,
    cb: PacketCallback,
    frame: Vec<f32>,
    out: Vec<u8>,
    /// Output sample position where the last real (device) data ends; packets ending at or
    /// before it count for the latency statistics.
    real_until: u64,
    /// `GetBuffer` QPC (µs) of the device packet most recently appended.
    last_packet_qpc: i64,
}

impl Sink {
    /// Append `samples` per channel of silence; very long gaps skip whole packets instead.
    fn gap(&mut self, samples: u64, st: &mut StatsAccumulator) {
        if samples == 0 {
            return;
        }
        let fs = self.cfg.frame_samples() as u64;
        if samples_to_us(samples, self.cfg.sample_rate) <= MAX_SILENCE_FILL_US {
            self.framer.push_silence(samples as usize * self.ch);
            self.timeline.advance(samples);
            st.silence_samples_inserted += samples;
            return;
        }
        // Complete the partial frame, skip whole frames, then pad the remainder.
        let pad = (self.framer.to_frame_boundary() / self.ch) as u64;
        let pad = pad.min(samples);
        self.framer.push_silence(pad as usize * self.ch);
        self.timeline.advance(pad);
        st.silence_samples_inserted += pad;
        self.flush(st);
        let rest = samples - pad;
        let skip = rest / fs;
        self.framer.skip_frames(skip);
        self.timeline.advance(skip * fs);
        let tail = rest - skip * fs;
        self.framer.push_silence(tail as usize * self.ch);
        self.timeline.advance(tail);
        st.silence_samples_inserted += tail;
    }

    /// Append real audio (already at the output rate / channel count), after `drop` samples per
    /// channel are discarded.
    fn audio(&mut self, data: &[f32], drop: u64, st: &mut StatsAccumulator) -> u64 {
        let drop_vals = (drop as usize * self.ch).min(data.len());
        let keep = &data[drop_vals..];
        self.framer.push(keep);
        self.timeline.advance((keep.len() / self.ch) as u64);
        self.real_until = self.timeline.position();
        st.samples_dropped += (drop_vals / self.ch) as u64;
        drop - (drop_vals / self.ch) as u64
    }

    /// Encode and deliver every complete frame.
    fn flush(&mut self, st: &mut StatsAccumulator) {
        while let Some(index) = self.framer.pop_into(&mut self.frame) {
            let n = match self.encoder.encode_f32(&self.frame, &mut self.out) {
                Ok(n) => n,
                Err(e) => {
                    st.last_error = Some(e.to_string());
                    continue;
                }
            };
            let pts = pts_us(index, self.cfg.frame_samples(), self.cfg.sample_rate);
            (self.cb)(&self.out[..n], pts);
            st.packets += 1;
            st.bytes += n as u64;
            if self.frame.iter().all(|&v| v == 0.0) {
                st.silent_packets += 1;
            }
            let end_pos = (index + 1) * self.cfg.frame_samples() as u64;
            if end_pos <= self.real_until {
                // The device packet that completed this frame was stamped `last_packet_qpc`
                // (WASAPI stamps loopback packets when the engine mixes them, i.e. when they
                // become readable).
                let lat = (qpc_now_us() - self.last_packet_qpc).clamp(0, u32::MAX as i64);
                st.push_latency_us(lat as u32);
            }
            st.qpc_origin_us = self.timeline.origin_us();
        }
    }
}

struct ComGuard;
impl Drop for ComGuard {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

fn pipeline_thread(cfg: AudioConfig, shared: Arc<Shared>, cb: PacketCallback, ready: mpsc::Sender<Result<()>>) {
    let mut ready = Some(ready);
    if let Err(e) = pipeline(&cfg, &shared, cb, &mut ready) {
        match ready.take() {
            Some(tx) => {
                let _ = tx.send(Err(e));
            }
            None => {
                if let Ok(mut s) = shared.stats.lock() {
                    s.ended_reason = Some(e.to_string());
                }
            }
        }
    }
    if let Ok(mut s) = shared.stats.lock() {
        s.finished = Some(Instant::now());
    }
    shared.running.store(false, Ordering::SeqCst);
}

fn pipeline(
    cfg: &AudioConfig,
    shared: &Shared,
    cb: PacketCallback,
    ready: &mut Option<mpsc::Sender<Result<()>>>,
) -> Result<()> {
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
        .ok()
        .ctx("CoInitializeEx")?;
    let _com = ComGuard;
    let wake = HANDLE(shared.wake as *mut _);
    let enumerator = enumerator()?;

    let changed = Arc::new(AtomicBool::new(false));
    let watcher: IMMNotificationClient = DeviceWatcher {
        changed: changed.clone(),
        wake: shared.wake,
    }
    .into();
    unsafe { enumerator.RegisterEndpointNotificationCallback(&watcher) }.ctx("RegisterEndpointNotificationCallback")?;
    struct Unregister<'a>(
        &'a windows::Win32::Media::Audio::IMMDeviceEnumerator,
        &'a IMMNotificationClient,
    );
    impl Drop for Unregister<'_> {
        fn drop(&mut self) {
            unsafe {
                let _ = self.0.UnregisterEndpointNotificationCallback(self.1);
            }
        }
    }
    let _unregister = Unregister(&enumerator, &watcher);

    let mut chain = Some(Chain::new(LoopbackCapture::open_default(&enumerator, wake)?, cfg));
    let mut sink = Sink {
        cfg: cfg.clone(),
        ch: cfg.channels as usize,
        timeline: Timeline::new(cfg.sample_rate, cfg.frame_ms),
        framer: Framer::new(cfg.frame_len()),
        encoder: Encoder::new(cfg)?,
        cb,
        frame: Vec::with_capacity(cfg.frame_len()),
        out: vec![0u8; MAX_PACKET],
        real_until: 0,
        last_packet_qpc: 0,
    };
    {
        let mut s = shared.stats.lock().unwrap();
        s.started = Some(Instant::now());
        s.device = chain.as_ref().map(|c| c.cap.info.clone());
    }
    if let Some(tx) = ready.take() {
        let _ = tx.send(Ok(()));
    }

    let wait_ms = (cfg.frame_ms / 2).max(5);
    let mut next_retry = Instant::now();
    let (mut raw, mut mixed, mut resampled) = (Vec::new(), Vec::new(), Vec::new());
    let mut pending_drop = 0u64;

    while !shared.stop.load(Ordering::SeqCst) {
        unsafe {
            let _ = WaitForSingleObject(wake, wait_ms);
        }
        if shared.stop.load(Ordering::SeqCst) {
            break;
        }
        let mut st = shared.stats.lock().unwrap();

        // Device change: reopen on the (new) default endpoint.
        if changed.swap(false, Ordering::SeqCst) && (cfg.follow_default_device || chain.is_none()) {
            let current = chain.as_ref().map(|c| c.cap.info.id.clone());
            let default_id = default_device_id(&enumerator);
            if default_id != current {
                chain = None;
                next_retry = Instant::now();
            }
        }
        if chain.is_none() && Instant::now() >= next_retry {
            match LoopbackCapture::open_default(&enumerator, wake) {
                Ok(cap) => {
                    st.device_changes += 1;
                    st.device = Some(cap.info.clone());
                    st.last_error = None;
                    chain = Some(Chain::new(cap, cfg));
                    pending_drop = 0;
                }
                Err(e) => {
                    st.last_error = Some(e.to_string());
                    st.device = None;
                    next_retry = Instant::now() + REOPEN_RETRY;
                }
            }
        }

        if let Some(c) = chain.as_mut() {
            let fmt = c.cap.format;
            let Chain { cap, mixer, resampler } = c;
            let r = cap.drain(|p| {
                st.device_packets += 1;
                st.device_frames += p.frames as u64;
                st.push_device_delay_us((qpc_now_us() - p.qpc_us).clamp(i32::MIN as i64, i32::MAX as i64) as i32);
                if p.discontinuity {
                    st.discontinuities += 1;
                }
                raw.clear();
                mixed.clear();
                resampled.clear();
                to_f32(&fmt, p.data, p.frames, &mut raw);
                mixer.process(&raw, &mut mixed);
                resampler.process(&mixed, &mut resampled);
                match sink.timeline.reconcile(p.qpc_us) {
                    Adjust::None => {}
                    Adjust::InsertSilence(n) => sink.gap(n, &mut st),
                    Adjust::Drop(n) => pending_drop = n,
                }
                sink.last_packet_qpc = p.qpc_us;
                pending_drop = sink.audio(&resampled, pending_drop, &mut st);
                sink.flush(&mut st);
            });
            if let Err(e) = r {
                // Typically AUDCLNT_E_DEVICE_INVALIDATED (unplugged, format changed, exclusive
                // app took over). Reopen shortly.
                st.last_error = Some(e.to_string());
                chain = None;
                next_retry = Instant::now() + Duration::from_millis(100);
            }
        }

        if cfg.fill_silence {
            let due = sink.timeline.silence_due(qpc_now_us());
            sink.gap(due, &mut st);
        }
        sink.flush(&mut st);
    }
    Ok(())
}
