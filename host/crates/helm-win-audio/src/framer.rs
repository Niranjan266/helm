//! Fixed-size framing and the QPC timeline (pure Rust).
//!
//! The output is one continuous sample timeline: packet `k` starts at output sample
//! `k * frame_samples`, so `pts_us = k * frame_ms * 1000` exactly and is strictly increasing.
//! [`Timeline`] keeps that timeline locked to the QPC clock: each WASAPI packet carries the QPC
//! time of its first sample; if it is far from where the timeline expects it (the source
//! paused, the device changed, a glitch), silence is inserted or samples are dropped.

/// Splits an interleaved sample stream into frames of `frame_len` values.
#[derive(Debug, Clone)]
pub struct Framer {
    frame_len: usize,
    buf: Vec<f32>,
    next_index: u64,
}

impl Framer {
    pub fn new(frame_len: usize) -> Framer {
        Framer {
            frame_len,
            buf: Vec::with_capacity(frame_len * 4),
            next_index: 0,
        }
    }

    pub fn push(&mut self, samples: &[f32]) {
        self.buf.extend_from_slice(samples);
    }

    pub fn push_silence(&mut self, values: usize) {
        self.buf.resize(self.buf.len() + values, 0.0);
    }

    /// Interleaved values waiting for a full frame.
    pub fn pending(&self) -> usize {
        self.buf.len()
    }

    /// Skip `frames` whole packets without emitting them (a long gap: their pts are never sent,
    /// the next packet's pts jumps). Only valid when no partial frame is pending.
    pub fn skip_frames(&mut self, frames: u64) {
        debug_assert!(self.buf.is_empty());
        self.next_index += frames;
    }

    /// Interleaved values needed to complete the pending partial frame (0 when none).
    pub fn to_frame_boundary(&self) -> usize {
        let r = self.buf.len() % self.frame_len;
        if r == 0 { 0 } else { self.frame_len - r }
    }

    /// Take the next complete frame: `(frame_index, samples)`. Call repeatedly.
    pub fn pop_into(&mut self, frame: &mut Vec<f32>) -> Option<u64> {
        if self.buf.len() < self.frame_len {
            return None;
        }
        frame.clear();
        frame.extend(self.buf.drain(..self.frame_len));
        let i = self.next_index;
        self.next_index += 1;
        Some(i)
    }
}

/// `pts_us` of packet `index` (exact integer math).
pub fn pts_us(index: u64, frame_samples: usize, sample_rate: u32) -> u64 {
    (index as u128 * frame_samples as u128 * 1_000_000 / sample_rate as u128) as u64
}

/// Microseconds covered by `samples` samples per channel.
pub fn samples_to_us(samples: u64, sample_rate: u32) -> i64 {
    (samples as i128 * 1_000_000 / sample_rate as i128) as i64
}

/// Samples per channel in `us` microseconds (rounded down, negative → 0).
pub fn us_to_samples(us: i64, sample_rate: u32) -> u64 {
    if us <= 0 {
        0
    } else {
        (us as i128 * sample_rate as i128 / 1_000_000) as u64
    }
}

/// What to do before appending a packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Adjust {
    /// The packet continues the timeline.
    None,
    /// Insert this many silent samples per channel first (a gap in the source).
    InsertSilence(u64),
    /// Drop this many samples per channel from the start of the packet's output (the timeline
    /// ran ahead, e.g. after synthesised silence).
    Drop(u64),
}

/// Maps the output sample position to QPC microseconds.
#[derive(Debug, Clone)]
pub struct Timeline {
    sample_rate: u32,
    /// QPC µs of output sample 0 (`None` until the first packet or silence fill).
    origin_us: Option<i64>,
    /// Output samples per channel appended so far (real + silence − dropped).
    position: u64,
    /// |error| allowed before correcting.
    tolerance_us: i64,
    /// Silence fill starts when the timeline lags `now` by more than this.
    fill_trigger_us: i64,
    /// While filling, keep the timeline this far behind `now`.
    fill_target_us: i64,
    filling: bool,
}

impl Timeline {
    pub fn new(sample_rate: u32, frame_ms: u32) -> Timeline {
        let frame_us = frame_ms as i64 * 1000;
        Timeline {
            sample_rate,
            origin_us: None,
            position: 0,
            tolerance_us: frame_us.max(20_000) + 10_000,
            fill_trigger_us: (2 * frame_us).max(50_000),
            fill_target_us: 10_000,
            filling: false,
        }
    }

    pub fn origin_us(&self) -> Option<i64> {
        self.origin_us
    }

    pub fn position(&self) -> u64 {
        self.position
    }

    pub fn is_filling(&self) -> bool {
        self.filling
    }

    /// QPC µs where the next appended sample belongs.
    pub fn expected_us(&self) -> Option<i64> {
        self.origin_us
            .map(|o| o + samples_to_us(self.position, self.sample_rate))
    }

