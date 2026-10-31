//! Windows system integration for the Helm PC agent.
//!
//! Modules:
//! - [`media`]: now-playing info, artwork and transport controls (WinRT
//!   `GlobalSystemMediaTransportControlsSessionManager`), plus a change watcher.
//! - [`audio`]: master volume / mute of the default render endpoint (Core Audio),
//!   a change watcher, and per-app sessions.
//! - [`display`]: brightness for internal panels (WMI) and external monitors (DDC/CI).
//! - [`power`]: lock / sleep / hibernate / restart / shut down / sign out, and a
//!   [`power::PowerScheduler`] with a dry-run backend for tests.
//! - [`net`]: adapter MAC/IPv4 list and a Wake-on-LAN magic packet builder.
//! - [`info`]: OS name/version, uptime and battery.
//!
//! # Threading
//!
//! Every function is safe to call from any thread, including tokio worker
//! threads. COM / WinRT work runs on dedicated, lazily started background
//! threads (one per subsystem, initialised as a COM multithreaded apartment),
//! so callers never need to initialise COM themselves. Calls are **blocking**:
//! most return in a few milliseconds, but DDC/CI brightness can take hundreds
//! of milliseconds per monitor, so from async code prefer
//! `tokio::task::spawn_blocking` for [`display`] calls (and it does no harm for
//! the others).
//!
//! Watchers ([`media::MediaWatcher`], [`audio::VolumeWatcher`]) own their own
//! thread and invoke the callback on it; keep callbacks short (e.g. send on a
//! channel). Dropping a watcher stops it.
//!
//! On non-Windows targets the OS-backed functions return
//! [`Error::Unsupported`]; the pure helpers (magic packet, MAC parsing,
//! scheduler with a custom backend, serde types) work everywhere.

#![warn(missing_docs)]

pub mod audio;
pub mod display;
pub mod info;
pub mod media;
pub mod net;
pub mod power;

#[cfg(windows)]
mod worker;

use std::fmt;

/// Errors returned by this crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The feature is not available on this platform or this machine.
    Unsupported(&'static str),
    /// The requested object (display, session, media session...) was not found.
    NotFound(String),
    /// An argument was out of range or malformed.
    InvalidArgument(String),
    /// The OS accepted the call but refused the request (e.g. the media app
    /// does not allow "next").
    Rejected(String),
    /// An OS API failed.
    Os {
        /// HRESULT / Win32 error code (as i32).
        code: i32,
        /// Human readable message.
        message: String,
    },
    /// Internal failure (worker thread gone, panic in a worker job...).
    Internal(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Unsupported(what) => write!(f, "unsupported: {what}"),
            Error::NotFound(what) => write!(f, "not found: {what}"),
            Error::InvalidArgument(what) => write!(f, "invalid argument: {what}"),
            Error::Rejected(what) => write!(f, "rejected: {what}"),
            Error::Os { code, message } => write!(f, "os error 0x{:08X}: {message}", *code as u32),
            Error::Internal(what) => write!(f, "internal error: {what}"),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(windows)]
impl From<windows::core::Error> for Error {
    fn from(e: windows::core::Error) -> Self {
        Error::Os { code: e.code().0, message: e.message().trim().to_string() }
    }
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;
