//! Error type of the crate.

use std::fmt;

/// Everything that can go wrong in this crate.
#[derive(Debug)]
pub enum Error {
    /// A Windows API call failed. `context` names the call (e.g. `"CreatePseudoConsole"`).
    #[cfg(windows)]
    Windows {
        context: &'static str,
        source: windows::core::Error,
    },
    /// Bad argument (size out of range, working directory missing, empty command line, ...).
    InvalidConfig(String),
    /// The session has already ended (the shell exited or was killed).
    Closed,
    /// Not available on this platform (every non-Windows target).
    Unsupported(String),
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
            Error::Closed => write!(f, "terminal session has ended"),
            Error::Unsupported(m) => write!(f, "unsupported: {m}"),
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
