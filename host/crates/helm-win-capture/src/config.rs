//! Stream configuration and its validation (pure Rust).

use crate::error::{Error, Result};

/// What to capture.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum StreamSource {
    /// The primary monitor.
    #[default]
    PrimaryMonitor,
    /// A monitor by [`crate::MonitorInfo::id`] (e.g. `\\.\DISPLAY1`).
    Monitor(String),
    /// A top-level window by its `HWND` value.
    Window(isize),
}

/// Which kind of H.264 encoder to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EncoderPreference {
    /// Hardware MFT (NVENC / AMF / Quick Sync) when one works, else the Microsoft software MFT.
    #[default]
    Auto,
    /// Fail instead of falling back to software.
    HardwareOnly,
    /// Always use the Microsoft software H.264 MFT.
    SoftwareOnly,
}

/// Parameters for [`crate::Streamer::start`].
#[derive(Debug, Clone, PartialEq)]
pub struct StreamConfig {
    pub source: StreamSource,
    /// The capture is scaled down (aspect preserved, never up) to fit in `max_width × max_height`.
    pub max_width: u32,
    pub max_height: u32,
    /// Target frame rate. Capture frames arriving faster are skipped.
    pub fps: u32,
    /// Constant bitrate in kbit/s.
    pub bitrate_kbps: u32,
    /// Keyframe interval in frames (default 120 = 2 s at 60 fps).
    pub gop_frames: u32,
    /// Re-encode the last frame when the screen is idle so the output keeps `fps`
    /// (helps decoders that only show output once the next frame arrives, and lets bitrate
    /// refine a static image). When false, idle screens produce no output.
    pub repeat_idle_frames: bool,
    pub encoder: EncoderPreference,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            source: StreamSource::PrimaryMonitor,
            max_width: 1920,
            max_height: 1080,
            fps: 60,
            bitrate_kbps: 12_000,
            gop_frames: 120,
            repeat_idle_frames: true,
            encoder: EncoderPreference::Auto,
        }
    }
}

pub const MIN_DIMENSION: u32 = 64;
pub const MAX_DIMENSION: u32 = 4096;
pub const MAX_FPS: u32 = 240;
pub const MIN_BITRATE_KBPS: u32 = 100;
pub const MAX_BITRATE_KBPS: u32 = 200_000;

impl StreamConfig {
    /// Check every field is in range. Called by `Streamer::start`.
    pub fn validate(&self) -> Result<()> {
        let bad = |m: String| Err(Error::InvalidConfig(m));
        for (name, v) in [("max_width", self.max_width), ("max_height", self.max_height)] {
            if !(MIN_DIMENSION..=MAX_DIMENSION).contains(&v) {
                return bad(format!("{name} = {v}, must be {MIN_DIMENSION}..={MAX_DIMENSION}"));
            }
        }
        if self.fps == 0 || self.fps > MAX_FPS {
            return bad(format!("fps = {}, must be 1..={MAX_FPS}", self.fps));
        }
        if !(MIN_BITRATE_KBPS..=MAX_BITRATE_KBPS).contains(&self.bitrate_kbps) {
            return bad(format!(
                "bitrate_kbps = {}, must be {MIN_BITRATE_KBPS}..={MAX_BITRATE_KBPS}",
                self.bitrate_kbps
            ));
        }
        if self.gop_frames == 0 || self.gop_frames > 100_000 {
            return bad(format!("gop_frames = {}, must be 1..=100000", self.gop_frames));
        }
        match &self.source {
            StreamSource::Monitor(id) if id.is_empty() => bad("monitor id is empty".into()),
            StreamSource::Window(0) => bad("window handle is 0".into()),
            _ => Ok(()),
        }
    }

    /// Duration of one frame in 100 ns units (Media Foundation time base).
    pub fn frame_duration_100ns(&self) -> i64 {
        10_000_000 / self.fps.max(1) as i64
    }
}

