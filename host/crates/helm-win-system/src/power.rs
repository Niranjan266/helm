//! Power actions and a delayed-action scheduler.
//!
//! Real actions go through [`SystemPowerBackend`] (`LockWorkStation`,
//! `SetSuspendState`, `InitiateShutdownW`, `ExitWindowsEx`, with
//! `SeShutdownPrivilege` enabled first where needed). Tests and tools should use
//! [`DryRunBackend`], which only records the actions it was asked to perform.

use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// A power action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PowerAction {
    /// Lock the workstation.
    Lock,
    /// Sleep (S3 / modern standby).
    Sleep,
    /// Hibernate (fails if hibernation is disabled).
    Hibernate,
    /// Restart.
    Restart,
    /// Shut down and power off.
    Shutdown,
    /// Sign out the interactive user.
    SignOut,
}

/// Something that can carry out power actions.
pub trait PowerBackend: Send + Sync + 'static {
    /// Perform `action` now.
    fn execute(&self, action: PowerAction) -> Result<()>;
}

/// The real backend: actually locks / sleeps / shuts down this PC.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemPowerBackend;

impl PowerBackend for SystemPowerBackend {
    fn execute(&self, action: PowerAction) -> Result<()> {
        imp::execute(action)
    }
}

/// A fake backend that records actions instead of performing them.
#[derive(Debug, Default)]
pub struct DryRunBackend {
    log: Mutex<Vec<PowerAction>>,
}

impl DryRunBackend {
    /// New, empty recorder.
    pub fn new() -> Self {
        Self::default()
    }
    /// Actions "executed" so far, oldest first.
    pub fn executed(&self) -> Vec<PowerAction> {
        self.log.lock().map(|l| l.clone()).unwrap_or_default()
    }
}

impl PowerBackend for DryRunBackend {
    fn execute(&self, action: PowerAction) -> Result<()> {
        if let Ok(mut l) = self.log.lock() {
            l.push(action);
        }
        Ok(())
    }
}

/// Perform a power action on this PC **now**. Use [`PowerScheduler`] for
/// delayed actions and [`DryRunBackend`] in tests.
///
/// Restart / shutdown are not forced: apps with unsaved work may hold them up
/// (Windows shows its usual "apps are preventing shutdown" screen).
pub fn power_action(action: PowerAction) -> Result<()> {
    SystemPowerBackend.execute(action)
}

/// A pending scheduled action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduledPower {
    /// What will happen.
    pub action: PowerAction,
    /// When, as Unix time in milliseconds.
    pub due_unix_ms: u64,
    /// Milliseconds left at the time this value was produced.
    pub remaining_ms: u64,
}

/// Longest allowed delay.
pub const MAX_DELAY: Duration = Duration::from_secs(31 * 24 * 3600);

type Listener = Box<dyn Fn(PowerAction, &Result<()>) + Send + 'static>;

struct Job {
    id: u64,
    action: PowerAction,
    deadline: Instant,
    due_unix_ms: u64,
}

impl Job {
    fn snapshot(&self) -> ScheduledPower {
        ScheduledPower {
            action: self.action,
            due_unix_ms: self.due_unix_ms,
            remaining_ms: self.deadline.saturating_duration_since(Instant::now()).as_millis() as u64,
        }
    }
}

struct State {
    next_id: u64,
    job: Option<Job>,
}

struct Shared {
    backend: Arc<dyn PowerBackend>,
    state: Mutex<State>,
    cv: Condvar,
    listener: Mutex<Option<Listener>>,
}

/// Runs at most one delayed power action at a time (no tokio needed: each
/// scheduled action waits on its own short-lived thread with a condvar, so
/// cancel / reschedule take effect immediately).
///
/// Scheduling replaces any pending action. Dropping the scheduler cancels the
/// pending action.
pub struct PowerScheduler {
    shared: Arc<Shared>,
}

impl PowerScheduler {
    /// Scheduler with a custom backend.
    pub fn new(backend: Arc<dyn PowerBackend>) -> Self {
        PowerScheduler {
            shared: Arc::new(Shared {
                backend,
                state: Mutex::new(State { next_id: 1, job: None }),
                cv: Condvar::new(),
                listener: Mutex::new(None),
            }),
        }
    }

    /// Scheduler that performs real power actions.
    pub fn system() -> Self {
        Self::new(Arc::new(SystemPowerBackend))
    }

    /// Scheduler with a [`DryRunBackend`]; the returned handle shows what ran.
    pub fn dry_run() -> (Self, Arc<DryRunBackend>) {
        let b = Arc::new(DryRunBackend::new());
        (Self::new(b.clone()), b)
    }

