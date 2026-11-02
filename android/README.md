# Helm — Android app

The phone side of Helm. Phase 0: find PCs on the LAN, pair by QR code, connect over a
pinned-TLS WebSocket, remember trusted PCs. Phase 1 (PROTOCOL.md §9): Dock home, apps and
windows, touchpad + keyboard, media/volume/brightness/power, Deck, files/sharing/clipboard/
screenshots, Wake-on-LAN, desk-dock settings. Phase 2 (§10, feature `p2`): the Screen tab —
live H.264 screen + Opus audio over a second pinned-TLS `/stream` socket, trackpad or
direct-touch input, pinch-zoom/pan, keyboard overlay, monitor picker, quality presets, stats
overlay and auto-reconnect. Phase 3 (§11, feature `p3`): Monitor (live `stats.tick` charts,
alerts editor -> Android notifications), task manager, per-app mixer, window snap/move and
virtual desktops, deck macros/repeat/app remotes/live state/widgets, profiles, PC file browser,
access levels + audit log, biometric check before admin actions, home-screen widget, Quick
Settings tiles (lock PC, mute) and a persistent media notification. Phase 4 (§12, `p4`): relay
fallback (pinned TLS inside the relay tunnel, never pairing), PC switcher with per-PC settings,
wake via another awake PC (`wol.send`), gyro air mouse (`gy`) and a "USB"/"Relay" link badge.
Phase 5 (§13, `p5`): record start/stop and pen mode (`pn` from stylus pressure/tilt/hover/
eraser, fingers ignored) on the Screen tab, a Terminal (ConPTY output in a small VT screen with
colours, input line + special keys, resize on layout change; admin + biometric check),
notification mirroring (NotificationListenerService, per-app allow-list in Settings, off by
default; toast actions and RemoteInput replies come back via `notify.action`), Plugins (enable,
grant capabilities), Triggers editor, and Home Assistant (connection, states, `ha` deck action).
Every p3/p4/p5 call is gated on the hello feature. The wire contract is
[`Docs/PROTOCOL.md`](../Docs/PROTOCOL.md).

Everyday (g6, [`Docs/protocol/everyday.md`](../Docs/protocol/everyday.md)): Clipboard history
(list, on/off + encrypted persist, copy to phone / share / copy on PC, delete, clear; refreshes
on `clipboard.history.changed`), quick file drop (files shared to Helm get a progress
notification and `file.drop` with the "When a file arrives on the PC" setting: open / save /
reveal), Find my phone (`phone.ring` -> full-screen alarm on the alarm stream at full volume,
vibration, flashing screen and torch until "Found it"; "Ring another phone" screen), PC
notifications (Settings toggle, off by default; per-app filter, every app off until enabled or
"All apps"; channel "PC notifications"), a Schedules editor (time, Mon..Sun or one-off date,
deck or power action) and a PC health line/card (battery, CPU temperature, system drive, uptime;
every 60 s). New code: `core/EverydayModels.kt`, `core/HelmApiG6.kt`, `device/EverydayBackground.kt`,
`device/PhoneRinger.kt`, `device/RingActivity.kt`, `ui/EverydayViewModel.kt` and the screens.
Every g6 call is gated on the hello feature.

## Layout

```
app/src/main/java/com/niranjan/helm/
  core/     plain Kotlin/JVM, no android.* imports (unit-tested on the JVM)
    PairingUri.kt       helm://pair?... parser + validation (§4)
    Identity.kt         DeviceSigner interface, software signer, signed-string format (§5)
    PinnedTls.kt        fingerprint pinning trust manager, OkHttp transport (§1)
    HelmConnection.kt   WebSocket client: hello -> pair/auth -> ready, req/res, state flow
    SavedPc.kt          saved-PC model + JSON codec, address ordering (+ WoL MACs)
    Protocol.kt         `in` input frames, VK codes, binary file frames (§9.1-9.2)
    Models.kt           Phase 1 result models, dock/deck codecs and dock editing
    HelmApi.kt          typed calls for every §9.3 method
    Touchpad.kt         acceleration, per-frame move/scroll batching, text-box diff
    WakeOnLan.kt        magic packet, MAC choice, UDP broadcast
    StreamFrames.kt     §10 binary stream frames (0x01-0x05) parser/encoder
    H264.kt             Annex-B NAL splitting, SPS parser, decoder (re)configure tracker
    StreamModels.kt     quality presets, stream.start params/result, stats counter, OpusHead
    ViewTransform.kt    zoom/pan of the remote screen + direct-touch coordinate mapping
    StreamSession.kt    stream.start -> /stream auth -> stream.open -> frames, stream.stats
    Phase3Models.kt     stats, alerts, processes, audio sessions, profiles, fs entries, PcPath
    HelmApiP3.kt        feature-gated calls for every §11/§12 method
    Tunnel.kt           relay tunnel v1: connect URL, 64 KiB chunks, mux frame codec, close codes
    RelayTransport.kt   TunnelSocket (java.net.Socket over the relay WebSocket) + RelayTransport
    Phase4.kt           gyro air-mouse maths, USB tethering detection
    Phase5Models.kt     recording, term events/keys/size, notify push/action + allow-list,
                        plugins, triggers, Home Assistant states
    TermScreen.kt       VT100/xterm screen model (SGR colours, cursor, erase, scrollback)
    HelmApiP5.kt        feature-gated calls for every §13 method + pen tilt maths
  device/   Android pieces: Keystore signer, stores, NSD discovery, settings, icon disk cache,
            content-URI reading and gallery/Downloads saving, VideoDecoder (MediaCodec ->
            SurfaceView) and AudioPlayer (Opus MediaCodec -> low-latency AudioTrack)
  ui/       Compose (Material 3): Home, RemoteShell (drawer + rail/bottom bar) and the Dock,
            Apps, Touchpad, Media & Controls, Deck, Files, Settings screens
  HelmApp.kt            app-scoped holder for identity, stores, discovery and the connection
```

