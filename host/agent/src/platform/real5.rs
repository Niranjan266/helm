//! Phase 5 pieces of the real platform: MP4 recorder, ConPTY terminals, synthetic pen,
//! toasts, Wi-Fi SSID and the time-zone offset.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use super::*;

/// Toasts kept alive so their click handlers keep working (oldest dropped first).
const MAX_LIVE_TOASTS: usize = 64;
pub const AUMID: &str = "Helm.Agent";

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Lazily created state the real platform needs for Phase 5.
#[derive(Default)]
pub struct Extras {
    notifier: OnceLock<Result<helm_win_notify::Notifier, String>>,
    toasts: Mutex<VecDeque<helm_win_notify::ShownToast>>,
    http: OnceLock<Arc<dyn HttpTransport>>,
}

impl std::fmt::Debug for Extras {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Extras").finish_non_exhaustive()
    }
}

impl Extras {
    pub fn http(&self) -> Arc<dyn HttpTransport> {
        self.http
            .get_or_init(|| Arc::new(crate::http::TlsHttpTransport::new()))
            .clone()
    }

    pub fn show_toast(&self, toast: &Toast, on_event: ToastCallback) -> PResult<()> {
        let notifier = self.notifier.get_or_init(|| {
            helm_win_notify::Notifier::new(&helm_win_notify::AppRegistration {
                aumid: AUMID.into(),
                display_name: "Helm".into(),
                icon_path: None,
            })
            .map_err(|e| e.to_string())
        });
        let notifier = notifier
            .as_ref()
            .map_err(|e| RpcError::unsupported(format!("toasts unavailable: {e}")))?;
        let shown = notifier
            .show(toast, on_event)
            .map_err(notify_err)?;
        let mut live = lock(&self.toasts);
        live.push_back(shown);
        while live.len() > MAX_LIVE_TOASTS {
            live.pop_front();
        }
        Ok(())
    }
}

fn notify_err(e: helm_win_notify::Error) -> RpcError {
    match e {
        helm_win_notify::Error::InvalidInput(m) => RpcError::bad_params(m),
        helm_win_notify::Error::Unsupported(m) => RpcError::unsupported(m),
        other => RpcError::internal(other.to_string()),
    }
}

// ---------------------------------------------------------------- recorder

struct RealRecorder {
    rec: helm_win_record::Recorder,
    decoder: Mutex<Option<helm_win_audio::Decoder>>,
    pcm: Mutex<Vec<f32>>,
    error: Mutex<Option<String>>,
}

impl RealRecorder {
    fn keep_error(&self, e: helm_win_record::Error) {
        let mut err = lock(&self.error);
        if err.is_none() {
            tracing::warn!("recording: {e}");
            *err = Some(e.to_string());
        }
    }
}

impl Recorder for RealRecorder {
    fn push_video(&self, annex_b: &[u8], is_key: bool, pts_us: u64) {
        if let Err(e) = self.rec.push_video(annex_b, is_key, pts_us) {
            self.keep_error(e);
        }
    }

    fn push_audio_opus(&self, packet: &[u8], pts_us: u64) {
        let mut dec = lock(&self.decoder);
        let Some(decoder) = dec.as_mut() else { return };
        let mut pcm = lock(&self.pcm);
        pcm.clear();
        if decoder.decode_f32(packet, &mut pcm).is_ok()
            && !pcm.is_empty()
            && let Err(e) = self.rec.push_audio(&pcm, pts_us)
        {
            self.keep_error(e);
        }
    }

    fn stop(&self) -> PResult<RecordedFile> {
        let info = self.rec.stop().map_err(record_err)?;
        Ok(RecordedFile {
            path: info.path,
            duration_ms: info.duration_ms,
        })
    }
}

pub fn start_recorder(path: &Path, config: RecordConfig) -> PResult<Arc<dyn Recorder>> {
    let video = helm_win_record::VideoConfig {
        width: config.width,
        height: config.height,
        fps: config.fps,
    };
    let rec = helm_win_record::Recorder::start(path, video, config.audio).map_err(record_err)?;
    let decoder = if config.audio {
        helm_win_audio::Decoder::new(48_000, 2)
            .map_err(|e| tracing::warn!("recording without audio: {e}"))
            .ok()
    } else {
        None
    };
    Ok(Arc::new(RealRecorder {
        rec,
        decoder: Mutex::new(decoder),
        pcm: Mutex::new(Vec::new()),
        error: Mutex::new(None),
    }))
}

// ---------------------------------------------------------------- terminal

fn term_err(e: helm_win_terminal::Error) -> RpcError {
    match e {
        helm_win_terminal::Error::InvalidConfig(m) => RpcError::bad_params(m),
        helm_win_terminal::Error::Unsupported(m) => RpcError::unsupported(m),
        other => RpcError::internal(other.to_string()),
    }
}

struct RealTerm(helm_win_terminal::Terminal);

impl TermSession for RealTerm {
    fn write(&self, bytes: &[u8]) -> PResult<()> {
        self.0.write(bytes).map_err(term_err)
    }
    fn resize(&self, cols: u16, rows: u16) -> PResult<()> {
        self.0.resize(cols, rows).map_err(term_err)
    }
    fn kill(&self) -> PResult<()> {
        self.0.kill().map_err(term_err)
    }
}

