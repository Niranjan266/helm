//! Window extras: move a window to another monitor, snap it, and switch /
//! create / close virtual desktops.
//!
//! Monitor indices follow the same order as `helm-win-capture`: primary
//! first, then by x, then by y. Geometry is in physical pixels (the calls run
//! per-monitor DPI aware). Virtual desktop actions inject the standard
//! Win+Ctrl shortcuts with `SendInput`, so they act on the interactive
//! session and fail with [`crate::Error::Refused`] when the input is blocked
//! (e.g. an elevated window is in front and the agent is not elevated).

use serde::{Deserialize, Serialize};

use crate::Result;

/// Axis-aligned rectangle, physical pixels; `right`/`bottom` exclusive.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Rect {
    /// Left edge.
    pub left: i32,
    /// Top edge.
    pub top: i32,
    /// Right edge (exclusive).
    pub right: i32,
    /// Bottom edge (exclusive).
    pub bottom: i32,
}

impl Rect {
    /// Width (never negative).
    pub fn width(&self) -> i32 {
        (self.right - self.left).max(0)
    }
    /// Height (never negative).
    pub fn height(&self) -> i32 {
        (self.bottom - self.top).max(0)
    }
}

/// A display monitor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Monitor {
    /// Index used by [`move_to_monitor`].
    pub index: usize,
    /// GDI device name, e.g. `\\.\DISPLAY1`.
    pub device: String,
    /// Full monitor area.
    pub bounds: Rect,
    /// Area excluding the taskbar and docked app bars.
    pub work: Rect,
    /// Whether this is the primary monitor.
    pub is_primary: bool,
}

/// Snap target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Snap {
    /// Left half of the window's monitor work area.
    Left,
    /// Right half of the window's monitor work area.
    Right,
    /// Maximize.
    Maximize,
    /// Restore from maximized / minimized.
    Restore,
}

/// Virtual desktop action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VirtualDesktop {
    /// Switch to the desktop on the right (Win+Ctrl+Right).
    Next,
    /// Switch to the desktop on the left (Win+Ctrl+Left).
    Prev,
    /// Create a desktop and switch to it (Win+Ctrl+D).
    New,
    /// Close the current desktop; its windows move to a neighbour
    /// (Win+Ctrl+F4).
    Close,
}

/// Virtual-key codes (Win32 `VK_*`) pressed in order and released in reverse
/// for a [`VirtualDesktop`] action. The last key is the "action" key.
pub fn desktop_keys(action: VirtualDesktop) -> [u16; 3] {
    const VK_LWIN: u16 = 0x5B;
    const VK_LCONTROL: u16 = 0xA2;
    let key = match action {
        VirtualDesktop::Next => 0x27, // VK_RIGHT
        VirtualDesktop::Prev => 0x25, // VK_LEFT
        VirtualDesktop::New => b'D' as u16,
        VirtualDesktop::Close => 0x73, // VK_F4
    };
    [VK_LWIN, VK_LCONTROL, key]
}

/// Where a window of rect `win` lands when moved from work area `from` to
/// work area `to`: its size is kept (shrunk to fit), its relative position is
/// scaled, and it is clamped fully inside `to`.
pub fn place_on_monitor(win: Rect, from: Rect, to: Rect) -> Rect {
    let w = win.width().min(to.width());
    let h = win.height().min(to.height());
    let scale = |off: i32, from_len: i32, to_len: i32, size: i32| -> i32 {
        let free_from = (from_len - size).max(1) as i64;
        let free_to = (to_len - size).max(0) as i64;
        ((off.clamp(0, free_from as i32) as i64 * free_to) / free_from) as i32
    };
    let x = to.left + scale(win.left - from.left, from.width(), to.width(), w);
    let y = to.top + scale(win.top - from.top, from.height(), to.height(), h);
    let x = x.clamp(to.left, (to.right - w).max(to.left));
    let y = y.clamp(to.top, (to.bottom - h).max(to.top));
    Rect { left: x, top: y, right: x + w, bottom: y + h }
}

/// Target rect for [`Snap::Left`] / [`Snap::Right`] in work area `work`
/// (`None` for the other variants). The right half gets the odd pixel.
pub fn snap_rect(work: Rect, snap: Snap) -> Option<Rect> {
    let half = work.width() / 2;
    match snap {
        Snap::Left => Some(Rect { right: work.left + half, ..work }),
        Snap::Right => Some(Rect { left: work.left + half, ..work }),
        Snap::Maximize | Snap::Restore => None,
    }
}