    /// Called (on the scheduler thread) after a scheduled action ran, with its result.
    pub fn set_listener<F>(&self, f: F)
    where
        F: Fn(PowerAction, &Result<()>) + Send + 'static,
    {
        if let Ok(mut l) = self.shared.listener.lock() {
            *l = Some(Box::new(f));
        }
    }

    /// Schedule `action` after `delay`, replacing any pending action.
    pub fn schedule(&self, action: PowerAction, delay: Duration) -> Result<ScheduledPower> {
        if delay > MAX_DELAY {
            return Err(Error::InvalidArgument(format!("delay {delay:?} exceeds {MAX_DELAY:?}")));
        }
        let now_unix = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        let (id, snap) = {
            let mut st = self.shared.state.lock().map_err(|_| Error::Internal("scheduler poisoned".into()))?;
            let id = st.next_id;
            st.next_id += 1;
            let job = Job {
                id,
                action,
                deadline: Instant::now() + delay,
                due_unix_ms: (now_unix + delay).as_millis() as u64,
            };
            let snap = job.snapshot();
            st.job = Some(job);
            (id, snap)
        };
        // Wake any previous waiter so it notices it was replaced.
        self.shared.cv.notify_all();
        let shared = Arc::clone(&self.shared);
        thread::Builder::new()
            .name("helm-power-timer".into())
            .spawn(move || wait_and_run(shared, id))
            .map_err(|e| Error::Internal(format!("spawn timer: {e}")))?;
        Ok(snap)
    }

    /// Cancel the pending action. Returns what was cancelled, if anything.
    pub fn cancel(&self) -> Option<ScheduledPower> {
        let job = self.shared.state.lock().ok()?.job.take();
        self.shared.cv.notify_all();
        job.map(|j| j.snapshot())
    }

    /// The pending action, if any.
    pub fn status(&self) -> Option<ScheduledPower> {
        self.shared.state.lock().ok()?.job.as_ref().map(Job::snapshot)
    }
}

