//! Media Foundation H.264 encoder MFT.
//!
//! * Enumeration: `MFTEnum2` (restricted to the capture device's adapter LUID) / `MFTEnumEx` with
//!   `MFT_CATEGORY_VIDEO_ENCODER` and `MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER`,
//!   then the Microsoft software encoder (`MFT_ENUM_FLAG_SYNCMFT`) as the fallback.
//! * Hardware MFTs are asynchronous: they are unlocked with `MF_TRANSFORM_ASYNC_UNLOCK` and
//!   driven by `METransformNeedInput` / `METransformHaveOutput` events, which an
//!   [`EventForwarder`] (`IMFAsyncCallback`) turns into messages for the pipeline thread.
//! * D3D11-aware MFTs get the `IMFDXGIDeviceManager` (`MFT_MESSAGE_SET_D3D_MANAGER`) and read
//!   NV12 textures directly (`MFCreateDXGISurfaceBuffer`). Otherwise NV12 is copied to system
//!   memory through a staging texture.
//! * Low latency (`CODECAPI_AVLowLatencyMode`, `MF_LOW_LATENCY`), CBR, no B-frames, fixed GOP,
//!   Main profile (Baseline fallback), on-demand IDR (`CODECAPI_AVEncVideoForceKeyFrame`).
//! * Output is Annex-B; SPS/PPS are guaranteed on every keyframe (see [`ParameterSets`]).

use std::mem::ManuallyDrop;

use windows::Win32::Foundation::{E_NOTIMPL, S_OK};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_NV12, DXGI_SAMPLE_DESC};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize};
use windows::Win32::System::Variant::{VARIANT, VT_BOOL, VT_UI4};
use windows::core::{GUID, HRESULT, Interface, PWSTR, Ref, implement};

use crate::annexb::{self, ParameterSets};
use crate::config::EncoderPreference;
use crate::d3d::D3dDevice;
use crate::error::{Context, Error, Result};

/// Description of an installed H.264 encoder MFT.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncoderInfo {
    /// `MFT_FRIENDLY_NAME_Attribute`, e.g. "NVIDIA H.264 Encoder MFT".
    pub name: String,
    pub hardware: bool,
}

/// Per-thread COM (MTA) + Media Foundation startup, undone on drop.
pub struct MfRuntime {
    com_initialized: bool,
}

impl MfRuntime {
    pub fn new() -> Result<Self> {
        // SAFETY: balanced by Drop on the same thread.
        unsafe {
            let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
            // S_FALSE = already initialised (still needs a matching CoUninitialize);
            // RPC_E_CHANGED_MODE = thread is STA, which MF also works with; don't uninit then.
            let com_initialized = hr.is_ok();
            MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET).ctx("MFStartup")?;
            Ok(Self { com_initialized })
        }
    }
}

impl Drop for MfRuntime {
    fn drop(&mut self) {
        // SAFETY: matches MFStartup / CoInitializeEx in `new`.
        unsafe {
            let _ = MFShutdown();
            if self.com_initialized {
                CoUninitialize();
            }
        }
    }
}

fn type_info(subtype: GUID) -> MFT_REGISTER_TYPE_INFO {
    MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: subtype }
}

