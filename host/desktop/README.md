# Helm desktop (`Helm.exe`)

The Windows app users install. A Tauri 2 window (plain HTML/CSS/JS in `ui/`) plus a tray icon,
running the Helm agent **in-process** through the `helm-agent` library (`../agent`). Closing the
window only hides it, so phones stay connected; **Quit Helm** in the tray stops the agent cleanly.

- Pages: Home (status, details, phones, pairing QR with countdown, log), Phones (access level,
  revoke), Phone (see and control a connected phone, with its owner's consent), Pair, Activity
  (`audit.log`), Settings (Start with Windows, port, pause, relay / Connect from anywhere, updates).
- Phone page (`src/phone.rs`, `ui/phone.js`, `ui/phone-core.js`;
  [Docs/protocol/phone-control.md](../../Docs/protocol/phone-control.md)): frames arrive over a Tauri
  binary channel (at most 4 unacknowledged) and are decoded with WebCodecs onto a canvas. `node --test
  tests/phone-core.test.mjs` tests the pure logic; `cargo test` runs that too when Node is installed.
- Updates (`src/updater.rs`): checks GitHub Releases at startup and daily; "Update available vX →
  Install" in Settings and the tray. Install downloads `Helm-windows-x64-setup.exe` through
  `helm-update` (HTTPS, size, SHA-256, Ed25519; a bad file is refused and deleted), runs it
  silently (`/S /R /ARGS --hidden`), and Helm comes back hidden with phones reconnecting by
  themselves. If the installer fails the old Helm is started again. A portable copy (no
  `uninstall.exe` next to it) runs the same installer, then the installed Helm copies itself over
  the old file and takes over Start with Windows. Refused when no release key is compiled in.
- Tray: Open Helm, Show pairing QR, Pause/Resume connections, Quit Helm, and the update item while
  one is on offer.
- One copy per user (`tauri-plugin-single-instance`); starting it again focuses the window.
  It also refuses to run the agent next to a console `helm-agent.exe` on the same data folder.
- Start with Windows writes `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\Helm` =
  `"<path>\Helm.exe" --hidden` (same value name the console agent used, so it replaces it).
- Stay reachable after restart (`src/reach.rs`, off by default): one UAC prompt installs the
  `HelmSignIn` service, a copy of Helm.exe in `%ProgramFiles%\Helm\SignIn` running as
  LocalSystem while you are **not** signed in (after a restart / Windows update, or signed
  out). Phones reach the PC on the LAN and through the relay with the same identity, but can
  only ping, read info and lock / sleep / restart / shut down; the screen, input, files, apps
  and settings are not offered. It stops as soon as you sign in and Helm.exe (Start with
  Windows) takes over port 47820. The key is sealed for this computer (DPAPI machine scope)
  in `service-identity.json` in your data folder; the service copies identity, devices and
  relay settings into SYSTEM's own profile and never writes into your folder. The NSIS hooks
  (`installer-hooks.nsh`) refresh the service's copy after an update and remove it on
  uninstall (each asks for UAC once, only when the service exists).
- Same data folder as the agent (`%APPDATA%\Helm`); logs go to `agent.log` there.
  `desktop.json` holds the chosen port.
- Arguments: `--hidden` (start in the tray), `--pair` (open on a fresh pairing code).

## Build

```powershell
cargo install tauri-cli --version "^2" --locked   # once
cargo tauri build      # target\release\Helm.exe and target\release\bundle\nsis\Helm_<ver>_x64-setup.exe
cargo test             # unit tests (status texts, Run-key command, args, settings, audit parsing)
```

Opening `ui/index.html` in a normal browser shows the layout with sample data.
Icons: `icons/helm-app.svg` is drawn by `icons/render-icon.ps1` to `app-icon.png`, then
`cargo tauri icon icons/app-icon.png -o icons`. Fonts: see [FONTS.md](FONTS.md).
