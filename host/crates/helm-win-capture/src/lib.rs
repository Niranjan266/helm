//! # helm-win-capture
//!
//! Screen capture for the Helm PC agent.
//!
//! * **Phase 1 – stills:** [`monitors`] and [`screenshot_png`] (GDI `BitBlt` → PNG).
//! * **Phase 2 – live:** [`Streamer`] captures a monitor or window with
//!   Windows.Graphics.Capture, converts BGRA → NV12 on the GPU with the D3D11 video processor
//!   and encodes low-latency H.264 with a Media Foundation encoder MFT (hardware when present),
//!   delivering Annex-B access units to a callback.
//!
//! All coordinates are physical pixels (see [`dpi`]). See `README.md` for the architecture,
//! threading model and how the agent should use this crate.

pub mod annexb;
pub mod config;
pub mod error;
pub mod pixels;
pub mod stats;

#[cfg(windows)]
pub mod capture;
#[cfg(windows)]
pub mod convert;
#[cfg(windows)]
pub mod d3d;
#[cfg(windows)]
pub mod dpi;
#[cfg(windows)]
pub mod encoder;
#[cfg(windows)]
mod monitors;
#[cfg(windows)]
mod screenshot;
#[cfg(windows)]
mod streamer;

pub use config::{EncoderPreference, StreamConfig, StreamSource};
pub use error::{Error, Result};
pub use stats::StreamStats;

#[cfg(windows)]
pub use monitors::{MonitorInfo, monitors};
#[cfg(windows)]
pub use screenshot::{
    RgbaImage, encode_png, screenshot_png, screenshot_primary_rgba, screenshot_rect_rgba, screenshot_rgba,
    virtual_desktop_rect,
};
#[cfg(windows)]
pub use encoder::{EncoderInfo, list_h264_encoders};
#[cfg(windows)]
pub use streamer::{AccessUnitCallback, StreamInfo, Streamer};