/// Enumerate H.264 encoder activates. `adapter_luid`: restrict to one GPU (uses `MFTEnum2`).
fn enum_activates(flags: MFT_ENUM_FLAG, adapter_luid: Option<&[u8; 8]>) -> Result<Vec<IMFActivate>> {
    let input = type_info(MFVideoFormat_NV12);
    let output = type_info(MFVideoFormat_H264);
    let mut pp: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut n = 0u32;
    // SAFETY: MF allocates the array with CoTaskMemAlloc; we take every element and free it.
    unsafe {
        match adapter_luid {
            Some(luid) => {
                let mut attrs: Option<IMFAttributes> = None;
                MFCreateAttributes(&mut attrs, 1).ctx("MFCreateAttributes")?;
                let attrs = attrs.ok_or_else(|| Error::Encoder("MFCreateAttributes returned null".into()))?;
                attrs.SetBlob(&MFT_ENUM_ADAPTER_LUID, luid).ctx("SetBlob(MFT_ENUM_ADAPTER_LUID)")?;
                MFTEnum2(MFT_CATEGORY_VIDEO_ENCODER, flags, Some(&input), Some(&output), &attrs, &mut pp, &mut n)
                    .ctx("MFTEnum2")?;
            }
            None => {
                MFTEnumEx(MFT_CATEGORY_VIDEO_ENCODER, flags, Some(&input), Some(&output), &mut pp, &mut n)
                    .ctx("MFTEnumEx")?;
            }
        }
        let mut out = Vec::with_capacity(n as usize);
        if !pp.is_null() {
            for a in std::slice::from_raw_parts_mut(pp, n as usize) {
                if let Some(a) = a.take() {
                    out.push(a);
                }
            }
            CoTaskMemFree(Some(pp as *const _));
        }
        Ok(out)
    }
}

fn friendly_name(attrs: &IMFAttributes) -> String {
    let mut p = PWSTR::null();
    let mut len = 0u32;
    // SAFETY: MF allocates the string; we free it with CoTaskMemFree.
    unsafe {
        if attrs.GetAllocatedString(&MFT_FRIENDLY_NAME_Attribute, &mut p, &mut len).is_ok() && !p.is_null() {
            let s = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as *const _));
            return s;
        }
    }
    "unnamed H.264 encoder".to_string()
}

const HW_FLAGS: MFT_ENUM_FLAG = MFT_ENUM_FLAG(MFT_ENUM_FLAG_HARDWARE.0 | MFT_ENUM_FLAG_SORTANDFILTER.0);
const SW_FLAGS: MFT_ENUM_FLAG = MFT_ENUM_FLAG(MFT_ENUM_FLAG_SYNCMFT.0 | MFT_ENUM_FLAG_SORTANDFILTER.0);

/// List installed H.264 encoders: hardware first (in the system's preferred order), then software.
/// Initialises COM/MF on the calling thread for the duration of the call.
pub fn list_h264_encoders() -> Result<Vec<EncoderInfo>> {
    let _rt = MfRuntime::new()?;
    let mut out = Vec::new();
    for (flags, hardware) in [(HW_FLAGS, true), (SW_FLAGS, false)] {
        for a in enum_activates(flags, None).unwrap_or_default() {
            out.push(EncoderInfo { name: friendly_name(&a), hardware });
        }
    }
    Ok(out)
}

/// Build a VT_UI4 VARIANT.
fn variant_u32(v: u32) -> VARIANT {
    let mut var = VARIANT::default();
    // SAFETY: writing the tag and the matching union member of a zeroed VARIANT.
    unsafe {
        (*var.Anonymous.Anonymous).vt = VT_UI4;
        (*var.Anonymous.Anonymous).Anonymous.ulVal = v;
    }
    var
}

/// Build a VT_BOOL VARIANT.
fn variant_bool(v: bool) -> VARIANT {
    let mut var = VARIANT::default();
    // SAFETY: as above.
    unsafe {
        (*var.Anonymous.Anonymous).vt = VT_BOOL;
        (*var.Anonymous.Anonymous).Anonymous.boolVal =
            windows::Win32::Foundation::VARIANT_BOOL(if v { -1 } else { 0 });
    }
    var
}

fn pack(hi: u32, lo: u32) -> u64 {
    ((hi as u64) << 32) | lo as u64
}

/// Encoder settings derived from the stream config.
#[derive(Debug, Clone, Copy)]
pub struct EncoderSettings {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub gop_frames: u32,
    /// `CODECAPI_AVEncCommonQualityVsSpeed` (0 = fastest).
    pub quality_vs_speed: u32,
}

