//! Safe wrappers around the Opus encoder / decoder (pure Rust, `unsafe-libopus`), plus
//! `encode` / `decode` round-trip helpers used by the tests and the example.

use crate::config::{AudioConfig, OpusMode};
use crate::error::{Error, Result};
use unsafe_libopus as unsafe_opus;

/// Largest packet we ask the encoder for (RFC 6716 allows up to 1275 bytes per frame; 60 ms at
/// 510 kbit/s is 3825 bytes, so 4000 covers every configuration we allow).
pub const MAX_PACKET: usize = 4000;

/// Human name of an Opus error code.
pub fn error_name(code: i32) -> &'static str {
    match code {
        unsafe_opus::OPUS_OK => "OPUS_OK",
        unsafe_opus::OPUS_BAD_ARG => "OPUS_BAD_ARG",
        unsafe_opus::OPUS_BUFFER_TOO_SMALL => "OPUS_BUFFER_TOO_SMALL",
        unsafe_opus::OPUS_INTERNAL_ERROR => "OPUS_INTERNAL_ERROR",
        unsafe_opus::OPUS_INVALID_PACKET => "OPUS_INVALID_PACKET",
        unsafe_opus::OPUS_UNIMPLEMENTED => "OPUS_UNIMPLEMENTED",
        unsafe_opus::OPUS_INVALID_STATE => "OPUS_INVALID_STATE",
        unsafe_opus::OPUS_ALLOC_FAIL => "OPUS_ALLOC_FAIL",
        _ => "unknown Opus error",
    }
}

fn check(context: &'static str, code: i32) -> Result<i32> {
    if code < 0 {
        Err(Error::Opus { context, code })
    } else {
        Ok(code)
    }
}

/// One Opus encoder instance (owns the C-style state).
pub struct Encoder {
    st: *mut unsafe_opus::OpusEncoder,
    channels: usize,
    frame_samples: usize,
}

// The state is only reachable through `&mut self`; moving it between threads is fine.
unsafe impl Send for Encoder {}

impl Encoder {
    /// Create an encoder for `config` (validated first).
    pub fn new(config: &AudioConfig) -> Result<Encoder> {
        config.validate()?;
        let app = match config.mode {
            OpusMode::LowDelay => unsafe_opus::OPUS_APPLICATION_RESTRICTED_LOWDELAY,
            OpusMode::Audio => unsafe_opus::OPUS_APPLICATION_AUDIO,
        };
        let mut err = 0i32;
        let st = unsafe {
            unsafe_opus::opus_encoder_create(config.sample_rate as i32, config.channels as i32, app, &mut err)
        };
        if st.is_null() || err != unsafe_opus::OPUS_OK {
            return Err(Error::Opus {
                context: "opus_encoder_create",
                code: err,
            });
        }
        let enc = Encoder {
            st,
            channels: config.channels as usize,
            frame_samples: config.frame_samples(),
        };
        unsafe {
            check(
                "OPUS_SET_BITRATE",
                unsafe_opus::opus_encoder_ctl!(
                    st,
                    unsafe_opus::OPUS_SET_BITRATE_REQUEST,
                    config.bitrate_kbps as i32 * 1000
                ),
            )?;
            check(
                "OPUS_SET_COMPLEXITY",
                unsafe_opus::opus_encoder_ctl!(st, unsafe_opus::OPUS_SET_COMPLEXITY_REQUEST, config.complexity as i32),
            )?;
            check(
                "OPUS_SET_VBR",
                unsafe_opus::opus_encoder_ctl!(
                    st,
                    unsafe_opus::OPUS_SET_VBR_REQUEST,
                    if config.cbr { 0i32 } else { 1i32 }
                ),
            )?;
            if !config.cbr {
                check(
                    "OPUS_SET_VBR_CONSTRAINT",
                    unsafe_opus::opus_encoder_ctl!(st, unsafe_opus::OPUS_SET_VBR_CONSTRAINT_REQUEST, 1i32),
                )?;
            }
            check(
                "OPUS_SET_SIGNAL",
                unsafe_opus::opus_encoder_ctl!(
                    st,
                    unsafe_opus::OPUS_SET_SIGNAL_REQUEST,
                    unsafe_opus::OPUS_SIGNAL_MUSIC
                ),
            )?;
        }
        Ok(enc)
    }

    /// Encoder look-ahead in samples per channel (the decoder output is delayed by this much;
    /// it is the Ogg "pre-skip").
    pub fn lookahead(&mut self) -> Result<u32> {
        let mut v = 0i32;
        unsafe {
            check(
                "OPUS_GET_LOOKAHEAD",
                unsafe_opus::opus_encoder_ctl!(self.st, unsafe_opus::OPUS_GET_LOOKAHEAD_REQUEST, &mut v),
            )?
        };
        Ok(v as u32)
    }

    /// Samples per channel per packet.
    pub fn frame_samples(&self) -> usize {
        self.frame_samples
    }

