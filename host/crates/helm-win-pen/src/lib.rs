//! # helm-win-pen
//!
//! Pen (stylus) injection for the Helm PC agent: pressure (0..=1024), tilt, rotation, eraser and
//! barrel button, with hover / contact / up / leave, through a synthetic pointer device
//! (`CreateSyntheticPointerDevice(PT_PEN)` + `InjectSyntheticPointerInput`, Windows 10 1809+).
//!
//! [`mapping`] is pure (sample → Win32 frame sequence) and tested everywhere. [`Pen`] is the
//! Windows device; on other platforms `Pen::new` returns [`Error::Unsupported`].
//!
//! Coordinates are **physical** virtual-desktop pixels: make the process per-monitor-v2 DPI aware.

pub mod mapping;

pub use mapping::{PenFrame, PenPhase, PenSample, PenTracker, ScreenRect, map_normalized};

use std::fmt;

/// Errors from this crate.
#[derive(Debug)]
pub enum Error {
    /// A Windows API call failed.
    Windows { context: &'static str, code: i32, message: String },
    /// Not available on this platform / Windows version.
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Windows { context, code, message } => {
                write!(f, "{context} failed: {message} (0x{:08X})", *code as u32)
            }
            Error::Unsupported(m) => write!(f, "unsupported: {m}"),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(windows)]
mod win;
#[cfg(windows)]
pub use win::Pen;

/// Stub for non-Windows builds.
#[cfg(not(windows))]
pub struct Pen(());

#[cfg(not(windows))]
impl Pen {
    pub fn new() -> Result<Pen> {
        Err(Error::Unsupported("pen injection needs Windows".into()))
    }
    pub fn inject(&self, _sample: &PenSample) -> Result<()> {
        Err(Error::Unsupported("pen injection needs Windows".into()))
    }
    pub fn release(&self) -> Result<()> {
        Ok(())
    }
}
