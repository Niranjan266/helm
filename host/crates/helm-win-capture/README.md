# helm-win-capture

Screen capture for the Helm PC agent (Windows 10 1903+ / Windows 11, MSVC).

* **Phase 1, stills:** `monitors()` and `screenshot_png()`. GDI `BitBlt` → PNG.
* **Phase 2, live:** `Streamer`. Windows.Graphics.Capture → D3D11 video processor
  (BGRA → NV12, scaling) → Media Foundation H.264 encoder MFT (hardware when present) →
  Annex-B access units delivered to your callback.

No FFmpeg, no vendor SDKs: only Win32/WinRT APIs through the `windows` crate. All code is
original, written from the Microsoft Learn documentation (see `CONTRIBUTING.md`, clean-room rule).

## Quick start

```rust
use helm_win_capture::{dpi, monitors, screenshot_png, StreamConfig, Streamer};

dpi::set_process_per_monitor_v2();            // once, at startup (see "DPI" below)

for m in monitors() {                         // primary first, physical pixels
    println!("{} {} {}x{} @({},{}) x{}", m.id, m.name, m.width, m.height, m.x, m.y, m.scale);
}
let png = screenshot_png(Some(r"\\.\DISPLAY1"))?; // one monitor
let all = screenshot_png(None)?;                  // whole virtual desktop

let streamer = Streamer::start(StreamConfig::default(), |au: &[u8], key: bool, pts_us: u64| {
    // Annex-B access unit; keyframes always start with SPS + PPS.
})?;
streamer.request_keyframe();
println!("{:?}", streamer.stats());
let final_stats = streamer.stop();
```

Examples:

```text
cargo run --release --example screenshot              # PNG of every monitor + virtual desktop
cargo run --release --example record_h264 -- --animate # 5 s, 1080p60 max, writes %TEMP%\helm-win-capture\out.h264
cargo run --release --example record_h264 -- --software --seconds 3 --max 1280x720 --fps 30
```

`--animate` shows a small click-through, non-activating colour square in the bottom-right corner
so every frame changes and latency is measured on real new frames. It never touches other windows.

## API

| Item | Purpose |
|---|---|
| `monitors() -> Vec<MonitorInfo>` | `id` (`\\.\DISPLAYn`, use as `monitor_id`), `name` (EDID friendly name / "Built-in display"), `x, y, width, height` (physical px, virtual-desktop coordinates), `is_primary`, `scale` (effective DPI / 96) |
| `screenshot_png(Option<&str>) -> Result<Vec<u8>>` | `Some(id)` = that monitor, `None` = the whole virtual desktop |
| `screenshot_rgba`, `screenshot_rect_rgba`, `screenshot_primary_rgba`, `encode_png` | lower-level still capture |
| `Streamer::start(StreamConfig, FnMut(&[u8], bool, u64))` | start a stream; returns once the encoder runs or with the error |
| `Streamer::request_keyframe()` | next frame becomes an IDR (`CODECAPI_AVEncVideoForceKeyFrame`) |
| `Streamer::set_bitrate(kbps)` | change the CBR target while running (`CODECAPI_AVEncCommonMeanBitRate`, applied before the next frame; encoders that reject it keep the old rate) |
| `Streamer::stats() / stop()` | `StreamStats` (fps, latency median/p95/max, drops, bitrate) |
| `Streamer::info()` | `StreamInfo`: encoder name, hardware?, GPU input?, async MFT?, profile, sizes |
| `Streamer::is_running()` | false after the source closed or an error (`stats().ended_reason`) |
| `list_h264_encoders()` | installed H.264 MFTs, hardware first |
| `window_frame_rect(hwnd)` | G3: the window's DWM frame (`DWMWA_EXTENDED_FRAME_BOUNDS`, physical px) = what window capture shows |
| `window_thumbnail_png(hwnd, max_side)` / `window_rgba` | G3: `PrintWindow(PW_RENDERFULLCONTENT)` still, cropped to the frame, box-filtered; `Unsupported` when minimized / hidden |
| `StreamConfig` | `source` (`PrimaryMonitor` / `Monitor(id)` / `Window(hwnd)`), `max_width`, `max_height`, `fps`, `bitrate_kbps`, `gop_frames` (120), `repeat_idle_frames` (true), `encoder` (`Auto` / `HardwareOnly` / `SoftwareOnly`), `quality_vs_speed` (33; 0 = fastest, the G3 gaming preset). `validate()` checks ranges |
| `annexb` | pure Annex-B helpers: NAL iteration, NAL / slice types, keyframe detection, `ParameterSets`, `summarize` |
| `capture`, `convert`, `encoder`, `d3d` | the pipeline stages, public for experiments |

