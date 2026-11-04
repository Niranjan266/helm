# Helm

**Control your whole Windows PC from your Android phone** — apps, input, media, screen,
files and automation. Secure, fast and fully open source (MIT).

> Status: **Phase 0 — foundations.** The phone can find the PC on your network, pair with a
> QR code over an encrypted connection, and stay trusted after reboots. Real controls start in
> Phase 1. See the [plan](Docs/PLAN.md).

## Why another one?

Helm is inspired by projects like WinDock, KDE Connect, Unified Remote and Macro Deck, and
aims to combine what they do best in one native, encrypted app: a launcher dock, a
Stream-Deck-style button grid, touchpad and keyboard, live low-latency screen view, file and
clipboard sharing, a system monitor, and safe access from anywhere.

Helm is **written from scratch** — other projects are references for ideas only, never sources
of code. It has **no AI features**.

## How it works (Phase 0)

```
Android app ──(mDNS: _helm._tcp)──▶ finds PC
            ──(scan QR: address + certificate fingerprint + one-time token)──▶ pairs
            ──(WebSocket over TLS, certificate pinned, device key signs a challenge)──▶ PC agent
```

- Every connection is encrypted; the phone only trusts the exact certificate shown in the QR.
- Each phone has its own key in Android Keystore — no PIN to type again after the first pairing.
- Full message contract: [Docs/PROTOCOL.md](Docs/PROTOCOL.md).

## Repository

| Path | What |
|---|---|
| `host/agent` | PC agent (Rust library + `helm-agent` console program) — see its README |
| `host/desktop` | `Helm.exe` desktop app (Tauri 2) that runs the agent in-process — see its README |
| `android` | Android app (Kotlin + Jetpack Compose) — see its README |
| `Docs/PLAN.md` | Research, full feature list, architecture and roadmap |
| `Docs/PROTOCOL.md` | Phone ↔ PC protocol |
| `Docs/protocol/phone-control.md` | The other way round: see and control the phone from the PC, with its owner's consent |

## Install

**Windows** — paste into PowerShell:

```powershell
irm https://raw.githubusercontent.com/Niranjan266/helm/main/scripts/install.ps1 | iex
```

It downloads the newest release (`Helm-windows-x64.exe`), checks its SHA-256, installs it as
`%LOCALAPPDATA%\Programs\Helm\Helm.exe`, adds a Start menu entry and opens Helm. Run it again
to update. Releases also include `Helm-windows-x64-setup.exe`, a regular installer.

Helm is a desktop app with a window (status, phones, pairing QR, activity, settings) and a tray
icon. **Closing the window keeps Helm running in the tray**, so phones stay connected; use the
tray's **Quit Helm** to stop it. *Start with Windows* (Settings) starts it hidden in the tray.
Windows 11 already has the WebView2 runtime it needs.

**Android** — download `Helm-android.apk` from
[Releases](https://github.com/Niranjan266/helm/releases) and install it.

## Quick start (developers)

```bash
cargo install tauri-cli --version "^2" --locked   # once
cd host/desktop
cargo tauri build      # target/release/Helm.exe (+ NSIS installer); or `cargo tauri dev`
```

The Helm window shows a pairing QR. The console agent still works for development:
`cd host/agent && cargo run --release` prints the QR in the terminal. Build the Android app (`android/README.md`), install it, tap
**Pair with QR** and scan. Windows will ask to allow Helm on **Private** networks — allow it.

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md) first — especially the clean-room rule.

## License

[MIT](LICENSE)
