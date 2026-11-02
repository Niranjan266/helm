# Everyday features — feature `g6`

Additions to [`../PROTOCOL.md`](../PROTOCOL.md) (same transport, `req`/`res`/`event` shapes,
tiers and error codes). The agent's `hello.features` gains `"g6"`; phones must not call any
method below unless it is advertised (treat it as `unsupported` and send nothing).

Nothing here is on by default where it reads private data: clipboard history and PC
notification forwarding are opt-in.

## 1. Clipboard history

The agent keeps the **last 20** clipboard items (newest first) while the setting is on.
Text items up to 1 MiB and images (PNG) up to 8 MiB are kept; bigger ones are skipped.
Items copied by password managers are **never** recorded: the agent skips a change when the
clipboard carries the format `ExcludeClipboardContentFromMonitorProcessing`, the format
`Clipboard Viewer Ignore`, or `CanIncludeInClipboardHistory` with a DWORD value of `0`.
A copy identical to the newest item only refreshes its time.

Settings live in `everyday.json`. With `persist:true` the list is also written to
`clipboard-history.bin`, encrypted with Windows DPAPI (current user); turning `persist` or
`enabled` off deletes that file. Turning `enabled` off also clears the list in memory.

| method | tier | params → result |
|---|---|---|
| `clipboard.history` | remote | `{}` → `{enabled, persist, items:[Item]}` |
| `clipboard.history.set` | remote | `{enabled?, persist?}` → same as `clipboard.history` |
| `clipboard.history.clear` | remote | `{}` → `{}` |
| `clipboard.history.delete` | remote | `{id}` → `{}` |
| `clipboard.pick` | remote | `{id, to:"phone"\|"pc"}` → `"phone"`: `{text}` or `{png}` (base64, full item); `"pc"`: `{}` (PC clipboard set, item moves to the top) |

`Item` = `{id:int, kind:"text"|"image", time_ms, size, text?, truncated?, thumb_png?, width?, height?}`

- `id` is unique for the agent's lifetime (also across a persisted reload).
- `size` = bytes of the full item (UTF-8 text or PNG).
- Text items carry `text` = the first 2000 characters and `truncated:true` when cut; the full
  text comes from `clipboard.pick{to:"phone"}`.
- Image items carry `thumb_png` (base64 PNG, longest side ≤ 128 px) plus `width`/`height` of
  the full picture.

Event (to every session, only while enabled): `clipboard.history.changed{}` — re-fetch.
Unknown `id` → `bad_params`.

## 2. Quick file drop

Sharing a file to Helm on the phone uploads it with `file.upload.begin` / binary frames /
`file.upload.end` (PROTOCOL §9.3, saved to `Downloads\Helm`) and then asks the PC to do
something with it:

| method | tier | params → result |
|---|---|---|
| `file.drop` | remote | `{path, mode:"open"\|"save"\|"reveal"}` → `{path, mode}` |

- `path` must be the exact `path` returned by `file.upload.end`: a regular file directly in the
  agent's downloads folder (checked after resolving the path); anything else is `bad_params`.
- `open` opens the file with its default app, `reveal` opens Explorer with the file selected,
  `save` does nothing more.
- Programs and scripts (`.exe .com .scr .bat .cmd .ps1 .psm1 .vbs .vbe .js .jse .wsf .wsh .msi
  .msp .lnk .url .hta .cpl .reg .jar`) are never opened: `open` falls back to `reveal`, and the
  result's `mode` says what was done.

The phone setting "When a file arrives on the PC" = open / just save / reveal in Explorer
(default open). The phone shows a progress notification while uploading.

## 3. Find my phone

| method | tier | params → result |
|---|---|---|
| `phone.list` | viewer | `{}` → `{phones:[{device_id, device_name, online, this}]}` (all paired devices) |
| `phone.ring` | remote | `{device_id?, stop?:bool}` → `{rang:n}` (number of sessions told) |

- `phone.ring` sends event `phone.ring{from:{device_id?, name}, stop}` to every open session of
  `device_id`, or, without `device_id`, of every **other** paired device. `stop:true` asks the
  phone(s) to stop ringing. `rang:0` means the phone is not connected.
