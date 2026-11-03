//! "Start with Windows": a value in the current user's Run key that starts `Helm.exe --hidden`.
//!
//! The value name is `Helm`, the same one the console agent's tray used, so turning this on
//! replaces an old `helm-agent.exe --background` entry instead of starting both.

use std::path::Path;

pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
pub const RUN_VALUE: &str = "Helm";

/// Full registry path, for messages: `HKEY_CURRENT_USER\...\Run`.
pub fn registry_path() -> String {
    format!(r"HKEY_CURRENT_USER\{RUN_KEY}")
}

/// The command line stored in the Run value for `exe`.
pub fn command_for(exe: &Path) -> String {
    format!("\"{}\" {}", exe.display(), crate::args::HIDDEN)
}

/// The program part of a Run command line (quoted or not).
pub fn program_of(command: &str) -> &str {
    let c = command.trim();
    if let Some(rest) = c.strip_prefix('"') {
        return rest.split('"').next().unwrap_or("");
    }
    // Unquoted: up to ".exe" when present (paths may contain spaces), else the first word.
    let lower = c.to_ascii_lowercase();
    match lower.find(".exe") {
        Some(i) => &c[..i + 4],
        None => c.split_whitespace().next().unwrap_or(""),
    }
}

/// Whether a Run command line starts `exe` (paths compared case-insensitively, `/` = `\`).
pub fn points_to(command: &str, exe: &Path) -> bool {
    let norm = |s: &str| s.replace('/', "\\").to_lowercase();
    let program = program_of(command);
    !program.is_empty() && norm(program) == norm(&exe.display().to_string())
}

/// On only when the Run value exists and starts this very executable.
pub fn is_enabled() -> bool {
    match (read_value(), std::env::current_exe()) {
        (Some(v), Ok(exe)) => points_to(&v, &exe),
        _ => false,
    }
}

/// Whether the Run value exists and starts `exe` (a portable copy being replaced by an update).
pub fn starts(exe: &Path) -> bool {
    read_value().is_some_and(|v| points_to(&v, exe))
}

/// Add (or point at this exe) / remove the Run value.
pub fn set_enabled(on: bool) -> anyhow::Result<()> {
    if on {
        let exe = std::env::current_exe()?;
        write_value(&command_for(&exe))
    } else if read_value().is_some() {
        delete_value()
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn read_value() -> Option<String> {
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
    use windows::core::HSTRING;
    let key = HSTRING::from(RUN_KEY);
    let value = HSTRING::from(RUN_VALUE);
    let mut bytes = 0u32;
    // SAFETY: size-only query into a local.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &key,
            &value,
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut bytes),
        )
    };
    if status != ERROR_SUCCESS || bytes == 0 {
        return None;
    }
    let mut buf = vec![0u16; (bytes as usize).div_ceil(2)];
    // SAFETY: buf holds `bytes` bytes; RegGetValueW writes at most that and NUL-terminates.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &key,
            &value,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
            Some(&mut bytes),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

#[cfg(windows)]
fn write_value(command: &str) -> anyhow::Result<()> {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegSetKeyValueW};
    use windows::core::HSTRING;
    let key = HSTRING::from(RUN_KEY);
    let value = HSTRING::from(RUN_VALUE);
    let data: Vec<u16> = command.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: data is a NUL-terminated UTF-16 buffer of the given byte length.
    unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            &key,
            &value,
            REG_SZ.0,
            Some(data.as_ptr() as *const core::ffi::c_void),
            (data.len() * 2) as u32,
        )
        .ok()?;
    }
    Ok(())
}

#[cfg(windows)]
fn delete_value() -> anyhow::Result<()> {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RegDeleteKeyValueW};
    use windows::core::HSTRING;
    let key = HSTRING::from(RUN_KEY);
    let value = HSTRING::from(RUN_VALUE);
    // SAFETY: plain registry call with valid strings.
    unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, &key, &value).ok()? };
    Ok(())
}

#[cfg(not(windows))]
fn read_value() -> Option<String> {
    None
}
#[cfg(not(windows))]
fn write_value(_command: &str) -> anyhow::Result<()> {
    anyhow::bail!("Start with Windows is only available on Windows")
}
#[cfg(not(windows))]
fn delete_value() -> anyhow::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn command_quotes_path_and_starts_hidden() {
        assert_eq!(
            command_for(Path::new(r"C:\Users\Me\AppData\Local\Programs\Helm\Helm.exe")),
            r#""C:\Users\Me\AppData\Local\Programs\Helm\Helm.exe" --hidden"#
        );
    }

    #[test]
    fn registry_path_is_hkcu_run() {
        assert_eq!(
            registry_path(),
            r"HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run"
        );
    }

    #[test]
    fn program_of_quoted_and_unquoted() {
        assert_eq!(program_of(r#""C:\A B\Helm.exe" --hidden"#), r"C:\A B\Helm.exe");
        assert_eq!(program_of(r"C:\A B\Helm.exe --hidden"), r"C:\A B\Helm.exe");
        assert_eq!(program_of(r"C:\tools\helm"), r"C:\tools\helm");
        assert_eq!(program_of(""), "");
    }

    #[test]
    fn points_to_only_this_exe() {
        let exe = Path::new(r"C:\Program Files\Helm\Helm.exe");
        assert!(points_to(&command_for(exe), exe));
        assert!(points_to(r#""c:/program files/helm/HELM.EXE" --hidden"#, exe));
        // The old console agent's entry is not "on" for the desktop app.
        assert!(!points_to(
            r#""C:\Program Files\Helm\helm-agent.exe" --background"#,
            exe
        ));
        assert!(!points_to("", exe));
    }
}
