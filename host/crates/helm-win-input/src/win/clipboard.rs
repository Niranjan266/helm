//! Win32 clipboard (Microsoft Learn: "Using the Clipboard", OpenClipboard, SetClipboardData,
//! AddClipboardFormatListener, GetClipboardSequenceNumber, message-only windows).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Duration;

use windows::Win32::Foundation::{
    ERROR_CLASS_ALREADY_EXISTS, GetLastError, GlobalFree, HANDLE, HGLOBAL, HWND, LPARAM, LRESULT,
    WPARAM,
};
use windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardData,
    GetClipboardSequenceNumber, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, RemoveClipboardFormatListener, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows::Win32::System::Ole::{CF_DIB, CF_DIBV5, CF_UNICODETEXT};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, HWND_MESSAGE,
    MSG, PostMessageW, PostQuitMessage, RegisterClassW, TranslateMessage, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_APP, WM_CLIPBOARDUPDATE, WM_DESTROY, WNDCLASSW,
};
use windows_core::{PCWSTR, w};

use crate::clipboard::{ClipboardChange, to_crlf};
use crate::dib::{PNG_SIGNATURE, dib_to_png, png_to_dib};
use crate::error::{Error, Result};

const OPEN_ATTEMPTS: u32 = 12;
/// Sequence numbers produced by our own writes (newest last).
static OWN_SEQUENCES: Mutex<VecDeque<u32>> = Mutex::new(VecDeque::new());
const OWN_SEQUENCES_KEPT: usize = 16;
/// Number of writes in progress in this process (see `wait_for_own_writes`).
static WRITES_IN_PROGRESS: AtomicUsize = AtomicUsize::new(0);

fn record_own(seq: u32) {
    let mut q = OWN_SEQUENCES.lock().unwrap_or_else(|p| p.into_inner());
    if q.len() == OWN_SEQUENCES_KEPT {
        q.pop_front();
    }
    q.push_back(seq);
}

fn is_own(seq: u32) -> bool {
    OWN_SEQUENCES
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .contains(&seq)
}

fn png_format() -> u32 {
    static FORMAT: OnceLock<u32> = OnceLock::new();
    *FORMAT.get_or_init(|| unsafe { RegisterClipboardFormatW(w!("PNG")) })
}

fn available(format: u32) -> bool {
    format != 0 && unsafe { IsClipboardFormatAvailable(format) }.is_ok()
}

/// The current clipboard sequence number.
pub fn sequence_number() -> Result<u32> {
    Ok(unsafe { GetClipboardSequenceNumber() })
}

/// Clipboard opened by this thread; closed on drop.
struct Opened;

impl Opened {
    fn new(owner: Option<HWND>) -> Result<Self> {
        for attempt in 0..OPEN_ATTEMPTS {
            if unsafe { OpenClipboard(owner) }.is_ok() {
                return Ok(Opened);
            }
            std::thread::sleep(Duration::from_millis(5 + 5 * attempt as u64));
        }
        Err(Error::ClipboardBusy)
    }
}

impl Drop for Opened {
    fn drop(&mut self) {
        let _ = unsafe { CloseClipboard() };
    }
}

/// A short-lived message-only window used as the clipboard owner while writing (the docs
/// say SetClipboardData fails after EmptyClipboard if the clipboard was opened without one).
struct OwnerWindow(HWND);

impl OwnerWindow {
    fn new() -> Result<Self> {
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                PCWSTR::null(),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )
        }
        .map_err(|e| Error::os(e, "CreateWindowExW"))?;
        Ok(Self(hwnd))
    }
}

impl Drop for OwnerWindow {
    fn drop(&mut self) {
        let _ = unsafe { DestroyWindow(self.0) };
    }
}

/// Copies the bytes of a clipboard item (clipboard must be open).
fn read_item(format: u32) -> Result<Option<Vec<u8>>> {
    if !available(format) {
        return Ok(None);
    }
    let handle = match unsafe { GetClipboardData(format) } {
        Ok(h) => h,
        Err(_) => return Ok(None),
    };
    let hglobal = HGLOBAL(handle.0);
    unsafe {
        let size = GlobalSize(hglobal);
        let ptr = GlobalLock(hglobal) as *const u8;
        if ptr.is_null() {
            return Err(Error::os(windows_core::Error::from_thread(), "GlobalLock"));
        }
        let bytes = std::slice::from_raw_parts(ptr, size).to_vec();
        let _ = GlobalUnlock(hglobal);
        Ok(Some(bytes))
    }
}

