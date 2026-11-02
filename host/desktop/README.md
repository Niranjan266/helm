# Helm desktop (`Helm.exe`)

The Windows app users install. A Tauri 2 window (plain HTML/CSS/JS in `ui/`) plus a tray icon,
running the Helm agent **in-process** through the `helm-agent` library (`../agent`). Closing the
window only hides it, so phones stay connected; **Quit Helm** in the tray stops the agent cleanly.

- Pages: Home (status, details, phones, pairing QR with countdown, log), Phones (access level,
  revoke), Pair, Activity (`audit.log`), Settings (Start with Windows, port, pause, relay,
  update check — updates are only downloaded and verified, never run).
- Tray: Open Helm, Show pairing QR, Pause/Resume connections, Quit Helm.
- One copy per user (`tauri-plugin-single-instance`); starting it again focuses the window.
  It also refuses to run the agent next to a console `helm-agent.exe` on the same data folder.
- Start with Windows writes `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\Helm` =
  `"<path>\Helm.exe" --hidden` (same value name the console agent used, so it replaces it).
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
