//! Master volume of the default playback device, and per-app audio sessions.
//!
//! Backed by Core Audio: `IMMDeviceEnumerator` -> default render endpoint
//! (`eRender`, `eConsole`) -> `IAudioEndpointVolume`, and
//! `IAudioSessionManager2` for per-app sessions.

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// Master volume state.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VolumeState {
    /// Scalar volume 0.0..=1.0 (the value the Windows volume slider shows / 100).
    pub level: f32,
    /// Muted.
    pub muted: bool,
}

/// One per-app audio session on the default playback device.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioSession {
    /// Session instance identifier; pass it to [`set_session_volume`] / [`set_session_muted`].
    pub id: String,
    /// Display name, falling back to the process executable name.
    pub name: String,
    /// Process id (0 for the system sounds session).
    pub pid: u32,
    /// Session volume 0.0..=1.0 (relative to the master volume).
    pub level: f32,
    /// Session muted.
    pub muted: bool,
    /// The "System sounds" session.
    pub is_system: bool,
    /// Currently producing audio.
    pub active: bool,
}

fn check_level(level: f32) -> Result<()> {
    if level.is_finite() && (0.0..=1.0).contains(&level) {
        Ok(())
    } else {
        Err(Error::InvalidArgument(format!("volume level {level} not in 0.0..=1.0")))
    }
}

/// Master volume and mute of the default playback device.
pub fn volume() -> Result<VolumeState> {
    imp::volume()
}

/// Set the master volume (0.0..=1.0).
pub fn set_volume(level: f32) -> Result<()> {
    check_level(level)?;
    imp::set_volume(level)
}

/// Mute or unmute the default playback device.
pub fn set_muted(muted: bool) -> Result<()> {
    imp::set_muted(muted)
}

/// Per-app sessions on the default playback device (expired sessions are skipped).
pub fn list_sessions() -> Result<Vec<AudioSession>> {
    imp::list_sessions()
}

/// Set one session's volume (0.0..=1.0) by [`AudioSession::id`].
pub fn set_session_volume(id: &str, level: f32) -> Result<()> {
    check_level(level)?;
    imp::set_session(id, Some(level), None)
}

/// Mute or unmute one session by [`AudioSession::id`].
pub fn set_session_muted(id: &str, muted: bool) -> Result<()> {
    imp::set_session(id, None, Some(muted))
}

/// Watches master volume / mute of the default playback device
/// (`IAudioEndpointVolumeCallback`) and follows default-device changes
/// (`IMMNotificationClient`).
///
/// The callback runs on the watcher's own thread; it fires for every change
/// (including ones made by this process) and once with the new device's state
/// when the default device changes. Dropping the watcher stops it.
pub struct VolumeWatcher {
    _inner: imp::WatcherInner,
}

impl VolumeWatcher {
    /// Start watching.
    pub fn start<F>(callback: F) -> Result<VolumeWatcher>
    where
        F: Fn(VolumeState) + Send + 'static,
    {
        Ok(VolumeWatcher { _inner: imp::WatcherInner::start(Box::new(callback))? })
    }
}

type Callback = Box<dyn Fn(VolumeState) + Send + 'static>;

#[cfg(windows)]
mod imp {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::mpsc;
    use std::thread::{self, JoinHandle};