/// One encoded access unit.
#[derive(Debug, Clone)]
pub struct EncodedFrame {
    /// Annex-B bytes; keyframes always start with SPS + PPS.
    pub data: Vec<u8>,
    /// Sample time (100 ns) of the input frame this came from.
    pub sample_time: i64,
    pub keyframe: bool,
}

/// Result of one `ProcessOutput` call.
pub enum OutputResult {
    Frame(EncodedFrame),
    NeedMoreInput,
    /// Output type renegotiated; call again.
    StreamChanged,
}

/// Forwards media events of an async MFT to a closure (on an MF work-queue thread).
/// The pipeline re-arms `BeginGetEvent` after handling each event.
#[implement(IMFAsyncCallback)]
pub struct EventForwarder {
    generator: IMFMediaEventGenerator,
    sink: Box<dyn Fn(u32, HRESULT)>,
}

/// Event type value used to report a failed `EndGetEvent` (e.g. after shutdown).
pub const EVENT_ERROR: u32 = u32::MAX;

impl IMFAsyncCallback_Impl for EventForwarder_Impl {
    fn GetParameters(&self, _flags: *mut u32, _queue: *mut u32) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn Invoke(&self, result: Ref<IMFAsyncResult>) -> windows::core::Result<()> {
        let result = result.ok()?;
        // SAFETY: completing the BeginGetEvent this callback was registered with.
        match unsafe { self.generator.EndGetEvent(result) } {
            Ok(ev) => {
                // SAFETY: valid event.
                let (t, st) = unsafe { (ev.GetType().unwrap_or(0), ev.GetStatus().unwrap_or(S_OK)) };
                (self.sink)(t, st);
            }
            Err(e) => (self.sink)(EVENT_ERROR, e.code()),
        }
        Ok(())
    }
}

/// A configured, streaming H.264 encoder MFT.
pub struct H264Encoder {
    transform: IMFTransform,
    activate: IMFActivate,
    codec_api: Option<ICodecAPI>,
    events: Option<IMFMediaEventGenerator>,
    pub info: EncoderInfo,
    /// NV12 textures go straight to the encoder (no CPU copy).
    pub gpu_input: bool,
    pub profile: &'static str,
    /// Human-readable list of ICodecAPI settings that the encoder accepted / rejected.
    pub codec_settings: Vec<String>,
    in_id: u32,
    out_id: u32,
    provides_samples: bool,
    out_buffer_size: u32,
    width: u32,
    height: u32,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    staging: Option<ID3D11Texture2D>,
    params: ParameterSets,
    shut_down: bool,
}

impl H264Encoder {
    /// Pick and configure an encoder. Tries hardware MFTs on the capture adapter (GPU input,
    /// then system-memory input), then the software MFT, according to `pref`.
    pub fn create(
        d3d: &D3dDevice,
        manager: &IMFDXGIDeviceManager,
        s: EncoderSettings,
        pref: EncoderPreference,
    ) -> Result<Self> {
        let mut errors: Vec<String> = Vec::new();
        if pref != EncoderPreference::SoftwareOnly {
            let luid = d3d.adapter_luid;
            let mut luid_bytes = [0u8; 8];
            luid_bytes[..4].copy_from_slice(&luid.LowPart.to_le_bytes());
            luid_bytes[4..].copy_from_slice(&luid.HighPart.to_le_bytes());
            let hw = match enum_activates(HW_FLAGS, Some(&luid_bytes)) {
                Ok(v) if !v.is_empty() => v,
                _ => enum_activates(HW_FLAGS, None).unwrap_or_default(),
            };
            for a in hw {
                for gpu in [true, false] {
                    match Self::configure(a.clone(), true, gpu.then_some(manager), d3d, s) {
                        Ok(e) => return Ok(e),
                        Err(e) => errors.push(format!("{} (gpu={gpu}): {e}", friendly_name(&a))),
                    }
                }
            }
        }
        if pref != EncoderPreference::HardwareOnly {
            for a in enum_activates(SW_FLAGS, None).unwrap_or_default() {
                match Self::configure(a.clone(), false, None, d3d, s) {
                    Ok(e) => return Ok(e),
                    Err(e) => errors.push(format!("{}: {e}", friendly_name(&a))),
                }
            }
        }
        if errors.is_empty() {
            errors.push("no H.264 encoder MFT found".into());
        }
        Err(Error::Encoder(errors.join("; ")))
    }

