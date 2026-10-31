//! Media Foundation sink-writer worker.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::core::HSTRING;

use crate::pure::{self, AUDIO_CHANNELS, AUDIO_RATE, RecordingInfo, Timeline, VideoConfig};
use crate::{Error, Result};

pub(crate) fn win_err(context: &'static str, e: windows::core::Error) -> Error {
    Error::Windows { context, code: e.code().0, message: e.message() }
}

pub(crate) trait Ctx<T> {
    fn ctx(self, c: &'static str) -> Result<T>;
}
impl<T> Ctx<T> for windows::core::Result<T> {
    fn ctx(self, c: &'static str) -> Result<T> {
        self.map_err(|e| win_err(c, e))
    }
}

/// Queue depth between the caller and the writer thread. `push_*` blocks when it is full.
const QUEUE: usize = 512;
/// AAC bitrate: 24 000 bytes/s = 192 kbit/s (one of the four rates the MF AAC encoder accepts).
const AAC_BYTES_PER_SEC: u32 = 24_000;

enum Msg {
    Video { au: Vec<u8>, key: bool, pts_us: u64 },
    Audio { pcm: Vec<i16>, pts_us: u64 },
    Stop(SyncSender<Result<RecordingInfo>>),
}

/// An MP4 recording in progress. `Send + Sync`: video and audio may be pushed from different
/// threads. Dropping it without [`Recorder::stop`] still finalizes the file.
pub struct Recorder {
    tx: Mutex<Option<SyncSender<Msg>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
    failure: Arc<Mutex<Option<String>>>,
    audio: bool,
}

impl Recorder {
    /// Start a recording to `path` (created / overwritten; the parent directory must exist).
    /// The file is opened when the first keyframe arrives (its SPS/PPS describe the stream);
    /// access units before it are dropped. Errors in the setup are returned by the first
    /// `push_video` with a keyframe, or by `stop`.
    pub fn start(path: impl AsRef<Path>, video: VideoConfig, audio: bool) -> Result<Recorder> {
        video.validate()?;
        let path = path.as_ref().to_path_buf();
        if path.as_os_str().is_empty() {
            return Err(Error::InvalidInput("empty output path".into()));
        }
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            if !parent.is_dir() {
                return Err(Error::InvalidInput(format!("directory does not exist: {}", parent.display())));
            }
        }
        let (tx, rx) = sync_channel(QUEUE);
        let failure: Arc<Mutex<Option<String>>> = Arc::default();
        let f = failure.clone();
        let thread = std::thread::Builder::new()
            .name("helm-record".into())
            .spawn(move || worker(rx, path, video, audio, f))
            .map_err(|e| Error::Stopped(format!("spawn: {e}")))?;
        Ok(Recorder { tx: Mutex::new(Some(tx)), thread: Mutex::new(Some(thread)), failure, audio })
    }

    fn send(&self, m: Msg) -> Result<()> {
        if let Some(f) = self.failure.lock().unwrap_or_else(|p| p.into_inner()).clone() {
            return Err(Error::Stopped(f));
        }
        let tx = self.tx.lock().unwrap_or_else(|p| p.into_inner()).clone();
        match tx {
            Some(tx) => tx.send(m).map_err(|_| Error::Stopped("writer thread ended".into())),
            None => Err(Error::Stopped("already stopped".into())),
        }
    }

    /// One H.264 Annex-B access unit. `pts_us` may start anywhere; keyframes must carry SPS/PPS.
    pub fn push_video(&self, au: &[u8], is_key: bool, pts_us: u64) -> Result<()> {
        if au.is_empty() {
            return Ok(());
        }
        self.send(Msg::Video { au: au.to_vec(), key: is_key, pts_us })
    }

