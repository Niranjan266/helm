//! Non-Windows stand-ins: every operation reports `Error::Unsupported`.

use crate::{AppEntry, Error, FocusResult, Result, WindowInfo, util::Target};

#[derive(Debug, Clone, Copy)]
pub enum ShowCmd {
    Minimize,
    Maximize,
    Restore,
}

pub fn list_apps() -> Result<Vec<AppEntry>> {
    Err(Error::Unsupported)
}
pub fn app_icon_png(_app_id: &str, _size_px: u32) -> Result<Vec<u8>> {
    Err(Error::Unsupported)
}
pub fn launch_app(_app_id: &str) -> Result<()> {
    Err(Error::Unsupported)
}
pub fn open_target(_target: Target) -> Result<()> {
    Err(Error::Unsupported)
}
pub fn list_windows() -> Result<Vec<WindowInfo>> {
    Err(Error::Unsupported)
}
pub fn focus_window(_hwnd: u64) -> Result<FocusResult> {
    Err(Error::Unsupported)
}
pub fn show_window(_hwnd: u64, _cmd: ShowCmd) -> Result<()> {
    Err(Error::Unsupported)
}
pub fn close_window(_hwnd: u64) -> Result<()> {
    Err(Error::Unsupported)
}
