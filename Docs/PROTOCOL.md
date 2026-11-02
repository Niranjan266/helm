# Helm Protocol — v1 (Phase 0)

This is the contract between the PC agent (`host/agent`) and the phone app (`android/`).
Both sides must follow it exactly. Change it here first, then in code.

## 1. Transport

- **Control channel:** WebSocket over TLS 1.3 (TLS 1.2 allowed as fallback).
- URL: `wss://<address>:<port>/ws`. Default port **47820**; if it is busy the agent picks a
  free port and advertises the real one (QR + mDNS).
- The agent uses a **self-signed certificate** (ECDSA P-256) created on first run and kept forever.
- The phone does **not** use normal CA validation. It **pins** the certificate: it accepts the
  connection only if `SHA-256(leaf certificate DER)` equals the fingerprint it got from the QR.
  Hostname verification is skipped (pinning replaces it).
- Each WebSocket message is one **UTF-8 JSON text frame**. Max frame size 1 MiB.
- Phone sends WebSocket pings every 15 s; agent answers with pongs (library default).

## 2. Identity

- **Host:** `host_id` = random UUID v4 made on first run, never changes. `host_name` = Windows
  computer name.
- **Cert fingerprint** `fp` = lowercase hex of SHA-256 over the certificate DER (64 chars).
- **Device (phone):** `device_id` = random UUID v4 made on first launch. Key pair = ECDSA P-256
  kept in Android Keystore (non-exportable).
- `public_key` = base64 (standard, with padding) of the **X.509 SubjectPublicKeyInfo DER**
  (this is what `java.security.PublicKey.getEncoded()` returns).
- Signatures = **SHA256withECDSA, ASN.1 DER encoded**, then base64 (standard).

## 3. Discovery (mDNS / DNS-SD)

- Service type: `_helm._tcp.local.`
- Instance name: `<host_name>` (agent may add a suffix if it clashes).
- Port: the real listening port.
- TXT records:
  - `v=1` (protocol version)
  - `id=<host_id>`
  - `name=<host_name>`
- Discovery only finds addresses. Trust always comes from the pinned fingerprint saved at pairing.

## 4. Pairing link (QR code)

The agent shows a QR code whose text is a URI:

```
helm://pair?v=1&id=<host_id>&n=<url-encoded host_name>&a=<ip1>,<ip2>&p=<port>&fp=<64 hex>&t=<token>
```

- `a` = comma-separated IPv4 addresses of the PC's non-loopback, up interfaces (LAN first).
- `t` = one-time pairing token: 32 random bytes, **base64url without padding** (43 chars).
- Token lifetime: **5 minutes**, single use. Only one token is active at a time.
- The same URI is printed as text so it can be copied, and the Android app registers the
  `helm://pair` deep link so `adb shell am start -d "<uri>"` or any QR scanner app can open it.
- Feature `g4`: 6-digit code pairing (no camera), approve on PC, USB tethering address first —
  see [protocol/pairing-security.md](protocol/pairing-security.md).

## 5. Handshake

After the WebSocket opens, the **agent speaks first**:

```json
{"type":"hello","protocol":1,"host_id":"…","host_name":"DESKTOP-1","nonce":"<base64 32 bytes>","agent_version":"0.1.0"}
```

The phone answers with **exactly one** of `pair` or `auth`.

### Signed message
Both use a signature over this UTF-8 string (fields joined with `|`, no spaces):

```
helm-v1|<purpose>|<host_id>|<nonce>
```

`purpose` is `pair` or `auth`; `nonce` is the exact base64 string from `hello`.

### 5.1 First time — `pair`

```json
{"type":"pair","token":"<t from QR>","device_id":"…","device_name":"Pixel 8","public_key":"<base64 SPKI>","signature":"<base64 DER sig, purpose=pair>"}
```

Agent checks: token matches (constant-time) and is not expired/used, public key parses as P-256,
signature verifies. Then it stores the device (tier `admin` in Phase 0), burns the token, and replies:

```json
{"type":"ready","device_id":"…","tier":"admin","host_id":"…","host_name":"DESKTOP-1"}
```

If the same `device_id` pairs again, its stored key and name are replaced.

### 5.2 Every later time — `auth`

```json
{"type":"auth","device_id":"…","signature":"<base64 DER sig, purpose=auth>"}
```

