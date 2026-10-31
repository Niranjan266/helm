//! GPU load and VRAM via the PDH "GPU Engine" / "GPU Adapter Memory" counter
//! sets, matched to DXGI adapters (names, dedicated VRAM size) by LUID.
//!
//! Load follows the Task Manager convention: utilisation is summed per engine
//! across processes, and the adapter's load is its busiest engine.

use std::collections::HashMap;

use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1,
};
use windows::Win32::System::Performance::{
    PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_VALID_DATA, PDH_FMT, PDH_FMT_COUNTERVALUE_ITEM_W,
    PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA, PdhAddEnglishCounterW, PdhCloseQuery,
    PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhOpenQueryW,
};
use windows::core::PCWSTR;

use crate::stats::GpuStats;
use crate::util::wide;

/// Not exported by the `windows` crate: don't clamp percentages at 100.
const PDH_FMT_NOCAP100: u32 = 0x8000;

#[derive(Debug, Clone)]
struct Adapter {
    luid: u64,
    name: String,
    vram_total: u64,
}

pub(crate) struct Gpu {
    query: PDH_HQUERY,
    engine: Option<PDH_HCOUNTER>,
    memory: Option<PDH_HCOUNTER>,
    adapters: Vec<Adapter>,
}

// SAFETY: PDH query/counter handles are process-wide opaque handles that may
// be used from any thread; `Gpu` is only used behind the sampler's Mutex.
unsafe impl Send for Gpu {}

impl Drop for Gpu {
    fn drop(&mut self) {
        if !self.query.is_invalid() {
            // SAFETY: query opened in `new`, closed once here.
            unsafe {
                PdhCloseQuery(self.query);
            }
        }
    }
}

impl Gpu {
    /// Opens the counters. Never fails: missing pieces just yield less data.
    pub(crate) fn new() -> Self {
        let adapters = dxgi_adapters();
        let mut query = PDH_HQUERY::default();
        // SAFETY: out-pointer to a local.
        let rc = unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut query) };
        if rc != 0 {
            return Self { query: PDH_HQUERY::default(), engine: None, memory: None, adapters };
        }
        let engine = add_counter(query, r"\GPU Engine(*)\Utilization Percentage");
        let memory = add_counter(query, r"\GPU Adapter Memory(*)\Dedicated Usage");
        // Prime the rate counter; the first formatted read needs two samples.
        // SAFETY: valid query.
        unsafe {
            PdhCollectQueryData(query);
        }
        Self { query, engine, memory, adapters }
    }

    pub(crate) fn sample(&mut self) -> Vec<GpuStats> {
        if self.adapters.is_empty() {
            return Vec::new();
        }
        let mut load = HashMap::new();
        let mut used = HashMap::new();
        if !self.query.is_invalid() {
            // SAFETY: valid query.
            let rc = unsafe { PdhCollectQueryData(self.query) };
            if rc == 0 {
                if let Some(c) = self.engine {
                    load = engine_load_by_luid(&read_array(c));
                }
                if let Some(c) = self.memory {
                    used = memory_by_luid(&read_array(c));
                }
            }
        }
        self.adapters
            .iter()
            .map(|a| GpuStats {
                name: a.name.clone(),
                load_pct: load.get(&a.luid).copied().unwrap_or(0.0).clamp(0.0, 100.0) as f32,
                vram_used: used.get(&a.luid).copied().unwrap_or(0.0).max(0.0) as u64,
                vram_total: a.vram_total,
                temp_c: None,
            })
            .collect()
    }
}

fn add_counter(query: PDH_HQUERY, path: &str) -> Option<PDH_HCOUNTER> {
    let w = wide(path);
    let mut c = PDH_HCOUNTER::default();
    // SAFETY: NUL-terminated path, out-pointer to a local.
    let rc = unsafe { PdhAddEnglishCounterW(query, PCWSTR(w.as_ptr()), 0, &mut c) };
    (rc == 0).then_some(c)
}

/// Reads all instances of a wildcard counter as (instance name, value).
fn read_array(counter: PDH_HCOUNTER) -> Vec<(String, f64)> {
    let fmt = PDH_FMT(PDH_FMT_DOUBLE.0 | PDH_FMT_NOCAP100);
    let mut size = 0u32;
    let mut count = 0u32;
    // SAFETY: size query with no buffer.
    let rc = unsafe { PdhGetFormattedCounterArrayW(counter, fmt, &mut size, &mut count, None) };
    if rc != PDH_MORE_DATA || size == 0 {
        return Vec::new();
    }
    // Buffer is items followed by their name strings; align to the item type.
    let item = size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>();
    let mut buf: Vec<PDH_FMT_COUNTERVALUE_ITEM_W> = vec![Default::default(); (size as usize).div_ceil(item)];
    // SAFETY: buf holds at least `size` bytes, suitably aligned.
    let rc = unsafe { PdhGetFormattedCounterArrayW(counter, fmt, &mut size, &mut count, Some(buf.as_mut_ptr())) };
    if rc != 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(count as usize);
    for it in buf.iter().take(count as usize) {
        let status = it.FmtValue.CStatus;
        if status != PDH_CSTATUS_VALID_DATA && status != PDH_CSTATUS_NEW_DATA {
            continue;
        }
        // SAFETY: PDH filled szName with a NUL-terminated string inside buf;
        // the union holds a double because we asked for PDH_FMT_DOUBLE.
        let (name, value) = unsafe { (it.szName.to_string().unwrap_or_default(), it.FmtValue.Anonymous.doubleValue) };
        out.push((name, value));
    }
    out
}

