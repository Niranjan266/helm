//! Cursor accuracy (feature `g8`, Docs/protocol/pointer.md).
//!
//! Relative `SendInput` motion goes through Windows pointer speed and "Enhance pointer
//! precision", so the same phone swipe moves the cursor a different distance on every PC.
//! In `absolute` mode the agent reads the real cursor position (`GetCursorPos`), adds the
//! phone's `mm` delta (plus the fractional part carried from the previous frame) and moves
//! to that exact pixel with `Injector::move_to`. Games and other apps that read raw input
//! or confine the cursor still need relative motion, so `auto` switches per frame.
//!
//! Everything here is pure except [`win`]; the agent reaches the desktop only through
//! `Platform::pointer_probe`, so tests never move the real cursor.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};

use crate::platform::Rect;

/// `pointer_mode` setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PointerMode {
    /// Absolute unless the foreground app needs raw input (see [`choose`]).
    #[default]
    Auto,
    Absolute,
    Relative,
}

impl PointerMode {
    pub fn parse(s: &str) -> Option<PointerMode> {
        match s {
            "auto" => Some(PointerMode::Auto),
            "absolute" => Some(PointerMode::Absolute),
            "relative" => Some(PointerMode::Relative),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            PointerMode::Auto => "auto",
            PointerMode::Absolute => "absolute",
            PointerMode::Relative => "relative",
        }
    }

    fn to_u8(self) -> u8 {
        match self {
            PointerMode::Auto => 0,
            PointerMode::Absolute => 1,
            PointerMode::Relative => 2,
        }
    }

    fn from_u8(v: u8) -> PointerMode {
        match v {
            1 => PointerMode::Absolute,
            2 => PointerMode::Relative,
            _ => PointerMode::Auto,
        }
    }
}

/// How one `mm` frame is injected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applied {
    Absolute,
    Relative,
}

/// What the PC looks like right now (one cheap probe per `mm` frame).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PointerProbe {
    /// Cursor position, physical pixels of the virtual desktop.
    pub x: i32,
    pub y: i32,
    /// The whole virtual desktop; the cursor is clamped inside it.
    pub bounds: Rect,
    /// A full-screen exclusive (Direct3D) app owns the screen.
    pub exclusive_fullscreen: bool,
    /// `ClipCursor` confines the cursor to less than the virtual desktop.
    pub cursor_clipped: bool,
    /// The foreground app hides the cursor (`CURSORINFO.flags` has no `CURSOR_SHOWING`).
    pub cursor_hidden: bool,
}

/// `auto` uses relative motion for apps that need raw input; otherwise absolute.
pub fn choose(mode: PointerMode, probe: &PointerProbe) -> Applied {
    match mode {
        PointerMode::Absolute => Applied::Absolute,
        PointerMode::Relative => Applied::Relative,
        PointerMode::Auto => {
            if probe.exclusive_fullscreen || probe.cursor_clipped || probe.cursor_hidden {
                Applied::Relative
            } else {
                Applied::Absolute
            }
        }
    }
}

/// Largest single `mm` delta honoured (pixels); anything bigger is a corrupt frame.
const MAX_DELTA: f64 = 20_000.0;

/// Turns fractional deltas into whole-pixel absolute positions, carrying the remainder.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AbsoluteMapper {
    carry_x: f64,
    carry_y: f64,
}

impl AbsoluteMapper {
    pub fn new() -> AbsoluteMapper {
        AbsoluteMapper::default()
    }

    /// Forget the carried fraction (after relative motion or a pointer jump).
    pub fn reset(&mut self) {
        *self = AbsoluteMapper::default();
    }

    /// The pixel to move to for `delta`, starting from the real cursor `cur`. `None` when
    /// the move is less than one pixel (the fraction is kept) or changes nothing.
    pub fn step(&mut self, cur: (i32, i32), bounds: Rect, dx: f32, dy: f32) -> Option<(i32, i32)> {
        let (nx, cx) = axis(cur.0, bounds.x, bounds.width, dx, self.carry_x);
        let (ny, cy) = axis(cur.1, bounds.y, bounds.height, dy, self.carry_y);
        self.carry_x = cx;
        self.carry_y = cy;
        ((nx, ny) != cur).then_some((nx, ny))
    }
}

