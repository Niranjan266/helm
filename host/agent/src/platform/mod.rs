//! Everything the agent does to the PC goes through the [`Platform`] trait, so the
//! integration tests can run the full protocol against [`fake::FakePlatform`] without
//! touching this computer (no input, no launches, no volume / clipboard / media changes).
//!
//! All methods are **blocking**; callers use `tokio::task::spawn_blocking`.

use std::fmt::Debug;
use std::sync::Arc;

use serde::Serialize;
use tokio::sync::mpsc;

pub use helm_win_apps::{AppEntry, AppKind, FocusResult, WindowInfo};
pub use helm_win_input::{Injector, InputEvent};
pub use helm_win_system::audio::VolumeState;
pub use helm_win_system::display::Display;
pub use helm_win_system::info::Battery;
pub use helm_win_system::media::{MediaAction, MediaState};
pub use helm_win_system::net::Adapter;
// Phase 3
pub use helm_win_sysmon::foreground::ForegroundInfo;
pub use helm_win_sysmon::process::{Priority, ProcessInfo};
pub use helm_win_sysmon::stats::Stats;
pub use helm_win_sysmon::window::{Snap, VirtualDesktop};
pub use helm_win_system::audio::AudioSession;

use crate::router::RpcError;

pub mod fake;
pub mod real;

pub type PResult<T> = Result<T, RpcError>;

/// What the window commands do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowCommand {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// One monitor, physical pixels of the virtual desktop.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Monitor {
    pub id: String,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub is_primary: bool,
}

/// A rectangle on the virtual desktop (physical pixels).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl From<&Monitor> for Rect {
    fn from(m: &Monitor) -> Rect {
        Rect {
            x: m.x,
            y: m.y,
            width: m.width,
            height: m.height,
        }
    }
}

/// What a screen stream captures.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VideoSource {
    /// A monitor by id; `None` = the primary monitor.
    Monitor(Option<String>),
    /// A top-level window by `hwnd`.
    Window(u64),
}

/// Parameters of a screen stream (already range-checked by the agent).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoParams {
    pub source: VideoSource,
    pub max_width: u32,
    pub max_height: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
}

/// Facts about a running screen stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoInfo {
    /// Encoded picture size.
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub hardware: bool,
    pub encoder_name: String,
}

/// Encoded H.264 access units: `(annex_b, is_keyframe, pts_us)`. Called on the encoder
/// thread; must not block (copy into a queue).
pub type AccessUnitSink = Box<dyn FnMut(&[u8], bool, u64) + Send + 'static>;
/// Opus packets (48 kHz stereo, 20 ms): `(packet, pts_us)`. Same rules as above.
pub type AudioSink = Box<dyn FnMut(&[u8], u64) + Send + 'static>;

/// A running screen stream. Dropping it stops capture and encoding (may block briefly:
/// drop it in `spawn_blocking`).
pub trait VideoStream: Send + Sync {
    fn info(&self) -> VideoInfo;
    /// Next frame becomes an IDR with SPS/PPS.
    fn request_keyframe(&self);
    /// Change the encoder's CBR target while running.
    fn set_bitrate(&self, kbps: u32);
    /// False once the stream stopped by itself (monitor unplugged, window closed, error).
    fn is_running(&self) -> bool;
    fn ended_reason(&self) -> Option<String>;
    /// Where the captured source is on the virtual desktop now (cursor / `ma` mapping).
    fn source_rect(&self) -> Option<Rect>;
}

/// A running system-audio stream. Dropping it stops it.
pub trait AudioStream: Send + Sync {
    fn is_running(&self) -> bool;
    /// Opus pre-skip in 48 kHz samples (the encoder look-ahead), for the phone decoder.
    fn pre_skip(&self) -> u32;
}

/// The mouse pointer, virtual-desktop pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct CursorState {
    pub x: i32,
    pub y: i32,
    pub visible: bool,
    /// Opaque id of the current cursor image (the `HCURSOR` value on Windows).
    pub handle: u64,
}

/// A cursor image as PNG with its hotspot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CursorShape {
    pub width: u16,
    pub height: u16,
    pub hot_x: u16,
    pub hot_y: u16,
    pub png: Vec<u8>,
}

/// Something changed on the PC (sent by the watchers).
#[derive(Clone, Debug, PartialEq)]
pub enum PlatformEvent {
    Media(Option<MediaState>),
    Volume(VolumeState),
    TextFocus(bool),
    Clipboard { has_text: bool, has_image: bool },
}

pub type EventSink = mpsc::UnboundedSender<PlatformEvent>;

/// Keeps the watchers alive; dropping it stops them.
pub type WatchGuard = Box<dyn Send>;

