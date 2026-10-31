//! Error type shared by every part of the crate.

use std::fmt;

/// Everything that can go wrong in this crate.
#[derive(Debug)]
pub enum Error {
    /// A Windows API call failed. `context` names the call (e.g. `"BitBlt"`).
    #[cfg(windows)]
    Windows {
        context: &'static str,
        source: windows::core::Error,
    },
    /// A `StreamConfig` (or other input) is out of range.
    InvalidConfig(String),
    /// No monitor matches the requested id.
    MonitorNotFound(String),
    /// PNG encoding failed.
    Png(String),
    /// No usable H.264 encoder, or the encoder rejected every configuration we tried.
    Encoder(String),
    /// The feature is not available on this system (e.g. Windows.Graphics.Capture missing).
    Unsupported(String),
    /// The streaming pipeline stopped (source closed, or a worker thread failed).
    Stopped(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(windows)]
            Error::Windows { context, source } => {
                write!(f, "{context} failed: {} (0x{:08X})", source.message(), source.code().0 as u32)
            }
            Error::InvalidConfig(m) => write!(f, "invalid configuration: {m}"),
            Error::MonitorNotFound(id) => write!(f, "monitor not found: {id}"),
            Error::Png(m) => write!(f, "PNG encoding failed: {m}"),
            Error::Encoder(m) => write!(f, "H.264 encoder error: {m}"),
            Error::Unsupported(m) => write!(f, "unsupported: {m}"),
            Error::Stopped(m) => write!(f, "stream stopped: {m}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            #[cfg(windows)]
            Error::Windows { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Attach the failing API name to a `windows::core::Result`.
#[cfg(windows)]
pub(crate) trait Context<T> {
    fn ctx(self, context: &'static str) -> Result<T>;
}

#[cfg(windows)]
impl<T> Context<T> for windows::core::Result<T> {
    fn ctx(self, context: &'static str) -> Result<T> {
        self.map_err(|source| Error::Windows { context, source })
    }
}
