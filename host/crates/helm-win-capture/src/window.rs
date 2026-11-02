//! Single windows: where a window is on the desktop (the rectangle Windows.Graphics.Capture
//! shows) and small still thumbnails for a window picker.
//!
//! * Frame rectangle: `DwmGetWindowAttribute(DWMWA_EXTENDED_FRAME_BOUNDS)` (physical pixels,
//!   without the invisible resize borders that `GetWindowRect` includes on Windows 10/11),
//!   falling back to `GetWindowRect`.
//! * Thumbnail: `PrintWindow(PW_RENDERFULLCONTENT)` into a 32-bit DIB section, so covered
//!   (occluded) windows and DirectComposition / DirectX content render too, cropped to the frame
//!   rectangle and box-filtered down. Minimized windows have nothing to render and are refused.
//!
//! Microsoft Learn: DwmGetWindowAttribute, DWMWINDOWATTRIBUTE, PrintWindow, CreateDIBSection.

use std::ffi::c_void;

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, GdiFlush, GetDC, HBITMAP, HDC, HGDIOBJ, ReleaseDC, SelectObject,
};
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, IsIconic, IsWindow, IsWindowVisible};

use crate::dpi::ThreadDpiScope;
use crate::error::{Context, Error, Result};
use crate::pixels::{bgra_to_rgba_in_place, crop_rgba, downscale_rgba, thumb_size};
use crate::screenshot::{RgbaImage, encode_png};

/// `PW_RENDERFULLCONTENT` (Windows 8.1+): render DirectComposition / DirectX content too.
const PW_RENDERFULLCONTENT: PRINT_WINDOW_FLAGS = PRINT_WINDOW_FLAGS(2);

/// A window rectangle on the virtual desktop, physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

fn hwnd(h: isize) -> HWND {
    HWND(h as *mut c_void)
}

fn from_rect(r: RECT) -> Option<WindowRect> {
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    (w > 0 && h > 0).then_some(WindowRect { x: r.left, y: r.top, width: w as u32, height: h as u32 })
}

/// `GetWindowRect` in physical pixels (includes the invisible resize borders).
fn outer_rect(h: isize) -> Option<WindowRect> {
    let _dpi = ThreadDpiScope::per_monitor_v2();
    let mut r = RECT::default();
    // SAFETY: out-pointer to a local; an invalid handle just fails.
    unsafe { GetWindowRect(hwnd(h), &mut r) }.ok()?;
    from_rect(r)
}

/// The visible frame of a top-level window (what window capture shows), physical pixels.
/// `None` if the handle is not a window or it has no area.
pub fn window_frame_rect(h: isize) -> Option<WindowRect> {
    // SAFETY: IsWindow accepts any value.
    if h == 0 || !unsafe { IsWindow(Some(hwnd(h))) }.as_bool() {
        return None;
    }
    let mut r = RECT::default();
    // SAFETY: out-pointer to a RECT of the size passed; DWM always reports physical pixels.
    let dwm = unsafe {
        DwmGetWindowAttribute(
            hwnd(h),
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut r as *mut RECT as *mut c_void,
            size_of::<RECT>() as u32,
        )
    };
    match dwm {
        Ok(()) => from_rect(r).or_else(|| outer_rect(h)),
        Err(_) => outer_rect(h),
    }
}

/// Releases the GDI objects in reverse order of creation.
struct Gdi {
    screen: HDC,
    mem: HDC,
    bitmap: Option<HBITMAP>,
    old: Option<HGDIOBJ>,
}

impl Drop for Gdi {
    fn drop(&mut self) {
        // SAFETY: each handle was created here and is released exactly once.
        unsafe {
            if let Some(old) = self.old {
                SelectObject(self.mem, old);
            }
            if let Some(b) = self.bitmap {
                let _ = DeleteObject(b.into());
            }
            if !self.mem.is_invalid() {
                let _ = DeleteDC(self.mem);
            }
            if !self.screen.is_invalid() {
                ReleaseDC(None, self.screen);
            }
        }
    }
}