    /// A packet whose first sample was recorded at `qpc_us` is about to be appended.
    pub fn reconcile(&mut self, qpc_us: i64) -> Adjust {
        self.filling = false;
        let Some(expected) = self.expected_us() else {
            self.origin_us = Some(qpc_us);
            return Adjust::None;
        };
        let diff = qpc_us - expected;
        if diff > self.tolerance_us {
            Adjust::InsertSilence(us_to_samples(diff, self.sample_rate))
        } else if diff < -self.tolerance_us {
            Adjust::Drop(us_to_samples(-diff, self.sample_rate))
        } else {
            Adjust::None
        }
    }

    /// Record that `samples` per channel were appended.
    pub fn advance(&mut self, samples: u64) {
        self.position += samples;
    }

    /// No data arrived; how many silent samples per channel should be synthesised now?
    /// (0 unless the source has been quiet for longer than the trigger.)
    pub fn silence_due(&mut self, now_us: i64) -> u64 {
        let Some(expected) = self.expected_us() else {
            // Nothing ever arrived: start the timeline now.
            self.origin_us = Some(now_us - self.fill_target_us);
            self.filling = true;
            return 0;
        };
        let lag = now_us - expected;
        if !self.filling && lag > self.fill_trigger_us {
            self.filling = true;
        }
        if self.filling && lag > self.fill_target_us {
            us_to_samples(lag - self.fill_target_us, self.sample_rate)
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_exact_sizes_and_indices() {
        let mut f = Framer::new(1920);
        let mut frame = Vec::new();
        f.push(&vec![0.5; 1000]);
        assert_eq!(f.pop_into(&mut frame), None);
        f.push(&vec![0.25; 3000]);
        assert_eq!(f.pop_into(&mut frame), Some(0));
        assert_eq!(frame.len(), 1920);
        assert_eq!(frame[999], 0.5);
        assert_eq!(frame[1000], 0.25);
        assert_eq!(f.pop_into(&mut frame), Some(1));
        assert_eq!(f.pop_into(&mut frame), None);
        assert_eq!(f.pending(), 4000 - 3840);
        f.push_silence(2000);
        assert_eq!(f.pop_into(&mut frame), Some(2));
        assert_eq!(frame[160], 0.0);
    }

    #[test]
    fn pts_math() {
        assert_eq!(pts_us(0, 960, 48_000), 0);
        assert_eq!(pts_us(1, 960, 48_000), 20_000);
        assert_eq!(pts_us(50, 960, 48_000), 1_000_000);
        // 24 h of packets: still exact.
        assert_eq!(pts_us(4_320_000, 960, 48_000), 86_400_000_000);
        assert_eq!(pts_us(3, 480, 24_000), 60_000);
        assert_eq!(samples_to_us(48_000, 48_000), 1_000_000);
        assert_eq!(us_to_samples(20_000, 48_000), 960);
        assert_eq!(us_to_samples(-5, 48_000), 0);
        let mut last = None;
        for i in 0..1000 {
            let p = pts_us(i, 960, 48_000);
            if let Some(l) = last {
                assert!(p > l);
            }
            last = Some(p);
        }
    }

    #[test]
    fn timeline_continuous_packets() {
        let mut t = Timeline::new(48_000, 20);
        let mut qpc = 1_000_000_000i64;
        for _ in 0..100 {
            // 10 ms packets with ±1 ms jitter on their timestamps.
            assert_eq!(t.reconcile(qpc + (qpc / 7 % 2000) - 1000), Adjust::None);
            t.advance(480);
            qpc += 10_000;
        }
        assert_eq!(t.origin_us(), Some(1_000_000_000 + (1_000_000_000 / 7 % 2000) - 1000));
        assert_eq!(t.position(), 48_000);
    }

    #[test]
    fn timeline_gap_inserts_silence_and_overlap_drops() {
        let mut t = Timeline::new(48_000, 20);
        t.reconcile(0);
        t.advance(4800); // 100 ms
        // Next packet 500 ms later than expected.
        match t.reconcile(600_000) {
            Adjust::InsertSilence(n) => assert_eq!(n, 24_000),
            a => panic!("{a:?}"),
        }
        t.advance(24_000);
        assert_eq!(t.expected_us(), Some(600_000));
        // Packet from 100 ms in the past.
        match t.reconcile(500_000) {
            Adjust::Drop(n) => assert_eq!(n, 4800),
            a => panic!("{a:?}"),
        }
    }

    #[test]
    fn silence_fill_hysteresis() {
        let mut t = Timeline::new(48_000, 20);
        // Nothing ever arrived: the first call only starts the timeline.
        assert_eq!(t.silence_due(1_000_000), 0);
        assert!(t.is_filling());
        assert_eq!(t.expected_us(), Some(990_000));
        // 20 ms later → 20 ms of silence due (keeps 10 ms behind now).
        assert_eq!(t.silence_due(1_020_000), 960);
        t.advance(960);
        // Real data resumes and stops filling.
        assert_eq!(t.reconcile(1_012_000), Adjust::None);
        t.advance(480);
        assert!(!t.is_filling());
        // A short pause (below the trigger) produces nothing...
        assert_eq!(t.silence_due(1_040_000), 0);
        // ...a long one fills up to 10 ms behind now.
        let due = t.silence_due(1_100_000);
        assert_eq!(due, us_to_samples(1_100_000 - 10_000 - 1_020_000, 48_000));
    }
}
