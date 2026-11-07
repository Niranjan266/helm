//! "Stay reachable after restart": the `HelmSignIn` Windows service.
//!
//! After Windows restarts (an update, a power cut) nobody is signed in, so `Helm.exe` (started
//! from the user's Run key) is not running and the phone cannot reach the PC. With this on, a
//! copy of `Helm.exe` runs as a LocalSystem service *only while the owner has no Windows
//! session*: it serves the same identity and paired phones on the same port, LAN and relay,
//! with screen-less methods only (`helm_agent::modules::PRE_LOGIN_METHODS`: ping, info,
//! lock / sleep / restart / shut down). When the owner signs in it stops at once and the
//! desktop app takes port 47820 back (it waits for the port; see [`wait_for_port`]).
//!
//! Pieces, all in this one executable:
//! * the toggle (unelevated): seals the identity key for this computer into
//!   `service-identity.json` in the user's data folder, then runs `Helm.exe --signin-install`
//!   elevated (one UAC prompt); turning it off runs `--signin-remove`.
//! * `--signin-install` / `--signin-remove` / `--signin-refresh` (elevated): copy `Helm.exe`
//!   to `%ProgramFiles%\Helm\SignIn` (admin-only, so the SYSTEM service never runs a
//!   user-writable binary) and register / remove / update the service.
//! * `--signin-run` (the SCM starts this as SYSTEM): copies identity, devices and relay
//!   settings out of the owner's data folder into SYSTEM's own profile (so SYSTEM never
//!   writes into a user-controlled folder), and runs the agent from there.

use std::path::{Path, PathBuf};

use helm_agent::identity::{IDENTITY_FILE, Identity, SERVICE_IDENTITY_FILE};

/// SCM entry point (what the service's command line starts).
pub const RUN_FLAG: &str = "--signin-run";
/// Elevated helper: register the service for `--data-dir <dir> --owner <sid>`.
pub const INSTALL_FLAG: &str = "--signin-install";
/// Elevated helper: remove the service and its files.
pub const REMOVE_FLAG: &str = "--signin-remove";
/// Elevated helper: put this (newer) Helm.exe in place of the service's copy.
pub const REFRESH_FLAG: &str = "--signin-refresh";

/// Settings files copied from the owner's folder for the service (besides the identity).
const COPIED: &[&str] = &[
    helm_agent::devices::DEVICES_FILE,
    helm_agent::relay::RELAY_FILE,
];
/// Nothing we copy is anywhere near this big.
const MAX_COPY: u64 = 4 * 1024 * 1024;

/// Copy what the pre-login agent needs from `user` (the owner's data folder, read only)
/// into `private` (SYSTEM's own folder). Links are refused; only JSON is copied.
pub fn snapshot(user: &Path, private: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(private)?;
    let read = |name: &str| -> anyhow::Result<Option<Vec<u8>>> {
        let path = user.join(name);
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            return Ok(None);
        };
        anyhow::ensure!(
            meta.is_file() && meta.len() <= MAX_COPY,
            "{} is not a plain file",
            path.display()
        );
        let bytes = std::fs::read(&path)?;
        serde_json::from_slice::<serde_json::Value>(&bytes)
            .map_err(|e| anyhow::anyhow!("{} is not JSON: {e}", path.display()))?;
        Ok(Some(bytes))
    };
    let identity = read(SERVICE_IDENTITY_FILE)?.ok_or_else(|| {
        anyhow::anyhow!("{SERVICE_IDENTITY_FILE} is missing: turn the setting off and on again")
    })?;
    helm_agent::util::atomic_write(&private.join(IDENTITY_FILE), &identity)?;
    for name in COPIED {
        let dest = private.join(name);
        match read(name) {
            Ok(Some(bytes)) => helm_agent::util::atomic_write(&dest, &bytes)?,
            // Gone or unusable in the owner's folder: don't serve a stale copy.
            Ok(None) | Err(_) => {
                let _ = std::fs::remove_file(&dest);
            }
        }
    }
    Ok(())
}

/// Write `service-identity.json` (the key sealed for this computer) next to identity.json.
pub fn export_identity(data_dir: &Path) -> anyhow::Result<()> {
    let id = Identity::load_existing(data_dir, helm_agent::identity::computer_name())?;
    id.save_for_service(&data_dir.join(SERVICE_IDENTITY_FILE))
}

pub fn forget_identity(data_dir: &Path) {
    let _ = std::fs::remove_file(data_dir.join(SERVICE_IDENTITY_FILE));
}