pub trait Platform: Send + Sync + Debug + 'static {
    fn list_apps(&self) -> PResult<Vec<AppEntry>>;
    fn app_icon_png(&self, app_id: &str, size: u32) -> PResult<Vec<u8>>;
    fn launch_app(&self, app_id: &str) -> PResult<()>;
    /// `target` was already checked by the agent (http/https/mailto URL or existing absolute path).
    fn open_target(&self, target: &str) -> PResult<()>;

    fn list_windows(&self) -> PResult<Vec<WindowInfo>>;
    fn focus_window(&self, hwnd: u64) -> PResult<FocusResult>;
    fn window_command(&self, hwnd: u64, cmd: WindowCommand) -> PResult<()>;

    fn media_state(&self) -> PResult<Option<MediaState>>;
    fn media_artwork_png(&self) -> PResult<Option<Vec<u8>>>;
    fn media_control(&self, action: MediaAction) -> PResult<()>;

    fn volume(&self) -> PResult<VolumeState>;
    fn set_volume(&self, level: f32) -> PResult<()>;
    fn set_muted(&self, muted: bool) -> PResult<()>;

    fn brightness_list(&self) -> PResult<Vec<Display>>;
    fn set_brightness(&self, id: &str, level: u8) -> PResult<()>;

    fn clipboard_text(&self) -> PResult<Option<String>>;
    fn clipboard_png(&self) -> PResult<Option<Vec<u8>>>;
    fn set_clipboard_text(&self, text: &str) -> PResult<()>;
    fn set_clipboard_png(&self, png: &[u8]) -> PResult<()>;

    /// A fresh injector (one per session, one per deck run).
    fn new_injector(&self) -> PResult<Injector>;

    fn network_adapters(&self) -> PResult<Vec<Adapter>>;
    fn battery(&self) -> PResult<Option<Battery>>;
    /// Broadcast a Wake-on-LAN magic packet on this PC's LAN (`wol.send`).
    fn send_wol(&self, packet: &[u8; 102]) -> PResult<()> {
        crate::wol::broadcast(packet)
            .map(|_| ())
            .map_err(|e| RpcError::internal(format!("could not send the magic packet: {e}")))
    }

    fn monitors(&self) -> PResult<Vec<Monitor>>;
    fn screenshot_png(&self, monitor_id: Option<&str>) -> PResult<Vec<u8>>;

    /// Start the change watchers (media, volume, text focus, clipboard).
    fn start_watchers(&self, events: EventSink) -> WatchGuard;

    // ---- Phase 2: streaming ----

    /// Start capturing + encoding. Blocks until the encoder runs (or fails).
    fn start_video(&self, params: VideoParams, sink: AccessUnitSink)
    -> PResult<Box<dyn VideoStream>>;
    /// Start system-audio capture → Opus. `unsupported` when not available.
    fn start_audio(&self, sink: AudioSink) -> PResult<Box<dyn AudioStream>>;
    /// Current pointer position, visibility and image id (cheap; polled ~60 Hz).
    fn cursor(&self) -> PResult<CursorState>;
    /// The image for `handle` (from [`Platform::cursor`]); `None` if it is gone.
    fn cursor_shape(&self, handle: u64) -> PResult<Option<CursorShape>>;
    // ---- Phase 3 (power user) ----

    /// A fresh stats sampler (rates are relative to its own previous sample).
    fn new_stats_sampler(&self) -> Arc<dyn StatsSampler>;
    fn processes(&self) -> PResult<Vec<ProcessInfo>>;
    /// The agent already refused pid 0, 4 and itself.
    fn kill_process(&self, pid: u32) -> PResult<()>;
    fn set_process_priority(&self, pid: u32, level: Priority) -> PResult<()>;

    fn audio_sessions(&self) -> PResult<Vec<AudioSession>>;
    fn set_session_volume(&self, id: &str, level: f32) -> PResult<()>;
    fn set_session_muted(&self, id: &str, muted: bool) -> PResult<()>;
    /// Default microphone muted; `None` when there is no microphone.
    fn mic_muted(&self) -> PResult<Option<bool>>;

    fn move_window_to_monitor(&self, hwnd: u64, monitor_index: usize) -> PResult<()>;
    fn snap_window(&self, hwnd: u64, pos: Snap) -> PResult<()>;
    fn virtual_desktop(&self, action: VirtualDesktop) -> PResult<()>;

    /// Fixed (non-removable, non-network) drive roots such as `C:\`.
    fn fixed_drives(&self) -> PResult<Vec<std::path::PathBuf>>;
    /// Local wall-clock time as `HH:MM` (deck clock widget).
    fn local_clock(&self) -> PResult<String>;

    /// Start the foreground-window watcher (profiles). Dropping the guard stops it.
    fn start_foreground_watcher(&self, sink: ForegroundSink) -> WatchGuard;
}

/// Collects [`Stats`] snapshots (blocking).
pub trait StatsSampler: Send + Sync {
    fn sample(&self) -> PResult<Stats>;
}

pub type ForegroundSink = mpsc::UnboundedSender<ForegroundInfo>;

/// Run a blocking platform call off the async runtime.
pub async fn blocking<T, F>(f: F) -> PResult<T>
where
    F: FnOnce() -> PResult<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| RpcError::internal(format!("platform call failed: {e}")))?
}

pub(crate) fn apps_err(e: helm_win_apps::Error) -> RpcError {
    match e {
        helm_win_apps::Error::InvalidArgument(m) => RpcError::bad_params(m),
        helm_win_apps::Error::NotFound(m) => RpcError::bad_params(format!("not found: {m}")),
        other => RpcError::internal(other.to_string()),
    }
}

pub(crate) fn system_err(e: helm_win_system::Error) -> RpcError {
    match e {
        helm_win_system::Error::InvalidArgument(m) => RpcError::bad_params(m),
        helm_win_system::Error::NotFound(m) => RpcError::bad_params(format!("not found: {m}")),
        helm_win_system::Error::Rejected(m) => RpcError::bad_params(format!("rejected: {m}")),
        other => RpcError::internal(other.to_string()),
    }
}

pub(crate) fn sysmon_err(e: helm_win_sysmon::Error) -> RpcError {
    match e {
        helm_win_sysmon::Error::InvalidArgument(m) => RpcError::bad_params(m),
        helm_win_sysmon::Error::NotFound(m) => RpcError::bad_params(format!("not found: {m}")),
        helm_win_sysmon::Error::Refused(m) => RpcError::forbidden(format!("refused: {m}")),
        other => RpcError::internal(other.to_string()),
    }
}

pub(crate) fn input_err(e: helm_win_input::Error) -> RpcError {
    RpcError::internal(e.to_string())
}
