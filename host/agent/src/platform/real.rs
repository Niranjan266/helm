//! The real platform: this PC, through the `helm-win-*` crates.

use std::path::Path;
use std::sync::mpsc as std_mpsc;
use std::thread::JoinHandle;

use helm_win_apps::IconCache;

use super::*;

#[derive(Debug)]
pub struct RealPlatform {
    icons: Option<IconCache>,
    extras: super::real5::Extras,
}

impl RealPlatform {
    /// `cache_dir` holds the on-disk app icon cache.
    pub fn new(cache_dir: &Path) -> RealPlatform {
        let icons = match IconCache::new(cache_dir) {
            Ok(c) => Some(c),
            Err(e) => {
                tracing::warn!("icon cache unavailable: {e}");
                None
            }
        };
        RealPlatform {
            icons,
            extras: Default::default(),
        }
    }
}

impl Platform for RealPlatform {
    fn list_apps(&self) -> PResult<Vec<AppEntry>> {
        helm_win_apps::list_apps().map_err(apps_err)
    }
    fn app_icon_png(&self, app_id: &str, size: u32) -> PResult<Vec<u8>> {
        match &self.icons {
            Some(cache) => cache.icon_png(app_id, size),
            None => helm_win_apps::app_icon_png(app_id, size),
        }
        .map_err(apps_err)
    }
    fn launch_app(&self, app_id: &str) -> PResult<()> {
        helm_win_apps::launch_app(app_id).map_err(apps_err)
    }
    fn open_target(&self, target: &str) -> PResult<()> {
        helm_win_apps::open_target(target).map_err(apps_err)
    }

    fn list_windows(&self) -> PResult<Vec<WindowInfo>> {
        helm_win_apps::list_windows().map_err(apps_err)
    }
    fn focus_window(&self, hwnd: u64) -> PResult<FocusResult> {
        helm_win_apps::focus_window(hwnd).map_err(apps_err)
    }
    fn window_command(&self, hwnd: u64, cmd: WindowCommand) -> PResult<()> {
        match cmd {
            WindowCommand::Minimize => helm_win_apps::minimize_window(hwnd),
            WindowCommand::Maximize => helm_win_apps::maximize_window(hwnd),
            WindowCommand::Restore => helm_win_apps::restore_window(hwnd),
            WindowCommand::Close => helm_win_apps::close_window(hwnd),
        }
        .map_err(apps_err)
    }

    fn media_state(&self) -> PResult<Option<MediaState>> {
        helm_win_system::media::media_state().map_err(system_err)
    }
    fn media_artwork_png(&self) -> PResult<Option<Vec<u8>>> {
        helm_win_system::media::media_artwork_png().map_err(system_err)
    }
    fn media_control(&self, action: MediaAction) -> PResult<()> {
        helm_win_system::media::media_control(action).map_err(system_err)
    }

    fn volume(&self) -> PResult<VolumeState> {
        helm_win_system::audio::volume().map_err(system_err)
    }
    fn set_volume(&self, level: f32) -> PResult<()> {
        helm_win_system::audio::set_volume(level).map_err(system_err)
    }
    fn set_muted(&self, muted: bool) -> PResult<()> {
        helm_win_system::audio::set_muted(muted).map_err(system_err)
    }

    fn brightness_list(&self) -> PResult<Vec<Display>> {
        helm_win_system::display::brightness_list().map_err(system_err)
    }
    fn set_brightness(&self, id: &str, level: u8) -> PResult<()> {
        helm_win_system::display::set_brightness(id, level).map_err(system_err)
    }

