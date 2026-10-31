//! Non-Windows stand-in so the crate (and its pure modules) builds everywhere.

use crate::error::{Error, Result};
use crate::shell::Shell;
use std::path::Path;
use std::time::Duration;

/// ConPTY session. Only available on Windows; every constructor fails elsewhere.
pub struct Terminal {
    _private: (),
}

impl Terminal {
    pub fn spawn<F>(_shell: Shell, _cols: u16, _rows: u16, _cwd: Option<&Path>, _on_output: F) -> Result<Terminal>
    where
        F: FnMut(&[u8]) + Send + 'static,
    {
        Err(Error::Unsupported("ConPTY terminals need Windows 10 1809+".into()))
    }

    pub fn spawn_with_exit<F, E>(
        shell: Shell,
        cols: u16,
        rows: u16,
        cwd: Option<&Path>,
        on_output: F,
        _on_exit: E,
    ) -> Result<Terminal>
    where
        F: FnMut(&[u8]) + Send + 'static,
        E: FnOnce(u32) + Send + 'static,
    {
        Self::spawn(shell, cols, rows, cwd, on_output)
    }

    pub fn pid(&self) -> u32 {
        0
    }
    pub fn write(&self, _bytes: &[u8]) -> Result<()> {
        Err(Error::Closed)
    }
    pub fn resize(&self, _cols: u16, _rows: u16) -> Result<()> {
        Err(Error::Closed)
    }
    pub fn size(&self) -> (u16, u16) {
        (0, 0)
    }
    pub fn wait(&self, _timeout: Option<Duration>) -> Option<u32> {
        Some(crate::KILLED_EXIT_CODE)
    }
    pub fn try_wait(&self) -> Option<u32> {
        Some(crate::KILLED_EXIT_CODE)
    }
    pub fn exit_code(&self) -> Option<u32> {
        Some(crate::KILLED_EXIT_CODE)
    }
    pub fn is_running(&self) -> bool {
        false
    }
    pub fn was_killed(&self) -> bool {
        false
    }
    pub fn kill(&self) -> Result<()> {
        Ok(())
    }
}
