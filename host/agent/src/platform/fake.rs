//! A fake PC for tests: canned data in, every call recorded, nothing touches the real system.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use helm_win_input::{Op, Sink};
use helm_win_system::display::DisplayKind;
use helm_win_system::media::PlaybackStatus;

use super::*;

/// Mutable fake state. Tests may change it through [`FakePlatform::with`].
#[derive(Debug, Clone)]
pub struct FakeState {
    pub apps: Vec<AppEntry>,
    pub windows: Vec<WindowInfo>,
    pub media: Option<MediaState>,
    pub artwork: Option<Vec<u8>>,
    pub volume: VolumeState,
    pub displays: Vec<Display>,
    pub clipboard_text: Option<String>,
    pub clipboard_png: Option<Vec<u8>>,
    pub adapters: Vec<Adapter>,
    pub battery: Option<Battery>,
    pub monitors: Vec<Monitor>,
    /// Size of each synthetic H.264 access unit the fake streamer emits.
    pub au_size: usize,
    /// `start_audio` works (true) or reports `unsupported`.
    pub audio_supported: bool,
    pub cursor: CursorState,
    // Phase 3
    pub stats: Stats,
    pub processes: Vec<ProcessInfo>,
    pub audio_sessions: Vec<AudioSession>,
    pub mic_muted: Option<bool>,
    pub fixed_drives: Vec<std::path::PathBuf>,
    pub clock: String,
    // Phase 5
    pub wifi_ssid: Option<String>,
    pub utc_offset_minutes: i32,
    /// `new_pen` works (true) or reports `unsupported`.
    pub pen_supported: bool,
}

pub fn sample_process(pid: u32, name: &str) -> ProcessInfo {
    ProcessInfo {
        pid,
        parent_pid: None,
        name: name.into(),
        exe: Some(format!("C:\\Fake\\{name}")),
        cpu_pct: 1.5,
        ram_bytes: 10_000_000,
        is_elevated: Some(false),
        priority: Some(Priority::Normal),
    }
}

pub fn sample_audio_session(id: &str, name: &str) -> AudioSession {
    AudioSession {
        id: id.into(),
        name: name.into(),
        pid: 1234,
        level: 0.8,
        muted: false,
        is_system: false,
        active: true,
    }
}

/// Stats snapshot with the given CPU / RAM use and one disk.
pub fn sample_stats(cpu: f32, ram_pct: u64) -> Stats {
    Stats {
        timestamp_ms: 1,
        cpu_total: cpu,
        cpu_per_core: vec![cpu, cpu],
        ram_used: ram_pct * 1_000_000,
        ram_total: 100_000_000,
        disks: vec![helm_win_sysmon::stats::DiskStats {
            mount: "C:\\".into(),
            used: 900,
            total: 1000,
            ..Default::default()
        }],
        ..Default::default()
    }
}

/// One tiny valid PNG (1x1, opaque black).
pub const TINY_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0x60, 0x60, 0x60, 0x00,
    0x00, 0x00, 0x04, 0x00, 0x01, 0x27, 0x34, 0x27, 0x0A, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E,
    0x44, 0xAE, 0x42, 0x60, 0x82,
];

pub fn sample_media(title: &str) -> MediaState {
    MediaState {
        app_id: "FakePlayer.exe".into(),
        title: title.into(),
        artist: "Fake Artist".into(),
        album: "Fake Album".into(),
        status: PlaybackStatus::Playing,
        position_ms: Some(1000),
        duration_ms: Some(180_000),
        rate: 1.0,
        has_artwork: true,
        can_play: true,
        can_pause: true,
        can_next: true,
        can_prev: true,
        can_stop: true,
        can_seek: true,
    }
}

pub fn sample_window(hwnd: u64, title: &str) -> WindowInfo {
    WindowInfo {
        hwnd,
        title: title.into(),
        pid: 1000 + hwnd as u32,
        exe_path: Some(format!("C:\\Fake\\{title}.exe")),
        app_id: Some(format!("Fake.{title}")),
        is_minimized: false,
        is_foreground: hwnd == 100,
    }
}