Agent looks up the device, verifies the signature with the stored key, updates `last_seen`,
replies with `ready` (same shape as above).

### 5.3 Failure

```json
{"type":"error","code":"<code>","message":"human readable"}
```

then the agent closes the socket. Codes:

| code | meaning |
|---|---|
| `bad_message` | not JSON, unknown type, missing field |
| `bad_token` | wrong, expired or already-used pairing token |
| `pairing_locked` | too many bad tokens (5 in a row) — pairing blocked for 60 s |
| `unknown_device` | `auth` from a device that is not paired (or was revoked) |
| `bad_signature` | signature or public key invalid |
| `timeout` | no `pair`/`auth` within 10 s of `hello` |

## 6. Requests, responses, events (after `ready`)

Phone → agent:

```json
{"type":"req","id":1,"method":"system.ping","params":{}}
```

- `id` = integer chosen by the phone, unique per connection.
- `params` is optional (treated as `{}`).

Agent → phone:

```json
{"type":"res","id":1,"ok":true,"result":{…}}
{"type":"res","id":1,"ok":false,"error":{"code":"unknown_method","message":"…"}}
```

Requests may be answered out of order. Response error codes: `unknown_method`, `bad_params`,
`forbidden` (device tier too low), `internal`.

Agent → phone events (none in Phase 0, shape reserved):

```json
{"type":"event","name":"media.changed","data":{…}}
```

## 7. Permission tiers

`viewer` < `remote` < `admin`. Every method declares a minimum tier; the router rejects
lower tiers with `forbidden`. Phase 0 pairs every device as `admin`.

## 8. Phase 0 methods

| method | tier | params | result |
|---|---|---|---|
| `system.ping` | viewer | `{}` | `{"pong":true,"time_ms":<unix ms on PC>}` |
| `system.info` | viewer | `{}` | `{"host_id","host_name","os":"Windows 11 …","agent_version","uptime_s":<int>}` |
| `devices.list` | admin | `{}` | `{"devices":[{"device_id","device_name","tier","paired_at_ms","last_seen_ms","this":bool}]}` |
| `devices.revoke` | admin | `{"device_id"}` | `{"revoked":true}` — a device may revoke itself (= "forget this PC") |

## 9. Phase 1 additions (protocol v1, feature `p1`)

`hello` gains `"features":["p1"]`. Older phones ignore it.

### 9.1 Input frames (no reply)
`{"type":"in","k":…}` — remote tier. Kinds (same as `helm-win-input` `InputEvent`):
`mm{dx,dy}` relative move (f32 px) · `mb{button:left|right|middle|x1|x2,down}` ·
`ms{dx,dy}` scroll in notches, `dy>0` = up, `dx>0` = right · `kd{vk}` / `ku{vk}` ·
`tx{text}` · `sc{keys:[vk…]}` shortcut. The agent releases all held keys/buttons when the
socket closes. Errors are reported once as event `input.error{code}` (`blocked_by_elevation`).

### 9.2 Binary frames (file data)
`[16 bytes transfer id][8 bytes offset, big-endian][data ≤ 256 KiB]`.

### 9.3 Methods

