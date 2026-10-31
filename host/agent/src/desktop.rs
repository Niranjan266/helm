//! Small desktop-integration helpers for `main.rs`: session checks, one instance per data
//! folder, message boxes, and the background log file.

use std::path::{Path, PathBuf};

/// Terminal Services session of this process (`None` if unknown / not Windows).
/// Session 0 is the services session: no desktop, so input and the tray cannot work there.
pub fn current_session_id() -> Option<u32> {
    #[cfg(windows)]
    {
        use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
        let mut session = 0u32;
        // SAFETY: out-pointer to a local.
        let ok = unsafe { ProcessIdToSessionId(std::process::id(), &mut session) }.is_ok();
        ok.then_some(session)
    }
    #[cfg(not(windows))]
    None
}

/// Name of the per-session mutex that keeps one agent per data folder.
pub fn instance_mutex_name(data_dir: &Path) -> String {
    use sha2::{Digest, Sha256};
    let canonical = std::fs::canonicalize(data_dir).unwrap_or_else(|_| data_dir.to_path_buf());
    let key = canonical.to_string_lossy().to_lowercase();
    let digest = Sha256::digest(key.as_bytes());
    format!("Local\\HelmAgent-{}", hex::encode(&digest[..8]))
}

/// Held for the life of the process; `None` from [`single_instance`] = another agent runs.
pub struct InstanceGuard {
    #[cfg(windows)]
    _handle: windows::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl Drop for InstanceGuard {
    fn drop(&mut self) {
        if !self._handle.is_invalid() {
            // SAFETY: we own this handle and close it once.
            unsafe {
                let _ = windows::Win32::Foundation::CloseHandle(self._handle);
            }
        }
    }
}

// SAFETY: a mutex handle is just kept open until exit; it is never used across threads.
unsafe impl Send for InstanceGuard {}

/// Claim this data folder. Returns `None` if another agent in this session already has it
/// (e.g. the service-started agent and a "Start with Windows" one).
pub fn single_instance(data_dir: &Path) -> Option<InstanceGuard> {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
        use windows::Win32::System::Threading::CreateMutexW;
        use windows::core::HSTRING;
        let name = HSTRING::from(instance_mutex_name(data_dir));
        // SAFETY: plain object creation; the handle is kept until the process exits.
        let handle = match unsafe { CreateMutexW(None, false, &name) } {
            Ok(h) => h,
            // Could not create it at all: don't block the agent over this.
            Err(_) => {
                return Some(InstanceGuard {
                    _handle: Default::default(),
                });
            }
        };
        // SAFETY: reads this thread's last-error value right after CreateMutexW.
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            return None;
        }
        Some(InstanceGuard { _handle: handle })
    }
    #[cfg(not(windows))]
    {
        let _ = data_dir;
        Some(InstanceGuard {})
    }
}

/// Show a message box (blocking). With `yes_no`, returns true for Yes.
pub fn message_box(title: &str, text: &str, yes_no: bool) -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::UI::WindowsAndMessaging::{
            IDYES, MB_ICONINFORMATION, MB_OK, MB_SETFOREGROUND, MB_YESNO, MessageBoxW,
        };
        use windows::core::HSTRING;
        let style = if yes_no { MB_YESNO } else { MB_OK } | MB_ICONINFORMATION | MB_SETFOREGROUND;
        // SAFETY: no owner window; strings live for the call.
        let r = unsafe { MessageBoxW(None, &HSTRING::from(text), &HSTRING::from(title), style) };
        r == IDYES
    }
    #[cfg(not(windows))]
    {
        println!("{title}: {text}");
        false
    }
}

pub const LOG_FILE: &str = "agent.log";
const LOG_ROTATE_BYTES: u64 = 1024 * 1024;

/// Open `<data_dir>\agent.log` for appending (rotated to `agent.log.1` past 1 MiB).
/// Used in `--background`, where there is no console to print to.
pub fn open_log(data_dir: &Path) -> std::io::Result<(std::fs::File, PathBuf)> {
    std::fs::create_dir_all(data_dir)?;
    let path = data_dir.join(LOG_FILE);
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > LOG_ROTATE_BYTES) {
        let _ = std::fs::rename(&path, data_dir.join(format!("{LOG_FILE}.1")));
    }
    let f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    Ok((f, path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutex_name_is_per_folder_and_case_insensitive() {
        let a = instance_mutex_name(Path::new("C:/nope/Helm-A"));
        let b = instance_mutex_name(Path::new("C:/nope/helm-a"));
        let c = instance_mutex_name(Path::new("C:/nope/Helm-B"));
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.starts_with("Local\\HelmAgent-") && a.len() == "Local\\HelmAgent-".len() + 16);
    }

    #[test]
    fn second_instance_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let first = single_instance(dir.path());
        assert!(first.is_some());
        if cfg!(windows) {
            assert!(single_instance(dir.path()).is_none());
        }
        drop(first);
    }

    #[test]
    fn log_rotates() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(LOG_FILE), vec![b'x'; 2 * 1024 * 1024]).unwrap();
        let (_f, path) = open_log(dir.path()).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
        assert!(dir.path().join("agent.log.1").exists());
    }

    #[test]
    fn session_id_is_known_on_windows() {
        assert_eq!(current_session_id().is_some(), cfg!(windows));
    }
}
