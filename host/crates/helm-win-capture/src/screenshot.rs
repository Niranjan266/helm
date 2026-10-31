//! Still screenshots with GDI: `BitBlt` (with `CAPTUREBLT`, so layered windows are included)
//! from the screen DC into a 32-bit top-down DIB section, then BGRA → RGBA → PNG.

use std::ffi::c_void;

use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GdiFlush, GetDC, HBITMAP, HDC, HGDIOBJ, ReleaseDC, SRCCOPY,
    SelectObject,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

use crate::dpi::ThreadDpiScope;
use crate::error::{Context, Error, Result};
use crate::monitors::find_monitor;
use crate::pixels::bgra_to_rgba_in_place;

/// An RGBA8 image, rows top to bottom, tightly packed (`width * 4` bytes per row).
#[derive(Debug, Clone)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// Releases the GDI objects in reverse order of creation, even on early return.
struct GdiCapture {
    screen: HDC,
    mem: HDC,
    bitmap: Option<HBITMAP>,
    old: Option<HGDIOBJ>,
}

impl Drop for GdiCapture {
    fn drop(&mut self) {
        // SAFETY: each handle was created by this struct and is released exactly once.
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

/// Capture a rectangle of the virtual desktop (physical pixels) as RGBA.
pub fn screenshot_rect_rgba(x: i32, y: i32, width: u32, height: u32) -> Result<RgbaImage> {
    if width == 0 || height == 0 || width > 32_768 || height > 32_768 {
        return Err(Error::InvalidConfig(format!("bad capture size {width}x{height}")));
    }
    let _dpi = ThreadDpiScope::per_monitor_v2();
    let (w, h) = (width as i32, height as i32);
    // SAFETY: standard GDI sequence; every object is owned by `GdiCapture` and freed on drop.
    // `bits` points to the DIB section's `w*h*4` bytes while the bitmap is alive.
    unsafe {
        let screen = GetDC(None);
        let mut g = GdiCapture { screen, mem: HDC::default(), bitmap: None, old: None };
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
                biHeight: -h, // negative = top-down rows
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
        BitBlt(g.mem, 0, 0, w, h, Some(screen), x, y, SRCCOPY | CAPTUREBLT).ctx("BitBlt")?;
        let _ = GdiFlush();
        // 32 bpp rows are always DWORD aligned, so the stride is exactly width*4.
        let len = width as usize * height as usize * 4;
        let mut pixels = std::slice::from_raw_parts(bits as *const u8, len).to_vec();
        drop(g);
        bgra_to_rgba_in_place(&mut pixels);
        Ok(RgbaImage { width, height, pixels })
    }
}

/// Bounds of the whole virtual desktop `(x, y, width, height)` in physical pixels.
pub fn virtual_desktop_rect() -> (i32, i32, u32, u32) {
    let _dpi = ThreadDpiScope::per_monitor_v2();
    // SAFETY: GetSystemMetrics has no preconditions.
    unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN).max(0) as u32,
            GetSystemMetrics(SM_CYVIRTUALSCREEN).max(0) as u32,
        )
    }
}

/// Screenshot as RGBA. `Some(id)` = that monitor ([`crate::MonitorInfo::id`]),
/// `None` = the whole virtual desktop (all monitors; areas not covered by a monitor are black).
pub fn screenshot_rgba(monitor_id: Option<&str>) -> Result<RgbaImage> {
    match monitor_id {
        Some(id) => {
            let m = find_monitor(Some(id))?.info;
            screenshot_rect_rgba(m.x, m.y, m.width, m.height)
        }
        None => {
            let (x, y, w, h) = virtual_desktop_rect();
            screenshot_rect_rgba(x, y, w, h)
        }
    }
}

/// Screenshot of the primary monitor as RGBA.
pub fn screenshot_primary_rgba() -> Result<RgbaImage> {
    let m = find_monitor(None)?.info;
    screenshot_rect_rgba(m.x, m.y, m.width, m.height)
}

/// Encode an RGBA image as PNG (fast compression: screenshots are large and sent once).
pub fn encode_png(img: &RgbaImage) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(img.pixels.len() / 4);
    {
        let mut enc = png::Encoder::new(&mut out, img.width, img.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let mut w = enc.write_header().map_err(|e| Error::Png(e.to_string()))?;
        w.write_image_data(&img.pixels).map_err(|e| Error::Png(e.to_string()))?;
        w.finish().map_err(|e| Error::Png(e.to_string()))?;
    }
    Ok(out)
}

/// Screenshot as PNG bytes. `Some(id)` = one monitor, `None` = the whole virtual desktop.
pub fn screenshot_png(monitor_id: Option<&str>) -> Result<Vec<u8>> {
    encode_png(&screenshot_rgba(monitor_id)?)
}
