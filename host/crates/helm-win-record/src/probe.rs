//! Inspect a media file with the Media Foundation source reader (used to verify recordings).

use std::path::Path;

use serde::{Deserialize, Serialize};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::Variant::VT_UI8;
use windows::core::{GUID, HSTRING};

use crate::Result;
use crate::win::Ctx;

/// One stream of a media file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StreamInfo {
    Video { codec: String, width: u32, height: u32 },
    Audio { codec: String, sample_rate: u32, channels: u32 },
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaInfo {
    pub duration_ms: u64,
    pub streams: Vec<StreamInfo>,
}

fn codec_name(g: GUID) -> String {
    match g {
        x if x == MFVideoFormat_H264 => "h264".into(),
        x if x == MFVideoFormat_HEVC => "hevc".into(),
        x if x == MFAudioFormat_AAC => "aac".into(),
        x if x == MFAudioFormat_PCM => "pcm".into(),
        x if x == MFAudioFormat_Float => "pcm_f32".into(),
        other => format!("{other:?}"),
    }
}

/// Open `path` with `MFCreateSourceReaderFromURL` and list its streams and duration.
/// Runs on a short-lived thread of its own (COM MTA + MFStartup), so it can be called anywhere.
pub fn probe(path: impl AsRef<Path>) -> Result<MediaInfo> {
    let path = path.as_ref().to_path_buf();
    std::thread::spawn(move || {
        let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
        let r = unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }.ctx("MFStartup").and_then(|()| {
            let r = probe_inner(&path);
            unsafe {
                let _ = MFShutdown();
            }
            r
        });
        if com {
            unsafe { CoUninitialize() };
        }
        r
    })
    .join()
    .unwrap_or_else(|_| Err(crate::Error::Stopped("probe thread panicked".into())))
}

fn probe_inner(path: &Path) -> Result<MediaInfo> {
    unsafe {
        let reader = MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), None)
            .ctx("MFCreateSourceReaderFromURL")?;
        let mut streams = Vec::new();
        for i in 0u32.. {
            let mt = match reader.GetNativeMediaType(i, 0) {
                Ok(mt) => mt,
                Err(e) if e.code() == MF_E_INVALIDSTREAMNUMBER => break,
                Err(e) => return Err(crate::win::win_err("GetNativeMediaType", e)),
            };
            let major = mt.GetGUID(&MF_MT_MAJOR_TYPE).ctx("MAJOR_TYPE")?;
            let sub = mt.GetGUID(&MF_MT_SUBTYPE).unwrap_or_default();
            streams.push(if major == MFMediaType_Video {
                let size = mt.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(0);
                StreamInfo::Video { codec: codec_name(sub), width: (size >> 32) as u32, height: size as u32 }
            } else if major == MFMediaType_Audio {
                StreamInfo::Audio {
                    codec: codec_name(sub),
                    sample_rate: mt.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND).unwrap_or(0),
                    channels: mt.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).unwrap_or(0),
                }
            } else {
                StreamInfo::Other
            });
        }
        let pv = reader
            .GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION)
            .ctx("GetPresentationAttribute(MF_PD_DURATION)")?;
        // MF_PD_DURATION is VT_UI8 (100 ns units); no heap data, nothing to clear.
        let inner = &pv.Anonymous.Anonymous;
        let hns = if inner.vt == VT_UI8 { inner.Anonymous.uhVal } else { 0 };
        Ok(MediaInfo { duration_ms: hns / 10_000, streams })
    }
}