fn dxgi_adapters() -> Vec<Adapter> {
    // SAFETY: COM factory creation; DXGI does not require CoInitialize.
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else {
        return Vec::new();
    };
    let mut out: Vec<Adapter> = Vec::new();
    for i in 0.. {
        // SAFETY: enumerating until DXGI_ERROR_NOT_FOUND.
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(i) }) else { break };
        // SAFETY: valid adapter.
        let Ok(desc) = (unsafe { adapter.GetDesc1() }) else { continue };
        if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            continue;
        }
        let luid = luid_key(desc.AdapterLuid.HighPart as u32, desc.AdapterLuid.LowPart);
        if out.iter().any(|a| a.luid == luid) {
            continue;
        }
        out.push(Adapter {
            luid,
            name: crate::util::from_wide(&desc.Description).trim().to_string(),
            vram_total: desc.DedicatedVideoMemory as u64,
        });
    }
    out
}

fn luid_key(high: u32, low: u32) -> u64 {
    (u64::from(high) << 32) | u64::from(low)
}

/// Parses `..._luid_0xHHHHHHHH_0xLLLLLLLL_...` from a PDH instance name.
pub(crate) fn parse_luid(instance: &str) -> Option<u64> {
    let rest = &instance[instance.find("luid_0x")? + 7..];
    let (high, rest) = rest.split_once('_')?;
    let rest = rest.strip_prefix("0x")?;
    let low = rest.split('_').next()?;
    Some(luid_key(u32::from_str_radix(high, 16).ok()?, u32::from_str_radix(low, 16).ok()?))
}

/// Engine id within an instance name: `phys_N_eng_M` (pid stripped).
fn engine_key(instance: &str) -> Option<&str> {
    let start = instance.find("phys_")?;
    let rest = &instance[start..];
    let end = rest.find("_engtype").unwrap_or(rest.len());
    Some(&rest[..end])
}

/// Busiest engine per adapter (sum over processes per engine, then max).
pub(crate) fn engine_load_by_luid(items: &[(String, f64)]) -> HashMap<u64, f64> {
    let mut per_engine: HashMap<(u64, &str), f64> = HashMap::new();
    for (name, v) in items {
        if let (Some(luid), Some(eng)) = (parse_luid(name), engine_key(name)) {
            *per_engine.entry((luid, eng)).or_default() += v;
        }
    }
    let mut out: HashMap<u64, f64> = HashMap::new();
    for ((luid, _), v) in per_engine {
        let e = out.entry(luid).or_default();
        *e = e.max(v);
    }
    out
}

/// Dedicated VRAM bytes in use per adapter.
pub(crate) fn memory_by_luid(items: &[(String, f64)]) -> HashMap<u64, f64> {
    let mut out: HashMap<u64, f64> = HashMap::new();
    for (name, v) in items {
        if let Some(luid) = parse_luid(name) {
            *out.entry(luid).or_default() += v;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_luid() {
        assert_eq!(
            parse_luid("pid_1234_luid_0x00000000_0x0000C3A1_phys_0_eng_0_engtype_3D"),
            Some(0xC3A1)
        );
        assert_eq!(parse_luid("luid_0x00000001_0x0000000F_phys_0"), Some((1 << 32) | 0xF));
        assert_eq!(parse_luid("_Total"), None);
        assert_eq!(parse_luid("luid_0xZZ_0x1"), None);
    }

    #[test]
    fn engine_load_sums_processes_and_takes_busiest_engine() {
        let items = vec![
            ("pid_1_luid_0x00000000_0x00000001_phys_0_eng_0_engtype_3D".to_string(), 30.0),
            ("pid_2_luid_0x00000000_0x00000001_phys_0_eng_0_engtype_3D".to_string(), 25.0),
            ("pid_2_luid_0x00000000_0x00000001_phys_0_eng_3_engtype_VideoDecode".to_string(), 40.0),
            ("pid_9_luid_0x00000000_0x00000002_phys_0_eng_0_engtype_3D".to_string(), 5.0),
        ];
        let m = engine_load_by_luid(&items);
        assert_eq!(m[&1], 55.0);
        assert_eq!(m[&2], 5.0);
    }

    #[test]
    fn memory_sums_per_adapter() {
        let items = vec![
            ("luid_0x00000000_0x00000001_phys_0".to_string(), 100.0),
            ("luid_0x00000000_0x00000001_phys_1".to_string(), 50.0),
            ("luid_0x00000000_0x00000002_phys_0".to_string(), 7.0),
        ];
        let m = memory_by_luid(&items);
        assert_eq!(m[&1], 150.0);
        assert_eq!(m[&2], 7.0);
    }
}
