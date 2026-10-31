//! Foreground app watcher (for per-app deck/dock profiles).
//!
//! [`ForegroundWatcher::start`] installs an out-of-context
//! `SetWinEventHook(EVENT_SYSTEM_FOREGROUND)` on its own thread with a message
//! loop, reports the current foreground window immediately, and then every
//! time it changes. Dropping the watcher unhooks and joins the thread.

use serde::{Deserialize, Serialize};

use crate::Result;

/// The window that just came to the foreground.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ForegroundInfo {
    /// Window handle (as used by `helm-win-apps`).
    pub hwnd: u64,
    /// Owning process id. For UWP apps hosted in `ApplicationFrameHost.exe`
    /// this is the hosted app's process when it can be found.
    pub pid: u32,
    /// Full exe path, when this process may query it.
    pub exe: Option<String>,
    /// Exe file name, e.g. `Code.exe` (empty if unknown). Match profiles on
    /// this, case-insensitively.
    pub exe_name: String,
    /// Window title (may be empty right at the switch).
    pub title: String,
}

/// File name part of a Windows path.
pub fn exe_name(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or("").to_string()
}

/// Running foreground watcher; stops when dropped.
pub struct ForegroundWatcher {
    #[cfg(windows)]
    thread_id: u32,
    #[cfg(windows)]
    join: Option<std::thread::JoinHandle<()>>,
}

impl ForegroundWatcher {
    /// Starts watching. `cb` runs on the watcher's thread, first with the
    /// current foreground window, then on every change (consecutive duplicates
    /// are suppressed). Keep it short, e.g. send on a channel.
    pub fn start<F>(cb: F) -> Result<Self>
    where
        F: FnMut(&ForegroundInfo) + Send + 'static,
    {
        os::start(Box::new(cb))
    }
}

/// Reads the current foreground window once.
pub fn current() -> Result<Option<ForegroundInfo>> {
    os::current()
}