    /// Encode exactly one frame of interleaved f32 (`frame_samples * channels` values, nominal
    /// range -1..1) into `out`. Returns the packet length.
    pub fn encode_f32(&mut self, pcm: &[f32], out: &mut [u8]) -> Result<usize> {
        if pcm.len() != self.frame_samples * self.channels {
            return Err(Error::InvalidConfig(format!(
                "encode_f32: got {} samples, frame needs {}",
                pcm.len(),
                self.frame_samples * self.channels
            )));
        }
        let n = unsafe {
            unsafe_opus::opus_encode_float(
                self.st,
                pcm.as_ptr(),
                self.frame_samples as i32,
                out.as_mut_ptr(),
                out.len().min(i32::MAX as usize) as i32,
            )
        };
        Ok(check("opus_encode_float", n)? as usize)
    }

    /// Same as [`Self::encode_f32`] for interleaved i16.
    pub fn encode_i16(&mut self, pcm: &[i16], out: &mut [u8]) -> Result<usize> {
        if pcm.len() != self.frame_samples * self.channels {
            return Err(Error::InvalidConfig(format!(
                "encode_i16: got {} samples, frame needs {}",
                pcm.len(),
                self.frame_samples * self.channels
            )));
        }
        let n = unsafe {
            unsafe_opus::opus_encode(
                self.st,
                pcm.as_ptr(),
                self.frame_samples as i32,
                out.as_mut_ptr(),
                out.len().min(i32::MAX as usize) as i32,
            )
        };
        Ok(check("opus_encode", n)? as usize)
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe { unsafe_opus::opus_encoder_destroy(self.st) };
    }
}

/// One Opus decoder instance.
pub struct Decoder {
    st: *mut unsafe_opus::OpusDecoder,
    channels: usize,
    max_frame: usize,
}

unsafe impl Send for Decoder {}

impl Decoder {
    pub fn new(sample_rate: u32, channels: u16) -> Result<Decoder> {
        let mut err = 0i32;
        let st = unsafe { unsafe_opus::opus_decoder_create(sample_rate as i32, channels as i32, &mut err) };
        if st.is_null() || err != unsafe_opus::OPUS_OK {
            return Err(Error::Opus {
                context: "opus_decoder_create",
                code: err,
            });
        }
        // 120 ms is the longest Opus packet.
        Ok(Decoder {
            st,
            channels: channels as usize,
            max_frame: sample_rate as usize * 120 / 1000,
        })
    }

    /// Decode one packet, appending interleaved f32 to `out`. Returns samples per channel.
    pub fn decode_f32(&mut self, packet: &[u8], out: &mut Vec<f32>) -> Result<usize> {
        let start = out.len();
        out.resize(start + self.max_frame * self.channels, 0.0);
        let n = unsafe {
            unsafe_opus::opus_decode_float(
                self.st,
                packet.as_ptr(),
                packet.len() as i32,
                out[start..].as_mut_ptr(),
                self.max_frame as i32,
                0,
            )
        };
        let n = match check("opus_decode_float", n) {
            Ok(n) => n as usize,
            Err(e) => {
                out.truncate(start);
                return Err(e);
            }
        };
        out.truncate(start + n * self.channels);
        Ok(n)
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe { unsafe_opus::opus_decoder_destroy(self.st) };
    }
}

/// Encode a whole interleaved f32 buffer into packets of `config.frame_ms` (the tail is padded
/// with silence). Returns the packets and the encoder look-ahead (pre-skip).
pub fn encode(config: &AudioConfig, pcm: &[f32]) -> Result<(Vec<Vec<u8>>, u32)> {
    let mut enc = Encoder::new(config)?;
    let lookahead = enc.lookahead()?;
    let len = config.frame_len();
    let mut packets = Vec::with_capacity(pcm.len() / len + 1);
    let mut buf = [0u8; MAX_PACKET];
    let mut frame = vec![0f32; len];
    for chunk in pcm.chunks(len) {
        frame[..chunk.len()].copy_from_slice(chunk);
        frame[chunk.len()..].fill(0.0);
        let n = enc.encode_f32(&frame, &mut buf)?;
        packets.push(buf[..n].to_vec());
    }
    Ok((packets, lookahead))
}

/// Decode packets back to interleaved f32 (still delayed by the encoder look-ahead).
pub fn decode(sample_rate: u32, channels: u16, packets: &[Vec<u8>]) -> Result<Vec<f32>> {
    let mut dec = Decoder::new(sample_rate, channels)?;
    let mut out = Vec::new();
    for p in packets {
        dec.decode_f32(p, &mut out)?;
    }
    Ok(out)
}