| method | tier | params → result |
|---|---|---|
| `apps.list` | viewer | → `{apps:[{app_id,name,kind:"packaged"|"desktop"}]}` |
| `apps.icon` | viewer | `{app_id,size}` → `{png:"<base64>"}` |
| `apps.launch` | remote | `{app_id}` → `{}` |
| `open` | remote | `{target}` (http/https/mailto URL or existing absolute path) → `{}` |
| `windows.list` | viewer | → `{windows:[{hwnd,title,pid,app_id,is_minimized,is_foreground}]}` |
| `windows.focus` | remote | `{hwnd}` → `{focused,method}` |
| `windows.minimize` / `maximize` / `restore` / `close` | remote | `{hwnd}` → `{}` |
| `dock.get` | viewer | → `{pages:[[tile…]…]}` tile = `{id,kind:"app"|"open",app_id?,target?,label?}` |
| `dock.set` | remote | `{pages}` → `{}` (broadcasts `dock.changed`) |
| `media.state` | viewer | → `{state: MediaState|null}` |
| `media.artwork` | viewer | → `{png:"<base64>"|null}` |
| `media.control` | remote | `{action:"play"|"pause"|"toggle_play_pause"|"next"|"previous"|"stop"|{"seek_ms":n}}` → `{}` |
| `audio.get` | viewer | → `{level,muted}` |
| `audio.set` | remote | `{level?,muted?}` → `{}` |
| `display.brightness.list` | viewer | → `{displays:[{id,name,kind,level,supported}]}` |
| `display.brightness.set` | remote | `{id,level}` → `{}` |
| `power.action` | admin | `{action:"lock"|"sleep"|"hibernate"|"restart"|"shutdown"|"sign_out",delay_s?}` → `{scheduled:{action,due_unix_ms}|null}` |
| `power.cancel` / `power.status` | admin / viewer | → `{scheduled:…|null}` |
| `deck.get` | viewer | → `{buttons:[{id,label,color,icon?,action}]}` |
| `deck.set` | remote | `{buttons}` → `{}` (broadcasts `deck.changed`) |
| `deck.run` | remote | `{id}` → `{}` |
| `clipboard.get` | remote | → `{text?,png?}` |
| `clipboard.set` | remote | `{text}` or `{png}` → `{}` |
| `clipboard.watch` | remote | `{on}` → `{}` (then `clipboard.changed{text?,has_image}` events) |
| `file.upload.begin` | remote | `{name,size,sha256}` → `{transfer_id}` (hex); saved to `Downloads\Helm` |
| `file.upload.end` | remote | `{transfer_id}` → `{path}` (fails `bad_params` if hash mismatch) |
| `file.download` | admin | `{path}` → `{transfer_id,size,sha256,name}` then binary frames, then event `file.done{transfer_id}` |
| `screen.monitors` | viewer | → `{monitors:[{id,name,x,y,width,height,is_primary}]}` |
| `screen.capture` | remote | `{monitor_id?}` → `{png:"<base64>"}` |
| `system.info` | viewer | adds `macs:[{mac,adapter_name,locally_administered}]` and `battery` |

Deck `action` = one of `{"hotkey":"ctrl+shift+esc"}`, `{"text":"…"}`, `{"launch":app_id}`,
`{"open":target}`, `{"media":action}`, `{"volume_step":0.05}`, `{"delay_ms":n}`.

### 9.4 Events (agent → phone)
`media.changed{state}`, `audio.changed{level,muted}`, `windows.changed{}` (re-list),
`input.textFocus{focused}`, `dock.changed`, `deck.changed`, `clipboard.changed`,
`power.scheduled{scheduled}`, `file.done`, `input.error`.

### 9.5 Clarifications (settled after Phase 1 build)
- `mm` deltas are **already accelerated by the phone**; the agent injects them 1:1.
- Pinch zoom = `kd` ctrl, `ms`, `ku` ctrl. `input.textFocus` data = `{focused}`.
- `hwnd` and monitor `id` are opaque: send back exactly what was received.
- `audio.set` uses `level` (0..1); brightness `level` is 0..100; deck `color` is `#RRGGBB`.
- `MediaState` = `helm-win-system` struct: `app_id,title,artist,album,status,position_ms?,duration_ms?,rate,has_artwork,can_*`.
- `dock.changed{pages}`, `deck.changed{buttons}`; `file.done{transfer_id,error?}`.
- `power.action` without `delay_s` replies `{scheduled:null}` and runs ~300 ms later; `power.cancel` returns the state after cancel.
- `screen.capture` with no `monitor_id` = primary monitor. `apps.icon` `size` defaults to 64.
- `input.error` codes: `blocked_by_elevation`, `forbidden`, `bad_params`, `input_failed`, `internal`.
- Upload flow control: sender keeps ≤ 2 MiB unacknowledged in its socket queue.
- **Open (Phase 2):** responses above 1 MiB (big screenshots/clipboard images) should move to
  binary transfers; until then receivers must accept text frames up to 16 MiB.

## 10. Phase 2 — screen & audio stream (feature `p2`)

- Stream socket: second WebSocket on the same TLS port, path `/stream`, same hello/auth
  handshake (pair not allowed). After `ready`, phone sends one text frame
  `{"type":"stream.open","token":"<from stream.start>"}`.
- Control socket methods (remote tier):
  - `stream.start{monitor_id?|hwnd?, max_width, max_height, fps, bitrate_kbps, audio:bool}` →
    `{token, width, height, fps, codec:"h264", encoder:"hardware"|"software"}`. Token is single-use, 30 s.
  - `stream.stop{}`, `stream.idr{}` (request keyframe), `stream.config{bitrate_kbps?, fps?}`.
  - `stream.stats{rx_kbps, decoded_fps, dropped, rtt_ms}` (phone → PC every 1 s; agent adapts
    bitrate: drop 20 % when dropped > 0 or backlog grows, raise 10 % every 5 s when healthy).