/// Global memory that is freed unless handed to the clipboard.
struct Global(Option<HGLOBAL>);

impl Global {
    fn with_bytes(bytes: &[u8]) -> Result<Self> {
        unsafe {
            let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1))
                .map_err(|e| Error::os(e, "GlobalAlloc"))?;
            let g = Global(Some(h));
            let ptr = GlobalLock(h) as *mut u8;
            if ptr.is_null() {
                return Err(Error::os(windows_core::Error::from_thread(), "GlobalLock"));
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
            let _ = GlobalUnlock(h);
            Ok(g)
        }
    }

    /// Gives the memory to the clipboard (which then owns it).
    fn put(mut self, format: u32) -> Result<()> {
        let h = self.0.expect("global memory present");
        unsafe { SetClipboardData(format, Some(HANDLE(h.0))) }
            .map_err(|e| Error::os(e, "SetClipboardData"))?;
        self.0 = None;
        Ok(())
    }
}

impl Drop for Global {
    fn drop(&mut self) {
        if let Some(h) = self.0.take() {
            let _ = unsafe { GlobalFree(Some(h)) };
        }
    }
}

struct WriteInProgress;

impl WriteInProgress {
    fn begin() -> Self {
        WRITES_IN_PROGRESS.fetch_add(1, Ordering::SeqCst);
        WriteInProgress
    }
}