impl Default for FakeState {
    fn default() -> Self {
        FakeState {
            apps: vec![
                AppEntry {
                    app_id: "Fake.Notes".into(),
                    name: "Notes".into(),
                    kind: AppKind::Desktop,
                    target_path: Some("C:\\Fake\\notes.exe".into()),
                },
                AppEntry {
                    app_id: "Fake.Store_abc!App".into(),
                    name: "Store".into(),
                    kind: AppKind::Packaged,
                    target_path: None,
                },
            ],
            windows: vec![sample_window(100, "Notes"), sample_window(200, "Browser")],
            media: Some(sample_media("Fake Song")),
            artwork: Some(TINY_PNG.to_vec()),
            volume: VolumeState {
                level: 0.5,
                muted: false,
            },
            displays: vec![Display {
                id: "wmi:0".into(),
                name: "Built-in display".into(),
                kind: DisplayKind::Internal,
                level: 70,
                supported: true,
            }],
            clipboard_text: Some("fake clipboard".into()),
            clipboard_png: None,
            adapters: vec![Adapter {
                adapter_name: "Ethernet".into(),
                description: "Fake NIC".into(),
                mac: "aa:bb:cc:dd:ee:ff".into(),
                ipv4: vec!["192.168.0.2".into()],
                is_up: true,
                if_type: 6,
                locally_administered: false,
            }],
            battery: Some(Battery {
                percent: Some(80),
                charging: true,
                ac_online: Some(true),
                seconds_remaining: None,
                saver_on: false,
            }),
            monitors: vec![Monitor {
                id: "\\\\.\\DISPLAY1".into(),
                name: "Fake Monitor".into(),
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
                is_primary: true,
            }],
            stats: sample_stats(10.0, 40),
            processes: vec![
                sample_process(1000, "notes.exe"),
                sample_process(2000, "game.exe"),
            ],
            audio_sessions: vec![sample_audio_session("sess-1", "Music")],
            mic_muted: Some(false),
            fixed_drives: Vec::new(),
            clock: "12:34".into(),
            wifi_ssid: None,
            utc_offset_minutes: 0,
            pen_supported: true,
            au_size: 200,
            audio_supported: true,
            cursor: CursorState {
                x: 960,
                y: 540,
                visible: true,
                handle: 0x1001,
            },
        }
    }
}

/// What the fake streamer was asked to do (shared with the emitting thread).
#[derive(Debug, Default)]
pub struct FakeVideoCtl {
    pub params: Mutex<Option<VideoParams>>,
    pub keyframe_requests: AtomicU64,
    pub bitrates: Mutex<Vec<u32>>,
    pub frames: AtomicU64,
    /// Set to make the stream end by itself with this reason.
    pub end_with: Mutex<Option<String>>,
    stop: AtomicBool,
    force_key: AtomicBool,
}

impl FakeVideoCtl {
    /// True once the agent dropped (stopped) this stream.
    pub fn is_stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }
}

/// Synthetic Annex-B access unit: IDR with SPS/PPS, or a P slice. `seq` follows the slice
/// header byte so tests can follow frames.
pub fn synthetic_au(key: bool, seq: u32, size: usize) -> Vec<u8> {
    let mut au = Vec::with_capacity(size.max(32));
    if key {
        au.extend_from_slice(&[0, 0, 0, 1, 0x67, 0x4D, 0x40, 0x2A]); // SPS (Main)
        au.extend_from_slice(&[0, 0, 0, 1, 0x68, 0xEE, 0x3C, 0x80]); // PPS
        au.extend_from_slice(&[0, 0, 0, 1, 0x65]); // IDR slice
    } else {
        au.extend_from_slice(&[0, 0, 0, 1, 0x41]); // non-IDR slice
    }
    au.extend_from_slice(&seq.to_be_bytes());
    while au.len() < size {
        au.push(0xAA);
    }
    au
}

