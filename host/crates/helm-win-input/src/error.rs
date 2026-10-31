use std::fmt;

/// Errors returned by this crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The feature is not available on this platform (non-Windows builds).
    Unsupported,
    /// `SendInput` inserted nothing and Windows reported access denied (or no reason).
    ///
    /// This is what User Interface Privilege Isolation looks like: the foreground window
    /// belongs to an elevated (administrator) process, or a secure desktop such as the UAC
    /// prompt or the lock screen is active. Running the agent elevated avoids the former.
    BlockedByElevation,
    /// `SendInput` failed for another reason. `inserted` of `requested` events got through.
    SendInputFailed {
        code: u32,
        inserted: u32,
        requested: u32,
    },
    /// The clipboard stayed open in another program for every retry.
    ClipboardBusy,
    /// Image data could not be decoded or encoded.
    Image(String),
    /// A Windows API call failed (`code` is the HRESULT or Win32 error, `context` names the call).
    Os { code: i32, context: &'static str },
    /// A worker thread could not be started or stopped cleanly.
    Thread(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Unsupported => write!(f, "not supported on this platform"),
            Error::BlockedByElevation => write!(
                f,
                "input was blocked: the target window is elevated or a secure desktop is active"
            ),
            Error::SendInputFailed {
                code,
                inserted,
                requested,
            } => write!(
                f,
                "SendInput inserted {inserted} of {requested} events (error {code})"
            ),
            Error::ClipboardBusy => write!(f, "the clipboard is in use by another program"),
            Error::Image(msg) => write!(f, "image conversion failed: {msg}"),
            Error::Os { code, context } => write!(f, "{context} failed (0x{:08X})", *code as u32),
            Error::Thread(msg) => write!(f, "worker thread error: {msg}"),
        }
    }
}

impl std::error::Error for Error {}

/// Result alias used across the crate.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(windows)]
impl Error {
    pub(crate) fn os(err: windows_core::Error, context: &'static str) -> Self {
        Error::Os {
            code: err.code().0,
            context,
        }
    }
}