/// Wait (up to `limit`) until `port` can be bound, so the desktop app gets the port the
/// phones know rather than a random one while the service is still letting go of it.
pub async fn wait_for_port(port: u16, limit: std::time::Duration) {
    if port == 0 {
        return;
    }
    let deadline = tokio::time::Instant::now() + limit;
    while std::net::TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, port)).is_err()
        && tokio::time::Instant::now() < deadline
    {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

/// Program-name-independent paths of the service's files.
pub struct Paths {
    /// `%ProgramFiles%\Helm\SignIn` (admin-only).
    pub install_dir: PathBuf,
    /// SYSTEM's `%APPDATA%\Helm SignIn` (SYSTEM and administrators only).
    pub private_dir: PathBuf,
}

impl Paths {
    pub fn exe(&self) -> PathBuf {
        self.install_dir.join("Helm.exe")
    }
}

#[cfg(windows)]
pub use imp::*;

#[cfg(windows)]
mod imp {
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use helm_service::prelogin::RunArgs;
    use helm_service::win::prelogin as svc;

    use super::*;

    fn known_folder(id: &windows::core::GUID) -> Option<PathBuf> {
        use windows::Win32::System::Com::CoTaskMemFree;
        use windows::Win32::UI::Shell::{KF_FLAG_DEFAULT, SHGetKnownFolderPath};
        // SAFETY: the returned PWSTR is ours and freed with CoTaskMemFree.
        unsafe {
            let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
            let s = p.to_string().ok();
            CoTaskMemFree(Some(p.0 as *const core::ffi::c_void));
            s.filter(|s| !s.is_empty()).map(PathBuf::from)
        }
    }

    /// From the registry-backed known folders, never from environment variables (which the
    /// user can override for the elevated helper).
    pub fn paths() -> Result<Paths, String> {
        use windows::Win32::UI::Shell::{FOLDERID_ProgramFiles, FOLDERID_System};
        let pf = known_folder(&FOLDERID_ProgramFiles).ok_or("no Program Files folder")?;
        let sys = known_folder(&FOLDERID_System).ok_or("no System32 folder")?;
        Ok(Paths {
            install_dir: pf.join("Helm").join("SignIn"),
            private_dir: sys
                .join("config")
                .join("systemprofile")
                .join("AppData")
                .join("Roaming")
                .join("Helm SignIn"),
        })
    }

    // ------------------------------------------------------------ toggle (unelevated)

    pub fn is_enabled() -> bool {
        svc::is_installed()
    }

    /// Turn it on or off: one UAC prompt. Returns the new state.
    pub fn set_enabled(data_dir: &Path, on: bool) -> Result<bool, String> {
        if on {
            let owner = helm_service::win::sessions::current_user_sid()?;
            export_identity(data_dir).map_err(|e| format!("{e:#}"))?;
            let args = RunArgs {
                data_dir: std::path::absolute(data_dir).map_err(|e| e.to_string())?,
                owner_sid: owner,
            };
            let mut argv = vec![INSTALL_FLAG.to_string()];
            argv.extend(args.to_args());
            if let Err(e) = run_elevated(&argv) {
                forget_identity(data_dir);
                return Err(e);
            }
        } else {
            run_elevated(&[REMOVE_FLAG.to_string()])?;
            forget_identity(data_dir);
        }
        Ok(is_enabled())
    }

    /// Run this exe with `args` through UAC and wait for it.
    fn run_elevated(args: &[String]) -> Result<(), String> {
        use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED};
        use windows::Win32::System::Threading::{
            GetExitCodeProcess, INFINITE, WaitForSingleObject,
        };
        use windows::Win32::UI::Shell::{
            SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
        };
        use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
        use windows::core::{HSTRING, PCWSTR, w};

        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut params = String::new();
        for a in args {
            if !params.is_empty() {
                params.push(' ');
            }
            helm_service::cmdline::push_arg(&mut params, a);
        }
        let file = HSTRING::from(exe.as_os_str());
        let params = HSTRING::from(params);
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
            lpVerb: w!("runas"),
            lpFile: PCWSTR(file.as_ptr()),
            lpParameters: PCWSTR(params.as_ptr()),
            nShow: SW_HIDE.0,
            ..Default::default()
        };
        // SAFETY: all strings outlive the call; hProcess is closed below.
        if let Err(e) = unsafe { ShellExecuteExW(&mut info) } {
            return Err(if e.code() == ERROR_CANCELLED.to_hresult() {
                "Windows did not get permission (the prompt was cancelled).".into()
            } else {
                format!("Could not ask Windows for permission: {e}")
            });
        }
        let mut code = 1u32;
        // SAFETY: valid process handle from ShellExecuteExW.
        unsafe {
            WaitForSingleObject(info.hProcess, INFINITE);
            let _ = GetExitCodeProcess(info.hProcess, &mut code);
            let _ = CloseHandle(info.hProcess);
        }
        match code {
            0 => Ok(()),
            _ => Err(format!(
                "Setting up the sign-in service failed (code {code}). Details are in {}.",
                paths()
                    .map(|p| p.install_dir.join("setup.log").display().to_string())
                    .unwrap_or_default()
            )),
        }
    }

    // ------------------------------------------------------------ command line

    /// Handle the service / helper flags. `Some(exit code)` = handled, exit now.
    pub fn handle_cli(argv: &[String]) -> Option<i32> {
        let (flag, rest) = argv.get(1..)?.split_first()?;
        let result = match flag.as_str() {
            RUN_FLAG => return Some(run_service(rest)),
            INSTALL_FLAG => install(rest),
            REMOVE_FLAG => remove(),
            REFRESH_FLAG => refresh(),
            _ => return None,
        };
        Some(match result {
            Ok(()) => 0,
            Err(e) => {
                setup_log(&format!("{flag}: {e}"));
                3
            }
        })
    }

    /// The helpers have no window; failures go to `<install dir>\setup.log`.
    fn setup_log(msg: &str) {
        use std::io::Write;
        if let Ok(p) = paths()
            && std::fs::create_dir_all(&p.install_dir).is_ok()
            && let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p.install_dir.join("setup.log"))
        {
            let _ = writeln!(f, "{msg}");
        }
    }

    fn copy_self(p: &Paths) -> Result<(), String> {
        let me = std::env::current_exe().map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&p.install_dir).map_err(|e| e.to_string())?;
        if same_file(&me, &p.exe()) {
            return Ok(());
        }
        // The old copy may take a moment to be released after the service stopped.
        let mut last = String::new();
        for _ in 0..20 {
            match std::fs::copy(&me, p.exe()) {
                Ok(_) => return Ok(()),
                Err(e) => last = e.to_string(),
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        Err(format!("copying Helm.exe: {last}"))
    }

    fn same_file(a: &Path, b: &Path) -> bool {
        match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
    }

    fn install(rest: &[String]) -> Result<(), String> {
        let args = RunArgs::parse(rest)?;
        if !args.data_dir.join(SERVICE_IDENTITY_FILE).is_file() {
            return Err(format!(
                "{} has no {SERVICE_IDENTITY_FILE}",
                args.data_dir.display()
            ));
        }
        let p = paths()?;
        svc::uninstall()?; // releases the old copy
        copy_self(&p)?;
        svc::install(&p.exe(), RUN_FLAG, &args)
    }

    fn remove() -> Result<(), String> {
        let p = paths()?;
        svc::uninstall()?;
        let _ = std::fs::remove_dir_all(&p.private_dir);
        for _ in 0..20 {
            if std::fs::remove_file(p.exe()).is_ok() || !p.exe().exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        let _ = std::fs::remove_file(p.install_dir.join("setup.log"));
        let _ = std::fs::remove_dir(&p.install_dir);
        Ok(())
    }

    fn refresh() -> Result<(), String> {
        let p = paths()?;
        svc::restart_with(|| copy_self(&p))
    }

    // ------------------------------------------------------------ the service itself

    /// The pre-login agent: `helm_agent` in-process, from SYSTEM's private copy.
    struct PreLoginAgent {
        args: RunArgs,
        private: PathBuf,
        rt: tokio::runtime::Runtime,
        server: Option<helm_agent::ServerHandle>,
    }

    impl svc::Agent for PreLoginAgent {
        fn start(&mut self) -> Result<(), String> {
            if self.server.is_some() {
                return Ok(());
            }
            snapshot(&self.args.data_dir, &self.private).map_err(|e| format!("{e:#}"))?;
            let mut config = helm_agent::Config::new(&self.private);
            config.port = crate::settings::Settings::load(&self.args.data_dir).port;
            config.enable_mdns = true;
            config.pre_login = true;
            let server = self
                .rt
                .block_on(Box::pin(helm_agent::start(config)))
                .map_err(|e| format!("{e:#}"))?;
            tracing::info!("pre-login agent on port {}", server.port());
            self.server = Some(server);
            Ok(())
        }

        fn stop(&mut self) {
            if let Some(s) = self.server.take() {
                self.rt.block_on(Box::pin(s.shutdown()));
                tracing::info!("pre-login agent stopped");
            }
        }

        fn alive(&mut self) -> bool {
            self.server.is_some()
        }
    }

    fn run_service(rest: &[String]) -> i32 {
        let Ok(args) = RunArgs::parse(rest) else {
            return 2;
        };
        let Ok(p) = paths() else {
            return 3;
        };
        let private = p.private_dir;
        if std::fs::create_dir_all(&private).is_ok()
            && let Ok((file, _)) = helm_agent::desktop::open_log(&private)
        {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(tracing_subscriber::EnvFilter::new("info"))
                .with_target(false)
                .with_ansi(false)
                .with_writer(std::sync::Mutex::new(file))
                .try_init();
        }
        let make: svc::Factory = Box::new(move |args: &RunArgs| -> Box<dyn svc::Agent> {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            Box::new(PreLoginAgent {
                args: args.clone(),
                private,
                rt,
                server: None,
            })
        });
        let log = Box::new(|m: &str| tracing::info!("{m}"));
        match svc::run_dispatcher(args, make, log) {
            Ok(()) => 0,
            Err(e) => {
                tracing::error!("{e} ({RUN_FLAG} is started by the Service Control Manager)");
                1
            }
        }
    }
}

#[cfg(not(windows))]
mod imp_none {
    use std::path::Path;

    pub fn is_enabled() -> bool {
        false
    }
    pub fn set_enabled(_data_dir: &Path, _on: bool) -> Result<bool, String> {
        Err("Only available on Windows".into())
    }
    pub fn handle_cli(_argv: &[String]) -> Option<i32> {
        None
    }
}
#[cfg(not(windows))]
pub use imp_none::*;

#[cfg(test)]
mod tests {
    use super::*;

    fn identity_json() -> &'static str {
        r#"{"version":1,"host_id":"x","cert_der":"","key":{"protection":"none","data":""}}"#
    }

    #[test]
    fn snapshot_copies_identity_devices_and_relay_only() {
        let user = tempfile::tempdir().unwrap();
        let private = tempfile::tempdir().unwrap();
        std::fs::write(user.path().join(SERVICE_IDENTITY_FILE), identity_json()).unwrap();
        std::fs::write(user.path().join(IDENTITY_FILE), "user-scope, not copied").unwrap();
        std::fs::write(user.path().join("devices.json"), r#"{"devices":[]}"#).unwrap();
        std::fs::write(user.path().join("relay.json"), r#"{"enabled":true}"#).unwrap();
        std::fs::write(user.path().join("audit.log"), "not copied").unwrap();
        snapshot(user.path(), private.path()).unwrap();
        assert_eq!(
            std::fs::read_to_string(private.path().join(IDENTITY_FILE)).unwrap(),
            identity_json()
        );
        assert!(private.path().join("devices.json").exists());
        assert!(private.path().join("relay.json").exists());
        assert!(!private.path().join("audit.log").exists());

        // Removed (or broken) in the owner's folder -> removed from the copy too.
        std::fs::remove_file(user.path().join("relay.json")).unwrap();
        std::fs::write(user.path().join("devices.json"), "garbage").unwrap();
        snapshot(user.path(), private.path()).unwrap();
        assert!(!private.path().join("relay.json").exists());
        assert!(!private.path().join("devices.json").exists());
    }

    #[test]
    fn snapshot_needs_the_service_identity() {
        let user = tempfile::tempdir().unwrap();
        let private = tempfile::tempdir().unwrap();
        assert!(snapshot(user.path(), private.path()).is_err());
        std::fs::write(user.path().join(SERVICE_IDENTITY_FILE), "not json").unwrap();
        assert!(snapshot(user.path(), private.path()).is_err());
        assert!(!private.path().join(IDENTITY_FILE).exists());
    }

    #[test]
    fn export_round_trips_through_the_snapshot() {
        let user = tempfile::tempdir().unwrap();
        let private = tempfile::tempdir().unwrap();
        let mine = Identity::load_or_create(user.path(), "PC".into()).unwrap();
        export_identity(user.path()).unwrap();
        snapshot(user.path(), private.path()).unwrap();
        let theirs = Identity::load_existing(private.path(), "PC".into()).unwrap();
        assert_eq!(mine.fingerprint, theirs.fingerprint);
        forget_identity(user.path());
        assert!(!user.path().join(SERVICE_IDENTITY_FILE).exists());
    }

    #[tokio::test]
    async fn wait_for_port_returns_for_a_free_or_any_port() {
        wait_for_port(0, std::time::Duration::from_secs(5)).await;
        let held = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
        let port = held.local_addr().unwrap().port();
        let t = std::time::Instant::now();
        wait_for_port(port, std::time::Duration::from_millis(600)).await;
        assert!(t.elapsed() >= std::time::Duration::from_millis(500));
    }

    #[test]
    fn flags_are_distinct_from_the_app_flags() {
        for f in [RUN_FLAG, INSTALL_FLAG, REMOVE_FLAG, REFRESH_FLAG] {
            assert!(f.starts_with("--signin-"));
            assert_ne!(f, crate::args::HIDDEN);
        }
    }
}