struct FakeVideo {
    ctl: Arc<FakeVideoCtl>,
    info: VideoInfo,
    rect: Rect,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl VideoStream for FakeVideo {
    fn info(&self) -> VideoInfo {
        self.info.clone()
    }
    fn request_keyframe(&self) {
        self.ctl.keyframe_requests.fetch_add(1, Ordering::SeqCst);
        self.ctl.force_key.store(true, Ordering::SeqCst);
    }
    fn set_bitrate(&self, kbps: u32) {
        self.ctl.bitrates.lock().unwrap().push(kbps);
    }
    fn is_running(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }
    fn ended_reason(&self) -> Option<String> {
        self.ctl.end_with.lock().unwrap().clone()
    }
    fn source_rect(&self) -> Option<Rect> {
        Some(self.rect)
    }
}

impl Drop for FakeVideo {
    fn drop(&mut self) {
        self.ctl.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct FakeAudio {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl AudioStream for FakeAudio {
    fn is_running(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }
    fn pre_skip(&self) -> u32 {
        120
    }
}

impl Drop for FakeAudio {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[derive(Debug)]
struct FakeSampler(Arc<Mutex<FakeState>>);

impl StatsSampler for FakeSampler {
    fn sample(&self) -> PResult<Stats> {
        Ok(self.0.lock().unwrap().stats.clone())
    }
}

#[derive(Debug, Default)]
pub struct FakePlatform {
    state: Arc<Mutex<FakeState>>,
    foreground: Mutex<Option<ForegroundSink>>,
    calls: Mutex<Vec<String>>,
    ops: Arc<Mutex<Vec<Op>>>,
    block_input: Arc<AtomicBool>,
    events: Mutex<Option<EventSink>>,
    video: Mutex<Option<Arc<FakeVideoCtl>>>,
    audio_running: Arc<AtomicU64>,
    // Phase 5
    recordings: Mutex<Vec<Arc<FakeRecording>>>,
    terminals: Mutex<Vec<Arc<FakeTerm>>>,
    pen: Arc<Mutex<Vec<PenSample>>>,
    pen_releases: Arc<AtomicU64>,
    toasts: Mutex<Vec<FakeToast>>,
    http: Arc<FakeHttp>,
}

struct FakeToast(Toast, Arc<ToastCallback>);

impl std::fmt::Debug for FakeToast {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug)]
struct RecordingSink {
    ops: Arc<Mutex<Vec<Op>>>,
    blocked: Arc<AtomicBool>,
}

impl Sink for RecordingSink {
    fn send(&mut self, ops: &[Op]) -> helm_win_input::Result<()> {
        if self.blocked.load(Ordering::SeqCst) {
            return Err(helm_win_input::Error::BlockedByElevation);
        }
        self.ops.lock().unwrap().extend_from_slice(ops);
        Ok(())
    }
}

impl FakePlatform {
    pub fn new() -> Arc<FakePlatform> {
        Arc::new(FakePlatform::default())
    }

    /// Change the canned state.
    pub fn with<R>(&self, f: impl FnOnce(&mut FakeState) -> R) -> R {
        f(&mut self.state.lock().unwrap())
    }

    /// Every call that would change something, in order (e.g. `launch_app:Fake.Notes`).
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    /// Low-level input operations "sent" so far.
    pub fn ops(&self) -> Vec<Op> {
        self.ops.lock().unwrap().clone()
    }

    /// Make every injection fail like a UIPI block.
    pub fn set_block_input(&self, on: bool) {
        self.block_input.store(on, Ordering::SeqCst);
    }

    /// Pretend a watcher saw a change. Returns false if watchers are not running.
    pub fn emit(&self, event: PlatformEvent) -> bool {
        match self.events.lock().unwrap().as_ref() {
            Some(tx) => tx.send(event).is_ok(),
            None => false,
        }
    }

    /// The most recently started fake video stream.
    pub fn video(&self) -> Option<Arc<FakeVideoCtl>> {
        self.video.lock().unwrap().clone()
    }

    /// Fake audio streams currently running.
    pub fn audio_streams(&self) -> u64 {
        self.audio_running.load(Ordering::SeqCst)
    }

    /// Pretend the foreground window changed. Returns false if the watcher is not running.
    pub fn emit_foreground(&self, exe_name: &str) -> bool {
        let info = ForegroundInfo {
            hwnd: 100,
            pid: 1000,
            exe: Some(format!("C:\\Fake\\{exe_name}")),
            exe_name: exe_name.into(),
            title: exe_name.into(),
        };
        match self.foreground.lock().unwrap().as_ref() {
            Some(tx) => tx.send(info).is_ok(),
            None => false,
        }
    }

    pub fn watchers_running(&self) -> bool {
        self.events.lock().unwrap().is_some()
    }

    // ---- Phase 5 ----

    /// Every recording started so far.
    pub fn recordings(&self) -> Vec<Arc<FakeRecording>> {
        self.recordings.lock().unwrap().clone()
    }

    /// Every terminal opened so far.
    pub fn terminals(&self) -> Vec<Arc<FakeTerm>> {
        self.terminals.lock().unwrap().clone()
    }

    /// Pen samples "injected" so far.
    pub fn pen_samples(&self) -> Vec<PenSample> {
        self.pen.lock().unwrap().clone()
    }

    /// How many pen devices were released (dropped).
    pub fn pen_releases(&self) -> u64 {
        self.pen_releases.load(Ordering::SeqCst)
    }

    /// Toasts "shown" so far.
    pub fn toasts(&self) -> Vec<Toast> {
        self.toasts.lock().unwrap().iter().map(|t| t.0.clone()).collect()
    }

    /// Pretend the user clicked toast `index` (button `action_id`, or the body for `None`).
    pub fn click_toast(&self, index: usize, action_id: Option<&str>, reply: Option<&str>) -> bool {
        let cb = match self.toasts.lock().unwrap().get(index) {
            Some(t) => t.1.clone(),
            None => return false,
        };
        cb(ToastEvent::Activated {
            action_id: action_id.map(Into::into),
            reply: reply.map(Into::into),
        });
        true
    }

    /// The scripted HTTP transport (responses and the requests it saw).
    pub fn http_fake(&self) -> Arc<FakeHttp> {
        self.http.clone()
    }


    fn record(&self, call: String) {
        self.calls.lock().unwrap().push(call);
    }

    fn state(&self) -> std::sync::MutexGuard<'_, FakeState> {
        self.state.lock().unwrap()
    }

    fn known_window(&self, hwnd: u64) -> PResult<()> {
        if self.state().windows.iter().any(|w| w.hwnd == hwnd) {
            Ok(())
        } else {
            Err(RpcError::bad_params(format!("not found: window {hwnd}")))
        }
    }
}

impl Platform for FakePlatform {
    fn list_apps(&self) -> PResult<Vec<AppEntry>> {
        Ok(self.state().apps.clone())
    }
    fn app_icon_png(&self, app_id: &str, size: u32) -> PResult<Vec<u8>> {
        if !self.state().apps.iter().any(|a| a.app_id == app_id) {
            return Err(RpcError::bad_params(format!("not found: app {app_id}")));
        }
        self.record(format!("app_icon_png:{app_id}:{size}"));
        Ok(TINY_PNG.to_vec())
    }
    fn launch_app(&self, app_id: &str) -> PResult<()> {
        if !self.state().apps.iter().any(|a| a.app_id == app_id) {
            return Err(RpcError::bad_params(format!("not found: app {app_id}")));
        }
        self.record(format!("launch_app:{app_id}"));
        Ok(())
    }
    fn open_target(&self, target: &str) -> PResult<()> {
        self.record(format!("open_target:{target}"));
        Ok(())
    }

    fn list_windows(&self) -> PResult<Vec<WindowInfo>> {
        Ok(self.state().windows.clone())
    }
    fn focus_window(&self, hwnd: u64) -> PResult<FocusResult> {
        self.known_window(hwnd)?;
        self.record(format!("focus_window:{hwnd}"));
        Ok(FocusResult {
            focused: true,
            method: "set_foreground_window".into(),
        })
    }
    fn window_command(&self, hwnd: u64, cmd: WindowCommand) -> PResult<()> {
        self.known_window(hwnd)?;
        self.record(format!("window_command:{hwnd}:{cmd:?}"));
        Ok(())
    }

    fn media_state(&self) -> PResult<Option<MediaState>> {
        Ok(self.state().media.clone())
    }
    fn media_artwork_png(&self) -> PResult<Option<Vec<u8>>> {
        Ok(self.state().artwork.clone())
    }
    fn media_control(&self, action: MediaAction) -> PResult<()> {
        if self.state().media.is_none() {
            return Err(RpcError::bad_params("not found: no media session"));
        }
        self.record(format!("media_control:{action:?}"));
        Ok(())
    }

    fn volume(&self) -> PResult<VolumeState> {
        Ok(self.state().volume)
    }
    fn set_volume(&self, level: f32) -> PResult<()> {
        if !(0.0..=1.0).contains(&level) {
            return Err(RpcError::bad_params("level out of range"));
        }
        self.record(format!("set_volume:{level:.2}"));
        self.state().volume.level = level;
        Ok(())
    }
    fn set_muted(&self, muted: bool) -> PResult<()> {
        self.record(format!("set_muted:{muted}"));
        self.state().volume.muted = muted;
        Ok(())
    }

    fn brightness_list(&self) -> PResult<Vec<Display>> {
        Ok(self.state().displays.clone())
    }
    fn set_brightness(&self, id: &str, level: u8) -> PResult<()> {
        let mut st = self.state();
        let Some(d) = st.displays.iter_mut().find(|d| d.id == id) else {
            return Err(RpcError::bad_params(format!("not found: display {id}")));
        };
        d.level = level;
        drop(st);
        self.record(format!("set_brightness:{id}:{level}"));
        Ok(())
    }

    fn clipboard_text(&self) -> PResult<Option<String>> {
        Ok(self.state().clipboard_text.clone())
    }
    fn clipboard_png(&self) -> PResult<Option<Vec<u8>>> {
        Ok(self.state().clipboard_png.clone())
    }
    fn set_clipboard_text(&self, text: &str) -> PResult<()> {
        self.record(format!("set_clipboard_text:{text}"));
        let mut st = self.state();
        st.clipboard_text = Some(text.to_string());
        st.clipboard_png = None;
        Ok(())
    }
    fn set_clipboard_png(&self, png: &[u8]) -> PResult<()> {
        if !png.starts_with(&TINY_PNG[..8]) {
            return Err(RpcError::bad_params("bad image: not a PNG"));
        }
        self.record(format!("set_clipboard_png:{}", png.len()));
        let mut st = self.state();
        st.clipboard_png = Some(png.to_vec());
        st.clipboard_text = None;
        Ok(())
    }

    fn new_injector(&self) -> PResult<Injector> {
        Ok(Injector::with_sink(RecordingSink {
            ops: self.ops.clone(),
            blocked: self.block_input.clone(),
        }))
    }

    fn network_adapters(&self) -> PResult<Vec<Adapter>> {
        Ok(self.state().adapters.clone())
    }
    fn battery(&self) -> PResult<Option<Battery>> {
        Ok(self.state().battery)
    }
    fn send_wol(&self, packet: &[u8; 102]) -> PResult<()> {
        self.record(format!("send_wol:{}", hex::encode(&packet[6..12])));
        Ok(())
    }

    fn monitors(&self) -> PResult<Vec<Monitor>> {
        Ok(self.state().monitors.clone())
    }
    fn screenshot_png(&self, monitor_id: Option<&str>) -> PResult<Vec<u8>> {
        if let Some(id) = monitor_id
            && !self.state().monitors.iter().any(|m| m.id == id)
        {
            return Err(RpcError::bad_params(format!("not found: monitor {id}")));
        }
        self.record(format!("screenshot_png:{}", monitor_id.unwrap_or("-")));
        Ok(TINY_PNG.to_vec())
    }

    fn start_watchers(&self, events: EventSink) -> WatchGuard {
        *self.events.lock().unwrap() = Some(events);
        Box::new(())
    }

    fn window_thumbnail_png(&self, hwnd: u64, max_side: u32) -> PResult<Option<Vec<u8>>> {
        let win = self.state().windows.iter().find(|w| w.hwnd == hwnd).cloned();
        let Some(win) = win else {
            return Err(RpcError::bad_params(format!("no window with handle {hwnd}")));
        };
        self.record(format!("window_thumbnail_png:{hwnd}:{max_side}"));
        Ok((!win.is_minimized).then(|| TINY_PNG.to_vec()))
    }

    fn start_video(
        &self,
        params: VideoParams,
        mut sink: AccessUnitSink,
    ) -> PResult<Box<dyn VideoStream>> {
        let st = self.state().clone();
        let monitor = match &params.source {
            VideoSource::Monitor(None) => st.monitors.iter().find(|m| m.is_primary),
            VideoSource::Monitor(Some(id)) => st.monitors.iter().find(|m| &m.id == id),
            VideoSource::Window(hwnd) => {
                if !st.windows.iter().any(|w| w.hwnd == *hwnd) {
                    return Err(RpcError::bad_params(format!("not found: window {hwnd}")));
                }
                st.monitors.iter().find(|m| m.is_primary)
            }
        };
        let Some(monitor) = monitor else {
            return Err(RpcError::bad_params("not found: monitor"));
        };
        let rect = Rect::from(monitor);
        // Same rule as the real encoder: fit inside max, keep aspect, even sizes.
        let scale = f64::min(
            1.0,
            f64::min(
                params.max_width as f64 / rect.width as f64,
                params.max_height as f64 / rect.height as f64,
            ),
        );
        let even = |v: f64| ((v as u32) & !1).max(2);
        let info = VideoInfo {
            width: even(rect.width as f64 * scale),
            height: even(rect.height as f64 * scale),
            fps: params.fps,
            hardware: true,
            encoder_name: "Fake H.264 Encoder".into(),
        };
        self.record(format!(
            "start_video:{:?}:{}x{}@{}:{}",
            params.source, params.max_width, params.max_height, params.fps, params.bitrate_kbps
        ));
        let ctl = Arc::new(FakeVideoCtl::default());
        *ctl.params.lock().unwrap() = Some(params.clone());
        *self.video.lock().unwrap() = Some(ctl.clone());
        let tctl = ctl.clone();
        let size = st.au_size;
        let fps = params.fps.max(1);
        let thread = std::thread::Builder::new()
            .name("fake-video".into())
            .spawn(move || {
                let interval = Duration::from_micros(1_000_000 / fps as u64);
                let gop = (fps * 2) as u64;
                let mut n: u64 = 0;
                while !tctl.stop.load(Ordering::SeqCst) && tctl.end_with.lock().unwrap().is_none()
                {
                    let forced = tctl.force_key.swap(false, Ordering::SeqCst);
                    let key = n.is_multiple_of(gop) || forced;
                    let au = synthetic_au(key, n as u32, size);
                    sink(&au, key, n * 1_000_000 / fps as u64);
                    tctl.frames.fetch_add(1, Ordering::SeqCst);
                    n += 1;
                    std::thread::sleep(interval);
                }
            })
            .map_err(|e| RpcError::internal(e.to_string()))?;
        Ok(Box::new(FakeVideo {
            ctl,
            info,
            rect,
            thread: Some(thread),
        }))
    }

    fn start_audio(&self, mut sink: AudioSink) -> PResult<Box<dyn AudioStream>> {
        if !self.state().audio_supported {
            return Err(RpcError::unsupported("fake: no audio"));
        }
        self.record("start_audio".into());
        let stop = Arc::new(AtomicBool::new(false));
        let tstop = stop.clone();
        let running = self.audio_running.clone();
        running.fetch_add(1, Ordering::SeqCst);
        let thread = std::thread::Builder::new()
            .name("fake-audio".into())
            .spawn(move || {
                let mut pts: u64 = 0;
                while !tstop.load(Ordering::SeqCst) {
                    sink(&[0xFC, 0xFF, 0xFE], pts);
                    pts += 20_000;
                    std::thread::sleep(Duration::from_millis(20));
                }
                running.fetch_sub(1, Ordering::SeqCst);
            })
            .map_err(|e| RpcError::internal(e.to_string()))?;
        Ok(Box::new(FakeAudio {
            stop,
            thread: Some(thread),
        }))
    }

    fn cursor(&self) -> PResult<CursorState> {
        Ok(self.state().cursor)
    }

    fn cursor_shape(&self, handle: u64) -> PResult<Option<CursorShape>> {
        if handle == 0 {
            return Ok(None);
        }
        Ok(Some(CursorShape {
            width: 1,
            height: 1,
            hot_x: 0,
            hot_y: 0,
            png: TINY_PNG.to_vec(),
        }))
    }

    fn new_stats_sampler(&self) -> Arc<dyn StatsSampler> {
        Arc::new(FakeSampler(self.state.clone()))
    }
    fn processes(&self) -> PResult<Vec<ProcessInfo>> {
        Ok(self.state().processes.clone())
    }
    fn kill_process(&self, pid: u32) -> PResult<()> {
        let mut st = self.state();
        let before = st.processes.len();
        st.processes.retain(|p| p.pid != pid);
        if st.processes.len() == before {
            return Err(RpcError::bad_params(format!("not found: process {pid}")));
        }
        drop(st);
        self.record(format!("kill_process:{pid}"));
        Ok(())
    }
    fn set_process_priority(&self, pid: u32, level: Priority) -> PResult<()> {
        let mut st = self.state();
        let Some(p) = st.processes.iter_mut().find(|p| p.pid == pid) else {
            return Err(RpcError::bad_params(format!("not found: process {pid}")));
        };
        p.priority = Some(level);
        drop(st);
        self.record(format!("set_process_priority:{pid}:{level:?}"));
        Ok(())
    }

    fn audio_sessions(&self) -> PResult<Vec<AudioSession>> {
        Ok(self.state().audio_sessions.clone())
    }
    fn set_session_volume(&self, id: &str, level: f32) -> PResult<()> {
        let mut st = self.state();
        let Some(s) = st.audio_sessions.iter_mut().find(|s| s.id == id) else {
            return Err(RpcError::bad_params(format!("not found: session {id}")));
        };
        s.level = level;
        drop(st);
        self.record(format!("set_session_volume:{id}:{level:.2}"));
        Ok(())
    }
    fn set_session_muted(&self, id: &str, muted: bool) -> PResult<()> {
        let mut st = self.state();
        let Some(s) = st.audio_sessions.iter_mut().find(|s| s.id == id) else {
            return Err(RpcError::bad_params(format!("not found: session {id}")));
        };
        s.muted = muted;
        drop(st);
        self.record(format!("set_session_muted:{id}:{muted}"));
        Ok(())
    }
    fn mic_muted(&self) -> PResult<Option<bool>> {
        Ok(self.state().mic_muted)
    }

    fn move_window_to_monitor(&self, hwnd: u64, monitor_index: usize) -> PResult<()> {
        self.known_window(hwnd)?;
        if monitor_index >= self.state().monitors.len() {
            return Err(RpcError::bad_params(format!(
                "not found: monitor {monitor_index}"
            )));
        }
        self.record(format!("move_window:{hwnd}:{monitor_index}"));
        Ok(())
    }
    fn snap_window(&self, hwnd: u64, pos: Snap) -> PResult<()> {
        self.known_window(hwnd)?;
        self.record(format!("snap_window:{hwnd}:{pos:?}"));
        Ok(())
    }
    fn virtual_desktop(&self, action: VirtualDesktop) -> PResult<()> {
        self.record(format!("virtual_desktop:{action:?}"));
        Ok(())
    }

    fn fixed_drives(&self) -> PResult<Vec<std::path::PathBuf>> {
        Ok(self.state().fixed_drives.clone())
    }
    fn local_clock(&self) -> PResult<String> {
        Ok(self.state().clock.clone())
    }

    fn start_foreground_watcher(&self, sink: ForegroundSink) -> WatchGuard {
        *self.foreground.lock().unwrap() = Some(sink);
        Box::new(())
    }

    fn start_recorder(
        &self,
        path: &std::path::Path,
        config: RecordConfig,
    ) -> PResult<Arc<dyn Recorder>> {
        self.record(format!(
            "start_recorder:{}x{}@{}:audio={}",
            config.width, config.height, config.fps, config.audio
        ));
        let rec = Arc::new(FakeRecording {
            path: path.to_path_buf(),
            config,
            video_frames: AtomicU64::new(0),
            keyframes: AtomicU64::new(0),
            audio_packets: AtomicU64::new(0),
            stopped: AtomicBool::new(false),
            pts: Mutex::new((None, 0)),
        });
        self.recordings.lock().unwrap().push(rec.clone());
        Ok(rec)
    }

    fn open_terminal(
        &self,
        shell: Shell,
        cols: u16,
        rows: u16,
        on_output: TermOutput,
        on_exit: TermExit,
    ) -> PResult<Box<dyn TermSession>> {
        helm_win_terminal::validate_size(cols, rows)
            .map_err(|e| RpcError::bad_params(e.to_string()))?;
        self.record(format!("open_terminal:{shell:?}:{cols}x{rows}"));
        let term = Arc::new(FakeTerm {
            shell,
            size: Mutex::new((cols, rows)),
            input: Mutex::new(Vec::new()),
            output: Mutex::new(Some(on_output)),
            exit: Mutex::new(Some(on_exit)),
            killed: AtomicBool::new(false),
        });
        self.terminals.lock().unwrap().push(term.clone());
        Ok(Box::new(FakeTermHandle(term)))
    }

    fn new_pen(&self) -> PResult<Box<dyn PenDevice>> {
        if !self.state().pen_supported {
            return Err(RpcError::unsupported("fake: no pen"));
        }
        self.record("new_pen".into());
        Ok(Box::new(FakePen {
            samples: self.pen.clone(),
            releases: self.pen_releases.clone(),
        }))
    }

    fn show_toast(&self, toast: &Toast, on_event: ToastCallback) -> PResult<()> {
        helm_win_notify::model::validate_toast(toast).map_err(|e| RpcError::bad_params(e.to_string()))?;
        self.record(format!("show_toast:{}", toast.title));
        self.toasts
            .lock()
            .unwrap()
            .push(FakeToast(toast.clone(), Arc::new(on_event)));
        Ok(())
    }

    fn http(&self) -> Arc<dyn HttpTransport> {
        self.http.clone()
    }

    fn wifi_ssid(&self) -> PResult<Option<String>> {
        Ok(self.state().wifi_ssid.clone())
    }

    fn utc_offset_minutes(&self) -> i32 {
        self.state().utc_offset_minutes
    }
}

// ---------------------------------------------------------------- Phase 5 fakes

/// A fake MP4 recording: counts what it was fed; `stop` writes a small placeholder file.
#[derive(Debug)]
pub struct FakeRecording {
    pub path: std::path::PathBuf,
    pub config: RecordConfig,
    pub video_frames: AtomicU64,
    pub keyframes: AtomicU64,
    pub audio_packets: AtomicU64,
    pub stopped: AtomicBool,
    /// (first video pts, last video pts)
    pts: Mutex<(Option<u64>, u64)>,
}

impl Recorder for FakeRecording {
    fn push_video(&self, _annex_b: &[u8], is_key: bool, pts_us: u64) {
        let mut pts = self.pts.lock().unwrap();
        if pts.0.is_none() && !is_key {
            return; // like the real one: the file starts at a keyframe
        }
        pts.0.get_or_insert(pts_us);
        pts.1 = pts_us;
        self.video_frames.fetch_add(1, Ordering::SeqCst);
        if is_key {
            self.keyframes.fetch_add(1, Ordering::SeqCst);
        }
    }
    fn push_audio_opus(&self, _packet: &[u8], _pts_us: u64) {
        self.audio_packets.fetch_add(1, Ordering::SeqCst);
    }
    fn stop(&self) -> PResult<RecordedFile> {
        if self.stopped.swap(true, Ordering::SeqCst) {
            return Err(RpcError::internal("fake: already stopped"));
        }
        let (first, last) = *self.pts.lock().unwrap();
        let Some(first) = first else {
            return Err(RpcError::internal(
                "no video frame was received; nothing was recorded",
            ));
        };
        std::fs::write(&self.path, b"fake mp4").map_err(|e| RpcError::internal(e.to_string()))?;
        Ok(RecordedFile {
            path: self.path.to_string_lossy().into_owned(),
            duration_ms: last.saturating_sub(first) / 1000,
        })
    }
}

/// A fake shell: echoes what is written; writing `exit` ends it with code 0, kill with 1.
pub struct FakeTerm {
    pub shell: Shell,
    pub size: Mutex<(u16, u16)>,
    pub input: Mutex<Vec<u8>>,
    output: Mutex<Option<TermOutput>>,
    exit: Mutex<Option<TermExit>>,
    pub killed: AtomicBool,
}

impl std::fmt::Debug for FakeTerm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeTerm").field("shell", &self.shell).finish()
    }
}

impl FakeTerm {
    fn end(self: &Arc<Self>, code: u32) {
        let this = self.clone();
        std::thread::spawn(move || {
            this.output.lock().unwrap().take();
            let exit = this.exit.lock().unwrap().take();
            if let Some(exit) = exit {
                exit(code);
            }
        });
    }