/// Grows `target` by the invisible resize borders, given the window's
/// `GetWindowRect` (`outer`) and its visible DWM frame (`visible`).
pub fn compensate_frame(target: Rect, outer: Rect, visible: Rect) -> Rect {
    Rect {
        left: target.left - (visible.left - outer.left).max(0),
        top: target.top - (visible.top - outer.top).max(0),
        right: target.right + (outer.right - visible.right).max(0),
        bottom: target.bottom + (outer.bottom - visible.bottom).max(0),
    }
}

/// Sorts monitors primary first, then by x, then by y, and renumbers them.
pub fn order_monitors(mut m: Vec<Monitor>) -> Vec<Monitor> {
    m.sort_by_key(|m| (!m.is_primary, m.bounds.left, m.bounds.top));
    for (i, mon) in m.iter_mut().enumerate() {
        mon.index = i;
    }
    m
}

/// Lists monitors in index order.
pub fn monitors() -> Result<Vec<Monitor>> {
    os::monitors()
}

/// Moves a window to monitor `monitor_index`, keeping its size (shrunk to fit)
/// and relative position. A maximized window is re-maximized on the target;
/// a minimized one is restored first.
pub fn move_to_monitor(hwnd: u64, monitor_index: usize) -> Result<()> {
    os::move_to_monitor(hwnd, monitor_index)
}

/// Snaps a window to the left / right half of its monitor, or maximizes /
/// restores it.
pub fn snap(hwnd: u64, to: Snap) -> Result<()> {
    os::snap(hwnd, to)
}

/// Switches, creates or closes a virtual desktop via the Win+Ctrl shortcuts.
pub fn virtual_desktop(action: VirtualDesktop) -> Result<()> {
    os::virtual_desktop(action)
}

/// A window's outer rect (`GetWindowRect`), for tests and diagnostics.
pub fn window_rect(hwnd: u64) -> Result<Rect> {
    os::window_rect(hwnd)
}

