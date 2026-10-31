//! DPI awareness.
//!
//! Windows virtualises coordinates and screen pixels for DPI-unaware code: on a 150 % monitor a
//! DPI-unaware process sees a 1280×720 "screen" and gets a stretched screenshot. Everything in
//! this crate that reads coordinates or screen pixels runs inside a [`ThreadDpiScope`] that
//! switches the *calling thread* to per-monitor-v2 awareness for the duration of the call, so
//! the results are always in physical pixels whatever the process awareness is.
//!
//! The agent should still declare per-monitor-v2 awareness for the whole process (best: in the
//! application manifest; otherwise call [`set_process_per_monitor_v2`] first thing in `main`),
//! so that input injection (`SendInput` absolute coordinates) and any windows it shows use the
//! same physical coordinate space as `monitors()`.

use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
    SetThreadDpiAwarenessContext,
};

/// Make the whole process per-monitor-v2 DPI aware.
///
/// Returns `true` if the call succeeded **or** awareness was already set (by a manifest or an
/// earlier call — Windows only lets it be set once; the error is then `E_ACCESSDENIED`).
pub fn set_process_per_monitor_v2() -> bool {
    // SAFETY: plain Win32 call with a predefined pseudo-handle.
    match unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) } {
        Ok(()) => true,
        Err(e) => e.code() == windows::Win32::Foundation::E_ACCESSDENIED,
    }
}

/// RAII guard: per-monitor-v2 awareness for the current thread, restored on drop.
pub(crate) struct ThreadDpiScope {
    previous: DPI_AWARENESS_CONTEXT,
}

impl ThreadDpiScope {
    pub(crate) fn per_monitor_v2() -> Self {
        // SAFETY: returns the previous context (or null on failure), which we restore.
        let previous = unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        Self { previous }
    }
}

impl Drop for ThreadDpiScope {
    fn drop(&mut self) {
        if !self.previous.0.is_null() {
            // SAFETY: restoring the value returned by SetThreadDpiAwarenessContext.
            unsafe { SetThreadDpiAwarenessContext(self.previous) };
        }
    }
}