/// The whole window (frame included) as RGBA, at its physical size.
pub fn window_rgba(h: isize) -> Result<RgbaImage> {
    // SAFETY: these accept any value.
    if h == 0 || !unsafe { IsWindow(Some(hwnd(h))) }.as_bool() {
        return Err(Error::InvalidConfig(format!("no window with handle {h}")));
    }
    if unsafe { IsIconic(hwnd(h)) }.as_bool() {
        return Err(Error::Unsupported("the window is minimized".into()));
    }
    if !unsafe { IsWindowVisible(hwnd(h)) }.as_bool() {
        return Err(Error::Unsupported("the window is hidden".into()));
    }
    let _dpi = ThreadDpiScope::per_monitor_v2();
    let outer = outer_rect(h).ok_or_else(|| Error::Unsupported("the window has no area".into()))?;
    if outer.width > 16_384 || outer.height > 16_384 {
        return Err(Error::Unsupported("the window is too large".into()));
    }
    let frame = window_frame_rect(h).unwrap_or(outer);
    let (w, hh) = (outer.width as i32, outer.height as i32);
    // SAFETY: standard GDI sequence; every object is owned by `Gdi` and freed on drop. `bits`
    // points to the DIB section's w*h*4 bytes while the bitmap is alive.
    let mut pixels = unsafe {
        let screen = GetDC(None);
        let mut g = Gdi { screen, mem: HDC::default(), bitmap: None, old: None };
        if screen.is_invalid() {
            return Err(Error::Windows { context: "GetDC", source: windows::core::Error::from_thread() });
        }
        g.mem = CreateCompatibleDC(Some(screen));
        if g.mem.is_invalid() {
            return Err(Error::Windows { context: "CreateCompatibleDC", source: windows::core::Error::from_thread() });
        }
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -hh,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let bmp = CreateDIBSection(Some(screen), &bmi, DIB_RGB_COLORS, &mut bits, None, 0).ctx("CreateDIBSection")?;
        g.bitmap = Some(bmp);
        if bits.is_null() {
            return Err(Error::Unsupported("CreateDIBSection returned no pixel memory".into()));
        }
        g.old = Some(SelectObject(g.mem, bmp.into()));
        if !PrintWindow(hwnd(h), g.mem, PW_RENDERFULLCONTENT).as_bool() {
            return Err(Error::Windows { context: "PrintWindow", source: windows::core::Error::from_thread() });
        }
        let _ = GdiFlush();
        let len = outer.width as usize * outer.height as usize * 4;
        std::slice::from_raw_parts(bits as *const u8, len).to_vec()
    };
    bgra_to_rgba_in_place(&mut pixels);
    // Keep only the visible frame (drop the invisible resize borders).
    let (dx, dy) = ((frame.x - outer.x).max(0) as u32, (frame.y - outer.y).max(0) as u32);
    let (pixels, cw, ch) = crop_rgba(&pixels, outer.width, outer.height, dx, dy, frame.width, frame.height);
    Ok(RgbaImage { width: cw, height: ch, pixels })
}

/// A thumbnail of the window whose longer side is at most `max_side` pixels.
pub fn window_thumbnail_rgba(h: isize, max_side: u32) -> Result<RgbaImage> {
    if !(16..=1024).contains(&max_side) {
        return Err(Error::InvalidConfig(format!("thumbnail size {max_side} must be 16..=1024")));
    }
    let full = window_rgba(h)?;
    let (tw, th) = thumb_size(full.width, full.height, max_side);
    if tw == 0 {
        return Err(Error::Unsupported("the window has no area".into()));
    }
    let pixels = downscale_rgba(&full.pixels, full.width, full.height, tw, th);
    Ok(RgbaImage { width: tw, height: th, pixels })
}

/// [`window_thumbnail_rgba`] as PNG bytes.
pub fn window_thumbnail_png(h: isize, max_side: u32) -> Result<Vec<u8>> {
    encode_png(&window_thumbnail_rgba(h, max_side)?)
}
