//! ConPTY session: `CreatePseudoConsole` + two anonymous pipes + `CreateProcessW` with
//! `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE`, inside a kill-on-close Job object.
//!
//! Threads per session:
//! * `helm-pty-out` reads the output pipe and calls `on_output`. It must never stop reading:
//!   a full output pipe blocks the pseudo console (and with it the shell).
//! * `helm-pty-wait` waits for the shell process, then closes the pseudo console (which breaks
//!   the output pipe so the reader drains the last bytes and ends), joins the reader, records
//!   the exit code and calls `on_exit`.

use crate::error::{Context, Error, Result};
use crate::shell::{Shell, validate_size};
use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle, ThreadId};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows::Win32::System::Console::{COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
    TerminateJobObject,
};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::Threading::{
    CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, INFINITE, InitializeProcThreadAttributeList,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION, ResumeThread,
    STARTF_USESTDHANDLES, STARTUPINFOEXW, TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
};
use windows::core::{PCWSTR, PWSTR};

/// A kernel handle closed on drop. Kernel handles may be used from any thread.
struct OwnedHandle(HANDLE);
unsafe impl Send for OwnedHandle {}
unsafe impl Sync for OwnedHandle {}
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

/// The pseudo console handle; `None` once closed.
struct PseudoConsole(Option<HPCON>);
unsafe impl Send for PseudoConsole {}
impl PseudoConsole {
    fn close(&mut self) {
        if let Some(h) = self.0.take() {
            unsafe { ClosePseudoConsole(h) };
        }
    }
}

struct Shared {
    hpc: Mutex<PseudoConsole>,
    input: Mutex<Option<OwnedHandle>>,
    size: Mutex<(u16, u16)>,
    process: OwnedHandle,
    job: OwnedHandle,
    exit: Mutex<Option<u32>>,
    exited: Condvar,
    killed: AtomicBool,
    reader_thread: Mutex<Option<ThreadId>>,
    waiter_thread_id: Mutex<Option<ThreadId>>,
}

/// A running shell in a Windows pseudo console. `Send + Sync`: share it with an `Arc`.
///
/// Dropping it kills the whole process tree (Job object) and waits for the worker threads,
/// unless the drop happens on one of the session's own callback threads.
pub struct Terminal {
    shared: Arc<Shared>,
    pid: u32,
    waiter: Mutex<Option<JoinHandle<()>>>,
}

impl Terminal {
    /// Starts `shell` in a `cols`×`rows` pseudo console. `cwd` defaults to the agent's working
    /// directory. `on_output` receives raw VT/UTF-8 bytes (split anywhere, even inside a UTF-8
    /// character or escape sequence) on the session's reader thread; keep it fast and
    /// non-blocking, e.g. push into a channel.
    pub fn spawn<F>(shell: Shell, cols: u16, rows: u16, cwd: Option<&Path>, on_output: F) -> Result<Terminal>
    where
        F: FnMut(&[u8]) + Send + 'static,
    {
        Self::spawn_with_exit(shell, cols, rows, cwd, on_output, |_| {})
    }

