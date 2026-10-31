# helm-win-input

Windows input helpers for the Helm PC agent (MIT). It has three parts:

- **Input injection.** `InputEvent` is the phone's `in` frame (ROADMAP §1.1). `Injector` is
  thread-safe and built on `SendInput`. It keeps the fractional part of mouse moves and
  wheel notches, tracks held keys and buttons, and `release_all()` lets go of all of them
  (it also runs on drop).
- **Text-focus hint.** `TextFocusWatcher` uses UI Automation focus events to report whether
  the focused element accepts typing, with a 100 ms debounce.
- **Clipboard.** `clipboard::{get_text, set_text, get_image_png, set_image_png}` and
  `ClipboardWatcher`, which ignores changes made through this crate.

Everything is written from the Microsoft Learn documentation (clean room, see
`CONTRIBUTING.md`).

## Frames

```json
{"type":"in","k":"mm","dx":1.5,"dy":-2}          // relative move, pixels (fractional ok)
{"type":"in","k":"mb","button":"left","down":true} // left|right|middle|x1|x2
{"type":"in","k":"ms","dx":0,"dy":-0.25}          // wheel notches; dy>0 = up, dx>0 = right
{"type":"in","k":"kd","vk":17}                    // key down (virtual-key code)
{"type":"in","k":"ku","vk":17}                    // key up
{"type":"in","k":"tx","text":"hi 😀\n"}            // Unicode text, \n = Enter
{"type":"in","k":"sc","keys":[17,16,27]}          // shortcut: press in order, release reversed
```

`serde_json::from_str::<InputEvent>(frame)` accepts the whole frame (`type` is ignored).

## Use

```rust
use std::sync::Arc;
use helm_win_input::{Injector, InputEvent, TextFocusWatcher, ClipboardWatcher, clipboard};

let injector = Arc::new(Injector::new()?);      // Send + Sync
injector.inject(&serde_json::from_str::<InputEvent>(frame)?)?;
injector.release_all()?;                         // when the phone disconnects

let focus = TextFocusWatcher::start(|focused| { /* send input.textFocus{focused} */ })?;
let clip = ClipboardWatcher::start(|change| { /* send clipboard.changed */ })?;
let text = clipboard::get_text()?;               // Option<String>
```

Key names map to virtual-key codes with `name_to_vk("ctrl")`, `vk_to_name(0x11)` and
`parse_shortcut("ctrl+shift+esc")`. `accelerate(dx, dy, speed)` is an optional pointer
curve. You can turn it on with `Injector::set_pointer_speed(Some(speed))`.

## Errors

`Error::BlockedByElevation` means `SendInput` inserted nothing because of UIPI. That
happens when the foreground window belongs to an elevated program, or when the UAC prompt
or lock screen is showing. `Error::ClipboardBusy` means another program kept the clipboard
open for every retry (about 0.4 s). Off Windows, the platform functions return
`Error::Unsupported`. The pure modules (`keys`, `motion`, `dib`, `inject::Planner`) work on
every platform.

## Tests

```text
cargo test                                          # pure tests only; nothing touches the desktop
cargo test --lib clipboard:: -- --ignored --test-threads=1   # live clipboard (restores it after)
cargo test --test live_input -- --ignored --test-threads=1   # live mouse/keyboard: focus a scratch Notepad first!
cargo run --example focus_watch                     # prints focus changes for 10 s
cargo run --example clipboard_watch                 # prints clipboard changes until Enter
```
