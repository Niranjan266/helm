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

## 10. Storage on the PC

`%APPDATA%\Helm\` (overridable for tests):
- `identity.json` — `host_id`, cert DER (base64), private key PKCS#8 **encrypted with Windows
  DPAPI** (current user) on Windows.
- `devices.json` — paired devices with public keys and tiers.
- `audit.log` — one line per pairing, auth success/failure, revoke.