    /// Interleaved stereo f32 samples at 48 kHz (`samples.len()` even). Same clock as video.
    /// Ignored (Ok) when the recorder was started without audio.
    pub fn push_audio(&self, samples: &[f32], pts_us: u64) -> Result<()> {
        if !self.audio || samples.is_empty() {
            return Ok(());
        }
        if samples.len() % AUDIO_CHANNELS as usize != 0 {
            return Err(Error::InvalidInput("audio must be interleaved stereo (even sample count)".into()));
        }
        let mut pcm = Vec::new();
        pure::f32_to_i16(samples, &mut pcm);
        self.send(Msg::Audio { pcm, pts_us })
    }

    /// Finalize the MP4 and return its path and duration. Further pushes fail with `Stopped`.
    pub fn stop(&self) -> Result<RecordingInfo> {
        let tx = self.tx.lock().unwrap_or_else(|p| p.into_inner()).take();
        let Some(tx) = tx else {
            return Err(Error::Stopped("already stopped".into()));
        };
        let (rtx, rrx) = sync_channel(1);
        let sent = tx.send(Msg::Stop(rtx)).is_ok();
        drop(tx);
        let result = if sent { rrx.recv().ok() } else { None };
        if let Some(t) = self.thread.lock().unwrap_or_else(|p| p.into_inner()).take() {
            let _ = t.join();
        }
        result.unwrap_or_else(|| Err(Error::Stopped("writer thread ended unexpectedly".into())))
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        if self.tx.get_mut().map(|t| t.is_some()).unwrap_or(false) {
            let _ = self.stop();
        }
    }
}

struct Writer {
    sink: IMFSinkWriter,
    video_index: u32,
    audio_index: Option<u32>,
}

struct State {
    path: PathBuf,
    video: VideoConfig,
    audio: bool,
    timeline: Timeline,
    writer: Option<Writer>,
    error: Option<Error>,
    video_frames: u64,
    video_dropped: u64,
    audio_frames: u64,
    end_hns: i64,
    /// Audio that arrived before the first keyframe opened the file (bounded, ~4 s).
    pending_audio: std::collections::VecDeque<(Vec<i16>, u64)>,
}

/// Cap for `pending_audio`, in chunks (10–20 ms each in practice).
const PENDING_AUDIO_CHUNKS: usize = 400;

fn worker(rx: Receiver<Msg>, path: PathBuf, video: VideoConfig, audio: bool, failure: Arc<Mutex<Option<String>>>) {
    // SAFETY: COM / MF init for this thread, balanced below.
    let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
    let mf = unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) };
    let mut st = State {
        path,
        video,
        audio,
        timeline: Timeline::new(),
        writer: None,
        error: mf.err().map(|e| win_err("MFStartup", e)),
        video_frames: 0,
        video_dropped: 0,
        audio_frames: 0,
        end_hns: 0,
        pending_audio: Default::default(),
    };
    let mut reply: Option<SyncSender<Result<RecordingInfo>>> = None;
    for m in rx.iter() {
        match m {
            Msg::Stop(r) => {
                reply = Some(r);
                break;
            }
            _ if st.error.is_some() => {}
            m => {
                if let Err(e) = st.handle(m) {
                    *failure.lock().unwrap_or_else(|p| p.into_inner()) = Some(e.to_string());
                    st.error = Some(e);
                }
            }
        }
    }
    let result = st.finish();
    if let Some(r) = reply {
        let _ = r.send(result);
    }
    drop(st);
    unsafe {
        let _ = MFShutdown();
        if com {
            CoUninitialize();
        }
    }
}

