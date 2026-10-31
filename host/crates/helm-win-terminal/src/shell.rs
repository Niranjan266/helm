//! Which program runs in the pseudo console, and argument validation (pure, testable).

use crate::error::{Error, Result};

/// Largest accepted `cols` / `rows` (the pseudo console size is a signed 16-bit `COORD`).
pub const MAX_DIMENSION: u16 = i16::MAX as u16;

/// The program started inside the pseudo console.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shell {
    /// `%SystemRoot%\System32\cmd.exe`
    Cmd,
    /// Windows PowerShell 5.1: `%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe -NoLogo`
    PowerShell,
    /// PowerShell 7+: `pwsh.exe -NoLogo`, found through `PATH`.
    Pwsh,
    /// Any command line, passed to `CreateProcessW` unchanged (quote paths with spaces yourself).
    Custom(String),
}

impl Shell {
    /// The command line for `CreateProcessW`, with system programs resolved under
    /// `system_root` (normally `%SystemRoot%`, e.g. `C:\Windows`). Absolute paths avoid
    /// picking up a `cmd.exe` planted in the working directory or on `PATH`.
    pub fn command_line_in(&self, system_root: &str) -> Result<String> {
        let root = system_root.trim_end_matches(['\\', '/']);
        let line = match self {
            Shell::Cmd => quote_if_needed(&format!(r"{root}\System32\cmd.exe")),
            Shell::PowerShell => format!(
                "{} -NoLogo",
                quote_if_needed(&format!(r"{root}\System32\WindowsPowerShell\v1.0\powershell.exe"))
            ),
            Shell::Pwsh => "pwsh.exe -NoLogo".to_string(),
            Shell::Custom(c) => c.trim().to_string(),
        };
        if line.is_empty() {
            return Err(Error::InvalidConfig("empty command line".into()));
        }
        if line.contains('\0') {
            return Err(Error::InvalidConfig("command line contains NUL".into()));
        }
        Ok(line)
    }

    /// [`Shell::command_line_in`] with `%SystemRoot%` (falls back to `C:\Windows`).
    pub fn command_line(&self) -> Result<String> {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
        self.command_line_in(&root)
    }
}

fn quote_if_needed(path: &str) -> String {
    if path.contains(' ') { format!("\"{path}\"") } else { path.to_string() }
}

/// Checks a terminal size: both dimensions in `1..=MAX_DIMENSION`.
pub fn validate_size(cols: u16, rows: u16) -> Result<()> {
    if cols == 0 || rows == 0 || cols > MAX_DIMENSION || rows > MAX_DIMENSION {
        return Err(Error::InvalidConfig(format!(
            "terminal size {cols}x{rows} out of range (1..={MAX_DIMENSION})"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_shells_use_absolute_paths() {
        assert_eq!(Shell::Cmd.command_line_in(r"C:\Windows").unwrap(), r"C:\Windows\System32\cmd.exe");
        assert_eq!(Shell::Cmd.command_line_in(r"C:\Windows\").unwrap(), r"C:\Windows\System32\cmd.exe");
        assert_eq!(
            Shell::PowerShell.command_line_in(r"C:\Windows").unwrap(),
            r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe -NoLogo"
        );
        assert_eq!(Shell::Pwsh.command_line_in("X:").unwrap(), "pwsh.exe -NoLogo");
    }

    #[test]
    fn root_with_space_is_quoted() {
        assert_eq!(Shell::Cmd.command_line_in(r"D:\My Win").unwrap(), r#""D:\My Win\System32\cmd.exe""#);
    }

    #[test]
    fn custom_validation() {
        assert_eq!(Shell::Custom(" cmd /k ver ".into()).command_line_in("C:").unwrap(), "cmd /k ver");
        assert!(Shell::Custom("   ".into()).command_line_in("C:").is_err());
        assert!(Shell::Custom("a\0b".into()).command_line_in("C:").is_err());
    }

    #[test]
    fn size_limits() {
        assert!(validate_size(80, 24).is_ok());
        assert!(validate_size(1, 1).is_ok());
        assert!(validate_size(MAX_DIMENSION, MAX_DIMENSION).is_ok());
        assert!(validate_size(0, 24).is_err());
        assert!(validate_size(80, 0).is_err());
        assert!(validate_size(MAX_DIMENSION + 1, 24).is_err());
    }
}
