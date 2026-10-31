# Design: phone as PC webcam and microphone (D7)

Status: **design only — for the owner's security review.** Nothing in this document is
implemented. No COM DLL, media source, filter or driver is built, registered or installed by
any Helm code today. Roadmap item: Phase 5 "Phone as webcam/mic (D7)".

Statements marked **(verify)** are believed correct for Windows 11 22H2–24H2 but must be
confirmed on a test machine in step 0 of the plan before any code is written.

## 1. Goal and non-goals

Goal: the Helm phone app streams its camera (and optionally its microphone) to the paired PC,
and ordinary Windows apps (Teams, Zoom, browsers, the Camera app, OBS) can pick "Helm Camera"
(and "Helm Microphone") as a device.

Non-goals: kernel-mode code written by us; anything that runs as SYSTEM; any persistent
machine-wide device that exists while Helm is not running; Windows 10 support in v1.

## 2. Constraints from the rest of Helm

- The PC agent runs **as the signed-in user, medium integrity**, installed per-user to
  `%LOCALAPPDATA%\Programs\Helm` by `scripts/install.ps1` with **no elevation**. Today nothing
  Helm ships needs admin. Any option below that needs admin breaks that property and must be a
  separate, opt-in component.
- All phone↔PC traffic is pinned TLS (PROTOCOL.md §1); video from the phone is untrusted input
  to the PC until decoded and validated.
- Clean-room: Microsoft samples (e.g. the "VirtualCamera" / "SimpleMediaSource" samples, SYSVAD)
  and OBS's virtual-camera code may be **read for API understanding only**; no code is copied.

## 3. Options — video

### 3.0 Android "USB webcam" mode (no PC code at all)

Android 14+ on supported devices (e.g. Pixel 6 and later) can expose the phone as a standard
USB Video Class (UVC) camera over the USB cable ("Webcam" in the USB preferences). Windows uses
its inbox UVC driver.

- Pros: zero Helm code on the PC, zero new privilege, works on Windows 10 too.
- Cons: USB only, device support is limited, no Helm control of resolution/overlays, no mic
  (video only) **(verify)**, not available on most non-Pixel phones.
- Use: document it in the app ("Use USB webcam mode if your phone supports it") as the
  zero-risk path. It does not replace a Wi-Fi option.

### 3.1 Windows 11 `MFCreateVirtualCamera` + custom media source (recommended)

Windows 11 (build 22000+) lets a user-mode app register a *software virtual camera* backed by a
**custom Media Foundation media source** (`mfvirtualcamera.h`). The OS camera stack
(Windows Camera Frame Server) loads the media source and exposes it to every camera API
(Media Foundation, WinRT `MediaCapture`, and DirectShow via the platform's bridge), so all apps
see it — unlike a DirectShow-only filter.

Key parameters of `MFCreateVirtualCamera(type, lifetime, access, friendlyName, sourceId, …)`:

| Parameter | Choices | Effect |
|---|---|---|
| `lifetime` | `MFVirtualCameraLifetime_Session` | Camera exists only while the creating process keeps the `IMFVirtualCamera` alive (Helm agent running and the user has started "Use as webcam"). |
| | `MFVirtualCameraLifetime_System` | Camera persists across reboots until `Remove()` is called. |
| `access` | `MFVirtualCameraAccess_CurrentUser` | Only the creating user sees the camera. |
| | `MFVirtualCameraAccess_AllUsers` | Every user on the PC sees it; **requires admin** to create **(verify)**. |
| `sourceId` | CLSID string | The COM class of *our* media source that Frame Server will instantiate. |

**Per-user vs machine-wide media source.** The *camera* can be per-user (`CurrentUser` access),
but the *media source* is a COM in-proc server loaded by Frame Server, a Windows service. A
service does not read the user's `HKCU\Software\Classes`, so the CLSID must be registered under
**`HKLM\Software\Classes\CLSID`** and the DLL must sit where the service account can read it.
In practice this means: **the media source registration is always machine-wide and needs admin
once, at install time**, even though the camera device itself is per-user and session-scoped
and can be created at runtime without admin **(verify both)**. There is no "fully per-user, no
admin" variant of this design.

**FrameServer loading constraints.**

- The media source DLL is loaded **into the Frame Server service process** (`svchost.exe`
  hosting `FrameServer`, plus `FrameServerMonitor`), not into the Helm agent and not into the
  consuming app. That process runs as **`NT AUTHORITY\LOCAL SERVICE`** in **session 0**
  **(verify with `sc qc FrameServer` and Process Explorer on the target build)**.
- Consequences:
  - The DLL cannot see the user's desktop session objects by default; it cannot use the agent's
    TLS connection, files in `%LOCALAPPDATA%`, or `HKCU`.
  - Frame data has to cross from the user's session to session 0 through an explicitly ACL'd
    kernel object (§5.3).
  - Frame Server may load the source for *other* users' apps if access were `AllUsers`; with
    `CurrentUser` only the owner's apps can open it.
  - The source must implement `IMFMediaSourceEx`, `IMFMediaStream2`, `IMFGetService`,
    `IKsControl` (can return "not supported" for unknown property sets) and be activatable via
    `IMFActivate` with the Frame Server's expectations (free-threaded, no UI, no blocking on the
    caller thread). Crashes or hangs take down **all cameras for all users** until Frame Server
    restarts — robustness is a hard requirement, not a nicety.
  - The DLL's own dependencies are resolved by the service's DLL search order: link the CRT
    statically, call `SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32)` semantics for any
    late loads, and depend on nothing outside `System32`.