pub fn open_terminal(
    shell: Shell,
    cols: u16,
    rows: u16,
    mut on_output: TermOutput,
    on_exit: TermExit,
) -> PResult<Box<dyn TermSession>> {
    let cwd = crate::paths::profile_dir();
    let cwd = cwd.is_dir().then_some(cwd);
    helm_win_terminal::Terminal::spawn_with_exit(
        shell,
        cols,
        rows,
        cwd.as_deref(),
        move |b: &[u8]| on_output(b),
        on_exit,
    )
    .map(|t| Box::new(RealTerm(t)) as Box<dyn TermSession>)
    .map_err(term_err)
}

// ---------------------------------------------------------------- pen

fn pen_err(e: helm_win_pen::Error) -> RpcError {
    match e {
        helm_win_pen::Error::Unsupported(m) => RpcError::unsupported(m),
        other => RpcError::internal(other.to_string()),
    }
}

struct RealPen(helm_win_pen::Pen);

impl PenDevice for RealPen {
    fn inject(&self, sample: &PenSample) -> PResult<()> {
        self.0.inject(sample).map_err(pen_err)
    }
    fn release(&self) -> PResult<()> {
        self.0.release().map_err(pen_err)
    }
}

pub fn new_pen() -> PResult<Box<dyn PenDevice>> {
    helm_win_pen::Pen::new()
        .map(|p| Box::new(RealPen(p)) as Box<dyn PenDevice>)
        .map_err(pen_err)
}

// ---------------------------------------------------------------- Wi-Fi

/// SSID of the first connected WLAN interface (WlanQueryInterface, current connection).
/// Windows 11 24H2+ needs location access for this; without it we report `None`.
#[cfg(windows)]
pub fn wifi_ssid() -> PResult<Option<String>> {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::NetworkManagement::WiFi::{
        WLAN_CONNECTION_ATTRIBUTES, WLAN_INTERFACE_INFO_LIST, WlanCloseHandle,
        WlanEnumInterfaces, WlanFreeMemory, WlanOpenHandle, WlanQueryInterface,
        wlan_interface_state_connected, wlan_intf_opcode_current_connection,
    };
    // SAFETY: every pointer the WLAN API returns is checked for null, read only while valid
    // and freed with WlanFreeMemory; the handle is closed on every path.
    unsafe {
        let mut version = 0u32;
        let mut handle = HANDLE::default();
        if WlanOpenHandle(2, None, &mut version, &mut handle) != 0 {
            return Ok(None); // WLAN service not running / no Wi-Fi hardware
        }
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
        let mut ssid = None;
        if WlanEnumInterfaces(handle, None, &mut list) == 0 && !list.is_null() {
            let count = (*list).dwNumberOfItems as usize;
            let first = (*list).InterfaceInfo.as_ptr();
            for i in 0..count {
                let info = &*first.add(i);
                if info.isState != wlan_interface_state_connected {
                    continue;
                }
                let mut size = 0u32;
                let mut data: *mut core::ffi::c_void = std::ptr::null_mut();
                let rc = WlanQueryInterface(
                    handle,
                    &info.InterfaceGuid,
                    wlan_intf_opcode_current_connection,
                    None,
                    &mut size,
                    &mut data,
                    None,
                );
                if rc == 0 && !data.is_null() {
                    if size as usize >= std::mem::size_of::<WLAN_CONNECTION_ATTRIBUTES>() {
                        let attrs = &*(data as *const WLAN_CONNECTION_ATTRIBUTES);
                        let s = &attrs.wlanAssociationAttributes.dot11Ssid;
                        let len = (s.uSSIDLength as usize).min(s.ucSSID.len());
                        if len > 0 {
                            ssid = Some(String::from_utf8_lossy(&s.ucSSID[..len]).into_owned());
                        }
                    }
                    WlanFreeMemory(data);
                }
                if ssid.is_some() {
                    break;
                }
            }
        }
        if !list.is_null() {
            WlanFreeMemory(list as *const core::ffi::c_void);
        }
        WlanCloseHandle(handle, None);
        Ok(ssid)
    }
}

#[cfg(not(windows))]
pub fn wifi_ssid() -> PResult<Option<String>> {
    Ok(None)
}

/// Local time minus UTC in minutes, including daylight saving time when in effect.
#[cfg(windows)]
pub fn utc_offset_minutes() -> i32 {
    use windows::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
    let mut tz = TIME_ZONE_INFORMATION::default();
    // SAFETY: fills a plain struct.
    let kind = unsafe { GetTimeZoneInformation(&mut tz) };
    let extra = match kind {
        1 => tz.StandardBias, // TIME_ZONE_ID_STANDARD
        2 => tz.DaylightBias, // TIME_ZONE_ID_DAYLIGHT
        _ => 0,
    };
    -(tz.Bias + extra)
}

#[cfg(not(windows))]
pub fn utc_offset_minutes() -> i32 {
    0
}
