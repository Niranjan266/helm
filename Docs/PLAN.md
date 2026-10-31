# PC Control App — Research & Master Plan

Working name: **Helm** (placeholder — rename any time).
Goal: one phone app that can control *everything* on a Windows PC — apps, input, media,
screen, files, system, automation — fast, encrypted, and on or off the home network.

Date: 2026-10-31

---

## 1. Research: what already exists

### 1.1 WinDock (the reference) — github.com/aliihsaad/WinDock

MIT licensed, ~185 stars, v0.2.6-preview (Oct 2026). Windows + Linux (Hyprland) host, Android + early iOS client.

**Architecture**
- Node.js host serves a web UI over **plain HTTP** on port 8620; UDP discovery on 41234.
- A C# (.NET 8) helper does the Windows work (launch, focus, volume, brightness, capture, input).
- Android app is a thin Kotlin shell around a **WebView** that loads the host's web UI.
- 6-digit PIN pairing; sessions are in memory (lost when host restarts).
- USB tethering and Tailscale IP supported manually.

**Features**
- App dock: Start-menu and packaged (UWP/MSIX) apps with real icons, pages of 4/6/8, drag to arrange.
- Controls: volume, brightness (WMI + DDC/CI), media play/pause, power actions.
- Capture: PNG screenshot, silent MP4 recording (≤1080p, 15 fps, 60 min max).
- Touchpad: move, tap, two-finger scroll/right-click, pinch-zoom (Ctrl+wheel), drag lock.
- Phone keyboard: auto-opens when a PC text field gets focus, Unicode/emoji.
- Signature pad → transparent PNG pasted into the active app.
- Switch-PC picker, update check in tray, appearance themes, performance mode.

**Weaknesses / gaps we can beat**
| Gap | Why it matters |
|---|---|
| Plain HTTP, no encryption | Anyone on the Wi-Fi can read keystrokes (their README admits it). |
| WebView client | Higher input latency, no native widgets / quick tiles / notifications. |
| No live screen view | You can't *see* the PC, only fire actions blindly. |
| No file transfer, clipboard sync, notifications | Basic KDE Connect features missing. |
| No macros / automation (listed "outside v1") | Biggest Stream-Deck-style value is missing. |
| No system monitor, task manager | Listed "outside v1". |
| LAN-only, manual Tailscale | No easy remote access. |
| Can't touch UAC / elevated apps / lock screen | Runs only in the user session. |
| No audio in recordings, no PC audio to phone | |
| Sessions lost on restart | Re-pair after every reboot. |

### 1.2 Other products in this space

| Product | Type | Strong points | What to borrow |
|---|---|---|---|
| **Unified Remote** | Closed, freemium | 100+ app-specific remotes, custom remotes, WoL, widgets, NFC tags, voice, screen viewer | App-specific remotes, NFC/widgets, custom remote layouts |
| **KDE Connect** | Open source (GPL) | Clipboard sync, file send, notification sync, SMS, battery, run commands, media, presenter | The "phone ↔ PC integration" feature set; plugin architecture |
| **Macro Deck** | Open source | Stream-Deck-style buttons, widgets, signed plugin ecosystem, Win/Mac/Linux | Button grid + plugin store + profiles |
| **Microsoft Phone Link** | Built into Windows | Notifications, calls, SMS, photos on PC | Notification mirroring UX (the reverse direction) |
| **Remote Mouse / PC Remote (Monect)** | Closed | Great touchpad, gamepad layouts, presentation mode | Gamepad + presenter layouts |
| **Chrome Remote Desktop / AnyDesk / TeamViewer** | Remote desktop | Full screen control from anywhere | Remote-desktop mode with trackpad vs direct-touch toggle |
| **RustDesk** | Open source remote desktop | Self-hostable relay, Rust, P2P with NAT traversal | Self-hosted relay/rendezvous design |
| **Sunshine + Moonlight** | Open source game streaming | Hardware encode (NVENC/AMF/QSV), 14–24 ms LAN latency, audio | Low-latency media pipeline design |
| **Moonlight-Web** | Open source | WebRTC + WebCodecs, <20 ms over Wi-Fi, virtual display driver | WebRTC for media, virtual display for headless use |
| **LocalSend** | Open source | Fast LAN file transfer, no account | File transfer protocol ideas |
| **PCLink** (Reddit project) | Hobby | "KDE Connect but more" — remote control + management | Confirms demand for an all-in-one tool |

