//! # helm-win-audio
//!
//! System-audio streaming for the Helm PC agent (roadmap Phase 2): WASAPI shared-mode
//! **loopback** of the default playback device → down/up-mix → 48 kHz → 20 ms frames → Opus
//! packets delivered to a callback, timestamped on the QPC clock.
//!
//! ```no_run
//! use helm_win_audio::{AudioConfig, AudioStreamer};
//! let s = AudioStreamer::start(AudioConfig::default(), |packet: &[u8], pts_us: u64| {
//!     // one Opus packet (20 ms); copy it into a bounded channel
//! })?;
//! println!("{:?}", s.stats());
//! let final_stats = s.stop();
//! # Ok::<(), helm_win_audio::Error>(())
//! ```
//!
//! The pure parts (format conversion, mixing, resampling, framing, timeline, Opus, Ogg) build
//! and test on any OS. See `README.md` for the design and how the agent should use this crate.

pub mod config;
pub mod error;
pub mod format;
pub mod framer;
pub mod mix;
pub mod ogg;
pub mod opus;
pub mod resample;
pub mod stats;

#[cfg(windows)]
mod streamer;
#[cfg(windows)]
pub mod wasapi;

pub use config::{AudioConfig, OpusMode};
pub use error::{Error, Result};
pub use opus::{Decoder, Encoder, decode, encode};
pub use stats::{AudioStats, DeviceInfo};

#[cfg(windows)]
pub use streamer::{AudioStreamer, PacketCallback};
#[cfg(windows)]
pub use wasapi::{default_device, qpc_now_us};
