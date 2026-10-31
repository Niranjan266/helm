//! Everything the agent does to the PC goes through the [`Platform`] trait, so the
//! integration tests can run the full protocol against [`fake::FakePlatform`] without
//! touching this computer (no input, no launches, no volume / clipboard / media changes).
//!
//! All methods are **blocking**; callers use `tokio::task::spawn_blocking`.

use std::fmt::Debug;

use serde::Serialize;
use tokio::sync::mpsc;

pub use helm_win_apps::{AppEntry, AppKind, FocusResult, WindowInfo};
pub use helm_win_input::{Injector, InputEvent};
pub use helm_win_system::audio::VolumeState;
pub use helm_win_system::display::Display;
pub use helm_win_system::info::Battery;
pub use helm_win_system::media::{MediaAction, MediaState};
pub use helm_win_system::net::Adapter;

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

    fn monitors(&self) -> PResult<Vec<Monitor>>;
    fn screenshot_png(&self, monitor_id: Option<&str>) -> PResult<Vec<u8>>;

    /// Start the change watchers (media, volume, text focus, clipboard).
    fn start_watchers(&self, events: EventSink) -> WatchGuard;
}

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

pub(crate) fn input_err(e: helm_win_input::Error) -> RpcError {
    RpcError::internal(e.to_string())
}
