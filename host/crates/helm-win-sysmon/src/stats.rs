//! Live system statistics: CPU, RAM, disks, network and GPU.
//!
//! CPU / RAM / disk / network come from the `sysinfo` crate; GPU from PDH +
//! DXGI (Windows only). Rates (disk and network bytes per second) are computed
//! between consecutive [`Sampler::sample`] calls, so the first sample reports
//! them as 0.

use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sysinfo::{CpuRefreshKind, Disks, MemoryRefreshKind, Networks, RefreshKind, System};

use crate::rate::RateTracker;
use crate::{Error, Result};

/// One disk / volume.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DiskStats {
    /// Mount point, e.g. `C:\`.
    pub mount: String,
    /// Volume label (may be empty).
    pub label: String,
    /// Bytes used.
    pub used: u64,
    /// Volume size in bytes.
    pub total: u64,
    /// Read bytes per second since the previous sample.
    pub read_bps: u64,
    /// Written bytes per second since the previous sample.
    pub write_bps: u64,
    /// `true` for USB sticks and other removable media.
    pub removable: bool,
}

/// One network interface with traffic.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct NetStats {
    /// Interface name as Windows reports it (e.g. "Wi-Fi").
    pub name: String,
    /// Received bytes per second since the previous sample.
    pub rx_bps: u64,
    /// Transmitted bytes per second since the previous sample.
    pub tx_bps: u64,
    /// Total bytes received since the interface came up.
    pub rx_total: u64,
    /// Total bytes transmitted since the interface came up.
    pub tx_total: u64,
}

/// One hardware GPU adapter.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct GpuStats {
    /// Adapter name from DXGI, e.g. "NVIDIA GeForce RTX 4060".
    pub name: String,
    /// Busiest engine utilisation, 0–100 (Task Manager's "GPU" figure).
    pub load_pct: f32,
    /// Dedicated VRAM in use, bytes.
    pub vram_used: u64,
    /// Dedicated VRAM size, bytes (small on integrated GPUs, which mostly use
    /// shared system memory).
    pub vram_total: u64,
    /// Temperature, °C. Not collected (needs a vendor/driver sidecar).
    pub temp_c: Option<f32>,
}

/// A full snapshot.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Stats {
    /// Unix time of the sample, milliseconds.
    pub timestamp_ms: u64,
    /// Time since the previous sample, milliseconds (0 for the first).
    pub interval_ms: u64,
    /// Total CPU usage, 0–100.
    pub cpu_total: f32,
    /// Per logical core usage, 0–100.
    pub cpu_per_core: Vec<f32>,
    /// CPU package temperature, °C. Not collected (out of scope).
    pub cpu_temp_c: Option<f32>,
    /// Physical memory in use, bytes.
    pub ram_used: u64,
    /// Physical memory size, bytes.
    pub ram_total: u64,
    /// Page file in use, bytes.
    pub swap_used: u64,
    /// Page file size, bytes.
    pub swap_total: u64,
    /// Volumes.
    pub disks: Vec<DiskStats>,
    /// Interfaces that have carried traffic.
    pub net: Vec<NetStats>,
    /// Hardware GPUs (empty on non-Windows or when counters are missing).
    pub gpu: Vec<GpuStats>,
}

impl Stats {
    /// RAM in use as a percentage (0 when total is unknown).
    pub fn ram_pct(&self) -> f32 {
        if self.ram_total == 0 { 0.0 } else { (self.ram_used as f64 * 100.0 / self.ram_total as f64) as f32 }
    }
}

struct Inner {
    sys: System,
    disks: Disks,
    nets: Networks,
    disk_rates: RateTracker,
    net_rates: RateTracker,
    /// Time of the last refresh (`None` until the first `sample`).
    last: Option<Instant>,
    /// When the CPU counters were last refreshed.
    cpu_at: Instant,
    #[cfg(windows)]
    gpu: crate::gpu::Gpu,
}

