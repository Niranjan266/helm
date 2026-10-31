//! Well-known user folders.

use std::path::PathBuf;

#[cfg(windows)]
fn known_folder(id: &windows::core::GUID) -> Option<PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{KF_FLAG_DEFAULT, SHGetKnownFolderPath};
    // SAFETY: the returned PWSTR is owned by us and freed with CoTaskMemFree.
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const core::ffi::c_void));
        s.filter(|s| !s.is_empty()).map(PathBuf::from)
    }
}

/// The user's profile folder (`C:\Users\<name>`).
pub fn profile_dir() -> PathBuf {
    #[cfg(windows)]
    if let Some(p) = known_folder(&windows::Win32::UI::Shell::FOLDERID_Profile) {
        return p;
    }
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Where uploads from the phone go: `<Downloads>\Helm`.
pub fn helm_downloads_dir() -> PathBuf {
    #[cfg(windows)]
    if let Some(p) = known_folder(&windows::Win32::UI::Shell::FOLDERID_Downloads) {
        return p.join("Helm");
    }
    profile_dir().join("Downloads").join("Helm")
}

/// Where recordings go: `<Videos>\Helm`.
pub fn helm_videos_dir() -> PathBuf {
    #[cfg(windows)]
    if let Some(p) = known_folder(&windows::Win32::UI::Shell::FOLDERID_Videos) {
        return p.join("Helm");
    }
    profile_dir().join("Videos").join("Helm")
}

/// The user's roaming application data folder (`%APPDATA%`), from the environment or, when
/// a launcher (e.g. a service) did not pass one, from the shell.
pub fn roaming_app_data() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("APPDATA").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    #[cfg(windows)]
    if let Some(p) = known_folder(&windows::Win32::UI::Shell::FOLDERID_RoamingAppData) {
        return Some(p);
    }
    std::env::var_os("USERPROFILE")
        .filter(|p| !p.is_empty())
        .map(|p| PathBuf::from(p).join("AppData").join("Roaming"))
}
