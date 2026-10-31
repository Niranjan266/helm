//! Windows implementation. All work runs on one STA worker thread (see crate docs).

mod apps;
mod icon;
mod launch;
mod wnd;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Mutex;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MSG, PM_NOREMOVE, PM_REMOVE, PeekMessageW, TranslateMessage,
};
use windows::core::PWSTR;

use crate::{AppEntry, Error, FocusResult, Result, WindowInfo, util::Target};

pub use wnd::ShowCmd;

/// How long a cached app list is trusted before membership checks re-enumerate.
const APPS_TTL: Duration = Duration::from_secs(120);

/// State owned by the worker thread.
#[derive(Default)]
pub(crate) struct State {
    apps: Option<(Instant, Vec<AppEntry>)>,
}

impl State {
    /// Fresh enumeration, stored in the cache.
    fn refresh_apps(&mut self) -> Result<&[AppEntry]> {
        let list = apps::enumerate()?;
        self.apps = Some((Instant::now(), list));
        Ok(&self.apps.as_ref().map(|(_, l)| l).expect("just set")[..])
    }

    /// Cached list if fresh enough, otherwise re-enumerated.
    fn apps(&mut self) -> Result<&[AppEntry]> {
        let fresh = matches!(&self.apps, Some((t, _)) if t.elapsed() < APPS_TTL);
        if !fresh {
            self.refresh_apps()?;
        }
        Ok(&self.apps.as_ref().map(|(_, l)| l).expect("set above")[..])
    }

    /// Finds a listed app by id (case-insensitive), re-enumerating once on a miss so newly
    /// installed apps are found.
    fn find_app(&mut self, app_id: &str) -> Result<AppEntry> {
        let find = |l: &[AppEntry]| {
            l.iter()
                .find(|a| a.app_id.eq_ignore_ascii_case(app_id))
                .cloned()
        };
        if let Some(a) = find(self.apps()?) {
            return Ok(a);
        }
        find(self.refresh_apps()?)
            .ok_or_else(|| Error::NotFound(format!("no app with id '{app_id}'")))
    }
}

type Job = Box<dyn FnOnce(&mut State) + Send>;

static WORKER: Mutex<Option<Sender<Job>>> = Mutex::new(None);

/// Starts the STA worker thread and returns its job sender.
fn start_worker() -> Result<Sender<Job>> {
    let (tx, rx) = mpsc::channel::<Job>();
    let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<()>>(1);
    std::thread::Builder::new()
        .name("helm-win-apps-sta".into())
        .spawn(move || {
            // SAFETY: plain COM initialisation of this new thread.
            let hr =
                unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
            if let Err(e) = hr.ok() {
                let _ = ready_tx.send(Err(win_err("CoInitializeEx")(e)));
                return;
            }
            // Create this thread's message queue (needed by AttachThreadInput and the STA).
            let mut msg = MSG::default();
            // SAFETY: valid out-pointer.
            unsafe {
                let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
            }
            let _ = ready_tx.send(Ok(()));
            let mut state = State::default();
            loop {
                match rx.recv_timeout(Duration::from_millis(50)) {
                    Ok(job) => job(&mut state),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
                pump_messages();
            }
            drop(state);
            // SAFETY: balances the successful CoInitializeEx above.
            unsafe { CoUninitialize() };
        })
        .map_err(|e| Error::Worker(format!("cannot spawn thread: {e}")))?;
    ready_rx
        .recv()
        .map_err(|_| Error::Worker("worker exited during start-up".into()))??;
    Ok(tx)
}

/// Dispatches any pending window messages for the STA.
fn pump_messages() {
    let mut msg = MSG::default();
    // SAFETY: standard message loop on the current thread with a valid MSG.
    unsafe {
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Runs `f` on the STA worker and waits for its result. Must not be called from the
/// worker itself (it would deadlock); internal code never does.
pub(crate) fn run<T, F>(f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(&mut State) -> Result<T> + Send + 'static,
{
    let (rtx, rrx) = mpsc::sync_channel::<Result<T>>(1);
    let job: Job = Box::new(move |st| {
        let r = catch_unwind(AssertUnwindSafe(|| f(st)))
            .unwrap_or_else(|_| Err(Error::Worker("operation panicked".into())));
        let _ = rtx.send(r);
    });
    {
        let mut guard = WORKER.lock().unwrap_or_else(|p| p.into_inner());
        if guard.is_none() {
            *guard = Some(start_worker()?);
        }
        if let Err(mpsc::SendError(job)) = guard.as_ref().expect("set above").send(job) {
            // Worker died; start a new one and retry once.
            let tx = start_worker()?;
            tx.send(job)
                .map_err(|_| Error::Worker("worker not accepting jobs".into()))?;
            *guard = Some(tx);
        }
    }
    rrx.recv()
        .map_err(|_| Error::Worker("worker dropped the reply".into()))?
}

/// Builds a closure that converts a `windows` error into [`Error::Windows`].
pub(crate) fn win_err(context: &'static str) -> impl Fn(windows::core::Error) -> Error {
    move |e| Error::Windows {
        context,
        code: e.code().0 as u32,
        message: e.message(),
    }
}

/// Copies a CoTaskMem-allocated wide string and frees it.
///
/// # Safety
/// `p` must be null or a NUL-terminated string allocated with `CoTaskMemAlloc`.
pub(crate) unsafe fn take_co_string(p: PWSTR) -> Option<String> {
    if p.is_null() {
        return None;
    }
    // SAFETY: guaranteed by caller.
    unsafe {
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

// ---- public entry points (called from lib.rs) -------------------------------------------

pub fn list_apps() -> Result<Vec<AppEntry>> {
    run(|st| Ok(st.refresh_apps()?.to_vec()))
}

pub fn app_icon_png(app_id: &str, size_px: u32) -> Result<Vec<u8>> {
    let id = app_id.to_string();
    run(move |st| {
        let app = st.find_app(&id)?;
        icon::icon_png(&app.app_id, size_px)
    })
}

pub fn launch_app(app_id: &str) -> Result<()> {
    let id = app_id.to_string();
    run(move |st| {
        let app = st.find_app(&id)?;
        launch::launch(&app)
    })
}

pub fn open_target(target: Target) -> Result<()> {
    run(move |_| launch::open(&target))
}

pub fn list_windows() -> Result<Vec<WindowInfo>> {
    run(|st| {
        // App list is only used to map exe paths to app ids; failure is not fatal.
        let apps = st.apps().map(|a| a.to_vec()).unwrap_or_default();
        wnd::list(&apps)
    })
}

pub fn focus_window(hwnd: u64) -> Result<FocusResult> {
    run(move |_| wnd::focus(hwnd))
}

pub fn show_window(hwnd: u64, cmd: ShowCmd) -> Result<()> {
    run(move |_| wnd::show(hwnd, cmd))
}

pub fn close_window(hwnd: u64) -> Result<()> {
    run(move |_| wnd::close(hwnd))
}
