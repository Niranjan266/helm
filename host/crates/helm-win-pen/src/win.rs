//! Windows synthetic pen device.

use std::sync::Mutex;

use windows::Win32::Foundation::POINT;
use windows::Win32::UI::Controls::{
    CreateSyntheticPointerDevice, DestroySyntheticPointerDevice, HSYNTHETICPOINTERDEVICE, POINTER_FEEDBACK_DEFAULT,
    POINTER_TYPE_INFO, POINTER_TYPE_INFO_0,
};
use windows::Win32::UI::Input::Pointer::{
    InjectSyntheticPointerInput, POINTER_BUTTON_CHANGE_TYPE, POINTER_FLAGS, POINTER_INFO, POINTER_PEN_INFO,
};
use windows::Win32::UI::WindowsAndMessaging::PT_PEN;

use crate::mapping::{PenFrame, PenSample, PenTracker};
use crate::{Error, Result};

fn win_err(context: &'static str, e: windows::core::Error) -> Error {
    Error::Windows { context, code: e.code().0, message: e.message() }
}

struct Inner {
    device: HSYNTHETICPOINTERDEVICE,
    tracker: PenTracker,
}

// SAFETY: HSYNTHETICPOINTERDEVICE is a plain user32 handle, usable from any thread; all access
// goes through the Mutex.
unsafe impl Send for Inner {}

/// One synthetic pen. `Send + Sync`; calls are serialized internally.
///
/// Dropping it lifts the pen (if touching), moves it out of range and destroys the device.
pub struct Pen {
    inner: Mutex<Inner>,
}

impl Pen {
    /// Create the synthetic pen device (`CreateSyntheticPointerDevice(PT_PEN, 1, DEFAULT)`,
    /// so Windows shows its normal pen feedback/cursor).
    pub fn new() -> Result<Pen> {
        let device = unsafe { CreateSyntheticPointerDevice(PT_PEN, 1, POINTER_FEEDBACK_DEFAULT) }
            .map_err(|e| win_err("CreateSyntheticPointerDevice", e))?;
        Ok(Pen { inner: Mutex::new(Inner { device, tracker: PenTracker::new() }) })
    }

    /// Inject one sample (0–3 Win32 frames, see [`PenTracker::next`]).
    pub fn inject(&self, sample: &PenSample) -> Result<()> {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let frames = g.tracker.next(sample);
        send(g.device, &frames)
    }

    /// Lift and remove the pen at its last position (call on disconnect).
    pub fn release(&self) -> Result<()> {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let frames = g.tracker.release();
        send(g.device, &frames)
    }

    /// True while the pen touches the screen.
    pub fn in_contact(&self) -> bool {
        self.inner.lock().unwrap_or_else(|p| p.into_inner()).tracker.in_contact()
    }
}

impl Drop for Pen {
    fn drop(&mut self) {
        let _ = self.release();
        let g = self.inner.get_mut().unwrap_or_else(|p| p.into_inner());
        unsafe { DestroySyntheticPointerDevice(g.device) };
    }
}

/// Build the `POINTER_TYPE_INFO` for one frame.
pub(crate) fn to_win32(f: &PenFrame) -> POINTER_TYPE_INFO {
    let pointer_info = POINTER_INFO {
        pointerType: PT_PEN,
        pointerId: 0,
        pointerFlags: POINTER_FLAGS(f.pointer_flags),
        ptPixelLocation: POINT { x: f.x, y: f.y },
        ButtonChangeType: POINTER_BUTTON_CHANGE_TYPE(f.button_change),
        ..Default::default()
    };
    POINTER_TYPE_INFO {
        r#type: PT_PEN,
        Anonymous: POINTER_TYPE_INFO_0 {
            penInfo: POINTER_PEN_INFO {
                pointerInfo: pointer_info,
                penFlags: f.pen_flags,
                penMask: f.pen_mask,
                pressure: f.pressure,
                rotation: f.rotation,
                tiltX: f.tilt_x,
                tiltY: f.tilt_y,
            },
        },
    }
}

