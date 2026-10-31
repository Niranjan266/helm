# helm-win-audio

System-audio streaming for the Helm PC agent (Windows 10 / 11, MSVC), roadmap Phase 2:
**WASAPI shared-mode loopback** of the default playback device → mix to stereo → 48 kHz →
**Opus, 20 ms packets** → your callback, timestamped on the QPC clock like `helm-win-capture`.

Clean-room: written from the Microsoft Learn WASAPI / Core Audio docs, RFC 6716 (Opus),
RFC 7845 (Ogg Opus) and RFC 3533 (Ogg). No code copied from other projects.

## Quick start

```rust
use helm_win_audio::{AudioConfig, AudioStreamer};

let streamer = AudioStreamer::start(AudioConfig::default(), |packet: &[u8], pts_us: u64| {
    // One Opus packet (20 ms, 48 kHz stereo). Copy it into a bounded channel; no I/O here.
})?;
println!("{:?}", streamer.stats());
let final_stats = streamer.stop();
```

```text
cargo run --release --example record_opus                    # 5 s → %TEMP%\helm-win-audio\out.opus (Ogg/Opus)
cargo run --release --example record_opus -- --seconds 10 --bitrate 96 --out C:\tmp\a.opus
cargo run --release --example record_opus -- --raw           # out.hpk: repeated [u32 len][u64 pts_us][packet]
                                            # also: --mono --cbr --frame-ms 10|20|40|60 --audio-mode --no-fill
```

The example only reads the loopback stream; it never plays anything.

## API

| Item | Purpose |
|---|---|
| `AudioStreamer::start(AudioConfig, FnMut(&[u8], u64))` | start; blocks until the device and encoder run (~40 ms) or returns the error (`NoDevice`, WASAPI, Opus) |
| `AudioStreamer::stats() / stop() / is_running() / config()` | `AudioStats` snapshot; `stop()` joins the thread and returns final stats |
| `AudioConfig` | `sample_rate` (48000), `channels` (2), `bitrate_kbps` (128; roadmap 96..128), `frame_ms` (20; 10/20/40/60), `mode` (`LowDelay` / `Audio`), `complexity` (8), `cbr` (false = constrained VBR), `fill_silence` (true), `follow_default_device` (true). `validate()` |
| `AudioStats` | `packets`, `silent_packets`, `bytes`, `bitrate_kbps`, `packets_per_s`, `device_packets/frames`, `discontinuities`, `silence_samples_inserted`, `samples_dropped`, `device_changes`, `device: DeviceInfo` (id, name, mix format, `low_latency`, `period_ms`), `qpc_origin_us`, `latency_median/p95/max_ms`, `device_delay_median/max_ms`, `ended_reason`, `last_error` |
| `default_device()` | `(endpoint id, friendly name)` of the default playback device |
| `qpc_now_us()` | QPC in µs (the clock of every timestamp here) |
| `encode(&cfg, &pcm) / decode(rate, ch, &packets)` | whole-buffer Opus round-trip helpers (tests, tools) |
| `Encoder` / `Decoder` | safe single-instance Opus wrappers (`encode_f32`, `encode_i16`, `lookahead`, `decode_f32`) |
| `ogg::OggOpusWriter` | minimal Ogg/Opus file writer (OpusHead/OpusTags, one packet per page, EOS) |
| `format`, `mix`, `resample`, `framer` | the pure pipeline stages, public for tests and experiments |

## Opus dependency: `unsafe-libopus`

The usual crates (`opus`, `audiopus`, `opusic-sys`, `opus-static-sys`, `opus-head-sys`,
`opus-codec`) all build the bundled libopus with **CMake** (some also bindgen/clang), which is
not on a stock MSVC + rustup machine (it is not on this one). `unsafe-libopus` 0.2 is the
reference **libopus 1.3.1 translated to Rust with c2rust**: same bitstream and quality as the C
library, **BSD-3-Clause** (same as libopus, MIT-compatible), pure Rust, so `cargo build` works
anywhere with no C toolchain, no DLL to ship. Its API is C-style and `unsafe`; `src/opus.rs`
wraps it (`Encoder`/`Decoder` own the state, check every return code, are `Send`).

Cost: the translated code is slow unoptimised and has a few spots that rely on wrapping
arithmetic, so this crate's `Cargo.toml` builds it with `opt-level = 3` and no overflow checks
even in dev/test. **Profiles only apply at the build root, so the agent must repeat this:**

```toml
# host/agent/Cargo.toml
[target.'cfg(windows)'.dependencies]
helm-win-audio = { path = "../crates/helm-win-audio" }

[profile.dev.package.unsafe-libopus]
opt-level = 3
overflow-checks = false
debug-assertions = false
```

