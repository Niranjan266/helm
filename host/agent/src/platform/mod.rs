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
// Phase 5
pub use helm_plugins::http::{HttpError, HttpRequest, HttpResponse, HttpTransport};
pub use helm_win_notify::{ReplyBox, Toast, ToastAction, ToastEvent};
pub use helm_win_pen::{PenPhase, PenSample};
pub use helm_win_terminal::Shell;

use crate::router::RpcError;

pub mod fake;
pub mod real;
mod real5;
#[cfg(windows)]
mod real_g6;

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

    // ---- Phase 5 (extras) ----

    /// Open an MP4 file for writing at `path` (the folder exists).
    fn start_recorder(&self, path: &std::path::Path, config: RecordConfig)
    -> PResult<Arc<dyn Recorder>>;
    /// Start a shell in a pseudo console (cwd: the user's profile folder).
    fn open_terminal(
        &self,
        shell: Shell,
        cols: u16,
        rows: u16,
        on_output: TermOutput,
        on_exit: TermExit,
    ) -> PResult<Box<dyn TermSession>>;
    /// A synthetic pen device (one per session).
    fn new_pen(&self) -> PResult<Box<dyn PenDevice>>;
    /// Show a Windows toast; `on_event` gets clicks while the agent runs.
    fn show_toast(&self, toast: &Toast, on_event: ToastCallback) -> PResult<()>;
    /// HTTP(S) client for plugins and Home Assistant.
    fn http(&self) -> Arc<dyn HttpTransport>;
    /// SSID of the connected Wi-Fi network (`None` = not on Wi-Fi or not readable).
    fn wifi_ssid(&self) -> PResult<Option<String>>;
    /// Local time minus UTC, in minutes (time triggers).
    fn utc_offset_minutes(&self) -> i32;

    // ---- g6 (everyday, Docs/protocol/everyday.md). Defaults are inert. ----

    /// False when the current clipboard content must not be recorded in the history
    /// (password managers: `ExcludeClipboardContentFromMonitorProcessing`,
    /// `Clipboard Viewer Ignore`, `CanIncludeInClipboardHistory` = 0).
    fn clipboard_history_allowed(&self) -> PResult<bool> {
        Ok(false)
    }
    /// Open Explorer with `path` selected (the agent already checked the path).
    fn reveal_in_explorer(&self, _path: &std::path::Path) -> PResult<()> {
        Err(RpcError::unsupported("reveal in Explorer is not available"))
    }
    /// Ask for / report access to Windows notifications.
    fn pc_notify_access(&self) -> PResult<NotifyAccess> {
        Ok(NotifyAccess::Unsupported)
    }
    /// Toast notifications currently in Action Center.
    fn pc_notifications(&self) -> PResult<Vec<PcNotification>> {
        Ok(Vec::new())
    }
    /// CPU / thermal-zone temperature in °C, if Windows exposes one.
    fn cpu_temp_c(&self) -> PResult<Option<f32>> {
        Ok(None)
    }
    /// Seconds since Windows started.
    fn system_uptime_s(&self) -> PResult<u64> {
        Err(RpcError::unsupported("uptime is not available"))
    }
}

/// Access to Windows notifications (`UserNotificationListener`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NotifyAccess {
    Allowed,
    Denied,
    Unspecified,
    Unsupported,
}

/// One Windows toast notification seen in Action Center.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PcNotification {
    pub id: String,
    pub app: String,
    pub title: String,
    pub body: String,
    pub time_ms: u64,
}

/// Collects [`Stats`] snapshots (blocking).
pub trait StatsSampler: Send + Sync {
    fn sample(&self) -> PResult<Stats>;
}

pub type ForegroundSink = mpsc::UnboundedSender<ForegroundInfo>;

// ---- Phase 5 types ----

/// What an MP4 recording contains (video size is the encoder's picture size).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub audio: bool,
}

/// A finished recording.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedFile {
    pub path: String,
    pub duration_ms: u64,
}

/// An MP4 being written. Pushes never block for long; the first error is kept and returned
/// by [`Recorder::stop`]. Timestamps are on one clock (µs), video starting at a keyframe.
pub trait Recorder: Send + Sync {
    fn push_video(&self, annex_b: &[u8], is_key: bool, pts_us: u64);
    /// One Opus packet (48 kHz stereo, 20 ms) as produced by [`Platform::start_audio`].
    fn push_audio_opus(&self, packet: &[u8], pts_us: u64);
    /// Finalize the file. Blocking.
    fn stop(&self) -> PResult<RecordedFile>;
}

/// Raw VT output of a terminal (called on a reader thread; must not block).
pub type TermOutput = Box<dyn FnMut(&[u8]) + Send + 'static>;
/// The shell ended (exit code), after all output was delivered.
pub type TermExit = Box<dyn FnOnce(u32) + Send + 'static>;

/// One pseudo-console shell. Dropping it kills the shell and everything it started.
pub trait TermSession: Send + Sync {
    fn write(&self, bytes: &[u8]) -> PResult<()>;
    fn resize(&self, cols: u16, rows: u16) -> PResult<()>;
    fn kill(&self) -> PResult<()>;
}

/// A synthetic pen. Dropping it lifts the pen.
pub trait PenDevice: Send {
    /// `sample` is in physical virtual-desktop pixels.
    fn inject(&self, sample: &PenSample) -> PResult<()>;
    fn release(&self) -> PResult<()>;
}

/// Toast activation / dismissal (called on a system thread).
pub type ToastCallback = Box<dyn Fn(ToastEvent) + Send + Sync + 'static>;

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

/// Map a recording error.
pub(crate) fn record_err(e: helm_win_record::Error) -> RpcError {
    match e {
        helm_win_record::Error::InvalidInput(m) => RpcError::bad_params(m),
        helm_win_record::Error::Unsupported(m) => RpcError::unsupported(m),
        helm_win_record::Error::NoVideo => {
            RpcError::internal("no video frame was received; nothing was recorded")
        }
        other => RpcError::internal(other.to_string()),
    }
}

pub(crate) fn input_err(e: helm_win_input::Error) -> RpcError {
    RpcError::internal(e.to_string())
}
