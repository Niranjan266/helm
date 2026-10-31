//! Streaming statistics (pure Rust).

use std::collections::VecDeque;
use std::time::Instant;

/// How many recent latency samples are kept for the median / p95.
pub const LATENCY_WINDOW: usize = 1024;

/// The capture endpoint currently (or last) in use.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeviceInfo {
    /// Endpoint id (`{0.0.0.00000000}.{guid}`), stable across reboots.
    pub id: String,
    /// Friendly name, e.g. "Speakers (Realtek(R) Audio)".
    pub name: String,
    /// Mix format, e.g. "48000 Hz, 2 ch, F32".
    pub format: String,
    pub sample_rate: u32,
    pub channels: u16,
    /// True when `IAudioClient3::InitializeSharedAudioStream` (low-latency period) was used.
    pub low_latency: bool,
    /// Engine period in ms (event interval).
    pub period_ms: f64,
}

/// Snapshot of a running (or finished) stream. Returned by `AudioStreamer::stats()` / `stop()`.
#[derive(Debug, Clone, Default)]
pub struct AudioStats {
    /// Seconds since the stream started.
    pub elapsed_s: f64,
    /// Opus packets delivered to the callback.
    pub packets: u64,
    /// Packets whose PCM was all zeros (nothing playing, or synthesised by `fill_silence`).
    pub silent_packets: u64,
    pub bytes: u64,
    /// Average Opus bitrate in kbit/s over `elapsed_s`.
    pub bitrate_kbps: f64,
    /// Packets per second over `elapsed_s` (50 for 20 ms frames when steady).
    pub packets_per_s: f64,
    /// WASAPI packets read.
    pub device_packets: u64,
    /// Device frames read (at the device rate).
    pub device_frames: u64,
    /// `AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY` seen (the engine dropped data: we were late).
    pub discontinuities: u64,
    /// Output samples (per channel) of silence inserted for gaps / quiet periods.
    pub silence_samples_inserted: u64,
    /// Output samples (per channel) dropped to re-align with the QPC clock.
    pub samples_dropped: u64,
    /// How many times the capture device was (re)opened after the first one.
    pub device_changes: u64,
    /// Current device, if any.
    pub device: Option<DeviceInfo>,
    /// QPC time (µs, `QueryPerformanceCounter` scaled) of the sample with `pts_us = 0`. Add it
    /// to a packet's `pts_us` to compare with other QPC timestamps (e.g. video frames).
    pub qpc_origin_us: Option<i64>,
    /// Capture → callback latency: QPC when the packet is handed to the callback minus the
    /// `GetBuffer` QPC stamp of the device packet that completed it (WASAPI stamps loopback
    /// data when the engine mixes it, i.e. when it becomes readable, so this is read delay +
    /// conversion + Opus encode; the frame's own 20 ms of buffering is on top). Median / p95 /
    /// max over the last [`LATENCY_WINDOW`] packets made of device data (silent device packets
    /// included, synthesised silence excluded).
    pub latency_median_ms: f64,
    pub latency_p95_ms: f64,
    pub latency_max_ms: f64,
    pub latency_samples: usize,
    /// Median / max of (QPC when a WASAPI packet is read − its `GetBuffer` QPC position): how
    /// late the engine hands loopback data to us. Over the last [`LATENCY_WINDOW`] packets.
    pub device_delay_median_ms: f64,
    pub device_delay_max_ms: f64,
    /// Set when the stream stopped by itself (error).
    pub ended_reason: Option<String>,
    /// Last non-fatal error (e.g. no device right now; the stream keeps retrying).
    pub last_error: Option<String>,
}

#[derive(Debug, Default)]
pub(crate) struct StatsAccumulator {
    pub packets: u64,
    pub silent_packets: u64,
    pub bytes: u64,
    pub device_packets: u64,
    pub device_frames: u64,
    pub discontinuities: u64,
    pub silence_samples_inserted: u64,
    pub samples_dropped: u64,
    pub device_changes: u64,
    pub device: Option<DeviceInfo>,
    pub qpc_origin_us: Option<i64>,
    pub latencies_us: VecDeque<u32>,
    pub device_delays_us: VecDeque<i32>,
    pub started: Option<Instant>,
    pub finished: Option<Instant>,
    pub ended_reason: Option<String>,
    pub last_error: Option<String>,
}

/// Value at quantile `q` (0..=1) of a sorted slice (nearest rank).
pub fn percentile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = (q * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

impl StatsAccumulator {
    pub fn push_latency_us(&mut self, us: u32) {
        if self.latencies_us.len() == LATENCY_WINDOW {
            self.latencies_us.pop_front();
        }
        self.latencies_us.push_back(us);
    }

    pub fn push_device_delay_us(&mut self, us: i32) {
        if self.device_delays_us.len() == LATENCY_WINDOW {
            self.device_delays_us.pop_front();
        }
        self.device_delays_us.push_back(us);
    }

    pub fn snapshot(&self) -> AudioStats {
        let mut dd: Vec<f64> = self.device_delays_us.iter().map(|&u| u as f64 / 1000.0).collect();
        dd.sort_by(|a, b| a.total_cmp(b));
        let elapsed_s = match self.started {
            Some(s) => self
                .finished
                .unwrap_or_else(Instant::now)
                .duration_since(s)
                .as_secs_f64(),
            None => 0.0,
        };
        let mut lat: Vec<f64> = self.latencies_us.iter().map(|&u| u as f64 / 1000.0).collect();
        lat.sort_by(|a, b| a.total_cmp(b));
        let per_s = |n: f64| if elapsed_s > 0.0 { n / elapsed_s } else { 0.0 };
        AudioStats {
            elapsed_s,
            packets: self.packets,
            silent_packets: self.silent_packets,
            bytes: self.bytes,
            bitrate_kbps: per_s(self.bytes as f64 * 8.0 / 1000.0),
            packets_per_s: per_s(self.packets as f64),
            device_packets: self.device_packets,
            device_frames: self.device_frames,
            discontinuities: self.discontinuities,
            silence_samples_inserted: self.silence_samples_inserted,
            samples_dropped: self.samples_dropped,
            device_changes: self.device_changes,
            device: self.device.clone(),
            qpc_origin_us: self.qpc_origin_us,
            latency_median_ms: percentile(&lat, 0.5),
            latency_p95_ms: percentile(&lat, 0.95),
            latency_max_ms: lat.last().copied().unwrap_or(0.0),
            latency_samples: lat.len(),
            device_delay_median_ms: percentile(&dd, 0.5),
            device_delay_max_ms: dd.last().copied().unwrap_or(0.0),
            ended_reason: self.ended_reason.clone(),
            last_error: self.last_error.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles() {
        let v: Vec<f64> = (1..=100).map(|x| x as f64).collect();
        assert_eq!(percentile(&v, 0.5), 50.0);
        assert_eq!(percentile(&v, 0.95), 95.0);
        assert_eq!(percentile(&[], 0.5), 0.0);
        assert_eq!(percentile(&[3.0], 0.95), 3.0);
    }

    #[test]
    fn latency_window_is_bounded() {
        let mut a = StatsAccumulator::default();
        for i in 0..(LATENCY_WINDOW as u32 + 10) {
            a.push_latency_us(i);
        }
        let s = a.snapshot();
        assert_eq!(s.latency_samples, LATENCY_WINDOW);
        assert_eq!(s.latency_max_ms, (LATENCY_WINDOW as u32 + 9) as f64 / 1000.0);
    }
}