/// One axis: returns the new position and the new carry. Whole pixels are taken towards
/// zero so left and right feel the same; hitting an edge drops the carry.
fn axis(cur: i32, origin: i32, size: u32, delta: f32, carry: f64) -> (i32, f64) {
    let d = if delta.is_finite() {
        f64::from(delta)
    } else if delta.is_infinite() {
        f64::from(delta.signum()) * MAX_DELTA
    } else {
        0.0
    };
    let total = d.clamp(-MAX_DELTA, MAX_DELTA) + carry;
    let whole = total.trunc();
    let frac = total - whole;
    let lo = i64::from(origin);
    let hi = lo + i64::from(size.max(1)) - 1;
    let want = i64::from(cur) + whole as i64;
    let pos = want.clamp(lo, hi);
    let carry = if pos == want { frac } else { 0.0 };
    (pos as i32, carry)
}

pub const SETTINGS_FILE: &str = "pointer.json";

#[derive(Default, Serialize, Deserialize)]
struct SettingsFile {
    #[serde(default)]
    pointer_mode: PointerMode,
}

/// `pointer.json`: the saved mode, shared by every session.
#[derive(Debug)]
pub struct PointerSettings {
    mode: AtomicU8,
    path: Option<PathBuf>,
}

impl PointerSettings {
    /// In-memory only (tests, detached sessions).
    pub fn in_memory(mode: PointerMode) -> PointerSettings {
        PointerSettings {
            mode: AtomicU8::new(mode.to_u8()),
            path: None,
        }
    }

    pub fn load(dir: &Path) -> PointerSettings {
        let path = dir.join(SETTINGS_FILE);
        let file: SettingsFile = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        PointerSettings {
            mode: AtomicU8::new(file.pointer_mode.to_u8()),
            path: Some(path),
        }
    }

    pub fn mode(&self) -> PointerMode {
        PointerMode::from_u8(self.mode.load(Ordering::Relaxed))
    }

    pub fn set_mode(&self, mode: PointerMode) -> std::io::Result<()> {
        self.mode.store(mode.to_u8(), Ordering::Relaxed);
        if let Some(path) = &self.path {
            let text = serde_json::to_string_pretty(&SettingsFile { pointer_mode: mode })?;
            std::fs::write(path, text)?;
        }
        Ok(())
    }
}

/// Windows probe: `GetCursorPos`, `GetCursorInfo`, `GetClipCursor`, `GetSystemMetrics`,
/// `SHQueryUserNotificationState` (Microsoft Learn). The process is per-monitor-v2 aware,
/// so every coordinate is a physical pixel.
#[cfg(windows)]
pub mod win {
    use windows::Win32::Foundation::{POINT, RECT};
    use windows::Win32::UI::Shell::{QUNS_RUNNING_D3D_FULL_SCREEN, SHQueryUserNotificationState};
    use windows::Win32::UI::WindowsAndMessaging::{
        CURSOR_SHOWING, CURSORINFO, GetClipCursor, GetCursorInfo, GetCursorPos, GetSystemMetrics,
        SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    };

    use super::PointerProbe;
    use crate::platform::{PResult, Rect};
    use crate::router::RpcError;