If a CMake toolchain becomes part of the build later, swapping in `opusic-c` / `audiopus` only
touches `src/opus.rs`.

## Pipeline

```
 default render endpoint (IMMDeviceEnumerator, eRender/eConsole)
   │  IAudioClient shared mode, AUDCLNT_STREAMFLAGS_LOOPBACK | EVENTCALLBACK, mix format
   ▼
 IAudioCaptureClient::GetBuffer  → bytes + frames + flags + QPC of first frame
   │  format::to_f32   (F32 / F64 / I16 / I24 / I32 / U8 → f32; SILENT flag → zeros; NaN → 0)
   │  mix::ChannelMixer (dwChannelMask: 5.1/7.1 → stereo at −3 dB for C/surrounds, LFE dropped,
   │                     normalised; mono ↔ stereo)
   │  resample::Resampler (pass-through at 48 kHz; else linear interpolation, box pre-filter for
   │                       integer ≥2x downsampling, state kept across packets)
   │  framer::Timeline  (QPC lock: insert silence on gaps, drop on overlap, silence fill)
   │  framer::Framer    (exact 960-sample frames)
   ▼
 opus::Encoder (RESTRICTED_LOWDELAY, 128 kbit/s constrained VBR, complexity 8, signal=music)
   ▼
 on_packet(packet, pts_us)            — all on the thread `helm-audio` (COM MTA)
```

* **Low latency.** `IAudioClient3::InitializeSharedAudioStream` with the engine's minimum period
  is tried first; if Windows refuses it (it does for loopback on this PC, `low_latency: false`)
  a fresh `IAudioClient::Initialize` with a 100 ms buffer is used. The buffer size does not add
  latency: the thread waits on the stream event with a `frame_ms / 2` timeout and drains
  everything available each wake-up.
* **Device changes.** An `IMMNotificationClient` sets a flag and signals the same event on
  `OnDefaultDeviceChanged(eRender, eConsole)`, device removal and state changes. The thread then
  compares the default endpoint id with the current one and reopens if different. Errors from
  the capture client (`AUDCLNT_E_DEVICE_INVALIDATED`, …) also reopen (retry every 500 ms while no
  device exists; `stats().last_error` says why). Each new device gets its own mixer/resampler;
  the timeline and encoder continue, so `pts_us` stays continuous across the switch.
* **Silence.** Windows delivers **no loopback packets at all while nothing plays** on many
  drivers. With `fill_silence` the timeline synthesises silent samples once the source has been
  quiet for > 50 ms (2 frames), keeping ~10 ms behind real time; the phone gets a steady 50
  packets/s (Opus codes digital silence in 3 bytes, ≈1.2 kbit/s). With `fill_silence = false`
  nothing is sent while quiet. Gaps longer than 1 s are not filled sample-by-sample: whole
  packets are skipped (pts jumps), so e.g. resuming from sleep never produces a burst.

### Timestamps

* `pts_us` of packet *k* is exactly `k × frame_ms × 1000` (0, 20000, 40000, …): strictly
  increasing, continuous, integer-exact. Sample 0 is at QPC `stats().qpc_origin_us`, so
  `qpc_origin_us + pts_us` is the QPC time of the packet's first sample.
* The timeline is locked to the QPC stamps WASAPI puts on each loopback packet. If a packet's
  stamp differs from where the timeline expects it by more than `max(frame, 20 ms) + 10 ms`,
  silence is inserted (source paused / device switch) or samples are dropped (timeline ran
  ahead, e.g. right after synthesised silence). Drift of the device clock vs QPC (tens of ppm) is
  corrected the same way, i.e. a rare ≤ 30 ms jump after minutes.
* **A/V sync with `helm-win-capture`:** both use QPC, but video `pts_us` starts at 0 at its
  own first frame. Convert both to absolute QPC (audio: `qpc_origin_us + pts`; video: record
  `qpc_now_us()` when its first access unit arrives, or extend that crate to expose its origin)
  and send the phone one common origin.
* WASAPI stamps loopback packets when the engine **mixes** them (they become readable at that
  moment: `device_delay_median_ms` ≈ 0.07 ms here). The sound reaches the speakers one device
  period or more later, so phone playback can be compared with the PC's speakers only roughly.

### Threading

| Thread | Does |
|---|---|
| caller | `start` waits for "running" or the error; `stats()` takes a mutex briefly; `stop()` / drop sets the flag, signals the event and joins |
| `helm-audio` (COM MTA) | owns the capture client, mixer, resampler, encoder; **calls `on_packet`** (with the stats mutex held: keep the callback short) |
| Core Audio notification thread | `IMMNotificationClient` → atomic flag + `SetEvent`, nothing else |

`AudioStreamer` is `Send + Sync`.

## How the agent should use it

