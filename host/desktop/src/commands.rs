//! Tauri commands the window calls (`window.__TAURI__.core.invoke`).

use std::sync::{Arc, Mutex};

use helm_agent::router::Tier;
use helm_agent::update::{self, UpdateStatus};
use serde::Serialize;
use tauri::State;

use crate::activity::Entry;
use crate::controller::{Controller, PairingView, PhoneView, SettingsView, StatusView};
use crate::settings::{Settings, validate_port};

type Ctl<'a> = State<'a, Arc<Controller>>;

/// Last "update available" result, so Download does not have to check again.
#[derive(Default)]
pub struct UpdateCache(pub Mutex<Option<UpdateStatus>>);

#[tauri::command]
pub fn get_status(ctl: Ctl<'_>) -> StatusView {
    ctl.status()
}

#[tauri::command]
pub fn get_phones(ctl: Ctl<'_>) -> Vec<PhoneView> {
    ctl.phones()
}

#[tauri::command]
pub fn set_tier(ctl: Ctl<'_>, device_id: String, tier: String) -> Result<(), String> {
    let tier = match tier.as_str() {
        "viewer" => Tier::Viewer,
        "remote" => Tier::Remote,
        "admin" => Tier::Admin,
        other => return Err(format!("unknown access level {other:?}")),
    };
    ctl.set_tier(&device_id, tier)
}

#[tauri::command]
pub fn revoke_phone(ctl: Ctl<'_>, device_id: String) -> Result<bool, String> {
    ctl.revoke(&device_id)
}

#[tauri::command]
pub fn get_pairing(ctl: Ctl<'_>) -> Result<Option<PairingView>, String> {
    ctl.current_pairing()
}

#[tauri::command]
pub fn new_pairing(ctl: Ctl<'_>) -> Result<PairingView, String> {
    ctl.new_pairing()
}

#[tauri::command]
pub fn get_activity(ctl: Ctl<'_>, limit: Option<usize>) -> Result<Vec<Entry>, String> {
    ctl.activity(limit.unwrap_or(200).min(1000))
}

#[tauri::command]
pub fn get_settings(ctl: Ctl<'_>) -> SettingsView {
    ctl.settings()
}

#[tauri::command]
pub fn set_autostart(on: bool) -> Result<bool, String> {
    crate::autostart::set_enabled(on).map_err(|e| format!("{e:#}"))?;
    Ok(crate::autostart::is_enabled())
}

#[tauri::command]
pub async fn set_port(ctl: Ctl<'_>, port: u32) -> Result<(), String> {
    let port = validate_port(port)?;
    let mut s = Settings::load(&ctl.data_dir);
    if s.port == port {
        return Ok(());
    }
    s.port = port;
    s.save(&ctl.data_dir).map_err(|e| format!("{e:#}"))?;
    let ctl = ctl.inner().clone();
    if ctl.run_state() == crate::status::RunState::Running {
        ctl.restart().await;
    }
    Ok(())
}

#[tauri::command]
pub fn set_relay(ctl: Ctl<'_>, enabled: bool, url: String) -> Result<(), String> {
    ctl.set_relay(enabled, &url)
}

#[tauri::command]
pub async fn set_paused(ctl: Ctl<'_>, paused: bool) -> Result<(), String> {
    ctl.inner().clone().set_paused(paused).await;
    Ok(())
}

#[derive(Serialize)]
pub struct UpdateView {
    /// "disabled" | "uptodate" | "available" | "failed"
    pub state: &'static str,
    pub summary: String,
}

#[tauri::command]
pub async fn check_updates(cache: State<'_, UpdateCache>) -> Result<UpdateView, String> {
    let status = tauri::async_runtime::spawn_blocking(update::check)
        .await
        .map_err(|e| e.to_string())?;
    let state = match &status {
        UpdateStatus::Disabled => "disabled",
        UpdateStatus::UpToDate => "uptodate",
        UpdateStatus::Available(_) => "available",
        UpdateStatus::Failed(_) => "failed",
    };
    let summary = status.summary();
    *cache.0.lock().unwrap_or_else(|p| p.into_inner()) = Some(status);
    Ok(UpdateView { state, summary })
}

/// Download + verify the installer found by the last check and show it in Explorer.
/// Nothing is installed or run.
#[tauri::command]
pub async fn download_update(cache: State<'_, UpdateCache>) -> Result<String, String> {
    let candidate = match cache.0.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
        Some(UpdateStatus::Available(c)) => c.clone(),
        _ => return Err("Check for updates first.".into()),
    };
    let path = tauri::async_runtime::spawn_blocking(move || update::download(&candidate))
        .await
        .map_err(|e| e.to_string())??;
    if let Some(folder) = path.parent() {
        let _ = helm_win_apps::open_target(&folder.to_string_lossy());
    }
    Ok(path.display().to_string())
}

#[tauri::command]
pub fn open_data_dir(ctl: Ctl<'_>) -> Result<(), String> {
    helm_win_apps::open_target(&ctl.data_dir.to_string_lossy()).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn quit_app(app: tauri::AppHandle) {
    app.exit(0);
}
