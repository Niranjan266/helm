//! The real platform: this PC, through the `helm-win-*` crates.

use std::path::Path;
use std::sync::mpsc as std_mpsc;
use std::thread::JoinHandle;

use helm_win_apps::IconCache;

use super::*;

#[derive(Debug)]
pub struct RealPlatform {
    icons: Option<IconCache>,
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
        RealPlatform { icons }
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