* **`audio.stream{on:true}`** (or with `stream.start`): `AudioStreamer::start` in
  `tokio::task::spawn_blocking`. In the callback, `try_send` a copy into a **bounded**
  `tokio::sync::mpsc`; when full, drop the packet (Opus decoders conceal a lost packet; the
  phone can use packet-loss concealment when pts jumps). Send as binary frames
  `{pts_us, packet}`. Keep the streamer in the session state; `stop()` on
  `audio.stream{on:false}` / disconnect.
* Tell the phone: 48 kHz, 2 channels, 20 ms, Opus, `pre-skip` = `Encoder::lookahead()`
  (120 samples in `LowDelay`) if it cares about the first 2.5 ms.
* Show `stats().device.name` in the UI; poll `stats()` for `device_changes` to notify.
* **Recording** (`record.start{audio}`) will use an MF sink writer (MP4/AAC); `OggOpusWriter`
  is here for the example and quick debugging.

## Measured (this dev PC)

Realtek speakers, mix format 48000 Hz 2 ch F32, Windows 11 26200, release build:

| What | Result |
|---|---|
| `start` | 40–42 ms (device activation + encoder) |
| Packet rate | 50.0 /s, pts gaps 0 over 3 s (`record_opus`) |
| Engine period / `low_latency` | 10 ms / false (IAudioClient3 refused for loopback) |
| Device read delay (`device_delay_median_ms`) | 0.06–0.07 ms (max ≤ 1 ms) |
| Read → callback latency (`latency_median_ms` / p95) | 0.25 / 0.32 ms (silence; encode of music adds ≈ 0.5 ms) |
| Silence | 3-byte packets, ≈ 1.2 kbit/s |
| Opus round-trip, 1 kHz enveloped sine, 128 kbit/s | SNR 42.3 dB (`LowDelay`, delay 120 samples), 42.2 dB (`Audio`, delay 312) |

Live numbers were taken with nothing playing (packets are real device packets flagged silent);
latency with real audio was not measured.

## Tests

```text
cargo test                     # 30 pure tests + 1 live read-only loopback test (1 s)
cargo clippy --all-targets     # clean
```

Pure: config validation and frame sizes; every sample type, silent flag, NaN, short buffer,
f32→i16 clipping; stereo pass-through, mono↔stereo, 5.1 downmix (LFE dropped, no clipping),
masks with too few bits; resampler pass-through, 44.1→48 kHz length/frequency/SNR, chunked ==
one-shot, 88.2/96/192 kHz → 48 kHz; framing and indices, pts math (exact after 24 h),
timeline jitter tolerance, gap → silence, overlap → drop, silence-fill hysteresis; Opus
round-trip SNR in both modes with delay = look-ahead and bitrate in range, silence packet size,
i16 path, mono CBR packet size; Ogg CRC (table vs bit-by-bit + check value), page lacing,
writer granules and EOS; percentiles and stats window.
Live: default device resolves; 1 s loopback gives 40–60 packets, pts 0, 20000, … strictly
increasing, every packet decodes to 960 stereo samples, device info and QPC origin present. It
passes whether or not anything is playing; on a PC without any playback device it skips.

## Windows features used

`windows` 0.62: `Win32_Foundation`, `Win32_Devices_FunctionDiscovery` (`PKEY_Device_FriendlyName`),
`Win32_Media_Audio`, `Win32_Security` + `Win32_System_Threading` (events),
`Win32_System_Com`, `Win32_System_Com_StructuredStorage` + `Win32_System_Variant` (PROPVARIANT),
`Win32_System_Performance` (QPC), `Win32_UI_Shell_PropertiesSystem` (IPropertyStore);
`windows-core` for `#[implement]`.

## Limitations / next steps

* Resampling is linear interpolation: fine for monitoring, audible aliasing is possible for
  44.1 kHz devices with strong high-frequency content. Most endpoints run at 48 kHz (copy).
* Clock drift between the device and QPC is corrected by rare ≤ 30 ms jumps, not by
  adaptive resampling.
* The Ogg writer was checked by its own parser tests only (no external player on this PC).
* Device switching, multichannel and non-48 kHz paths are covered by pure tests; live they were
  only exercised on one 48 kHz stereo device.
* Loopback captures the whole mix, including Helm's own sounds and any audio the phone sends
  back to the PC (echo if the phone also plays to the PC). Per-process loopback
  (`ActivateAudioInterfaceAsync` with `AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK`, Windows 10
  2004+) could exclude the agent's own process later.
* No MMCSS (`AvSetMmThreadCharacteristics("Pro Audio")`) yet; the 100 ms buffer absorbs
  scheduling hiccups.
* Protected audio (some DRM players) is delivered as silence by Windows.