    /// Like [`Terminal::spawn`], plus `on_exit(code)`, called once after the shell ended and
    /// every output byte has been delivered to `on_output`.
    pub fn spawn_with_exit<F, E>(
        shell: Shell,
        cols: u16,
        rows: u16,
        cwd: Option<&Path>,
        on_output: F,
        on_exit: E,
    ) -> Result<Terminal>
    where
        F: FnMut(&[u8]) + Send + 'static,
        E: FnOnce(u32) + Send + 'static,
    {
        validate_size(cols, rows)?;
        let command_line = shell.command_line()?;
        let cwd_wide = match cwd {
            Some(dir) => {
                if !dir.is_dir() {
                    return Err(Error::InvalidConfig(format!("working directory {} does not exist", dir.display())));
                }
                Some(wide(dir.as_os_str()))
            }
            None => None,
        };

        // Pipes: we write `in_write`, the pseudo console reads `in_read`; it writes
        // `out_write`, we read `out_read`.
        let (in_read, in_write) = pipe()?;
        let (out_read, out_write) = pipe()?;

        let size = COORD { X: cols as i16, Y: rows as i16 };
        let hpc = unsafe { CreatePseudoConsole(size, in_read.0, out_write.0, 0) }.ctx("CreatePseudoConsole")?;
        let mut pc = PseudoConsole(Some(hpc));

        let spawned = (|| -> Result<(OwnedHandle, OwnedHandle, u32)> {
            let mut attrs = AttributeList::new(1)?;
            unsafe {
                UpdateProcThreadAttribute(
                    attrs.ptr(),
                    0,
                    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                    Some(hpc.0 as *const c_void),
                    std::mem::size_of::<HPCON>(),
                    None,
                    None,
                )
            }
            .ctx("UpdateProcThreadAttribute")?;

            let mut si = STARTUPINFOEXW::default();
            si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
            // Without this, a console child of a parent whose own stdio is redirected (CI, a
            // service, `cargo test`) can pick up those handles instead of the pseudo console.
            // Null handles + STARTF_USESTDHANDLES make it use the pseudo console's.
            si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
            si.lpAttributeList = attrs.ptr();

            let mut cmd: Vec<u16> = command_line.encode_utf16().chain(Some(0)).collect();
            let mut pi = PROCESS_INFORMATION::default();
            unsafe {
                CreateProcessW(
                    PCWSTR::null(),
                    Some(PWSTR(cmd.as_mut_ptr())),
                    None,
                    None,
                    false,
                    EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | CREATE_SUSPENDED,
                    None,
                    cwd_wide.as_ref().map_or(PCWSTR::null(), |w| PCWSTR(w.as_ptr())),
                    &si.StartupInfo,
                    &mut pi,
                )
            }
            .ctx("CreateProcessW")?;
            let process = OwnedHandle(pi.hProcess);
            let thread = OwnedHandle(pi.hThread);

            // Put the (still suspended) shell into a kill-on-close job before it can start
            // children, so kill/drop reach the whole tree.
            let job = match create_kill_on_close_job()
                .and_then(|job| unsafe { AssignProcessToJobObject(job.0, process.0) }.ctx("AssignProcessToJobObject").map(|_| job))
            {
                Ok(job) => job,
                Err(e) => {
                    unsafe {
                        let _ = TerminateProcess(process.0, crate::KILLED_EXIT_CODE);
                    }
                    return Err(e);
                }
            };
            unsafe { ResumeThread(thread.0) };
            Ok((process, job, pi.dwProcessId))
        })();

        // The pseudo console duplicated its pipe ends; ours are no longer needed.
        drop(in_read);
        drop(out_write);

        let (process, job, pid) = match spawned {
            Ok(v) => v,
            Err(e) => {
                pc.close();
                return Err(e);
            }
        };

        let shared = Arc::new(Shared {
            hpc: Mutex::new(pc),
            input: Mutex::new(Some(in_write)),
            size: Mutex::new((cols, rows)),
            process,
            job,
            exit: Mutex::new(None),
            exited: Condvar::new(),
            killed: AtomicBool::new(false),
            reader_thread: Mutex::new(None),
            waiter_thread_id: Mutex::new(None),
        });

        let reader = {
            let shared = shared.clone();
            thread::Builder::new()
                .name("helm-pty-out".into())
                .spawn(move || {
                    *shared.reader_thread.lock().unwrap() = Some(thread::current().id());
                    read_loop(out_read, on_output);
                })
                .map_err(|e| Error::InvalidConfig(format!("cannot start reader thread: {e}")))?
        };

        let waiter = {
            let shared = shared.clone();
            thread::Builder::new()
                .name("helm-pty-wait".into())
                .spawn(move || {
                    *shared.waiter_thread_id.lock().unwrap() = Some(thread::current().id());
                    unsafe { WaitForSingleObject(shared.process.0, INFINITE) };
                    let mut code = 0u32;
                    if unsafe { GetExitCodeProcess(shared.process.0, &mut code) }.is_err() {
                        code = crate::KILLED_EXIT_CODE;
                    }
                    // Closing the pseudo console flushes and breaks the output pipe: the reader
                    // gets the final bytes, then EOF. On older Windows builds ClosePseudoConsole
                    // waits for that drain, which is fine because the reader is independent.
                    shared.hpc.lock().unwrap().close();
                    let _ = reader.join();
                    shared.input.lock().unwrap().take();
                    *shared.exit.lock().unwrap() = Some(code);
                    shared.exited.notify_all();
                    on_exit(code);
                })
        };
        let waiter = match waiter {
            Ok(w) => w,
            Err(e) => {
                unsafe {
                    let _ = TerminateJobObject(shared.job.0, crate::KILLED_EXIT_CODE);
                }
                shared.hpc.lock().unwrap().close();
                return Err(Error::InvalidConfig(format!("cannot start wait thread: {e}")));
            }
        };

        Ok(Terminal { shared, pid, waiter: Mutex::new(Some(waiter)) })
    }

