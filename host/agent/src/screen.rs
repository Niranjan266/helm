//! `screen.monitors` / `screen.capture` backed by `helm-win-capture` (GDI stills).

use crate::platform::{Monitor, PResult};
use crate::router::RpcError;

#[cfg(windows)]
pub fn monitors() -> PResult<Vec<Monitor>> {
    Ok(helm_win_capture::monitors()
        .into_iter()
        .map(|m| Monitor {
            id: m.id,
            name: m.name,
            x: m.x,
            y: m.y,
            width: m.width,
            height: m.height,
            is_primary: m.is_primary,
        })
        .collect())
}

/// `None` = the primary monitor.
#[cfg(windows)]
pub fn screenshot_png(monitor_id: Option<&str>) -> PResult<Vec<u8>> {
    let id = match monitor_id {
        Some(id) => {
            if !helm_win_capture::monitors().iter().any(|m| m.id == id) {
                return Err(RpcError::bad_params(format!("no monitor with id {id:?}")));
            }
            id.to_string()
        }
        None => helm_win_capture::monitors()
            .into_iter()
            .find(|m| m.is_primary)
            .map(|m| m.id)
            .ok_or_else(|| RpcError::internal("no primary monitor found"))?,
    };
    helm_win_capture::screenshot_png(Some(&id)).map_err(|e| RpcError::internal(e.to_string()))
}

#[cfg(not(windows))]
pub fn monitors() -> PResult<Vec<Monitor>> {
    Err(RpcError::internal(
        "screen capture is only available on Windows",
    ))
}

#[cfg(not(windows))]
pub fn screenshot_png(_monitor_id: Option<&str>) -> PResult<Vec<u8>> {
    Err(RpcError::internal(
        "screen capture is only available on Windows",
    ))
}