**Takeaway:** nobody combines *all* of: launcher dock + Stream-Deck macros + touchpad/keyboard +
live low-latency screen + KDE-Connect integration + system/task manager + secure remote access,
in one native, encrypted app. That combination is the product.

### 1.3 Key technical facts from research
- Hardware video encode (NVENC/AMF/QuickSync via Media Foundation) adds ~5–15 ms; software encode adds 30–100 ms. Use hardware.
- WebRTC is the proven transport for screen + audio + input with NAT traversal (Moonlight-Web, RustDesk-like setups).
- Windows foreground-focus rules can block "bring app to front" (WinDock hit this). Workarounds: `AllowSetForegroundWindow`, simulated Alt key trick, or `SetForegroundWindow` from a process that received the last input.
- Controlling UAC prompts / secure desktop / lock screen needs a **Windows service running as SYSTEM** that launches a helper into the active session (how AnyDesk/RustDesk do it).
- Brightness: WMI for laptop panels, DDC/CI (`dxva2` monitor configuration API) for external monitors.
- Media: `GlobalSystemMediaTransportControlsSessionManager` (WinRT) gives now-playing title, artwork, position and per-app control — better than media keys.
- Per-app volume: Core Audio `IAudioSessionManager2`.

---

## 2. Product vision & feature list

Priority: **P0** = MVP, **P1** = v1.0, **P2** = later/advanced.

### A. Connection & security
| # | Feature | Pri |
|---|---|---|
| A1 | Auto-discovery on LAN (mDNS `_helm._tcp`) + manual IP | P0 |
| A2 | **QR-code pairing** (host shows QR with address + cert fingerprint + one-time key) | P0 |
| A3 | End-to-end encryption (TLS 1.3 with pinned self-signed cert) on every channel | P0 |
| A4 | Persistent trusted devices (survive reboots), revoke from PC or phone | P0 |
| A5 | Per-device permission tiers: *Viewer*, *Remote*, *Admin* (power, terminal, files) | P1 |
| A6 | Confirmation on PC for dangerous actions (optional), audit log of every action | P1 |
| A7 | USB connection (USB tethering / ADB reverse) for zero-Wi-Fi use | P1 |
| A8 | Remote access outside home: built-in Tailscale detection + own relay (WebRTC TURN/rendezvous on Cloudflare) | P1 |
| A9 | Multi-PC: switch between PCs, per-PC layouts | P1 |
| A10 | Wake-on-LAN (and via relay when away) | P0 |
| A11 | Biometric lock on the phone app before Admin actions | P1 |

### B. Launcher & dock (WinDock parity, then beyond)
| # | Feature | Pri |
|---|---|---|
| B1 | Pin apps (Win32 + UWP/MSIX) with real icons, pages, drag to reorder | P0 |
| B2 | Running-app badges, focus / minimize / close / kill | P0 |
| B3 | Open files, folders, URLs, Steam games, scripts as tiles | P0 |
| B4 | Search all installed apps | P0 |
| B5 | Profiles (Work / Gaming / Editing) that auto-switch by foreground app | P1 |
| B6 | Window manager: move window to monitor, snap layouts, maximize, virtual desktops switch/create | P1 |
| B7 | Live window thumbnails in the task switcher | P2 |

