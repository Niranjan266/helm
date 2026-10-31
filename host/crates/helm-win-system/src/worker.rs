//! Dedicated COM (MTA) worker threads.
//!
//! COM and WinRT objects in this crate are created and used on a background
//! thread that owns its apartment, so callers (including tokio worker threads)
//! never have to initialise COM. Each subsystem gets its own worker so a slow
//! DDC/CI monitor query can't delay a media command.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc;
use std::thread;

use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};

use crate::{Error, Result};

type Job = Box<dyn FnOnce() + Send + 'static>;

/// A thread that runs closures one at a time inside a COM MTA.
pub(crate) struct Worker {
    tx: mpsc::Sender<Job>,
}

impl Worker {
    pub(crate) fn spawn(name: &str) -> Worker {
        let (tx, rx) = mpsc::channel::<Job>();
        thread::Builder::new()
            .name(name.to_string())
            .spawn(move || {
                let _com = ComApartment::enter();
                for job in rx {
                    job();
                }
            })
            .expect("failed to spawn helm-win-system worker thread");
        Worker { tx }
    }

    /// Run `f` on the worker and block until it returns.
    ///
    /// Must not be called from the worker thread itself (it would deadlock);
    /// internal code never does.
    pub(crate) fn run<R, F>(&self, f: F) -> Result<R>
    where
        R: Send + 'static,
        F: FnOnce() -> R + Send + 'static,
    {
        let (rtx, rrx) = mpsc::sync_channel::<thread::Result<R>>(1);
        self.tx
            .send(Box::new(move || {
                let _ = rtx.send(catch_unwind(AssertUnwindSafe(f)));
            }))
            .map_err(|_| Error::Internal("worker thread stopped".into()))?;
        match rrx.recv() {
            Ok(Ok(r)) => Ok(r),
            Ok(Err(_)) => Err(Error::Internal("worker job panicked".into())),
            Err(_) => Err(Error::Internal("worker thread stopped".into())),
        }
    }
}

/// RAII guard: CoInitializeEx(MTA) on creation, CoUninitialize on drop
/// (only if initialisation succeeded).
pub(crate) struct ComApartment {
    initialised: bool,
}

impl ComApartment {
    pub(crate) fn enter() -> ComApartment {
        // S_OK and S_FALSE (already initialised) both need a matching CoUninitialize.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        ComApartment { initialised: hr.is_ok() }
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.initialised {
            unsafe { CoUninitialize() };
        }
    }
}

/// Lazily started worker stored in a `static`.
macro_rules! static_worker {
    ($fn_name:ident, $thread_name:literal) => {
        fn $fn_name() -> &'static $crate::worker::Worker {
            static W: std::sync::OnceLock<$crate::worker::Worker> = std::sync::OnceLock::new();
            W.get_or_init(|| $crate::worker::Worker::spawn($thread_name))
        }
    };
}
pub(crate) use static_worker;

/// Convert a NUL-terminated UTF-16 buffer to a String.
pub(crate) fn wide_to_string(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// Convert a Rust string to a NUL-terminated UTF-16 buffer.
pub(crate) fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
