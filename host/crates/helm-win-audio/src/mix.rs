//! Channel down/up-mixing to mono or stereo (pure Rust).
//!
//! Speaker positions come from `WAVEFORMATEXTENSIBLE::dwChannelMask`: the n-th set bit is the
//! n-th interleaved channel. Without a mask the usual Windows order for that channel count is
//! assumed.

const FL: u32 = 0x1;
const FR: u32 = 0x2;
const FC: u32 = 0x4;
const LFE: u32 = 0x8;
const BL: u32 = 0x10;
const BR: u32 = 0x20;
const FLC: u32 = 0x40;
const FRC: u32 = 0x80;
const BC: u32 = 0x100;
const SL: u32 = 0x200;
const SR: u32 = 0x400;

/// Default mask for a channel count (KSAUDIO_SPEAKER_* layouts).
pub fn default_mask(channels: u16) -> u32 {
    match channels {
        1 => FC,
        2 => FL | FR,
        3 => FL | FR | FC,
        4 => FL | FR | BL | BR,
        5 => FL | FR | FC | BL | BR,
        6 => FL | FR | FC | LFE | BL | BR,
        7 => FL | FR | FC | LFE | BC | SL | SR,
        8 => FL | FR | FC | LFE | BL | BR | SL | SR,
        _ => 0,
    }
}

/// Gains for one source channel: (to left, to right).
fn gains_for(position: u32) -> (f32, f32) {
    const H: f32 = std::f32::consts::FRAC_1_SQRT_2; // -3 dB
    match position {
        FL | FLC => (1.0, 0.0),
        FR | FRC => (0.0, 1.0),
        FC | BC => (H, H),
        BL | SL => (H, 0.0),
        BR | SR => (0.0, H),
        LFE => (0.0, 0.0),
        // Top / unknown speakers: centre-ish.
        _ => (0.5, 0.5),
    }
}

/// Maps `src` interleaved channels to `dst` (1 or 2) interleaved channels with a gain matrix.
#[derive(Debug, Clone)]
pub struct ChannelMixer {
    src: usize,
    dst: usize,
    /// `matrix[d * src + s]`
    matrix: Vec<f32>,
    identity: bool,
}

impl ChannelMixer {
    pub fn new(src_channels: u16, channel_mask: u32, dst_channels: u16) -> ChannelMixer {
        let src = src_channels.max(1) as usize;
        let dst = dst_channels.clamp(1, 2) as usize;
        let mut mask = if channel_mask != 0 {
            channel_mask
        } else {
            default_mask(src_channels)
        };
        let mut positions = Vec::with_capacity(src);
        for _ in 0..src {
            if mask == 0 {
                positions.push(0); // more channels than mask bits: unknown
            } else {
                let bit = mask & mask.wrapping_neg();
                positions.push(bit);
                mask &= !bit;
            }
        }
        // Stereo gains per source channel.
        let mut lr: Vec<(f32, f32)> = positions.iter().map(|&p| gains_for(p)).collect();
        if src == 1 {
            lr = vec![(1.0, 1.0)]; // mono source: same signal on both sides
        }
        // Normalise so a full-scale signal on every channel cannot exceed full scale by much:
        // scale each output by 1 / sum of its gains when that sum is above 1.
        let sum_l: f32 = lr.iter().map(|g| g.0).sum();
        let sum_r: f32 = lr.iter().map(|g| g.1).sum();
        let norm = |s: f32| if s > 1.0 { 1.0 / s } else { 1.0 };
        // Only normalise multichannel sources; plain stereo stays bit-exact.
        let (nl, nr) = if src > 2 {
            (norm(sum_l), norm(sum_r))
        } else {
            (1.0, 1.0)
        };
        let mut matrix = vec![0f32; dst * src];
        for (s, &(l, r)) in lr.iter().enumerate() {
            if dst == 2 {
                matrix[s] = l * nl;
                matrix[src + s] = r * nr;
            } else {
                matrix[s] = if src == 1 { 1.0 } else { 0.5 * (l * nl + r * nr) };
            }
        }
        let identity = src == dst && (src == 1 || (matrix == [1.0, 0.0, 0.0, 1.0]));
        ChannelMixer {
            src,
            dst,
            matrix,
            identity,
        }
    }

    pub fn src_channels(&self) -> usize {
        self.src
    }

    /// Mix whole frames of `input`, appending to `out`.
    pub fn process(&self, input: &[f32], out: &mut Vec<f32>) {
        if self.identity {
            out.extend_from_slice(input);
            return;
        }
        out.reserve(input.len() / self.src * self.dst);
        for frame in input.chunks_exact(self.src) {
            for d in 0..self.dst {
                let row = &self.matrix[d * self.src..(d + 1) * self.src];
                out.push(row.iter().zip(frame).map(|(g, x)| g * x).sum());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_passthrough() {
        let m = ChannelMixer::new(2, 0x3, 2);
        let mut out = Vec::new();
        m.process(&[0.1, -0.2, 0.3, 0.4], &mut out);
        assert_eq!(out, vec![0.1, -0.2, 0.3, 0.4]);
    }

    #[test]
    fn mono_up_and_stereo_down() {
        let mut out = Vec::new();
        ChannelMixer::new(1, 0, 2).process(&[0.5, -0.25], &mut out);
        assert_eq!(out, vec![0.5, 0.5, -0.25, -0.25]);
        out.clear();
        ChannelMixer::new(2, 0, 1).process(&[1.0, 0.0, 0.2, 0.4], &mut out);
        assert!((out[0] - 0.5).abs() < 1e-6 && (out[1] - 0.3).abs() < 1e-6, "{out:?}");
    }

    #[test]
    fn surround_51_to_stereo() {
        // FL FR FC LFE BL BR
        let m = ChannelMixer::new(6, 0x3F, 2);
        let mut out = Vec::new();
        m.process(&[1.0, 0.0, 0.0, 0.0, 0.0, 0.0], &mut out); // FL only
        assert!(out[0] > 0.3 && out[1] == 0.0, "{out:?}");
        out.clear();
        m.process(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0], &mut out); // LFE dropped
        assert_eq!(out, vec![0.0, 0.0]);
        out.clear();
        m.process(&[1.0; 6], &mut out); // all full scale stays within full scale
        assert!(out.iter().all(|v| *v <= 1.0001 && *v > 0.9), "{out:?}");
        out.clear();
        m.process(&[0.0, 0.0, 1.0, 0.0, 0.0, 0.0], &mut out); // centre to both
        assert!((out[0] - out[1]).abs() < 1e-6 && out[0] > 0.2);
    }

    #[test]
    fn more_channels_than_mask_bits() {
        let m = ChannelMixer::new(4, 0x3, 2);
        let mut out = Vec::new();
        m.process(&[0.0, 0.0, 1.0, 1.0], &mut out);
        assert!(out.iter().all(|v| v.is_finite()));
        assert_eq!(out.len(), 2);
    }
}
