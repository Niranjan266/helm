//! Turning [`InputEvent`]s into low-level operations, and the thread-safe [`Injector`].
//!
//! The translation ([`Planner`]) is pure and platform independent; a [`Sink`] performs the
//! operations. On Windows the default sink calls `SendInput`.

use std::sync::{Mutex, MutexGuard};

use crate::error::Result;
use crate::event::{InputEvent, MouseButton};
use crate::motion::{Accumulator2, WHEEL_DELTA, accelerate};

/// `dwExtraInfo` value attached to every event Helm injects, so other code (for example a
/// low-level hook in the agent) can recognise Helm's own input.
pub const INJECTED_MARKER: usize = 0x4845_4C4D; // "HELM"

const VK_TAB: u16 = 0x09;
const VK_RETURN: u16 = 0x0D;

/// One low-level input operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// Relative pointer motion in whole pixels.
    Move {
        dx: i32,
        dy: i32,
    },
    Button {
        button: MouseButton,
        down: bool,
    },
    /// Vertical wheel in `WHEEL_DELTA` units (positive = forward / up).
    Wheel {
        delta: i32,
    },
    /// Horizontal wheel in `WHEEL_DELTA` units (positive = right).
    HWheel {
        delta: i32,
    },
    /// Virtual-key press or release.
    Key {
        vk: u16,
        down: bool,
    },
    /// One UTF-16 code unit typed with `KEYEVENTF_UNICODE`.
    Unicode {
        unit: u16,
        down: bool,
    },
}

/// Something that can perform [`Op`]s. Implemented by the Windows `SendInput` sink; tests
/// and the agent can provide their own (for example a recorder).
pub trait Sink: Send {
    /// Performs the operations in order, as one batch where possible.
    fn send(&mut self, ops: &[Op]) -> Result<()>;
}

/// Splits text into key operations: `\r\n`, `\r` and `\n` become Enter, `\t` becomes Tab,
/// other control characters are dropped, everything else is sent as UTF-16 code units
/// (characters outside the BMP become a surrogate pair, high unit first).
pub fn text_ops(text: &str) -> Vec<Op> {
    let mut ops = Vec::with_capacity(text.len() * 2);
    let mut chars = text.chars().peekable();
    let key = |ops: &mut Vec<Op>, vk| {
        ops.push(Op::Key { vk, down: true });
        ops.push(Op::Key { vk, down: false });
    };
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                key(&mut ops, VK_RETURN);
            }
            '\n' => key(&mut ops, VK_RETURN),
            '\t' => key(&mut ops, VK_TAB),
            c if c.is_control() => {}
            c => {
                let mut buf = [0u16; 2];
                for &unit in c.encode_utf16(&mut buf).iter() {
                    ops.push(Op::Unicode { unit, down: true });
                    ops.push(Op::Unicode { unit, down: false });
                }
            }
        }
    }
    ops
}

/// The UTF-16 code units for one character (1 unit, or a surrogate pair).
pub fn utf16_units(c: char) -> Vec<u16> {
    let mut buf = [0u16; 2];
    c.encode_utf16(&mut buf).to_vec()
}

/// Pure state machine: tracks held keys and buttons and the sub-pixel / sub-notch
/// remainders, and translates events into [`Op`]s.
#[derive(Debug, Default, Clone)]
pub struct Planner {
    held_keys: Vec<u16>,
    buttons: u8,
    motion: Accumulator2,
    wheel: Accumulator2,
    pointer_speed: Option<f32>,
}

impl Planner {
    pub fn new() -> Self {
        Self::default()
    }

    /// `Some(speed)` runs mouse deltas through [`accelerate`]; `None` (default) uses them as-is.
    pub fn set_pointer_speed(&mut self, speed: Option<f32>) {
        self.pointer_speed = speed;
    }

    pub fn held_keys(&self) -> &[u16] {
        &self.held_keys
    }

    pub fn held_buttons(&self) -> Vec<MouseButton> {
        MouseButton::ALL
            .into_iter()
            .filter(|b| self.buttons & b.bit() != 0)
            .collect()
    }

