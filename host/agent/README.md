# helm-agent

The Windows PC side of Helm. Phase 0: secure discovery, QR pairing and trusted devices.
The wire contract is [`Docs/PROTOCOL.md`](../../Docs/PROTOCOL.md).

## Build

Needs Rust (stable, MSVC toolchain). No C toolchain beyond MSVC is needed (rustls uses `ring`).

```powershell
cargo build --release      # target\release\helm-agent.exe
cargo test                 # unit + end-to-end tests (TLS + WebSocket, in-process)
```

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

Log level: set `RUST_LOG` (e.g. `RUST_LOG=debug`).

The QR is printed in the console (drawn for a dark background), the link is printed as text,
and `pair-qr.svg` is written into the data folder (deleted again when the agent stops).

## Where data lives (`%APPDATA%\Helm`)

- `identity.json` — host id, self-signed certificate, private key encrypted with Windows DPAPI
  (current user). Copying it to another user/PC will not work, by design.
- `devices.json` — paired phones (public keys, tiers, timestamps).
- `audit.log` — one tab-separated line per pairing, auth success/failure and revoke.
- `pair-qr.svg` — current pairing QR while the agent runs.

Never share or commit anything from this folder.

## Firewall

The first time it runs, Windows Defender Firewall asks whether to allow `helm-agent`.
Allow it on **Private networks** (your home Wi-Fi). Public networks are not needed.
mDNS uses UDP 5353; the control channel uses TCP 47820 (or the port shown at start).
