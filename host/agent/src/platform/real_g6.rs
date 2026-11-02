//! g6 (Docs/protocol/everyday.md) calls on this PC: clipboard-history exclusion formats,
//! Explorer "reveal", Windows notifications (`UserNotificationListener`), thermal-zone
//! temperature (PDH) and system uptime. Written from the Microsoft Learn documentation.

use std::path::Path;

use super::{NotifyAccess, PResult, PcNotification};
use crate::router::RpcError;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// ---------------------------------------------------------------- clipboard

/// False when a password manager (or any app) asked monitors / history to skip this content.
pub fn clipboard_history_allowed() -> PResult<bool> {
    use windows::Win32::Foundation::HGLOBAL;
    use windows::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
        RegisterClipboardFormatW,
    };
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    use windows::core::PCWSTR;

    let format = |name: &str| {
        let w = wide(name);
        // SAFETY: NUL-terminated string that outlives the call.
        unsafe { RegisterClipboardFormatW(PCWSTR(w.as_ptr())) }
    };
    let exclude = format("ExcludeClipboardContentFromMonitorProcessing");
    let ignore = format("Clipboard Viewer Ignore");
    let can_include = format("CanIncludeInClipboardHistory");
    // SAFETY: plain queries of registered format ids.
    unsafe {
        if (exclude != 0 && IsClipboardFormatAvailable(exclude).is_ok())
            || (ignore != 0 && IsClipboardFormatAvailable(ignore).is_ok())
        {
            return Ok(false);
        }
        if can_include == 0 || IsClipboardFormatAvailable(can_include).is_err() {
            return Ok(true);
        }
    }
    // CanIncludeInClipboardHistory is present: read its DWORD (0 = do not record).
    let mut opened = false;
    for _ in 0..10 {
        // SAFETY: opening the clipboard without an owner window; closed below.
        if unsafe { OpenClipboard(None) }.is_ok() {
            opened = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(40));
    }
    if !opened {
        // Unknown: be safe and skip the item.
        return Ok(false);
    }
    // SAFETY: the clipboard is open; the handle is a global memory block owned by the
    // clipboard, locked and unlocked here and never freed by us.
    let value = unsafe {
        let v = match GetClipboardData(can_include) {
            Ok(h) if !h.is_invalid() => {
                let mem = HGLOBAL(h.0);
                let p = GlobalLock(mem) as *const u8;
                let v = if !p.is_null() && GlobalSize(mem) >= 4 {
                    Some(u32::from_ne_bytes([*p, *p.add(1), *p.add(2), *p.add(3)]))
                } else {
                    None
                };
                if !p.is_null() {
                    let _ = GlobalUnlock(mem);
                }
                v
            }
            _ => None,
        };
        let _ = CloseClipboard();
        v
    };
    // Unreadable counts as "do not record".
    Ok(value.is_some_and(|v| v != 0))
}

// ---------------------------------------------------------------- Explorer

pub fn reveal_in_explorer(path: &Path) -> PResult<()> {
    use std::os::windows::process::CommandExt;
    let p = path.to_string_lossy();
    if p.contains('"') {
        return Err(RpcError::bad_params("path contains a quote"));
    }
    std::process::Command::new("explorer.exe")
        .raw_arg(format!("/select,\"{p}\""))
        .spawn()
        .map(|_| ())
        .map_err(|e| RpcError::internal(format!("could not start Explorer: {e}")))
}

// ---------------------------------------------------------------- notifications

fn listener() -> windows::core::Result<windows::UI::Notifications::Management::UserNotificationListener> {
    windows::UI::Notifications::Management::UserNotificationListener::Current()
}

pub fn notify_access() -> NotifyAccess {
    use windows::UI::Notifications::Management::UserNotificationListenerAccessStatus as S;
    let Ok(l) = listener() else {
        return NotifyAccess::Unsupported;
    };
    let status = l
        .RequestAccessAsync()
        .and_then(|op| op.join())
        .or_else(|_| l.GetAccessStatus());
    match status {
        Ok(S::Allowed) => NotifyAccess::Allowed,
        Ok(S::Denied) => NotifyAccess::Denied,
        Ok(_) => NotifyAccess::Unspecified,
        Err(_) => NotifyAccess::Unsupported,
    }
}

