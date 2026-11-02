# Presentation mode — hello feature `g2`

Extends [`PROTOCOL.md`](../PROTOCOL.md). An agent that supports this lists `"g2"` in
`hello.features`; phones must not call `present.*` or send `lp` frames otherwise.

## Methods

| method | tier | params → result |
|---|---|---|
| `present.state` | viewer | `{}` → `{app, title, exe, slide, slide_status, laser:{visible, monitor_id}}` |
| `present.go` | remote | `{action:"next"\|"prev"\|"first"\|"last"\|"black"\|"start"\|"end"}` → `{app, keys}` |
| `present.laser` | remote | `{monitor_id?: string\|null, trail?: bool (default true)}` → `{monitor: Monitor\|null, trail}` |

### `present.state`
- `app`: the foreground window classified as `"powerpoint"` (`POWERPNT.EXE`), `"slides"`
  (a browser window titled "… Google Slides …"), `"pdf"` (Acrobat/Reader, SumatraPDF, Foxit,
  PDF-XChange, Okular, Nitro, PDFgear, or a browser tab whose title is a `.pdf` file) or `null`.
- `title`, `exe`: foreground window title and exe name (`null` if none).
- `slide`: PowerPoint's slide via its automation interface (`PowerPoint.Application`; works
  even when another window is in front): `{index, count, notes, showing}`. `index` is 1-based (on a
  black/white/end screen: the show position, notes empty); `showing` = a slide show runs (else the editor's
  current slide); `notes` = the notes-page body text (`\r` → `\n`, at most 16 000 chars).
- `slide_status`: `"ok"`, `"no_presentation"` (PowerPoint has nothing open), `"unsupported"`
  (PowerPoint not running / not installed / cannot be automated) or `"busy"` (PowerPoint
  rejected the call, e.g. a dialog is open). Slides and PDF viewers never report a slide.

### `present.go`
Keys are pressed (down + up) in the foreground window; the agent never changes focus.

| action | PowerPoint | Google Slides | PDF app | PDF in browser | nothing known |
|---|---|---|---|---|---|
| next / prev | Right / Left | Right / Left | PageDown / PageUp | PageDown / PageUp | Right / Left |
| first / last | Home / End | Home / End | Home / End | Home / End | Home / End |
| black | B | B | — | — | — |
| start | F5 | Ctrl+F5 | Ctrl+L | — | — |
| end | Esc | Esc | Esc | Esc | Esc |

`—` → error `unsupported` (nothing is sent). Unknown `action` → `bad_params`. `keys` is the
combination sent, e.g. `"ctrl+f5"`.

### `present.laser`
Chooses the laser monitor (`id` from `screen.monitors`; unknown id → `bad_params`) and whether
a short fading trail follows the dot. `monitor_id: null` (default) = the monitor `ma` uses: the
streamed source while this session streams, else the primary monitor. Settings belong to the
session that set them and are forgotten when it disconnects.

## Laser frames — `in` kind `lp` (remote tier, no reply)

```json
{"type":"in","k":"lp","x":0.42,"y":0.31,"on":true}
{"type":"in","k":"lp","on":false}
```

- `x`, `y` in 0..1 of the laser monitor (clamped); required when `on` is true (default true).
- `on:false` hides the dot. The agent also hides it after 3 s without `lp` frames and when the
  session that drove it disconnects.
- Errors come once as `input.error{code}` (`forbidden`, `bad_params`, `input_failed`).
- The overlay is a topmost, click-through (`WS_EX_LAYERED | WS_EX_TRANSPARENT`), non-activating
  tool window covering that monitor; it draws a red glowing dot (radius ≈ 1 % of the monitor
  height) and, with `trail`, up to 24 fading points over 220 ms. It shows in screenshots and
  the screen stream. Phones should send at most ~60 frames/s and skip unchanged positions.

## Phone behaviour (Helm Android)
Presentation screen (drawer, only with `g2`): big Prev/Next, Start/First/Black/Last/End,
slide `x / y` + speaker notes from `present.state` (polled every 1 s while open), elapsed timer
with optional countdown (vibrates at 5 min, 1 min and 0 left), laser by tilt (game rotation
vector; the direction at press = screen centre, ±20° yaw / ±12.5° pitch spans the screen) or
by dragging on a 16:9 pad; volume up/down = previous/next while the screen is open.
