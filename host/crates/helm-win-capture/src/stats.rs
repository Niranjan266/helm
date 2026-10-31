//! Streaming statistics (pure Rust).

use std::collections::VecDeque;

/// How many recent latency samples are kept for the median / p95.
pub const LATENCY_WINDOW: usize = 1024;

/// Snapshot of a running (or finished) stream. Returned by `Streamer::stats()` / `stop()`.
#[derive(Debug, Clone, Default)]
pub struct StreamStats {
    /// Seconds since the first frame was submitted to the encoder.
    pub elapsed_s: f64,
    /// Fresh frames accepted from Windows.Graphics.Capture (after fps pacing).
    pub frames_captured: u64,
    /// Capture frames skipped because they arrived faster than the target fps.
    pub frames_paced_out: u64,
    /// Capture frames dropped because the encoder was behind (no free GPU slot / queue full).
    pub frames_dropped: u64,
    /// Frames re-submitted because the screen did not change (keeps a constant frame rate).
    pub frames_repeated: u64,
    /// Encoded access units delivered to the callback.
    pub frames_encoded: u64,
    pub keyframes: u64,
    pub bytes: u64,
    /// Encoded access units per second over `elapsed_s`.
    pub encoded_fps: f64,
    /// Fresh captured frames per second over `elapsed_s`.
    pub captured_fps: f64,
    /// Average output bitrate in kbit/s.
    pub bitrate_kbps: f64,
    /// Capture → encoded latency of fresh frames: from the moment Windows.Graphics.Capture hands
    /// us the frame (`FrameArrived`, QPC clock) to the moment the encoded access unit is handed
    /// to the callback (includes GPU colour conversion, queueing and encoding).
    /// Over the last [`LATENCY_WINDOW`] frames.
    pub latency_median_ms: f64,
    pub latency_p95_ms: f64,
    pub latency_max_ms: f64,
    pub latency_samples: usize,
    /// Set when the pipeline stopped by itself (error, source closed).
    pub ended_reason: Option<String>,
}

/// Mutable accumulator behind `StreamStats`.
#[derive(Debug, Default)]
pub(crate) struct StatsAccumulator {
    pub frames_captured: u64,
    pub frames_paced_out: u64,
    pub frames_dropped: u64,
    pub frames_repeated: u64,
    pub frames_encoded: u64,
    pub keyframes: u64,
    pub bytes: u64,
    /// Latencies in microseconds.
    pub latencies_us: VecDeque<u32>,
    pub started: Option<std::time::Instant>,
    pub finished: Option<std::time::Instant>,
    pub ended_reason: Option<String>,
}

impl StatsAccumulator {
    pub fn push_latency_us(&mut self, us: u32) {
        if self.latencies_us.len() == LATENCY_WINDOW {
            self.latencies_us.pop_front();
        }
        self.latencies_us.push_back(us);
    }

    pub fn snapshot(&self) -> StreamStats {
        let elapsed_s = match self.started {
            Some(s) => self.finished.unwrap_or_else(std::time::Instant::now).duration_since(s).as_secs_f64(),
            None => 0.0,
        };
        let mut lat: Vec<f64> = self.latencies_us.iter().map(|&u| u as f64 / 1000.0).collect();
        lat.sort_by(|a, b| a.total_cmp(b));
        let per_s = |n: u64| if elapsed_s > 0.0 { n as f64 / elapsed_s } else { 0.0 };
        StreamStats {
            elapsed_s,
            frames_captured: self.frames_captured,
            frames_paced_out: self.frames_paced_out,
            frames_dropped: self.frames_dropped,
            frames_repeated: self.frames_repeated,
            frames_encoded: self.frames_encoded,
            keyframes: self.keyframes,
            bytes: self.bytes,
            encoded_fps: per_s(self.frames_encoded),
            captured_fps: per_s(self.frames_captured),
            bitrate_kbps: per_s(self.bytes) * 8.0 / 1000.0,
            latency_median_ms: percentile(&lat, 50.0),
            latency_p95_ms: percentile(&lat, 95.0),
            latency_max_ms: lat.last().copied().unwrap_or(0.0),
            latency_samples: lat.len(),
            ended_reason: self.ended_reason.clone(),
        }
    }
}

/// Nearest-rank percentile of an ascending-sorted slice. Returns 0 for an empty slice.
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let p = p.clamp(0.0, 100.0);
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_nearest_rank() {
        let v: Vec<f64> = (1..=100).map(|x| x as f64).collect();
        assert_eq!(percentile(&v, 50.0), 50.0);
        assert_eq!(percentile(&v, 95.0), 95.0);
        assert_eq!(percentile(&v, 100.0), 100.0);
        assert_eq!(percentile(&v, 0.0), 1.0);
        assert_eq!(percentile(&[7.0], 95.0), 7.0);
        assert_eq!(percentile(&[], 50.0), 0.0);
    }

    #[test]
    fn latency_window_is_bounded() {
        let mut acc = StatsAccumulator::default();
        for i in 0..(LATENCY_WINDOW as u32 + 10) {
            acc.push_latency_us(i);
        }
        assert_eq!(acc.latencies_us.len(), LATENCY_WINDOW);
        assert_eq!(*acc.latencies_us.front().unwrap(), 10);
    }

    #[test]
    fn snapshot_computes_rates() {
        let mut acc = StatsAccumulator::default();
        let now = std::time::Instant::now();
        acc.started = Some(now);
        acc.finished = Some(now + std::time::Duration::from_secs(2));
        acc.frames_encoded = 120;
        acc.bytes = 250_000;
        for us in [1000, 2000, 3000, 4000] {
            acc.push_latency_us(us);
        }
        let s = acc.snapshot();
        assert!((s.encoded_fps - 60.0).abs() < 1e-9);
        assert!((s.bitrate_kbps - 1000.0).abs() < 1e-9);
        assert_eq!(s.latency_median_ms, 2.0);
        assert_eq!(s.latency_p95_ms, 4.0);
        assert_eq!(s.latency_max_ms, 4.0);
    }
}