/// Collects [`Stats`]. Cheap to keep around; share it with an `Arc`.
///
/// Create one per consumer (rates are relative to *that* sampler's previous
/// call), e.g. one per `stats.subscribe` stream.
pub struct Sampler {
    inner: Mutex<Inner>,
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Sampler {
    /// Starts the counters. Takes a few tens of milliseconds.
    pub fn new() -> Self {
        let sys = System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::nothing().with_cpu_usage())
                .with_memory(MemoryRefreshKind::everything()),
        );
        Self {
            inner: Mutex::new(Inner {
                sys,
                disks: Disks::new_with_refreshed_list(),
                nets: Networks::new_with_refreshed_list(),
                disk_rates: RateTracker::new(),
                net_rates: RateTracker::new(),
                last: None,
                cpu_at: Instant::now(),
                #[cfg(windows)]
                gpu: crate::gpu::Gpu::new(),
            }),
        }
    }

    /// Takes a snapshot. Blocks for up to [`sysinfo::MINIMUM_CPU_UPDATE_INTERVAL`]
    /// (~200 ms) if called sooner than that after the previous call, so CPU
    /// figures are always meaningful.
    pub fn sample(&self) -> Result<Stats> {
        let mut g = self.inner.lock().map_err(|_| Error::Internal("sampler lock poisoned".into()))?;
        let inner = &mut *g;

        let since_cpu = inner.cpu_at.elapsed();
        if since_cpu < sysinfo::MINIMUM_CPU_UPDATE_INTERVAL {
            std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL - since_cpu);
        }
        inner.sys.refresh_cpu_usage();
        inner.cpu_at = Instant::now();
        inner.sys.refresh_memory();
        inner.disks.refresh(true);
        inner.nets.refresh(true);

        let now = Instant::now();
        let elapsed = inner.last.map(|t| now - t).unwrap_or(Duration::ZERO);
        inner.last = Some(now);

        let mut disks = Vec::new();
        let mut disk_keys = Vec::new();
        for d in inner.disks.list() {
            let total = d.total_space();
            if total == 0 {
                continue;
            }
            let mount = d.mount_point().to_string_lossy().into_owned();
            let usage = d.usage();
            let (rk, wk) = (format!("r:{mount}"), format!("w:{mount}"));
            let read_bps = inner.disk_rates.update(&rk, usage.total_read_bytes, elapsed);
            let write_bps = inner.disk_rates.update(&wk, usage.total_written_bytes, elapsed);
            disk_keys.push(rk);
            disk_keys.push(wk);
            disks.push(DiskStats {
                label: d.name().to_string_lossy().into_owned(),
                used: total.saturating_sub(d.available_space()),
                total,
                read_bps,
                write_bps,
                removable: d.is_removable(),
                mount,
            });
        }
        inner.disk_rates.retain(&disk_keys.iter().map(String::as_str).collect::<Vec<_>>());
        disks.sort_by(|a, b| a.mount.cmp(&b.mount));

        let mut net = Vec::new();
        let mut net_keys = Vec::new();
        for (name, data) in inner.nets.list() {
            let (rx_total, tx_total) = (data.total_received(), data.total_transmitted());
            let (rk, tk) = (format!("r:{name}"), format!("t:{name}"));
            let rx_bps = inner.net_rates.update(&rk, rx_total, elapsed);
            let tx_bps = inner.net_rates.update(&tk, tx_total, elapsed);
            net_keys.push(rk);
            net_keys.push(tk);
            if !keep_interface(name, rx_total, tx_total) {
                continue;
            }
            net.push(NetStats { name: name.clone(), rx_bps, tx_bps, rx_total, tx_total });
        }
        inner.net_rates.retain(&net_keys.iter().map(String::as_str).collect::<Vec<_>>());
        net.sort_by(|a, b| a.name.cmp(&b.name));

        #[cfg(windows)]
        let gpu = inner.gpu.sample();
        #[cfg(not(windows))]
        let gpu = Vec::new();

        let sys = &inner.sys;
        Ok(Stats {
            timestamp_ms: SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0),
            interval_ms: elapsed.as_millis() as u64,
            cpu_total: sys.global_cpu_usage().clamp(0.0, 100.0),
            cpu_per_core: sys.cpus().iter().map(|c| c.cpu_usage().clamp(0.0, 100.0)).collect(),
            cpu_temp_c: None,
            ram_used: sys.used_memory(),
            ram_total: sys.total_memory(),
            swap_used: sys.used_swap(),
            swap_total: sys.total_swap(),
            disks,
            net,
            gpu,
        })
    }
}

/// Hide loopback, idle and NDIS filter/QoS pseudo-interfaces.
pub(crate) fn keep_interface(name: &str, rx_total: u64, tx_total: u64) -> bool {
    let lower = name.to_ascii_lowercase();
    if rx_total == 0 && tx_total == 0 {
        return false;
    }
    !(lower.contains("loopback")
        || lower.contains("-wfp ")
        || lower.contains("filter-")
        || lower.contains("qos packet scheduler")
        || lower.contains("lightweight filter"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interface_filter() {
        assert!(keep_interface("Wi-Fi", 10, 0));
        assert!(!keep_interface("Wi-Fi", 0, 0));
        assert!(!keep_interface("Loopback Pseudo-Interface 1", 5, 5));
        assert!(!keep_interface("Wi-Fi-WFP Native MAC Layer LightWeight Filter-0000", 5, 5));
        assert!(!keep_interface("Ethernet-QoS Packet Scheduler-0000", 5, 5));
    }

    #[test]
    fn ram_pct() {
        let s = Stats { ram_used: 25, ram_total: 100, ..Default::default() };
        assert_eq!(s.ram_pct(), 25.0);
        assert_eq!(Stats::default().ram_pct(), 0.0);
    }

    #[test]
    fn serde_names_are_snake_case() {
        let s = Stats {
            disks: vec![DiskStats { mount: "C:\\".into(), ..Default::default() }],
            ..Default::default()
        };
        let j = serde_json::to_value(&s).unwrap();
        assert!(j.get("cpu_per_core").is_some());
        assert!(j["disks"][0].get("read_bps").is_some());
        assert!(j.get("cpu_temp_c").unwrap().is_null());
    }
}