**Signed DLL.** Windows does not appear to *require* Authenticode for a software camera media
source **(verify)**, but:

- Machines with Smart App Control, WDAC/App Control policies or some EDR products will refuse to
  load an unsigned DLL into a service.
- We must ship it signed (OV/EV certificate or Azure Trusted Signing) together with a signed
  installer, and the agent must check the installed DLL's signature and publisher before calling
  `MFCreateVirtualCamera` (refuse to create the camera if the file is not ours).
- Self-built (unsigned) open-source builds are documented as "developer only".

Pros: native, works with every app, no kernel code, camera disappears when Helm stops (Session
lifetime), per-user visibility.
Cons: Windows 11 only; one-time admin install of a machine-wide COM DLL that runs inside a
shared Windows service.

### 3.2 OBS-style DirectShow source filter

Register a DirectShow source filter (COM DLL, 32-bit **and** 64-bit) under
`CLSID_VideoInputDeviceCategory`. Apps that enumerate cameras through DirectShow load the DLL
**in their own process** and pull frames from a shared-memory queue written by the agent.

- Pros: works on Windows 10; well-trodden approach.
- Cons:
  - Invisible to Media Foundation / WinRT apps (Windows Camera app, many UWP/WinUI and newer
    browser capture paths) **(verify per app)**.
  - Still needs a machine-wide (HKLM) or per-user (HKCU) COM registration; HKCU works for
    non-elevated apps of that user only, so it *can* be admin-free — but elevated apps and
    services ignore HKCU.
  - **Our DLL is injected into every process that enumerates video devices**: browsers,
    conferencing apps, elevated tools. A bug is a crash in someone else's app; a memory-safety
    bug is code execution in that app's context (including elevated ones if registered in
    HKLM). Large blast radius, hard to update while loaded.
  - The camera exists permanently while registered (no session lifetime).

Verdict: fallback for Windows 10 only, not in v1, separate review if ever pursued.

### 3.3 Kernel-mode camera (AVStream minidriver)

Rejected: kernel code, requires an EV certificate and Microsoft attestation/WHQL signing via
Partner Center, a bug is a BSOD or local privilege escalation, and Windows 11 provides 3.1
exactly so that this is unnecessary.

## 4. Options — audio (microphone)

Windows has **no user-mode equivalent of `MFCreateVirtualCamera` for audio**: an app-selectable
microphone endpoint needs an audio driver (a PortCls/WaveRT miniport or an APO attached to an
existing endpoint).

| Option | Privilege | Notes |
|---|---|---|
| A. Helm-written virtual audio driver | Kernel, admin install, attestation-signed | Same objections as 3.3. Rejected for v1. |
| B. User-installed third-party virtual cable (e.g. a "virtual audio cable" product the user already trusts) | Helm adds nothing privileged | Agent decodes Opus from the phone and renders it with WASAPI (user mode) to the cable's *render* endpoint; apps choose the cable's *capture* endpoint as mic. Helm only lists endpoints and lets the user pick one; it never installs the driver. **Recommended.** |
| C. Bluetooth headset profile (phone as BT mic) | None on PC | Phones generally cannot act as an HFP headset towards a PC. Not viable. |
| D. No PC mic; apps get audio through the camera | — | Video conferencing apps take mic separately; not a substitute. |

Recommendation: **v1 = video only**; v1.1 = option B as an opt-in "Send phone mic to:" picker.
Revisit a Helm-owned driver only if Windows gains a user-mode virtual audio API.

## 5. Recommended design

### 5.1 Components

