//! Starting the agent inside a user session from the SYSTEM service.
//!
//! WTSQueryUserToken (SYSTEM only) gives the primary token of the user
//! signed in to a session; CreateEnvironmentBlock builds that user's
//! environment (so %APPDATA% points at their profile); CreateProcessAsUserW
//! starts the agent on the interactive desktop `winsta0\default`.
//!
//! The process is placed in a job object with KILL_ON_JOB_CLOSE, so if the
//! service dies the agent goes with it instead of lingering unsupervised.

use std::ffi::c_void;
use std::path::Path;
use std::time::Instant;

use windows::Win32::Foundation::{CloseHandle, ERROR_NO_TOKEN, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};
use windows::Win32::System::RemoteDesktop::{WTSGetActiveConsoleSessionId, WTSQueryUserToken};
use windows::Win32::System::Threading::{
    CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT,
    CreateProcessAsUserW, GetExitCodeProcess, PROCESS_INFORMATION, ResumeThread, STARTUPINFOW,
    TerminateProcess, WaitForSingleObject,
};
use windows::core::{PCWSTR, PWSTR};

/// `WTSGetActiveConsoleSessionId`, with "no session" mapped to `None`.
pub fn active_console_session() -> Option<u32> {
    // SAFETY: no arguments, no preconditions.
    let id = unsafe { WTSGetActiveConsoleSessionId() };
    (id != u32::MAX).then_some(id)
}

#[derive(Debug)]
pub enum LaunchError {
    /// Nobody is signed in to the session (logon screen).
    NoUser,
    Other(String),
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LaunchError::NoUser => f.write_str("no user signed in to the session"),
            LaunchError::Other(s) => f.write_str(s),
        }
    }
}

/// Owned kernel handle, closed on drop.
struct Owned(HANDLE);

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: we own the handle and close it exactly once.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

/// A running agent process.
pub struct Child {
    process: Owned,
    _job: Owned,
    pub pid: u32,
    pub session: u32,
    pub started: Instant,
}

impl Child {
    /// `Some(exit_code)` once the process has exited.
    pub fn try_wait(&self) -> Option<u32> {
        // SAFETY: valid process handle owned by self.
        unsafe {
            if WaitForSingleObject(self.process.0, 0) != WAIT_OBJECT_0 {
                return None;
            }
            let mut code = 0u32;
            let _ = GetExitCodeProcess(self.process.0, &mut code);
            Some(code)
        }
    }

    /// Hard-kills the agent. The caller then waits for `try_wait`.
    pub fn kill(&self) {
        // SAFETY: valid process handle with PROCESS_TERMINATE (we created it).
        unsafe {
            let _ = TerminateProcess(self.process.0, 1);
        }
    }

    /// Blocks up to `ms` milliseconds for the process to exit.
    pub fn wait_ms(&self, ms: u32) {
        // SAFETY: valid process handle.
        unsafe {
            let _ = WaitForSingleObject(self.process.0, ms);
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Starts `command_line` (from [`crate::cmdline::build`]) in `session`.
pub fn launch_in_session(
    session: u32,
    agent_exe: &Path,
    command_line: &str,
) -> Result<Child, LaunchError> {
    let other = |what: &str, e: windows::core::Error| LaunchError::Other(format!("{what}: {e}"));

    // 1. The signed-in user's primary token.
    let mut token = HANDLE::default();
    // SAFETY: out-pointer to a local; the service runs as SYSTEM.
    if let Err(e) = unsafe { WTSQueryUserToken(session, &mut token) } {
        return Err(if e.code() == ERROR_NO_TOKEN.to_hresult() {
            LaunchError::NoUser
        } else {
            other("WTSQueryUserToken", e)
        });
    }
    let token = Owned(token);

    // 2. The user's environment block.
    let mut env: *mut c_void = std::ptr::null_mut();
    // SAFETY: valid token; env is freed below with DestroyEnvironmentBlock.
    unsafe { CreateEnvironmentBlock(&mut env, Some(token.0), false) }
        .map_err(|e| other("CreateEnvironmentBlock", e))?;

    // 3. A kill-on-close job object.
    // SAFETY: plain object creation; struct is fully initialised.
    let job = unsafe {
        let job =
            CreateJobObjectW(None, PCWSTR::null()).map_err(|e| other("CreateJobObjectW", e))?;
        let job = Owned(job);
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
        .map_err(|e| other("SetInformationJobObject", e))?;
        job
    };

    // 4. The process, created suspended so it joins the job before running.
    let app = wide(&agent_exe.display().to_string());
    let mut cmd = wide(command_line);
    let mut desktop = wide("winsta0\\default");
    let cwd = agent_exe.parent().map(|p| wide(&p.display().to_string()));
    let si = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        lpDesktop: PWSTR(desktop.as_mut_ptr()),
        ..Default::default()
    };
    let mut pi = PROCESS_INFORMATION::default();
    // SAFETY: all buffers outlive the call; cmd is a mutable, NUL-terminated
    // buffer as CreateProcessAsUserW requires.
    let created = unsafe {
        CreateProcessAsUserW(
            Some(token.0),
            PCWSTR(app.as_ptr()),
            Some(PWSTR(cmd.as_mut_ptr())),
            None,
            None,
            false,
            CREATE_UNICODE_ENVIRONMENT
                | CREATE_NO_WINDOW
                | CREATE_SUSPENDED
                | CREATE_NEW_PROCESS_GROUP,
            Some(env as *const c_void),
            cwd.as_ref().map_or(PCWSTR::null(), |c| PCWSTR(c.as_ptr())),
            &si,
            &mut pi,
        )
    };
    // SAFETY: env came from CreateEnvironmentBlock.
    unsafe {
        let _ = DestroyEnvironmentBlock(env);
    }
    created.map_err(|e| other("CreateProcessAsUserW", e))?;

    let process = Owned(pi.hProcess);
    let thread = Owned(pi.hThread);
    // SAFETY: handles from CreateProcessAsUserW.
    unsafe {
        if let Err(e) = AssignProcessToJobObject(job.0, process.0) {
            let _ = TerminateProcess(process.0, 1);
            return Err(other("AssignProcessToJobObject", e));
        }
        ResumeThread(thread.0);
    }

    Ok(Child {
        process,
        _job: job,
        pid: pi.dwProcessId,
        session,
        started: Instant::now(),
    })
}
