//! In-app updates: check GitHub Releases at startup and daily, and on "Install" download the NSIS
//! installer through `helm-update` (HTTPS, size, SHA-256, Ed25519), run it silently and let it
//! start Helm again hidden. Phones reconnect by themselves, as after any PC restart.
//!
//! Only a file `helm-update` has verified is ever run. Builds without a release key refuse.
//!
//! A portable copy (no `uninstall.exe` next to it, e.g. `Helm-windows-x64.exe` from
//! `scripts/install.ps1`) updates through the same installer: it leaves a marker, and the freshly
//! installed Helm copies itself over the old file and takes over Start with Windows.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use helm_agent::update::{self, UpdateStatus, WINDOWS_SETUP_ASSET};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};

use crate::controller::{CHANGED_EVENT, Controller, NAVIGATE_EVENT};

pub const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
const FIRST_CHECK_AFTER: Duration = Duration::from_secs(20);
const TICK: Duration = Duration::from_secs(60 * 60);
const PORTABLE_MARKER: &str = "update-portable.json";

/// Update state shared by the window, the tray and the daily check.
#[derive(Default)]
pub struct Updates {
    status: Mutex<Option<UpdateStatus>>,
    last_error: Mutex<Option<String>>,
    last_check: Mutex<Option<SystemTime>>,
    checking: AtomicBool,
    installing: AtomicBool,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct UpdateView {
    /// "unknown" | "checking" | "disabled" | "uptodate" | "available" | "failed" | "installing"
    pub state: &'static str,
    pub summary: String,
    /// The version on offer, when one is.
    pub version: Option<String>,
    pub current: String,
}

impl Updates {
    pub fn view(&self) -> UpdateView {
        let status = lock(&self.status).clone();
        let error = lock(&self.last_error).clone();
        view_of(
            status.as_ref(),
            error.as_deref(),
            self.checking.load(Ordering::Relaxed),
            self.installing.load(Ordering::Relaxed),
        )
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn view_of(
    status: Option<&UpdateStatus>,
    install_error: Option<&str>,
    checking: bool,
    installing: bool,
) -> UpdateView {
    let version = match status {
        Some(UpdateStatus::Available(c)) => Some(c.version.to_string()),
        _ => None,
    };
    let state = match status {
        _ if installing => "installing",
        _ if checking => "checking",
        None => "unknown",
        Some(UpdateStatus::Disabled) => "disabled",
        Some(UpdateStatus::UpToDate) => "uptodate",
        Some(UpdateStatus::Available(_)) => "available",
        Some(UpdateStatus::Failed(_)) => "failed",
    };
    let summary = match (state, install_error, status) {
        ("installing", ..) => format!(
            "Downloading and verifying Helm {}. Helm restarts by itself; phones reconnect.",
            version.as_deref().unwrap_or("")
        ),
        ("checking", ..) => "Checking for updates…".to_string(),
        (_, Some(e), _) => format!("Update not installed: {e}"),
        (_, None, Some(s)) => s.summary(),
        (_, None, None) => format!("You have Helm {}.", env!("CARGO_PKG_VERSION")),
    };
    UpdateView {
        state,
        summary,
        version,
        current: env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// A failed check (offline, GitHub down) does not take back an update already found.
fn merge(previous: Option<UpdateStatus>, new: UpdateStatus) -> UpdateStatus {
    match (previous, new) {
        (Some(prev @ UpdateStatus::Available(_)), UpdateStatus::Failed(_)) => prev,
        (_, new) => new,
    }
}

fn check_due(last: Option<SystemTime>, now: SystemTime) -> bool {
    match last {
        None => true,
        // A clock moved backwards counts as due.
        Some(t) => now.duration_since(t).map_or(true, |d| d >= CHECK_EVERY),
    }
}

/// Text of the tray item for a version on offer.
pub fn tray_label(version: &str) -> String {
    format!("Update available v{version} → Install")
}

fn publish(app: &AppHandle) {
    let _ = app.emit(CHANGED_EVENT, json!({ "kind": "update" }));
    let Some(updates) = app.try_state::<Updates>() else {
        return;
    };
    let view = updates.view();
    crate::tray::set_update(app, &view);
}

/// Check GitHub Releases now (never while installing).
pub async fn check_now(app: &AppHandle) -> UpdateView {
    let updates = app.state::<Updates>();
    if updates.installing.load(Ordering::Relaxed) || updates.checking.swap(true, Ordering::Relaxed)
    {
        return updates.view();
    }
    publish(app);
    let new = tauri::async_runtime::spawn_blocking(update::check)
        .await
        .unwrap_or_else(|e| UpdateStatus::Failed(e.to_string()));
    if let UpdateStatus::Failed(e) = &new {
        tracing::info!("update check failed: {e}");
    }
    {
        let mut status = lock(&updates.status);
        *status = Some(merge(status.take(), new));
    }
    *lock(&updates.last_error) = None;
    *lock(&updates.last_check) = Some(SystemTime::now());
    updates.checking.store(false, Ordering::Relaxed);
    publish(app);
    updates.view()
}

/// Download, verify and start the installer of the update on offer, then quit. On success this
/// does not return to a running app; on failure nothing was run and Helm keeps working.
pub async fn install(app: &AppHandle) -> Result<(), String> {
    let updates = app.state::<Updates>();
    if updates.installing.swap(true, Ordering::Relaxed) {
        return Err("The update is already being installed.".into());
    }
    *lock(&updates.last_error) = None;
    publish(app);
    let result = download_and_start(app).await;
    if let Err(e) = &result {
        tracing::warn!("update not installed: {e}");
        *lock(&updates.last_error) = Some(e.clone());
        updates.installing.store(false, Ordering::Relaxed);
        publish(app);
    }
    result
}

async fn download_and_start(app: &AppHandle) -> Result<(), String> {
    let updates = app.state::<Updates>();
    let candidate = match lock(&updates.status).as_ref() {
        Some(UpdateStatus::Available(c)) => c.clone(),
        Some(UpdateStatus::Disabled) => return Err(UpdateStatus::Disabled.summary()),
        _ => return Err("Check for updates first.".into()),
    };
    // Only the NSIS installer takes the silent/restart arguments below.
    if candidate.package.name != WINDOWS_SETUP_ASSET {
        return Err(format!(
            "this release has no {WINDOWS_SETUP_ASSET}; get it from the releases page."
        ));
    }
    let setup = tauri::async_runtime::spawn_blocking(move || update::download(&candidate))
        .await
        .map_err(|e| e.to_string())??;
    tracing::info!("verified installer at {}", setup.display());

    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let data_dir = app.state::<std::sync::Arc<Controller>>().data_dir.clone();
    let portable = is_portable(&exe);
    if portable {
        write_marker(&data_dir, &exe).map_err(|e| e.to_string())?;
    }
    if let Err(e) = spawn_installer(&setup, &exe) {
        if portable {
            let _ = std::fs::remove_file(data_dir.join(PORTABLE_MARKER));
        }
        return Err(format!("could not start the installer: {e}"));
    }
    tracing::info!("installer started; quitting so it can replace Helm");
    app.exit(0);
    Ok(())
}

/// Install from the tray: no window is shown unless something goes wrong.
pub fn install_from_tray(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if install(&app).await.is_err() {
            crate::tray::show_main(&app);
            let _ = app.emit(NAVIGATE_EVENT, "settings");
        }
    });
}

/// Startup work: finish a portable self-replace, then check now and at most daily.
pub fn start_background(app: AppHandle) {
    let data_dir = app.state::<std::sync::Arc<Controller>>().data_dir.clone();
    tauri::async_runtime::spawn(async move {
        let _ = tauri::async_runtime::spawn_blocking(move || finish_portable(&data_dir)).await;
        tokio::time::sleep(FIRST_CHECK_AFTER).await;
        // Installers verified earlier have done their work.
        let _ = std::fs::remove_dir_all(update::config().download_dir);
        loop {
            let last = *lock(&app.state::<Updates>().last_check);
            if check_due(last, SystemTime::now()) {
                check_now(&app).await;
            }
            tokio::time::sleep(TICK).await;
        }
    });
}

// ------------------------------------------------------------------ running the installer

/// The command line given to `cmd.exe`: wait for this Helm to exit, install silently and start
/// Helm hidden (`/R /ARGS`, Tauri's NSIS template). If the installer fails, start the old Helm
/// again so the PC does not silently drop off the phones. Paths travel in environment variables,
/// which cmd does not re-parse, so spaces and `&` in them are safe.
const INSTALL_SCRIPT: &str = "/d /c \"ping -n 3 127.0.0.1 >nul & \
     start \"\" /wait \"%HELM_UPDATE_SETUP%\" /S /R /ARGS --hidden & \
     if errorlevel 1 start \"\" \"%HELM_UPDATE_RELAUNCH%\" --hidden\"";

#[cfg(windows)]
fn spawn_installer(setup: &Path, relaunch: &Path) -> io::Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    // A full path, so nothing next to a portable Helm can stand in for cmd.exe.
    let system32 =
        PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()))
            .join("System32");
    std::process::Command::new(system32.join("cmd.exe"))
        .raw_arg(INSTALL_SCRIPT)
        .env("HELM_UPDATE_SETUP", setup)
        .env("HELM_UPDATE_RELAUNCH", relaunch)
        .current_dir(&system32)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(drop)
}

