//! Display brightness.
//!
//! - Internal panels (laptops, tablets): WMI `root\wmi` classes
//!   `WmiMonitorBrightness` (read) and `WmiMonitorBrightnessMethods.WmiSetBrightness` (write).
//! - External monitors: DDC/CI via `dxva2` (`GetPhysicalMonitorsFromHMONITOR`,
//!   `GetMonitorBrightness`, `SetMonitorBrightness`). Many monitors don't
//!   implement DDC/CI or have it disabled in their OSD; those are listed with
//!   `supported: false`. DDC/CI is slow (tens to hundreds of ms per monitor),
//!   so monitors that failed are not retried for [`DDC_RETRY`].
//!
//! Each HMONITOR is matched to its WMI instance by hardware/instance id, so an
//! internal panel is never also probed over DDC/CI.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// Internal panel or external monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayKind {
    /// Built-in panel controlled through WMI.
    Internal,
    /// External monitor controlled through DDC/CI.
    External,
}

/// A display and its brightness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Display {
    /// Stable-for-this-session id to pass to [`set_brightness`]
    /// (`wmi:<instance>` or `ddc:<\\.\DISPLAYn>#<index>`; external ids may
    /// change when monitors are reconnected).
    pub id: String,
    /// Human readable name.
    pub name: String,
    /// Internal or external.
    pub kind: DisplayKind,
    /// Brightness 0..=100 (0 when `supported` is false).
    pub level: u8,
    /// Whether brightness can be read and set.
    pub supported: bool,
}

/// How long a monitor that failed DDC/CI is skipped before being probed again.
pub const DDC_RETRY: Duration = Duration::from_secs(300);

/// All active displays with their brightness. Never fails just because a
/// display has no brightness control; such displays get `supported: false`.
pub fn brightness_list() -> Result<Vec<Display>> {
    imp::brightness_list()
}

/// Set brightness (0..=100) of the display with the given [`Display::id`].
pub fn set_brightness(id: &str, level: u8) -> Result<()> {
    if level > 100 {
        return Err(Error::InvalidArgument(format!("brightness {level} not in 0..=100")));
    }
    imp::set_brightness(id, level)
}

/// Normalise a WMI instance name or a monitor device-interface path to a
/// comparable key: `DISPLAY\BOE0A1B\4&2b4fa8a2&0&UID265988_0` and
/// `\\?\DISPLAY#BOE0A1B#4&2b4fa8a2&0&UID265988#{e6f07b5f-...}` both become
/// `display\boe0a1b\4&2b4fa8a2&0&uid265988`.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn monitor_key(s: &str) -> String {
    let s = s.strip_prefix(r"\\?\").unwrap_or(s);
    let s = match s.find("#{") {
        Some(i) => &s[..i],
        None => s,
    };
    let s = s.replace('#', "\\");
    // WMI instance names end with "_<n>".
    let s = match s.rfind('_') {
        Some(i) if s[i + 1..].chars().all(|c| c.is_ascii_digit()) && i + 1 < s.len() => &s[..i],
        _ => &s[..],
    };
    s.to_ascii_lowercase()
}

/// Map a DDC/CI raw value to 0..=100.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn to_percent(min: u32, cur: u32, max: u32) -> u8 {
    if max <= min {
        return 0;
    }
    let cur = cur.clamp(min, max);
    (((cur - min) as u64 * 100 + (max - min) as u64 / 2) / (max - min) as u64) as u8
}

/// Map 0..=100 to a DDC/CI raw value.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn from_percent(min: u32, level: u8, max: u32) -> u32 {
    if max <= min {
        return min;
    }
    min + (((max - min) as u64 * level.min(100) as u64 + 50) / 100) as u32
}

#[cfg(windows)]
mod imp {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;

