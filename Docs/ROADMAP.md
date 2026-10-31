# Helm — Phase-by-Phase Build Plan

This is the working roadmap: every phase broken into concrete tasks, protocol methods,
implementation approach, tests and an acceptance gate. Feature IDs (A1, B2 …) refer to
`PLAN.md` §2. A phase is **done** only when its gate passes; then it is committed and the next
phase starts.

Rules that apply to every phase:
- Clean-room: all code is ours; other projects are idea references only (`CONTRIBUTING.md`).
- No AI features.
- Protocol changes go into `PROTOCOL.md` **before** code, so PC and phone agree.
- Each phase is built by parallel agents (PC side and phone side, split further when files don't
  overlap), then integrated, cross-tested, reviewed and committed.
- Dangerous actions (power, kill, files delete, terminal) require tier `admin` and are written to
  `audit.log`.

Status legend: ☐ not started · ◐ in progress · ☑ done

---

## Phase 0 — Foundations ☑ (2026-10-31: agent 42 tests, Android 44 tests, real E2E pair→auth→revoke OK)

Goal: phone finds the PC, pairs by QR over pinned TLS, and stays trusted after PC reboots.

| Task | Side |
|---|---|
| Repo, MIT license, clean-room CONTRIBUTING, protocol v1 contract | repo |
| Identity: host UUID, self-signed P-256 cert, DPAPI-protected key | PC |
| TLS WebSocket server, hello/pair/auth handshake, lockout, timeouts | PC |
| Module/router with permission tiers; `system.*`, `devices.*` | PC |
| mDNS `_helm._tcp`, pairing URI + console/SVG QR, audit log, CLI | PC |
| Pairing URI parser, Keystore signer, pinned TLS client, connection state machine | Phone |
| NSD discovery, saved PCs, Compose UI (home, connected), code-scanner, deep link | Phone |

**Gate:** `cargo test` + `gradlew testDebugUnitTest assembleDebug` pass; the Android client's
real E2E test pairs with the running Rust agent, reconnects with `auth` after an agent restart,
and revokes itself.

---

## Phase 1 — MVP "daily driver" ◐ (2026-10-31: agent 62 tests, Android 83 tests, real E2E of read-only methods OK; real-phone checklist pending)

Goal: replace WinDock for a week of normal use.

### 1.1 Protocol additions (write first)
- Add **fire-and-forget input frames** so touchpad motion doesn't wait for responses:
  `{"type":"in","k":"<kind>",…}` — no `id`, no reply. Kinds: `mm` (move dx,dy), `mb`
  (button, down), `ms` (scroll dx,dy), `kd`/`ku` (virtual key), `tx` (text).
- Add **binary frames** for file data: first 16 bytes transfer id, next 8 bytes offset (BE), rest data.
- Events become live: `media.changed`, `clipboard.changed`, `windows.changed`, `power.scheduled`.

