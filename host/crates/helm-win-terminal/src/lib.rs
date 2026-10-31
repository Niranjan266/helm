//! # helm-win-terminal
//!
//! Interactive terminal sessions for the Helm PC agent, built on the Windows pseudo console
//! (ConPTY, Windows 10 1809+). A [`Terminal`] runs `cmd.exe` or PowerShell attached to a
//! pseudo console; you write keystrokes (UTF-8 / VT input) with [`Terminal::write`] and
//! receive the rendered screen as raw VT bytes in the output callback, ready to feed into a
//! terminal emulator on the phone.
//!
//! The process tree runs inside a kill-on-close Job object, so [`Terminal::kill`] and
//! dropping the handle end the shell *and* everything it started.
//!
//! See `README.md` for the threading model and how the agent should use this crate.

pub mod error;
pub mod shell;
pub mod vt;

#[cfg(windows)]
mod conpty;
#[cfg(not(windows))]
mod stub;

pub use error::{Error, Result};
pub use shell::{MAX_DIMENSION, Shell, validate_size};

#[cfg(windows)]
pub use conpty::Terminal;
#[cfg(not(windows))]
pub use stub::Terminal;

/// Exit code reported for a session ended with [`Terminal::kill`] (or by dropping it).
/// [`Terminal::was_killed`] tells it apart from a shell that really returned 1.
pub const KILLED_EXIT_CODE: u32 = 1;
