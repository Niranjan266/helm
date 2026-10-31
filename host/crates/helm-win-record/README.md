# helm-win-record

MP4 recording for the Helm PC agent. H.264 Annex-B access units from `helm-win-capture` are
**passed through** (not re-encoded); optional interleaved stereo f32 PCM at 48 kHz is encoded to
AAC (192 kbit/s) by Media Foundation. Written with the MF sink writer (`MFCreateSinkWriterFromURL`,
MPEG-4 container, throttling disabled). Clean-room, from the Microsoft Learn documentation only.

```rust
use helm_win_record::{Recorder, VideoConfig, probe};
let rec = Recorder::start(r"C:\Users\me\Videos\helm.mp4", VideoConfig { width: 1920, height: 1080, fps: 60 }, true)?;
rec.push_video(au, is_key, pts_us)?;   // Streamer callback
rec.push_audio(&pcm_f32, pts_us)?;      // audio capture, same clock
let info = rec.stop()?;                  // RecordingInfo { path, duration_ms, video_frames, audio_frames, video_dropped }
let media = probe(&info.path)?;          // MediaInfo { duration_ms, streams: [Video{h264,w,h}, Audio{aac,48000,2}] }
```

* **Blocking, thread-safe:** `Recorder` is `Send + Sync`. Pushes go through a bounded queue
  (512) to a `helm-record` thread (COM MTA + `MFStartup`) that owns the sink writer; a push blocks
  only when the queue is full. Errors from the writer surface on the next push and on `stop()`.
* **Start:** the file is opened at the **first keyframe**; its SPS/PPS become
  `MF_MT_MPEG_SEQUENCE_HEADER`. Earlier access units are dropped (`video_dropped`). Call
  `request_keyframe()` on the streamer when recording starts. Up to ~400 audio chunks that arrive
  before the keyframe are held, then written if they are not older than it.
* **Time:** file time 0 = first keyframe's `pts_us`. Video times are forced strictly increasing.
* **Stop / drop:** `stop()` finalizes and returns the info. Dropping without `stop()` also finalizes.
  `stop()` with no keyframe ever → `Error::NoVideo` (no file).
* Non-Windows builds: `Recorder::start` returns `Error::Unsupported`.

## Tests

```text
cargo test                 # pure (Annex-B, PCM, timeline, config) + live
cargo clippy --all-targets
```

The live test encodes 2 s of synthetic 320×240@30 NV12 frames with the Microsoft software H.264
MFT, records them with a 440 Hz tone, then opens the MP4 with `MFCreateSourceReaderFromURL` and
checks: duration ≈ 2 s, an H.264 320×240 stream and an AAC 48 kHz stereo stream.

`windows` 0.62 features: `Win32_Foundation`, `Win32_Media_MediaFoundation`, `Win32_System_Com`,
`Win32_System_Com_StructuredStorage`, `Win32_System_Variant`.