    fn clipboard_text(&self) -> PResult<Option<String>> {
        helm_win_input::clipboard::get_text().map_err(input_err)
    }
    fn clipboard_png(&self) -> PResult<Option<Vec<u8>>> {
        helm_win_input::clipboard::get_image_png().map_err(input_err)
    }
    fn set_clipboard_text(&self, text: &str) -> PResult<()> {
        helm_win_input::clipboard::set_text(text).map_err(input_err)
    }
    fn set_clipboard_png(&self, png: &[u8]) -> PResult<()> {
        helm_win_input::clipboard::set_image_png(png).map_err(|e| match e {
            helm_win_input::Error::Image(m) => RpcError::bad_params(format!("bad image: {m}")),
            other => input_err(other),
        })
    }

    fn new_injector(&self) -> PResult<Injector> {
        Injector::new().map_err(input_err)
    }

    fn network_adapters(&self) -> PResult<Vec<Adapter>> {
        helm_win_system::net::mac_addresses().map_err(system_err)
    }
    fn battery(&self) -> PResult<Option<Battery>> {
        helm_win_system::info::battery().map_err(system_err)
    }

    fn monitors(&self) -> PResult<Vec<Monitor>> {
        crate::screen::monitors()
    }
    fn screenshot_png(&self, monitor_id: Option<&str>) -> PResult<Vec<u8>> {
        crate::screen::screenshot_png(monitor_id)
    }

    fn start_video(
        &self,
        params: VideoParams,
        sink: AccessUnitSink,
    ) -> PResult<Box<dyn VideoStream>> {
        crate::screen::start_video(params, sink)
    }
    fn window_thumbnail_png(&self, hwnd: u64, max_side: u32) -> PResult<Option<Vec<u8>>> {
        crate::screen::window_thumbnail_png(hwnd, max_side)
    }
    fn start_audio(&self, sink: AudioSink) -> PResult<Box<dyn AudioStream>> {
        crate::screen::start_audio(sink)
    }
    fn cursor(&self) -> PResult<CursorState> {
        crate::cursor::cursor()
    }
    fn cursor_shape(&self, handle: u64) -> PResult<Option<CursorShape>> {
        crate::cursor::cursor_shape(handle)
    }

    fn start_watchers(&self, events: EventSink) -> WatchGuard {
        let (stop_tx, stop_rx) = std_mpsc::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("helm-watchers".into())
            .spawn(move || run_watchers(events, stop_rx));
        match thread {
            Ok(join) => Box::new(Watchers {
                stop: Some(stop_tx),
                join: Some(join),
            }),
            Err(e) => {
                tracing::warn!("could not start watchers: {e}");
                Box::new(())
            }
        }
    }

    fn new_stats_sampler(&self) -> Arc<dyn StatsSampler> {
        Arc::new(RealSampler(helm_win_sysmon::stats::Sampler::new()))
    }
    fn processes(&self) -> PResult<Vec<ProcessInfo>> {
        helm_win_sysmon::process::processes().map_err(sysmon_err)
    }
    fn kill_process(&self, pid: u32) -> PResult<()> {
        helm_win_sysmon::process::kill(pid).map_err(sysmon_err)
    }
    fn set_process_priority(&self, pid: u32, level: Priority) -> PResult<()> {
        helm_win_sysmon::process::set_priority(pid, level).map_err(sysmon_err)
    }
    fn audio_sessions(&self) -> PResult<Vec<AudioSession>> {
        helm_win_system::audio::list_sessions().map_err(system_err)
    }
    fn set_session_volume(&self, id: &str, level: f32) -> PResult<()> {
        helm_win_system::audio::set_session_volume(id, level).map_err(system_err)
    }
    fn set_session_muted(&self, id: &str, muted: bool) -> PResult<()> {
        helm_win_system::audio::set_session_muted(id, muted).map_err(system_err)
    }
    fn mic_muted(&self) -> PResult<Option<bool>> {
        mic_muted()
    }
    fn move_window_to_monitor(&self, hwnd: u64, monitor_index: usize) -> PResult<()> {
        helm_win_sysmon::window::move_to_monitor(hwnd, monitor_index).map_err(sysmon_err)
    }
    fn snap_window(&self, hwnd: u64, pos: Snap) -> PResult<()> {
        helm_win_sysmon::window::snap(hwnd, pos).map_err(sysmon_err)
    }
    fn virtual_desktop(&self, action: VirtualDesktop) -> PResult<()> {
        helm_win_sysmon::window::virtual_desktop(action).map_err(sysmon_err)
    }
    fn fixed_drives(&self) -> PResult<Vec<std::path::PathBuf>> {
        fixed_drives()
    }
    fn local_clock(&self) -> PResult<String> {
        local_clock()
    }
    fn start_foreground_watcher(&self, sink: ForegroundSink) -> WatchGuard {
        match helm_win_sysmon::foreground::ForegroundWatcher::start(move |info| {
            let _ = sink.send(info.clone());
        }) {
            Ok(w) => Box::new(w),
            Err(e) => {
                tracing::warn!("foreground watcher unavailable: {e}");
                Box::new(())
            }
        }
    }