### 1.2 PC agent work
| Area | Methods | Windows approach |
|---|---|---|
| App inventory (B1, B4) | `apps.list`, `apps.icon{app_id,size}` | Enumerate `shell:AppsFolder` via `IShellItem`/`IEnumShellItems` (covers Win32 shortcuts *and* packaged apps); icons via `IShellItemImageFactory::GetImage` → PNG; cache icons on disk |
| Launch (B1, B3) | `apps.launch{app_id}`, `open{target}` (file/folder/URL) | `IApplicationActivationManager` for packaged AUMIDs, `ShellExecuteExW` otherwise |
| Windows (B2) | `windows.list`, `windows.focus/minimize/maximize/close{hwnd}` | `EnumWindows` + visibility/owner/cloak filters; focus with `SetForegroundWindow` after `AllowSetForegroundWindow` / attach-input fallback; close with `WM_CLOSE` |
| Dock storage (B1) | `dock.get`, `dock.set{pages}` | JSON in data dir, synced to all phones via `dock.changed` event |
| Input (C1–C3) | `in` frames | `SendInput` (relative mouse, wheel/hwheel, VK keys, `KEYEVENTF_UNICODE` text); release held buttons/keys on disconnect |
| Text-focus hint (C2) | event `input.textFocus{focused}` | UI Automation focus-changed handler, check `IsKeyboardFocusable` + edit/document control types |
| Media (E1) | `media.state`, `media.control{action}`, event `media.changed` | WinRT `GlobalSystemMediaTransportControlsSessionManager` (title, artist, artwork thumbnail, timeline) |
| Volume (E2 master) | `audio.get`, `audio.set{volume,muted}` | Core Audio `IAudioEndpointVolume` + change callback |
| Brightness (E3) | `display.brightness.get/set` | WMI `WmiMonitorBrightness(Methods)` for panels; `dxva2` `Get/SetMonitorBrightness` (DDC/CI) for external monitors |
| Power (E4) | `power.action{action,delay_s}`, `power.cancel` | `LockWorkStation`, `SetSuspendState`, `InitiateShutdownW`/`ExitWindowsEx`; agent-side timer with `power.scheduled` event |
| Deck (H1–H2) | `deck.get`, `deck.set`, `deck.run{button_id}` | Actions: hotkey, text, launch, open, media, volume, delay — run in the agent |
| Files (G1) | `file.upload.begin/end`, `file.download{path}` | Chunked binary frames, SHA-256 check, save to `Downloads\Helm`; reject paths outside allowed roots |
| Clipboard (G3) | `clipboard.get/set{text|png}`, event `clipboard.changed` | Win32 clipboard + `AddClipboardFormatListener` on a hidden message window; opt-in auto-sync |
| Share URL (G6) | `open{target:url}` | same as launch |
| Screenshot (D5) | `screen.capture{monitor}` → PNG | GDI `BitBlt` from the desktop DC (fast enough for stills) |
| Wake-on-LAN (A10) | `system.info` adds `macs[]` | phone sends the magic packet itself |
| Tray (UI) | — | `tray-icon` crate: Show pairing QR (opens a small window/SVG), Paired devices, Open data folder, Start with Windows, Quit |

### 1.3 Android work
- Navigation shell: landscape **Dock** home + swipe-up drawer (Apps, Touchpad, Media & Controls, Deck, Files, Settings).
- Dock grid (4/6/8 per page), long-press to arrange, add from searchable app list, running badges.
- Touchpad screen: 1-finger move with acceleration, tap = click, 2-finger scroll/right-tap, pinch = Ctrl+wheel, drag lock, speed slider; special-keys bar; phone keyboard with auto-open on `input.textFocus`.
- Media card with artwork + controls, volume slider, brightness slider, power buttons with confirm + timer.
- Deck editor + runner (grid of colored buttons, icon picker, single action each).
- Share target: "Send to PC" for files, text, URLs; file receive into Downloads; clipboard push/pull + optional auto-sync while open.
- Wake-on-LAN button for saved PCs that are offline; screenshot viewer with save/share.
- Desk-dock mode: keep screen on, idle dim; themes (dark/AMOLED), haptics.

### 1.4 Agents
- PC-A: apps, windows, dock, open, deck, tray. PC-B: input, media, audio, brightness, power, clipboard, files, screenshot.
- Phone-A: dock, apps, deck, settings/themes. Phone-B: touchpad/keyboard, media/controls, files/share/clipboard, WoL.
- Each in its own git worktree; integrator merges and resolves conflicts.

**Gate:** all tests pass; scripted E2E (JVM client vs real agent) exercises every new method;
manual checklist on a real phone: launch/focus 5 apps, type a sentence, scroll a page, play/pause,
change volume, send a photo both ways, run a deck button, schedule + cancel shutdown.

---

## Phase 2 — "See the screen" ☐

Goal: live screen and full remote desktop on the LAN with < 50 ms glass-to-glass at 1080p60.

