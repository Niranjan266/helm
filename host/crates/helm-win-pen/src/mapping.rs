//! Pure mapping from Helm pen samples to Win32 pointer frames.
//!
//! Nothing here calls Windows, so it is tested on every platform. The flag values mirror the
//! Win32 `POINTER_FLAG_*`, `POINTER_CHANGE_*`, `PEN_FLAG_*` and `PEN_MASK_*` constants
//! (a Windows-only test checks they match the `windows` crate).

use serde::{Deserialize, Serialize};

/// `POINTER_FLAG_*` values.
pub mod pointer_flag {
    pub const INRANGE: u32 = 0x0000_0002;
    pub const INCONTACT: u32 = 0x0000_0004;
    pub const FIRSTBUTTON: u32 = 0x0000_0010;
    pub const SECONDBUTTON: u32 = 0x0000_0020;
    pub const PRIMARY: u32 = 0x0000_2000;
    pub const DOWN: u32 = 0x0001_0000;
    pub const UPDATE: u32 = 0x0002_0000;
    pub const UP: u32 = 0x0004_0000;
}

/// `POINTER_BUTTON_CHANGE_TYPE` values.
pub mod button_change {
    pub const NONE: i32 = 0;
    pub const FIRSTBUTTON_DOWN: i32 = 1;
    pub const FIRSTBUTTON_UP: i32 = 2;
}

/// `PEN_FLAG_*` values.
pub mod pen_flag {
    pub const BARREL: u32 = 1;
    pub const INVERTED: u32 = 2;
    pub const ERASER: u32 = 4;
}

/// `PEN_MASK_*` values.
pub mod pen_mask {
    pub const PRESSURE: u32 = 1;
    pub const ROTATION: u32 = 2;
    pub const TILT_X: u32 = 4;
    pub const TILT_Y: u32 = 8;
    pub const ALL: u32 = PRESSURE | ROTATION | TILT_X | TILT_Y;
}

/// Win32 pen pressure range is 0..=1024.
pub const MAX_PRESSURE: u32 = 1024;

/// What the pen is doing in this sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PenPhase {
    /// In range above the screen, not touching.
    Hover,
    /// Touching the screen.
    Contact,
    /// Lifted from the screen, still in range (same as `Hover` after a contact).
    Up,
    /// Out of range (pen moved away). Ends the interaction.
    Leave,
}

/// One pen sample from the phone, already in physical screen pixels.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PenSample {
    pub x: i32,
    pub y: i32,
    pub phase: PenPhase,
    /// 0.0 ..= 1.0 (clamped). Ignored unless `phase` is `Contact`.
    #[serde(default)]
    pub pressure: f32,
    /// Degrees, -90 ..= 90 (clamped). Positive = tilted to the right.
    #[serde(default)]
    pub tilt_x: f32,
    /// Degrees, -90 ..= 90 (clamped). Positive = tilted towards the user.
    #[serde(default)]
    pub tilt_y: f32,
    /// Degrees clockwise, wrapped into 0 ..= 359.
    #[serde(default)]
    pub rotation: f32,
    /// Eraser end (or eraser button) active.
    #[serde(default)]
    pub eraser: bool,
    /// Barrel (side) button pressed.
    #[serde(default)]
    pub barrel: bool,
}

impl PenSample {
    pub fn new(x: i32, y: i32, phase: PenPhase) -> Self {
        PenSample { x, y, phase, pressure: 0.0, tilt_x: 0.0, tilt_y: 0.0, rotation: 0.0, eraser: false, barrel: false }
    }
}

/// One Win32 pen frame (the fields of `POINTER_PEN_INFO` we fill in).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PenFrame {
    pub x: i32,
    pub y: i32,
    pub pointer_flags: u32,
    pub button_change: i32,
    pub pen_flags: u32,
    pub pen_mask: u32,
    pub pressure: u32,
    pub rotation: u32,
    pub tilt_x: i32,
    pub tilt_y: i32,
}

/// 0.0..=1.0 → 0..=1024. NaN → 0.
pub fn map_pressure(p: f32) -> u32 {
    if p.is_nan() {
        return 0;
    }
    (p.clamp(0.0, 1.0) * MAX_PRESSURE as f32).round() as u32
}

/// Degrees → -90..=90. NaN → 0.
pub fn map_tilt(t: f32) -> i32 {
    if t.is_nan() {
        return 0;
    }
    t.clamp(-90.0, 90.0).round() as i32
}

/// Degrees → 0..=359. NaN → 0.
pub fn map_rotation(r: f32) -> u32 {
    if !r.is_finite() {
        return 0;
    }
    (r.rem_euclid(360.0).round() as u32) % 360
}

