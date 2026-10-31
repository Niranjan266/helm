//! System monitoring and control for the Helm PC agent (Phase 3: Monitor,
//! Task manager, Profiles, Windows).
//!
//! Modules:
//! - [`stats`]: [`stats::Sampler`] produces [`stats::Stats`] snapshots — CPU
//!   (total and per core), RAM, disks (space + I/O rates), network rates and
//!   GPU load / VRAM.
//! - [`process`]: process list with CPU % / RAM / elevation, [`process::kill`]
//!   and [`process::set_priority`] (both refuse pid 0, pid 4 and this process).
//! - [`foreground`]: [`foreground::ForegroundWatcher`] reports the foreground
//!   app (exe path + window title) whenever it changes.
//! - [`window`]: move a window to another monitor, snap it, and switch /
//!   create / close virtual desktops.
//! - [`alerts`]: a pure threshold evaluator over [`stats::Stats`].
//! - [`rate`]: pure counter-to-rate helpers used by the sampler.
//!
//! # Threading
//!
//! Every function and type is safe to use from any thread. Calls are
//! **blocking**: [`stats::Sampler::sample`] may sleep up to ~200 ms on its
//! first use (CPU usage needs two readings), [`process::processes`] likewise.
//! From async code wrap calls in `tokio::task::spawn_blocking`.
//! [`foreground::ForegroundWatcher`] runs its callback on its own thread and
//! stops when dropped.
//!
//! On non-Windows targets the sampler and process list still work (via
//! `sysinfo`, without GPU data); Windows-only operations return
//! [`Error::Unsupported`].

#![warn(missing_docs)]

pub mod alerts;
pub mod foreground;
pub mod process;
pub mod rate;
pub mod stats;
pub mod window;

#[cfg(windows)]
mod gpu;
#[cfg(windows)]
mod util;

use std::fmt;

/// Errors returned by this crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The feature is not available on this platform.
    Unsupported(&'static str),
    /// The process / window / monitor was not found.
    NotFound(String),
    /// An argument was out of range or malformed.
    InvalidArgument(String),
    /// The request was refused by a safety rule (e.g. killing pid 4) or by
    /// the OS for lack of rights.
    Refused(String),
    /// An OS API failed.
    Os {
        /// HRESULT / Win32 error code (as i32).
        code: i32,
        /// Human readable message.
        message: String,
    },
    /// Internal failure (thread gone, poisoned lock...).
    Internal(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Unsupported(what) => write!(f, "unsupported: {what}"),
            Error::NotFound(what) => write!(f, "not found: {what}"),
            Error::InvalidArgument(what) => write!(f, "invalid argument: {what}"),
            Error::Refused(what) => write!(f, "refused: {what}"),
            Error::Os { code, message } => write!(f, "os error 0x{:08X}: {message}", *code as u32),
            Error::Internal(what) => write!(f, "internal error: {what}"),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(windows)]
impl From<windows::core::Error> for Error {
    fn from(e: windows::core::Error) -> Self {
        // E_ACCESSDENIED / ERROR_ACCESS_DENIED as HRESULT.
        if e.code().0 == 0x8007_0005_u32 as i32 {
            return Error::Refused(format!("access denied: {}", e.message().trim()));
        }
        Error::Os { code: e.code().0, message: e.message().trim().to_string() }
    }
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;
