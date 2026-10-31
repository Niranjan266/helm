//! Live test: encode 2 s of synthetic NV12 frames with the Microsoft software H.264 MFT
//! (same Annex-B output shape as helm-win-capture), record them plus a 440 Hz tone, then open
//! the MP4 with MFCreateSourceReaderFromURL (`probe`) and check both streams.

#![cfg(windows)]

use std::mem::ManuallyDrop;

use helm_win_record::{Recorder, StreamInfo, VideoConfig, probe};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};

const W: u32 = 320;
const H: u32 = 240;
const FPS: u32 = 30;

fn pack(a: u32, b: u32) -> u64 {
    ((a as u64) << 32) | b as u64
}

struct Enc {
    mft: IMFTransform,
    out_size: u32,
}

impl Enc {
    fn new() -> Enc {
        unsafe {
            let mft: IMFTransform = CoCreateInstance(&CLSID_MSH264EncoderMFT, None, CLSCTX_INPROC_SERVER).unwrap();
            let ot = MFCreateMediaType().unwrap();
            ot.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).unwrap();
            ot.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264).unwrap();
            ot.SetUINT32(&MF_MT_AVG_BITRATE, 1_000_000).unwrap();
            ot.SetUINT64(&MF_MT_FRAME_SIZE, pack(W, H)).unwrap();
            ot.SetUINT64(&MF_MT_FRAME_RATE, pack(FPS, 1)).unwrap();
            ot.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1)).unwrap();
            ot.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32).unwrap();
            ot.SetUINT32(&MF_MT_MAX_KEYFRAME_SPACING, FPS).unwrap();
            mft.SetOutputType(0, &ot, 0).unwrap();
            let it = MFCreateMediaType().unwrap();
            it.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).unwrap();
            it.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12).unwrap();
            it.SetUINT64(&MF_MT_FRAME_SIZE, pack(W, H)).unwrap();
            it.SetUINT64(&MF_MT_FRAME_RATE, pack(FPS, 1)).unwrap();
            it.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1)).unwrap();
            it.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32).unwrap();
            mft.SetInputType(0, &it, 0).unwrap();
            let out_size = mft.GetOutputStreamInfo(0).unwrap().cbSize.max(W * H * 2);
            mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0).unwrap();
            mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0).unwrap();
            Enc { mft, out_size }
        }
    }

    fn input(&self, frame: u32) {
        let mut data = vec![0u8; (W * H * 3 / 2) as usize];
        for y in 0..H {
            for x in 0..W {
                data[(y * W + x) as usize] = ((x + y + frame * 4) % 220 + 16) as u8;
            }
        }
        for v in &mut data[(W * H) as usize..] {
            *v = 128;
        }
        unsafe {
            let buf = MFCreateMemoryBuffer(data.len() as u32).unwrap();
            let mut p = std::ptr::null_mut();
            buf.Lock(&mut p, None, None).unwrap();
            std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
            buf.Unlock().unwrap();
            buf.SetCurrentLength(data.len() as u32).unwrap();
            let s = MFCreateSample().unwrap();
            s.AddBuffer(&buf).unwrap();
            s.SetSampleTime(frame as i64 * 10_000_000 / FPS as i64).unwrap();
            s.SetSampleDuration(10_000_000 / FPS as i64).unwrap();
            self.mft.ProcessInput(0, &s, 0).unwrap();
        }
    }

    /// (annex-b bytes, keyframe, time in 100 ns)
    fn outputs(&self) -> Vec<(Vec<u8>, bool, i64)> {
        let mut v = Vec::new();
        unsafe {
            loop {
                let s = MFCreateSample().unwrap();
                s.AddBuffer(&MFCreateMemoryBuffer(self.out_size).unwrap()).unwrap();
                let mut db = MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: ManuallyDrop::new(Some(s)),
                    dwStatus: 0,
                    pEvents: ManuallyDrop::new(None),
                };
                let mut status = 0;
                let r = self.mft.ProcessOutput(0, std::slice::from_mut(&mut db), &mut status);
                let sample = ManuallyDrop::take(&mut db.pSample);
                drop(ManuallyDrop::take(&mut db.pEvents));
                match r {
                    Ok(()) => {}
                    Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => break,
                    Err(e) => panic!("ProcessOutput: {e}"),
                }
                let sample = sample.unwrap();
                let key = sample.GetUINT32(&MFSampleExtension_CleanPoint).unwrap_or(0) != 0;
                let t = sample.GetSampleTime().unwrap_or(0);
                let b = sample.ConvertToContiguousBuffer().unwrap();
                let mut p = std::ptr::null_mut();
                let mut len = 0;
                b.Lock(&mut p, None, Some(&mut len)).unwrap();
                let data = std::slice::from_raw_parts(p, len as usize).to_vec();
                b.Unlock().unwrap();
                v.push((data, key, t));
            }
        }
        v
    }
}