- Stream frames (binary, agent → phone), 1-byte kind + 8-byte pts µs (BE) + payload:
  - `0x01` H.264 access unit (Annex-B), `0x02` H.264 keyframe access unit (with SPS/PPS)
  - `0x03` Opus packet (48 kHz stereo, 20 ms)
  - `0x04` cursor: JSON `{x,y,visible,shape_id}` in stream pixel coordinates
  - `0x05` cursor shape: `[u32 shape_id][u16 w][u16 h][u16 hot_x][u16 hot_y]` + PNG
- Backpressure: agent keeps ≤ 3 video frames queued; beyond that it drops non-key frames and
  requests an IDR once the queue drains.
- Input while streaming still uses `in` frames on the control socket. Direct-touch mode sends
  absolute moves: `{"type":"in","k":"ma","x":0..1,"y":0..1}` normalised to the streamed monitor.
- `record.start{audio}` → `{path}`, `record.stop` → `{path,duration_ms}` (MP4 in Videos\Helm).

### 10.1 Clarifications (settled after Phase 2 build)
- `0x05` header fields are big-endian; cursor PNG is in native monitor pixels.
- `stream.start` params are optional (defaults 1920×1080, 60 fps, 8000 kbps; no source = primary
  monitor); result adds `audio:bool` and `opus_pre_skip` (samples, null without audio).
- `stream.stats` is a normal request → `{bitrate_kbps}` (or `{}`); `dropped` = since last report.
- `stream.config` → `{bitrate_kbps,fps,width,height}`; fps change restarts the encoder.
- Event `stream.ended{reason}`. One stream per control session; a new start replaces it.
- Video/audio/cursor pts share one clock starting at `stream.start`; off-monitor cursor → `visible:false`.
- New error code `unsupported` (e.g. `record.*` until MP4 recording lands).
- Stream keyframes may exceed 1 MiB: `/stream` binary frames are allowed up to 16 MiB.

## 11. Phase 3 — power user (feature `p3`)

| method | tier | params → result |
|---|---|---|
| `stats.subscribe` | viewer | `{interval_ms ≥ 500}` → `{}`; events `stats.tick{Stats}` (helm-win-sysmon struct) |
| `stats.unsubscribe` | viewer | → `{}` |
| `alerts.get` / `alerts.set` | viewer / admin | `{rules:[{id,kind:"cpu_above"|"ram_above"|"disk_free_below",threshold,for_s?,mount?}]}`; events `alert{id,firing,value}` |
| `process.list` | viewer | → `{processes:[{pid,name,exe,cpu_pct,ram_bytes}]}` |
| `process.kill` / `process.priority` | admin | `{pid}` / `{pid,level:"idle"|"below_normal"|"normal"|"above_normal"|"high"}` → `{}` |
| `audio.sessions` | viewer | → `{sessions:[{id,name,pid,level,muted}]}` |
| `audio.session.set` | remote | `{id,level?,muted?}` → `{}` |
| `windows.move` | remote | `{hwnd,monitor_index}` → `{}` |
| `windows.snap` | remote | `{hwnd,pos:"left"|"right"|"maximize"|"restore"}` → `{}` |
| `desktop.switch` | remote | `{dir:"next"|"prev"|"new"|"close"}` → `{}` |
| `profiles.get` / `profiles.set` | viewer / remote | `{profiles:[{id,name,match_exe?,deck_id?,dock_page?}]}`; event `profile.active{id\|null}` from the foreground watcher |
| `fs.list` | admin | `{path}` → `{entries:[{name,is_dir,size,modified_ms}]}` (roots: user profile + fixed drives; `path:""` lists roots) |
| `fs.delete` / `fs.mkdir` / `fs.open` | admin | `{path}` → `{}` (`fs.open` opens on PC) |
| `devices.set_tier` | admin | `{device_id,tier}` → `{}` |
| `audit.list` | admin | `{limit}` → `{lines:[…]}` |

