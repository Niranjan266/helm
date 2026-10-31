//! Stream configuration (pure Rust).

use crate::error::{Error, Result};

/// Opus application mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpusMode {
    /// `OPUS_APPLICATION_RESTRICTED_LOWDELAY`: CELT only, 2.5 ms encoder look-ahead. Best for
    /// desktop / game / music audio at 64 kbit/s and up. Default.
    LowDelay,
    /// `OPUS_APPLICATION_AUDIO`: SILK + CELT hybrid, 6.5 ms look-ahead. Slightly better at low
    /// bitrates (< 64 kbit/s) and for speech.
    Audio,
}

/// What [`crate::AudioStreamer::start`] captures and how it encodes.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioConfig {
    /// Output (Opus) sample rate. Opus supports 8000, 12000, 16000, 24000, 48000. Default 48000.
    pub sample_rate: u32,
    /// Output channels: 1 or 2. Default 2.
    pub channels: u16,
    /// Target bitrate, 6..=510 kbit/s. The roadmap's range for desktop audio is 96..=128. Default 128.
    pub bitrate_kbps: u32,
    /// Packet duration in ms: 10, 20, 40 or 60. Default 20.
    pub frame_ms: u32,
    /// Opus application. Default [`OpusMode::LowDelay`].
    pub mode: OpusMode,
    /// Opus complexity 0..=10. Default 8 (≈ 1–2 % of one core for stereo 48 kHz).
    pub complexity: u8,
    /// Constant bitrate (true) or constrained VBR (false, default). CBR makes every packet the
    /// same size, which is easier on a congested link but wastes bits on silence.
    pub cbr: bool,
    /// When nothing is playing, Windows delivers no loopback data at all. With `fill_silence`
    /// (default true) the streamer synthesises silent frames on the QPC clock, so the phone gets
    /// a steady packet every `frame_ms` (cheap: Opus codes digital silence in a few bytes).
    /// Turn it off to send nothing while the PC is silent.
    pub fill_silence: bool,
    /// Follow changes of the default playback device (headphones plugged in, user switched
    /// output). Default true. When false the first device is kept until it disappears.
    pub follow_default_device: bool,
}

impl Default for AudioConfig {
    fn default() -> Self {
        AudioConfig {
            sample_rate: 48_000,
            channels: 2,
            bitrate_kbps: 128,
            frame_ms: 20,
            mode: OpusMode::LowDelay,
            complexity: 8,
            cbr: false,
            fill_silence: true,
            follow_default_device: true,
        }
    }
}

impl AudioConfig {
    /// Check ranges. `AudioStreamer::start` calls this too.
    pub fn validate(&self) -> Result<()> {
        if ![8_000, 12_000, 16_000, 24_000, 48_000].contains(&self.sample_rate) {
            return Err(Error::InvalidConfig(format!(
                "sample_rate {} (Opus needs 8000, 12000, 16000, 24000 or 48000)",
                self.sample_rate
            )));
        }
        if !(1..=2).contains(&self.channels) {
            return Err(Error::InvalidConfig(format!("channels {} (1 or 2)", self.channels)));
        }
        if !(6..=510).contains(&self.bitrate_kbps) {
            return Err(Error::InvalidConfig(format!(
                "bitrate_kbps {} (6..=510)",
                self.bitrate_kbps
            )));
        }
        if ![10, 20, 40, 60].contains(&self.frame_ms) {
            return Err(Error::InvalidConfig(format!(
                "frame_ms {} (10, 20, 40 or 60)",
                self.frame_ms
            )));
        }
        if self.complexity > 10 {
            return Err(Error::InvalidConfig(format!("complexity {} (0..=10)", self.complexity)));
        }
        Ok(())
    }

    /// Samples per channel in one packet (960 for 48 kHz / 20 ms).
    pub fn frame_samples(&self) -> usize {
        (self.sample_rate as usize * self.frame_ms as usize) / 1000
    }

    /// Interleaved samples in one packet (`frame_samples * channels`).
    pub fn frame_len(&self) -> usize {
        self.frame_samples() * self.channels as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_valid_and_48k_20ms() {
        let c = AudioConfig::default();
        c.validate().unwrap();
        assert_eq!(c.frame_samples(), 960);
        assert_eq!(c.frame_len(), 1920);
    }

    #[test]
    fn rejects_out_of_range() {
        let bad = [
            AudioConfig {
                sample_rate: 44_100,
                ..Default::default()
            },
            AudioConfig {
                channels: 0,
                ..Default::default()
            },
            AudioConfig {
                channels: 6,
                ..Default::default()
            },
            AudioConfig {
                bitrate_kbps: 5,
                ..Default::default()
            },
            AudioConfig {
                bitrate_kbps: 600,
                ..Default::default()
            },
            AudioConfig {
                frame_ms: 15,
                ..Default::default()
            },
            AudioConfig {
                complexity: 11,
                ..Default::default()
            },
        ];
        for c in bad {
            assert!(c.validate().is_err(), "{c:?} should be invalid");
        }
    }

    #[test]
    fn frame_sizes_for_other_rates() {
        let c = AudioConfig {
            sample_rate: 16_000,
            channels: 1,
            frame_ms: 60,
            ..Default::default()
        };
        c.validate().unwrap();
        assert_eq!(c.frame_samples(), 960);
        assert_eq!(c.frame_len(), 960);
        let c = AudioConfig {
            frame_ms: 10,
            ..Default::default()
        };
        assert_eq!(c.frame_samples(), 480);
    }
}