    /// Translates one event and updates the tracked state.
    pub fn plan(&mut self, event: &InputEvent) -> Vec<Op> {
        match *event {
            InputEvent::MouseMove { dx, dy } => {
                let (dx, dy) = match self.pointer_speed {
                    Some(speed) => accelerate(dx, dy, speed),
                    None => (dx, dy),
                };
                match self.motion.push(dx, dy, 1.0) {
                    (0, 0) => vec![],
                    (dx, dy) => vec![Op::Move { dx, dy }],
                }
            }
            InputEvent::MouseButton { button, down } => {
                if down {
                    self.buttons |= button.bit();
                } else {
                    self.buttons &= !button.bit();
                }
                vec![Op::Button { button, down }]
            }
            InputEvent::Scroll { dx, dy } => {
                let (h, v) = self.wheel.push(dx, dy, WHEEL_DELTA);
                let mut ops = Vec::new();
                if v != 0 {
                    ops.push(Op::Wheel { delta: v });
                }
                if h != 0 {
                    ops.push(Op::HWheel { delta: h });
                }
                ops
            }
            InputEvent::KeyDown { vk } => {
                if !valid_vk(vk) {
                    return vec![];
                }
                if !self.held_keys.contains(&vk) {
                    self.held_keys.push(vk);
                }
                vec![Op::Key { vk, down: true }]
            }
            InputEvent::KeyUp { vk } => {
                if !valid_vk(vk) {
                    return vec![];
                }
                self.held_keys.retain(|&k| k != vk);
                vec![Op::Key { vk, down: false }]
            }
            InputEvent::Text { ref text } => text_ops(text),
            InputEvent::Shortcut { ref keys } => {
                // Keys the user is already holding stay held; everything else is pressed in
                // order and released in reverse order.
                let mut pressed: Vec<u16> = Vec::new();
                for &vk in keys {
                    if valid_vk(vk) && !self.held_keys.contains(&vk) && !pressed.contains(&vk) {
                        pressed.push(vk);
                    }
                }
                let downs = pressed.iter().map(|&vk| Op::Key { vk, down: true });
                let ups = pressed.iter().rev().map(|&vk| Op::Key { vk, down: false });
                downs.chain(ups).collect()
            }
        }
    }

    /// Releases every held key (most recent first) and button, and clears remainders.
    pub fn release_all(&mut self) -> Vec<Op> {
        let mut ops: Vec<Op> = self
            .held_keys
            .drain(..)
            .rev()
            .map(|vk| Op::Key { vk, down: false })
            .collect();
        for b in MouseButton::ALL {
            if self.buttons & b.bit() != 0 {
                ops.push(Op::Button {
                    button: b,
                    down: false,
                });
            }
        }
        self.buttons = 0;
        self.motion.reset();
        self.wheel.reset();
        ops
    }
}

fn valid_vk(vk: u16) -> bool {
    (1..=0xFE).contains(&vk)
}

struct Inner {
    planner: Planner,
    sink: Box<dyn Sink>,
}

/// Thread-safe input injector (`Send + Sync`; share it with `Arc`).
///
/// Keeps track of what is held so [`Injector::release_all`] can let go of everything when a
/// phone disconnects; dropping the injector does the same.
pub struct Injector {
    inner: Mutex<Inner>,
}

impl Injector {
    /// An injector that uses `SendInput`. Returns [`crate::Error::Unsupported`] off Windows.
    pub fn new() -> Result<Self> {
        #[cfg(windows)]
        {
            Ok(Self::with_sink(crate::win::sendinput::SendInputSink))
        }
        #[cfg(not(windows))]
        {
            Err(crate::Error::Unsupported)
        }
    }

