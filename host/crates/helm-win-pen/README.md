# helm-win-pen

Pen (stylus) injection for the Helm PC agent through a synthetic pointer device
(`CreateSyntheticPointerDevice(PT_PEN, 1, POINTER_FEEDBACK_DEFAULT)` + `InjectSyntheticPointerInput`,
Windows 10 1809+). Pressure 0..=1024, tilt X/Y ±90°, rotation, eraser, barrel button; hover,
contact, up and leave. Clean-room, from the Microsoft Learn documentation only.

```rust
use helm_win_pen::{Pen, PenPhase, PenSample, map_normalized, ScreenRect};
let pen = Pen::new()?;                                  // Send + Sync
let (x, y) = map_normalized(0.5, 0.5, ScreenRect { x: 0, y: 0, width: 1920, height: 1080 });
let mut s = PenSample::new(x, y, PenPhase::Contact);
s.pressure = 0.6; s.tilt_x = 15.0; s.eraser = false;
pen.inject(&s)?;
pen.release()?;                                          // on disconnect (Drop does it too)
```

* `PenSample` (serde, snake_case `phase`: `hover | contact | up | leave`) is in **physical**
  virtual-desktop pixels: make the agent per-monitor-v2 DPI aware.
* `PenTracker` (pure) turns samples into the Win32 frame sequence: a hover frame before the first
  down, `DOWN`/`UPDATE`/`UP` with `FIRSTBUTTON` changes, an up before leave when still touching.
  Eraser sets `PEN_FLAG_INVERTED` (+ `PEN_FLAG_ERASER` in contact); barrel sets `PEN_FLAG_BARREL`.
* Injection cannot reach windows of higher integrity (elevated apps, UAC) from a normal process.
* Non-Windows builds: `Pen::new` returns `Error::Unsupported`.

## Tests

```text
cargo test                     # pure mapping + Win32 constant/struct checks (no input injected)
cargo test -- --ignored        # live stroke: injects a real pen stroke, run only on an idle machine
cargo clippy --all-targets
```

The live test has **not** been run yet (the dev PC was in use).

`windows` 0.62 features: `Win32_Foundation`, `Win32_UI_Controls`, `Win32_UI_Input_Pointer`,
`Win32_UI_WindowsAndMessaging`.