/// A rectangle in physical virtual-desktop pixels (e.g. a monitor).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Normalized 0..1 coordinates on `rect` → physical pixel, clamped inside the rectangle.
pub fn map_normalized(nx: f32, ny: f32, rect: ScreenRect) -> (i32, i32) {
    fn axis(n: f32, origin: i32, len: u32) -> i32 {
        let n = if n.is_nan() { 0.0 } else { n.clamp(0.0, 1.0) };
        let span = len.saturating_sub(1) as f32;
        origin + (n * span).round() as i32
    }
    (axis(nx, rect.x, rect.width), axis(ny, rect.y, rect.height))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Out,
    Hover,
    Contact,
}

/// Turns a stream of [`PenSample`]s into the Win32 frame sequence (down / update / up / leave),
/// inserting the transitions Windows expects (e.g. a hover frame before the first down, an up
/// before leaving while in contact).
#[derive(Debug)]
pub struct PenTracker {
    state: State,
    last: (i32, i32),
}

impl Default for PenTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl PenTracker {
    pub fn new() -> Self {
        PenTracker { state: State::Out, last: (0, 0) }
    }

    /// True while the pen touches the screen.
    pub fn in_contact(&self) -> bool {
        self.state == State::Contact
    }

    /// True while the pen is in range (hover or contact).
    pub fn in_range(&self) -> bool {
        self.state != State::Out
    }

    /// Frames for one sample (0 to 3 frames).
    pub fn next(&mut self, s: &PenSample) -> Vec<PenFrame> {
        use pointer_flag::*;
        let target = match s.phase {
            PenPhase::Hover | PenPhase::Up => State::Hover,
            PenPhase::Contact => State::Contact,
            PenPhase::Leave => State::Out,
        };
        let mut out = Vec::with_capacity(2);
        let hover = |s: &PenSample, flags: u32, change: i32| frame(s, PRIMARY | flags, change, false);
        let contact = |s: &PenSample, flags: u32, change: i32| frame(s, PRIMARY | flags, change, true);
        match (self.state, target) {
            (State::Out, State::Out) => {}
            (State::Out, State::Hover) | (State::Hover, State::Hover) => {
                out.push(hover(s, INRANGE | UPDATE, button_change::NONE));
            }
            (State::Out, State::Contact) => {
                out.push(hover(s, INRANGE | UPDATE, button_change::NONE));
                out.push(contact(s, INRANGE | INCONTACT | FIRSTBUTTON | DOWN, button_change::FIRSTBUTTON_DOWN));
            }
            (State::Hover, State::Contact) => {
                out.push(contact(s, INRANGE | INCONTACT | FIRSTBUTTON | DOWN, button_change::FIRSTBUTTON_DOWN));
            }
            (State::Contact, State::Contact) => {
                out.push(contact(s, INRANGE | INCONTACT | FIRSTBUTTON | UPDATE, button_change::NONE));
            }
            (State::Contact, State::Hover) => {
                out.push(hover(s, INRANGE | UP, button_change::FIRSTBUTTON_UP));
            }
            (State::Contact, State::Out) => {
                out.push(hover(s, INRANGE | UP, button_change::FIRSTBUTTON_UP));
                out.push(hover(s, UPDATE, button_change::NONE));
            }
            (State::Hover, State::Out) => {
                out.push(hover(s, UPDATE, button_change::NONE));
            }
        }
        self.state = target;
        self.last = (s.x, s.y);
        out
    }

    /// Frames that lift and remove the pen at its last position (used on disconnect / drop).
    pub fn release(&mut self) -> Vec<PenFrame> {
        let s = PenSample::new(self.last.0, self.last.1, PenPhase::Leave);
        self.next(&s)
    }
}

fn frame(s: &PenSample, pointer_flags: u32, button_change: i32, touching: bool) -> PenFrame {
    let mut pen_flags = 0;
    if s.barrel {
        pen_flags |= pen_flag::BARREL;
    }
    if s.eraser {
        pen_flags |= pen_flag::INVERTED;
        if touching {
            pen_flags |= pen_flag::ERASER;
        }
    }
    PenFrame {
        x: s.x,
        y: s.y,
        pointer_flags,
        button_change,
        pen_flags,
        pen_mask: pen_mask::ALL,
        pressure: if touching { map_pressure(s.pressure) } else { 0 },
        rotation: map_rotation(s.rotation),
        tilt_x: map_tilt(s.tilt_x),
        tilt_y: map_tilt(s.tilt_y),
    }
}

#[cfg(test)]
mod tests {
    use super::pointer_flag::*;
    use super::*;

    fn s(x: i32, y: i32, phase: PenPhase) -> PenSample {
        PenSample::new(x, y, phase)
    }

