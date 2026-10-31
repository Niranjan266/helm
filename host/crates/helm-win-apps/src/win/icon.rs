//! Extracting app icons as PNG.

use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits,
    GetObjectW, HBITMAP, HGDIOBJ, ReleaseDC,
};
use windows::Win32::UI::Shell::{IShellItemImageFactory, SIIGBF_BIGGERSIZEOK, SIIGBF_ICONONLY};
use windows::core::Interface;

use super::{apps, win_err};
use crate::{Error, Result, util};

/// Deletes the bitmap when dropped.
struct OwnedBitmap(HBITMAP);
impl Drop for OwnedBitmap {
    fn drop(&mut self) {
        // SAFETY: we own the bitmap returned by GetImage.
        unsafe {
            let _ = DeleteObject(HGDIOBJ(self.0.0));
        }
    }
}

pub(crate) fn icon_png(app_id: &str, size_px: u32) -> Result<Vec<u8>> {
    let item = apps::item_for(app_id)?;
    let factory: IShellItemImageFactory = item
        .cast()
        .map_err(win_err("QueryInterface(IShellItemImageFactory)"))?;
    let side = size_px as i32;
    // SAFETY: COM call on a valid factory.
    let hbm = unsafe {
        factory
            .GetImage(
                SIZE { cx: side, cy: side },
                SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK,
            )
            .map_err(win_err("IShellItemImageFactory::GetImage"))?
    };
    let bitmap = OwnedBitmap(hbm);
    let (rgba, w, h) = bitmap_to_rgba(&bitmap)?;
    let (rgba, w, h) = util::downscale_rgba(&rgba, w, h, size_px.min(w), size_px.min(h));
    util::encode_png(&rgba, w, h)
}

/// Reads a GDI bitmap as straight RGBA, top-down.
fn bitmap_to_rgba(bmp: &OwnedBitmap) -> Result<(Vec<u8>, u32, u32)> {
    let mut info = BITMAP::default();
    // SAFETY: out-pointer sized for BITMAP.
    let n = unsafe {
        GetObjectW(
            HGDIOBJ(bmp.0.0),
            size_of::<BITMAP>() as i32,
            Some(&mut info as *mut BITMAP as *mut _),
        )
    };
    if n == 0 {
        return Err(Error::Windows {
            context: "GetObjectW",
            code: 0,
            message: "no bitmap info".into(),
        });
    }
    let w = info.bmWidth;
    let h = info.bmHeight.abs();
    if !(1..=4096).contains(&w) || !(1..=4096).contains(&h) {
        return Err(Error::Png(format!("unexpected icon bitmap size {w}x{h}")));
    }
    let mut bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h, // negative: request top-down rows
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bgra = vec![0u8; (w as usize) * (h as usize) * 4];
    // SAFETY: buffer holds h rows of w 32-bit pixels, matching the header we pass.
    let lines = unsafe {
        let hdc = GetDC(None);
        let lines = GetDIBits(
            hdc,
            bmp.0,
            0,
            h as u32,
            Some(bgra.as_mut_ptr() as *mut _),
            &mut bmi,
            DIB_RGB_COLORS,
        );
        ReleaseDC(None, hdc);
        lines
    };
    if lines != h {
        return Err(Error::Windows {
            context: "GetDIBits",
            code: 0,
            message: format!("copied {lines} of {h} rows"),
        });
    }
    Ok((util::bgra_to_rgba(&bgra), w as u32, h as u32))
}