    use windows::Win32::Foundation::{CloseHandle, S_OK};
    use windows::Win32::Media::Audio::Endpoints::{
        IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl,
    };
    use windows::Win32::Media::Audio::{
        AUDIO_VOLUME_NOTIFICATION_DATA, AudioSessionStateActive, AudioSessionStateExpired, DEVICE_STATE, EDataFlow,
        ERole, IAudioSessionControl2, IAudioSessionManager2, IMMDevice, IMMDeviceEnumerator, IMMNotificationClient,
        IMMNotificationClient_Impl, ISimpleAudioVolume, MMDeviceEnumerator, eConsole, eRender,
    };
    use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree};
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
    use windows::core::{Interface, PCWSTR, PWSTR, implement};

    use super::{AudioSession, Callback, VolumeState};
    use crate::worker::{ComApartment, static_worker, wide_to_string};
    use crate::{Error, Result};

    static_worker!(worker, "helm-audio");

    /// HRESULT_FROM_WIN32(ERROR_NOT_FOUND): no default device.
    const E_NOTFOUND: i32 = 0x8007_0490_u32 as i32;

    fn map_err(e: windows::core::Error) -> Error {
        if e.code().0 == E_NOTFOUND {
            Error::NotFound("no default playback device".into())
        } else {
            e.into()
        }
    }

    fn enumerator() -> Result<IMMDeviceEnumerator> {
        Ok(unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? })
    }

    fn default_device(en: &IMMDeviceEnumerator) -> Result<IMMDevice> {
        unsafe { en.GetDefaultAudioEndpoint(eRender, eConsole) }.map_err(map_err)
    }

    fn endpoint_volume(dev: &IMMDevice) -> Result<IAudioEndpointVolume> {
        Ok(unsafe { dev.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)? })
    }

    fn read(ep: &IAudioEndpointVolume) -> Result<VolumeState> {
        unsafe {
            Ok(VolumeState { level: ep.GetMasterVolumeLevelScalar()?, muted: ep.GetMute()?.as_bool() })
        }
    }

    fn with_endpoint<R: Send + 'static>(f: impl FnOnce(&IAudioEndpointVolume) -> Result<R> + Send + 'static) -> Result<R> {
        worker().run(move || {
            let en = enumerator()?;
            let ep = endpoint_volume(&default_device(&en)?)?;
            f(&ep)
        })?
    }

    pub(super) fn volume() -> Result<VolumeState> {
        with_endpoint(read)
    }

    pub(super) fn set_volume(level: f32) -> Result<()> {
        with_endpoint(move |ep| unsafe { ep.SetMasterVolumeLevelScalar(level, std::ptr::null()) }.map_err(Into::into))
    }

    pub(super) fn set_muted(muted: bool) -> Result<()> {
        with_endpoint(move |ep| unsafe { ep.SetMute(muted, std::ptr::null()) }.map_err(Into::into))
    }

    /// Take ownership of a CoTaskMem string.
    unsafe fn take_pwstr(p: PWSTR) -> String {
        if p.is_null() {
            return String::new();
        }
        let s = unsafe { p.to_string() }.unwrap_or_default();
        unsafe { CoTaskMemFree(Some(p.0 as *const _)) };
        s
    }

    fn process_name(pid: u32) -> Option<String> {
        if pid == 0 {
            return None;
        }
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            let r = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
            let _ = CloseHandle(h);
            r.ok()?;
            let path = wide_to_string(&buf[..len as usize]);
            let file = path.rsplit('\\').next().unwrap_or(&path);
            Some(file.strip_suffix(".exe").or_else(|| file.strip_suffix(".EXE")).unwrap_or(file).to_string())
        }
    }

    /// Enumerate live sessions as (info, volume interface).
    fn sessions() -> Result<Vec<(AudioSession, ISimpleAudioVolume)>> {
        let en = enumerator()?;
        let dev = default_device(&en)?;
        let mut out = Vec::new();
        unsafe {
            let mgr: IAudioSessionManager2 = dev.Activate(CLSCTX_ALL, None)?;
            let list = mgr.GetSessionEnumerator()?;
            for i in 0..list.GetCount()? {
                let Ok(ctl) = list.GetSession(i) else { continue };
                let state = ctl.GetState().unwrap_or(AudioSessionStateExpired);
                if state == AudioSessionStateExpired {
                    continue;
                }
                let Ok(ctl2) = ctl.cast::<IAudioSessionControl2>() else { continue };
                let Ok(vol) = ctl.cast::<ISimpleAudioVolume>() else { continue };
                let id = ctl2.GetSessionInstanceIdentifier().map(|p| take_pwstr(p)).unwrap_or_default();
                if id.is_empty() {
                    continue;
                }
                let pid = ctl2.GetProcessId().unwrap_or(0);
                let is_system = ctl2.IsSystemSoundsSession() == S_OK;
                let display = ctl.GetDisplayName().map(|p| take_pwstr(p)).unwrap_or_default();
                let name = if is_system {
                    "System sounds".to_string()
                } else if !display.is_empty() && !display.starts_with('@') {
                    display
                } else {
                    process_name(pid).unwrap_or_else(|| format!("pid {pid}"))
                };
                let info = AudioSession {
                    id,
                    name,
                    pid,
                    level: vol.GetMasterVolume().unwrap_or(0.0),
                    muted: vol.GetMute().map(|b| b.as_bool()).unwrap_or(false),
                    is_system,
                    active: state == AudioSessionStateActive,
                };
                out.push((info, vol));
            }
        }
        Ok(out)
    }

    pub(super) fn list_sessions() -> Result<Vec<AudioSession>> {
        worker().run(|| sessions().map(|v| v.into_iter().map(|(s, _)| s).collect()))?
    }

    pub(super) fn set_session(id: &str, level: Option<f32>, muted: Option<bool>) -> Result<()> {
        let id = id.to_string();
        worker().run(move || {
            let (_, vol) = sessions()?
                .into_iter()
                .find(|(s, _)| s.id == id)
                .ok_or_else(|| Error::NotFound(format!("audio session {id}")))?;
            unsafe {
                if let Some(l) = level {
                    vol.SetMasterVolume(l, std::ptr::null())?;
                }
                if let Some(m) = muted {
                    vol.SetMute(m, std::ptr::null())?;
                }
            }
            Ok(())
        })?
    }

    // ----- watcher -------------------------------------------------------

    enum Msg {
        Volume(VolumeState),
        DefaultChanged,
        Stop,
    }

    #[implement(IAudioEndpointVolumeCallback)]
    struct VolumeCallback {
        tx: mpsc::Sender<Msg>,
    }

    impl IAudioEndpointVolumeCallback_Impl for VolumeCallback_Impl {
        fn OnNotify(&self, pnotify: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> windows::core::Result<()> {
            if let Some(d) = unsafe { pnotify.as_ref() } {
                let _ = self.tx.send(Msg::Volume(VolumeState { level: d.fMasterVolume, muted: d.bMuted.as_bool() }));
            }
            Ok(())
        }
    }

    #[implement(IMMNotificationClient)]
    struct DeviceCallback {
        tx: mpsc::Sender<Msg>,
    }

    impl IMMNotificationClient_Impl for DeviceCallback_Impl {
        fn OnDeviceStateChanged(&self, _: &PCWSTR, _: DEVICE_STATE) -> windows::core::Result<()> {
            Ok(())
        }
        fn OnDeviceAdded(&self, _: &PCWSTR) -> windows::core::Result<()> {
            Ok(())
        }
        fn OnDeviceRemoved(&self, _: &PCWSTR) -> windows::core::Result<()> {
            Ok(())
        }
        fn OnDefaultDeviceChanged(&self, flow: EDataFlow, role: ERole, _: &PCWSTR) -> windows::core::Result<()> {
            // Never call back into Core Audio from here; just signal the watcher thread.
            if flow == eRender && role == eConsole {
                let _ = self.tx.send(Msg::DefaultChanged);
            }
            Ok(())
        }
        fn OnPropertyValueChanged(&self, _: &PCWSTR, _: &windows::Win32::Foundation::PROPERTYKEY) -> windows::core::Result<()> {
            Ok(())
        }
    }

    pub(crate) struct WatcherInner {
        tx: mpsc::Sender<Msg>,
        thread: Option<JoinHandle<()>>,
    }

    fn bind(en: &IMMDeviceEnumerator, cb: &IAudioEndpointVolumeCallback) -> Option<IAudioEndpointVolume> {
        let ep = endpoint_volume(&default_device(en).ok()?).ok()?;
        unsafe { ep.RegisterControlChangeNotify(cb).ok()? };
        Some(ep)
    }

    fn unbind(ep: Option<IAudioEndpointVolume>, cb: &IAudioEndpointVolumeCallback) {
        if let Some(ep) = ep {
            let _ = unsafe { ep.UnregisterControlChangeNotify(cb) };
        }
    }

    impl WatcherInner {
        pub(crate) fn start(callback: Callback) -> Result<WatcherInner> {
            let (tx, rx) = mpsc::channel::<Msg>();
            let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<()>>(1);
            let tx2 = tx.clone();
            let thread = thread::Builder::new()
                .name("helm-volume-watch".into())
                .spawn(move || {
                    let _com = ComApartment::enter();
                    let en = match enumerator() {
                        Ok(e) => e,
                        Err(e) => {
                            let _ = ready_tx.send(Err(e));
                            return;
                        }
                    };
                    let dev_cb: IMMNotificationClient = DeviceCallback { tx: tx2.clone() }.into();
                    if let Err(e) = unsafe { en.RegisterEndpointNotificationCallback(&dev_cb) } {
                        let _ = ready_tx.send(Err(e.into()));
                        return;
                    }
                    let vol_cb: IAudioEndpointVolumeCallback = VolumeCallback { tx: tx2 }.into();
                    // No default device yet is fine: we bind when one appears.
                    let mut ep = bind(&en, &vol_cb);
                    let _ = ready_tx.send(Ok(()));
                    let call = |s: VolumeState| {
                        let _ = catch_unwind(AssertUnwindSafe(|| callback(s)));
                    };
                    for msg in rx.iter() {
                        match msg {
                            Msg::Stop => break,
                            Msg::Volume(s) => call(s),
                            Msg::DefaultChanged => {
                                unbind(ep.take(), &vol_cb);
                                ep = bind(&en, &vol_cb);
                                if let Some(s) = ep.as_ref().and_then(|e| read(e).ok()) {
                                    call(s);
                                }
                            }
                        }
                    }
                    unbind(ep, &vol_cb);
                    let _ = unsafe { en.UnregisterEndpointNotificationCallback(&dev_cb) };
                })
                .map_err(|e| Error::Internal(format!("spawn watcher: {e}")))?;
            match ready_rx.recv() {
                Ok(Ok(())) => Ok(WatcherInner { tx, thread: Some(thread) }),
                Ok(Err(e)) => {
                    let _ = thread.join();
                    Err(e)
                }
                Err(_) => Err(Error::Internal("volume watcher thread exited".into())),
            }
        }
    }

    impl Drop for WatcherInner {
        fn drop(&mut self) {
            let _ = self.tx.send(Msg::Stop);
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{AudioSession, Callback, VolumeState};
    use crate::{Error, Result};

    const U: Error = Error::Unsupported("audio requires Windows");

    pub(super) fn volume() -> Result<VolumeState> {
        Err(U)
    }
    pub(super) fn set_volume(_: f32) -> Result<()> {
        Err(U)
    }
    pub(super) fn set_muted(_: bool) -> Result<()> {
        Err(U)
    }
    pub(super) fn list_sessions() -> Result<Vec<AudioSession>> {
        Err(U)
    }
    pub(super) fn set_session(_: &str, _: Option<f32>, _: Option<bool>) -> Result<()> {
        Err(U)
    }
    pub(crate) struct WatcherInner;
    impl WatcherInner {
        pub(crate) fn start(_: Callback) -> Result<WatcherInner> {
            Err(U)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_levels() {
        assert!(matches!(set_volume(1.5), Err(Error::InvalidArgument(_))));
        assert!(matches!(set_volume(-0.1), Err(Error::InvalidArgument(_))));
        assert!(matches!(set_volume(f32::NAN), Err(Error::InvalidArgument(_))));
        assert!(matches!(set_session_volume("x", 2.0), Err(Error::InvalidArgument(_))));
    }

    #[test]
    fn serde_shape() {
        let v = serde_json::to_value(VolumeState { level: 0.5, muted: true }).unwrap();
        assert_eq!(v, serde_json::json!({"level": 0.5, "muted": true}));
        let s = AudioSession {
            id: "id".into(),
            name: "Spotify".into(),
            pid: 42,
            level: 1.0,
            muted: false,
            is_system: false,
            active: true,
        };
        let v = serde_json::to_value(s).unwrap();
        assert_eq!(v["pid"], 42);
        assert_eq!(v["is_system"], false);
    }
}
