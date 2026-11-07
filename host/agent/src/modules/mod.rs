//! Feature modules. Each one implements [`crate::router::Module`].

use std::sync::Arc;

use serde_json::Value;

use crate::router::{Module, RpcError};

pub mod apps;
pub mod clipboard;
pub mod devices;
pub mod files;
pub mod layout;
pub mod media;
pub mod power;
pub mod relay;
pub mod screen;
pub mod stream;
pub mod system;
// Phase 3
pub mod desktop;
pub mod fs;
pub mod monitor;
// Phase 5
pub mod extras;
// g6 (everyday)
pub mod everyday;
// G3 (Docs/protocol/streaming-2.md)
pub mod g3;
// G2: presentation mode
pub mod present;
// g8: pointer accuracy (Docs/protocol/pointer.md)
pub mod pointer;
// G5: dock folders, smart order, deck pages, icons
pub mod g5;
// g9: the PC views and controls the phone (Docs/protocol/phone-control.md)
pub mod phone_control;

/// All modules shipped in this build.
pub fn all() -> Vec<Arc<dyn Module>> {
    vec![
        Arc::new(system::SystemModule),
        Arc::new(devices::DevicesModule),
        Arc::new(apps::AppsModule),
        Arc::new(layout::LayoutModule),
        Arc::new(media::MediaModule),
        Arc::new(power::PowerModule),
        Arc::new(clipboard::ClipboardModule),
        Arc::new(files::FilesModule),
        Arc::new(screen::ScreenModule),
        Arc::new(stream::StreamModule),
        Arc::new(relay::RelayModule),
        Arc::new(monitor::MonitorModule),
        Arc::new(desktop::DesktopModule),
        Arc::new(fs::FsModule),
        Arc::new(extras::ExtrasModule),
        Arc::new(crate::g4::PairingModule),
        Arc::new(everyday::EverydayModule),
        Arc::new(g3::G3Module),
        Arc::new(present::PresentModule),
        Arc::new(g5::G5Module),
        Arc::new(pointer::PointerModule),
        Arc::new(phone_control::PhoneControlModule),
    ]
}

/// What a phone can do before anyone signs in to Windows (the agent runs as LocalSystem
/// with no desktop): stay connected, read basic info, and lock / sleep / restart / shut
/// down the PC. Nothing that shows the screen, injects input, opens files, runs programs or
/// changes paired devices or settings.
pub const PRE_LOGIN_METHODS: &[&str] = &[
    "system.ping",
    "system.info",
    "power.action",
    "power.cancel",
    "power.status",
    "relay.get",
    "wol.send",
    "devices.list",
];

/// Required string parameter.
pub(crate) fn str_param<'a>(params: &'a Value, name: &str) -> Result<&'a str, RpcError> {
    params
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::bad_params(format!("{name} (string) is required")))
}

/// Optional string parameter (`null` = absent).
pub(crate) fn opt_str<'a>(params: &'a Value, name: &str) -> Result<Option<&'a str>, RpcError> {
    match params.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(RpcError::bad_params(format!("{name} must be a string"))),
    }
}

pub(crate) fn u64_param(params: &Value, name: &str) -> Result<u64, RpcError> {
    params
        .get(name)
        .and_then(Value::as_u64)
        .ok_or_else(|| RpcError::bad_params(format!("{name} (non-negative integer) is required")))
}

pub(crate) fn opt_u64(params: &Value, name: &str) -> Result<Option<u64>, RpcError> {
    match params.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .map(Some)
            .ok_or_else(|| RpcError::bad_params(format!("{name} must be a non-negative integer"))),
    }
}

pub(crate) fn opt_bool(params: &Value, name: &str) -> Result<Option<bool>, RpcError> {
    match params.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(RpcError::bad_params(format!(
            "{name} must be true or false"
        ))),
    }
}

pub(crate) fn opt_f64(params: &Value, name: &str) -> Result<Option<f64>, RpcError> {
    match params.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_f64()
            .map(Some)
            .ok_or_else(|| RpcError::bad_params(format!("{name} must be a number"))),
    }
}

/// Deserialize one parameter into a typed value.
pub(crate) fn typed_param<T: serde::de::DeserializeOwned>(
    params: &Value,
    name: &str,
) -> Result<T, RpcError> {
    let v = params
        .get(name)
        .ok_or_else(|| RpcError::bad_params(format!("{name} is required")))?;
    serde_json::from_value(v.clone()).map_err(|e| RpcError::bad_params(format!("{name}: {e}")))
}

pub(crate) fn internal(e: impl std::fmt::Display) -> RpcError {
    RpcError::internal(e.to_string())
}

/// `open{target}` check: http/https/mailto URL, or an existing absolute local path.
pub(crate) fn check_open_target(target: &str) -> Result<String, RpcError> {
    use helm_win_apps::util::{Target, classify_target, ensure_exists};
    match classify_target(target).map_err(crate::platform::apps_err)? {
        Target::Url(u) => Ok(u),
        Target::Path(p) => {
            ensure_exists(&p).map_err(crate::platform::apps_err)?;
            Ok(p)
        }
    }
}