### C. Input
| # | Feature | Pri |
|---|---|---|
| C1 | Touchpad: move, tap, 2-finger scroll / right-click, pinch-zoom, 3-finger gestures (task view, desktop) | P0 |
| C2 | Phone keyboard with auto-open on text focus, Unicode/emoji | P0 |
| C3 | Special keys bar: Ctrl/Alt/Win/Fn, F1–F12, arrows, shortcuts | P0 |
| C4 | Gyro "air mouse" | P1 |
| C5 | Virtual gamepad (Xbox layout) via ViGEm-style driver or `Windows.Gaming.Input` injection | P2 |
| C6 | Voice typing (phone speech-to-text → PC) | P1 |
| C7 | Pen/stylus drawing tablet mode with pressure (Windows Ink injection `InjectSyntheticPointerInput`) | P2 |
| C8 | Signature pad → paste as image (WinDock parity) | P1 |

### D. Screen & media streaming
| # | Feature | Pri |
|---|---|---|
| D1 | **Live screen view** (low-latency, hardware H.264/HEVC, WebRTC) | P1 |
| D2 | Full **remote desktop**: tap-to-click on the stream, trackpad mode or direct-touch mode, zoom/pan | P1 |
| D3 | Choose monitor / single window streaming | P1 |
| D4 | PC audio → phone (listen to PC sound remotely) | P1 |
| D5 | Screenshot (save on PC and/or send to phone), screen recording **with audio** and cursor | P0 (screenshot) / P1 |
| D6 | Phone as **second monitor** (virtual display driver) | P2 |
| D7 | Phone camera/mic as PC webcam/mic | P2 |

### E. Media & system controls
| # | Feature | Pri |
|---|---|---|
| E1 | Now-playing card with artwork, seek bar, play/pause/next (GSMTC) | P0 |
| E2 | Master volume + **per-app volume mixer**, output device switch | P0 / P1 |
| E3 | Brightness (laptop + DDC/CI monitors), night light toggle | P0 |
| E4 | Power: lock, sleep, hibernate, restart, shutdown, sign out, **scheduled shutdown timer** | P0 |
| E5 | Quick toggles: Wi-Fi, Bluetooth, dark mode, focus assist/DND, mic mute, airplane | P1 |
| E6 | Display: resolution, refresh rate, primary monitor, projection mode (Win+P) | P2 |

### F. Monitoring & management
| # | Feature | Pri |
|---|---|---|
| F1 | Live dashboard: CPU, GPU, RAM, disk, network, temps, fans, battery (LibreHardwareMonitor) | P1 |
| F2 | Task manager: list processes, sort by CPU/RAM, end task, priority | P1 |
| F3 | Services list: start/stop (Admin tier) | P2 |
| F4 | Alerts: notify phone when CPU temp high, download finished, render done, PC idle/awake | P1 |
| F5 | Remote PowerShell/terminal (Admin tier, audited) | P2 |
| F6 | Event log / crash viewer, disk space cleaner shortcut | P2 |

### G. Phone ↔ PC integration (KDE-Connect level)
| # | Feature | Pri |
|---|---|---|
| G1 | **File transfer** both ways (Android share sheet "Send to PC", drag-drop on PC tray) | P0 |
| G2 | Remote file browser: browse PC drives, download, upload, open on PC, delete (Admin) | P1 |
| G3 | **Clipboard sync** (text + images), manual or automatic | P0 |
| G4 | Phone notifications mirrored to PC (Windows toast), reply from PC | P2 |
| G5 | "Find my phone" (ring) from PC, phone battery shown on PC | P1 |
| G6 | Open phone link on PC browser (share URL → PC) | P0 |

### H. Automation & macros (Stream-Deck level)
| # | Feature | Pri |
|---|---|---|
| H1 | **Button deck**: custom grid of buttons with icons/colors | P0 |
| H2 | Action types: hotkey, type text, launch, open URL, run script, media, volume, delay, mouse click at position | P0 |
| H3 | **Multi-step macros** with delays, loops, conditions | P1 |
| H4 | Toggle/stateful buttons (e.g. mic on/off reflects real state) | P1 |
| H5 | Live widgets on buttons (CPU %, clock, now playing) | P1 |
| H6 | App-specific remotes: PowerPoint presenter (+laser pointer), VLC, Spotify, YouTube, browser, OBS (WebSocket API), Discord mute | P1 |
| H7 | Triggers: on time, on app start, on phone connect, on NFC tag, on Wi-Fi network | P2 |
| H8 | Plugin SDK (sandboxed plugins, JS or WASM) + import/export decks as files | P2 |
| H9 | Home Assistant / smart home integration (lights with the PC) | P2 |