    fn configure(
        activate: IMFActivate,
        hardware: bool,
        manager: Option<&IMFDXGIDeviceManager>,
        d3d: &D3dDevice,
        s: EncoderSettings,
    ) -> Result<Self> {
        let name = friendly_name(&activate);
        // SAFETY: activation of a registered MFT.
        let transform: IMFTransform = unsafe { activate.ActivateObject() }.ctx("IMFActivate::ActivateObject")?;
        let mut enc = Self {
            transform,
            activate,
            codec_api: None,
            events: None,
            info: EncoderInfo { name, hardware },
            gpu_input: false,
            profile: "",
            codec_settings: Vec::new(),
            in_id: 0,
            out_id: 0,
            provides_samples: false,
            out_buffer_size: 0,
            width: s.width,
            height: s.height,
            device: d3d.device.clone(),
            context: d3d.context.clone(),
            staging: None,
            params: ParameterSets::new(),
            shut_down: false,
        };
        // On error `enc` is dropped, which shuts the transform down properly.
        enc.setup(manager, s)?;
        Ok(enc)
    }

    fn setup(&mut self, manager: Option<&IMFDXGIDeviceManager>, s: EncoderSettings) -> Result<()> {
        let t = self.transform.clone();
        // SAFETY: COM calls on a freshly activated transform, in the order the MF docs require
        // (unlock → D3D manager → codec properties → output type → input type → start).
        unsafe {
            let attrs = t.GetAttributes().ok();
            let is_async = attrs.as_ref().and_then(|a| a.GetUINT32(&MF_TRANSFORM_ASYNC).ok()).unwrap_or(0) != 0;
            if let Some(a) = &attrs {
                if is_async {
                    a.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1).ctx("SetUINT32(MF_TRANSFORM_ASYNC_UNLOCK)")?;
                }
                let _ = a.SetUINT32(&MF_LOW_LATENCY, 1);
            }

            if let Some(m) = manager {
                let aware = attrs.as_ref().and_then(|a| a.GetUINT32(&MF_SA_D3D11_AWARE).ok()).unwrap_or(0) != 0;
                if !aware {
                    return Err(Error::Encoder("encoder is not D3D11 aware".into()));
                }
                t.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, m.as_raw() as usize)
                    .ctx("ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER)")?;
                self.gpu_input = true;
            }

            // Stream ids: E_NOTIMPL means "fixed, zero-based".
            let (mut i, mut o) = ([0u32], [0u32]);
            if t.GetStreamIDs(&mut i, &mut o).is_ok() {
                self.in_id = i[0];
                self.out_id = o[0];
            }

            self.codec_api = t.cast::<ICodecAPI>().ok();
            let bps = s.bitrate_kbps.saturating_mul(1000);
            self.set_codec("RateControlMode=CBR", &CODECAPI_AVEncCommonRateControlMode, variant_u32(eAVEncCommonRateControlMode_CBR.0 as u32));
            self.set_codec("MeanBitRate", &CODECAPI_AVEncCommonMeanBitRate, variant_u32(bps));
            self.set_codec("LowLatencyMode", &CODECAPI_AVLowLatencyMode, variant_bool(true));
            self.set_codec("GOPSize", &CODECAPI_AVEncMPVGOPSize, variant_u32(s.gop_frames));
            self.set_codec("BPictureCount=0", &CODECAPI_AVEncMPVDefaultBPictureCount, variant_u32(0));
            self.set_codec("QualityVsSpeed", &CODECAPI_AVEncCommonQualityVsSpeed, variant_u32(s.quality_vs_speed.min(100)));