/// Opus library version string.
pub fn version() -> String {
    let p = unsafe_opus::opus_get_version_string();
    p.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, channels: u16, freq: f32, amp: f32, seconds: f32) -> Vec<f32> {
        let n = (rate as f32 * seconds) as usize;
        let mut v = Vec::with_capacity(n * channels as usize);
        for i in 0..n {
            let t = i as f32 / rate as f32;
            // A slow envelope keeps the signal from being periodic, so the delay search below
            // has exactly one best lag.
            let env = 0.75 + 0.25 * (2.0 * std::f32::consts::PI * 2.0 * t).sin();
            let s = amp * env * (2.0 * std::f32::consts::PI * freq * t).sin();
            for c in 0..channels {
                // Slightly different right channel so a channel swap would be noticed.
                v.push(if c == 0 { s } else { 0.8 * s });
            }
        }
        v
    }

    /// SNR (dB) of `out` against `reference` after the best integer delay and gain, ignoring
    /// the first `skip` samples per channel.
    pub(crate) fn snr_db(reference: &[f32], out: &[f32], channels: usize, skip: usize, max_lag: usize) -> (f64, usize) {
        let frames = reference.len() / channels;
        let mut best = (f64::MIN, 0);
        for lag in 0..max_lag {
            let mut sig = 0f64;
            let mut cross = 0f64;
            let mut outp = 0f64;
            for i in skip..frames.saturating_sub(max_lag) {
                for c in 0..channels {
                    let r = reference[i * channels + c] as f64;
                    let Some(&o) = out.get((i + lag) * channels + c) else {
                        continue;
                    };
                    sig += r * r;
                    cross += r * o as f64;
                    outp += (o as f64) * (o as f64);
                }
            }
            if sig == 0.0 || outp == 0.0 || cross <= 0.0 {
                continue;
            }
            let g = cross / outp; // best gain applied to output
            let noise = sig - 2.0 * g * cross + g * g * outp;
            let snr = 10.0 * (sig / noise.max(1e-20)).log10();
            if snr > best.0 {
                best = (snr, lag);
            }
        }
        best
    }

    #[test]
    fn version_is_libopus() {
        assert!(version().contains("libopus"), "{}", version());
    }

    #[test]
    fn round_trip_sine_snr() {
        for mode in [OpusMode::LowDelay, OpusMode::Audio] {
            let cfg = AudioConfig {
                bitrate_kbps: 128,
                mode,
                ..Default::default()
            };
            let pcm = sine(48_000, 2, 1000.0, 0.5, 1.0);
            let (packets, lookahead) = encode(&cfg, &pcm).unwrap();
            assert_eq!(packets.len(), 50, "1 s of 20 ms packets");
            assert!(packets.iter().all(|p| !p.is_empty() && p.len() <= MAX_PACKET));
            let out = decode(48_000, 2, &packets).unwrap();
            assert_eq!(out.len(), 50 * 960 * 2);
            let (snr, lag) = snr_db(&pcm, &out, 2, 4800, 1000);
            assert!(snr > 20.0, "{mode:?}: SNR {snr:.1} dB at lag {lag}");
            assert!(
                (lag as i64 - lookahead as i64).abs() <= 2,
                "{mode:?}: lag {lag} vs look-ahead {lookahead}"
            );
            let bytes: usize = packets.iter().map(|p| p.len()).sum();
            let kbps = bytes as f64 * 8.0 / 1000.0;
            eprintln!("{mode:?}: SNR {snr:.1} dB, delay {lag} samples, {kbps:.1} kbit/s");
            assert!(
                (60.0..=180.0).contains(&kbps),
                "{mode:?}: {kbps:.1} kbit/s for 128 target"
            );
        }
    }

    #[test]
    fn lookahead_lowdelay_is_2_5ms() {
        let mut e = Encoder::new(&AudioConfig::default()).unwrap();
        assert_eq!(e.lookahead().unwrap(), 120);
    }

    #[test]
    fn silence_is_tiny_and_i16_path_works() {
        let cfg = AudioConfig::default();
        let mut e = Encoder::new(&cfg).unwrap();
        let mut buf = [0u8; MAX_PACKET];
        let silent = vec![0i16; cfg.frame_len()];
        let mut sizes = Vec::new();
        for _ in 0..10 {
            sizes.push(e.encode_i16(&silent, &mut buf).unwrap());
        }
        assert!(*sizes.last().unwrap() < 20, "silence packets: {sizes:?}");
        assert!(e.encode_i16(&silent[1..], &mut buf).is_err());
        let mut d = Decoder::new(48_000, 2).unwrap();
        let mut out = Vec::new();
        assert_eq!(d.decode_f32(&buf[..sizes[9]], &mut out).unwrap(), 960);
        assert!(out.iter().all(|s| s.abs() < 1e-3));
        assert!(d.decode_f32(&[0xFF, 0xFF, 0xFF], &mut out).is_err() || out.len() >= 1920);
    }

    #[test]
    fn mono_and_cbr() {
        let cfg = AudioConfig {
            channels: 1,
            cbr: true,
            bitrate_kbps: 96,
            ..Default::default()
        };
        let pcm = sine(48_000, 1, 440.0, 0.3, 0.5);
        let (packets, _) = encode(&cfg, &pcm).unwrap();
        // CBR at 96 kbit/s, 20 ms = 240 bytes per packet.
        assert!(
            packets.iter().skip(1).all(|p| p.len() == 240),
            "{:?}",
            packets.iter().map(|p| p.len()).collect::<Vec<_>>()
        );
        let out = decode(48_000, 1, &packets).unwrap();
        let (snr, _) = snr_db(&pcm, &out, 1, 2400, 1000);
        assert!(snr > 20.0, "mono SNR {snr:.1}");
    }
}
