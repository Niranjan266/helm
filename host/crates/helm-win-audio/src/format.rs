//! Device mix formats and conversion of raw WASAPI buffers to interleaved f32 (pure Rust).

use crate::error::{Error, Result};

/// Sample encoding of a device buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleType {
    F32,
    F64,
    I16,
    /// Packed 24-bit little-endian (3 bytes per sample).
    I24,
    /// 32-bit container; `valid_bits` (e.g. 24) of the most significant bits are used.
    I32,
    U8,
}

/// A device (mix) format, parsed from `WAVEFORMATEX` / `WAVEFORMATEXTENSIBLE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_type: SampleType,
    /// Bytes per sample (one channel).
    pub bytes_per_sample: u16,
    /// `dwChannelMask` (speaker positions, `SPEAKER_*` bits) or 0 when unknown.
    pub channel_mask: u32,
}

const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

impl DeviceFormat {
    /// Interpret the fields of a `WAVEFORMATEX` (+ the extensible part when `format_tag` is
    /// `WAVE_FORMAT_EXTENSIBLE`). `sub_format_data1` is `SubFormat.data1` of the extensible
    /// struct (the KSDATAFORMAT_SUBTYPE_* GUIDs differ only there: 1 = PCM, 3 = IEEE float).
    #[allow(clippy::too_many_arguments)]
    pub fn from_wave_format(
        format_tag: u16,
        channels: u16,
        sample_rate: u32,
        bits_per_sample: u16,
        block_align: u16,
        valid_bits: u16,
        channel_mask: u32,
        sub_format_data1: u32,
    ) -> Result<DeviceFormat> {
        let (tag, mask, valid) = if format_tag == WAVE_FORMAT_EXTENSIBLE {
            (
                sub_format_data1 as u16,
                channel_mask,
                if valid_bits == 0 { bits_per_sample } else { valid_bits },
            )
        } else {
            (format_tag, 0, bits_per_sample)
        };
        if channels == 0 || sample_rate == 0 {
            return Err(Error::UnsupportedFormat(format!(
                "{channels} channels at {sample_rate} Hz"
            )));
        }
        let bytes = bits_per_sample / 8;
        if bytes == 0 || block_align != bytes * channels {
            return Err(Error::UnsupportedFormat(format!(
                "block align {block_align} for {channels} x {bits_per_sample} bit"
            )));
        }
        let sample_type = match (tag, bits_per_sample) {
            (WAVE_FORMAT_IEEE_FLOAT, 32) => SampleType::F32,
            (WAVE_FORMAT_IEEE_FLOAT, 64) => SampleType::F64,
            (WAVE_FORMAT_PCM, 8) => SampleType::U8,
            (WAVE_FORMAT_PCM, 16) => SampleType::I16,
            (WAVE_FORMAT_PCM, 24) => SampleType::I24,
            (WAVE_FORMAT_PCM, 32) => SampleType::I32,
            _ => {
                return Err(Error::UnsupportedFormat(format!(
                    "format tag {tag:#x}, {bits_per_sample} bit (valid {valid})"
                )));
            }
        };
        Ok(DeviceFormat {
            sample_rate,
            channels,
            sample_type,
            bytes_per_sample: bytes,
            channel_mask: mask,
        })
    }

    /// Bytes per frame (all channels).
    pub fn block_align(&self) -> usize {
        self.bytes_per_sample as usize * self.channels as usize
    }

    /// Short description, e.g. `"48000 Hz, 2 ch, F32"`.
    pub fn describe(&self) -> String {
        format!("{} Hz, {} ch, {:?}", self.sample_rate, self.channels, self.sample_type)
    }
}