## Build

Requirements: Android SDK (platform 34), and **JDK 17 or 21**. AGP does not run on newer JDKs,
so point `JAVA_HOME` at Android Studio's bundled JBR (Java 21):

```powershell
$env:JAVA_HOME = "C:\Program Files\Android\Android Studio\jbr"
cd android
.\gradlew.bat testDebugUnitTest assembleDebug
```

`local.properties` must contain `sdk.dir=...` (Android Studio writes it; it is git-ignored).

The debug APK is `app/build/outputs/apk/debug/app-debug.apk`. Install with
`adb install -r app\build\outputs\apk\debug\app-debug.apk`.

You can start pairing without a camera by sending the deep link:

```powershell
adb shell am start -a android.intent.action.VIEW -d "'helm://pair?v=1&id=...&t=...'"
```

## Tests

`.\gradlew.bat testDebugUnitTest` runs JVM tests only (no device needed):

- `PairingUriTest` — parsing, URL-encoded names, bad input.
- `SigningTest` — exact `helm-v1|<purpose>|<host_id>|<nonce>` string, SPKI key, DER signature.
- `PinnedTlsTest` — SHA-256 pinning trust manager, real TLS against MockWebServer.
- `HelmConnectionTest` — a fake agent on MockWebServer: pair success, `bad_token`, auth,
  wrong key, host mismatch, address fallback, req/res (incl. out-of-order), timeouts, and the
  whole handshake over `wss://` with pinning.
- `SavedPcTest` — saved-PC JSON and address order.
- `Phase1ConnectionTest` — features, events, input frames (no id), chunked upload + hash
  mismatch, download via binary frames + `file.done`, dock/deck set + change events.
- `Phase1CodecTest` — input frame shapes, binary frame layout, dock/deck codecs and editing,
  result parsing, Wake-on-LAN packet/MAC choice/UDP send, touchpad maths and text diff.
- `Phase3CodecTest` — mux frames, tunnel chunking, relay URLs, saved-PC relay/prefs, deck
  macro/repeat/remote/state/widget codecs, alerts/process/profile/fs codecs, paths, gyro, USB.
- `Phase3ConnectionTest` — every §11/§12 method against the FakeAgent (events included) and
  feature gating (`unsupported`, nothing sent).
- `Phase5CodecTest` — VT screen (wrap, SGR, CUP/erase, split UTF-8, insert/delete, alt screen,
  resize, scroll region), terminal keys/size, `pn` frames + tilt, notify JSON/allow-list,
  plugins, triggers (unknown kept), HA states and the `ha` deck action.
- `Phase5ConnectionTest` — every §13 method against the FakeAgent (term output/exit and
  `notify.action` events included) and `p5` gating.
- `EverydayCodecTest` — clipboard items/pick, drop modes, ring events + alarm-volume
  save/restore (fake volume), PC-notification access and per-app filter, schedule codec (unknown
  actions kept, read-only fields not sent), health parsing and formatting.
- `EverydayConnectionTest` — every g6 method against the FakeAgent (`FakeEveryday`): events
  (`clipboard.history.changed`, `phone.ring`, `pc.notification`, `schedule.changed`/`ran`),
  `forbidden` power schedules, and `g6` gating (`unsupported`, nothing sent).
- `RelayTunnelTest` — a fake relay (MockWebServer + mux framing) in front of a TLS FakeAgent:
  LAN-fail -> relay auth with real pinned TLS, 700 KB download through the tunnel, the relay
  sees no plaintext, pin mismatch, host offline (404), `/stream` over relay, pairing refused.
- `E2eRealAgentTest` — **opt-in**, skipped unless `HELM_E2E_URI` is set (below).

## Opt-in end-to-end test against the real PC agent

1. Start the Rust agent (`host/agent`) and copy the `helm://pair?...` link it prints
   (the token is single-use and valid for 5 minutes).
2. Run, from `android/`:

```powershell
$env:JAVA_HOME = "C:\Program Files\Android\Android Studio\jbr"
$env:HELM_E2E_URI = 'helm://pair?v=1&id=...&n=...&a=...&p=47820&fp=...&t=...'
.\gradlew.bat testDebugUnitTest --tests "com.niranjan.helm.core.E2eRealAgentTest" --rerun
Remove-Item Env:HELM_E2E_URI
```

Use single quotes so PowerShell does not touch the `&` characters. The test pairs with a fresh
software key over the real pinned TLS client, calls `system.ping` and `system.info`,
disconnects, reconnects with `auth`, calls `devices.list`, revokes itself with
`devices.revoke`, and checks that a further `auth` gets `unknown_device`. If the agent
advertises `p1` it also calls the read-only `apps.list`, `windows.list`, `media.state`,
`audio.get`, `dock.get`, `deck.get`, `screen.monitors` and `system.info` (macs) before revoking. Results are printed
with a `[helm-e2e]` prefix.