    /// Process id of the shell.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Sends input: text as UTF-8, keys as VT sequences (`"\r"` = Enter, `"\x1b[A"` = Up,
    /// `"\x03"` = Ctrl+C). Blocks only while the pseudo console's input pipe is full.
    pub fn write(&self, bytes: &[u8]) -> Result<()> {
        let guard = self.shared.input.lock().unwrap();
        let Some(h) = guard.as_ref() else { return Err(Error::Closed) };
        let mut rest = bytes;
        while !rest.is_empty() {
            let chunk = &rest[..rest.len().min(64 * 1024)];
            let mut written = 0u32;
            unsafe { WriteFile(h.0, Some(chunk), Some(&mut written), None) }.map_err(|e| {
                if self.try_wait().is_some() { Error::Closed } else { Error::Windows { context: "WriteFile", source: e } }
            })?;
            rest = &rest[written as usize..];
        }
        Ok(())
    }

    /// Resizes the pseudo console (the shell sees a window-size change; output is re-rendered).
    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        validate_size(cols, rows)?;
        let guard = self.shared.hpc.lock().unwrap();
        let Some(h) = guard.0 else { return Err(Error::Closed) };
        unsafe { ResizePseudoConsole(h, COORD { X: cols as i16, Y: rows as i16 }) }.ctx("ResizePseudoConsole")?;
        *self.shared.size.lock().unwrap() = (cols, rows);
        Ok(())
    }

    /// Current size `(cols, rows)`.
    pub fn size(&self) -> (u16, u16) {
        *self.shared.size.lock().unwrap()
    }

    /// Waits until the shell has exited **and** all its output was delivered. `None` timeout
    /// waits forever. Returns the exit code, or `None` on timeout.
    pub fn wait(&self, timeout: Option<Duration>) -> Option<u32> {
        let deadline = timeout.map(|t| Instant::now() + t);
        let mut exit = self.shared.exit.lock().unwrap();
        loop {
            if let Some(code) = *exit {
                return Some(code);
            }
            match deadline {
                None => exit = self.shared.exited.wait(exit).unwrap(),
                Some(d) => {
                    let now = Instant::now();
                    if now >= d {
                        return None;
                    }
                    exit = self.shared.exited.wait_timeout(exit, d - now).unwrap().0;
                }
            }
        }
    }

    /// Exit code if the session has fully ended, without blocking.
    pub fn try_wait(&self) -> Option<u32> {
        *self.shared.exit.lock().unwrap()
    }

    /// Same as [`Terminal::try_wait`].
    pub fn exit_code(&self) -> Option<u32> {
        self.try_wait()
    }

    /// True until the shell has exited and its output was drained.
    pub fn is_running(&self) -> bool {
        self.try_wait().is_none()
    }

    /// True if the session ended through [`Terminal::kill`] or drop.
    pub fn was_killed(&self) -> bool {
        self.shared.killed.load(Ordering::SeqCst)
    }

    /// Terminates the shell and every process it started (`TerminateJobObject`). Returns
    /// immediately; use [`Terminal::wait`] to wait for the final output and exit code.
    pub fn kill(&self) -> Result<()> {
        if self.try_wait().is_some() {
            return Ok(());
        }
        self.shared.killed.store(true, Ordering::SeqCst);
        unsafe { TerminateJobObject(self.shared.job.0, crate::KILLED_EXIT_CODE) }.ctx("TerminateJobObject")
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.kill();
        let me = thread::current().id();
        let on_own_thread = *self.shared.reader_thread.lock().unwrap() == Some(me)
            || *self.shared.waiter_thread_id.lock().unwrap() == Some(me);
        if let Some(w) = self.waiter.lock().unwrap().take()
            && !on_own_thread
        {
            let _ = w.join();
        }
    }
}

