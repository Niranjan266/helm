//! Mouse pointer for the stream overlay (PROTOCOL §10, frames `0x04` / `0x05`):
//! `GetCursorInfo` for position / visibility / image handle, `GetIconInfo` + `GetDIBits`
//! for the image (Microsoft Learn: GetCursorInfo, CURSORINFO, GetIconInfo, ICONINFO, GetDIBits).
//!
//! Coordinates are virtual-desktop pixels in the process's DPI context; `main` makes the
//! process per-monitor-v2 aware so they are physical pixels like `helm-win-capture`.

use crate::platform::{CursorShape, CursorState, PResult, Rect};
use crate::router::RpcError;

/// Converts the cursor's AND mask and optional colour image to straight RGBA.
///
/// * `mask` – the AND mask as 32-bit BGRX rows (white = 1). For monochrome cursors it has
///   `2 * height` rows: AND on top, XOR below.
/// * `color` – the colour image (BGRA) or `None` for a monochrome cursor.
///
/// Rules: colour pixels with any alpha are used as-is (alpha cursors). Colour cursors without
/// alpha take their alpha from the mask (AND = 0 → opaque). Monochrome: AND 0 / XOR 0 = black,
/// AND 0 / XOR 1 = white, AND 1 / XOR 0 = transparent, AND 1 / XOR 1 ("invert the screen",
/// e.g. the I-beam) = opaque black with a white outline cannot be expressed in RGBA, so it is
/// drawn as half-transparent black.
pub fn cursor_rgba(mask: &[u8], color: Option<&[u8]>, width: usize, height: usize) -> Vec<u8> {
    let n = width * height;
    let mut out = vec![0u8; n * 4];
    let bit = |buf: &[u8], i: usize| buf.get(i * 4).copied().unwrap_or(0) >= 128;
    match color {
        Some(c) => {
            let has_alpha = c.chunks_exact(4).take(n).any(|p| p[3] != 0);
            for i in 0..n {
                let p = &c[i * 4..i * 4 + 4];
                let a = if has_alpha {
                    p[3]
                } else if bit(mask, i) {
                    0
                } else {
                    255
                };
                out[i * 4..i * 4 + 4].copy_from_slice(&[p[2], p[1], p[0], a]);
            }
        }
        None => {
            for i in 0..n {
                let and = bit(mask, i);
                let xor = bit(mask, i + n);
                let px = match (and, xor) {
                    (false, false) => [0, 0, 0, 255],
                    (false, true) => [255, 255, 255, 255],
                    (true, false) => [0, 0, 0, 0],
                    (true, true) => [0, 0, 0, 128],
                };
                out[i * 4..i * 4 + 4].copy_from_slice(&px);
            }
        }
    }
    out
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;

    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC,
        GetDIBits, GetObjectW, HBITMAP, HDC, ReleaseDC,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CURSOR_SHOWING, CURSORINFO, GetCursorInfo, GetIconInfo, GetWindowRect, HICON, ICONINFO,
    };

    use super::*;

    pub fn cursor() -> PResult<CursorState> {
        let mut ci = CURSORINFO {
            cbSize: std::mem::size_of::<CURSORINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: out-pointer to a correctly sized local.
        unsafe { GetCursorInfo(&mut ci) }
            .map_err(|e| RpcError::internal(format!("GetCursorInfo failed: {e}")))?;
        Ok(CursorState {
            x: ci.ptScreenPos.x,
            y: ci.ptScreenPos.y,
            visible: ci.flags.0 & CURSOR_SHOWING.0 != 0 && !ci.hCursor.is_invalid(),
            handle: ci.hCursor.0 as usize as u64,
        })
    }

    /// 32-bit top-down copy of a bitmap (`None` on failure).
    unsafe fn bits32(dc: HDC, bmp: HBITMAP) -> Option<(usize, usize, Vec<u8>)> {
        let mut info = BITMAP::default();
        // SAFETY: (caller) `bmp` is a valid bitmap; out-pointer to a local of that size.
        let got = unsafe {
            GetObjectW(
                bmp.into(),
                std::mem::size_of::<BITMAP>() as i32,
                Some(&mut info as *mut BITMAP as *mut c_void),
            )
        };
        if got == 0 || info.bmWidth <= 0 || info.bmHeight <= 0 || info.bmWidth > 1024 || info.bmHeight > 2048 {
            return None;
        }
        let (w, h) = (info.bmWidth as usize, info.bmHeight as usize);
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32), // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut buf = vec![0u8; w * h * 4];
        // SAFETY: `buf` holds exactly `h` rows of `w` 32-bit pixels as described by `bmi`.
        let lines = unsafe {
            GetDIBits(
                dc,
                bmp,
                0,
                h as u32,
                Some(buf.as_mut_ptr() as *mut c_void),
                &mut bmi,
                DIB_RGB_COLORS,
            )
        };
        (lines == h as i32).then_some((w, h, buf))
    }

    pub fn cursor_shape(handle: u64) -> PResult<Option<CursorShape>> {
        if handle == 0 {
            return Ok(None);
        }
        let mut ii = ICONINFO::default();
        // SAFETY: `handle` came from GetCursorInfo; GetIconInfo fails cleanly if it is gone.
        if unsafe { GetIconInfo(HICON(handle as usize as *mut c_void), &mut ii) }.is_err() {
            return Ok(None);
        }
        // SAFETY: screen DC, released below; bitmaps from GetIconInfo are ours to delete.
        let result = unsafe {
            let dc = GetDC(None);
            let mask = bits32(dc, ii.hbmMask);
            let color = if ii.hbmColor.is_invalid() {
                None
            } else {
                bits32(dc, ii.hbmColor)
            };
            ReleaseDC(None, dc);
            if !ii.hbmMask.is_invalid() {
                let _ = DeleteObject(ii.hbmMask.into());
            }
            if !ii.hbmColor.is_invalid() {
                let _ = DeleteObject(ii.hbmColor.into());
            }
            match (mask, color) {
                (Some((w, h, m)), Some((cw, ch, c))) if cw == w && ch == h => {
                    Some((w, h, cursor_rgba(&m, Some(&c), w, h)))
                }
                (Some((w, h2, m)), None) if h2 >= 2 => {
                    let h = h2 / 2;
                    Some((w, h, cursor_rgba(&m, None, w, h)))
                }
                (_, Some((w, h, c))) => Some((w, h, cursor_rgba(&[], Some(&c), w, h))),
                _ => None,
            }
        };
        let Some((w, h, rgba)) = result else {
            return Ok(None);
        };
        let png = helm_win_capture::encode_png(&helm_win_capture::RgbaImage {
            width: w as u32,
            height: h as u32,
            pixels: rgba,
        })
        .map_err(|e| RpcError::internal(e.to_string()))?;
        Ok(Some(CursorShape {
            width: w as u16,
            height: h as u16,
            hot_x: (ii.xHotspot as usize).min(w.saturating_sub(1)) as u16,
            hot_y: (ii.yHotspot as usize).min(h.saturating_sub(1)) as u16,
            png,
        }))
    }

    /// Window rectangle (virtual-desktop pixels), `None` if the window is gone.
    pub fn window_rect(hwnd: u64) -> Option<Rect> {
        let mut r = RECT::default();
        // SAFETY: out-pointer to a local; an invalid hwnd just fails.
        unsafe { GetWindowRect(HWND(hwnd as usize as *mut c_void), &mut r) }.ok()?;
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        (w > 0 && h > 0).then_some(Rect {
            x: r.left,
            y: r.top,
            width: w as u32,
            height: h as u32,
        })
    }
}

