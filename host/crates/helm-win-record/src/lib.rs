//! # helm-win-record
//!
//! MP4 recording for the Helm PC agent. Feed it the H.264 Annex-B access units that
//! `helm-win-capture` produces (passed through, not re-encoded) and optional interleaved stereo
//! f32 PCM at 48 kHz (encoded to AAC by Media Foundation); it writes an `.mp4` with the Media
//! Foundation sink writer.
//!
//! ```no_run
//! # #[cfg(windows)] fn demo() -> helm_win_record::Result<()> {
//! use helm_win_record::{Recorder, VideoConfig};
//! let rec = Recorder::start("C:\\Users\\me\\Videos\\helm.mp4", VideoConfig { width: 1920, height: 1080, fps: 60 }, true)?;
//! # let (au, key, pts, pcm, apts): (Vec<u8>, bool, u64, Vec<f32>, u64) = Default::default();
//! rec.push_video(&au, key, pts)?;   // from the Streamer callback
//! rec.push_audio(&pcm, apts)?;      // from the audio capture
//! let info = rec.stop()?;           // finalizes the file
//! # Ok(()) }
//! ```
//!
//! [`pure`] (Annex-B parsing, PCM conversion, timeline) is tested everywhere. On non-Windows
//! targets [`Recorder::start`] returns [`Error::Unsupported`].

pub mod pure;

pub use pure::{AUDIO_CHANNELS, AUDIO_RATE, RecordingInfo, VideoConfig};

use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// A Windows / Media Foundation call failed.
    Windows { context: &'static str, code: i32, message: String },
    InvalidInput(String),
    /// The recording already stopped (or its worker failed); no more samples accepted.
    Stopped(String),
    /// `stop()` was called before any keyframe arrived, so there is nothing to write.
    NoVideo,
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Windows { context, code, message } => {
                write!(f, "{context} failed: {message} (0x{:08X})", *code as u32)
            }
            Error::InvalidInput(m) => write!(f, "invalid input: {m}"),
            Error::Stopped(m) => write!(f, "recording stopped: {m}"),
            Error::NoVideo => write!(f, "no keyframe was received; nothing recorded"),
            Error::Unsupported(m) => write!(f, "unsupported: {m}"),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(windows)]
mod probe;
#[cfg(windows)]
mod win;
#[cfg(windows)]
pub use probe::{MediaInfo, StreamInfo, probe};
#[cfg(windows)]
pub use win::Recorder;

#[cfg(not(windows))]
pub struct Recorder(());

#[cfg(not(windows))]
impl Recorder {
    pub fn start(_path: impl AsRef<std::path::Path>, _video: VideoConfig, _audio: bool) -> Result<Recorder> {
        Err(Error::Unsupported("recording needs Windows Media Foundation".into()))
    }
    pub fn push_video(&self, _au: &[u8], _is_key: bool, _pts_us: u64) -> Result<()> {
        Err(Error::Unsupported("recording needs Windows".into()))
    }
    pub fn push_audio(&self, _samples: &[f32], _pts_us: u64) -> Result<()> {
        Err(Error::Unsupported("recording needs Windows".into()))
    }
    pub fn stop(&self) -> Result<RecordingInfo> {
        Err(Error::Unsupported("recording needs Windows".into()))
    }
}