### I. No AI (decided 2026-10-31)
The app has **no AI features** — no cloud AI API and no local model. Everything is
deterministic: buttons, macros, scripts and rules the user configures.

### J. Phone-side native extras
| # | Feature | Pri |
|---|---|---|
| J1 | Home-screen widgets (media, quick buttons), Quick Settings tiles (lock PC, mute) | P1 |
| J2 | Persistent media notification with controls | P1 |
| J3 | Landscape "desk dock" mode with idle dimming (WinDock parity) | P0 |
| J4 | Tablet layouts, Wear OS quick controls | P2 |
| J5 | Themes, dark/AMOLED, haptics | P0 |

---

## 3. Architecture

### 3.1 Big picture

```
 ┌──────────────── PHONE (Android, Kotlin + Jetpack Compose) ────────────────┐
 │  UI: Dock · Deck · Touchpad · Screen · Files · Monitor · Settings         │
 │  Core: discovery (mDNS) · pairing (QR) · TLS client · WebRTC client       │
 │  Native: widgets · QS tiles · share target · notification listener        │
 └───────────────┬───────────────────────────────┬───────────────────────────┘
                 │ Control channel               │ Media channel
                 │ WebSocket over TLS 1.3        │ WebRTC (DTLS-SRTP)
                 │ (protobuf messages)           │ video · audio · input datachannel
                 ▼                               ▼
 ┌──────────────────────── WINDOWS PC ─────────────────────────────────────────┐
 │  helm-agent (user session, Rust)                                            │
 │   ├─ API router + permission check + audit log                              │
 │   ├─ modules: apps · input · media · audio · display · power · files ·      │
 │   │           clipboard · capture · monitor · macros · plugins · ai         │
 │   ├─ media engine: Windows.Graphics.Capture → Media Foundation HW encoder   │
 │   │                → WebRTC (str0m / webrtc-rs)                             │
 │   └─ tray + settings window (Tauri)                                         │
 │  helm-service (SYSTEM, optional, P2) — UAC/lock-screen input, wake, update  │
 └────────────────────────────────┬────────────────────────────────────────────┘
                                  │ (only when away from home)
                    ┌─────────────▼──────────────┐
                    │ Relay (Cloudflare Worker + │
                    │ Durable Object signalling, │
                    │ TURN for fallback)         │
                    │ never sees decrypted data  │
                    └────────────────────────────┘
```

### 3.2 Technology choices (with reasons)

| Part | Choice | Why | Alternative |
|---|---|---|---|
| PC agent | **Rust** + `windows-rs` | Single small binary, fast, safe, full Win32/WinRT/COM access, good WebRTC crates | C# .NET 8 (easier WinRT, bigger runtime) — fine if you prefer C# |
| PC UI (tray, settings, QR) | **Tauri 2** | You already used it in Nibi; tiny, web UI | WinUI 3 |
| Phone app | **Kotlin + Jetpack Compose** (native) | Lowest latency, widgets/tiles/share-target, MediaCodec HW decode | Flutter (if iOS soon matters more) |
| Control protocol | WebSocket + TLS 1.3, **protobuf** messages | Typed, versioned, compact, both sides codegen | JSON (simpler, use for prototype) |
| Media | **WebRTC** | NAT traversal, HW decode on Android, audio sync, datachannel for low-latency input | Custom UDP (more work) |
| Screen capture | Windows.Graphics.Capture (fallback DXGI Desktop Duplication) | Modern, per-window or per-monitor, GPU texture | — |
| Encode | Media Foundation H.264/HEVC (NVENC/AMF/QSV under the hood) | HW encode, no extra install | FFmpeg (licensing/size) |
| Hardware stats | LibreHardwareMonitor (via small sidecar) or WMI/PDH | Temps/fans need LHM | — |
| Storage (PC) | SQLite (settings, decks, devices, audit log) | Simple, local | JSON files |
| Relay | Cloudflare Workers + Durable Objects (signalling), coturn/Cloudflare Calls TURN | Free tier, you already use CF | Self-hosted on VPS |