    use windows::Win32::Devices::Display::{
        DestroyPhysicalMonitors, GetMonitorBrightness, GetNumberOfPhysicalMonitorsFromHMONITOR,
        GetPhysicalMonitorsFromHMONITOR, PHYSICAL_MONITOR, SetMonitorBrightness,
    };
    use windows::Win32::Foundation::{LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{
        DISPLAY_DEVICEW, EnumDisplayDevicesW, EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
        MONITORINFOEXW,
    };
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, CoCreateInstance, CoSetProxyBlanket, EOAC_NONE, RPC_C_AUTHN_LEVEL_CALL,
        RPC_C_IMP_LEVEL_IMPERSONATE,
    };
    use windows::Win32::System::Rpc::{RPC_C_AUTHN_WINNT, RPC_C_AUTHZ_NONE};
    use windows::Win32::System::Variant::{VARIANT, VT_BSTR, VT_I4, VT_UI1, VariantClear, VariantToUInt32};
    use windows::Win32::System::Wmi::{
        IWbemClassObject, IWbemLocator, IWbemServices, WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY,
        WBEM_FLAG_RETURN_WBEM_COMPLETE, WBEM_INFINITE, WbemLocator,
    };
    use windows::core::{BOOL, BSTR, PCWSTR};

    use super::{DDC_RETRY, Display, DisplayKind, from_percent, monitor_key, to_percent};
    use crate::worker::{static_worker, to_wide, wide_to_string};
    use crate::{Error, Result};

    static_worker!(worker, "helm-display");

    const EDD_GET_DEVICE_INTERFACE_NAME: u32 = 1;

    // ----- WMI ------------------------------------------------------------