    #[test]
    fn pressure_tilt_rotation_ranges() {
        assert_eq!(map_pressure(0.0), 0);
        assert_eq!(map_pressure(0.5), 512);
        assert_eq!(map_pressure(1.0), 1024);
        assert_eq!(map_pressure(7.0), 1024);
        assert_eq!(map_pressure(-1.0), 0);
        assert_eq!(map_pressure(f32::NAN), 0);
        assert_eq!(map_tilt(45.4), 45);
        assert_eq!(map_tilt(-120.0), -90);
        assert_eq!(map_tilt(f32::NAN), 0);
        assert_eq!(map_rotation(370.0), 10);
        assert_eq!(map_rotation(-90.0), 270);
        assert_eq!(map_rotation(359.7), 0);
        assert_eq!(map_rotation(f32::INFINITY), 0);
    }

    #[test]
    fn normalized_mapping_clamps_inside_rect() {
        let r = ScreenRect { x: -1920, y: 0, width: 1920, height: 1080 };
        assert_eq!(map_normalized(0.0, 0.0, r), (-1920, 0));
        assert_eq!(map_normalized(1.0, 1.0, r), (-1, 1079));
        assert_eq!(map_normalized(0.5, 0.5, r), (-1920 + 960, 540));
        assert_eq!(map_normalized(2.0, -3.0, r), (-1, 0));
        assert_eq!(map_normalized(f32::NAN, 0.0, r), (-1920, 0));
    }

    #[test]
    fn stroke_sequence() {
        let mut t = PenTracker::new();
        let h = t.next(&s(10, 10, PenPhase::Hover));
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].pointer_flags, PRIMARY | INRANGE | UPDATE);
        assert_eq!(h[0].pressure, 0);

        let mut c = s(11, 11, PenPhase::Contact);
        c.pressure = 0.25;
        let d = t.next(&c);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].pointer_flags, PRIMARY | INRANGE | INCONTACT | FIRSTBUTTON | DOWN);
        assert_eq!(d[0].button_change, button_change::FIRSTBUTTON_DOWN);
        assert_eq!(d[0].pressure, 256);
        assert!(t.in_contact());

        c.x = 20;
        let m = t.next(&c);
        assert_eq!(m[0].pointer_flags, PRIMARY | INRANGE | INCONTACT | FIRSTBUTTON | UPDATE);
        assert_eq!(m[0].button_change, button_change::NONE);
        assert_eq!(m[0].x, 20);

        let u = t.next(&s(20, 11, PenPhase::Up));
        assert_eq!(u[0].pointer_flags, PRIMARY | INRANGE | UP);
        assert_eq!(u[0].button_change, button_change::FIRSTBUTTON_UP);
        assert_eq!(u[0].pressure, 0);
        assert!(t.in_range() && !t.in_contact());

        let l = t.next(&s(20, 11, PenPhase::Leave));
        assert_eq!(l[0].pointer_flags, PRIMARY | UPDATE);
        assert!(!t.in_range());
        assert!(t.next(&s(20, 11, PenPhase::Leave)).is_empty());
    }

    #[test]
    fn contact_from_out_of_range_hovers_first() {
        let mut t = PenTracker::new();
        let f = t.next(&s(5, 5, PenPhase::Contact));
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].pointer_flags & (INCONTACT | DOWN), 0);
        assert_ne!(f[1].pointer_flags & DOWN, 0);
    }

    #[test]
    fn leave_while_touching_lifts_first_and_release_works() {
        let mut t = PenTracker::new();
        t.next(&s(5, 5, PenPhase::Contact));
        let f = t.next(&s(6, 6, PenPhase::Leave));
        assert_eq!(f.len(), 2);
        assert_ne!(f[0].pointer_flags & UP, 0);
        assert_eq!(f[1].pointer_flags & INRANGE, 0);

        t.next(&s(7, 8, PenPhase::Contact));
        let r = t.release();
        assert_eq!(r.len(), 2);
        assert_eq!((r[0].x, r[0].y), (7, 8));
        assert!(t.release().is_empty());
    }

    #[test]
    fn eraser_and_barrel_flags() {
        let mut t = PenTracker::new();
        let mut e = s(1, 1, PenPhase::Hover);
        e.eraser = true;
        e.barrel = true;
        let h = t.next(&e);
        assert_eq!(h[0].pen_flags, pen_flag::INVERTED | pen_flag::BARREL);
        e.phase = PenPhase::Contact;
        let c = t.next(&e);
        assert_eq!(c[0].pen_flags, pen_flag::INVERTED | pen_flag::ERASER | pen_flag::BARREL);
        assert_eq!(c[0].pen_mask, pen_mask::ALL);
    }

    #[test]
    fn sample_serde_defaults() {
        let v: PenSample = serde_json::from_str(r#"{"x":3,"y":4,"phase":"contact","pressure":0.5}"#).unwrap();
        assert_eq!(v.phase, PenPhase::Contact);
        assert_eq!(v.pressure, 0.5);
        assert!(!v.eraser);
        let back = serde_json::to_string(&v).unwrap();
        assert!(back.contains("\"phase\":\"contact\""));
    }
}