### 3.3 Protocol sketch

Every message: `Envelope { id, type, device_id, timestamp, payload }`.

- **Request/response**: `apps.list`, `apps.launch`, `audio.setVolume`, `power.shutdown{delay}` …
- **Events (PC → phone)**: `media.changed`, `window.focused`, `stats.tick`, `clipboard.changed`, `textFocus.changed`.
- **Streams**: high-rate input (mouse deltas) goes over the WebRTC datachannel (unreliable/unordered) when the stream is open, otherwise over the WebSocket.
- Every request is checked against the device's permission tier; Admin requests are written to the audit log.
- Versioning: `hello { protocol_version, features[] }` on connect so old phones and new hosts degrade gracefully.

### 3.4 Pairing & security flow
1. First run: host generates a long-term key pair + self-signed cert, stores the private key with DPAPI.
2. Phone scans QR → gets `{host_id, addresses, port, cert_sha256, one_time_token}`.
3. Phone connects over TLS, **verifies cert fingerprint** (pinning), sends token + its own public key.
4. Host shows "Allow *Pixel 8*?" toast → user clicks Allow → device saved with tier.
5. Afterwards: mutual auth by keys; no PIN needed again, survives reboots.
6. Rate-limit and lockout on bad tokens; tokens expire in 5 minutes.
7. Firewall rule added by installer for **private networks only**.

---

## 4. Repository layout

```
windows control/
├─ Docs/                 PLAN.md, PROTOCOL.md, SECURITY.md, decisions/
├─ proto/                *.proto shared message definitions
├─ host/
│  ├─ agent/             Rust crate: core, router, modules/*, media/
│  ├─ ui/                Tauri app (tray, settings, QR pairing, deck editor)
│  ├─ service/           (P2) SYSTEM service
│  └─ installer/         WiX or Inno Setup script
├─ android/              Kotlin + Compose app (feature modules per area)
├─ relay/                Cloudflare Worker (signalling) — P1
├─ plugins/              example plugins (OBS, Spotify, PowerPoint)
├─ scripts/              build / release scripts (local, like your Dangle release.ps1)
└─ tests/                protocol conformance + integration tests
```

---

## 5. Phased roadmap

Estimates assume one developer + Claude Code, part-time. Each phase ends with a usable build.

### Phase 0 — Foundations (week 1)
- Repo setup, `proto/` v0, Rust agent skeleton with module trait, Compose app skeleton.
- mDNS discovery, TLS server with self-signed cert, QR pairing, trusted-device storage.
- **Done when:** phone finds the PC, pairs by QR, reconnects after PC reboot without re-pairing.

### Phase 1 — MVP "daily driver" (weeks 2–4)
- Dock: app inventory (Win32 + UWP), icons, launch/focus/close, running badges (B1–B4).
- Touchpad + keyboard + special keys (C1–C3).
- Media card (GSMTC), master volume, brightness, power + shutdown timer (E1–E4).
- Button deck v1 with single actions (H1–H2).
- File send both ways + clipboard sync + share URL (G1, G3, G6).
- Screenshot to phone, Wake-on-LAN, landscape dock mode, themes.
- Tray app with QR, device list, logs.
- **Done when:** you use it for a full week instead of WinDock without missing anything.

### Phase 2 — "See the screen" (weeks 5–7)
- Capture → HW encode → WebRTC pipeline; Android MediaCodec decode via WebRTC.
- Live screen view, then full remote desktop (trackpad + direct touch), monitor picker (D1–D3).
- PC audio to phone (D4), recording with audio (D5).
- Input moved to WebRTC datachannel while streaming.
- **Target:** < 50 ms glass-to-glass on 5 GHz Wi-Fi at 1080p60; adaptive bitrate.

