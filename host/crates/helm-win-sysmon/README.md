# helm-win-sysmon

System monitor, task manager, foreground-app watcher, window/virtual-desktop
helpers and threshold alerts for the Helm PC agent (ROADMAP Phase 3: Monitor
F1/F4, Task manager F2, Profiles B5, Windows B6). MIT licensed, written from
the Microsoft Learn documentation and the `sysinfo` crate docs.

```toml
helm-win-sysmon = { path = "../crates/helm-win-sysmon" }
```

| Module       | API | Backed by |
|--------------|-----|-----------|
| `stats`      | `Sampler::new()`, `Sampler::sample() -> Stats { cpu_total, cpu_per_core, ram_used/total, swap_used/total, disks[{mount, used, total, read_bps, write_bps}], net[{name, rx_bps, tx_bps}], gpu[{name, load_pct, vram_used, vram_total}] }` | `sysinfo` (CPU, RAM, disk space + `IOCTL_DISK_PERFORMANCE`, `GetIfTable2`); PDH `\GPU Engine(*)\Utilization Percentage`, `\GPU Adapter Memory(*)\Dedicated Usage`; DXGI `IDXGIFactory1::EnumAdapters1` |
| `process`    | `processes() -> [ProcessInfo { pid, parent_pid, name, exe, cpu_pct, ram_bytes, is_elevated, priority }]`, `kill(pid)`, `set_priority(pid, Priority)`, `priority(pid)`, `check_target(pid, self_pid)` | `sysinfo`; `OpenProcessToken` + `TokenElevation`; `TerminateProcess`; `Get/SetPriorityClass` |
| `foreground` | `ForegroundWatcher::start(cb(&ForegroundInfo { hwnd, pid, exe, exe_name, title }))`, `current()` | `SetWinEventHook(EVENT_SYSTEM_FOREGROUND, WINEVENT_OUTOFCONTEXT)` on its own message-loop thread |
| `window`     | `monitors()`, `move_to_monitor(hwnd, index)`, `snap(hwnd, Left/Right/Maximize/Restore)`, `virtual_desktop(Next/Prev/New/Close)`, `window_rect(hwnd)` + pure geometry helpers | `EnumDisplayMonitors`, `SetWindowPos`, `ShowWindow`, `DWMWA_EXTENDED_FRAME_BOUNDS`; `SendInput` Win+Ctrl+Right/Left/D/F4 |
| `alerts`     | `Alerts::new(rules)`, `evaluate(&Stats, now_ms) -> [AlertEvent]`; `Condition::{CpuAbove{pct, for_s}, RamAbove{pct}, DiskFreeBelowPct{mount, pct}, DiskFreeBelowBytes{mount, bytes}}` | pure |
| `rate`       | `per_second(prev, cur, elapsed)`, `RateTracker` | pure |

All public types derive serde `Serialize`/`Deserialize` with snake_case names
(enums too: `"below_normal"`, `{"kind":"cpu_above",...}`). `hwnd` values are
`u64`, the same as `helm-win-apps`; monitor indices use the same order as
`helm-win-capture` (primary first, then x, then y).

## Semantics

- **CPU**: 0–100 of the whole machine. Process `cpu_pct` is divided by the
  logical core count, so it matches Task Manager rather than `top`.
- **Rates** (`read_bps`, `write_bps`, `rx_bps`, `tx_bps`) are measured between
  two `sample()` calls of the *same* `Sampler`; the first sample reports 0, and
  a counter that goes backwards (reset) reports 0 for that interval. Use one
  `Sampler` per subscription.
- **GPU load** is the busiest engine (per-engine utilisation summed over
  processes), like Task Manager's headline figure. `vram_*` is dedicated
  memory; integrated GPUs mostly use shared memory, so their figures are small.
  Software adapters (Microsoft Basic Render Driver) are skipped.
- **Temperatures** (`cpu_temp_c`, `gpu[].temp_c`) are always `None`: they need
  a privileged sidecar and are out of scope here.
- **Network** hides loopback, interfaces that never carried traffic, and NDIS
  filter / QoS pseudo-interfaces.
- **Alerts** are edge-triggered: one event when a rule starts firing (CPU: after
  `for_s` seconds continuously above), one with `firing: false` when it clears.
  Values equal to the threshold do not fire.
- **Foreground**: reports the current window at start, then each change
  (consecutive duplicates suppressed). UWP apps hosted by
  `ApplicationFrameHost.exe` are resolved to the hosted app's process where
  possible. Match profiles on `exe_name`, case-insensitively.

## Safety rules

- `kill` and `set_priority` refuse pid 0 (Idle), pid 4 (System) and the agent's
  own process before touching the OS (`Error::Refused`). `set_priority` also
  refuses `Realtime`. Protected processes, other users' processes and elevated
  processes are refused by Windows unless the agent is elevated
  (`Error::Refused` for access denied). Gate both behind the admin tier.
- `snap` / `move_to_monitor` check `IsWindow` and return `Error::NotFound`
  otherwise.

## Threading

Everything is `Send + Sync` and callable from any thread. Calls block:
`Sampler::sample` and `processes` sleep up to ~200 ms when called less than
200 ms after their previous call (CPU usage needs two readings), so from async
code use `tokio::task::spawn_blocking`. `ForegroundWatcher` runs its callback on
its own thread (keep it short: send on a channel) and stops when dropped.
Window functions run per-monitor-DPI-aware for the duration of the call and use
physical pixels.

On non-Windows targets `Sampler` and `processes` work via `sysinfo` (no GPU);
the rest return `Error::Unsupported`.

## Tests

```
cargo test                    # pure tests + read-only live tests
cargo test -- --ignored       # side effects, see below
cargo run --example demo      # read-only dump: stats, processes, monitors, 5 s of foreground changes
```

Ignored tests only act on things they create: `kill_own_child` and
`set_priority_own_child` spawn a hidden `ping -n 60 127.0.0.1`;
`move_and_snap_own_window` creates its own `STATIC` window.
`virtual_desktop_new_then_close` visibly creates a desktop and closes it, so run
it only when you're not using the PC.

## Limitations

- Snap places the window geometrically (left/right half of the work area); it
  does not trigger Windows Snap Assist or snap layouts.
- Virtual desktop shortcuts act on whatever session has input; `SendInput` is
  blocked when an elevated window has focus and the agent is not elevated, or
  on the secure desktop (UAC, lock screen).
- Disk I/O rates need the disk performance counters that Windows enables by
  default; if they are disabled the rates stay 0.
