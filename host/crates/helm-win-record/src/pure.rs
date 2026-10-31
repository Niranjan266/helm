//! Pure helpers: Annex-B parsing, PCM conversion, timestamp mapping, config checks.

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// Audio format the recorder accepts: interleaved stereo f32 at 48 kHz.
pub const AUDIO_RATE: u32 = 48_000;
pub const AUDIO_CHANNELS: u32 = 2;

/// Video stream description.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

impl VideoConfig {
    pub fn validate(&self) -> Result<()> {
        if self.width < 16 || self.height < 16 || self.width > 8192 || self.height > 8192 {
            return Err(Error::InvalidInput(format!("video size {}x{} out of range", self.width, self.height)));
        }
        if self.width % 2 != 0 || self.height % 2 != 0 {
            return Err(Error::InvalidInput("video width and height must be even".into()));
        }
        if self.fps == 0 || self.fps > 240 {
            return Err(Error::InvalidInput(format!("fps {} out of range 1..=240", self.fps)));
        }
        Ok(())
    }

    /// One frame in 100 ns units.
    pub fn frame_duration_hns(&self) -> i64 {
        10_000_000 / self.fps.max(1) as i64
    }
}

/// Result of [`crate::Recorder::stop`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingInfo {
    pub path: String,
    pub duration_ms: u64,
    pub video_frames: u64,
    pub audio_frames: u64,
    /// Access units dropped because they arrived before the first keyframe.
    pub video_dropped: u64,
}

/// Iterate NAL unit payloads (without start codes) of an Annex-B buffer.
pub fn nal_units(buf: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= buf.len() {
        if buf[i] == 0 && buf[i + 1] == 0 && buf[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut out = Vec::with_capacity(starts.len());
    for (k, &s) in starts.iter().enumerate() {
        let mut end = if k + 1 < starts.len() { starts[k + 1] - 3 } else { buf.len() };
        // Strip zero bytes before the next start code (4-byte start codes / trailing zeros).
        while end > s && buf[end - 1] == 0 {
            end -= 1;
        }
        if end > s {
            out.push(&buf[s..end]);
        }
    }
    out
}

/// NAL unit type of a payload.
pub fn nal_type(nal: &[u8]) -> u8 {
    nal.first().map_or(0, |b| b & 0x1F)
}

/// True if the access unit contains an IDR slice.
pub fn has_idr(au: &[u8]) -> bool {
    nal_units(au).iter().any(|n| nal_type(n) == 5)
}

/// SPS + PPS of an access unit, re-emitted as Annex-B (`00 00 00 01` start codes): the form
/// Media Foundation uses for `MF_MT_MPEG_SEQUENCE_HEADER`. `None` if either is missing.
pub fn sequence_header(au: &[u8]) -> Option<Vec<u8>> {
    let nals = nal_units(au);
    let sps = nals.iter().find(|n| nal_type(n) == 7)?;
    let pps = nals.iter().find(|n| nal_type(n) == 8)?;
    let mut out = Vec::with_capacity(sps.len() + pps.len() + 8);
    for n in [sps, pps] {
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(n);
    }
    Some(out)
}

/// f32 [-1, 1] → i16 PCM (clamped, NaN → 0).
pub fn f32_to_i16(samples: &[f32], out: &mut Vec<i16>) {
    out.reserve(samples.len());
    out.extend(samples.iter().map(|&s| if s.is_nan() { 0 } else { (s.clamp(-1.0, 1.0) * 32767.0).round() as i16 }));
}

/// Maps caller timestamps (µs, any origin) to file time (100 ns from 0). The origin is the
/// first keyframe; video before it is dropped, audio before it is dropped. Video times are
/// forced strictly increasing.
#[derive(Debug, Default)]
pub struct Timeline {
    base_us: Option<u64>,
    last_video: Option<i64>,
}

impl Timeline {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn started(&self) -> bool {
        self.base_us.is_some()
    }

    /// File time for a video access unit, or `None` to drop it.
    pub fn video(&mut self, pts_us: u64, is_key: bool) -> Option<i64> {
        let base = match self.base_us {
            Some(b) => b,
            None if is_key => {
                self.base_us = Some(pts_us);
                pts_us
            }
            None => return None,
        };
        let mut t = (pts_us.saturating_sub(base) as i64) * 10;
        if let Some(last) = self.last_video {
            if t <= last {
                t = last + 1;
            }
        }
        self.last_video = Some(t);
        Some(t)
    }

    /// File time for an audio chunk, or `None` to drop it (before the first keyframe).
    pub fn audio(&self, pts_us: u64) -> Option<i64> {
        let base = self.base_us?;
        if pts_us < base {
            return None;
        }
        Some(((pts_us - base) as i64) * 10)
    }
}

/// Duration in 100 ns of `frames` stereo frames at 48 kHz.
pub fn audio_duration_hns(frames: u64) -> i64 {
    (frames as i64 * 10_000_000) / AUDIO_RATE as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_validation() {
        assert!(VideoConfig { width: 1920, height: 1080, fps: 60 }.validate().is_ok());
        assert!(VideoConfig { width: 1921, height: 1080, fps: 60 }.validate().is_err());
        assert!(VideoConfig { width: 8, height: 8, fps: 30 }.validate().is_err());
        assert!(VideoConfig { width: 640, height: 480, fps: 0 }.validate().is_err());
        assert_eq!(VideoConfig { width: 640, height: 480, fps: 30 }.frame_duration_hns(), 333_333);
    }

    #[test]
    fn nal_splitting_and_sequence_header() {
        let au = [0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 9, 9, 0, 0];
        let n = nal_units(&au);
        assert_eq!(n, vec![&[0x67, 1, 2][..], &[0x68, 3][..], &[0x65, 9, 9][..]]);
        assert!(has_idr(&au));
        assert_eq!(sequence_header(&au).unwrap(), vec![0, 0, 0, 1, 0x67, 1, 2, 0, 0, 0, 1, 0x68, 3]);
        let p = [0, 0, 1, 0x41, 5];
        assert!(!has_idr(&p));
        assert!(sequence_header(&p).is_none());
        assert!(nal_units(&[1, 2, 3]).is_empty());
    }

    #[test]
    fn pcm_conversion() {
        let mut out = Vec::new();
        f32_to_i16(&[0.0, 1.0, -1.0, 2.0, -3.0, 0.5, f32::NAN], &mut out);
        assert_eq!(out, vec![0, 32767, -32767, 32767, -32767, 16384, 0]);
    }

    #[test]
    fn timeline_starts_at_first_keyframe() {
        let mut t = Timeline::new();
        assert_eq!(t.audio(500), None);
        assert_eq!(t.video(900, false), None);
        assert_eq!(t.video(1_000, true), Some(0));
        assert!(t.started());
        assert_eq!(t.video(34_333, false), Some(333_330));
        // Non-increasing input is nudged forward.
        assert_eq!(t.video(34_333, false), Some(333_331));
        assert_eq!(t.video(2_000, false), Some(333_332));
        assert_eq!(t.audio(999), None);
        assert_eq!(t.audio(11_000), Some(100_000));
        assert_eq!(audio_duration_hns(480), 100_000);
    }

    #[test]
    fn info_serde() {
        let i = RecordingInfo { path: "a.mp4".into(), duration_ms: 2000, video_frames: 60, audio_frames: 96000, video_dropped: 0 };
        let s = serde_json::to_string(&i).unwrap();
        assert!(s.contains("\"duration_ms\":2000"));
    }
}