fn send(device: HSYNTHETICPOINTERDEVICE, frames: &[PenFrame]) -> Result<()> {
    for f in frames {
        let info = [to_win32(f)];
        unsafe { InjectSyntheticPointerInput(device, &info) }.map_err(|e| win_err("InjectSyntheticPointerInput", e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::{button_change, pen_flag, pen_mask, pointer_flag};
    use windows::Win32::UI::Input::Pointer as P;
    use windows::Win32::UI::WindowsAndMessaging as W;

    #[test]
    fn constants_match_win32() {
        assert_eq!(pointer_flag::INRANGE, P::POINTER_FLAG_INRANGE.0);
        assert_eq!(pointer_flag::INCONTACT, P::POINTER_FLAG_INCONTACT.0);
        assert_eq!(pointer_flag::FIRSTBUTTON, P::POINTER_FLAG_FIRSTBUTTON.0);
        assert_eq!(pointer_flag::SECONDBUTTON, P::POINTER_FLAG_SECONDBUTTON.0);
        assert_eq!(pointer_flag::PRIMARY, P::POINTER_FLAG_PRIMARY.0);
        assert_eq!(pointer_flag::DOWN, P::POINTER_FLAG_DOWN.0);
        assert_eq!(pointer_flag::UPDATE, P::POINTER_FLAG_UPDATE.0);
        assert_eq!(pointer_flag::UP, P::POINTER_FLAG_UP.0);
        assert_eq!(button_change::NONE, P::POINTER_CHANGE_NONE.0);
        assert_eq!(button_change::FIRSTBUTTON_DOWN, P::POINTER_CHANGE_FIRSTBUTTON_DOWN.0);
        assert_eq!(button_change::FIRSTBUTTON_UP, P::POINTER_CHANGE_FIRSTBUTTON_UP.0);
        assert_eq!(pen_flag::BARREL, W::PEN_FLAG_BARREL);
        assert_eq!(pen_flag::INVERTED, W::PEN_FLAG_INVERTED);
        assert_eq!(pen_flag::ERASER, W::PEN_FLAG_ERASER);
        assert_eq!(pen_mask::PRESSURE, W::PEN_MASK_PRESSURE);
        assert_eq!(pen_mask::ROTATION, W::PEN_MASK_ROTATION);
        assert_eq!(pen_mask::TILT_X, W::PEN_MASK_TILT_X);
        assert_eq!(pen_mask::TILT_Y, W::PEN_MASK_TILT_Y);
    }

    #[test]
    fn frame_to_win32_struct() {
        let f = PenFrame {
            x: -5,
            y: 7,
            pointer_flags: pointer_flag::INRANGE | pointer_flag::INCONTACT,
            button_change: button_change::FIRSTBUTTON_DOWN,
            pen_flags: pen_flag::ERASER,
            pen_mask: pen_mask::ALL,
            pressure: 900,
            rotation: 45,
            tilt_x: -30,
            tilt_y: 12,
        };
        let w = to_win32(&f);
        assert_eq!(w.r#type, PT_PEN);
        let pen = unsafe { w.Anonymous.penInfo };
        assert_eq!(pen.pointerInfo.pointerType, PT_PEN);
        assert_eq!((pen.pointerInfo.ptPixelLocation.x, pen.pointerInfo.ptPixelLocation.y), (-5, 7));
        assert_eq!(pen.pointerInfo.pointerFlags.0, f.pointer_flags);
        assert_eq!(pen.pointerInfo.ButtonChangeType.0, f.button_change);
        assert_eq!((pen.pressure, pen.rotation, pen.tiltX, pen.tiltY), (900, 45, -30, 12));
        assert_eq!(pen.penFlags, pen_flag::ERASER);
    }
}