            // Output type (must be set before the input type for encoders).
            let mut set = None;
            for (profile, label) in [(eAVEncH264VProfile_Main.0, "Main"), (eAVEncH264VProfile_Base.0, "Baseline")] {
                for with_color in [true, false] {
                    let mt = self.output_type(s, bps, profile as u32, with_color)?;
                    if t.SetOutputType(self.out_id, &mt, 0).is_ok() {
                        set = Some(label);
                        break;
                    }
                }
                if set.is_some() {
                    break;
                }
            }
            self.profile = set.ok_or_else(|| Error::Encoder("encoder rejected every H.264 output type".into()))?;

            self.set_input_type(s)?;

            let info = t.GetOutputStreamInfo(self.out_id).ctx("GetOutputStreamInfo")?;
            self.provides_samples = info.dwFlags
                & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0) as u32
                != 0;
            self.out_buffer_size = info.cbSize.max(s.width * s.height);

            // Parameter sets advertised on the output type (not every encoder fills this in).
            if let Ok(cur) = t.GetOutputCurrentType(self.out_id) {
                if let Ok(n) = cur.GetBlobSize(&MF_MT_MPEG_SEQUENCE_HEADER) {
                    let mut blob = vec![0u8; n as usize];
                    if n > 0 && cur.GetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &mut blob, None).is_ok() {
                        self.params.learn(&blob);
                    }
                }
            }

            if is_async {
                self.events = Some(t.cast::<IMFMediaEventGenerator>().ctx("QueryInterface(IMFMediaEventGenerator)")?);
            }
            t.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0).ctx("NOTIFY_BEGIN_STREAMING")?;
            t.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0).ctx("NOTIFY_START_OF_STREAM")?;
        }
        Ok(())
    }

    fn set_codec(&mut self, label: &str, api: &GUID, value: VARIANT) {
        let ok = match &self.codec_api {
            // SAFETY: valid GUID and VARIANT pointers for the duration of the call.
            Some(c) => unsafe { c.SetValue(api, &value) }.is_ok(),
            None => false,
        };
        self.codec_settings.push(format!("{label}:{}", if ok { "ok" } else { "rejected" }));
    }

    fn output_type(&self, s: EncoderSettings, bps: u32, profile: u32, with_color: bool) -> Result<IMFMediaType> {
        // SAFETY: populating a new media type.
        unsafe {
            let mt = MFCreateMediaType().ctx("MFCreateMediaType")?;
            mt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).ctx("MF_MT_MAJOR_TYPE")?;
            mt.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264).ctx("MF_MT_SUBTYPE")?;
            mt.SetUINT32(&MF_MT_AVG_BITRATE, bps).ctx("MF_MT_AVG_BITRATE")?;
            mt.SetUINT64(&MF_MT_FRAME_SIZE, pack(s.width, s.height)).ctx("MF_MT_FRAME_SIZE")?;
            mt.SetUINT64(&MF_MT_FRAME_RATE, pack(s.fps, 1)).ctx("MF_MT_FRAME_RATE")?;
            mt.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1)).ctx("MF_MT_PIXEL_ASPECT_RATIO")?;
            mt.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32).ctx("MF_MT_INTERLACE_MODE")?;
            mt.SetUINT32(&MF_MT_MPEG2_PROFILE, profile).ctx("MF_MT_MPEG2_PROFILE")?;
            if with_color {
                self.set_color_attrs(&mt);
            }
            Ok(mt)
        }
    }

    /// BT.709 limited range, matching the video processor output (written to the SPS VUI).
    fn set_color_attrs(&self, mt: &IMFMediaType) {
        // SAFETY: setting attributes on a media type we own.
        unsafe {
            let _ = mt.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32);
            let _ = mt.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0 as u32);
            let _ = mt.SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32);
        }
    }

    fn set_input_type(&self, s: EncoderSettings) -> Result<()> {
        let t = &self.transform;
        // SAFETY: populating / setting media types on our transform.
        unsafe {
            let mt = MFCreateMediaType().ctx("MFCreateMediaType")?;
            mt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).ctx("MF_MT_MAJOR_TYPE")?;
            mt.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12).ctx("MF_MT_SUBTYPE")?;
            mt.SetUINT64(&MF_MT_FRAME_SIZE, pack(s.width, s.height)).ctx("MF_MT_FRAME_SIZE")?;
            mt.SetUINT64(&MF_MT_FRAME_RATE, pack(s.fps, 1)).ctx("MF_MT_FRAME_RATE")?;
            mt.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1)).ctx("MF_MT_PIXEL_ASPECT_RATIO")?;
            mt.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32).ctx("MF_MT_INTERLACE_MODE")?;
            self.set_color_attrs(&mt);
            if t.SetInputType(self.in_id, &mt, 0).is_ok() {
                return Ok(());
            }
            // Fall back to the encoder's own NV12 type, completed with our size and rate.
            let mut idx = 0;
            while let Ok(avail) = t.GetInputAvailableType(self.in_id, idx) {
                idx += 1;
                if avail.GetGUID(&MF_MT_SUBTYPE).ok() != Some(MFVideoFormat_NV12) {
                    continue;
                }
                let _ = avail.SetUINT64(&MF_MT_FRAME_SIZE, pack(s.width, s.height));
                let _ = avail.SetUINT64(&MF_MT_FRAME_RATE, pack(s.fps, 1));
                if t.SetInputType(self.in_id, &avail, 0).is_ok() {
                    return Ok(());
                }
            }
        }
        Err(Error::Encoder("encoder rejected NV12 input".into()))
    }

    pub fn is_async(&self) -> bool {
        self.events.is_some()
    }

    /// Create the event callback for an async encoder. `sink(event_type, status)` is called on
    /// an MF thread for every event; call [`Self::request_event`] once per handled event.
    pub fn event_forwarder(&self, sink: impl Fn(u32, HRESULT) + 'static) -> Option<IMFAsyncCallback> {
        let generator = self.events.clone()?;
        Some(EventForwarder { generator, sink: Box::new(sink) }.into())
    }

    /// Ask for the next event (async encoders only).
    pub fn request_event(&self, cb: &IMFAsyncCallback) -> Result<()> {
        if let Some(g) = &self.events {
            // SAFETY: one outstanding request at a time (the pipeline guarantees it).
            unsafe { g.BeginGetEvent(cb, None) }.ctx("IMFMediaEventGenerator::BeginGetEvent")?;
        }
        Ok(())
    }

    /// Make the next encoded frame an IDR.
    pub fn force_keyframe(&self) -> bool {
        match &self.codec_api {
            // SAFETY: valid pointers for the call.
            Some(c) => unsafe { c.SetValue(&CODECAPI_AVEncVideoForceKeyFrame, &variant_u32(1)) }.is_ok(),
            None => false,
        }
    }

    /// Change the CBR target while streaming (`CODECAPI_AVEncCommonMeanBitRate` is a dynamic
    /// property for H.264 encoders). Returns false when the encoder rejects the change.
    pub fn set_bitrate(&self, kbps: u32) -> bool {
        match &self.codec_api {
            // SAFETY: valid pointers for the call.
            Some(c) => unsafe {
                c.SetValue(&CODECAPI_AVEncCommonMeanBitRate, &variant_u32(kbps.saturating_mul(1000)))
            }
            .is_ok(),
            None => false,
        }
    }

    /// Wrap an NV12 texture in an `IMFSample`. With GPU input the sample references the
    /// texture (it must not be overwritten until the matching output arrives); otherwise the
    /// pixels are copied to system memory and the texture is free again on return.
    pub fn make_sample(&mut self, nv12: &ID3D11Texture2D, time: i64, duration: i64) -> Result<IMFSample> {
        // SAFETY: MF / D3D calls on valid objects; mapped memory is read only while mapped and
        // the destination buffer is sized for the full NV12 frame.
        unsafe {
            let sample = MFCreateSample().ctx("MFCreateSample")?;
            let buffer = if self.gpu_input {
                let b = MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, nv12, 0, false)
                    .ctx("MFCreateDXGISurfaceBuffer")?;
                if let Ok(b2) = b.cast::<IMF2DBuffer>() {
                    if let Ok(len) = b2.GetContiguousLength() {
                        let _ = b.SetCurrentLength(len);
                    }
                }
                b
            } else {
                self.copy_to_memory(nv12)?
            };
            sample.AddBuffer(&buffer).ctx("IMFSample::AddBuffer")?;
            sample.SetSampleTime(time).ctx("SetSampleTime")?;
            sample.SetSampleDuration(duration).ctx("SetSampleDuration")?;
            Ok(sample)
        }
    }

    unsafe fn copy_to_memory(&mut self, nv12: &ID3D11Texture2D) -> Result<IMFMediaBuffer> {
        let (w, h) = (self.width as usize, self.height as usize);
        if self.staging.is_none() {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: self.width,
                Height: self.height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_NV12,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
            };
            let mut tex = None;
            // SAFETY: valid descriptor.
            unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut tex)) }.ctx("CreateTexture2D(staging NV12)")?;
            self.staging = tex;
        }
        let staging = self.staging.as_ref().expect("created above");
        let size = w * h * 3 / 2;
        // SAFETY: see make_sample.
        unsafe {
            self.context.CopyResource(staging, nv12);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).ctx("Map(staging NV12)")?;
            let pitch = mapped.RowPitch as usize;
            let src = mapped.pData as *const u8;
            let result = (|| -> Result<IMFMediaBuffer> {
                let buf = MFCreateMemoryBuffer(size as u32).ctx("MFCreateMemoryBuffer")?;
                let mut dst: *mut u8 = std::ptr::null_mut();
                buf.Lock(&mut dst, None, None).ctx("IMFMediaBuffer::Lock")?;
                for r in 0..h {
                    std::ptr::copy_nonoverlapping(src.add(r * pitch), dst.add(r * w), w);
                }
                let uv_src = src.add(pitch * h);
                let uv_dst = dst.add(w * h);
                for r in 0..h / 2 {
                    std::ptr::copy_nonoverlapping(uv_src.add(r * pitch), uv_dst.add(r * w), w);
                }
                let _ = buf.Unlock();
                buf.SetCurrentLength(size as u32).ctx("SetCurrentLength")?;
                Ok(buf)
            })();
            self.context.Unmap(staging, 0);
            result
        }
    }

    /// Feed one sample. `Ok(false)` = the encoder is not accepting input right now
    /// (`MF_E_NOTACCEPTING`): drain output and retry.
    pub fn process_input(&self, sample: &IMFSample) -> Result<bool> {
        // SAFETY: valid sample.
        match unsafe { self.transform.ProcessInput(self.in_id, sample, 0) } {
            Ok(()) => Ok(true),
            Err(e) if e.code() == MF_E_NOTACCEPTING => Ok(false),
            Err(e) => Err(Error::Windows { context: "IMFTransform::ProcessInput", source: e }),
        }
    }

    /// Pull one output sample.
    pub fn process_output(&mut self) -> Result<OutputResult> {
        // SAFETY: MFT_OUTPUT_DATA_BUFFER ownership: we hand in (optionally) our sample and take
        // back whatever is in the struct afterwards, exactly once, via ManuallyDrop::take.
        unsafe {
            let ours = if self.provides_samples {
                None
            } else {
                let s = MFCreateSample().ctx("MFCreateSample")?;
                let b = MFCreateMemoryBuffer(self.out_buffer_size).ctx("MFCreateMemoryBuffer")?;
                s.AddBuffer(&b).ctx("AddBuffer")?;
                Some(s)
            };
            let mut db = MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: self.out_id,
                pSample: ManuallyDrop::new(ours),
                dwStatus: 0,
                pEvents: ManuallyDrop::new(None),
            };
            let mut status = 0u32;
            let r = self.transform.ProcessOutput(0, std::slice::from_mut(&mut db), &mut status);
            let sample = ManuallyDrop::take(&mut db.pSample);
            drop(ManuallyDrop::take(&mut db.pEvents));
            match r {
                Ok(()) => {}
                Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(OutputResult::NeedMoreInput),
                Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    self.renegotiate_output()?;
                    return Ok(OutputResult::StreamChanged);
                }
                Err(e) => return Err(Error::Windows { context: "IMFTransform::ProcessOutput", source: e }),
            }
            let sample = sample.ok_or_else(|| Error::Encoder("ProcessOutput returned no sample".into()))?;
            let time = sample.GetSampleTime().unwrap_or(0);
            let clean = sample.GetUINT32(&MFSampleExtension_CleanPoint).unwrap_or(0) != 0;
            let buf = sample.ConvertToContiguousBuffer().ctx("ConvertToContiguousBuffer")?;
            let mut p: *mut u8 = std::ptr::null_mut();
            let mut len = 0u32;
            buf.Lock(&mut p, None, Some(&mut len)).ctx("IMFMediaBuffer::Lock")?;
            let raw = std::slice::from_raw_parts(p, len as usize).to_vec();
            let _ = buf.Unlock();
            let data = self.params.fix_access_unit(raw);
            let has_slices = annexb::nal_units(&data).any(|n| matches!(n.nal_type(), 1 | 5));
            let keyframe = if has_slices { annexb::is_keyframe(&data) } else { clean };
            Ok(OutputResult::Frame(EncodedFrame { data, sample_time: time, keyframe }))
        }
    }

    fn renegotiate_output(&mut self) -> Result<()> {
        // SAFETY: COM calls on our transform.
        unsafe {
            let mt = self.transform.GetOutputAvailableType(self.out_id, 0).ctx("GetOutputAvailableType")?;
            self.transform.SetOutputType(self.out_id, &mt, 0).ctx("SetOutputType(renegotiate)")?;
            let info = self.transform.GetOutputStreamInfo(self.out_id).ctx("GetOutputStreamInfo")?;
            self.provides_samples = info.dwFlags
                & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0) as u32
                != 0;
            self.out_buffer_size = info.cbSize.max(self.width * self.height);
        }
        Ok(())
    }

    /// End the stream and release the MFT. Called by Drop; safe to call twice.
    pub fn shutdown(&mut self) {
        if self.shut_down {
            return;
        }
        self.shut_down = true;
        // SAFETY: COM calls on our transform; errors are irrelevant at this point.
        unsafe {
            let _ = self.transform.ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            let _ = self.transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
            let _ = self.transform.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
            if let Ok(sd) = self.transform.cast::<IMFShutdown>() {
                let _ = sd.Shutdown();
            }
            let _ = self.activate.ShutdownObject();
        }
    }
}

impl Drop for H264Encoder {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Create the DXGI device manager that shares `d3d` with Media Foundation.
pub fn create_device_manager(d3d: &D3dDevice) -> Result<IMFDXGIDeviceManager> {
    let mut token = 0u32;
    let mut mgr: Option<IMFDXGIDeviceManager> = None;
    // SAFETY: out-pointers to locals; ResetDevice with the token MF just gave us.
    unsafe {
        MFCreateDXGIDeviceManager(&mut token, &mut mgr).ctx("MFCreateDXGIDeviceManager")?;
        let mgr = mgr.ok_or_else(|| Error::Encoder("MFCreateDXGIDeviceManager returned null".into()))?;
        mgr.ResetDevice(&d3d.device, token).ctx("IMFDXGIDeviceManager::ResetDevice")?;
        Ok(mgr)
    }
}