### Phase 3 — Power user (weeks 8–10)
- System monitor dashboard + alerts, task manager (F1, F2, F4).
- Per-app volume mixer, output device switch, quick toggles (E2, E5).
- Multi-step macros, stateful toggles, live widgets, profiles by foreground app (H3–H5, B5).
- App-specific remotes: PowerPoint presenter, Spotify, VLC, YouTube, OBS (H6).
- Window manager + virtual desktops (B6).
- Remote file browser (G2), permission tiers + audit log + biometric lock (A5, A6, A11).
- Android widgets, QS tiles, media notification (J1, J2).

### Phase 4 — Anywhere access (weeks 11–13)
- Cloudflare relay (signalling + TURN), works on mobile data (A8); multi-PC (A9).
- Voice typing via the phone's own keyboard dictation (C6) — no AI on our side.
- USB mode (A7), gyro air mouse (C4).

### Phase 5 — Advanced (later)
- SYSTEM service for UAC/lock screen control, auto-update.
- Plugin SDK + deck import/export, triggers (NFC, time, Wi-Fi), Home Assistant.
- Phone as second monitor (virtual display driver), phone as webcam/mic, gamepad, pen tablet.
- Notification mirroring phone → PC, remote terminal, iOS client.

---

## 6. Testing & quality

- **Unit tests**: each Rust module behind a trait so it can be mocked (no real shutdowns in tests!).
- **Protocol conformance tests**: shared fixtures in `tests/` run against both agent and Android client.
- **Integration**: headless agent in CI on a Windows runner (GitHub Actions or local script) — launch a test window, focus, type, read back.
- **Android**: Compose UI tests + emulator against a fake host.
- **Latency harness**: on-screen timestamp → phone camera / frame stamp comparison; record numbers per release.
- **Security checks**: fuzz the message parser (`cargo fuzz`), verify unpaired devices get nothing, verify tier enforcement.
- **Manual acceptance gates** per phase on a *real phone* (WinDock's lesson: emulator ≠ real device).

---

## 7. Risks & mitigations

| Risk | Mitigation |
|---|---|
| Windows blocks `SetForegroundWindow` | Use input-attached trick / `AllowSetForegroundWindow`; fall back to minimize+restore; document limits |
| Can't control UAC prompts / elevated apps from user session | Phase 5 SYSTEM service; until then show clear "elevated window — can't control" message |
| Antivirus flags input-injection / remote-control binaries | Code-sign the installer, avoid packers, submit to Microsoft Defender for review |
| Wi-Fi latency spikes | Adaptive bitrate, HEVC, lower fps on congestion, USB mode |
| Security hole = someone controls your PC | TLS + pinning, mutual keys, tiers, audit log, private-network firewall only, relay never sees plaintext, external review before public release |
| Feature creep (this list is huge) | Ship strictly by phase; P2 items are a backlog, not a promise |
| Driver needs (gamepad, virtual display) | Use existing signed drivers where licenses allow; keep them optional add-ons |
| Licensing / originality | Clean-room rule: read other projects (MIT or GPL) for ideas only, never copy their code; all code here is original and MIT licensed |

---

## 8. Decisions

Decided 2026-10-31:
- **Stack** — Rust PC agent, native Kotlin + Compose Android app (the recommended stack).
- **Fully open source**, MIT license. Clean-room rule: other projects (WinDock, KDE Connect,
  Sunshine, RustDesk, …) are studied for *ideas* only; **no code is copied** from any of them.
  Everything in this repo is our own work. See `CONTRIBUTING.md`.
- **No AI** of any kind (no API, no local model).
- Phase 0 uses **JSON** messages (documented in `Docs/PROTOCOL.md`); protobuf can come later
  if message volume needs it.

Still open:
1. **Name** — "Helm" is a working name.
2. **Remote access** — own Cloudflare relay (recommended) or Tailscale-only (needed by Phase 4).