#[cfg(windows)]
mod os {
    use windows::Win32::Foundation::{HWND, LPARAM, RECT};
    use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
    use windows::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO,
        MONITORINFOEXW, MonitorFromWindow,
    };
    use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
        SendInput, VIRTUAL_KEY,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowRect, IsIconic, IsWindow, IsZoomed, SW_MAXIMIZE, SW_RESTORE, SWP_NOACTIVATE, SWP_NOZORDER,
        SetWindowPos, ShowWindow,
    };
    use windows::core::BOOL;

    use super::{Monitor, Rect, Snap, VirtualDesktop, compensate_frame, desktop_keys, place_on_monitor, snap_rect};
    use crate::{Error, Result};

    const MONITORINFOF_PRIMARY: u32 = 1;

    /// Per-monitor-v2 DPI awareness for the current thread while alive.
    struct DpiScope(windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT);
    impl DpiScope {
        fn new() -> Self {
            // SAFETY: thread-local setting, restored on drop.
            Self(unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) })
        }
    }
    impl Drop for DpiScope {
        fn drop(&mut self) {
            if !self.0.is_invalid() {
                // SAFETY: restoring the previous context.
                unsafe { SetThreadDpiAwarenessContext(self.0) };
            }
        }
    }

    fn rect(r: RECT) -> Rect {
        Rect { left: r.left, top: r.top, right: r.right, bottom: r.bottom }
    }

    fn hwnd_of(v: u64) -> Result<HWND> {
        let h = HWND(v as usize as *mut core::ffi::c_void);
        // SAFETY: IsWindow accepts any value.
        if v == 0 || !unsafe { IsWindow(Some(h)) }.as_bool() {
            return Err(Error::NotFound(format!("window 0x{v:X}")));
        }
        Ok(h)
    }

    fn monitor_info(hmon: HMONITOR) -> Option<Monitor> {
        let mut mi = MONITORINFOEXW::default();
        mi.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        // SAFETY: MONITORINFOEXW with cbSize set, passed as MONITORINFO.
        if !unsafe { GetMonitorInfoW(hmon, &mut mi as *mut _ as *mut MONITORINFO) }.as_bool() {
            return None;
        }
        Some(Monitor {
            index: 0,
            device: crate::util::from_wide(&mi.szDevice),
            bounds: rect(mi.monitorInfo.rcMonitor),
            work: rect(mi.monitorInfo.rcWork),
            is_primary: mi.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
        })
    }

    pub(super) fn monitors() -> Result<Vec<Monitor>> {
        let _dpi = DpiScope::new();
        unsafe extern "system" fn proc_(h: HMONITOR, _: HDC, _: *mut RECT, lp: LPARAM) -> BOOL {
            // SAFETY: lp is the &mut Vec passed below.
            let v = unsafe { &mut *(lp.0 as *mut Vec<HMONITOR>) };
            v.push(h);
            BOOL(1)
        }
        let mut handles: Vec<HMONITOR> = Vec::new();
        // SAFETY: callback only pushes into `handles`, which outlives the call.
        unsafe {
            let _ = EnumDisplayMonitors(None, None, Some(proc_), LPARAM(&mut handles as *mut _ as isize));
        }
        Ok(super::order_monitors(handles.into_iter().filter_map(monitor_info).collect()))
    }

    fn get_rect(h: HWND) -> Result<Rect> {
        let mut r = RECT::default();
        // SAFETY: valid hwnd, out-pointer to a local.
        unsafe { GetWindowRect(h, &mut r) }?;
        Ok(rect(r))
    }

    pub(super) fn window_rect(hwnd: u64) -> Result<Rect> {
        let _dpi = DpiScope::new();
        get_rect(hwnd_of(hwnd)?)
    }

    fn visible_rect(h: HWND) -> Option<Rect> {
        let mut r = RECT::default();
        // SAFETY: RECT-sized buffer for DWMWA_EXTENDED_FRAME_BOUNDS.
        unsafe {
            DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut _ as *mut _, size_of::<RECT>() as u32)
        }
        .ok()?;
        Some(rect(r))
    }

    fn set_rect(h: HWND, r: Rect) -> Result<()> {
        // SAFETY: valid hwnd.
        unsafe { SetWindowPos(h, None, r.left, r.top, r.width(), r.height(), SWP_NOZORDER | SWP_NOACTIVATE) }?;
        Ok(())
    }

    /// Places the window so its *visible* frame fills `target`.
    fn set_visible_rect(h: HWND, target: Rect) -> Result<()> {
        let outer = get_rect(h)?;
        let r = visible_rect(h).map(|v| compensate_frame(target, outer, v)).unwrap_or(target);
        set_rect(h, r)
    }

    fn work_of(h: HWND) -> Result<Rect> {
        // SAFETY: valid hwnd.
        let hmon = unsafe { MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST) };
        monitor_info(hmon).map(|m| m.work).ok_or_else(|| Error::NotFound("window's monitor".into()))
    }

    /// Restores a minimized/maximized window; returns whether it was maximized.
    fn unmaximize(h: HWND) -> bool {
        // SAFETY: valid hwnd.
        unsafe {
            let zoomed = IsZoomed(h).as_bool();
            if zoomed || IsIconic(h).as_bool() {
                let _ = ShowWindow(h, SW_RESTORE);
            }
            zoomed
        }
    }

    pub(super) fn move_to_monitor(hwnd: u64, monitor_index: usize) -> Result<()> {
        let _dpi = DpiScope::new();
        let h = hwnd_of(hwnd)?;
        let mons = monitors()?;
        let target = mons
            .get(monitor_index)
            .ok_or_else(|| Error::NotFound(format!("monitor {monitor_index} (have {})", mons.len())))?;
        let was_max = unmaximize(h);
        let from = work_of(h)?;
        let outer = get_rect(h)?;
        set_rect(h, place_on_monitor(outer, from, target.work))?;
        if was_max {
            // SAFETY: valid hwnd.
            unsafe {
                let _ = ShowWindow(h, SW_MAXIMIZE);
            }
        }
        Ok(())
    }

    pub(super) fn snap(hwnd: u64, to: Snap) -> Result<()> {
        let _dpi = DpiScope::new();
        let h = hwnd_of(hwnd)?;
        match to {
            Snap::Maximize => {
                // SAFETY: valid hwnd.
                unsafe {
                    let _ = ShowWindow(h, SW_MAXIMIZE);
                }
                Ok(())
            }
            Snap::Restore => {
                // SAFETY: valid hwnd.
                unsafe {
                    let _ = ShowWindow(h, SW_RESTORE);
                }
                Ok(())
            }
            Snap::Left | Snap::Right => {
                unmaximize(h);
                let work = work_of(h)?;
                let target = snap_rect(work, to).expect("left/right have a rect");
                set_visible_rect(h, target)
            }
        }
    }

    fn key(vk: u16, up: bool) -> INPUT {
        let mut flags = KEYBD_EVENT_FLAGS(0);
        // Arrow keys are extended keys.
        if matches!(vk, 0x25..=0x28) {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }
        if up {
            flags |= KEYEVENTF_KEYUP;
        }
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 },
            },
        }
    }

    pub(super) fn virtual_desktop(action: VirtualDesktop) -> Result<()> {
        let keys = desktop_keys(action);
        let mut inputs: Vec<INPUT> = keys.iter().map(|&k| key(k, false)).collect();
        inputs.extend(keys.iter().rev().map(|&k| key(k, true)));
        // SAFETY: slice of fully initialised INPUTs.
        let sent = unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
        if sent as usize != inputs.len() {
            return Err(Error::Refused("SendInput was blocked (UIPI or secure desktop)".into()));
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod os {
    use super::{Monitor, Rect, Snap, VirtualDesktop};
    use crate::{Error, Result};

    pub(super) fn monitors() -> Result<Vec<Monitor>> {
        Err(Error::Unsupported("monitors"))
    }
    pub(super) fn move_to_monitor(_hwnd: u64, _i: usize) -> Result<()> {
        Err(Error::Unsupported("move window"))
    }
    pub(super) fn snap(_hwnd: u64, _to: Snap) -> Result<()> {
        Err(Error::Unsupported("snap window"))
    }
    pub(super) fn virtual_desktop(_a: VirtualDesktop) -> Result<()> {
        Err(Error::Unsupported("virtual desktops"))
    }
    pub(super) fn window_rect(_hwnd: u64) -> Result<Rect> {
        Err(Error::Unsupported("window rect"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(l: i32, t: i32, rr: i32, b: i32) -> Rect {
        Rect { left: l, top: t, right: rr, bottom: b }
    }

    #[test]
    fn place_keeps_size_and_relative_position() {
        let from = r(0, 0, 1920, 1040);
        let to = r(1920, 0, 3840, 1040);
        let win = r(100, 50, 900, 650);
        assert_eq!(place_on_monitor(win, from, to), r(2020, 50, 2820, 650));
    }

    #[test]
    fn place_shrinks_and_clamps_to_smaller_monitor() {
        let from = r(0, 0, 3840, 2160);
        let to = r(-1280, 0, 0, 1024);
        let win = r(3000, 1800, 5000, 3000); // partly off-screen, larger than target
        let p = place_on_monitor(win, from, to);
        assert_eq!((p.width(), p.height()), (1280, 1024));
        assert_eq!(p, to);
        let small = place_on_monitor(r(3500, 2000, 3800, 2100), from, to);
        assert!(small.left >= to.left && small.right <= to.right);
        assert!(small.top >= to.top && small.bottom <= to.bottom);
    }

    #[test]
    fn snap_halves() {
        let work = r(0, 0, 1921, 1040);
        assert_eq!(snap_rect(work, Snap::Left), Some(r(0, 0, 960, 1040)));
        assert_eq!(snap_rect(work, Snap::Right), Some(r(960, 0, 1921, 1040)));
        assert_eq!(snap_rect(work, Snap::Maximize), None);
    }

    #[test]
    fn frame_compensation() {
        let outer = r(93, 100, 507, 407);
        let visible = r(100, 100, 500, 400);
        assert_eq!(compensate_frame(r(0, 0, 960, 1040), outer, visible), r(-7, 0, 967, 1047));
    }

    #[test]
    fn desktop_shortcuts() {
        assert_eq!(desktop_keys(VirtualDesktop::Next), [0x5B, 0xA2, 0x27]);
        assert_eq!(desktop_keys(VirtualDesktop::Prev), [0x5B, 0xA2, 0x25]);
        assert_eq!(desktop_keys(VirtualDesktop::New), [0x5B, 0xA2, b'D' as u16]);
        assert_eq!(desktop_keys(VirtualDesktop::Close), [0x5B, 0xA2, 0x73]);
    }

    #[test]
    fn monitor_order() {
        let m = |x: i32, primary: bool| Monitor {
            index: 99,
            device: String::new(),
            bounds: r(x, 0, x + 100, 100),
            work: r(x, 0, x + 100, 100),
            is_primary: primary,
        };
        let o = order_monitors(vec![m(200, false), m(-100, false), m(0, true)]);
        assert_eq!(o.iter().map(|m| (m.bounds.left, m.index)).collect::<Vec<_>>(), vec![(0, 0), (-100, 1), (200, 2)]);
    }
}