    pub fn probe() -> PResult<PointerProbe> {
        // SAFETY: plain Win32 queries into local, correctly sized structs.
        unsafe {
            let mut pt = POINT::default();
            GetCursorPos(&mut pt).map_err(|e| RpcError::internal(format!("GetCursorPos: {e}")))?;
            let bounds = Rect {
                x: GetSystemMetrics(SM_XVIRTUALSCREEN),
                y: GetSystemMetrics(SM_YVIRTUALSCREEN),
                width: GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1) as u32,
                height: GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1) as u32,
            };
            let mut info = CURSORINFO {
                cbSize: std::mem::size_of::<CURSORINFO>() as u32,
                ..Default::default()
            };
            let hidden =
                GetCursorInfo(&mut info).is_ok() && (info.flags.0 & CURSOR_SHOWING.0) == 0;
            let mut clip = RECT::default();
            let clipped = GetClipCursor(&mut clip).is_ok()
                && !(clip.left <= bounds.x
                    && clip.top <= bounds.y
                    && i64::from(clip.right) >= i64::from(bounds.x) + i64::from(bounds.width)
                    && i64::from(clip.bottom) >= i64::from(bounds.y) + i64::from(bounds.height));
            let exclusive = SHQueryUserNotificationState()
                .map(|s| s == QUNS_RUNNING_D3D_FULL_SCREEN)
                .unwrap_or(false);
            Ok(PointerProbe {
                x: pt.x,
                y: pt.y,
                bounds,
                exclusive_fullscreen: exclusive,
                cursor_clipped: clipped,
                cursor_hidden: hidden,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESK: Rect = Rect {
        x: -1920,
        y: 0,
        width: 3840,
        height: 1080,
    };

    fn probe() -> PointerProbe {
        PointerProbe {
            x: 100,
            y: 100,
            bounds: DESK,
            exclusive_fullscreen: false,
            cursor_clipped: false,
            cursor_hidden: false,
        }
    }

    #[test]
    fn mode_names_round_trip() {
        for m in [PointerMode::Auto, PointerMode::Absolute, PointerMode::Relative] {
            assert_eq!(PointerMode::parse(m.as_str()), Some(m));
            assert_eq!(PointerMode::from_u8(m.to_u8()), m);
        }
        assert_eq!(PointerMode::parse("fast"), None);
    }

    #[test]
    fn auto_goes_relative_for_raw_input_apps() {
        assert_eq!(choose(PointerMode::Auto, &probe()), Applied::Absolute);
        let tweaks: [fn(&mut PointerProbe); 3] = [
            |p| p.exclusive_fullscreen = true,
            |p| p.cursor_clipped = true,
            |p| p.cursor_hidden = true,
        ];
        for tweak in tweaks {
            let mut p = probe();
            tweak(&mut p);
            assert_eq!(choose(PointerMode::Auto, &p), Applied::Relative);
            assert_eq!(choose(PointerMode::Absolute, &p), Applied::Absolute);
        }
        assert_eq!(choose(PointerMode::Relative, &probe()), Applied::Relative);
    }

    #[test]
    fn whole_pixels_move_and_fractions_carry() {
        let mut m = AbsoluteMapper::new();
        assert_eq!(m.step((100, 100), DESK, 0.4, 0.0), None);
        assert_eq!(m.step((100, 100), DESK, 0.4, 0.0), None);
        // 0.4 * 3 = 1.2 -> one pixel, 0.2 stays
        assert_eq!(m.step((100, 100), DESK, 0.4, 0.0), Some((101, 100)));
        assert_eq!(m.step((101, 100), DESK, 0.8, 0.0), Some((102, 100)));
    }

    #[test]
    fn left_and_right_are_symmetric() {
        let mut a = AbsoluteMapper::new();
        let mut b = AbsoluteMapper::new();
        assert_eq!(a.step((50, 50), DESK, 2.7, -2.7), Some((52, 48)));
        assert_eq!(b.step((50, 50), DESK, -2.7, 2.7), Some((48, 52)));
        // the 0.7 remainders add up to the next pixel
        assert_eq!(a.step((52, 48), DESK, 0.4, -0.4), Some((53, 47)));
    }

    #[test]
    fn clamps_to_the_virtual_desktop_and_drops_the_carry() {
        let mut m = AbsoluteMapper::new();
        // already at the right/top edge: nothing to do
        assert_eq!(m.step((1919, 0), DESK, 50.5, -50.5), None);
        // no stored overshoot: the first step back moves exactly one pixel
        assert_eq!(m.step((1919, 0), DESK, -1.0, 1.0), Some((1918, 1)));
        assert_eq!(m.step((-1920, 1079), DESK, -9.0, 9.0), None);
    }

    #[test]
    fn garbage_deltas_are_bounded() {
        let mut m = AbsoluteMapper::new();
        assert_eq!(m.step((10, 10), DESK, f32::NAN, f32::INFINITY), Some((10, 1079)));
        let mut m = AbsoluteMapper::new();
        assert_eq!(m.step((10, 10), DESK, f32::NAN, 0.0), None);
    }

    #[test]
    fn reset_clears_the_carry() {
        let mut m = AbsoluteMapper::new();
        assert_eq!(m.step((0, 0), DESK, 0.9, 0.0), None);
        m.reset();
        assert_eq!(m.step((0, 0), DESK, 0.2, 0.0), None);
    }

    #[test]
    fn settings_persist() {
        let dir = tempfile::tempdir().unwrap();
        let s = PointerSettings::load(dir.path());
        assert_eq!(s.mode(), PointerMode::Auto);
        s.set_mode(PointerMode::Relative).unwrap();
        assert_eq!(PointerSettings::load(dir.path()).mode(), PointerMode::Relative);
        std::fs::write(dir.path().join(SETTINGS_FILE), "not json").unwrap();
        assert_eq!(PointerSettings::load(dir.path()).mode(), PointerMode::Auto);
    }
}