Deck additions: action `{"macro":[action…]}` (sequence, `delay_ms` allowed inside, max 50 steps),
`{"repeat":{"times":n,"action":…}}`, `{"remote":"powerpoint.next"|"powerpoint.prev"|"powerpoint.start"|"powerpoint.end"|"browser.back"|"browser.forward"|"browser.tab_next"|"browser.tab_prev"|"browser.reload"|"obs.toggle_record"|"obs.toggle_stream"|"obs.scene:<name>"}`.
Button `state` binding: `{"bind":"mic_muted"|"audio_muted"|"media_playing"}` → events
`deck.state{id,on}`. Widgets: `{"widget":"cpu"|"ram"|"clock"|"now_playing"}` → `deck.widget{id,text}`.
OBS uses obs-websocket v5 at `ws://127.0.0.1:4455` with password from `obs.json` in data dir.

## 12. Phase 4 — anywhere access (feature `p4`)

- Relay tunnel: see `relay/README.md` "Tunnel protocol v1". The agent keeps one outgoing
  connection to the relay when `relay.enabled`; the phone, when LAN connect fails, opens
  `wss://<relay>/connect/<host_id>` and runs the normal pinned TLS + `/ws` (or `/stream`) inside it.
- `pair` is refused over relayed channels (pair on LAN only).
- `relay.get` / `relay.set` (admin): `{enabled, url}`. Saved PCs store `relay_url` from `system.info`
  (which adds `relay:{enabled,url}`).
- Remote wake: `wol.send{mac}` (remote tier) — an awake agent sends the magic packet on its LAN.
- USB: no protocol change (USB tethering is just another LAN address).
- `in` kind `gy{dx,dy}` = gyro air-mouse deltas (same handling as `mm`).

## 13. Phase 5 — extras (feature `p5`)

| method | tier | params → result |
|---|---|---|
| `record.start` / `record.stop` | remote | `{audio}` → `{path}` / → `{path,duration_ms}` (MP4, Videos\Helm; uses the active stream's encoder or starts one) |
| `term.open` | admin | `{shell:"powershell"|"cmd",cols,rows}` → `{term_id}`; output as events `term.output{term_id,data_b64}`, end `term.exit{term_id,code}` |
| `term.write` / `term.resize` / `term.close` | admin | `{term_id,data_b64}` / `{term_id,cols,rows}` / `{term_id}` → `{}` |
| `pen` input frames | remote | `{"type":"in","k":"pn","x":0..1,"y":0..1,"pressure":0..1,"tilt_x","tilt_y","state":"hover"|"down"|"up","eraser":bool}` |
| `notify.push` | remote | `{app,title,body,icon_png_b64?,actions:[{id,label}],reply:bool,key}` → `{}`; Windows toast; clicks → event `notify.action{key,action_id,reply_text?}` |
| `plugins.list` / `plugins.set` | viewer / admin | → `{plugins:[{id,name,version,capabilities,granted,enabled,error?}]}` / `{id,enabled,granted}` |
| `triggers.get` / `triggers.set` | viewer / remote | `{triggers:[{id,when:{time:"HH:MM days"}|{app_start:exe}|{phone_connected:device_id?}|{wifi:ssid},action}]}` |
| `ha.config.set` | admin | `{url,token}` (stored DPAPI-encrypted) → `{}`; `ha.states` (viewer) → `{states:[…]}`; deck action `{"ha":{"service":"light.toggle","entity_id":"…"}}` |

Clarifications: `pn` tilt is in degrees; the agent sends pen "leave" after 300 ms without `pn`
frames. Toast body click → `notify.action{action_id:null}`; reply button id is `"reply"`.
`plugins.set` returns the full list (entries add `running`, `error`). Plugins emit `plugin.event{id,name,data}`.
Every `term.*` call and `ha.config.set` is written to `audit.log` (term.write logs byte counts only). Android sends its own
notifications through `notify.push` only when the user enables "Mirror notifications" for an app.

Everyday features (feature `g6`: clipboard history, quick file drop, find my phone, PC
notifications, schedules, health card) are specified in [`protocol/everyday.md`](protocol/everyday.md).

## 14. Storage on the PC

`%APPDATA%\Helm\` (overridable for tests):
- `identity.json` — `host_id`, cert DER (base64), private key PKCS#8 **encrypted with Windows
  DPAPI** (current user) on Windows.
- `devices.json` — paired devices with public keys and tiers.
- `audit.log` — one line per pairing, auth success/failure, revoke.
