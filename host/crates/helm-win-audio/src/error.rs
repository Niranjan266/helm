//! Error type shared by every part of the crate.

use std::fmt;

/// Everything that can go wrong in this crate.
#[derive(Debug)]
pub enum Error {
    /// A Windows API call failed. `context` names the call (e.g. `"IAudioClient::Initialize"`).
    #[cfg(windows)]
    Windows {
        context: &'static str,
        source: windows::core::Error,
    },
    /// An `AudioConfig` (or other input) is out of range.
    InvalidConfig(String),
    /// The Opus encoder or decoder returned an error code.
    Opus { context: &'static str, code: i32 },
    /// The device mix format is not something we can convert.
    UnsupportedFormat(String),
    /// No audio render endpoint is present.
    NoDevice,
    /// The streaming thread stopped or could not start.
    Stopped(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(windows)]
            Error::Windows { context, source } => {
                write!(
                    f,
                    "{context} failed: {} (0x{:08X})",
                    source.message(),
                    source.code().0 as u32
                )
            }
            Error::InvalidConfig(m) => write!(f, "invalid configuration: {m}"),
            Error::Opus { context, code } => write!(f, "{context} failed: {} ({code})", crate::opus::error_name(*code)),
            Error::UnsupportedFormat(m) => write!(f, "unsupported device format: {m}"),
            Error::NoDevice => write!(f, "no audio playback device"),
            Error::Stopped(m) => write!(f, "audio stream stopped: {m}"),
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

/// `result.ctx("Call")` turns a `windows::core::Result` into our `Result`.
#[cfg(windows)]
pub(crate) trait Ctx<T> {
    fn ctx(self, context: &'static str) -> Result<T>;
}

#[cfg(windows)]
impl<T> Ctx<T> for windows::core::Result<T> {
    fn ctx(self, context: &'static str) -> Result<T> {
        self.map_err(|source| Error::Windows { context, source })
    }
}
