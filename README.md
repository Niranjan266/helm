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
| `host/agent` | PC agent (Rust) — see its README |
| `android` | Android app (Kotlin + Jetpack Compose) — see its README |
| `Docs/PLAN.md` | Research, full feature list, architecture and roadmap |
| `Docs/PROTOCOL.md` | Phone ↔ PC protocol |

## Quick start (developers)

```bash
cd host/agent
cargo run --release
```

The agent prints a QR code. Build the Android app (`android/README.md`), install it, tap
**Pair with QR** and scan. Windows will ask to allow Helm on **Private** networks — allow it.

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md) first — especially the clean-room rule.

## License

[MIT](LICENSE)