#[cfg(windows)]
mod os {
    use std::cell::RefCell;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::mpsc;

    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, EVENT_SYSTEM_FOREGROUND, EnumChildWindows, GetForegroundWindow, GetMessageW,
        GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, MSG, PM_NOREMOVE, PeekMessageW,
        PostThreadMessageW, TranslateMessage, WINEVENT_OUTOFCONTEXT, WM_QUIT,
    };
    use windows::core::BOOL;

    use super::{ForegroundInfo, ForegroundWatcher, exe_name};
    use crate::{Error, Result};

    type Callback = Box<dyn FnMut(&ForegroundInfo) + Send>;

    thread_local! {
        static STATE: RefCell<Option<(Callback, u64)>> = const { RefCell::new(None) };
    }

    fn pid_of(hwnd: HWND) -> u32 {
        let mut pid = 0u32;
        // SAFETY: out-pointer to a local.
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        pid
    }

    fn title_of(hwnd: HWND) -> String {
        // SAFETY: plain Win32 calls with a correctly sized buffer.
        unsafe {
            let n = GetWindowTextLengthW(hwnd);
            if n <= 0 {
                return String::new();
            }
            let mut buf = vec![0u16; n as usize + 1];
            let got = GetWindowTextW(hwnd, &mut buf);
            String::from_utf16_lossy(&buf[..got.max(0) as usize])
        }
    }

    /// For ApplicationFrameHost windows, the hosted UWP app's pid.
    fn hosted_pid(hwnd: HWND, frame_pid: u32) -> Option<u32> {
        struct Search {
            frame_pid: u32,
            found: Option<u32>,
        }
        unsafe extern "system" fn proc_(child: HWND, lp: LPARAM) -> BOOL {
            // SAFETY: lp is the &mut Search passed below, alive for the call.
            let s = unsafe { &mut *(lp.0 as *mut Search) };
            let pid = pid_of(child);
            if pid != 0 && pid != s.frame_pid {
                s.found = Some(pid);
                return BOOL(0);
            }
            BOOL(1)
        }
        let mut s = Search { frame_pid, found: None };
        // SAFETY: callback only touches `s`, which outlives the call.
        unsafe {
            let _ = EnumChildWindows(Some(hwnd), Some(proc_), LPARAM(&mut s as *mut _ as isize));
        }
        s.found
    }

    pub(super) fn info_for(hwnd: HWND) -> Option<ForegroundInfo> {
        if hwnd.0.is_null() {
            return None;
        }
        let mut pid = pid_of(hwnd);
        let mut exe = crate::util::process_exe(pid);
        if exe.as_deref().map(exe_name).is_some_and(|n| n.eq_ignore_ascii_case("ApplicationFrameHost.exe")) {
            if let Some(p) = hosted_pid(hwnd, pid) {
                pid = p;
                exe = crate::util::process_exe(p).or(exe);
            }
        }
        Some(ForegroundInfo {
            hwnd: hwnd.0 as usize as u64,
            pid,
            exe_name: exe.as_deref().map(exe_name).unwrap_or_default(),
            exe,
            title: title_of(hwnd),
        })
    }

    fn deliver(hwnd: HWND) {
        let Some(info) = info_for(hwnd) else { return };
        STATE.with(|s| {
            if let Some((cb, last)) = s.borrow_mut().as_mut() {
                if *last == info.hwnd {
                    return;
                }
                *last = info.hwnd;
                // Never unwind across the FFI boundary.
                let _ = catch_unwind(AssertUnwindSafe(|| cb(&info)));
            }
        });
    }

    unsafe extern "system" fn on_event(
        _hook: HWINEVENTHOOK,
        _event: u32,
        hwnd: HWND,
        id_object: i32,
        id_child: i32,
        _thread: u32,
        _time: u32,
    ) {
        // OBJID_WINDOW / CHILDID_SELF only.
        if id_object == 0 && id_child == 0 {
            deliver(hwnd);
        }
    }

    pub(super) fn start(cb: Callback) -> Result<ForegroundWatcher> {
        let (tx, rx) = mpsc::channel::<Result<u32>>();
        let join = std::thread::Builder::new()
            .name("helm-foreground".into())
            .spawn(move || {
                // SAFETY: standard message-loop thread setup.
                unsafe {
                    let mut msg = MSG::default();
                    // Create the message queue so PostThreadMessageW works.
                    let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
                    let hook = SetWinEventHook(
                        EVENT_SYSTEM_FOREGROUND,
                        EVENT_SYSTEM_FOREGROUND,
                        None,
                        Some(on_event),
                        0,
                        0,
                        WINEVENT_OUTOFCONTEXT,
                    );
                    if hook.is_invalid() {
                        let _ = tx.send(Err(Error::Os { code: -1, message: "SetWinEventHook failed".into() }));
                        return;
                    }
                    STATE.with(|s| *s.borrow_mut() = Some((cb, 0)));
                    let _ = tx.send(Ok(GetCurrentThreadId()));
                    deliver(GetForegroundWindow());
                    while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                    let _ = UnhookWinEvent(hook);
                    STATE.with(|s| *s.borrow_mut() = None);
                }
            })
            .map_err(|e| Error::Internal(format!("spawn: {e}")))?;
        match rx.recv() {
            Ok(Ok(thread_id)) => Ok(ForegroundWatcher { thread_id, join: Some(join) }),
            Ok(Err(e)) => {
                let _ = join.join();
                Err(e)
            }
            Err(_) => Err(Error::Internal("foreground thread exited".into())),
        }
    }

    pub(super) fn current() -> Result<Option<ForegroundInfo>> {
        // SAFETY: plain Win32 call.
        Ok(info_for(unsafe { GetForegroundWindow() }))
    }

    impl Drop for ForegroundWatcher {
        fn drop(&mut self) {
            // SAFETY: posting WM_QUIT to our own thread's queue.
            unsafe {
                let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
            }
            if let Some(j) = self.join.take() {
                let _ = j.join();
            }
        }
    }
}

#[cfg(not(windows))]
mod os {
    use super::{ForegroundInfo, ForegroundWatcher};
    use crate::{Error, Result};

    pub(super) fn start(_cb: Box<dyn FnMut(&ForegroundInfo) + Send>) -> Result<ForegroundWatcher> {
        Err(Error::Unsupported("foreground watcher"))
    }
    pub(super) fn current() -> Result<Option<ForegroundInfo>> {
        Err(Error::Unsupported("foreground window"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exe_name_from_path() {
        assert_eq!(exe_name(r"C:\Program Files\App\Code.exe"), "Code.exe");
        assert_eq!(exe_name("notepad.exe"), "notepad.exe");
        assert_eq!(exe_name(""), "");
    }
}