### Approach
- **Phase 2 transport:** a dedicated authenticated TLS WebSocket `/stream` carrying binary
  H.264 access units + Opus audio frames (LAN latency is fine; simpler than WebRTC). Phase 4 moves
  media onto WebRTC for internet use.
- Capture: **Windows.Graphics.Capture** (`Direct3D11CaptureFramePool`, free-threaded) per monitor
  or per window; fallback DXGI Desktop Duplication.
- Encode: **Media Foundation H.264 hardware encoder MFT** (NVENC/AMF/QSV under the hood), low-latency
  mode, CBR, no B-frames, GOP ~2 s, on-demand IDR; GPU texture → NV12 via video processor MFT.
- Audio: WASAPI **loopback** capture → Opus (libopus via `opus` crate) 20 ms frames.
- Adaptive bitrate from phone-reported receive stats; frame-drop when the socket backs up.
- Cursor: send cursor position + shape separately (overlay on phone) so it never smears.

### Methods
`stream.start{monitor|hwnd, max_w, max_h, fps, bitrate}`, `stream.stop`, `stream.idr`,
`stream.stats{…}` (phone → PC), `monitors.list`, `audio.stream{on}`; record: `record.start{audio}`,
`record.stop` (MP4 via MF sink writer with audio).

### Android
- `MediaCodec` H.264 decoder to a `SurfaceView` (low-latency flag where supported), jitter-free
  presentation (render immediately), Opus decode → `AudioTrack` low-latency.
- Remote desktop screen: **trackpad mode** (relative) and **direct-touch mode** (tap where you
  see), pinch-zoom/pan the view, keyboard overlay, monitor picker, quality presets, stats overlay.

**Gate:** latency harness (millisecond clock on PC screen, photographed next to phone) shows
median < 50 ms on 5 GHz Wi-Fi at 1080p60; 30-minute session with no crash or memory growth;
recording with audio plays back in Windows Media Player.

---

## Phase 3 — Power user ☐

| Area | Work |
|---|---|
| Monitor (F1, F4) | `stats.subscribe{interval}` event stream: CPU (per core), RAM, disks, network, battery via `sysinfo`/PDH; GPU load/VRAM via PDH "GPU Engine" counters + DXGI; temps/fans **optional** via LibreHardwareMonitor sidecar (needs admin). Alerts: thresholds evaluated on PC → event → Android notification |
| Task manager (F2) | `process.list` (CPU %, RAM, name, icon), `process.kill{pid}` (admin), `process.priority` |
| Audio mixer (E2, E5) | per-app volume via `IAudioSessionManager2`; output device list + switch (`IPolicyConfig`-free approach: `IMMDeviceEnumerator` + default-device set where supported); quick toggles: dark mode (registry + broadcast), mic mute, Wi-Fi/Bluetooth radios (`Windows.Devices.Radios`), Focus/DND where API exists |
| Macros (H3–H5) | multi-step macros (sequence, delay, repeat, if-window-active), stateful toggle buttons bound to real state (mic, mute, dark mode), live widgets on buttons (CPU %, clock, now playing) |
| Profiles (B5) | deck/dock profiles that switch automatically by foreground app (`SetWinEventHook EVENT_SYSTEM_FOREGROUND`) |
| App remotes (H6) | PowerPoint presenter (slides, timer, notes via keyboard + laser pointer overlay window), Spotify/VLC/YouTube (media session + hotkeys), browser (tabs, back/forward, zoom), OBS (obs-websocket v5 client) |
| Windows (B6) | move window to monitor, snap left/right/max, virtual desktops: switch via Win+Ctrl+arrows, create/close via shortcuts |
| File browser (G2) | `fs.list/stat/download/upload/open/delete/mkdir` under allowed roots; thumbnails for images |
| Security (A5, A6, A11) | tiers editable in tray UI, optional "confirm on PC" toast for admin actions, audit log viewer, biometric gate on phone for admin actions |
| Android native (J1, J2) | home-screen widgets (media, 4 deck buttons), Quick Settings tiles (lock PC, mute), persistent media notification |

