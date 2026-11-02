# Helm streaming, part 2 (feature `g3`)

Extends [PROTOCOL.md §10](../PROTOCOL.md) (Phase 2 screen stream). Everything here is gated on
the hello feature **`g3`**; against an agent without it the phone behaves exactly as in §10.
Nothing in §10 changes meaning.

| Area | Status |
|---|---|
| Stream one window (`stream.start{hwnd}`) + picker thumbnails (`windows.thumb`) | done (agent, capture crate, phone) |
| Gaming preset (`stream.start{preset:"gaming"}`) | done |
| Input on the `/stream` socket | done, control socket stays the fallback |
| Remote-desktop drag select ("Drag mode") | done (phone only, uses `mb` / `ma` / `mm`) |
| Second screen: mirror one monitor + rotation lock | done (no driver) |
| Second screen: a *new* virtual monitor (IddCx driver) | researched below, not built |

## 1. Window streams

* `windows.list` (§9.3) gives the `hwnd`s. `stream.start{hwnd}` (already in §10) captures that
  top-level window with Windows.Graphics.Capture `CreateForWindow`.
* The captured area is the window's **DWM frame**: `DwmGetWindowAttribute(DWMWA_EXTENDED_FRAME_BOUNDS)`
  (physical pixels, without the invisible resize borders that `GetWindowRect` includes on
  Windows 10/11). Measured: a 640×400 (logical) test window at 125 % → frame 782×491, WGC
  content 782×491, encoded 782×490 (even).
* The encoded size is fixed at start (fit into `max_width × max_height`). If the window is
  resized, the capture is **letterboxed** into the same picture.
* **`ma{x,y}` during a window stream is normalised to the encoded picture** and the agent
  undoes the letterbox (points on the black bars clamp to the window edge). For monitor streams
  the picture and the monitor have the same aspect, so this is the same as §10.
* Cursor frames (`0x04`) keep using stream pixels; the agent re-reads the window rectangle every
  250 ms so a moved window keeps the cursor and `ma` right.
* The window closing ends the stream (`stream.ended{reason}`), like an unplugged monitor.

### `windows.thumb{hwnd, size?}` → `{png}` (remote tier)

* `size` = longer side in pixels, 32..=512, default 256. Aspect ratio kept, never upscaled.
* `png` is base64 PNG, or **`null`** when the window has nothing to show (minimized or hidden).
* Errors: `bad_params` (missing / zero / unknown `hwnd`, size out of range).
* PC side: `PrintWindow(PW_RENDERFULLCONTENT)` into a 32-bit DIB (covered windows and
  DirectComposition / DirectX content render too), cropped to the DWM frame, box-filtered down
  (`helm-win-capture::window_thumbnail_png`). ~1 KB for a 160×100 flat window.
* The phone loads thumbnails one after another when the picker opens (foreground window first).

## 2. Gaming preset

`stream.start{preset:"gaming", ...}`; `preset` is `"default"` (the §10 behaviour, also when
absent) or `"gaming"`. Anything else → `bad_params`.

| | default | gaming |
|---|---|---|
| defaults when not given | 1920×1080, 60 fps, 8000 kbit/s, audio off | 1280×720, **120 fps**, 20000 kbit/s, **audio on** |
| `CODECAPI_AVEncCommonQualityVsSpeed` | 33 | **0** (fastest) |
| GOP (`AVEncMPVGOPSize`) | 120 frames | 5 s of frames (60..=1200); keyframes otherwise only on `stream.idr` / new viewer / backpressure |
| everything else | `MF_LOW_LATENCY`, `AVLowLatencyMode`, CBR, no B-frames | same |