- The PC tray menu has **Ring my phone**, which rings every connected phone (`from.name` = the
  computer name, no `device_id`).
- On `phone.ring{stop:false}` the phone plays a loud sound on the **alarm** stream at maximum
  volume (also when the ringer is silent or on vibrate), vibrates and flashes the screen and
  torch until the user taps **Found it**; it then restores the alarm volume. Stopping is local
  only; nothing is sent back.

## 4. PC notifications on the phone

The agent reads Windows notifications with `UserNotificationListener` (WinRT). Windows asks
the user once for permission ("Let Helm access your notifications"); when access is denied or
the API is not available for this build, the result says so and nothing is forwarded.

| method | tier | params → result |
|---|---|---|
| `pc.notify.watch` | remote | `{on}` → `{access:"allowed"\|"denied"\|"unspecified"\|"unsupported"}` |

- While at least one session watches (and access is `allowed`), the agent checks the toast list
  every 2 s and sends event `pc.notification{id, app, title, body, time_ms}` to watching
  sessions for every notification that appeared since watching started (the ones already in
  Action Center are not sent). `id` is the Windows notification id (string). Helm's own toasts
  are skipped. `title` ≤ 500 and `body` ≤ 4000 characters.
- `access` is re-checked on every `watch{on:true}`; `"unspecified"` = the user has not answered
  the Windows prompt yet.

Phone: off by default (Settings → "Show PC notifications"); a per-app filter lists every app
seen so far, each off until enabled (or "all apps"). Allowed ones become Android notifications
in the channel "PC notifications".

## 5. Scheduled actions

The agent runs deck actions (PROTOCOL §9.3 / §11 / §13 action objects) and power actions at a
local time, once or on repeating weekdays. Saved in `schedules.json`.

| method | tier | params → result |
|---|---|---|
| `schedule.list` | viewer | `{}` → `{schedules:[Schedule]}` |
| `schedule.set` | remote (admin if any schedule has a `power` action) | `{schedules:[Schedule]}` → `{schedules:[Schedule]}` (normalised, replaces all) |

`Schedule` = `{id, name?, enabled, time:"HH:MM", days:[1..7], date?:"YYYY-MM-DD", action}`

- `days` uses ISO weekdays, Monday = 1 … Sunday = 7. **Empty `days` = one-off.** A one-off
  without `date` gets the date of the next time `HH:MM` comes (today or tomorrow) when saved.
- `action` = any deck action, or `{"power":"lock"|"sleep"|"hibernate"|"restart"|"shutdown"|"sign_out"}`.
- The agent adds read-only fields to results: `next_ms` (unix ms of the next run, `null` when
  disabled or past) and `last_run_ms` (`null` if never). They are ignored in `schedule.set`.
- At most 64 schedules; `id` 1-64 printable characters, unique; `name` ≤ 100 characters.
- Time is the PC's local time. A run that was missed (PC asleep or agent stopped) is still made
  if it is at most 10 minutes late; older ones are skipped. A one-off is disabled after its
  run (or skip).
- Events (to every session): `schedule.ran{id, ok, error?}` and `schedule.changed{schedules}`
  (after `schedule.set` and when a one-off disables itself). Every run is written to
  `audit.log`.

## 6. Health card

| method | tier | params → result |
|---|---|---|
| `system.health` | viewer | `{}` → `{battery, cpu_temp_c, disks, uptime_s, agent_uptime_s}` |

- `battery` = `{percent?, charging, ac_online?}` or `null` (desktop PC without a battery).
- `cpu_temp_c` = number (°C) from the ACPI thermal zone counters, or `null` when Windows does
  not expose one (common on desktops).
- `disks` = `[{mount, label, free_bytes, total_bytes}]` for fixed drives.
- `uptime_s` = time since Windows started; `agent_uptime_s` = since the agent started.

The phone's PC card shows battery, CPU temperature (if any), free space of the system drive and
uptime, refreshed when the card is shown and every 60 s while connected.
