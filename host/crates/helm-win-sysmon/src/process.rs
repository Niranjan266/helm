//! Task manager: list processes, kill, change priority.
//!
//! [`kill`] and [`set_priority`] refuse pid 0 (System Idle Process), pid 4
//! (System) and the current process; see [`check_target`]. Other protected
//! processes are refused by Windows itself ([`Error::Refused`]).

use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use crate::{Error, Result};

/// Windows process priority class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    /// `IDLE_PRIORITY_CLASS` ("Low" in Task Manager).
    Idle,
    /// `BELOW_NORMAL_PRIORITY_CLASS`.
    BelowNormal,
    /// `NORMAL_PRIORITY_CLASS`.
    Normal,
    /// `ABOVE_NORMAL_PRIORITY_CLASS`.
    AboveNormal,
    /// `HIGH_PRIORITY_CLASS`.
    High,
    /// `REALTIME_PRIORITY_CLASS`. Reported only; [`set_priority`] refuses it
    /// because it can starve the input and audio threads of the whole PC.
    Realtime,
}

/// One process.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ProcessInfo {
    /// Process id.
    pub pid: u32,
    /// Parent process id, if known.
    pub parent_pid: Option<u32>,
    /// Image name, e.g. `chrome.exe`.
    pub name: String,
    /// Full image path, when this process may query it.
    pub exe: Option<String>,
    /// CPU usage since the previous list, 0–100 of the whole machine (like
    /// Task Manager). 0 on the very first call.
    pub cpu_pct: f32,
    /// Working set, bytes.
    pub ram_bytes: u64,
    /// Whether the process token is elevated; `None` when access is denied.
    pub is_elevated: Option<bool>,
    /// Priority class; `None` when access is denied.
    pub priority: Option<Priority>,
}

/// Why [`kill`] / [`set_priority`] refuse a pid, or `Ok(())` if allowed.
///
/// Pure rule: pid 0 and 4 are kernel pseudo-processes and `self_pid` is the
/// agent itself (killing it would cut the phone off).
pub fn check_target(pid: u32, self_pid: u32) -> Result<()> {
    match pid {
        0 => Err(Error::Refused("pid 0 is the System Idle Process".into())),
        4 => Err(Error::Refused("pid 4 is the System process".into())),
        p if p == self_pid => Err(Error::Refused("refusing to target the Helm agent itself".into())),
        _ => Ok(()),
    }
}

struct ProcState {
    sys: System,
    last: Option<Instant>,
}

fn state() -> &'static Mutex<ProcState> {
    static STATE: OnceLock<Mutex<ProcState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(ProcState { sys: System::new(), last: None }))
}

fn refresh_kind() -> ProcessRefreshKind {
    ProcessRefreshKind::nothing().with_cpu().with_memory().with_exe(UpdateKind::OnlyIfNotSet)
}

/// Lists running processes, sorted by pid.
///
/// CPU % is measured since the previous call (process-wide shared state). If
/// the previous call was less than ~200 ms ago, or this is the first call,
/// this blocks for up to ~200 ms to take a second reading.
pub fn processes() -> Result<Vec<ProcessInfo>> {
    let mut g = state().lock().map_err(|_| Error::Internal("process lock poisoned".into()))?;
    let st = &mut *g;
    let need_two = st.last.is_none_or(|t| t.elapsed() < sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
    if need_two {
        st.sys.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh_kind());
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
    }
    st.sys.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh_kind());
    st.last = Some(Instant::now());

    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f32;
    let mut out: Vec<ProcessInfo> = st
        .sys
        .processes()
        .values()
        .map(|p| {
            let pid = p.pid().as_u32();
            let (is_elevated, priority) = os::query(pid);
            ProcessInfo {
                pid,
                parent_pid: p.parent().map(|pp| pp.as_u32()),
                name: p.name().to_string_lossy().into_owned(),
                exe: p.exe().map(|e| e.to_string_lossy().into_owned()),
                cpu_pct: (p.cpu_usage() / cores).clamp(0.0, 100.0),
                ram_bytes: p.memory(),
                is_elevated,
                priority,
            }
        })
        .collect();
    out.sort_by_key(|p| p.pid);
    Ok(out)
}

/// Terminates a process (exit code 1). Refuses pid 0, 4 and this process.
/// Killing processes of other users or elevated processes needs the agent to
/// run elevated; otherwise returns [`Error::Refused`].
pub fn kill(pid: u32) -> Result<()> {
    check_target(pid, std::process::id())?;
    os::kill(pid)
}

/// Sets the priority class. Refuses pid 0, 4, this process and
/// [`Priority::Realtime`].
pub fn set_priority(pid: u32, level: Priority) -> Result<()> {
    check_target(pid, std::process::id())?;
    if level == Priority::Realtime {
        return Err(Error::Refused("realtime priority is not allowed".into()));
    }
    os::set_priority(pid, level)
}

/// Reads one process's priority class.
pub fn priority(pid: u32) -> Result<Priority> {
    os::priority(pid)
}

#[cfg(windows)]
mod os {
    use super::Priority;
    use crate::util::open_process;
    use crate::{Error, Result};
    use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
    use windows::Win32::System::Threading::{
        ABOVE_NORMAL_PRIORITY_CLASS, BELOW_NORMAL_PRIORITY_CLASS, GetPriorityClass, HIGH_PRIORITY_CLASS,
        IDLE_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS, OpenProcessToken, PROCESS_CREATION_FLAGS,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION, PROCESS_TERMINATE,
        REALTIME_PRIORITY_CLASS, SetPriorityClass, TerminateProcess,
    };