## DPI (what the agent must do)

Windows hands DPI-unaware code scaled coordinates and stretched screenshots (on a 125 % panel
a 1920×1080 screen looks like 1536×864). This crate is safe either way: every coordinate or
pixel read runs inside `SetThreadDpiAwarenessContext(PER_MONITOR_AWARE_V2)` and restores the
previous context afterwards, so `monitors()` and screenshots are **always physical pixels**. A
live test checks this from a process that never declared DPI awareness.

The agent should still make the **whole process** per-monitor-v2 aware, so `SendInput`
absolute coordinates, cursor positions and any windows it shows (tray, QR window) use the same
coordinate space as `monitors()`:

* best: the application manifest, `<dpiAwareness>PerMonitorV2</dpiAwareness>`, or
* first thing in `main`: `helm_win_capture::dpi::set_process_per_monitor_v2();`
  (returns true when set or already set).

## Streaming architecture

```
 Windows.Graphics.Capture                 GPU (same ID3D11Device everywhere)            Media Foundation
 ┌──────────────────────┐  BGRA tex   ┌───────────────────────────────┐ NV12 tex ┌──────────────────────────┐
 │ GraphicsCaptureItem  │ ──────────► │ ID3D11VideoProcessor           │ ───────► │ H.264 encoder MFT        │
 │ (monitor or window)  │ FrameArrived│ BGRA→NV12 BT.709 limited,      │ ring of 6│ (HW async, D3D11-aware   │
 │ FramePool (free-thr.)│             │ scale + letterbox to out size  │ slots    │  via IMFDXGIDeviceManager│
 └──────────────────────┘             └───────────────────────────────┘          │  or SW sync, CPU copy)   │
                                                                                   └────────────┬─────────────┘
                                                                         Annex-B AU (+SPS/PPS on IDR)
                                                                                                ▼
                                                                         on_access_unit(bytes, key, pts_us)
```

1. **Device.** One hardware D3D11 device with `BGRA_SUPPORT | VIDEO_SUPPORT`, multithread
   protected (`ID3D11Multithread`), wrapped for WinRT with `CreateDirect3D11DeviceFromDXGIDevice`
   and shared with MF through `MFCreateDXGIDeviceManager`.
2. **Capture.** `IGraphicsCaptureItemInterop::CreateForMonitor` / `CreateForWindow`,
   `Direct3D11CaptureFramePool::CreateFreeThreaded` (2 buffers), session with
   `IsCursorCaptureEnabled = false` (the cursor is sent separately per the protocol),
   `IsBorderRequired = false` (Windows 11) and `MinUpdateInterval` ≈ ¾ frame (Windows 11 24H2+,
   best effort). When `ContentSize` changes, the pool is `Recreate`d. The video processor reads
   only the content rectangle and letterboxes it into the fixed output size, so a resized window
   never changes the encoded resolution.
3. **Pacing.** Frames arriving faster than `fps` are skipped on the composition clock
   (`SystemRelativeTime`), so a 144 Hz monitor still gives 60 fps.
4. **Conversion.** `ID3D11VideoProcessor` writes NV12 into one of 6 ring textures. Full-range
   sRGB in, BT.709 limited range out (`ID3D11VideoContext1` colour spaces). The same values go
   into the encoder media type so the SPS VUI matches. If a driver cannot bind the capture
   texture as video-processor input, the frame is first copied into our own BGRA texture.
5. **Encoder selection.** `MFTEnum2` with `MFT_ENUM_ADAPTER_LUID` = the capture GPU (hybrid
   laptops!), falling back to `MFTEnumEx(MFT_CATEGORY_VIDEO_ENCODER, HARDWARE | SORTANDFILTER)`.
   Each hardware MFT is tried with GPU input (`MF_SA_D3D11_AWARE` + `MFT_MESSAGE_SET_D3D_MANAGER`),
   then with system-memory input, then the Microsoft software MFT (`SYNCMFT`).
6. **Encoder configuration** (before the media types, as MF requires): `MF_LOW_LATENCY`,
   `CODECAPI_AVLowLatencyMode`, CBR (`AVEncCommonRateControlMode`, `AVEncCommonMeanBitRate`),
   `AVEncMPVGOPSize`, `AVEncMPVDefaultBPictureCount = 0`, `AVEncCommonQualityVsSpeed = 33`.
   Output type H.264 Main (Baseline fallback), progressive, 1:1 PAR; input type NV12.
   `StreamInfo::codec_settings` reports which properties each encoder accepted (NVIDIA rejects
   the B-frame count property but emits no B-frames in low-latency mode, which the tests check).
