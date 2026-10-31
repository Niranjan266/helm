//! Top-level window listing and control.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, PROPERTYKEY, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::Storage::Packaging::Appx::GetApplicationUserModelId;
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PropVariantToStringAlloc};
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentThreadId, OpenProcess, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
    VIRTUAL_KEY, VK_MENU,
};
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumWindows, GW_OWNER, GWL_EXSTYLE, GetForegroundWindow, GetWindow,
    GetWindowLongPtrW, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic,
    IsWindow, IsWindowVisible, PostMessageW, SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE,
    SetForegroundWindow, ShowWindowAsync, WM_CLOSE, WS_EX_TOOLWINDOW,
};
use windows::core::{BOOL, GUID, PWSTR};

use super::{take_co_string, win_err};
use crate::{AppEntry, Error, FocusResult, Result, WindowInfo};

/// `System.AppUserModel.ID` (propkey.h): {9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3}, 5.
const PKEY_APP_USER_MODEL_ID: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3),
    pid: 5,
};

/// Unassigned virtual-key code, used so the injected Alt press does not open a menu.
const VK_UNASSIGNED: VIRTUAL_KEY = VIRTUAL_KEY(0xE8);

#[derive(Debug, Clone, Copy)]
pub enum ShowCmd {
    Minimize,
    Maximize,
    Restore,
}

fn to_hwnd(h: u64) -> HWND {
    HWND(h as usize as *mut core::ffi::c_void)
}

fn from_hwnd(h: HWND) -> u64 {
    h.0 as usize as u64
}

unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam is the &mut Vec passed by `top_level_windows` for the call's duration.
    let v = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
    v.push(hwnd);
    BOOL(1)
}

fn top_level_windows() -> Result<Vec<HWND>> {
    let mut v: Vec<HWND> = Vec::new();
    // SAFETY: the callback only touches `v`, which outlives EnumWindows.
    unsafe {
        EnumWindows(Some(collect), LPARAM(&mut v as *mut Vec<HWND> as isize))
            .map_err(win_err("EnumWindows"))?
    };
    Ok(v)
}

/// Returns the title if `hwnd` is a window we expose (visible, unowned, not a tool window,
/// not cloaked, non-empty title).
fn app_window_title(hwnd: HWND) -> Option<String> {
    // SAFETY: all calls accept any HWND value and fail gracefully on invalid ones.
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() || !IsWindowVisible(hwnd).as_bool() {
            return None;
        }
        if GetWindow(hwnd, GW_OWNER).is_ok_and(|o| !o.is_invalid()) {
            return None;
        }
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        if ex & WS_EX_TOOLWINDOW.0 != 0 {
            return None;
        }
        let mut cloaked = 0u32;
        if DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut _, 4).is_ok()
            && cloaked != 0
        {
            return None;
        }
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return None;
        }
        let mut buf = vec![0u16; len as usize + 1];
        let n = GetWindowTextW(hwnd, &mut buf);
        let title = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
        if title.trim().is_empty() {
            None
        } else {
            Some(title)
        }
    }
}

/// Resolves a caller-supplied handle to a window we expose, or `NotFound`.
fn checked(hwnd: u64) -> Result<HWND> {
    let h = to_hwnd(hwnd);
    if hwnd == 0 || app_window_title(h).is_none() {
        return Err(Error::NotFound(format!("no app window with hwnd {hwnd}")));
    }
    Ok(h)
}

struct OwnedHandle(HANDLE);
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: we own the process handle.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Image path and (for packaged processes) AUMID of a process.
fn process_info(pid: u32) -> (Option<String>, Option<String>) {
    // SAFETY: limited-information access; handle closed by OwnedHandle.
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return (None, None);
        };
        let h = OwnedHandle(h);
        let mut buf = vec![0u16; 32_768];
        let mut len = buf.len() as u32;
        let exe =
            QueryFullProcessImageNameW(h.0, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len)
                .ok()
                .map(|_| String::from_utf16_lossy(&buf[..len as usize]));
        let mut alen = buf.len() as u32;
        let aumid = (GetApplicationUserModelId(h.0, &mut alen, Some(PWSTR(buf.as_mut_ptr()))).0
            == 0)
            .then(|| String::from_utf16_lossy(&buf[..(alen as usize).saturating_sub(1)]))
            .filter(|s| !s.is_empty());
        (exe, aumid)
    }
}

/// Explicit AUMID set on the window itself (`SHGetPropertyStoreForWindow`).
fn window_aumid(hwnd: HWND) -> Option<String> {
    // SAFETY: COM calls on the STA; the PROPVARIANT is cleared after use.
    unsafe {
        let store: IPropertyStore = SHGetPropertyStoreForWindow(hwnd).ok()?;
        let mut pv = store.GetValue(&PKEY_APP_USER_MODEL_ID).ok()?;
        let s = PropVariantToStringAlloc(&pv)
            .ok()
            .and_then(|p| take_co_string(p));
        let _ = PropVariantClear(&mut pv);
        s.filter(|s| !s.is_empty())
    }
}