    /// An injector that performs operations through a custom sink.
    pub fn with_sink(sink: impl Sink + 'static) -> Self {
        Self {
            inner: Mutex::new(Inner {
                planner: Planner::new(),
                sink: Box::new(sink),
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Injects one event. State (held keys/buttons) is tracked even if the sink fails, so a
    /// later `release_all` still sends the matching releases.
    pub fn inject(&self, event: &InputEvent) -> Result<()> {
        let mut inner = self.lock();
        let ops = inner.planner.plan(event);
        if ops.is_empty() {
            return Ok(());
        }
        inner.sink.send(&ops)
    }

    /// Releases all held keys and mouse buttons. The tracked state is cleared even when the
    /// sink reports an error.
    pub fn release_all(&self) -> Result<()> {
        let mut inner = self.lock();
        let ops = inner.planner.release_all();
        if ops.is_empty() {
            return Ok(());
        }
        inner.sink.send(&ops)
    }

    /// See [`Planner::set_pointer_speed`].
    pub fn set_pointer_speed(&self, speed: Option<f32>) {
        self.lock().planner.set_pointer_speed(speed);
    }

    pub fn held_keys(&self) -> Vec<u16> {
        self.lock().planner.held_keys().to_vec()
    }

    pub fn held_buttons(&self) -> Vec<MouseButton> {
        self.lock().planner.held_buttons()
    }
}

impl Drop for Injector {
    fn drop(&mut self) {
        let _ = self.release_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;
    use std::sync::Arc;

    #[derive(Clone, Default)]
    struct Recorder(Arc<Mutex<Vec<Op>>>, Arc<Mutex<bool>>);

    impl Recorder {
        fn take(&self) -> Vec<Op> {
            std::mem::take(&mut *self.0.lock().unwrap())
        }
    }

    impl Sink for Recorder {
        fn send(&mut self, ops: &[Op]) -> Result<()> {
            if *self.1.lock().unwrap() {
                return Err(Error::BlockedByElevation);
            }
            self.0.lock().unwrap().extend_from_slice(ops);
            Ok(())
        }
    }

    fn key(vk: u16, down: bool) -> Op {
        Op::Key { vk, down }
    }

    #[test]
    fn surrogate_pairs_are_split() {
        assert_eq!(utf16_units('a'), vec![0x61]);
        assert_eq!(utf16_units('é'), vec![0xE9]);
        assert_eq!(utf16_units('😀'), vec![0xD83D, 0xDE00]);
        assert_eq!(utf16_units('\u{10FFFF}'), vec![0xDBFF, 0xDFFF]);
        let ops = text_ops("😀");
        assert_eq!(
            ops,
            vec![
                Op::Unicode {
                    unit: 0xD83D,
                    down: true
                },
                Op::Unicode {
                    unit: 0xD83D,
                    down: false
                },
                Op::Unicode {
                    unit: 0xDE00,
                    down: true
                },
                Op::Unicode {
                    unit: 0xDE00,
                    down: false
                },
            ]
        );
    }

    #[test]
    fn newlines_become_enter() {
        let ops = text_ops("a\nb\r\nc\rd\te\u{7}");
        let enter = [key(0x0D, true), key(0x0D, false)];
        let u = |c: u16| {
            [
                Op::Unicode {
                    unit: c,
                    down: true,
                },
                Op::Unicode {
                    unit: c,
                    down: false,
                },
            ]
        };
        let mut expected = Vec::new();
        expected.extend(u(b'a' as u16));
        expected.extend(enter);
        expected.extend(u(b'b' as u16));
        expected.extend(enter);
        expected.extend(u(b'c' as u16));
        expected.extend(enter);
        expected.extend(u(b'd' as u16));
        expected.extend([key(0x09, true), key(0x09, false)]);
        expected.extend(u(b'e' as u16));
        assert_eq!(ops, expected);
        assert!(text_ops("").is_empty());
    }

    #[test]
    fn mouse_move_accumulates_sub_pixels() {
        let mut p = Planner::new();
        assert!(
            p.plan(&InputEvent::MouseMove { dx: 0.4, dy: -0.4 })
                .is_empty()
        );
        assert_eq!(p.plan(&InputEvent::MouseMove { dx: 0.4, dy: -0.4 }), vec![]);
        assert_eq!(
            p.plan(&InputEvent::MouseMove { dx: 0.4, dy: -0.4 }),
            vec![Op::Move { dx: 1, dy: -1 }]
        );
        assert_eq!(
            p.plan(&InputEvent::MouseMove { dx: 5.0, dy: 0.0 }),
            vec![Op::Move { dx: 5, dy: 0 }]
        );
    }

    #[test]
    fn pointer_speed_applies_curve() {
        let mut p = Planner::new();
        p.set_pointer_speed(Some(2.0));
        assert_eq!(
            p.plan(&InputEvent::MouseMove { dx: 1.0, dy: 0.0 }),
            vec![Op::Move { dx: 2, dy: 0 }]
        );
    }

    #[test]
    fn scroll_accumulates_both_axes() {
        let mut p = Planner::new();
        assert_eq!(
            p.plan(&InputEvent::Scroll { dx: 0.5, dy: -1.0 }),
            vec![Op::Wheel { delta: -120 }, Op::HWheel { delta: 60 }]
        );
        assert!(
            p.plan(&InputEvent::Scroll { dx: 0.0, dy: 0.004 })
                .is_empty()
        );
        assert_eq!(
            p.plan(&InputEvent::Scroll { dx: 0.0, dy: 0.005 }),
            vec![Op::Wheel { delta: 1 }]
        );
    }

    #[test]
    fn shortcut_presses_in_order_releases_in_reverse() {
        let mut p = Planner::new();
        assert_eq!(
            p.plan(&InputEvent::Shortcut {
                keys: vec![0x11, 0x10, 0x1B]
            }),
            vec![
                key(0x11, true),
                key(0x10, true),
                key(0x1B, true),
                key(0x1B, false),
                key(0x10, false),
                key(0x11, false),
            ]
        );
        assert!(p.held_keys().is_empty());
    }

    #[test]
    fn shortcut_leaves_already_held_keys_alone() {
        let mut p = Planner::new();
        p.plan(&InputEvent::KeyDown { vk: 0x11 });
        assert_eq!(
            p.plan(&InputEvent::Shortcut {
                keys: vec![0x11, 0x43, 0x43, 0]
            }),
            vec![key(0x43, true), key(0x43, false)]
        );
        assert_eq!(p.held_keys(), &[0x11]);
    }

    #[test]
    fn invalid_vks_are_ignored() {
        let mut p = Planner::new();
        assert!(p.plan(&InputEvent::KeyDown { vk: 0 }).is_empty());
        assert!(p.plan(&InputEvent::KeyUp { vk: 0xFFFF }).is_empty());
        assert!(p.held_keys().is_empty());
    }

    #[test]
    fn release_all_lets_go_of_everything() {
        let rec = Recorder::default();
        let inj = Injector::with_sink(rec.clone());
        inj.inject(&InputEvent::KeyDown { vk: 0x10 }).unwrap();
        inj.inject(&InputEvent::KeyDown { vk: 0x41 }).unwrap();
        inj.inject(&InputEvent::KeyDown { vk: 0x41 }).unwrap(); // auto-repeat
        inj.inject(&InputEvent::MouseButton {
            button: MouseButton::Left,
            down: true,
        })
        .unwrap();
        inj.inject(&InputEvent::MouseButton {
            button: MouseButton::X1,
            down: true,
        })
        .unwrap();
        inj.inject(&InputEvent::MouseButton {
            button: MouseButton::X1,
            down: false,
        })
        .unwrap();
        assert_eq!(inj.held_keys(), vec![0x10, 0x41]);
        assert_eq!(inj.held_buttons(), vec![MouseButton::Left]);
        rec.take();
        inj.release_all().unwrap();
        assert_eq!(
            rec.take(),
            vec![
                key(0x41, false),
                key(0x10, false),
                Op::Button {
                    button: MouseButton::Left,
                    down: false
                }
            ]
        );
        assert!(inj.held_keys().is_empty());
        inj.release_all().unwrap();
        assert!(rec.take().is_empty());
    }

    #[test]
    fn drop_releases() {
        let rec = Recorder::default();
        {
            let inj = Injector::with_sink(rec.clone());
            inj.inject(&InputEvent::KeyDown { vk: 0x5B }).unwrap();
            rec.take();
        }
        assert_eq!(rec.take(), vec![key(0x5B, false)]);
    }

    #[test]
    fn sink_errors_are_reported_and_state_still_cleared() {
        let rec = Recorder::default();
        let inj = Injector::with_sink(rec.clone());
        *rec.1.lock().unwrap() = true;
        assert_eq!(
            inj.inject(&InputEvent::KeyDown { vk: 0x41 }),
            Err(Error::BlockedByElevation)
        );
        assert_eq!(inj.held_keys(), vec![0x41]);
        assert_eq!(inj.release_all(), Err(Error::BlockedByElevation));
        assert!(inj.held_keys().is_empty());
    }

    #[test]
    fn injector_is_send_and_sync() {
        fn check<T: Send + Sync>() {}
        check::<Injector>();
    }
}