```
Phone (Helm app)                    PC, user session (medium IL)            PC, session 0
-----------------                    ----------------------------            ----------------------------
Camera2 -> MediaCodec H.264  --TLS-> helm agent: /camera socket            FrameServer (LOCAL SERVICE)
(foreground service, camera          - validates + decodes H.264 (MF          loads helm_vcam.dll
 type, visible notification)           hardware decoder, user process)        (HKLM CLSID, Program Files)
                                     - converts/scales to NV12 fixed size   - reads frames from the
                                     - writes to a ring in a section    ==>    read-only section view
                                     - holds IMFVirtualCamera                - delivers IMFSamples
                                       (Session lifetime, CurrentUser)       - no network, no codecs
```

### 5.2 Rules for the media source DLL (`helm_vcam.dll`)

- **Does almost nothing**: maps a section read-only, validates a fixed header, copies NV12 into
  `IMFSample`s, emits a static "Phone not connected" frame when the ring is stale. No network,
  no decoder, no file or registry writes, no UI, no logging to user-writable paths.
- **All untrusted parsing stays in the user-mode agent.** The H.264 bitstream from the phone is
  decoded in the agent (medium IL, the user's own security context), never in the
  `LOCAL SERVICE` process that hosts every user's cameras.
- Header: magic, version, width, height, stride, format, frame sequence, timestamp, plus a
  writer heartbeat. Width/height must match one of the formats the source advertised
  (e.g. 1280×720 and 1920×1080 NV12 @ 30 fps); anything else → treat as stale. All offsets are
  computed from the advertised format, never read from the section.
- Written in C++ with `/guard:cf`, `/Qspectre`, CET shadow stack compatible, static CRT, or in
  Rust with `windows-rs` and `#![deny(unsafe_op_in_unsafe_fn)]`; the header parser is fuzzed.
- Tolerates the agent vanishing at any moment (torn frames are harmless: sequence numbers are
  written before and after the payload; mismatched → reuse the previous frame).

### 5.3 Cross-session frame channel

- The **agent** creates the section (it runs in the user's session and lacks
  `SeCreateGlobalPrivilege`, so it creates it in its session namespace,
  `\Sessions\<id>\BaseNamedObjects\Helm.VCam.<random>`), with an explicit DACL:
  owner = the user (read/write), `NT AUTHORITY\LOCAL SERVICE` = **read only**
  (`FILE_MAP_READ`), nobody else. Mandatory label: default.
- The agent hands the section name to the source through the virtual camera's own properties
  (`IMFVirtualCamera::AddProperty` / `AddRegistryEntry`) **(verify the source can read them
  from its activation attributes)**. The random suffix is per camera instance.
- The DLL opens it read-only; a malicious writer (= the user themselves) can at worst make the
  user's own virtual camera show garbage.

### 5.4 Lifetime and consent

- The camera is created only when the phone user taps "Use as webcam" and the PC accepts
  (new `camera.start` method, `remote` tier, per-PC toggle "Allow phone as webcam", off by
  default), and removed on `camera.stop`, disconnect, agent exit, or session lock.
- `MFVirtualCameraLifetime_Session` + `MFVirtualCameraAccess_CurrentUser` only. The agent never
  calls the `System` / `AllUsers` variants.
- Phone: Android foreground service of type `camera`, ongoing notification, Android's own
  camera-in-use indicator. PC: tray badge "Phone camera active".
- Windows privacy settings (Settings › Privacy › Camera) continue to gate which apps may open it.

### 5.5 Protocol sketch (to be added to PROTOCOL.md when built, feature `p6cam`)

- `camera.caps` (viewer) → `{available, reason?, formats:[{w,h,fps}]}`; `available:false,
  reason:"not_installed"|"unsupported_os"|"bad_signature"`.
- `camera.start` (remote) `{w,h,fps,facing}` → `{camera_token}`; then the phone opens a second
  pinned-TLS socket `/camera` (same auth as `/stream`, §10) and sends Annex-B H.264 access units
  in §10-style binary frames, phone → PC.
- `camera.stop` (remote) → `{}`; event `camera.state{active}`.
- Limits: max 1920×1080, 30 fps, 8 Mbit/s, 1 MiB per access unit; anything larger closes the
  socket.

## 6. Security analysis

### 6.1 Privilege boundaries

| Boundary | Who is on each side | Risk | Mitigation |
|---|---|---|---|
| Phone → agent (network) | Paired phone → user-session agent | Malformed H.264 / oversized frames | Pinned TLS + device auth (existing), size limits, decode with the OS decoder in the agent process, feature off by default. |
| Agent → Frame Server (section) | User (medium IL) → `LOCAL SERVICE` (session 0) | User-to-service input; a parser bug in the DLL = code execution as LOCAL SERVICE inside the shared camera service | Fixed-format header only, offsets derived from advertised formats, read-only mapping, fuzzing, no codecs in the DLL. |
| Installer → machine | Admin (one-time, opt-in) | Installs a DLL loaded by a Windows service | Separate signed per-machine MSI "Helm Camera", no custom actions beyond files + HKLM CLSID keys, clean uninstall. |
| DLL file on disk | Admin-only writable location vs users | **DLL planting / replacement = privilege escalation into LOCAL SERVICE** | Install to `%ProgramFiles%\Helm Camera\` (inherits admin-only write ACL). Never register a DLL path under `%LOCALAPPDATA%`, `%TEMP%`, the per-user Helm folder, or any user-writable directory. Agent verifies the HKLM `InprocServer32` path is under `%ProgramFiles%` and the file's Authenticode publisher before use. |
| COM registration | HKLM vs HKCU | HKCU shadowing of the CLSID | Frame Server does not read HKCU; we register only under HKLM, `ThreadingModel=Both`. |
| Other users on the PC | User A vs user B | B sees/uses A's phone camera | `CurrentUser` access; per-instance random section name with DACL for A + LOCAL SERVICE only. |

### 6.2 What runs as SYSTEM

- **Nothing of ours at runtime.** The agent runs as the user; the media source runs inside
  Frame Server as `LOCAL SERVICE` **(verify)**.
- **At install time only**: Windows Installer executes per-machine file copies and registry
  writes as SYSTEM. The MSI therefore contains no custom actions (no scripts, no EXE launched
  elevated), so no Helm code executes as SYSTEM.
- If step 0 finds that Frame Server hosts sources as SYSTEM on some build, the threat model above
  changes (DLL bug = SYSTEM compromise) and the design must be re-reviewed before proceeding.

### 6.3 Signing

- `helm_vcam.dll` and `HelmCamera.msi`: Authenticode, SHA-256, timestamped; same publisher as
  the Helm release artefacts. Release checksums continue to be published as today.
- The agent refuses to create the camera when the DLL signature is missing, invalid, or from a
  different publisher (`camera.caps` → `reason:"bad_signature"`), so a tampered or third-party
  DLL registered under our CLSID is never activated *by Helm* (Frame Server would still load
  whatever is registered — protecting that registration is the job of the HKLM ACL).
- No kernel drivers ⇒ no attestation signing, no EV-only requirement.

### 6.4 Residual risks (accepted for review)

- The DLL is long-lived inside a shared Windows service; a crash affects every camera on the
  PC until Frame Server restarts. Mitigated by minimal code + fuzzing, not eliminated.
- A user who installs the camera component has added admin-installed code to the machine; the
  uninstall must be complete and documented.
- Third-party virtual audio cable (audio option B) is outside Helm's control; Helm only
  renders audio into an endpoint the user picked.

## 7. Step plan

0. **Spike, no merge** (throwaway branch, test VM, Windows 11 23H2 + 24H2): confirm every
   **(verify)** item — Frame Server account and session, whether `Session` + `CurrentUser`
   creation works without admin, whether an unsigned source loads with/without Smart App
   Control, how the source reads per-camera properties, DirectShow/MF/WinRT visibility in Teams,
   Zoom, Chrome, Edge, Firefox, the Camera app and OBS. Record results in this document.
1. **Owner security review of this document** with spike results. Go / no-go.
2. **Protocol + phone**: `camera.caps/start/stop`, `/camera` socket, Camera2 → MediaCodec H.264
   encoder, foreground service `camera`, front/back switch, consent toggle. JVM tests for
   framing and limits (fake agent), like `StreamSessionTest`.
3. **Agent receive path** (`host/crates/helm-win-vcam`, no COM DLL yet): socket, limits, MF
   hardware H.264 decode, NV12 scale, ring writer + section with the §5.3 DACL; tests write a
   ring and a test reader validates it. Feature reported `available:false,
   reason:"not_installed"`.
4. **Media source DLL** in its own crate/project; header parser shared with step 3 tests and
   fuzzed (cargo-fuzz / libFuzzer) before first load. Second security review of the code.
5. **Installer**: separate signed per-machine MSI "Helm Camera" (files + HKLM CLSID only),
   uninstaller, agent signature/path check, tray indicator. Dev builds install only on a test
   VM.
6. **Release**: opt-in download, off by default per PC, documented in README with the privacy
   and uninstall notes. USB webcam mode (3.0) documented as the no-install alternative.
7. **Audio (v1.1)**: Opus from phone → WASAPI render to a user-chosen endpoint (option B). No
   driver.
8. **Later / separate review**: Windows 10 DirectShow fallback (3.2). Kernel components: not
   planned.
