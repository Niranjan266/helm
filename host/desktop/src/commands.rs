//! Tauri commands the window calls (`window.__TAURI__.core.invoke`).

use std::sync::Arc;

use helm_agent::g4::PendingPairing;
use helm_agent::router::Tier;
use tauri::State;

use crate::activity::Entry;
use crate::controller::{CodeView, Controller, PairingView, PhoneView, SettingsView, StatusView};
use crate::settings::{Settings, validate_port};
use crate::updater::{UpdateView, Updates};

type Ctl<'a> = State<'a, Arc<Controller>>;

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
pub fn get_code_pairing(ctl: Ctl<'_>) -> Option<CodeView> {
    ctl.current_code()
}

#[tauri::command]
pub fn new_code_pairing(ctl: Ctl<'_>) -> Result<CodeView, String> {
    ctl.new_code()
}

#[tauri::command]
pub fn get_pending_pairings(ctl: Ctl<'_>) -> Vec<PendingPairing> {
    ctl.pending_pairings()
}

#[tauri::command]
pub fn resolve_pairing(ctl: Ctl<'_>, request_id: u64, allow: bool) -> bool {
    ctl.resolve_pairing(request_id, allow)
}

#[tauri::command]
pub fn set_require_approval(ctl: Ctl<'_>, on: bool) -> Result<bool, String> {
    ctl.set_require_approval(on)
}

#[tauri::command]
pub fn ring_phone(ctl: Ctl<'_>, device_id: String) -> Result<usize, String> {
    ctl.ring_phone(&device_id)
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

/// "Stay reachable after restart": install / remove the sign-in service (one UAC prompt).
#[tauri::command]
pub async fn set_reach(ctl: Ctl<'_>, on: bool) -> Result<bool, String> {
    let data_dir = ctl.data_dir.clone();
    tauri::async_runtime::spawn_blocking(move || crate::reach::set_enabled(&data_dir, on))
        .await
        .map_err(|e| e.to_string())?
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

#[tauri::command]
pub fn get_update(updates: State<'_, Updates>) -> UpdateView {
    updates.view()
}

#[tauri::command]
pub async fn check_updates(app: tauri::AppHandle) -> Result<UpdateView, String> {
    Ok(crate::updater::check_now(&app).await)
}

/// Download + verify the installer found by the last check, run it and quit.
#[tauri::command]
pub async fn install_update(app: tauri::AppHandle) -> Result<(), String> {
    crate::updater::install(&app).await
}

#[tauri::command]
pub fn open_data_dir(ctl: Ctl<'_>) -> Result<(), String> {
    helm_win_apps::open_target(&ctl.data_dir.to_string_lossy()).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn quit_app(app: tauri::AppHandle) {
    app.exit(0);
}