    fn start_recorder(
        &self,
        path: &Path,
        config: RecordConfig,
    ) -> PResult<Arc<dyn Recorder>> {
        super::real5::start_recorder(path, config)
    }
    fn open_terminal(
        &self,
        shell: Shell,
        cols: u16,
        rows: u16,
        on_output: TermOutput,
        on_exit: TermExit,
    ) -> PResult<Box<dyn TermSession>> {
        super::real5::open_terminal(shell, cols, rows, on_output, on_exit)
    }
    fn new_pen(&self) -> PResult<Box<dyn PenDevice>> {
        super::real5::new_pen()
    }
    fn show_toast(&self, toast: &Toast, on_event: ToastCallback) -> PResult<()> {
        self.extras.show_toast(toast, on_event)
    }
    fn http(&self) -> Arc<dyn HttpTransport> {
        self.extras.http()
    }
    fn wifi_ssid(&self) -> PResult<Option<String>> {
        super::real5::wifi_ssid()
    }
    fn utc_offset_minutes(&self) -> i32 {
        super::real5::utc_offset_minutes()
    }

    // ---- g6 ----
    #[cfg(windows)]
    fn clipboard_history_allowed(&self) -> PResult<bool> {
        super::real_g6::clipboard_history_allowed()
    }
    #[cfg(windows)]
    fn reveal_in_explorer(&self, path: &Path) -> PResult<()> {
        super::real_g6::reveal_in_explorer(path)
    }
    #[cfg(windows)]
    fn pc_notify_access(&self) -> PResult<super::NotifyAccess> {
        Ok(super::real_g6::notify_access())
    }
    #[cfg(windows)]
    fn pc_notifications(&self) -> PResult<Vec<super::PcNotification>> {
        super::real_g6::notifications()
    }
    #[cfg(windows)]
    fn cpu_temp_c(&self) -> PResult<Option<f32>> {
        Ok(super::real_g6::cpu_temp_c())
    }
    #[cfg(windows)]
    fn system_uptime_s(&self) -> PResult<u64> {
        Ok(super::real_g6::uptime_s())
    }
}

// ---- Phase 3 helpers ----

struct RealSampler(helm_win_sysmon::stats::Sampler);

impl StatsSampler for RealSampler {
    fn sample(&self) -> PResult<Stats> {
        self.0.sample().map_err(sysmon_err)
    }
}