#[cfg(windows)]
pub use win::{cursor, cursor_shape, window_rect};

#[cfg(not(windows))]
pub fn cursor() -> PResult<CursorState> {
    Err(RpcError::unsupported("cursor is only available on Windows"))
}

#[cfg(not(windows))]
pub fn cursor_shape(_handle: u64) -> PResult<Option<CursorShape>> {
    Ok(None)
}

#[cfg(not(windows))]
pub fn window_rect(_hwnd: u64) -> Option<Rect> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monochrome_cursor_rules() {
        // 4x1 cursor: AND row then XOR row, as BGRX (white = 1).
        let w = [255u8, 255, 255, 0];
        let b = [0u8, 0, 0, 0];
        let mut mask = Vec::new();
        for px in [b, b, w, w, b, w, b, w] {
            mask.extend_from_slice(&px);
        }
        let rgba = cursor_rgba(&mask, None, 4, 1);
        assert_eq!(&rgba[0..4], &[0, 0, 0, 255]); // AND 0 XOR 0
        assert_eq!(&rgba[4..8], &[255, 255, 255, 255]); // AND 0 XOR 1
        assert_eq!(&rgba[8..12], &[0, 0, 0, 0]); // AND 1 XOR 0
        assert_eq!(&rgba[12..16], &[0, 0, 0, 128]); // AND 1 XOR 1 (invert)
    }

    #[test]
    fn colour_cursor_alpha_from_image_or_mask() {
        // BGRA with alpha: used as-is, channels swapped to RGBA.
        let color = [10u8, 20, 30, 200, 1, 2, 3, 0];
        assert_eq!(
            cursor_rgba(&[], Some(&color), 2, 1),
            vec![30, 20, 10, 200, 3, 2, 1, 0]
        );
        // No alpha anywhere: AND mask decides (white = transparent).
        let color = [10u8, 20, 30, 0, 1, 2, 3, 0];
        let mask = [0u8, 0, 0, 0, 255, 255, 255, 0];
        assert_eq!(
            cursor_rgba(&mask, Some(&color), 2, 1),
            vec![30, 20, 10, 255, 3, 2, 1, 0]
        );
    }
}
