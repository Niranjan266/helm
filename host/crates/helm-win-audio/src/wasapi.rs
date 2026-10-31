//! WASAPI shared-mode loopback capture of the default render endpoint, plus default-device
//! change notifications (`IMMNotificationClient`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{HANDLE, PROPERTYKEY};
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK, DEVICE_STATE, EDataFlow, ERole,
    IAudioCaptureClient, IAudioClient, IAudioClient3, IMMDevice, IMMDeviceEnumerator, IMMNotificationClient,
    IMMNotificationClient_Impl, MMDeviceEnumerator, WAVEFORMATEX, WAVEFORMATEXTENSIBLE, eConsole, eRender,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, STGM_READ,
};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Threading::SetEvent;
use windows::core::{Interface, PCWSTR, implement};

use crate::error::{Ctx, Result};
use crate::format::DeviceFormat;
use crate::stats::DeviceInfo;

/// Current QPC time in microseconds (same clock as `helm-win-capture` timestamps, which use
/// QPC in 100 ns units: divide those by 10).
pub fn qpc_now_us() -> i64 {
    let mut f = 0i64;
    let mut c = 0i64;
    unsafe {
        let _ = QueryPerformanceFrequency(&mut f);
        let _ = QueryPerformanceCounter(&mut c);
    }
    if f <= 0 {
        return 0;
    }
    (c as i128 * 1_000_000 / f as i128) as i64
}

/// Buffer duration requested from the legacy `IAudioClient::Initialize` path (100 ns units).
/// Latency is set by how fast we drain the buffer (event + timeout), not by its size; a larger
/// buffer only gives headroom against overruns when the thread is descheduled.
const BUFFER_100NS: i64 = 100 * 10_000;

/// An initialised, started loopback capture on one endpoint.
pub struct LoopbackCapture {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    pub format: DeviceFormat,
    pub info: DeviceInfo,
}

/// One packet read from the capture buffer.
pub struct Packet<'a> {
    /// `None` when the engine marked the packet silent.
    pub data: Option<&'a [u8]>,
    pub frames: usize,
    /// QPC time of the first frame, µs.
    pub qpc_us: i64,
    pub discontinuity: bool,
}

fn device_name(device: &IMMDevice) -> String {
    unsafe {
        let Ok(store) = device.OpenPropertyStore(STGM_READ) else {
            return String::new();
        };
        match store.GetValue(&PKEY_Device_FriendlyName as *const PROPERTYKEY) {
            Ok(v) => v.to_string(),
            Err(_) => String::new(),
        }
    }
}

fn device_id(device: &IMMDevice) -> String {
    unsafe {
        match device.GetId() {
            Ok(p) => {
                let s = p.to_string().unwrap_or_default();
                CoTaskMemFree(Some(p.0 as _));
                s
            }
            Err(_) => String::new(),
        }
    }
}

/// Endpoint id of the current default render device (console role).
pub fn default_device_id(enumerator: &IMMDeviceEnumerator) -> Option<String> {
    let d = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }.ok()?;
    Some(device_id(&d))
}

/// `(id, friendly name)` of the current default playback device.
/// Callable from any thread: initialises COM (MTA) for the call if the thread has not.
pub fn default_device() -> Result<(String, String)> {
    struct Com(bool);
    impl Drop for Com {
        fn drop(&mut self) {
            if self.0 {
                unsafe { CoUninitialize() };
            }
        }
    }
    // S_OK / S_FALSE must be balanced by CoUninitialize; RPC_E_CHANGED_MODE (an STA thread)
    // is fine to use as is.
    let _com = Com(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok());
    let e = enumerator()?;
    let d = unsafe { e.GetDefaultAudioEndpoint(eRender, eConsole) }.map_err(|_| crate::error::Error::NoDevice)?;
    Ok((device_id(&d), device_name(&d)))
}

pub fn enumerator() -> Result<IMMDeviceEnumerator> {
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ctx("CoCreateInstance(MMDeviceEnumerator)") }
}

/// Parse the `WAVEFORMATEX` WASAPI handed us.
///
/// # Safety
/// `wf` must point to a valid `WAVEFORMATEX` (extensible when the tag says so).
unsafe fn parse_format(wf: *const WAVEFORMATEX) -> Result<DeviceFormat> {
    let w = unsafe { std::ptr::read_unaligned(wf) };
    let (valid, mask, sub) = if w.wFormatTag == 0xFFFE && w.cbSize >= 22 {
        let x = unsafe { std::ptr::read_unaligned(wf as *const WAVEFORMATEXTENSIBLE) };
        let valid = unsafe { x.Samples.wValidBitsPerSample };
        (valid, x.dwChannelMask, x.SubFormat.data1)
    } else {
        (0, 0, 0)
    };
    DeviceFormat::from_wave_format(
        w.wFormatTag,
        w.nChannels,
        w.nSamplesPerSec,
        w.wBitsPerSample,
        w.nBlockAlign,
        valid,
        mask,
        sub,
    )
}

impl LoopbackCapture {
    /// Open the default render endpoint in loopback mode, signalling `event` when data is ready,
    /// and start it.
    pub fn open_default(enumerator: &IMMDeviceEnumerator, event: HANDLE) -> Result<LoopbackCapture> {
        let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
            .map_err(|_| crate::error::Error::NoDevice)?;
        Self::open(&device, event)
    }