/// Append `frames` frames of `data` (device layout) to `out` as interleaved f32 (-1..1).
/// `data` may be `None` for a silent packet (`AUDCLNT_BUFFERFLAGS_SILENT`): zeros are appended.
pub fn to_f32(fmt: &DeviceFormat, data: Option<&[u8]>, frames: usize, out: &mut Vec<f32>) {
    let n = frames * fmt.channels as usize;
    let Some(data) = data else {
        out.resize(out.len() + n, 0.0);
        return;
    };
    let need = frames * fmt.block_align();
    let data = &data[..need.min(data.len())];
    let bps = fmt.bytes_per_sample as usize;
    out.reserve(n);
    for s in data.chunks_exact(bps) {
        let v = match fmt.sample_type {
            SampleType::F32 => f32::from_le_bytes([s[0], s[1], s[2], s[3]]),
            SampleType::F64 => f64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]) as f32,
            SampleType::I16 => i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.0,
            SampleType::I24 => (i32::from_le_bytes([0, s[0], s[1], s[2]]) >> 8) as f32 / 8_388_608.0,
            SampleType::I32 => i32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f32 / 2_147_483_648.0,
            SampleType::U8 => (s[0] as f32 - 128.0) / 128.0,
        };
        // NaN / inf from a misbehaving APO would poison the encoder state.
        out.push(if v.is_finite() { v } else { 0.0 });
    }
    // A short buffer (should not happen) is padded so the frame count stays right.
    let expected = n;
    let got = data.len() / bps;
    if got < expected {
        out.resize(out.len() + (expected - got), 0.0);
    }
}

/// Interleaved f32 → i16 with clipping (for `Encoder::encode_i16` users).
pub fn f32_to_i16(input: &[f32], out: &mut Vec<i16>) {
    out.extend(input.iter().map(|&v| (v.clamp(-1.0, 1.0) * 32767.0).round() as i16));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_typical_mix_format() {
        // WAVEFORMATEXTENSIBLE, 48 kHz stereo float, front left|right.
        let f = DeviceFormat::from_wave_format(0xFFFE, 2, 48_000, 32, 8, 32, 0x3, 3).unwrap();
        assert_eq!(f.sample_type, SampleType::F32);
        assert_eq!(f.block_align(), 8);
        assert_eq!(f.channel_mask, 3);
        let f = DeviceFormat::from_wave_format(1, 2, 44_100, 16, 4, 0, 0, 0).unwrap();
        assert_eq!(f.sample_type, SampleType::I16);
        let f = DeviceFormat::from_wave_format(0xFFFE, 6, 48_000, 32, 24, 24, 0x3F, 1).unwrap();
        assert_eq!(f.sample_type, SampleType::I32);
        assert!(DeviceFormat::from_wave_format(2, 2, 48_000, 4, 2, 0, 0, 0).is_err()); // ADPCM
        assert!(DeviceFormat::from_wave_format(1, 2, 48_000, 16, 3, 0, 0, 0).is_err()); // bad align
    }

    #[test]
    fn converts_each_sample_type() {
        let mut out = Vec::new();
        let f = DeviceFormat::from_wave_format(3, 1, 48_000, 32, 4, 0, 0, 0).unwrap();
        to_f32(&f, Some(&0.25f32.to_le_bytes()), 1, &mut out);
        let f = DeviceFormat::from_wave_format(1, 1, 48_000, 16, 2, 0, 0, 0).unwrap();
        to_f32(&f, Some(&(-16384i16).to_le_bytes()), 1, &mut out);
        let f = DeviceFormat::from_wave_format(1, 1, 48_000, 24, 3, 0, 0, 0).unwrap();
        to_f32(&f, Some(&[0x00, 0x00, 0x40]), 1, &mut out); // 0x400000 = +0.5
        let f = DeviceFormat::from_wave_format(1, 1, 48_000, 32, 4, 0, 0, 0).unwrap();
        to_f32(&f, Some(&(i32::MIN).to_le_bytes()), 1, &mut out);
        let f = DeviceFormat::from_wave_format(1, 1, 48_000, 8, 1, 0, 0, 0).unwrap();
        to_f32(&f, Some(&[192]), 1, &mut out);
        assert_eq!(out, vec![0.25, -0.5, 0.5, -1.0, 0.5]);
    }

    #[test]
    fn silent_and_nan_and_short() {
        let f = DeviceFormat::from_wave_format(3, 2, 48_000, 32, 8, 0, 0, 0).unwrap();
        let mut out = vec![9.0];
        to_f32(&f, None, 3, &mut out);
        assert_eq!(out, vec![9.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        let mut out = Vec::new();
        let mut bytes = f32::NAN.to_le_bytes().to_vec();
        bytes.extend(1.0f32.to_le_bytes());
        to_f32(&f, Some(&bytes), 2, &mut out); // only one frame of data for two frames
        assert_eq!(out, vec![0.0, 1.0, 0.0, 0.0]);
    }

    #[test]
    fn i16_conversion_clips() {
        let mut out = Vec::new();
        f32_to_i16(&[0.0, 1.5, -2.0, 0.5], &mut out);
        assert_eq!(out, vec![0, 32767, -32767, 16384]);
    }
}