fn read_loop<F: FnMut(&[u8])>(out_read: OwnedHandle, mut on_output: F) {
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let mut read = 0u32;
        let ok = unsafe { ReadFile(out_read.0, Some(&mut buf), Some(&mut read), None) };
        if ok.is_err() || read == 0 {
            // ERROR_BROKEN_PIPE after ClosePseudoConsole: the session is over.
            break;
        }
        on_output(&buf[..read as usize]);
    }
}

fn pipe() -> Result<(OwnedHandle, OwnedHandle)> {
    let mut r = HANDLE::default();
    let mut w = HANDLE::default();
    unsafe { CreatePipe(&mut r, &mut w, None, 0) }.ctx("CreatePipe")?;
    Ok((OwnedHandle(r), OwnedHandle(w)))
}

fn create_kill_on_close_job() -> Result<OwnedHandle> {
    let job = OwnedHandle(unsafe { CreateJobObjectW(None, PCWSTR::null()) }.ctx("CreateJobObjectW")?);
    let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    unsafe {
        SetInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    }
    .ctx("SetInformationJobObject")?;
    Ok(job)
}

fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    s.encode_wide().chain(Some(0)).collect()
}

/// `PROC_THREAD_ATTRIBUTE_LIST` storage (pointer-aligned), deleted on drop.
struct AttributeList {
    buf: Vec<usize>,
}

impl AttributeList {
    fn new(count: u32) -> Result<Self> {
        let mut bytes = 0usize;
        // The size query "fails" with ERROR_INSUFFICIENT_BUFFER by design.
        let _ = unsafe { InitializeProcThreadAttributeList(None, count, None, &mut bytes) };
        if bytes == 0 {
            return Err(Error::InvalidConfig("InitializeProcThreadAttributeList returned size 0".into()));
        }
        let mut buf = vec![0usize; bytes.div_ceil(std::mem::size_of::<usize>())];
        let list = LPPROC_THREAD_ATTRIBUTE_LIST(buf.as_mut_ptr() as *mut c_void);
        unsafe { InitializeProcThreadAttributeList(Some(list), count, None, &mut bytes) }
            .ctx("InitializeProcThreadAttributeList")?;
        Ok(AttributeList { buf })
    }

    fn ptr(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        LPPROC_THREAD_ATTRIBUTE_LIST(self.buf.as_mut_ptr() as *mut c_void)
    }
}

impl Drop for AttributeList {
    fn drop(&mut self) {
        unsafe { DeleteProcThreadAttributeList(self.ptr()) };
    }
}

#[allow(dead_code)]
fn _assert_send_sync() {
    fn f<T: Send + Sync>() {}
    f::<Terminal>();
}
