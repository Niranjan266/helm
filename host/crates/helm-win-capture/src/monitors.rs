//! Monitor enumeration: `EnumDisplayMonitors` + `GetMonitorInfoW` + `GetDpiForMonitor`, with
//! friendly names from the display configuration API (`QueryDisplayConfig`).

use std::collections::HashMap;

use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
    DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_OUTPUT_TECHNOLOGY_DISPLAYPORT_EMBEDDED,
    DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INTERNAL, DISPLAYCONFIG_OUTPUT_TECHNOLOGY_UDI_EMBEDDED, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SOURCE_DEVICE_NAME,
    DISPLAYCONFIG_TARGET_DEVICE_NAME, DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes,
    QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
};
use windows::Win32::Foundation::{ERROR_SUCCESS, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::core::BOOL;

use crate::dpi::ThreadDpiScope;
use crate::error::{Error, Result};

/// `MONITORINFOF_PRIMARY`
const MONITORINFOF_PRIMARY: u32 = 1;

/// One monitor, in **physical pixels** of the virtual desktop.
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorInfo {
    /// GDI device name, e.g. `\\.\DISPLAY1`. Stable while the display topology is unchanged;
    /// use it as `monitor_id` for [`crate::screenshot_png`] and the streamer.
    pub id: String,
    /// Friendly name from the monitor's EDID (e.g. `"DELL U2720Q"`), `"Built-in display"` for a
    /// nameless laptop panel, or `id` when unknown.
    pub name: String,
    /// Top-left corner in virtual-desktop coordinates (the primary monitor is at 0,0).
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub is_primary: bool,
    /// Effective DPI / 96 (1.0 = 100 %, 1.5 = 150 %).
    pub scale: f32,
}

#[derive(Debug, Clone)]
pub(crate) struct RawMonitor {
    pub handle: HMONITOR,
    pub info: MonitorInfo,
}

fn wide_to_string(w: &[u16]) -> String {
    let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..end])
}

/// Map GDI device name (`\\.\DISPLAY1`) → EDID friendly name. Best effort: empty on failure.
fn friendly_names() -> HashMap<String, String> {
    let mut map = HashMap::new();
    // SAFETY: buffers are sized by GetDisplayConfigBufferSizes and the counts are updated by
    // QueryDisplayConfig; every DisplayConfigGetDeviceInfo header has the right type and size.
    unsafe {
        let (mut np, mut nm) = (0u32, 0u32);
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm) != ERROR_SUCCESS {
            return map;
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
        if QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut np, paths.as_mut_ptr(), &mut nm, modes.as_mut_ptr(), None)
            != ERROR_SUCCESS
        {
            return map;
        }
        paths.truncate(np as usize);
        for p in &paths {
            let mut src = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
            src.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
            src.header.size = size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
            src.header.adapterId = p.sourceInfo.adapterId;
            src.header.id = p.sourceInfo.id;
            if DisplayConfigGetDeviceInfo(&mut src.header) != 0 {
                continue;
            }
            let mut tgt = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
            tgt.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
            tgt.header.size = size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
            tgt.header.adapterId = p.targetInfo.adapterId;
            tgt.header.id = p.targetInfo.id;
            if DisplayConfigGetDeviceInfo(&mut tgt.header) != 0 {
                continue;
            }
            let mut friendly = wide_to_string(&tgt.monitorFriendlyDeviceName);
            // Laptop panels often have no EDID name.
            let internal = [
                DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INTERNAL,
                DISPLAYCONFIG_OUTPUT_TECHNOLOGY_DISPLAYPORT_EMBEDDED,
                DISPLAYCONFIG_OUTPUT_TECHNOLOGY_UDI_EMBEDDED,
            ]
            .contains(&tgt.outputTechnology);
            if friendly.is_empty() && internal {
                friendly = "Built-in display".to_string();
            }
            if !friendly.is_empty() {
                map.insert(wide_to_string(&src.viewGdiDeviceName), friendly);
            }
        }
    }
    map
}

unsafe extern "system" fn enum_proc(hmon: HMONITOR, _hdc: HDC, _rect: *mut RECT, data: LPARAM) -> BOOL {
    // SAFETY: `data` is the `&mut Vec<HMONITOR>` passed by `raw_monitors`, alive for the call.
    let list = unsafe { &mut *(data.0 as *mut Vec<HMONITOR>) };
    list.push(hmon);
    true.into()
}

/// Enumerate monitors with their handles (handles are only valid until the topology changes).
pub(crate) fn raw_monitors() -> Vec<RawMonitor> {
    let _dpi = ThreadDpiScope::per_monitor_v2();
    let mut handles: Vec<HMONITOR> = Vec::new();
    // SAFETY: the callback only pushes into `handles`, which outlives the call.
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(enum_proc), LPARAM(&mut handles as *mut _ as isize));
    }
    let names = friendly_names();
    let mut out = Vec::with_capacity(handles.len());
    for h in handles {
        let mut mi = MONITORINFOEXW::default();
        mi.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        // SAFETY: MONITORINFOEXW starts with MONITORINFO and cbSize says which one we pass.
        let ok = unsafe { GetMonitorInfoW(h, &mut mi.monitorInfo as *mut MONITORINFO) }.as_bool();
        if !ok {
            continue;
        }
        let (mut dx, mut dy) = (96u32, 96u32);
        // SAFETY: valid monitor handle, out-pointers to locals.
        if unsafe { GetDpiForMonitor(h, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) }.is_err() {
            dx = 96;
        }
        let r = mi.monitorInfo.rcMonitor;
        let id = wide_to_string(&mi.szDevice);
        let name = names.get(&id).cloned().unwrap_or_else(|| id.clone());
        out.push(RawMonitor {
            handle: h,
            info: MonitorInfo {
                name,
                id,
                x: r.left,
                y: r.top,
                width: (r.right - r.left).max(0) as u32,
                height: (r.bottom - r.top).max(0) as u32,
                is_primary: mi.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
                scale: dx as f32 / 96.0,
            },
        });
    }
    // Primary first, then left-to-right, top-to-bottom: a stable, human order.
    out.sort_by_key(|m| (!m.info.is_primary, m.info.x, m.info.y));
    out
}

/// All monitors, primary first. Coordinates and sizes are physical pixels.
pub fn monitors() -> Vec<MonitorInfo> {
    raw_monitors().into_iter().map(|m| m.info).collect()
}

/// Find a monitor by id (`None` = primary).
pub(crate) fn find_monitor(id: Option<&str>) -> Result<RawMonitor> {
    let all = raw_monitors();
    let found = match id {
        None => all.iter().find(|m| m.info.is_primary).or(all.first()),
        Some(id) => all.iter().find(|m| m.info.id.eq_ignore_ascii_case(id)),
    };
    found.cloned().ok_or_else(|| Error::MonitorNotFound(id.unwrap_or("<primary>").to_string()))
}
