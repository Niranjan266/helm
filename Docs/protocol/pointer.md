# Pointer accuracy (hello feature `g8`)

The phone's relative `mm` frames now land on the exact pixel the phone meant, whatever the
PC's mouse settings are. Nothing about the `in` frame format changed; this page adds one
PC-side setting and the rule the agent uses to inject `mm`.

## Why

`SendInput` relative motion is passed through Windows pointer speed and "Enhance pointer
precision", so the same swipe moved the cursor a different distance on every PC. Absolute
motion is not affected by either.

## Agent rule for `in{k:"mm"}` (and `gy`, which is handled as `mm`)

The mode is the `pointer_mode` setting: `auto` (default), `absolute` or `relative`.

* **relative**: unchanged. `SendInput` with `MOUSEEVENTF_MOVE`, the fractional part carried
  by the injector.
* **absolute**: read the real cursor position (`GetCursorPos`, physical pixels of the
  per-monitor-v2 DPI aware agent), add `dx`/`dy` plus the fraction carried from the previous
  frame, truncate towards zero, clamp to the virtual desktop and move there with the same
  absolute injection as `ma` (`MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK`). A frame that
  adds up to less than one pixel moves nothing and keeps its fraction. The carry is cleared
  when the agent injects relative motion, an `ma` frame, or hits a desktop edge.
* **auto**: absolute, except relative while the foreground app needs raw input:
  * a full-screen exclusive Direct3D app owns the screen
    (`SHQueryUserNotificationState` = `QUNS_RUNNING_D3D_FULL_SCREEN`);
  * the cursor is confined (`GetClipCursor` is smaller than the virtual desktop);
  * the cursor is hidden (`GetCursorInfo` flags have no `CURSOR_SHOWING`).

  The probe runs per `mm` frame, so the switch happens within one frame of the app changing.

If the platform cannot be probed (`Platform::pointer_probe` unsupported), the agent behaves
as `relative`. Absolute moves are tracked as injected input like any other, so the existing
`input.error` codes (`blocked_by_elevation`, `input_failed`) apply.

## Methods

| Method | Tier | Params | Result |
|---|---|---|---|
| `pointer.settings` | viewer | none | `{mode}` |
| `pointer.set` | remote | `{mode}` | `{mode}` |

`mode` is `"auto"`, `"absolute"` or `"relative"`; anything else is `bad_params`. The value is
saved in `pointer.json` next to the other agent settings, applies to all phones and takes
effect for the next `mm` frame of every open session. `pointer.set` is written to the audit
log (`pointer_mode`). Both methods are gated on `g8` in `hello.features`.

## Phone side (not part of the wire contract)

For reference, the Android app now:

* converts touch deltas to dp and multiplies by 2.6 PC pixels per dp (the feel of a typical
  420 dpi phone), so every phone feels the same;
* consumes every historical pointer sample of a touch event;
* holds back tremor smaller than 0.3 dp that reverses direction (anything that adds up is
  released, so slow deliberate motion is delayed by less than a pixel, never lost);
* uses a gain curve that is exactly 1 up to 0.12 dp/ms, rises smoothly and caps at 2.2;
* keeps the sub-pixel remainder between frames (`mm` carries two decimals).