/// 100 ns ticks between 1601-01-01 and 1970-01-01.
const FILETIME_UNIX_EPOCH: i64 = 116_444_736_000_000_000;

pub fn notifications() -> PResult<Vec<PcNotification>> {
    use windows::UI::Notifications::{KnownNotificationBindings, NotificationKinds};
    let err = |e: windows::core::Error| RpcError::internal(format!("notifications: {e}"));
    let l = listener().map_err(|e| RpcError::unsupported(format!("notifications: {e}")))?;
    let list = l
        .GetNotificationsAsync(NotificationKinds::Toast)
        .and_then(|op| op.join())
        .map_err(err)?;
    let generic = KnownNotificationBindings::ToastGeneric().map_err(err)?;
    let mut out = Vec::new();
    for i in 0..list.Size().map_err(err)? {
        let Ok(n) = list.GetAt(i) else { continue };
        let Ok(id) = n.Id() else { continue };
        let app = n
            .AppInfo()
            .and_then(|a| a.DisplayInfo())
            .and_then(|d| d.DisplayName())
            .map(|h| h.to_string())
            .unwrap_or_default();
        let time_ms = n
            .CreationTime()
            .map(|t| ((t.UniversalTime - FILETIME_UNIX_EPOCH).max(0) / 10_000) as u64)
            .unwrap_or(0);
        let texts: Vec<String> = n
            .Notification()
            .and_then(|x| x.Visual())
            .and_then(|v| v.GetBinding(&generic))
            .and_then(|b| b.GetTextElements())
            .map(|els| {
                (0..els.Size().unwrap_or(0))
                    .filter_map(|j| els.GetAt(j).ok())
                    .filter_map(|t| t.Text().ok().map(|h| h.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let title = texts.first().cloned().unwrap_or_default();
        let body = texts.get(1..).map(|r| r.join("\n")).unwrap_or_default();
        out.push(PcNotification {
            id: id.to_string(),
            app,
            title,
            body,
            time_ms,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------- health

/// Highest `\Thermal Zone Information(*)\Temperature` (Kelvin) in °C, if any.
pub fn cpu_temp_c() -> Option<f32> {
    use windows::Win32::System::Performance::{
        PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE_ITEM_W,
        PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA, PdhAddEnglishCounterW,
        PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhOpenQueryW,
    };
    use windows::core::PCWSTR;

    let mut query = PDH_HQUERY::default();
    // SAFETY: out-pointer to a local.
    if unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut query) } != 0 {
        return None;
    }
    let path = wide(r"\Thermal Zone Information(*)\Temperature");
    let mut counter = PDH_HCOUNTER::default();
    let mut best: Option<f64> = None;
    // SAFETY: valid query, NUL-terminated path, buffers sized as PDH asks; query closed below.
    unsafe {
        if PdhAddEnglishCounterW(query, PCWSTR(path.as_ptr()), 0, &mut counter) == 0
            && PdhCollectQueryData(query) == 0
        {
            let mut size = 0u32;
            let mut count = 0u32;
            let rc = PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, None);
            if rc == PDH_MORE_DATA && size > 0 {
                let item = size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>();
                let mut buf: Vec<PDH_FMT_COUNTERVALUE_ITEM_W> =
                    vec![Default::default(); (size as usize).div_ceil(item)];
                if PdhGetFormattedCounterArrayW(
                    counter,
                    PDH_FMT_DOUBLE,
                    &mut size,
                    &mut count,
                    Some(buf.as_mut_ptr()),
                ) == 0
                {
                    for it in buf.iter().take(count as usize) {
                        let st = it.FmtValue.CStatus;
                        if st != PDH_CSTATUS_VALID_DATA && st != PDH_CSTATUS_NEW_DATA {
                            continue;
                        }
                        let kelvin = it.FmtValue.Anonymous.doubleValue;
                        if kelvin > 200.0 && kelvin < 400.0 {
                            best = Some(best.map_or(kelvin, |b: f64| b.max(kelvin)));
                        }
                    }
                }
            }
        }
        PdhCloseQuery(query);
    }
    best.map(|k| ((k - 273.15) * 10.0).round() as f32 / 10.0)
}

pub fn uptime_s() -> u64 {
    // SAFETY: plain query.
    unsafe { windows::Win32::System::SystemInformation::GetTickCount64() / 1000 }
}