* Explicit `max_width` / `max_height` / `fps` / `bitrate_kbps` / `audio` still win.
* **Where the encoder allows:** fps up to 240 is accepted. If only the Microsoft software
  encoder is available and fps > 60, the agent restarts the encoder at 60 fps before replying
  (the reply's `fps` is the real one).
* The phone offers 720p60, 720p120, 1080p60 and 1080p120 (12–30 Mbit/s), always with sound.
* Frame pacing comes from the monitor: on a 60 Hz panel, 120 fps means "every composed frame,
  plus idle repeats"; a 120/144 Hz panel really delivers 120.

`stream.start` result gains `preset` (`"default"` | `"gaming"`) and `input_on_stream: true`.

Measured (RTX 4050 Laptop, NVENC through MF, own window, 1280×720 max, fps 120,
QualityVsSpeed 0, GOP 600): `QualityVsSpeed:ok`, `GOPSize:ok`, first AU an IDR with SPS/PPS,
requested keyframe arrives, pts strictly increasing. A static window is only repeated at ~29 fps
(idle repeats run on a sleep timer; a changing picture is paced by the capture).

## 3. Input on the `/stream` socket

After `stream.open`, the phone may send **text** frames `{"type":"in", ...}` on `/stream`.
They are handled exactly like `in` frames on the control socket of the session that started the
stream: same tier check, same per-session injection thread (so ordering with control-socket
input is kept per socket), errors still come back as `input.error` events **on the control
socket**. Any other text frame on `/stream` is ignored.

* The phone uses it only when `g3` is in hello **and** `input_on_stream` is true in the
  `stream.start` result. If the `/stream` socket refuses a frame (closed), the frame goes over
  the control socket instead; when the stream ends, input returns to the control socket.
* Why: the control socket also carries bulk traffic (uploads, screenshots, thumbnails, file
  lists). An input frame queued behind a large message waits for it.

Measured with the in-process agent and `FakePlatform` (debug build, loopback TLS, 40 samples,
median from "frame sent" to "injector got it", `cargo test --test g3 -- --nocapture`):

| | control socket | `/stream` socket |
|---|---|---|
| idle | 243 µs | 227 µs |
| a 900 KB request just sent on the control socket | 17.6 ms | 12.7 ms |

So: no difference when idle, about 28 % less delay when the control socket is busy (the
remaining delay is the agent's CPU time on the big message in a debug build). On Wi-Fi the gap
grows with the size of what is queued. The control socket remains the fallback and is used for
everything that is not `in`.

## 4. Drag select (phone only)

New toolbar chip **Drag mode** on the Screen tab (both Trackpad and Touch mode):

* **Long press** (450 ms, finger still) → haptic, left button **down** (`mb{left,down:true}`),
  in Touch mode after an `ma` to the finger.
* **Drag** while held → `ma` moves (Touch, batched every 16 ms) or `mm` moves (Trackpad).
* **Lift** → final `ma` (Touch), then `mb{left,down:false}`.
* Quick tap → left click. **Two-finger tap → right click.** Two-finger drag → scroll (pans the view
  when zoomed). One-finger drag before the long press → scroll / pan (Touch) or cursor move
  (Trackpad). Three-finger tap → show / hide the controls.
* If the gesture is cancelled while held (screen left), the button is released.

The state machine is `core/StreamG3.kt` `DragSelect` (unit-tested, time passed in).

## 5. Second screen

### 5.1 Now, without a driver: "show only monitor N"

The **Second screen…** dialog lists the monitors in agent order (Monitor 1 = primary, then left to
right) and a rotation option, then streams only that monitor:

* view only (a tap shows the controls; no input is sent), controls hidden, screen kept on
  (`FLAG_KEEP_SCREEN_ON`);
* **rotation lock**: *Match the monitor* (landscape for wide, portrait for tall monitors),
  *Landscape*, *Portrait* or *Rotate freely* (`requestedOrientation` = sensor landscape /
  sensor portrait; restored on exit);
* stream size capped to the phone screen in the matching orientation (never above the monitor
  or the quality preset), e.g. a 2560×1440 monitor on a 1080×2400 phone streams at 2400×1080 max.

This turns the phone into a mirror of an existing secondary monitor. Useful today with a real
second monitor, or with a **headless HDMI / DisplayPort dummy plug** (a few dollars): Windows sees
a real monitor, extends the desktop onto it, and Helm mirrors it to the phone. No software
install, no signing.

### 5.2 Later: a virtual monitor (Indirect Display Driver)

To add a monitor that does not physically exist, Windows needs a display driver. The supported
way for software monitors is an **Indirect Display Driver (IDD)** built on the **IddCx** class
extension (Microsoft Learn: "Indirect display driver model overview", "IddCx" reference). Notes
from the documentation:

**What it is**

* A **UMDF 2** (user-mode) driver, not a kernel driver. It cannot crash the kernel; it runs in a
  WUDFHost process. Available since Windows 10 1607 (IddCx 1.0); later versions add features
  (hardware cursor, HDR / wide colour on Windows 11 22H2+ with IddCx 1.10).
* The driver reports an adapter (`IddCxAdapterInitAsync`), then a monitor
  (`IddCxMonitorCreate` + `IddCxMonitorArrival`) with an **EDID** or a list of modes. Windows then
  treats it as a real monitor: it shows in Display settings, can be extended / arranged, and DWM
  composes the desktop into a **swap chain** the driver receives
  (`EVT_IDD_CX_MONITOR_ASSIGN_SWAPCHAIN`, `IddCxSwapChainReleaseAndAcquireBuffer`).
* The driver **must keep draining the swap chain** (acquire / release every frame), even if it
  throws the frames away, or DWM stalls that monitor.
* Mode changes come from Windows (`EVT_IDD_CX_ADAPTER_COMMIT_MODES`); the driver can offer new
  modes with `IddCxMonitorUpdateModes` (e.g. when the phone rotates or a different phone connects).

**Design that fits Helm**

* Keep the driver minimal: announce one monitor with the phone's size, drain the swap chain,
  nothing else. The agent captures that monitor with the **existing** Windows.Graphics.Capture →
  NV12 → H.264 pipeline (it is just another `\\.\DISPLAYn`), so there is no shared-memory video
  path to write and the protocol only needs a `display.virtual.add{width,height,hz}` /
  `display.virtual.remove{}` pair plus the usual `stream.start{monitor_id}`.
* Agent ↔ driver control: a device interface with two IOCTLs (add / remove monitor with a mode
  list), opened by the agent with `CreateFile` on the interface path. The driver checks the
  caller (only the interactive user's agent).
* Device creation: a **software device** created by the agent with `SwDeviceCreate` (lives while
  the agent runs, disappears with it — no orphaned monitor after a crash) or a root-enumerated
  device created by the installer.

**Signing (the real cost)**

* 64-bit Windows 10/11 only loads driver packages whose catalog is signed in a way the kernel
  trusts. For distribution that means **Microsoft-signed**: either *attestation signing* through
  the Partner Center hardware dashboard (no HLK test run; allowed for retail Windows 10/11
  desktops; not offered through Windows Update by default) or full **WHQL** (HLK tests).
* Partner Center hardware program enrolment requires an **EV code-signing certificate**
  (from a CA, roughly US$250–600 / year, hardware token or cloud HSM) to sign the submission.
  Every driver update is resubmitted and re-signed by Microsoft.
* Test signing (`bcdedit /set testsigning on`) works for development only: it shows a watermark,
  needs Secure Boot off on many PCs and breaks anti-cheat — not acceptable for users.
* An open-source project can still publish sources under MIT; only the signed binary has to come
  from whoever owns the EV certificate and the Partner Center account. Forks would need their own.

**Install**

* Needs administrator rights once: `pnputil /add-driver helm_idd.inf /install` (or
  `DiInstallDriver`) from the elevated installer (`host/installer/helm.iss`), then the agent
  creates the device at run time without elevation (software device) — to be verified;
  root-enumerated devices need elevation to create.
* Uninstall: remove the device and `pnputil /delete-driver oemNN.inf /uninstall`.

**Licensing / clean-room**

* The WDK and IddCx headers / libraries are free to use for building and distributing drivers.
* Microsoft's `IddSampleDriver` (Windows-driver-samples, MIT) shows the flow; under Helm's
  clean-room rule (CONTRIBUTING.md) the driver would be written from the Microsoft Learn reference
  pages, not copied from the sample or from third-party virtual-display projects.
* The EDID needs a 3-letter PNP manufacturer id; registering one has a fee, unregistered ids risk
  clashing with real vendors. Alternatively report modes without an EDID (IddCx allows a monitor
  description with no EDID and a driver-supplied mode list).
* Protected content (HDCP / PlayReady hardware DRM) is black on an indirect display, as with any
  capture.

**Effort estimate**: driver ~600–900 lines of C/C++ (UMDF 2, IddCx), INF, signing pipeline, an
agent `display.virtual.*` module and a phone toggle. Blocked on an EV certificate + Partner
Center account; until then §5.1 (mirror an existing or dummy-plug monitor) is what Helm ships.

## 6. Summary of wire changes

| Direction | Message | New |
|---|---|---|
| agent → phone | `hello.features` | contains `"g3"` |
| phone → agent | `windows.thumb{hwnd, size?}` → `{png \| null}` | method (remote) |
| phone → agent | `stream.start{..., preset?}` | `preset: "default" \| "gaming"` |
| agent → phone | `stream.start` result | `preset`, `input_on_stream` |
| phone → agent on `/stream` | `{"type":"in", ...}` text frames | allowed after `stream.open` |
| semantics | `ma` during a window stream | normalised to the picture, letterbox undone |