impl Drop for WriteInProgress {
    fn drop(&mut self) {
        WRITES_IN_PROGRESS.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Lets a write from another thread of this process finish recording its sequence number
/// before the watcher decides whether a change was ours. Bounded, so it can never hang.
fn wait_for_own_writes() {
    for _ in 0..50 {
        if WRITES_IN_PROGRESS.load(Ordering::SeqCst) == 0 {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Empties the clipboard and puts the given items on it, remembering the sequence numbers
/// so our own watcher ignores the change.
fn write_items(items: Vec<(u32, Global)>) -> Result<()> {
    let _writing = WriteInProgress::begin();
    let owner = OwnerWindow::new()?;
    let result = {
        let _open = Opened::new(Some(owner.0))?;
        unsafe { EmptyClipboard() }.map_err(|e| Error::os(e, "EmptyClipboard"))?;
        let mut result = Ok(());
        for (format, mem) in items {
            if let Err(e) = mem.put(format) {
                result = Err(e);
                break;
            }
        }
        record_own(unsafe { GetClipboardSequenceNumber() });
        result
    };
    // Closing the clipboard makes Windows add synthesized formats (CF_TEXT, CF_LOCALE, …),
    // which bumps the sequence number again, so record the value after closing as well.
    record_own(unsafe { GetClipboardSequenceNumber() });
    drop(owner);
    result
}

/// Returns the clipboard text, or `None` if there is none.
pub fn get_text() -> Result<Option<String>> {
    let bytes = {
        let _open = Opened::new(None)?;
        read_item(CF_UNICODETEXT.0 as u32)?
    };
    Ok(bytes.map(|b| {
        let units: Vec<u16> = b
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
        String::from_utf16_lossy(&units[..end])
    }))
}

/// Replaces the clipboard with `text`. Lone `\n` / `\r` become `\r\n`.
pub fn set_text(text: &str) -> Result<()> {
    let mut bytes: Vec<u8> = to_crlf(text)
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    bytes.extend_from_slice(&[0, 0]);
    write_items(vec![(CF_UNICODETEXT.0 as u32, Global::with_bytes(&bytes)?)])
}

/// Returns the clipboard image as PNG, or `None` if there is no image.
///
/// Prefers the registered "PNG" format (keeps alpha exactly), then CF_DIBV5, then CF_DIB.
pub fn get_image_png() -> Result<Option<Vec<u8>>> {
    enum Raw {
        Png(Vec<u8>),
        Dib(Vec<u8>),
    }
    let raw = {
        let _open = Opened::new(None)?;
        match read_item(png_format())? {
            Some(p) if p.starts_with(PNG_SIGNATURE) => Some(Raw::Png(p)),
            _ => match read_item(CF_DIBV5.0 as u32)? {
                Some(d) => Some(Raw::Dib(d)),
                None => read_item(CF_DIB.0 as u32)?.map(Raw::Dib),
            },
        }
    };
    match raw {
        Some(Raw::Png(p)) => Ok(Some(p)),
        Some(Raw::Dib(d)) => dib_to_png(&d).map(Some),
        None => Ok(None),
    }
}

/// Replaces the clipboard with a PNG image: CF_DIB (32-bit) for every program plus the
/// registered "PNG" format for programs that understand it. Windows synthesises CF_DIBV5
/// and CF_BITMAP from the DIB.
pub fn set_image_png(png: &[u8]) -> Result<()> {
    let dib = png_to_dib(png)?; // also validates the PNG
    let mut items = vec![(CF_DIB.0 as u32, Global::with_bytes(&dib)?)];
    let fmt = png_format();
    if fmt != 0 {
        items.push((fmt, Global::with_bytes(png)?));
    }
    write_items(items)
}

// ---------------------------------------------------------------------------------------
// Watcher

const WM_STOP: u32 = WM_APP + 1;
const CLASS_NAME: PCWSTR = w!("HelmClipboardWatcher");

type Callback = Box<dyn Fn(ClipboardChange) + Send + 'static>;

struct WatchState {
    callback: Rc<dyn Fn(ClipboardChange)>,
    last_seq: u32,
}

thread_local! {
    static STATE: RefCell<Option<WatchState>> = const { RefCell::new(None) };
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CLIPBOARDUPDATE => {
            wait_for_own_writes();
            let seq = unsafe { GetClipboardSequenceNumber() };
            let callback = STATE.with(|s| {
                let mut s = s.borrow_mut();
                let state = s.as_mut()?;
                if seq == state.last_seq {
                    return None;
                }
                state.last_seq = seq;
                (!is_own(seq)).then(|| state.callback.clone())
            });
            if let Some(cb) = callback {
                let change = ClipboardChange {
                    sequence: seq,
                    has_text: available(CF_UNICODETEXT.0 as u32),
                    has_image: available(CF_DIB.0 as u32)
                        || available(CF_DIBV5.0 as u32)
                        || available(png_format()),
                };
                let _ = catch_unwind(AssertUnwindSafe(|| cb(change)));
            }
            LRESULT(0)
        }
        WM_STOP => {
            unsafe {
                let _ = RemoveClipboardFormatListener(hwnd);
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn create_listener_window() -> Result<HWND> {
    unsafe {
        let instance = GetModuleHandleW(None).map_err(|e| Error::os(e, "GetModuleHandleW"))?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
            return Err(Error::os(
                windows_core::Error::from_thread(),
                "RegisterClassW",
            ));
        }
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS_NAME,
            PCWSTR::null(),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance.into()),
            None,
        )
        .map_err(|e| Error::os(e, "CreateWindowExW"))?;
        if let Err(e) = AddClipboardFormatListener(hwnd) {
            let _ = DestroyWindow(hwnd);
            return Err(Error::os(e, "AddClipboardFormatListener"));
        }
        Ok(hwnd)
    }
}

pub(crate) struct Watcher {
    hwnd: isize,
    thread: Option<JoinHandle<()>>,
}

impl Watcher {
    pub(crate) fn start(callback: Callback) -> Result<Self> {
        let (ready_tx, ready_rx) = mpsc::channel::<Result<isize>>();
        let thread = std::thread::Builder::new()
            .name("helm-clipboard-watch".into())
            .spawn(move || {
                STATE.with(|s| {
                    *s.borrow_mut() = Some(WatchState {
                        callback: Rc::from(callback as Box<dyn Fn(ClipboardChange)>),
                        last_seq: unsafe { GetClipboardSequenceNumber() },
                    })
                });
                let hwnd = match create_listener_window() {
                    Ok(h) => h,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(hwnd.0 as isize));
                let mut msg = MSG::default();
                unsafe {
                    while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
                STATE.with(|s| s.borrow_mut().take());
            })
            .map_err(|e| Error::Thread(e.to_string()))?;
        match ready_rx.recv() {
            Ok(Ok(hwnd)) => Ok(Self {
                hwnd,
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                let _ = thread.join();
                Err(Error::Thread(
                    "clipboard watcher thread exited during start-up".into(),
                ))
            }
        }
    }

    pub(crate) fn stop(mut self) {
        let hwnd = HWND(self.hwnd as *mut core::ffi::c_void);
        let _ = unsafe { PostMessageW(Some(hwnd), WM_STOP, WPARAM(0), LPARAM(0)) };
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