#[cfg(not(windows))]
fn spawn_installer(_setup: &Path, _relaunch: &Path) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "Windows only"))
}

// ------------------------------------------------------------------ portable copies

/// The NSIS installer leaves `uninstall.exe` next to `Helm.exe`; a portable copy has none.
fn is_portable(exe: &Path) -> bool {
    exe.parent()
        .is_some_and(|dir| !dir.join("uninstall.exe").exists())
}

#[derive(Debug, Serialize, Deserialize)]
struct PortableMarker {
    target: PathBuf,
}

fn write_marker(data_dir: &Path, exe: &Path) -> io::Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let text = serde_json::to_string(&PortableMarker {
        target: exe.to_path_buf(),
    })?;
    helm_agent::util::atomic_write(&data_dir.join(PORTABLE_MARKER), text.as_bytes())
}

/// The marker only ever names an existing Helm program file.
fn valid_target(target: &Path) -> bool {
    let name_ok = target
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|n| n.starts_with("helm") && n.ends_with(".exe"));
    name_ok && target.is_absolute() && target.is_file()
}

/// Copy `source` over `target` through a temporary file next to it.
fn replace_file(source: &Path, target: &Path) -> io::Result<()> {
    let mut tmp = target.as_os_str().to_owned();
    tmp.push(".new");
    let tmp = PathBuf::from(tmp);
    std::fs::copy(source, &tmp)?;
    std::fs::rename(&tmp, target).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// In the freshly installed Helm: replace the portable file the update started from.
fn finish_portable(data_dir: &Path) {
    let marker = data_dir.join(PORTABLE_MARKER);
    let Ok(text) = std::fs::read_to_string(&marker) else {
        return;
    };
    let _ = std::fs::remove_file(&marker);
    let Ok(PortableMarker { target }) = serde_json::from_str(&text) else {
        return;
    };
    let Ok(me) = std::env::current_exe() else {
        return;
    };
    // The same file: the installer failed and the old copy was restarted.
    let same = std::fs::canonicalize(&me).ok() == std::fs::canonicalize(&target).ok();
    if same || is_portable(&me) || !valid_target(&target) {
        return;
    }
    if crate::autostart::starts(&target)
        && let Err(e) = crate::autostart::set_enabled(true)
    {
        tracing::warn!(
            "could not move Start with Windows to {}: {e:#}",
            me.display()
        );
    }
    for _ in 0..30 {
        match replace_file(&me, &target) {
            Ok(()) => {
                tracing::info!("replaced portable copy {}", target.display());
                return;
            }
            // The old process may still be on its way out.
            Err(_) => std::thread::sleep(Duration::from_secs(1)),
        }
    }
    tracing::warn!("could not replace portable copy {}", target.display());
}

#[cfg(test)]
mod tests {
    use super::*;
    use helm_agent::update::UpdateCandidate;

    fn available(version: &str) -> UpdateStatus {
        let asset = |name: &str| helm_update_asset(name);
        UpdateStatus::Available(Box::new(UpdateCandidate {
            version: helm_agent::update::Version::parse(version).unwrap(),
            tag: format!("v{version}"),
            notes: None,
            page_url: None,
            package: asset(WINDOWS_SETUP_ASSET),
            signature: asset("x.sig"),
            checksum: asset("x.sha256"),
        }))
    }

    fn helm_update_asset(name: &str) -> helm_agent::update::Asset {
        helm_agent::update::Asset {
            name: name.into(),
            browser_download_url: format!("https://example.invalid/{name}"),
            size: 1,
        }
    }

    #[test]
    fn a_failed_check_keeps_a_found_update() {
        let found = available("9.0.0");
        assert_eq!(
            merge(Some(found.clone()), UpdateStatus::Failed("offline".into())),
            found
        );
        assert_eq!(
            merge(Some(found), UpdateStatus::UpToDate),
            UpdateStatus::UpToDate
        );
        assert_eq!(
            merge(None, UpdateStatus::Failed("x".into())),
            UpdateStatus::Failed("x".into())
        );
    }

    #[test]
    fn checks_at_most_daily() {
        let now = SystemTime::now();
        assert!(check_due(None, now));
        assert!(!check_due(Some(now - Duration::from_secs(3600)), now));
        assert!(check_due(Some(now - CHECK_EVERY), now));
        assert!(check_due(Some(now + Duration::from_secs(60)), now));
    }

    #[test]
    fn views_and_labels() {
        let v = view_of(Some(&available("9.1.0")), None, false, false);
        assert_eq!(v.state, "available");
        assert_eq!(v.version.as_deref(), Some("9.1.0"));
        assert_eq!(tray_label("9.1.0"), "Update available v9.1.0 → Install");

        let v = view_of(Some(&UpdateStatus::Disabled), None, false, false);
        assert_eq!(v.state, "disabled");
        assert!(v.version.is_none());
        assert!(v.summary.contains("disabled"));

        let v = view_of(
            Some(&available("9.1.0")),
            Some("SHA-256 mismatch"),
            false,
            false,
        );
        assert_eq!(v.state, "available");
        assert!(v.summary.contains("not installed") && v.summary.contains("SHA-256"));

        assert_eq!(view_of(None, None, false, true).state, "installing");
        assert_eq!(view_of(None, None, true, false).state, "checking");
        assert_eq!(view_of(None, None, false, false).state, "unknown");
    }

    #[test]
    fn install_script_is_silent_restarts_hidden_and_has_a_fallback() {
        assert!(INSTALL_SCRIPT.contains("/wait \"%HELM_UPDATE_SETUP%\" /S /R /ARGS --hidden"));
        // Not `||`: it does not see the exit code `start /wait` passes on (tried with stand-ins).
        assert!(
            INSTALL_SCRIPT
                .contains("if errorlevel 1 start \"\" \"%HELM_UPDATE_RELAUNCH%\" --hidden")
        );
    }

    #[test]
    fn portable_is_told_apart_by_the_uninstaller() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("Helm.exe");
        assert!(is_portable(&exe));
        std::fs::write(dir.path().join("uninstall.exe"), b"").unwrap();
        assert!(!is_portable(&exe));
    }

    #[test]
    fn marker_targets_are_restricted() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("Helm-windows-x64.exe");
        let other = dir.path().join("notes.exe");
        let not_exe = dir.path().join("helm.txt");
        for f in [&good, &other, &not_exe] {
            std::fs::write(f, b"x").unwrap();
        }
        assert!(valid_target(&good));
        assert!(!valid_target(&other));
        assert!(!valid_target(&not_exe));
        assert!(!valid_target(&dir.path().join("Helm-missing.exe")));
        assert!(!valid_target(Path::new("Helm.exe")));
    }

    #[test]
    fn marker_round_trip_and_replace() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("Helm-windows-x64.exe");
        let source = dir.path().join("new.exe");
        std::fs::write(&target, b"old").unwrap();
        std::fs::write(&source, b"new").unwrap();

        write_marker(dir.path(), &target).unwrap();
        let text = std::fs::read_to_string(dir.path().join(PORTABLE_MARKER)).unwrap();
        let m: PortableMarker = serde_json::from_str(&text).unwrap();
        assert_eq!(m.target, target);

        replace_file(&source, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert!(!dir.path().join("Helm-windows-x64.exe.new").exists());
    }
}