7. **Async hardware MFTs** are unlocked (`MF_TRANSFORM_ASYNC_UNLOCK`) and driven by events:
   an `IMFAsyncCallback` (`BeginGetEvent`) forwards `METransformNeedInput` /
   `METransformHaveOutput` to the pipeline thread, which re-arms after each event. Sync MFTs use
   the classic `ProcessInput` → `ProcessOutput` until `MF_E_TRANSFORM_NEED_MORE_INPUT` loop.
   `MF_E_TRANSFORM_STREAM_CHANGE` renegotiates the output type.
8. **Output.** Encoders emit Annex-B. `ParameterSets` caches SPS/PPS (from the stream and from
   `MF_MT_MPEG_SEQUENCE_HEADER`) and prepends them to any IDR that lacks them, so a phone can
   join at any keyframe. `is_keyframe` is derived from the NAL types (IDR present).

### Threading

| Thread | Does | Never |
|---|---|---|
| caller | `Streamer::start` waits for the pipeline to report "running" or an error | — |
| capture callback (system thread pool) | pace, take a free ring slot, GPU convert, post `Frame{slot}` | block on the encoder (no free slot → frame dropped) |
| pipeline thread `helm-stream` (COM MTA + `MFStartup`) | owns the MFT; one channel for frames, MFT events, keyframe requests, stop; **calls `on_access_unit`** | — |
| MF work-queue thread | `IMFAsyncCallback::Invoke` → message to the pipeline | touch encoder state |

Latency policy: the pipeline always encodes the **newest** frame. Older queued frames are
dropped (`frames_dropped`), never queued. A slot is reused only after the encoder has
returned the output for it (GPU input) or after the CPU copy (system-memory input). The last
submitted slot is protected so it can be re-encoded when the screen is idle
(`repeat_idle_frames`: after 1.5 frame intervals without a new frame, repeats at `fps`). This keeps
the decoder fed and lets CBR refine a static image. Turn it off to send nothing while idle.

### Timestamps and stats

* `pts_us` starts at 0 for the first frame and follows the frame arrival clock (QPC). It is
  strictly increasing.
* **Latency** = QPC when `FrameArrived` hands us the frame → QPC when the encoded access unit
  is passed to the callback (GPU conversion + queueing + encode). `SystemRelativeTime` is *not*
  used for this: it is DWM's target composition time and is usually 5–11 ms in the future when
  the callback runs.
* `StreamStats`: `frames_captured` (fresh), `frames_repeated`, `frames_dropped`,
  `frames_paced_out`, `frames_encoded`, `keyframes`, `bytes`, `encoded_fps`, `captured_fps`,
  `bitrate_kbps`, `latency_median_ms` / `p95` / `max` over the last 1024 fresh frames.

## How the agent should use it

```toml
# host/agent/Cargo.toml
[target.'cfg(windows)'.dependencies]
helm-win-capture = { path = "../crates/helm-win-capture" }
```

* **Startup:** make the process per-monitor-v2 DPI aware (manifest or `dpi::set_process_per_monitor_v2()`).
* **`monitors.list`:** `monitors()` is cheap (no capture involved); call it per request. Ids are stable while
  the display topology stays the same; re-list after `WM_DISPLAYCHANGE`.
* **`screen.capture{monitor}`:** `screenshot_png(Some(id))` takes ~45–65 ms for 1080p (capture +
  PNG). Run it in `tokio::task::spawn_blocking`. Missing id → `Error::MonitorNotFound`.
* **`stream.start{monitor|hwnd,max_w,max_h,fps,bitrate}`:** build a `StreamConfig`, `validate()`,
  `Streamer::start` (blocks while the device and encoder are created: run it in `spawn_blocking`). In the
  callback, copy the bytes into a **bounded** `tokio::sync::mpsc` with `try_send`. If the channel
  is full (the socket is backing up), drop the access unit and call `request_keyframe()` once the
  socket drains. The decoder can only recover at an IDR. Do not do I/O inside the callback.
  `Streamer` is `Send + Sync`; keep it in the session state.
* **`stream.idr`:** `request_keyframe()`. Also call it when a new viewer joins.
* **`stream.stop` / disconnect:** `stop()` (joins the pipeline thread, returns final stats) or
  just drop the `Streamer`.
* **Monitor unplugged / window closed:** the pipeline stops by itself; `is_running()` becomes
  false and `stats().ended_reason` says why. Send the phone an event and offer the monitor list.
* **Adaptive bitrate:** `set_bitrate(kbps)` at runtime (the agent drives it from `stream.stats`).

## Measured (this dev PC)