impl Drop for PowerScheduler {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn wait_and_run(shared: Arc<Shared>, id: u64) {
    let Ok(mut st) = shared.state.lock() else { return };
    let action = loop {
        let deadline = match st.job.as_ref() {
            Some(j) if j.id == id => j.deadline,
            _ => return, // cancelled or replaced
        };
        let now = Instant::now();
        if now >= deadline {
            let job = st.job.take();
            break job.map(|j| j.action);
        }
        st = match shared.cv.wait_timeout(st, deadline - now) {
            Ok((g, _)) => g,
            Err(_) => return,
        };
    };
    drop(st);
    if let Some(action) = action {
        let result = shared.backend.execute(action);
        if let Ok(l) = shared.listener.lock() {
            if let Some(f) = l.as_ref() {
                f(action, &result);
            }
        }
    }
}

#[cfg(windows)]
mod imp {
    use windows::Win32::Foundation::{CloseHandle, ERROR_NOT_ALL_ASSIGNED, GetLastError, HANDLE, LUID};
    use windows::Win32::Security::{
        AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED, SE_SHUTDOWN_NAME,
        TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
    };
    use windows::Win32::System::Power::SetSuspendState;
    use windows::Win32::System::Shutdown::{
        EWX_LOGOFF, ExitWindowsEx, InitiateShutdownW, LockWorkStation, SHTDN_REASON_FLAG_PLANNED,
        SHTDN_REASON_MAJOR_OTHER, SHTDN_REASON_MINOR_OTHER, SHUTDOWN_POWEROFF, SHUTDOWN_RESTART,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    use windows::core::PCWSTR;

    use super::PowerAction;
    use crate::{Error, Result};

    fn last_error() -> Error {
        windows::core::Error::from_thread().into()
    }

    fn enable_shutdown_privilege() -> Result<()> {
        unsafe {
            let mut token = HANDLE::default();
            OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token)?;
            let mut luid = LUID::default();
            let r = LookupPrivilegeValueW(PCWSTR::null(), SE_SHUTDOWN_NAME, &mut luid).and_then(|_| {
                let tp = TOKEN_PRIVILEGES {
                    PrivilegeCount: 1,
                    Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }],
                };
                AdjustTokenPrivileges(token, false, Some(&tp), 0, None, None)
            });
            let not_all = GetLastError() == ERROR_NOT_ALL_ASSIGNED;
            let _ = CloseHandle(token);
            r?;
            if not_all {
                return Err(Error::Rejected("SeShutdownPrivilege not held by this account".into()));
            }
        }
        Ok(())
    }

    fn shutdown(flags: windows::Win32::System::Shutdown::SHUTDOWN_FLAGS) -> Result<()> {
        enable_shutdown_privilege()?;
        let reason = SHTDN_REASON_MAJOR_OTHER | SHTDN_REASON_MINOR_OTHER | SHTDN_REASON_FLAG_PLANNED;
        let rc = unsafe { InitiateShutdownW(PCWSTR::null(), PCWSTR::null(), 0, flags, reason) };
        if rc == 0 {
            Ok(())
        } else {
            Err(windows::core::Error::from(windows::core::HRESULT::from_win32(rc)).into())
        }
    }

    pub(super) fn execute(action: PowerAction) -> Result<()> {
        unsafe {
            match action {
                PowerAction::Lock => Ok(LockWorkStation()?),
                PowerAction::Sleep | PowerAction::Hibernate => {
                    enable_shutdown_privilege()?;
                    if SetSuspendState(action == PowerAction::Hibernate, false, false) {
                        Ok(())
                    } else {
                        Err(last_error())
                    }
                }
                PowerAction::Restart => shutdown(SHUTDOWN_RESTART),
                PowerAction::Shutdown => shutdown(SHUTDOWN_POWEROFF),
                PowerAction::SignOut => {
                    let reason = SHTDN_REASON_MAJOR_OTHER | SHTDN_REASON_MINOR_OTHER | SHTDN_REASON_FLAG_PLANNED;
                    Ok(ExitWindowsEx(EWX_LOGOFF, reason)?)
                }
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::PowerAction;
    use crate::{Error, Result};

    pub(super) fn execute(_: PowerAction) -> Result<()> {
        Err(Error::Unsupported("power actions require Windows"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn dry_run_executes_after_delay() {
        let (s, backend) = PowerScheduler::dry_run();
        let (tx, rx) = mpsc::channel();
        s.set_listener(move |a, r| {
            let _ = tx.send((a, r.is_ok()));
        });
        let snap = s.schedule(PowerAction::Shutdown, Duration::from_millis(80)).unwrap();
        assert_eq!(snap.action, PowerAction::Shutdown);
        assert!(snap.remaining_ms <= 80);
        assert_eq!(s.status().map(|p| p.action), Some(PowerAction::Shutdown));
        assert!(backend.executed().is_empty());
        let (a, ok) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!((a, ok), (PowerAction::Shutdown, true));
        assert_eq!(backend.executed(), vec![PowerAction::Shutdown]);
        assert!(s.status().is_none());
    }

    #[test]
    fn cancel_prevents_execution() {
        let (s, backend) = PowerScheduler::dry_run();
        s.schedule(PowerAction::Restart, Duration::from_millis(100)).unwrap();
        let c = s.cancel().unwrap();
        assert_eq!(c.action, PowerAction::Restart);
        assert!(s.cancel().is_none());
        assert!(s.status().is_none());
        thread::sleep(Duration::from_millis(250));
        assert!(backend.executed().is_empty());
    }

    #[test]
    fn reschedule_replaces_pending() {
        let (s, backend) = PowerScheduler::dry_run();
        s.schedule(PowerAction::Shutdown, Duration::from_millis(60)).unwrap();
        s.schedule(PowerAction::Lock, Duration::from_millis(120)).unwrap();
        assert_eq!(s.status().unwrap().action, PowerAction::Lock);
        thread::sleep(Duration::from_millis(400));
        assert_eq!(backend.executed(), vec![PowerAction::Lock]);
    }

    #[test]
    fn drop_cancels() {
        let (s, backend) = PowerScheduler::dry_run();
        s.schedule(PowerAction::Sleep, Duration::from_millis(60)).unwrap();
        drop(s);
        thread::sleep(Duration::from_millis(200));
        assert!(backend.executed().is_empty());
    }

    #[test]
    fn zero_delay_and_limits() {
        let (s, backend) = PowerScheduler::dry_run();
        assert!(matches!(
            s.schedule(PowerAction::Lock, MAX_DELAY + Duration::from_secs(1)),
            Err(Error::InvalidArgument(_))
        ));
        s.schedule(PowerAction::SignOut, Duration::ZERO).unwrap();
        let t = Instant::now();
        while backend.executed().is_empty() && t.elapsed() < Duration::from_secs(5) {
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(backend.executed(), vec![PowerAction::SignOut]);
    }

    #[test]
    fn serde_shape() {
        assert_eq!(serde_json::to_value(PowerAction::SignOut).unwrap(), "sign_out");
        let a: PowerAction = serde_json::from_str("\"hibernate\"").unwrap();
        assert_eq!(a, PowerAction::Hibernate);
        let v = serde_json::to_value(ScheduledPower { action: PowerAction::Shutdown, due_unix_ms: 1, remaining_ms: 2 }).unwrap();
        assert_eq!(v, serde_json::json!({"action": "shutdown", "due_unix_ms": 1, "remaining_ms": 2}));
    }
}
