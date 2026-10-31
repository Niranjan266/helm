//! `helm-win-apps`: app inventory, app icons, launching and top-level window control for
//! the Helm PC agent.
//!
//! # Threading
//!
//! Every public function is **blocking** and may be called from any thread. Internally,
//! all shell/COM/window work runs on one dedicated worker thread that is initialised as a
//! COM single-threaded apartment (STA) and pumps its message queue while idle. Calls are
//! sent to it over a channel and the caller waits for the reply, so callers never need to
//! initialise COM themselves. From async code (tokio), wrap calls in
//! `tokio::task::spawn_blocking`. Calls are serialised; a slow shell operation (e.g. a
//! cold icon extraction) delays the next call.
//!
//! On non-Windows targets every function returns [`Error::Unsupported`].

use serde::{Deserialize, Serialize};

mod cache;
pub mod util;

#[cfg(not(windows))]
mod stub;
#[cfg(windows)]
mod win;
#[cfg(not(windows))]
use stub as win;

pub use cache::IconCache;

/// Errors returned by this crate. No function panics on bad input.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The platform is not Windows.
    #[error("not supported on this platform")]
    Unsupported,
    /// The caller passed a malformed or disallowed argument.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// The app, window or path does not exist (or is not one this crate exposes).
    #[error("not found: {0}")]
    NotFound(String),
    /// A Windows API call failed.
    #[error("{context} failed: {message} (0x{code:08X})")]
    Windows {
        /// What we were doing.
        context: &'static str,
        /// HRESULT (or HRESULT-wrapped Win32 error) as an unsigned value.
        code: u32,
        /// System message text.
        message: String,
    },
    /// Filesystem error (icon cache).
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// PNG encoding error.
    #[error("PNG encoding failed: {0}")]
    Png(String),
    /// The internal STA worker thread is not available.
    #[error("internal worker thread unavailable: {0}")]
    Worker(String),
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// How an app is installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppKind {
    /// MSIX/AppX/Store app; `app_id` is its AUMID.
    Packaged,
    /// Classic Win32 app (Start-menu shortcut, explicit AUMID, or file path).
    Desktop,
}

/// One entry of `shell:AppsFolder` (what the Start menu "All apps" list shows).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppEntry {
    /// Stable id: the item's parsing name inside `shell:AppsFolder`. For packaged apps this
    /// is the AUMID (`Family_publisher!App`); for desktop apps it is an explicit AUMID or a
    /// path such as `{KnownFolderGuid}\notepad.exe`.
    pub app_id: String,
    /// Display name.
    pub name: String,
    /// Packaged or desktop.
    pub kind: AppKind,
    /// Resolved file-system target for desktop apps, when the shell exposes one.
    pub target_path: Option<String>,
}

/// A top-level, user-visible window (roughly: what Alt+Tab / the taskbar shows).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowInfo {
    /// Window handle as an integer. Only meaningful on this PC and only while the window lives.
    pub hwnd: u64,
    /// Window title.
    pub title: String,
    /// Owning process id.
    pub pid: u32,
    /// Full path of the owning process image, if it could be queried.
    pub exe_path: Option<String>,
    /// Matching `AppEntry::app_id`, if known.
    pub app_id: Option<String>,
    /// Window is minimised.
    pub is_minimized: bool,
    /// Window is the current foreground window.
    pub is_foreground: bool,
}

/// Outcome of [`focus_window`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusResult {
    /// `GetForegroundWindow()` equals the target after the attempt.
    pub focused: bool,
    /// Which technique succeeded (or the last one tried): `already_foreground`,
    /// `set_foreground_window`, `attach_thread_input`, `alt_key_input`, or `none`.
    pub method: String,
}

/// Lists every app in `shell:AppsFolder`, deduplicated by id and sorted by name.
pub fn list_apps() -> Result<Vec<AppEntry>> {
    win::list_apps()
}

/// Returns the app's icon as a PNG of at most `size_px` x `size_px` (clamped to 16..=256).
/// `app_id` must be one returned by [`list_apps`].
pub fn app_icon_png(app_id: &str, size_px: u32) -> Result<Vec<u8>> {
    util::validate_app_id(app_id)?;
    win::app_icon_png(app_id, util::clamp_icon_size(size_px))
}

/// Launches an app by its [`AppEntry::app_id`] (must be one returned by [`list_apps`]).
pub fn launch_app(app_id: &str) -> Result<()> {
    util::validate_app_id(app_id)?;
    win::launch_app(app_id)
}

/// Opens a file, folder or URL with its default handler.
///
/// URLs must be `http`, `https` or `mailto`. Paths must be absolute drive-letter paths that
/// exist; UNC/device paths are rejected. Note: opening an executable file runs it, exactly
/// as double-clicking it in Explorer would.
pub fn open_target(target: &str) -> Result<()> {
    let t = util::classify_target(target)?;
    if let util::Target::Path(p) = &t {
        util::ensure_exists(p)?;
    }
    win::open_target(t)
}

/// Lists visible, unowned, non-tool, non-cloaked top-level windows that have a title.
pub fn list_windows() -> Result<Vec<WindowInfo>> {
    win::list_windows()
}

/// Brings a window to the foreground, working around the foreground lock if needed.
/// `hwnd` must be a window that [`list_windows`] would return.
pub fn focus_window(hwnd: u64) -> Result<FocusResult> {
    win::focus_window(hwnd)
}

/// Minimises a window (asynchronously, so a hung app cannot block the caller).
pub fn minimize_window(hwnd: u64) -> Result<()> {
    win::show_window(hwnd, win::ShowCmd::Minimize)
}

/// Maximises a window.
pub fn maximize_window(hwnd: u64) -> Result<()> {
    win::show_window(hwnd, win::ShowCmd::Maximize)
}

/// Restores a minimised or maximised window.
pub fn restore_window(hwnd: u64) -> Result<()> {
    win::show_window(hwnd, win::ShowCmd::Restore)
}

/// Asks a window to close by posting `WM_CLOSE` (the app may show a "save changes?" prompt).
pub fn close_window(hwnd: u64) -> Result<()> {
    win::close_window(hwnd)
}