RTX 4050 Laptop GPU, 1920×1080 @ 60 Hz, 125 % scale, Windows 11 26200, release build,
`record_h264 --animate` (5 s, 1080p60 max, 12 Mbps CBR, GOP 120, keyframe requested at 2.5 s):

| Encoder | Path | Encoded fps | Fresh fps | Latency median / p95 / max | File (5 s) |
|---|---|---|---|---|---|
| NVIDIA H.264 Encoder MFT | hardware, async, D3D11 texture input, Main@4.2 | 60.0 | 60.0 | 2.9 / 5.1 / 7.1 ms | 8.1 MB (≈12.9 Mbps) |
| Microsoft H264 Encoder MFT | software, sync, CPU copy, Main@4.2 | 60.1 | 60.1 | 5.9 / 8.6 / 43 ms | 5.3 MB (≈8.4 Mbps) |

Both streams: SPS/PPS on all 4 IDRs (t = 0, GOP at 2.0 s, forced at 2.5 s, GOP 2.0 s later), no B
slices, picture count = access units = fps × duration. 720p30 (scaled): 30.0 fps, 2.4 ms median.
Screenshot: 1080p PNG in 45–65 ms (~840 KiB).

## Tests

```text
cargo test                                               # pure + live read-only (monitors, screenshot PNG, encoders)
cargo test --release -- --ignored --test-threads=1       # streaming: HW auto, SW, own-window capture
cargo test --release --test window -- --ignored --nocapture  # G3: own window rect, thumbnail, 120 fps low-latency stream
cargo clippy --all-targets
```

Pure tests: BGRA→RGBA, pitched copy, Annex-B splitting (3/4-byte start codes, emulation
prevention, trailing zeros), NAL / slice types, SPS profile/level, SPS/PPS insertion, config
validation, output-size fit and letterboxing, percentile/latency window, ring-slot policy.
Live tests: monitors non-empty with exactly one primary at (0,0); physical sizes without process
DPI awareness; primary screenshot decodes as an opaque RGBA PNG of the monitor's size; virtual
desktop size; unknown id error; an H.264 MFT is installed. Streaming tests (ignored by default):
first AU is an IDR with SPS/PPS, keyframe flags match the NAL types, no B slices, pts strictly
increasing, the requested keyframe arrives; window capture uses a window the test creates itself.

## Windows features used

`windows` 0.62 features: `Foundation`, `Graphics`, `Graphics_Capture`, `Graphics_DirectX`,
`Graphics_DirectX_Direct3D11`, `Win32_Foundation`, `Win32_Devices_Display` (friendly names),
`Win32_Graphics_Gdi`, `Win32_Graphics_Direct3D`, `Win32_Graphics_Direct3D11`,
`Win32_Graphics_Dxgi`, `Win32_Graphics_Dxgi_Common`, `Win32_Media_MediaFoundation`,
`Win32_System_Com`, `Win32_System_Ole` + `Win32_System_Variant` (ICodecAPI VARIANTs),
`Win32_System_Performance` (QPC), `Win32_System_LibraryLoader` (examples/tests windows),
`Win32_System_WinRT`, `Win32_System_WinRT_Direct3D11`, `Win32_System_WinRT_Graphics_Capture`,
`Win32_UI_HiDpi`, `Win32_UI_WindowsAndMessaging`. Plus `windows-core` (for `#[implement]`)
and `png`.

## Limitations / next steps

* **No DXGI Desktop Duplication fallback** yet (the roadmap's fallback for systems without
  Windows.Graphics.Capture). On Windows 10 1903+ / 11 WGC is always present.
* **Secure desktop** (UAC prompts, lock screen, Ctrl+Alt+Del) is not capturable by a normal user
  process with either GDI or WGC. A SYSTEM-service helper would be needed.
* **Cursor** is not in the picture (by design). The agent sends position/shape separately.
* Runtime `set_bitrate` was verified through the agent with the fake only; acceptance of the dynamic
  change by AMD / Intel MFTs is untested. **One stream per `Streamer`**. fps changes need a restart.
  Two monitors means two `Streamer`s (each has its own device and encoder session; consumer
  NVIDIA GPUs allow a limited number of concurrent NVENC sessions).
* The software MFT works at 1080p60 here but costs a lot of CPU. Lower `max_*` / `fps` when
  `StreamInfo::hardware` is false.
* HDR / wide-gamut monitors are captured as 8-bit sRGB (`B8G8R8A8UIntNormalized`).
* Hardware encoders verified on NVIDIA only. AMD AMF / Intel QSV MFTs follow the same code path
  (async, D3D11-aware) but are untested here.
* Audio (WASAPI loopback → Opus) is out of scope for this crate.