impl State {
    fn handle(&mut self, m: Msg) -> Result<()> {
        match m {
            Msg::Video { au, key, pts_us } => {
                let key = key || pure::has_idr(&au);
                if !self.timeline.started() && !key {
                    self.video_dropped += 1;
                    return Ok(());
                }
                if self.writer.is_none() {
                    let header = pure::sequence_header(&au).ok_or_else(|| {
                        Error::InvalidInput("first keyframe has no SPS/PPS (needed for the MP4 header)".into())
                    })?;
                    self.writer = Some(open_writer(&self.path, self.video, self.audio, &header)?);
                    let t = self.timeline.video(pts_us, key).expect("keyframe starts the timeline");
                    self.write_video(&au, key, t)?;
                    for (pcm, apts) in std::mem::take(&mut self.pending_audio) {
                        self.write_audio(&pcm, apts)?;
                    }
                    return Ok(());
                }
                let Some(t) = self.timeline.video(pts_us, key) else {
                    self.video_dropped += 1;
                    return Ok(());
                };
                self.write_video(&au, key, t)?;
            }
            Msg::Audio { pcm, pts_us } => {
                if self.writer.is_none() {
                    if self.pending_audio.len() == PENDING_AUDIO_CHUNKS {
                        self.pending_audio.pop_front();
                    }
                    self.pending_audio.push_back((pcm, pts_us));
                    return Ok(());
                }
                self.write_audio(&pcm, pts_us)?;
            }
            Msg::Stop(_) => unreachable!(),
        }
        Ok(())
    }

    fn write_video(&mut self, au: &[u8], key: bool, t: i64) -> Result<()> {
        let dur = self.video.frame_duration_hns();
        let w = self.writer.as_ref().expect("writer");
        let sample = make_sample(au, t, dur)?;
        unsafe {
            sample.SetUINT32(&MFSampleExtension_CleanPoint, key as u32).ctx("SetUINT32(CleanPoint)")?;
            w.sink.WriteSample(w.video_index, &sample).ctx("IMFSinkWriter::WriteSample(video)")?;
        }
        self.video_frames += 1;
        self.end_hns = self.end_hns.max(t + dur);
        Ok(())
    }

    /// Writes one PCM chunk; chunks from before the first keyframe are dropped.
    fn write_audio(&mut self, pcm: &[i16], pts_us: u64) -> Result<()> {
        let (Some(w), Some(t)) = (self.writer.as_ref(), self.timeline.audio(pts_us)) else {
            return Ok(());
        };
        let Some(idx) = w.audio_index else { return Ok(()) };
        let frames = (pcm.len() / AUDIO_CHANNELS as usize) as u64;
        let dur = pure::audio_duration_hns(frames);
        // SAFETY: i16 slice viewed as bytes.
        let bytes = unsafe { std::slice::from_raw_parts(pcm.as_ptr().cast::<u8>(), pcm.len() * 2) };
        let sample = make_sample(bytes, t, dur)?;
        unsafe { w.sink.WriteSample(idx, &sample) }.ctx("IMFSinkWriter::WriteSample(audio)")?;
        self.audio_frames += frames;
        self.end_hns = self.end_hns.max(t + dur);
        Ok(())
    }

    fn finish(&mut self) -> Result<RecordingInfo> {
        let writer = self.writer.take();
        let fin = match &writer {
            Some(w) => unsafe { w.sink.Finalize() }.ctx("IMFSinkWriter::Finalize"),
            None => Err(Error::NoVideo),
        };
        drop(writer);
        if let Some(e) = self.error.take() {
            return Err(e);
        }
        fin?;
        Ok(RecordingInfo {
            path: self.path.to_string_lossy().into_owned(),
            duration_ms: (self.end_hns.max(0) as u64) / 10_000,
            video_frames: self.video_frames,
            audio_frames: self.audio_frames,
            video_dropped: self.video_dropped,
        })
    }
}

fn make_sample(data: &[u8], time: i64, duration: i64) -> Result<IMFSample> {
    unsafe {
        let buf = MFCreateMemoryBuffer(data.len() as u32).ctx("MFCreateMemoryBuffer")?;
        let mut p: *mut u8 = std::ptr::null_mut();
        buf.Lock(&mut p, None, None).ctx("IMFMediaBuffer::Lock")?;
        std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
        buf.Unlock().ctx("IMFMediaBuffer::Unlock")?;
        buf.SetCurrentLength(data.len() as u32).ctx("SetCurrentLength")?;
        let s = MFCreateSample().ctx("MFCreateSample")?;
        s.AddBuffer(&buf).ctx("IMFSample::AddBuffer")?;
        s.SetSampleTime(time).ctx("SetSampleTime")?;
        s.SetSampleDuration(duration).ctx("SetSampleDuration")?;
        Ok(s)
    }
}

