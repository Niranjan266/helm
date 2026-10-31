# helm-win-system

Windows system integration for the Helm PC agent: media, volume, brightness,
power, network adapters and OS info. MIT licensed, written from the Microsoft
Learn documentation.

```toml
helm-win-system = { path = "../crates/helm-win-system" }
```

| Module    | API | Backed by |
|-----------|-----|-----------|
| `media`   | `media_state()`, `media_artwork()`, `media_artwork_png()`, `media_control(MediaAction)`, `MediaWatcher::start(cb)` | WinRT `GlobalSystemMediaTransportControlsSessionManager` |
| `audio`   | `volume()`, `set_volume(f32)`, `set_muted(bool)`, `VolumeWatcher::start(cb)`, `list_sessions()`, `set_session_volume(id, f32)`, `set_session_muted(id, bool)` | Core Audio `IAudioEndpointVolume`, `IAudioEndpointVolumeCallback`, `IMMNotificationClient`, `IAudioSessionManager2` |
| `display` | `brightness_list()`, `set_brightness(id, u8)` | WMI `WmiMonitorBrightness(Methods)` (internal), dxva2 DDC/CI (external) |
| `power`   | `power_action(PowerAction)`, `PowerScheduler` (`schedule`/`cancel`/`status`), `PowerBackend`, `DryRunBackend` | `LockWorkStation`, `SetSuspendState`, `InitiateShutdownW`, `ExitWindowsEx` |
| `net`     | `mac_addresses()`, `wol_magic_packet(&[u8; 6])`, `parse_mac`, `format_mac` | `GetAdaptersAddresses` |
| `info`    | `os_info()`, `uptime_s()`, `battery()` (`None` without a battery) | registry, `GetTickCount64`, `GetSystemPowerStatus` |

All public types derive serde `Serialize`/`Deserialize` with snake_case names.

## Threading

Every function can be called from any thread, including tokio workers. COM and
WinRT work runs on lazily started per-subsystem background threads (COM MTA),
so callers never initialise COM. Calls block: most take a few ms, DDC/CI
brightness can take hundreds of ms per monitor, so wrap `display` calls (or
all calls) in `tokio::task::spawn_blocking`. Watchers run their callback on
their own thread and stop when dropped.

On non-Windows targets the OS-backed functions return `Error::Unsupported`.

## Safety in tests

`cargo test` never pauses/plays media, never changes volume or brightness
(except writing back the value it just read), and never runs a real power
action: scheduler tests use `PowerScheduler::dry_run()`. Tests with visible
side effects are `#[ignore]`d.

```
cargo test
cargo run --example demo      # read-only dump of what the crate sees
```

## Limitations

- Media: only apps that publish to the Windows media flyout (SMTC). Some apps
  (often browsers) report a stale or missing timeline; `position_ms` /
  `duration_ms` are then `None` or approximate.
- Brightness: external monitors need DDC/CI enabled in the monitor's OSD;
  failures are cached for 5 minutes. External ids (`ddc:\\.\DISPLAYn#i`) can
  change when monitors are reconnected.
- Power: restart/shutdown are not forced, so apps with unsaved work can hold
  them up. `Lock` only works from a process in the interactive session.
  Hibernate fails when hibernation is disabled.
- Wake-on-LAN: prefer a wired adapter with `locally_administered == false`;
  randomised Wi-Fi MACs can't be woken.
