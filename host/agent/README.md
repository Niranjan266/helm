# helm-agent

The Windows PC side of Helm. Phase 0: secure discovery, QR pairing and trusted devices.
Phase 1 (`features:["p1"]`): apps, windows, input, media, volume, brightness, power, dock,
deck, clipboard, file transfer, screenshots and live events.
The wire contract is [`Docs/PROTOCOL.md`](../../Docs/PROTOCOL.md).

## Build

Needs Rust (stable, MSVC toolchain). No C toolchain beyond MSVC is needed (rustls uses `ring`).

```powershell
cargo build --release      # target\release\helm-agent.exe
cargo test                 # unit + end-to-end tests (TLS + WebSocket, in-process)
cargo clippy --all-targets
```

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
- `alerts.json`, `profiles.json` — Phase 3 alert rules and profiles; `obs.json` (optional,
  written by you) — OBS websocket password/port.

Files sent from the phone go to `<Downloads>\Helm` (name made safe, never overwritten:
`a (1).txt`). `file.download` (admin) only serves regular files under the user's profile
folder, never the Helm data folder, at most 4 GiB, SHA-256 announced first.

## Architecture (Phase 1)

- `platform/` — the `Platform` trait: every OS call (apps, windows, input injector, media,
  volume, brightness, clipboard, adapters/battery, monitors/screenshots, watchers).
  `real.rs` uses the `helm-win-apps`, `helm-win-input`, `helm-win-system` and
  `helm-win-capture` crates; `fake.rs` is the test double. Calls are blocking and run in
  `spawn_blocking`.
- `session.rs` — per connection: outgoing queue (text + binary frames with backpressure),
  one injector thread for `in` frames (`release_all` on disconnect), uploads, clipboard-watch
  flag, work to run after a response (download streaming, immediate power actions).
- `server.rs` — session registry and `broadcast`; watcher events → `media.changed`,
  `audio.changed`, `input.textFocus`, `clipboard.changed` (watching sessions only); window
  list polled every 2 s while phones are connected → `windows.changed`.
- Power uses `PowerScheduler` (`SystemPowerBackend`, or `DryRunBackend` when
  `Config::power_dry_run`); power, cancel, uploads and downloads are written to `audit.log`.

Never share or commit anything from this folder.

## Phase 3 — power user (`features:["p1","p3"]`)

PROTOCOL §11. OS calls go through new `Platform` methods (stats sampler, processes, audio
sessions, mic mute, window move/snap, virtual desktops, fixed drives, clock, foreground
watcher) backed by `helm-win-sysmon` and `helm-win-system`; tests use the fake only.

- `p3.rs` — alert rules (`alerts.json`, edge-triggered `alert{id,firing,value}`), profiles
  (`profiles.json`; the foreground watcher matches `match_exe` case-insensitively and
  broadcasts `profile.active{id|null}`), per-session `stats.tick` streams, and a 1 s ticker
  that evaluates alerts and sends `deck.state{id,on}` / `deck.widget{id,text}` on change
  (everything is re-sent when a phone connects).
- `modules/monitor.rs` (`stats.*`, `alerts.*`, `process.*`, `profiles.*`),
  `modules/desktop.rs` (`audio.sessions`, `audio.session.set`, `windows.move/snap`,
  `desktop.switch`), `modules/fs.rs` (`fs.*`), `modules/devices.rs` (+ `devices.set_tier`,
  `audit.list`).
- Deck: `macro` (≤ 50 expanded steps, nesting ≤ 4), `repeat{times,action}`, `remote`
  (PowerPoint/browser = hotkeys: Right/Left/F5/Esc, Alt+Left/Right, Ctrl(+Shift)+Tab, F5;
  OBS via `obs.rs`). Buttons may carry `state:{bind}` and `widget` (`action` stays required).
- `obs.rs` — obs-websocket v5 client (Hello → Identify with
  `base64(sha256(base64(sha256(password+salt))+challenge))` → Request). Needs `obs.json` in
  the data folder: `{"password":"…","port":4455}` (host is always 127.0.0.1); without it OBS
  actions fail with `bad_params` and nothing is contacted.

Safety: `process.kill/priority` are admin-only, refuse pid 0, 4 and the agent itself, and
refuse `realtime`. `fs.*` (admin) canonicalizes every path (no `..`, junctions resolved),
allows only the profile folder and fixed drives, hides and refuses the Helm data folder,
never deletes a root, and deletes only files and **empty** folders. `devices.set_tier`
refuses the calling device's own tier and disconnects the changed device so it re-auths with
the new tier. Kills, priorities, alert/tier changes and fs changes go to `audit.log`;
`audit.list{limit≤1000}` returns the last lines.

## Firewall

The first time it runs, Windows Defender Firewall asks whether to allow `helm-agent`.
Allow it on **Private networks** (your home Wi-Fi). Public networks are not needed.
mDNS uses UDP 5353; the control channel uses TCP 47820 (or the port shown at start).
