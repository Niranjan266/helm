# G1 — touchpad gestures & on-screen gamepad

Phone-side features. **No new wire messages**: everything below is sent with the existing
Phase 1 `in` frames (PROTOCOL.md §9.1: `mm`, `mb`, `ms`, `kd`, `ku`, `sc`). They therefore work
with any agent that advertises `p1`, and no PC driver is installed.

The hello feature string **`g1`** is reserved for this feature set. An agent MAY list it, but
the phone does not require it, because no agent-side behaviour changes. If a later revision adds
G1-only frames (for example a virtual-controller backend), they will be gated on `g1` and
specified here.

## 1. Touchpad gestures

| gesture | frames |
|---|---|
| three-finger swipe up | `sc{keys:[LWIN,TAB]}` (Task View) |
| three-finger swipe down | `sc{keys:[LWIN,0x44]}` (Win+D, show desktop) |
| three-finger swipe left / right, mode **apps** (default) | `sc{keys:[ALT,SHIFT,TAB]}` / `sc{keys:[ALT,TAB]}` |
| three-finger swipe left / right, mode **desktops** | `sc{keys:[LWIN,CONTROL,LEFT]}` / `sc{keys:[LWIN,CONTROL,RIGHT]}` |
| three-finger tap | `mb{middle}` down + up |

- A swipe fires once per gesture, after the three-finger centroid has travelled 48 dp with one
  axis at least 1.5× the other (`ThreeFingerSwipe.classify`).
- **Momentum scrolling**: after a two-finger scroll, the release velocity (notches/ms, averaged
  over the last 80 ms) seeds an exponential decay `v(t) = v0·e^(−t/325 ms)`, capped at
  0.12 notches/ms; flicks under 0.012 notches/ms do not coast. Each display frame sends the
  exact integral for that frame as `ms`. Any new touch cancels it.
- **Precision mode**: fixed gain 0.35 × speed, no acceleration.
- Unchanged: moves are flushed as each touch event arrives, tap-slop motion is held then
  released in one piece, and the pointer boost uses the smoothed finger speed.
- Settings are phone-local (`helm_gestures` preferences).

## 2. On-screen gamepad

Landscape controller: D-pad, A/B/X/Y, LB/RB/LT/RT, Start/Select, two thumbsticks. Each control
is bound to one of: nothing, a key (`kd`/`ku` while held), or a mouse button (`mb` down/up).
Each stick is bound to nothing, four keys (8-way, press at 0.5 / release at 0.35 tilt), or
mouse movement (`mm` each frame, `speed · |v|² · dt`, radial dead zone rescaled to 0..1).

- Keys and buttons are reference counted, so two controls on the same key do not release it
  early. Leaving the screen, stopping the app or switching mapping releases everything held.
  The agent also releases held input when the socket closes (§9.1).
- Presets: `keyboard` (WASD + arrows), `racing` (left stick steers with ←/→, RT = ↑, LT = ↓),
  `mouse_look` (left stick WASD, right stick = mouse, RT/LT = left/right click).
- The selected preset and a custom mapping are saved **per PC** on the phone
  (`helm_gamepad` preferences, key `pc:<host_id>`), as JSON:

```json
{"selected":"custom",
 "custom":{"name":"Custom","buttons":{"A":"key:32","RT":"mouse:left"},
           "left":"keys:87,83,65,68","right":"mouse:1400.0","deadzone":0.15}}
```

Binding strings: `none`, `key:<vk 1..254>`, `mouse:<left|right|middle|x1|x2>`; sticks `none`,
`keys:<up>,<down>,<left>,<right>` (0 = no key), `mouse:<px per second>`. Anything unknown
decodes to `none`; an unreadable profile falls back to the keyboard preset.
