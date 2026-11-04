# View and control the phone from the PC (feature `g9`)

Everything else in Helm goes phone → PC. This goes the other way: the PC sees the phone's screen
and, if the phone allows it, taps and types on it. It exposes the whole phone, so the rules come
first. Gated on the hello feature **`g9`**; nothing here changes the meaning of any other message.

| Area | Where |
|---|---|
| Wire contract | this file |
| PC, agent | `host/agent/src/phone/` (`PhoneHub`), `src/modules/phone_control.rs`, `tests/phone.rs` |
| PC, window | `host/desktop/src/phone.rs`, `ui/phone-core.js` (pure, tested), `ui/phone.js` |
| Phone | `core/PhoneControl.kt`, `core/PhoneLink.kt` (pure, tested), `device/Phone*.kt`, `ui/Phone*.kt` |

## 1. Rules

* **Off by default.** The phone's "Let PC see this phone" switch starts off. While off the PC is
  told `disabled` and nothing reaches the person.
* **Asked every time.** A session starts only from a PC request that the person accepts on the
  phone's consent screen, followed by Android's own capture prompt. Neither answer is kept:
  `PhoneSessionLogic` returns to `Idle` after each session and nothing survives that. There is no
  auto-accept and no "remember".
* **The phone decides.** Any paired phone may be asked, whatever its tier on the PC (all `phone.*`
  methods are `viewer` tier): tiers limit what a phone may do to the PC, and this is the phone's own
  screen. Only an authenticated session can reach any of this.
* **Always visible.** During a session the phone shows an ongoing notification with **Stop**, and
  (with the accessibility service on) a red "PC is controlling" pill over everything.
* **It ends** on Stop (notification, pill, or app), screen off or lock, disconnect of either socket,
  the switch going off, Android ending the capture, and after **10 minutes without input from the
  PC**.
* **PC events are dropped unless a session is active and control was granted.** Checked twice: on
  the PC (`PhoneHub::input`, `not_allowed`) and again on the phone (`PhoneSessionLogic.mayInject`).
  Control is a separate choice on the consent screen and is off until the person turns it on.
* **Secure screens are not bypassed** (section 7).
* **Nothing is read.** The accessibility service's event handler is empty. On the focused node it
  looks only at flags and cursor offsets, never at the text. No event data or screen text is read,
  stored or sent.
* **Audit.** `audit.log` on the PC gets `phone_request`, `phone_session_start` (with `control=`,
  `relayed=`, `size=`), `phone_session_stop` (with `reason=`, `secs=`), and `phone_request_end` for
  requests that never went live. A phone can only give reasons from the list in section 6.

## 2. Flow

```
PC window        agent (PhoneHub)            phone
   | request ------> | phone.request ----------> | consent screen, then Android's prompt
   |                 | <---- phone.decline ----- | (no)
   |                 | <---- phone.accept ------ | (yes: capture running)  -> {token}
   |                 | <== /stream: auth, phone.open{token} ==
   | <- Active ----- | == binary frames =======> |
   | input --------> | == phone.in events =====> | performed if control granted
   | stop ---------> | phone.cancel -----------> | capture stops
   |                 | <---- phone.stop -------- | (ended on the phone)
```

One session at a time per agent (`busy`). A request not answered in 120 s ends with `timeout` and
the phone hears `phone.cancel`. A token not used in 30 s ends with `open_timeout`.

## 3. Control socket

Phone → agent requests (`viewer` tier). A `request_id` that is not this session's current one, or
that belongs to another phone's session, gets `bad_params`.

| method | params | result |
|---|---|---|
| `phone.caps` | `enabled`, `accessibility` (bools) | `{}`. Sent after every connect and on every change. No `phone.caps` = the phone is reported `unsupported`. |
| `phone.accept` | `request_id`, `width`, `height` (16..=8192), `rotation` (0/90/180/270, default 0), `control` (bool) | `{token}`: 32 random bytes, base64url, single use, 30 s, this device only |
| `phone.decline` | `request_id`, `reason` | `{}` |
| `phone.state` | `request_id` plus any of `width`, `height`, `rotation`, `control`, `accessibility`, `secure` (`"password"` or `null`) | `{}` |
| `phone.stop` | `request_id`, `reason?` | `{}` |

Agent → phone events:

| event | data |
|---|---|
| `phone.request` | `request_id`, `pc_name`, `relayed`, `max_long`, `max_short`, `fps`, `bitrate_kbps` |
| `phone.cancel` | `request_id`, `reason` (`timeout`, `open_timeout`, `stopped_by_pc`, `disconnected`, `agent_stopping`) |
| `phone.idr` | `{}`: send a keyframe soon |

`max_long` / `max_short` bound the picture in either orientation; the phone never upscales.

## 4. Stream socket

The same `/stream` path, handshake and limits as PROTOCOL §10 (`auth` only, pinned TLS, works inside
the relay tunnel), but the first text frame is `{"type":"phone.open","token":"…"}`. A bad token gets
`bad_token` and a close. This is a second socket of the same authenticated device rather than binary
on `/ws` so that a large keyframe never holds up the control socket, and so that uploads (binary on
`/ws` means file data) stay unambiguous.

**Phone → PC, binary**, the frame layout of §10, `[kind u8][pts_us u64 BE][payload]`:

* `0x01` H.264 access unit (Annex-B); `0x02` keyframe access unit, with SPS and PPS.
* `0x06` **part**: more of the access unit with this pts follows. Sockets accept at most 1 MiB per
  message from a phone, and an unusual keyframe can exceed that, so such a unit is sent as `0x06`
  parts (up to 900 KiB each) and a final `0x01` / `0x02` with the same pts. The agent joins them
  before anything sees them; a unit over 16 MiB is dropped and a keyframe requested.

