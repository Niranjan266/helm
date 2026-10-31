# helm-agent

The Windows PC side of Helm. Phase 0: secure discovery, QR pairing and trusted devices.
Phase 1 (`features:["p1"]`): apps, windows, input, media, volume, brightness, power, dock,
deck, clipboard, file transfer, screenshots and live events.
Phase 2 (`features:["p1","p2"]`): live screen (H.264) + system audio (Opus) + cursor overlay on
a second `/stream` socket, adaptive bitrate, direct-touch `ma` input.
The wire contract is [`Docs/PROTOCOL.md`](../../Docs/PROTOCOL.md).

## Build

Needs Rust (stable, MSVC toolchain). No C toolchain beyond MSVC is needed (rustls uses `ring`).

```powershell
cargo build --release      # target\release\helm-agent.exe
cargo test                 # unit + end-to-end tests (TLS + WebSocket, in-process)
cargo clippy --all-targets
cargo test --test phase2 -- --ignored --nocapture   # live: streams this PC's screen 2 s, injects nothing
```

`Cargo.toml` builds `unsafe-libopus` (helm-win-audio's codec) optimised even in dev/test.

Tests never touch this PC: they run against `platform::fake::FakePlatform` (records calls,
canned data) with `Config::power_dry_run = true`, and save/serve files in temp folders.

## Run

```powershell
helm-agent                         # run; shows a pairing QR if no phone is paired yet
helm-agent --pair                  # always show a pairing QR at start
helm-agent devices                 # list paired devices and exit
helm-agent revoke <device_id>      # remove a paired device and exit
```

While it runs, type `p` + Enter for a fresh pairing QR (new one-time token, valid 5 minutes),
`q` + Enter or Ctrl+C to quit.

| flag | meaning |
|---|---|
| `--data-dir <path>` | data folder (default `%APPDATA%\Helm`) |
| `--port <n>` | TCP port (default 47820; if busy, a free port is used and advertised; `0` = any) |
| `--no-mdns` | don't advertise `_helm._tcp.local.` on the network |
| `--pair` | show a pairing QR at start even if devices are paired |
| `--no-tray` | console only, no notification-area icon |
| `--background` | no console window; the tray icon is the UI (used by Start with Windows) |

### Tray icon

Right-click the Helm icon in the notification area:
**Show pairing QR** (new token, opens `pair-qr.svg` in the default browser),
**Paired devices** (opens the data folder for now), **Start with Windows** (toggles
`HKCU\Software\Microsoft\Windows\CurrentVersion\Run\Helm` = `"<exe>" --background`),
**Quit Helm**.

Log level: set `RUST_LOG` (e.g. `RUST_LOG=debug`).

The QR is printed in the console (drawn for a dark background), the link is printed as text,
and `pair-qr.svg` is written into the data folder (deleted again when the agent stops).

## Where data lives (`%APPDATA%\Helm`)

- `identity.json` — host id, self-signed certificate, private key encrypted with Windows DPAPI
  (current user). Copying it to another user/PC will not work, by design.
- `devices.json` — paired phones (public keys, tiers, timestamps).
- `audit.log` — one tab-separated line per pairing, auth success/failure and revoke.
- `pair-qr.svg` — current pairing QR while the agent runs.
- `dock.json`, `deck.json` — dock pages and deck buttons (written atomically, synced to all
  phones with `dock.changed` / `deck.changed`).
- `icon-cache\` — app icon PNGs.

Files sent from the phone go to `<Downloads>\Helm` (name made safe, never overwritten:
`a (1).txt`). `file.download` (admin) only serves regular files under the user's profile
folder, never the Helm data folder, at most 4 GiB, SHA-256 announced first.

## Architecture (Phases 1–2)

- `platform/` — the `Platform` trait: every OS call (apps, windows, input injector, media,
  volume, brightness, clipboard, adapters/battery, monitors/screenshots, watchers).
  `real.rs` uses the `helm-win-apps`, `helm-win-input`, `helm-win-system`,
  `helm-win-capture` and `helm-win-audio` crates (plus `cursor.rs`); `fake.rs` is the test double. Calls are blocking and run in
  `spawn_blocking`.
- `session.rs` — per connection: outgoing queue (text + binary frames with backpressure),
  one injector thread for `in` frames (`release_all` on disconnect), uploads, clipboard-watch
  flag, work to run after a response (download streaming, immediate power actions).
- `server.rs` — session registry and `broadcast`; watcher events → `media.changed`,
  `audio.changed`, `input.textFocus`, `clipboard.changed` (watching sessions only); window
  list polled every 2 s while phones are connected → `windows.changed`.
- `stream/` (Phase 2) — `mod.rs` pure parts: frame encoding `[kind][pts µs BE][payload]`,
  `StreamQueue` (≤ 3 video frames; overflow drops non-key frames until the next keyframe, asks for
  an IDR when it drains; audio ≤ 25 packets drop-oldest; cursor position coalesced), `Abr`
  (−20 % on phone drops / growing backlog / agent drops, +10 % after 5 s healthy, floor 500 kbit/s,
  ceiling = requested), `StreamClock` (video + audio + cursor on one clock, anchored at first
  delivery), `TokenStore` (single-use, device-bound, 30 s). `active.rs` runs a stream (encoder via
  `Platform::start_video`, audio via `start_audio`, cursor thread polling `GetCursorInfo` at ~60 Hz
  and sending each new cursor image once); `socket.rs` serves `/stream` (auth only, then
  `stream.open{token}`). `modules/stream.rs` has the methods; a watchdog ends streams that were not
  opened in time or whose capture stopped (`stream.ended{reason}` event). Closing either socket
  stops the stream. `record.start/stop` answer `unsupported` (MP4 recording not built yet).
- The process is made per-monitor-v2 DPI aware at start, so capture, cursor and `ma` share
  physical virtual-desktop pixels.
- Power uses `PowerScheduler` (`SystemPowerBackend`, or `DryRunBackend` when
  `Config::power_dry_run`); power, cancel, uploads and downloads are written to `audit.log`.

Never share or commit anything from this folder.

## Firewall

The first time it runs, Windows Defender Firewall asks whether to allow `helm-agent`.
Allow it on **Private networks** (your home Wi-Fi). Public networks are not needed.
mDNS uses UDP 5353; the control channel uses TCP 47820 (or the port shown at start).