    const E_INVALIDARG_WIN32: i32 = 0x8007_0057_u32 as i32;

    fn open(pid: u32, access: windows::Win32::System::Threading::PROCESS_ACCESS_RIGHTS) -> Result<crate::util::OwnedHandle> {
        open_process(pid, access).map_err(|e| {
            if e.code().0 == E_INVALIDARG_WIN32 {
                Error::NotFound(format!("process {pid}"))
            } else {
                Error::from(e)
            }
        })
    }

    fn to_flags(p: Priority) -> PROCESS_CREATION_FLAGS {
        match p {
            Priority::Idle => IDLE_PRIORITY_CLASS,
            Priority::BelowNormal => BELOW_NORMAL_PRIORITY_CLASS,
            Priority::Normal => NORMAL_PRIORITY_CLASS,
            Priority::AboveNormal => ABOVE_NORMAL_PRIORITY_CLASS,
            Priority::High => HIGH_PRIORITY_CLASS,
            Priority::Realtime => REALTIME_PRIORITY_CLASS,
        }
    }

    fn from_class(c: u32) -> Option<Priority> {
        [
            Priority::Idle,
            Priority::BelowNormal,
            Priority::Normal,
            Priority::AboveNormal,
            Priority::High,
            Priority::Realtime,
        ]
        .into_iter()
        .find(|p| to_flags(*p).0 == c)
    }

    pub(super) fn query(pid: u32) -> (Option<bool>, Option<Priority>) {
        let Ok(h) = open_process(pid, PROCESS_QUERY_LIMITED_INFORMATION) else { return (None, None) };
        // SAFETY: valid process handle.
        let prio = from_class(unsafe { GetPriorityClass(h.0) });
        let mut token = windows::Win32::Foundation::HANDLE::default();
        // SAFETY: out-pointer to a local; token closed by the guard below.
        if unsafe { OpenProcessToken(h.0, TOKEN_QUERY, &mut token) }.is_err() {
            return (None, prio);
        }
        let token = crate::util::OwnedHandle(token);
        let mut elev = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        // SAFETY: buffer is a TOKEN_ELEVATION of the stated size.
        let ok = unsafe {
            GetTokenInformation(
                token.0,
                TokenElevation,
                Some(&mut elev as *mut _ as *mut core::ffi::c_void),
                size_of::<TOKEN_ELEVATION>() as u32,
                &mut len,
            )
        }
        .is_ok();
        (ok.then_some(elev.TokenIsElevated != 0), prio)
    }

    pub(super) fn kill(pid: u32) -> Result<()> {
        let h = open(pid, PROCESS_TERMINATE)?;
        // SAFETY: handle opened with PROCESS_TERMINATE.
        unsafe { TerminateProcess(h.0, 1) }?;
        Ok(())
    }

    pub(super) fn set_priority(pid: u32, level: Priority) -> Result<()> {
        let h = open(pid, PROCESS_SET_INFORMATION)?;
        // SAFETY: handle opened with PROCESS_SET_INFORMATION.
        unsafe { SetPriorityClass(h.0, to_flags(level)) }?;
        Ok(())
    }

    pub(super) fn priority(pid: u32) -> Result<Priority> {
        let h = open(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
        // SAFETY: valid process handle.
        let c = unsafe { GetPriorityClass(h.0) };
        if c == 0 {
            return Err(windows::core::Error::from_thread().into());
        }
        from_class(c).ok_or_else(|| Error::Internal(format!("unknown priority class 0x{c:X}")))
    }
}

#[cfg(not(windows))]
mod os {
    use super::Priority;
    use crate::{Error, Result};

    pub(super) fn query(_pid: u32) -> (Option<bool>, Option<Priority>) {
        (None, None)
    }
    pub(super) fn kill(_pid: u32) -> Result<()> {
        Err(Error::Unsupported("process kill"))
    }
    pub(super) fn set_priority(_pid: u32, _level: Priority) -> Result<()> {
        Err(Error::Unsupported("process priority"))
    }
    pub(super) fn priority(_pid: u32) -> Result<Priority> {
        Err(Error::Unsupported("process priority"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusal_rules() {
        let me = 1234;
        assert!(matches!(check_target(0, me), Err(Error::Refused(_))));
        assert!(matches!(check_target(4, me), Err(Error::Refused(_))));
        assert!(matches!(check_target(me, me), Err(Error::Refused(_))));
        assert!(check_target(5678, me).is_ok());
    }

    #[test]
    fn public_fns_refuse_without_touching_os() {
        let me = std::process::id();
        for pid in [0, 4, me] {
            assert!(matches!(kill(pid), Err(Error::Refused(_))));
            assert!(matches!(set_priority(pid, Priority::Normal), Err(Error::Refused(_))));
        }
    }

    #[test]
    fn realtime_refused() {
        // A pid that would otherwise pass check_target; refused before any OS call.
        assert!(matches!(set_priority(u32::MAX - 3, Priority::Realtime), Err(Error::Refused(_))));
    }

    #[test]
    fn priority_serde() {
        assert_eq!(serde_json::to_string(&Priority::BelowNormal).unwrap(), "\"below_normal\"");
    }
}