H.264 Baseline, no B-frames, low latency; on-demand keyframes; pts from the encoder. There is no
audio.

**PC → phone, text**: `phone.in` events, `{"type":"event","name":"phone.in","data":{…}}`:

| `k` | fields | Android |
|---|---|---|
| `tap` | `x`, `y` | one 50 ms stroke |
| `long` | `x`, `y` | one 700 ms stroke |
| `swipe` | `x1`, `y1`, `x2`, `y2`, `ms` (1..=5000) | one line stroke |
| `down` / `move` / `up` | `x`, `y` | a live drag: chained `continueStroke`s; moves during a stroke are joined into the next |
| `scroll` | `x`, `y`, `dy`, `dx` (wheel notches, ±20; positive = what is below / to the right) | a swipe the other way, 12 % of the screen per notch, moved off the edges so that it fits |
| `global` | `action`: `back`, `home`, `recents`, `notifications` | `performGlobalAction` |
| `text` | `text` (1..=8192 bytes) | section 5 |
| `key` | `key`: `backspace`, `enter` | section 5 |

`x`, `y` are **normalised 0..1 to the picture the phone sends** and clamped, so they follow rotation
without any extra handling: the phone maps them onto its real display size at that moment. A finger
the PC has said nothing about for 30 s is lifted; so is any finger when the session ends.

## 5. Text without reading

`ACTION_SET_TEXT` replaces a field's whole content, and Helm does not read fields, so:

* The field is known to be empty (showing its hint), or holds exactly what the PC typed with the
  cursor at its end: `ACTION_SET_TEXT`. The only field text the phone keeps is what the PC typed, in
  memory, for the current session and field.
* Anything else: clipboard + `ACTION_PASTE`, which goes in at the cursor and cannot overwrite. This
  replaces the phone's clipboard, which is cleared a second later so that typed text does not stay
  there.
* `backspace`: edit our own text, or select one character before the cursor and `ACTION_CUT`.
* `enter`: `ACTION_IME_ENTER` where available (Android 11+), else paste a newline.
* A password field gets nothing (section 7).

## 6. Reasons

`phone.decline` / `phone.stop`: `declined`, `capture_denied`, `disabled`, `busy`,
`stopped_on_phone`, `screen_locked`, `idle`, `capture_stopped`, `encoder_failed`, `unsupported`.
Anything else is recorded as `stopped_on_phone`. From the agent itself: `timeout`, `open_timeout`,
`stopped_by_pc`, `disconnected`, `agent_stopping`.

## 7. Secure screens

| What | What happens | The PC shows |
|---|---|---|
| Apps with `FLAG_SECURE`, DRM | Android sends black; the phone cannot tell and does not try to get round it | The window notices a black picture for 2 s and explains (a dark screen looks the same, and the text says so) |
| Lock screen | The session ends with `screen_locked` | "The screen was locked" |
| Password field | Typing is refused and `phone.state{secure:"password"}` is sent; cleared by the next text that is not refused | "Password field: typing from the PC is blocked" |

## 8. Backpressure

Both ends use the rules of §10: at most 3 access units wait. A delta that does not fit is dropped,
and so is every delta after it until a keyframe; a keyframe always gets in, replacing what waits;
when the queue drains after a drop a keyframe is requested.

* Phone: `PhoneSendQueue` between encoder and socket. The sender stops writing while OkHttp holds more
  than 1 MiB unwritten, so a slow network fills this queue instead of the socket. Keyframe requests
  are limited to one per 500 ms.
* Agent: `StreamQueue` between socket and desktop app; drained after drops = `phone.idr`.
* Window: at most 4 frames unacknowledged between Rust and JS; JS drops deltas (and asks for a
  keyframe) when `decodeQueueSize` exceeds 3.

## 9. Relay

It works, and it warns: the window and the consent screen both say it will be slow. The agent lowers
the request to 1280×720 at most, 15 fps and 1500 kbit/s. Traffic stays end-to-end encrypted inside
the tunnel, and counts against the relay owner's quota.

## 10. Lib API (`helm_agent::phone`, reached as `AppState::phone`)

`request(state, device_id, RequestOptions) -> PhoneInfo`, `stop(state)`, `input(PhoneInput)`,
`request_keyframe()`, `next_frame().await` (one consumer; §10 layout), `subscribe()` (`PhoneEvent`s:
`Targets`, `Waiting`, `Accepted`, `Active`, `State`, `Ended`), `targets()`, `current()`.
`request` must run inside the Tokio runtime. Refusals carry a stable `code`: `not_connected`,
`unsupported`, `disabled`, `busy`, `not_allowed`, `bad_input`.

## 11. Android notes

* Foreground service type `mediaProjection`; the service starts before `getMediaProjection`, as
  Android 14 requires. The prompt asks for the whole display (`createConfigForDefaultDisplay` on
  Android 14+), since control would land in the wrong place on a single-app capture.
* One projection, one virtual display: rotation keeps the display, and swaps its size and surface
  to a new encoder. The first frame after that is a keyframe with a new SPS.
* The consent screen ignores touches through overlays (`filterTouchesWhenObscured`,
  `setHideOverlayWindows`), so another app cannot trick the person into accepting.
* Android 13+ blocks accessibility for apps installed outside a store until the person allows
  "restricted settings" in App info. The in-app steps say so.
* A long press followed by a drag (moving a home-screen icon) is not offered from the PC yet: a
  press that moves early is a drag, and a press held still is a long press.