    /// False once the fake shell ended.
    pub fn is_running(&self) -> bool {
        self.exit.lock().unwrap().is_some()
    }
}

struct FakeTermHandle(Arc<FakeTerm>);

impl TermSession for FakeTermHandle {
    fn write(&self, bytes: &[u8]) -> PResult<()> {
        let t = &self.0;
        t.input.lock().unwrap().extend_from_slice(bytes);
        if let Some(out) = t.output.lock().unwrap().as_mut() {
            out(bytes);
        }
        if bytes.windows(4).any(|w| w == b"exit") {
            t.end(0);
        }
        Ok(())
    }
    fn resize(&self, cols: u16, rows: u16) -> PResult<()> {
        helm_win_terminal::validate_size(cols, rows)
            .map_err(|e| RpcError::bad_params(e.to_string()))?;
        *self.0.size.lock().unwrap() = (cols, rows);
        Ok(())
    }
    fn kill(&self) -> PResult<()> {
        if !self.0.killed.swap(true, Ordering::SeqCst) {
            self.0.end(1);
        }
        Ok(())
    }
}

impl Drop for FakeTermHandle {
    fn drop(&mut self) {
        if self.0.is_running() {
            let _ = self.kill();
        }
    }
}

struct FakePen {
    samples: Arc<Mutex<Vec<PenSample>>>,
    releases: Arc<AtomicU64>,
}

impl PenDevice for FakePen {
    fn inject(&self, sample: &PenSample) -> PResult<()> {
        self.samples.lock().unwrap().push(*sample);
        Ok(())
    }
    fn release(&self) -> PResult<()> {
        Ok(())
    }
}

impl Drop for FakePen {
    fn drop(&mut self) {
        self.releases.fetch_add(1, Ordering::SeqCst);
    }
}

/// `(method, url, status, body)`.
type Route = (String, String, u16, Vec<u8>);

/// Scripted HTTP: `(method, url)` → `(status, body)`; anything else gets 404.
#[derive(Debug, Default)]
pub struct FakeHttp {
    routes: Mutex<Vec<Route>>,
    requests: Mutex<Vec<HttpRequest>>,
}

impl FakeHttp {
    pub fn route(&self, method: &str, url: &str, status: u16, body: &str) {
        self.routes.lock().unwrap().push((
            method.into(),
            url.into(),
            status,
            body.as_bytes().to_vec(),
        ));
    }
    pub fn requests(&self) -> Vec<HttpRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl HttpTransport for FakeHttp {
    fn send(&self, req: &HttpRequest) -> Result<HttpResponse, HttpError> {
        self.requests.lock().unwrap().push(req.clone());
        let routes = self.routes.lock().unwrap();
        let (status, body) = routes
            .iter()
            .rev()
            .find(|(m, u, _, _)| *m == req.method && *u == req.url)
            .map(|(_, _, s, b)| (*s, b.clone()))
            .unwrap_or((404, b"not found".to_vec()));
        Ok(HttpResponse {
            status,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body,
        })
    }
}