    fn wmi() -> Result<IWbemServices> {
        unsafe {
            let loc: IWbemLocator = CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER)?;
            let empty = BSTR::new();
            let svc = loc.ConnectServer(&BSTR::from(r"ROOT\WMI"), &empty, &empty, &empty, 0, &empty, None)?;
            CoSetProxyBlanket(
                &svc,
                RPC_C_AUTHN_WINNT,
                RPC_C_AUTHZ_NONE,
                PCWSTR::null(),
                RPC_C_AUTHN_LEVEL_CALL,
                RPC_C_IMP_LEVEL_IMPERSONATE,
                None,
                EOAC_NONE,
            )?;
            Ok(svc)
        }
    }

    fn query(svc: &IWbemServices, wql: &str) -> Result<Vec<IWbemClassObject>> {
        let mut out = Vec::new();
        unsafe {
            let en = svc.ExecQuery(
                &BSTR::from("WQL"),
                &BSTR::from(wql),
                WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
                None,
            )?;
            loop {
                let mut objs = [None];
                let mut n = 0u32;
                let hr = en.Next(WBEM_INFINITE, &mut objs, &mut n);
                if hr.is_err() {
                    // e.g. WBEM_E_NOT_SUPPORTED when the class has no instances.
                    if out.is_empty() {
                        return Err(windows::core::Error::from(hr).into());
                    }
                    break;
                }
                match objs[0].take() {
                    Some(o) if n > 0 => out.push(o),
                    _ => break,
                }
            }
        }
        Ok(out)
    }

    struct Var(VARIANT);
    impl Drop for Var {
        fn drop(&mut self) {
            let _ = unsafe { VariantClear(&mut self.0) };
        }
    }

    fn get(obj: &IWbemClassObject, name: &str) -> Result<Var> {
        let w = to_wide(name);
        let mut v = Var(VARIANT::default());
        unsafe { obj.Get(PCWSTR(w.as_ptr()), 0, &mut v.0, None, None)? };
        Ok(v)
    }

    fn get_string(obj: &IWbemClassObject, name: &str) -> Result<String> {
        let v = get(obj, name)?;
        unsafe {
            let inner = &v.0.Anonymous.Anonymous;
            if inner.vt == VT_BSTR { Ok(inner.Anonymous.bstrVal.to_string()) } else { Ok(String::new()) }
        }
    }

    fn get_u32(obj: &IWbemClassObject, name: &str) -> Result<u32> {
        let v = get(obj, name)?;
        Ok(unsafe { VariantToUInt32(&v.0)? })
    }

    /// (instance name, level) of active internal panels.
    fn wmi_list(svc: &IWbemServices) -> Vec<(String, u8)> {
        let Ok(objs) = query(svc, "SELECT InstanceName, CurrentBrightness FROM WmiMonitorBrightness WHERE Active = TRUE")
        else {
            return Vec::new();
        };
        objs.iter()
            .filter_map(|o| {
                let name = get_string(o, "InstanceName").ok()?;
                let level = get_u32(o, "CurrentBrightness").ok()?;
                Some((name, level.min(100) as u8))
            })
            .collect()
    }

    fn wmi_set(instance: &str, level: u8) -> Result<()> {
        let svc = wmi()?;
        let objs = query(&svc, "SELECT * FROM WmiMonitorBrightnessMethods WHERE Active = TRUE")?;
        let target = objs
            .iter()
            .find(|o| get_string(o, "InstanceName").map(|n| n.eq_ignore_ascii_case(instance)).unwrap_or(false))
            .ok_or_else(|| Error::NotFound(format!("display wmi:{instance}")))?;
        let path = get_string(target, "__PATH")?;
        unsafe {
            let mut class = None;
            svc.GetObject(
                &BSTR::from("WmiMonitorBrightnessMethods"),
                WBEM_FLAG_RETURN_WBEM_COMPLETE,
                None,
                Some(&mut class),
                None,
            )?;
            let class = class.ok_or_else(|| Error::Internal("WmiMonitorBrightnessMethods class missing".into()))?;
            let mut in_sig = None;
            let m = to_wide("WmiSetBrightness");
            class.GetMethod(PCWSTR(m.as_ptr()), 0, &mut in_sig, std::ptr::null_mut())?;
            let params = in_sig
                .ok_or_else(|| Error::Internal("WmiSetBrightness signature missing".into()))?
                .SpawnInstance(0)?;

            // CIM uint32 travels as VT_I4, uint8 as VT_UI1.
            let mut timeout = VARIANT::default();
            (*timeout.Anonymous.Anonymous).vt = VT_I4;
            (*timeout.Anonymous.Anonymous).Anonymous.lVal = 1;
            let mut bright = VARIANT::default();
            (*bright.Anonymous.Anonymous).vt = VT_UI1;
            (*bright.Anonymous.Anonymous).Anonymous.bVal = level;
            let t = to_wide("Timeout");
            let b = to_wide("Brightness");
            params.Put(PCWSTR(t.as_ptr()), 0, &timeout, 0)?;
            params.Put(PCWSTR(b.as_ptr()), 0, &bright, 0)?;
            svc.ExecMethod(
                &BSTR::from(path),
                &BSTR::from("WmiSetBrightness"),
                WBEM_FLAG_RETURN_WBEM_COMPLETE,
                None,
                &params,
                None,
                None,
            )?;
        }
        Ok(())
    }

    // ----- monitors / DDC/CI ---------------------------------------------

    struct Monitor {
        hmon: HMONITOR,
        device: String,
        /// (normalised key, DeviceString) of each attached monitor device.
        attached: Vec<(String, String)>,
    }

    unsafe extern "system" fn collect(hmon: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        let v = unsafe { &mut *(data.0 as *mut Vec<HMONITOR>) };
        v.push(hmon);
        BOOL(1)
    }

    fn monitors() -> Vec<Monitor> {
        let mut hmons: Vec<HMONITOR> = Vec::new();
        unsafe {
            let _ = EnumDisplayMonitors(None, None, Some(collect), LPARAM(&mut hmons as *mut _ as isize));
        }
        hmons
            .into_iter()
            .filter_map(|hmon| {
                let mut mi = MONITORINFOEXW::default();
                mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
                if !unsafe { GetMonitorInfoW(hmon, &mut mi as *mut _ as *mut MONITORINFO) }.as_bool() {
                    return None;
                }
                let device = wide_to_string(&mi.szDevice);
                let dev_w = to_wide(&device);
                let mut attached = Vec::new();
                for i in 0..16 {
                    let mut dd = DISPLAY_DEVICEW { cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
                    let ok = unsafe {
                        EnumDisplayDevicesW(PCWSTR(dev_w.as_ptr()), i, &mut dd, EDD_GET_DEVICE_INTERFACE_NAME)
                    };
                    if !ok.as_bool() {
                        break;
                    }
                    attached.push((monitor_key(&wide_to_string(&dd.DeviceID)), wide_to_string(&dd.DeviceString)));
                }
                Some(Monitor { hmon, device, attached })
            })
            .collect()
    }

    /// Physical monitor handles for an HMONITOR, destroyed on drop.
    struct Physical(Vec<PHYSICAL_MONITOR>);
    impl Physical {
        fn get(hmon: HMONITOR) -> Physical {
            let mut n = 0u32;
            if unsafe { GetNumberOfPhysicalMonitorsFromHMONITOR(hmon, &mut n) }.is_err() || n == 0 {
                return Physical(Vec::new());
            }
            let mut v = vec![PHYSICAL_MONITOR::default(); n as usize];
            if unsafe { GetPhysicalMonitorsFromHMONITOR(hmon, &mut v) }.is_err() {
                return Physical(Vec::new());
            }
            Physical(v)
        }
    }
    impl Drop for Physical {
        fn drop(&mut self) {
            if !self.0.is_empty() {
                let _ = unsafe { DestroyPhysicalMonitors(&self.0) };
            }
        }
    }

    /// DDC/CI failures: id -> when it last failed.
    fn ddc_failures() -> &'static Mutex<HashMap<String, Instant>> {
        static F: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
        F.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn ddc_read(pm: &PHYSICAL_MONITOR) -> Option<(u32, u32, u32)> {
        let h = pm.hPhysicalMonitor;
        let (mut min, mut cur, mut max) = (0u32, 0u32, 0u32);
        let ok = unsafe { GetMonitorBrightness(h, &mut min, &mut cur, &mut max) };
        (ok != 0 && max > min).then_some((min, cur, max))
    }

    fn ddc_id(device: &str, index: usize) -> String {
        format!("ddc:{device}#{index}")
    }

    pub(super) fn brightness_list() -> Result<Vec<Display>> {
        worker().run(|| {
            let wmi_items = wmi().map(|s| wmi_list(&s)).unwrap_or_default();
            let mons = monitors();
            let mut out = Vec::new();

            for (inst, level) in &wmi_items {
                let key = monitor_key(inst);
                let name = mons
                    .iter()
                    .flat_map(|m| m.attached.iter())
                    .find(|(k, _)| *k == key)
                    .map(|(_, s)| s.clone())
                    .filter(|s| !s.is_empty() && s != "Generic PnP Monitor")
                    .unwrap_or_else(|| "Built-in display".to_string());
                out.push(Display {
                    id: format!("wmi:{inst}"),
                    name,
                    kind: DisplayKind::Internal,
                    level: *level,
                    supported: true,
                });
            }

            let wmi_keys: Vec<String> = wmi_items.iter().map(|(i, _)| monitor_key(i)).collect();
            for m in &mons {
                if m.attached.iter().any(|(k, _)| wmi_keys.contains(k)) {
                    continue; // internal panel, handled via WMI
                }
                let phys = Physical::get(m.hmon);
                for (idx, pm) in phys.0.iter().enumerate() {
                    let id = ddc_id(&m.device, idx);
                    let desc = pm.szPhysicalMonitorDescription;
                    let mut name = wide_to_string(&desc);
                    if name.is_empty() || name == "Generic PnP Monitor" {
                        name = m.attached.get(idx).map(|(_, s)| s.clone()).unwrap_or_default();
                    }
                    if name.is_empty() {
                        name = format!("Monitor {}", m.device.trim_start_matches(r"\\.\"));
                    }
                    let skip = ddc_failures()
                        .lock()
                        .ok()
                        .and_then(|f| f.get(&id).map(|t| t.elapsed() < DDC_RETRY))
                        .unwrap_or(false);
                    let read = if skip { None } else { ddc_read(pm) };
                    if read.is_none() && !skip {
                        if let Ok(mut f) = ddc_failures().lock() {
                            f.insert(id.clone(), Instant::now());
                        }
                    }
                    let (level, supported) = match read {
                        Some((min, cur, max)) => (to_percent(min, cur, max), true),
                        None => (0, false),
                    };
                    out.push(Display { id, name, kind: DisplayKind::External, level, supported });
                }
            }
            Ok(out)
        })?
    }

    fn ddc_set(id: &str, level: u8) -> Result<()> {
        let rest = &id["ddc:".len()..];
        let (device, idx) = rest
            .rsplit_once('#')
            .and_then(|(d, i)| i.parse::<usize>().ok().map(|i| (d, i)))
            .ok_or_else(|| Error::InvalidArgument(format!("bad display id {id}")))?;
        let mon = monitors()
            .into_iter()
            .find(|m| m.device.eq_ignore_ascii_case(device))
            .ok_or_else(|| Error::NotFound(format!("display {id}")))?;
        let phys = Physical::get(mon.hmon);
        let pm = phys.0.get(idx).ok_or_else(|| Error::NotFound(format!("display {id}")))?;
        let (min, _, max) = ddc_read(pm).ok_or_else(|| {
            if let Ok(mut f) = ddc_failures().lock() {
                f.insert(id.to_string(), Instant::now());
            }
            Error::Unsupported("monitor does not support DDC/CI brightness")
        })?;
        let h = pm.hPhysicalMonitor;
        if unsafe { SetMonitorBrightness(h, from_percent(min, level, max)) } == 0 {
            return Err(Error::Rejected(format!("monitor refused brightness change ({id})")));
        }
        if let Ok(mut f) = ddc_failures().lock() {
            f.remove(id);
        }
        Ok(())
    }

    pub(super) fn set_brightness(id: &str, level: u8) -> Result<()> {
        let id = id.to_string();
        worker().run(move || {
            if let Some(inst) = id.strip_prefix("wmi:") {
                wmi_set(inst, level)
            } else if id.starts_with("ddc:") {
                ddc_set(&id, level)
            } else {
                Err(Error::NotFound(format!("display {id}")))
            }
        })?
    }
}

#[cfg(not(windows))]
mod imp {
    use super::Display;
    use crate::{Error, Result};

    pub(super) fn brightness_list() -> Result<Vec<Display>> {
        Err(Error::Unsupported("brightness requires Windows"))
    }
    pub(super) fn set_brightness(_: &str, _: u8) -> Result<()> {
        Err(Error::Unsupported("brightness requires Windows"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_match_between_wmi_and_interface_paths() {
        let wmi = r"DISPLAY\BOE0A1B\4&2b4fa8a2&0&UID265988_0";
        let iface = r"\\?\DISPLAY#BOE0A1B#4&2b4fa8a2&0&UID265988#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";
        assert_eq!(monitor_key(wmi), monitor_key(iface));
        assert_eq!(monitor_key(wmi), r"display\boe0a1b\4&2b4fa8a2&0&uid265988");
        assert_ne!(monitor_key(wmi), monitor_key(r"DISPLAY\DEL4321\5&1&0&UID1_0"));
    }

    #[test]
    fn percent_mapping() {
        assert_eq!(to_percent(0, 50, 100), 50);
        assert_eq!(to_percent(0, 37, 75), 49);
        assert_eq!(to_percent(10, 10, 10), 0);
        assert_eq!(to_percent(0, 200, 100), 100);
        assert_eq!(from_percent(0, 50, 100), 50);
        assert_eq!(from_percent(0, 100, 75), 75);
        assert_eq!(from_percent(5, 0, 105), 5);
        for l in 0..=100u8 {
            assert_eq!(to_percent(0, from_percent(0, l, 100), 100), l);
        }
    }

    #[test]
    fn rejects_out_of_range() {
        assert!(matches!(set_brightness("wmi:x", 101), Err(Error::InvalidArgument(_))));
    }

    #[test]
    fn serde_shape() {
        let d = Display { id: "wmi:x".into(), name: "Built-in display".into(), kind: DisplayKind::Internal, level: 70, supported: true };
        let v = serde_json::to_value(d).unwrap();
        assert_eq!(v["kind"], "internal");
        assert_eq!(v["level"], 70);
    }
}