fn pack(hi: u32, lo: u32) -> u64 {
    ((hi as u64) << 32) | lo as u64
}

fn open_writer(path: &Path, v: VideoConfig, audio: bool, seq_header: &[u8]) -> Result<Writer> {
    unsafe {
        let mut attrs: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attrs, 2).ctx("MFCreateAttributes")?;
        let attrs = attrs.expect("attributes");
        // The caller paces the samples (live capture); never block WriteSample on the encoder.
        attrs.SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1).ctx("SetUINT32(DISABLE_THROTTLING)")?;
        attrs.SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4).ctx("SetGUID(CONTAINERTYPE)")?;
        let url = HSTRING::from(path.as_os_str());
        let sink = MFCreateSinkWriterFromURL(&url, None, &attrs).ctx("MFCreateSinkWriterFromURL")?;

        // Video: H.264 passthrough (input type = output type).
        let vt = MFCreateMediaType().ctx("MFCreateMediaType")?;
        vt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).ctx("video MAJOR_TYPE")?;
        vt.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264).ctx("video SUBTYPE")?;
        vt.SetUINT64(&MF_MT_FRAME_SIZE, pack(v.width, v.height)).ctx("FRAME_SIZE")?;
        vt.SetUINT64(&MF_MT_FRAME_RATE, pack(v.fps, 1)).ctx("FRAME_RATE")?;
        vt.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1)).ctx("PIXEL_ASPECT_RATIO")?;
        vt.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32).ctx("INTERLACE_MODE")?;
        vt.SetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, seq_header).ctx("MPEG_SEQUENCE_HEADER")?;
        let video_index = sink.AddStream(&vt).ctx("AddStream(video)")?;
        sink.SetInputMediaType(video_index, &vt, None).ctx("SetInputMediaType(video)")?;

        let audio_index = if audio {
            let at = MFCreateMediaType().ctx("MFCreateMediaType")?;
            at.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).ctx("audio MAJOR_TYPE")?;
            at.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC).ctx("audio SUBTYPE")?;
            at.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16).ctx("BITS_PER_SAMPLE")?;
            at.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, AUDIO_RATE).ctx("SAMPLES_PER_SECOND")?;
            at.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, AUDIO_CHANNELS).ctx("NUM_CHANNELS")?;
            at.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, AAC_BYTES_PER_SEC).ctx("AVG_BYTES_PER_SECOND")?;
            let idx = sink.AddStream(&at).ctx("AddStream(audio)")?;

            let it = MFCreateMediaType().ctx("MFCreateMediaType")?;
            it.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).ctx("pcm MAJOR_TYPE")?;
            it.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM).ctx("pcm SUBTYPE")?;
            it.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16).ctx("pcm BITS_PER_SAMPLE")?;
            it.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, AUDIO_RATE).ctx("pcm SAMPLES_PER_SECOND")?;
            it.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, AUDIO_CHANNELS).ctx("pcm NUM_CHANNELS")?;
            it.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, AUDIO_CHANNELS * 2).ctx("pcm BLOCK_ALIGNMENT")?;
            it.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, AUDIO_RATE * AUDIO_CHANNELS * 2).ctx("pcm AVG_BYTES")?;
            it.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1).ctx("pcm ALL_SAMPLES_INDEPENDENT")?;
            sink.SetInputMediaType(idx, &it, None).ctx("SetInputMediaType(audio PCM -> AAC)")?;
            Some(idx)
        } else {
            None
        };

        sink.BeginWriting().ctx("IMFSinkWriter::BeginWriting")?;
        Ok(Writer { sink, video_index, audio_index })
    }
}