#[test]
fn records_two_seconds_and_source_reader_sees_both_streams() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_FULL).unwrap();
    }
    let dir = std::env::temp_dir().join("helm-win-record-test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("two_seconds.mp4");
    let _ = std::fs::remove_file(&path);

    let rec = Recorder::start(&path, VideoConfig { width: W, height: H, fps: FPS }, true).unwrap();
    let enc = Enc::new();
    let base_us = 5_000_000u64; // arbitrary clock origin
    let mut pushed = 0;
    let mut push = |aus: Vec<(Vec<u8>, bool, i64)>| {
        for (au, key, t) in aus {
            rec.push_video(&au, key, base_us + (t / 10) as u64).unwrap();
            pushed += 1;
        }
    };
    let total_frames = FPS * 2;
    let mut audio_frames_sent = 0u64;
    for f in 0..total_frames {
        enc.input(f);
        push(enc.outputs());
        // 1/30 s of a 440 Hz tone = 1600 stereo frames.
        let n = 48_000 / FPS as usize;
        let mut pcm = Vec::with_capacity(n * 2);
        for i in 0..n {
            let t = (audio_frames_sent + i as u64) as f32 / 48_000.0;
            let v = (t * 440.0 * std::f32::consts::TAU).sin() * 0.3;
            pcm.push(v);
            pcm.push(v);
        }
        rec.push_audio(&pcm, base_us + audio_frames_sent * 1_000_000 / 48_000).unwrap();
        audio_frames_sent += n as u64;
    }
    unsafe { enc.mft.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0).unwrap() };
    push(enc.outputs());

    let info = rec.stop().unwrap();
    println!("{info:?}");
    assert!(pushed >= total_frames as usize - 2, "encoder produced {pushed} AUs");
    assert_eq!(info.video_dropped, 0);
    assert!(info.video_frames >= total_frames as u64 - 2);
    assert!(info.audio_frames > 90_000);
    assert!((1900..=2100).contains(&info.duration_ms), "duration {}", info.duration_ms);
    assert!(rec.push_video(&[0, 0, 1, 0x65], true, 0).is_err(), "push after stop must fail");

    let media = probe(&path).unwrap();
    println!("{media:?}");
    assert!((1800..=2200).contains(&media.duration_ms), "probed duration {}", media.duration_ms);
    assert!(media.streams.iter().any(|s| matches!(s, StreamInfo::Video { codec, width, height }
        if codec == "h264" && *width == W && *height == H)));
    assert!(media.streams.iter().any(|s| matches!(s, StreamInfo::Audio { codec, sample_rate, channels }
        if codec == "aac" && *sample_rate == 48_000 && *channels == 2)));
}

#[test]
fn stop_without_keyframe_is_no_video() {
    let path = std::env::temp_dir().join("helm-win-record-empty.mp4");
    let rec = Recorder::start(&path, VideoConfig { width: 64, height: 64, fps: 30 }, false).unwrap();
    rec.push_video(&[0, 0, 1, 0x41, 1, 2], false, 0).unwrap();
    assert!(matches!(rec.stop(), Err(helm_win_record::Error::NoVideo)));
}
