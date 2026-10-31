//! A small streaming sample-rate converter (pure Rust).
//!
//! Linear interpolation, with a 2-tap-per-octave box pre-filter when downsampling by 2x or
//! more (e.g. a 96 or 192 kHz device). Good enough for monitoring desktop audio on a phone;
//! most devices already run at 48 kHz, where this is a plain copy. State is kept between calls,
//! so feeding a signal in arbitrary chunks gives the same output as one big call.

/// Streaming linear-interpolation resampler for interleaved f32.
#[derive(Debug, Clone)]
pub struct Resampler {
    in_rate: u32,
    out_rate: u32,
    channels: usize,
    /// Input samples consumed per output sample.
    step: f64,
    /// Position of the next output sample, relative to `prev` (index 0 of the virtual buffer
    /// `[prev, input...]`).
    pos: f64,
    prev: Vec<f32>,
    have_prev: bool,
    /// Integer decimation applied before interpolation (1 = none).
    decim: usize,
    decim_acc: Vec<f32>,
    decim_n: usize,
    scratch: Vec<f32>,
}

impl Resampler {
    pub fn new(in_rate: u32, out_rate: u32, channels: usize) -> Resampler {
        let channels = channels.max(1);
        let decim = (in_rate / out_rate.max(1)).max(1) as usize;
        let eff_in = in_rate as f64 / decim as f64;
        Resampler {
            in_rate,
            out_rate,
            channels,
            step: eff_in / out_rate as f64,
            pos: 0.0,
            prev: vec![0.0; channels],
            have_prev: false,
            decim,
            decim_acc: vec![0.0; channels],
            decim_n: 0,
            scratch: Vec::new(),
        }
    }

    pub fn is_passthrough(&self) -> bool {
        self.in_rate == self.out_rate
    }

    pub fn in_rate(&self) -> u32 {
        self.in_rate
    }

    /// Convert whole frames of `input`, appending to `out`.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if self.is_passthrough() {
            out.extend_from_slice(input);
            return;
        }
        let ch = self.channels;
        let mut data = std::mem::take(&mut self.scratch);
        data.clear();
        if self.decim > 1 {
            for frame in input.chunks_exact(ch) {
                for (a, x) in self.decim_acc.iter_mut().zip(frame) {
                    *a += x;
                }
                self.decim_n += 1;
                if self.decim_n == self.decim {
                    let k = 1.0 / self.decim as f32;
                    for a in self.decim_acc.iter_mut() {
                        data.push(*a * k);
                        *a = 0.0;
                    }
                    self.decim_n = 0;
                }
            }
        } else {
            data.extend_from_slice(input);
        }
        let n = data.len() / ch;
        if n > 0 {
            if !self.have_prev {
                // Start exactly on the first input sample (x[1]) so output sample k sits at
                // input time k * step: no start-up delay.
                self.prev.copy_from_slice(&data[..ch]);
                self.have_prev = true;
                self.pos = 1.0;
            }
            // Virtual buffer x[0] = prev, x[i + 1] = data[i]; need x[i] and x[i + 1].
            let sample = |i: usize, c: usize, prev: &[f32]| if i == 0 { prev[c] } else { data[(i - 1) * ch + c] };
            out.reserve(((n as f64) / self.step) as usize * ch + ch);
            while self.pos < n as f64 {
                let i = self.pos as usize;
                let t = (self.pos - i as f64) as f32;
                for c in 0..ch {
                    let a = sample(i, c, &self.prev);
                    let b = sample(i + 1, c, &self.prev);
                    out.push(a + (b - a) * t);
                }
                self.pos += self.step;
            }
            self.pos -= n as f64;
            self.prev.copy_from_slice(&data[(n - 1) * ch..n * ch]);
        }
        self.scratch = data;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, freq: f64, frames: usize, ch: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let s = (2.0 * std::f64::consts::PI * freq * i as f64 / rate as f64).sin() as f32;
                std::iter::repeat_n(s, ch)
            })
            .collect()
    }

    /// Estimate frequency from zero crossings (channel 0).
    fn freq(v: &[f32], rate: u32, ch: usize) -> f64 {
        let x: Vec<f32> = v.iter().step_by(ch).copied().collect();
        let crossings = x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        crossings as f64 * rate as f64 / x.len() as f64
    }

    #[test]
    fn passthrough_is_copy() {
        let mut r = Resampler::new(48_000, 48_000, 2);
        let mut out = Vec::new();
        r.process(&[0.1, 0.2, 0.3, 0.4], &mut out);
        assert_eq!(out, vec![0.1, 0.2, 0.3, 0.4]);
    }

    #[test]
    fn upsample_44k1_keeps_frequency_and_length() {
        let input = sine(44_100, 1000.0, 44_100, 2);
        let mut r = Resampler::new(44_100, 48_000, 2);
        let mut out = Vec::new();
        r.process(&input, &mut out);
        let frames = out.len() / 2;
        assert!((frames as i64 - 48_000).abs() <= 2, "{frames}");
        let f = freq(&out, 48_000, 2);
        assert!((f - 1000.0).abs() < 2.0, "{f}");
        // Linear interpolation of a 1 kHz tone at 44.1 kHz is accurate to ~-40 dB.
        let reference = sine(48_000, 1000.0, frames, 2);
        let err: f64 = out
            .iter()
            .zip(&reference)
            .skip(200)
            .map(|(a, b)| ((a - b) as f64).powi(2))
            .sum();
        let sig: f64 = reference.iter().skip(200).map(|v| (*v as f64).powi(2)).sum();
        let snr = 10.0 * (sig / err).log10();
        assert!(snr > 30.0, "SNR {snr:.1} dB");
    }

    #[test]
    fn chunked_equals_one_shot() {
        let input = sine(44_100, 440.0, 10_000, 2);
        let mut a = Resampler::new(44_100, 48_000, 2);
        let mut one = Vec::new();
        a.process(&input, &mut one);
        let mut b = Resampler::new(44_100, 48_000, 2);
        let mut chunked = Vec::new();
        let mut off = 0;
        for (k, size) in [1usize, 7, 441, 1000, 13, 3000].iter().cycle().enumerate() {
            if off >= 10_000 || k > 1000 {
                break;
            }
            let end = (off + size).min(10_000);
            b.process(&input[off * 2..end * 2], &mut chunked);
            off = end;
        }
        assert_eq!(one.len(), chunked.len());
        for (x, y) in one.iter().zip(&chunked) {
            assert!((x - y).abs() < 1e-5);
        }
    }

    #[test]
    fn downsample_96k_and_192k() {
        for rate in [96_000u32, 192_000, 88_200] {
            let input = sine(rate, 1000.0, rate as usize, 1);
            let mut r = Resampler::new(rate, 48_000, 1);
            let mut out = Vec::new();
            r.process(&input, &mut out);
            assert!((out.len() as i64 - 48_000).abs() <= 3, "{rate}: {}", out.len());
            let f = freq(&out, 48_000, 1);
            assert!((f - 1000.0).abs() < 2.0, "{rate}: {f}");
            let peak = out.iter().fold(0f32, |m, v| m.max(v.abs()));
            assert!(peak > 0.95 && peak <= 1.0, "{rate}: peak {peak}");
        }
    }
}
