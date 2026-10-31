//! A fake PC for tests: canned data in, every call recorded, nothing touches the real system.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

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
        }
    }
}

#[derive(Debug, Default)]
pub struct FakePlatform {
    state: Mutex<FakeState>,
    calls: Mutex<Vec<String>>,
    ops: Arc<Mutex<Vec<Op>>>,
    block_input: Arc<AtomicBool>,
    events: Mutex<Option<EventSink>>,
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

    pub fn watchers_running(&self) -> bool {
        self.events.lock().unwrap().is_some()
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
}