    pub fn open(device: &IMMDevice, event: HANDLE) -> Result<LoopbackCapture> {
        unsafe {
            let id = device_id(device);
            let name = device_name(device);
            let client: IAudioClient = device
                .Activate(CLSCTX_ALL, None)
                .ctx("IMMDevice::Activate(IAudioClient)")?;
            let mix = client.GetMixFormat().ctx("IAudioClient::GetMixFormat")?;
            let parsed = parse_format(mix);
            let result = parsed.and_then(|format| {
                let flags = AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK;
                // Low-latency path: IAudioClient3 with the engine's minimum period. Windows may
                // refuse it for loopback streams; then use the classic Initialize.
                let mut low_period = None;
                if let Ok(c3) = client.cast::<IAudioClient3>() {
                    let (mut def, mut fund, mut min, mut max) = (0u32, 0u32, 0u32, 0u32);
                    if c3
                        .GetSharedModeEnginePeriod(mix, &mut def, &mut fund, &mut min, &mut max)
                        .is_ok()
                        && min > 0
                        && c3.InitializeSharedAudioStream(flags, min, mix, None).is_ok()
                    {
                        low_period = Some(min);
                    }
                }
                let (client, low_latency, period_frames) = match low_period {
                    Some(min) => (client.clone(), true, min),
                    None => {
                        // A failed Initialize* leaves a client unusable: always use a fresh one.
                        let fresh: IAudioClient = device
                            .Activate(CLSCTX_ALL, None)
                            .ctx("IMMDevice::Activate(IAudioClient)")?;
                        fresh
                            .Initialize(AUDCLNT_SHAREMODE_SHARED, flags, BUFFER_100NS, 0, mix, None)
                            .ctx("IAudioClient::Initialize(loopback)")?;
                        let mut def = 0i64;
                        let _ = fresh.GetDevicePeriod(Some(&mut def), None);
                        let pf = (def as u64 * format.sample_rate as u64 / 10_000_000) as u32;
                        (fresh, false, pf)
                    }
                };
                client.SetEventHandle(event).ctx("IAudioClient::SetEventHandle")?;
                let capture: IAudioCaptureClient = client
                    .GetService()
                    .ctx("IAudioClient::GetService(IAudioCaptureClient)")?;
                client.Start().ctx("IAudioClient::Start")?;
                let info = DeviceInfo {
                    id,
                    name,
                    format: format.describe(),
                    sample_rate: format.sample_rate,
                    channels: format.channels,
                    low_latency,
                    period_ms: period_frames as f64 * 1000.0 / format.sample_rate as f64,
                };
                Ok(LoopbackCapture {
                    client,
                    capture,
                    format,
                    info,
                })
            });
            CoTaskMemFree(Some(mix as _));
            result
        }
    }

    /// Read every packet currently available, calling `f` for each.
    pub fn drain(&mut self, mut f: impl FnMut(Packet<'_>)) -> Result<()> {
        unsafe {
            loop {
                let n = self
                    .capture
                    .GetNextPacketSize()
                    .ctx("IAudioCaptureClient::GetNextPacketSize")?;
                if n == 0 {
                    return Ok(());
                }
                let mut data: *mut u8 = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                let mut qpc = 0u64;
                self.capture
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, Some(&mut qpc))
                    .ctx("IAudioCaptureClient::GetBuffer")?;
                let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
                let bytes = frames as usize * self.format.block_align();
                let slice = if silent || data.is_null() {
                    None
                } else {
                    Some(std::slice::from_raw_parts(data, bytes))
                };
                f(Packet {
                    data: slice,
                    frames: frames as usize,
                    // GetBuffer reports QPC in 100 ns units.
                    qpc_us: (qpc / 10) as i64,
                    discontinuity: flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32 != 0,
                });
                self.capture
                    .ReleaseBuffer(frames)
                    .ctx("IAudioCaptureClient::ReleaseBuffer")?;
            }
        }
    }
}

impl Drop for LoopbackCapture {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
        }
    }
}

/// Raises a flag and wakes the capture thread when the default render device changes or a
/// device is removed / changes state.
#[implement(IMMNotificationClient)]
pub struct DeviceWatcher {
    pub changed: Arc<AtomicBool>,
    /// Event handle (as isize so the struct is Send + Sync).
    pub wake: isize,
}

impl DeviceWatcher {
    fn poke(&self) {
        self.changed.store(true, Ordering::SeqCst);
        unsafe {
            let _ = SetEvent(HANDLE(self.wake as *mut _));
        }
    }
}

impl IMMNotificationClient_Impl for DeviceWatcher_Impl {
    fn OnDeviceStateChanged(&self, _id: &PCWSTR, _state: DEVICE_STATE) -> windows::core::Result<()> {
        // A disabled / unplugged endpoint may be ours; the capture loop also sees
        // AUDCLNT_E_DEVICE_INVALIDATED, this only makes the reaction faster.
        self.poke();
        Ok(())
    }
    fn OnDeviceAdded(&self, _id: &PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }
    fn OnDeviceRemoved(&self, _id: &PCWSTR) -> windows::core::Result<()> {
        self.poke();
        Ok(())
    }
    fn OnDefaultDeviceChanged(&self, flow: EDataFlow, role: ERole, _id: &PCWSTR) -> windows::core::Result<()> {
        if flow == eRender && role == eConsole {
            self.poke();
        }
        Ok(())
    }
    fn OnPropertyValueChanged(&self, _id: &PCWSTR, _key: &PROPERTYKEY) -> windows::core::Result<()> {
        Ok(())
    }
}