pub(crate) fn list(apps: &[AppEntry]) -> Result<Vec<WindowInfo>> {
    let by_id: HashMap<String, &str> = apps
        .iter()
        .map(|a| (a.app_id.to_lowercase(), a.app_id.as_str()))
        .collect();
    let by_exe: HashMap<String, &str> = apps
        .iter()
        .filter_map(|a| {
            let t = a.target_path.as_ref()?;
            t.to_lowercase()
                .ends_with(".exe")
                .then(|| (t.to_lowercase(), a.app_id.as_str()))
        })
        .collect();
    let mut procs: HashMap<u32, (Option<String>, Option<String>)> = HashMap::new();
    // SAFETY: no preconditions.
    let fg = unsafe { GetForegroundWindow() };
    let mut out = Vec::new();
    for hwnd in top_level_windows()? {
        let Some(title) = app_window_title(hwnd) else {
            continue;
        };
        let mut pid = 0u32;
        // SAFETY: valid out-pointer.
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        let (exe_path, proc_aumid) = procs
            .entry(pid)
            .or_insert_with(|| process_info(pid))
            .clone();
        // Prefer an AUMID that names a listed app (window's own, then the packaged
        // process's), then an exe-path match, then whatever raw AUMID the window carries
        // (e.g. a browser's per-profile id).
        let candidates: Vec<String> = [window_aumid(hwnd), proc_aumid]
            .into_iter()
            .flatten()
            .collect();
        let listed = |id: &String| by_id.get(&id.to_lowercase()).map(|s| s.to_string());
        let app_id = candidates
            .iter()
            .find_map(listed)
            .or_else(|| {
                exe_path
                    .as_ref()
                    .and_then(|e| by_exe.get(&e.to_lowercase()).map(|s| s.to_string()))
            })
            .or_else(|| candidates.into_iter().next());
        out.push(WindowInfo {
            hwnd: from_hwnd(hwnd),
            title,
            pid,
            exe_path,
            app_id,
            // SAFETY: any HWND accepted.
            is_minimized: unsafe { IsIconic(hwnd).as_bool() },
            is_foreground: hwnd == fg,
        });
    }
    Ok(out)
}

fn is_foreground(hwnd: HWND) -> bool {
    // SAFETY: no preconditions.
    unsafe { GetForegroundWindow() == hwnd }
}

/// Polls briefly, since activation of another thread's window can land a moment later.
fn wait_foreground(hwnd: HWND, timeout: Duration) -> bool {
    let start = Instant::now();
    loop {
        if is_foreground(hwnd) {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

pub(crate) fn focus(hwnd: u64) -> Result<FocusResult> {
    let h = checked(hwnd)?;
    let done = |method: &str| {
        Ok(FocusResult {
            focused: true,
            method: method.into(),
        })
    };
    let wait = Duration::from_millis(150);
    if is_foreground(h) {
        // SAFETY: any HWND accepted.
        if unsafe { IsIconic(h).as_bool() } {
            unsafe {
                let _ = ShowWindowAsync(h, SW_RESTORE);
            }
        }
        return done("already_foreground");
    }
    // SAFETY: plain user32 calls on a validated window; thread attachment is undone below.
    unsafe {
        if IsIconic(h).as_bool() {
            let _ = ShowWindowAsync(h, SW_RESTORE);
        }

        // 1. Plain SetForegroundWindow (works when we are allowed to set the foreground).
        let _ = SetForegroundWindow(h);
        if wait_foreground(h, wait) {
            return done("set_foreground_window");
        }

        // 2. Share input state with the current foreground thread, then retry.
        let me = GetCurrentThreadId();
        let fg = GetForegroundWindow();
        let fg_thread = if fg.is_invalid() {
            0
        } else {
            GetWindowThreadProcessId(fg, None)
        };
        if fg_thread != 0 && fg_thread != me && AttachThreadInput(me, fg_thread, true).as_bool() {
            let _ = BringWindowToTop(h);
            let _ = SetForegroundWindow(h);
            let _ = AttachThreadInput(me, fg_thread, false);
            if wait_foreground(h, wait) {
                return done("attach_thread_input");
            }
        }

        // 3. Make this process the source of the last input event by injecting an Alt
        //    press (with an unassigned key in between so no menu bar is activated).
        let inputs = [
            key(VK_MENU, false),
            key(VK_UNASSIGNED, false),
            key(VK_UNASSIGNED, true),
            key(VK_MENU, true),
        ];
        let sent = SendInput(&inputs, size_of::<INPUT>() as i32);
        if sent == inputs.len() as u32 {
            let _ = SetForegroundWindow(h);
            if wait_foreground(h, wait) {
                return done("alt_key_input");
            }
        }
    }
    Ok(FocusResult {
        focused: is_foreground(h),
        method: "none".into(),
    })
}

pub(crate) fn show(hwnd: u64, cmd: ShowCmd) -> Result<()> {
    let h = checked(hwnd)?;
    let sw = match cmd {
        ShowCmd::Minimize => SW_MINIMIZE,
        ShowCmd::Maximize => SW_MAXIMIZE,
        ShowCmd::Restore => SW_RESTORE,
    };
    // SAFETY: validated window; async variant never blocks on a hung target.
    unsafe {
        let _ = ShowWindowAsync(h, sw);
    }
    Ok(())
}

pub(crate) fn close(hwnd: u64) -> Result<()> {
    let h = checked(hwnd)?;
    // SAFETY: validated window; posting does not wait for the target.
    unsafe {
        PostMessageW(Some(h), WM_CLOSE, WPARAM(0), LPARAM(0))
            .map_err(win_err("PostMessageW(WM_CLOSE)"))
    }
}