**Gate:** each area has E2E coverage; a deck with 3 macros, a toggle and a live widget works on
the phone; alerts arrive when CPU > threshold.

---

## Phase 4 — Anywhere access ☐

| Area | Work |
|---|---|
| Relay (A8) | Cloudflare Worker + Durable Object per host for signalling (host keeps one outgoing WebSocket; phone connects through it); ends-to-end encryption stays phone↔PC (relay sees only ciphertext); TURN via a TURN provider for WebRTC fallback |
| WebRTC | control as a data channel, media as H.264/Opus tracks (`str0m` on PC, `webrtc-sdk` Android build on phone); LAN keeps the direct path |
| Multi-PC (A9) | switch PCs, per-PC dock/deck, background connections to several PCs |
| USB (A7) | USB tethering detection + "USB" badge; same protocol over the tether network |
| Remote WoL | wake a PC through another always-on Helm host on the same LAN |
| Input extras (C4, C6) | gyro air mouse; dictation via the phone keyboard (no AI on our side) |

**Gate:** with the phone on mobile data, pair-free reconnect to the home PC works and screen
stream runs at ≥ 720p30; relay logs show no plaintext.

---

## Phase 5 — Advanced ☐

| Area | Work |
|---|---|
| SYSTEM service | Windows service that starts the agent in the active session and handles input on UAC/secure desktop and lock screen (`WTSGetActiveConsoleSessionId`, `CreateProcessAsUser`) |
| Installer & updates | Inno Setup / WiX installer, firewall rule for Private profile, signed releases, in-app update check against GitHub Releases with Ed25519 signature verification |
| Plugins (H8) | WASM plugins (wasmtime) with a small capability API; deck import/export as files |
| Triggers (H7) | time, app start, phone connect, Wi-Fi network, NFC tag (Android) |
| Smart home (H9) | Home Assistant REST/WebSocket integration as a plugin |
| Phone as webcam/mic (D7) | Windows 11 `MFCreateVirtualCamera` (no driver needed) fed by the phone camera stream |
| Pen tablet (C7) | `InjectSyntheticPointerInput` with pressure/tilt from stylus |
| Second monitor (D6) | research: Indirect Display (IddCx) driver — needs driver signing; may stay optional |
| Gamepad (C5) | research: virtual controller options and their licenses |
| Notifications (G4) | Android `NotificationListenerService` → Windows toast via WinRT, reply actions |
| Terminal (F5) | ConPTY-backed PowerShell session, admin tier, fully audited |
| iOS client | after Android is stable |

**Gate:** per feature; release 1.0 when Phases 0–4 gates pass and the security review is clean.

---

## Phase 5b — remaining features + test-everything ◐ (started 2026-10-31)

| Step | Work | Where |
|---|---|---|
| A | MP4 recording (H.264 + AAC via MF sink writer), pen input (`InjectSyntheticPointerInput`), Windows toasts for phone notifications | `host/crates/helm-win-record`, `helm-win-pen`, `helm-win-notify` |
| B | ConPTY terminal sessions; phone-as-webcam spike (`MFCreateVirtualCamera`) | `host/crates/helm-win-terminal`, `helm-win-vcam` |
| C | WASM plugin host (wasmtime, capability API) + trigger engine (time, app start, phone connect, Wi-Fi) + Home Assistant client | `host/crates/helm-plugins` |
| D | `scripts/test-all.ps1` (every crate, agent, relay, Android, real E2E), GitHub Actions CI (tests only), `scripts/release.ps1` | `scripts/`, `.github/` |
| E | After merges: integrate A–C into agent + Android (protocol §14), security review, full test run, real E2E | agent, android |

## Release process (all phases)
- Version per phase: `0.1.x` = Phase 0/1, `0.2` = Phase 2, … `1.0` after Phase 4.
- GitHub Actions only runs tests (open-source contributors need CI); release builds are made
  locally with a script in `scripts/` and published to GitHub Releases with SHA-256 sums.