/// Default capture device (`eCapture`, `eCommunications`) mute state via Core Audio.
#[cfg(windows)]
fn mic_muted() -> PResult<Option<bool>> {
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::Media::Audio::{
        IMMDeviceEnumerator, MMDeviceEnumerator, eCapture, eCommunications,
    };
    use windows::Win32::System::Com::{
        CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
    };
    // SAFETY: COM is initialised on this blocking-pool thread for the duration of the call
    // (and uninitialised only if this call initialised it); interfaces drop before that.
    unsafe {
        let init = CoInitializeEx(None, COINIT_MULTITHREADED);
        let result = (|| -> windows::core::Result<Option<bool>> {
            let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let dev = match en.GetDefaultAudioEndpoint(eCapture, eCommunications) {
                Ok(d) => d,
                // HRESULT_FROM_WIN32(ERROR_NOT_FOUND): no microphone.
                Err(e) if e.code().0 == 0x8007_0490_u32 as i32 => return Ok(None),
                Err(e) => return Err(e),
            };
            let vol: IAudioEndpointVolume = dev.Activate(CLSCTX_ALL, None)?;
            Ok(Some(vol.GetMute()?.as_bool()))
        })();
        if init.is_ok() {
            CoUninitialize();
        }
        result.map_err(|e| RpcError::internal(format!("microphone state: {e}")))
    }
}

#[cfg(not(windows))]
fn mic_muted() -> PResult<Option<bool>> {
    Ok(None)
}

#[cfg(windows)]
fn fixed_drives() -> PResult<Vec<std::path::PathBuf>> {
    use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
    const DRIVE_FIXED: u32 = 3;
    // SAFETY: plain Win32 query.
    let mask = unsafe { GetLogicalDrives() };
    let mut out = Vec::new();
    for i in 0..26u8 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let root = format!("{}:\\", (b'A' + i) as char);
        let wide: Vec<u16> = root.encode_utf16().chain(Some(0)).collect();
        // SAFETY: `wide` is NUL-terminated and outlives the call.
        let kind = unsafe { GetDriveTypeW(windows::core::PCWSTR(wide.as_ptr())) };
        if kind == DRIVE_FIXED {
            out.push(std::path::PathBuf::from(root));
        }
    }
    Ok(out)
}

#[cfg(not(windows))]
fn fixed_drives() -> PResult<Vec<std::path::PathBuf>> {
    Ok(vec![std::path::PathBuf::from("/")])
}

#[cfg(windows)]
fn local_clock() -> PResult<String> {
    // SAFETY: GetLocalTime only returns a struct.
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    Ok(format!("{:02}:{:02}", t.wHour, t.wMinute))
}

#[cfg(not(windows))]
fn local_clock() -> PResult<String> {
    let s = (crate::util::now_ms() / 1000) as u64;
    Ok(format!("{:02}:{:02}", (s / 3600) % 24, (s / 60) % 60))
}

struct Watchers {
    stop: Option<std_mpsc::Sender<()>>,
    join: Option<JoinHandle<()>>,
}

impl Drop for Watchers {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// Owns every watcher on one thread (some watcher types are not `Send`) until stopped.
fn run_watchers(events: EventSink, stop: std_mpsc::Receiver<()>) {
    let tx = events.clone();
    let media = helm_win_system::media::MediaWatcher::start(move |state| {
        let _ = tx.send(PlatformEvent::Media(state));
    })
    .map_err(|e| tracing::warn!("media watcher unavailable: {e}"))
    .ok();
    let tx = events.clone();
    let volume = helm_win_system::audio::VolumeWatcher::start(move |v| {
        let _ = tx.send(PlatformEvent::Volume(v));
    })
    .map_err(|e| tracing::warn!("volume watcher unavailable: {e}"))
    .ok();
    let tx = events.clone();
    let focus = helm_win_input::TextFocusWatcher::start(move |focused| {
        let _ = tx.send(PlatformEvent::TextFocus(focused));
    })
    .map_err(|e| tracing::warn!("text-focus watcher unavailable: {e}"))
    .ok();
    let tx = events;
    let clipboard = helm_win_input::ClipboardWatcher::start(move |c| {
        let _ = tx.send(PlatformEvent::Clipboard {
            has_text: c.has_text,
            has_image: c.has_image,
        });
    })
    .map_err(|e| tracing::warn!("clipboard watcher unavailable: {e}"))
    .ok();
    // Blocks until the guard is dropped (sender gone).
    let _ = stop.recv();
    drop(clipboard);
    drop(focus);
    drop(volume);
    drop(media);
}