/// Size of the encoded picture for a `src_w × src_h` source that must fit in `max_w × max_h`:
/// aspect ratio preserved, never upscaled, both sides rounded down to even numbers (NV12 needs
/// even dimensions) and at least 2.
pub fn fit_output_size(src_w: u32, src_h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    let even = |v: u32| (v & !1).max(2);
    if src_w == 0 || src_h == 0 {
        return (even(max_w), even(max_h));
    }
    if src_w <= max_w && src_h <= max_h {
        return (even(src_w), even(src_h));
    }
    // scale = min(max_w/src_w, max_h/src_h), computed in integers to avoid float rounding.
    let (w, h) = if (max_w as u64) * (src_h as u64) <= (max_h as u64) * (src_w as u64) {
        (max_w, ((src_h as u64 * max_w as u64) / src_w as u64) as u32)
    } else {
        (((src_w as u64 * max_h as u64) / src_h as u64) as u32, max_h)
    };
    (even(w), even(h))
}

/// Destination rectangle `(left, top, right, bottom)` that letterboxes a `src` image into a
/// `dst` frame with the aspect ratio preserved and the image centred (even coordinates).
pub fn letterbox_rect(src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> (u32, u32, u32, u32) {
    if src_w == 0 || src_h == 0 {
        return (0, 0, dst_w, dst_h);
    }
    let (w, h) = if (dst_w as u64) * (src_h as u64) <= (dst_h as u64) * (src_w as u64) {
        (dst_w, ((src_h as u64 * dst_w as u64) / src_w as u64) as u32)
    } else {
        (((src_w as u64 * dst_h as u64) / src_h as u64) as u32, dst_h)
    };
    let (w, h) = (w.min(dst_w), h.min(dst_h));
    let left = ((dst_w - w) / 2) & !1;
    let top = ((dst_h - h) / 2) & !1;
    (left, top, (left + w).min(dst_w), (top + h).min(dst_h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid() {
        StreamConfig::default().validate().unwrap();
    }

    #[test]
    fn rejects_out_of_range_values() {
        let base = StreamConfig::default();
        let cases = [
            StreamConfig { fps: 0, ..base.clone() },
            StreamConfig { fps: 1000, ..base.clone() },
            StreamConfig { max_width: 10, ..base.clone() },
            StreamConfig { max_height: 100_000, ..base.clone() },
            StreamConfig { bitrate_kbps: 1, ..base.clone() },
            StreamConfig { bitrate_kbps: 1_000_000, ..base.clone() },
            StreamConfig { gop_frames: 0, ..base.clone() },
            StreamConfig { source: StreamSource::Monitor(String::new()), ..base.clone() },
            StreamConfig { source: StreamSource::Window(0), ..base.clone() },
        ];
        for c in cases {
            assert!(matches!(c.validate(), Err(Error::InvalidConfig(_))), "{c:?}");
        }
    }

    #[test]
    fn frame_duration() {
        let c = StreamConfig { fps: 60, ..Default::default() };
        assert_eq!(c.frame_duration_100ns(), 166_666);
    }

    #[test]
    fn fit_keeps_small_sources() {
        assert_eq!(fit_output_size(1920, 1080, 1920, 1080), (1920, 1080));
        assert_eq!(fit_output_size(1280, 720, 1920, 1080), (1280, 720));
        assert_eq!(fit_output_size(1281, 721, 1920, 1080), (1280, 720));
    }

    #[test]
    fn fit_scales_down_preserving_aspect() {
        assert_eq!(fit_output_size(3840, 2160, 1920, 1080), (1920, 1080));
        assert_eq!(fit_output_size(2560, 1600, 1920, 1080), (1728, 1080));
        assert_eq!(fit_output_size(3440, 1440, 1920, 1080), (1920, 802));
        assert_eq!(fit_output_size(1080, 1920, 1920, 1080), (606, 1080));
    }

    #[test]
    fn fit_outputs_even_sizes() {
        let (w, h) = fit_output_size(1366, 768, 1000, 1000);
        assert_eq!((w % 2, h % 2), (0, 0));
        assert_eq!(fit_output_size(0, 0, 640, 480), (640, 480));
    }

    #[test]
    fn letterbox_centres_image() {
        assert_eq!(letterbox_rect(1920, 1080, 1920, 1080), (0, 0, 1920, 1080));
        // 4:3 into 16:9: pillarbox.
        assert_eq!(letterbox_rect(1024, 768, 1920, 1080), (240, 0, 1680, 1080));
        // 21:9 into 16:9: letterbox.
        let (l, t, r, b) = letterbox_rect(3440, 1440, 1920, 1080);
        assert_eq!((l, r), (0, 1920));
        assert!(t > 0 && b < 1080 && t % 2 == 0);
    }
}
