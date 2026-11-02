//! Pixel-format helpers (pure Rust, no Windows calls).

/// Convert tightly packed 32-bit BGRA pixels (the GDI / DXGI `B8G8R8A8` layout) to RGBA.
///
/// The alpha channel is forced to 255: GDI leaves the 4th byte of screen pixels undefined
/// (usually 0), and a screenshot is always opaque. Any trailing bytes that do not form a whole
/// pixel are ignored.
pub fn bgra_to_rgba(bgra: &[u8]) -> Vec<u8> {
    let mut out = bgra[..bgra.len() / 4 * 4].to_vec();
    bgra_to_rgba_in_place(&mut out);
    out
}

/// In-place variant of [`bgra_to_rgba`]: swaps B and R and sets A = 255 for every pixel.
pub fn bgra_to_rgba_in_place(buf: &mut [u8]) {
    for px in buf.chunks_exact_mut(4) {
        px.swap(0, 2);
        px[3] = 255;
    }
}

/// Copy `rows` rows of `row_bytes` bytes from a pitched source into a tightly packed `dst`.
///
/// Used when reading GPU / DIB memory whose row pitch is larger than the visible row.
pub fn copy_pitched(src: &[u8], src_pitch: usize, row_bytes: usize, rows: usize, dst: &mut Vec<u8>) {
    dst.reserve(row_bytes * rows);
    for r in 0..rows {
        let start = r * src_pitch;
        dst.extend_from_slice(&src[start..start + row_bytes]);
    }
}

/// Size of a thumbnail of a `w × h` image whose longer side is at most `max_side`: aspect
/// preserved, never upscaled, each side at least 1. `(0, 0)` for an empty image.
pub fn thumb_size(w: u32, h: u32, max_side: u32) -> (u32, u32) {
    if w == 0 || h == 0 || max_side == 0 {
        return (0, 0);
    }
    let longer = w.max(h);
    if longer <= max_side {
        return (w, h);
    }
    let scale = |v: u32| ((v as u64 * max_side as u64 + longer as u64 / 2) / longer as u64).max(1) as u32;
    (scale(w), scale(h))
}

/// Box-filter downscale of tightly packed RGBA (`src_w × src_h`) to `dst_w × dst_h`
/// (each destination pixel is the average of the source pixels it covers). Equal sizes copy;
/// a larger destination degrades to nearest-neighbour.
pub fn downscale_rgba(src: &[u8], src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> Vec<u8> {
    let (sw, sh, dw, dh) = (src_w as usize, src_h as usize, dst_w as usize, dst_h as usize);
    assert!(src.len() >= sw * sh * 4, "source buffer too small");
    if dw == 0 || dh == 0 {
        return Vec::new();
    }
    if dw == sw && dh == sh {
        return src[..sw * sh * 4].to_vec();
    }
    let mut out = vec![0u8; dw * dh * 4];
    for dy in 0..dh {
        let y0 = dy * sh / dh;
        let y1 = ((dy + 1) * sh / dh).max(y0 + 1).min(sh);
        for dx in 0..dw {
            let x0 = dx * sw / dw;
            let x1 = ((dx + 1) * sw / dw).max(x0 + 1).min(sw);
            let mut acc = [0u64; 4];
            for y in y0..y1 {
                let row = &src[(y * sw + x0) * 4..(y * sw + x1) * 4];
                for px in row.chunks_exact(4) {
                    for c in 0..4 {
                        acc[c] += px[c] as u64;
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u64;
            let o = (dy * dw + dx) * 4;
            for c in 0..4 {
                out[o + c] = ((acc[c] + n / 2) / n) as u8;
            }
        }
    }
    out
}

/// Crop tightly packed RGBA (`w` wide) to the rectangle `(x, y, cw, ch)` (clamped to the image).
pub fn crop_rgba(src: &[u8], w: u32, h: u32, x: u32, y: u32, cw: u32, ch: u32) -> (Vec<u8>, u32, u32) {
    let x = x.min(w);
    let y = y.min(h);
    let cw = cw.min(w - x);
    let ch = ch.min(h - y);
    let mut out = Vec::with_capacity(cw as usize * ch as usize * 4);
    for r in y..y + ch {
        let start = (r as usize * w as usize + x as usize) * 4;
        out.extend_from_slice(&src[start..start + cw as usize * 4]);
    }
    (out, cw, ch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swaps_channels_and_forces_opaque_alpha() {
        let bgra = [1u8, 2, 3, 0, 10, 20, 30, 40];
        assert_eq!(bgra_to_rgba(&bgra), vec![3, 2, 1, 255, 30, 20, 10, 255]);
    }

    #[test]
    fn ignores_partial_trailing_pixel() {
        let bgra = [1u8, 2, 3, 4, 9, 9];
        assert_eq!(bgra_to_rgba(&bgra), vec![3, 2, 1, 255]);
    }

    #[test]
    fn empty_input() {
        assert!(bgra_to_rgba(&[]).is_empty());
    }

    #[test]
    fn in_place_matches_copying_version() {
        let src: Vec<u8> = (0..64u8).collect();
        let mut inplace = src.clone();
        bgra_to_rgba_in_place(&mut inplace);
        assert_eq!(inplace, bgra_to_rgba(&src));
    }

    #[test]
    fn copy_pitched_drops_padding() {
        // 2 rows of 3 visible bytes, pitch 5.
        let src = [1u8, 2, 3, 0, 0, 4, 5, 6, 0, 0];
        let mut dst = Vec::new();
        copy_pitched(&src, 5, 3, 2, &mut dst);
        assert_eq!(dst, vec![1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn thumb_size_keeps_aspect_and_never_upscales() {
        assert_eq!(thumb_size(1920, 1080, 256), (256, 144));
        assert_eq!(thumb_size(1080, 1920, 256), (144, 256));
        assert_eq!(thumb_size(200, 100, 256), (200, 100));
        assert_eq!(thumb_size(5000, 3, 100), (100, 1));
        assert_eq!(thumb_size(0, 100, 256), (0, 0));
        assert_eq!(thumb_size(801, 601, 400), (400, 300));
    }

    #[test]
    fn downscale_averages_blocks() {
        // 4x2 → 2x1: left block (0, 100, 0, 100) avg 50, right block all 200.
        let mut src = Vec::new();
        for v in [0u8, 100, 200, 200, 100, 0, 200, 200] {
            src.extend_from_slice(&[v, v, v, 255]);
        }
        let out = downscale_rgba(&src, 4, 2, 2, 1);
        assert_eq!(out, vec![50, 50, 50, 255, 200, 200, 200, 255]);
        assert_eq!(downscale_rgba(&src, 4, 2, 4, 2), src);
        assert!(downscale_rgba(&src, 4, 2, 0, 1).is_empty());
        // Non-integer ratio: 3x1 → 2x1 covers [0] and [1..3].
        let src3: Vec<u8> = [10u8, 20, 40].iter().flat_map(|&v| [v, 0, 0, 255]).collect();
        assert_eq!(downscale_rgba(&src3, 3, 1, 2, 1), vec![10, 0, 0, 255, 30, 0, 0, 255]);
    }

    #[test]
    fn crop_clamps_to_the_image() {
        let src: Vec<u8> = (0..16u8).flat_map(|v| [v, 0, 0, 255]).collect(); // 4x4, red = index
        let (c, w, h) = crop_rgba(&src, 4, 4, 1, 2, 2, 5);
        assert_eq!((w, h), (2, 2));
        assert_eq!(c.chunks(4).map(|p| p[0]).collect::<Vec<_>>(), vec![9, 10, 13, 14]);
        let (c, w, h) = crop_rgba(&src, 4, 4, 9, 9, 2, 2);
        assert_eq!((w, h, c.len()), (0, 0, 0));
    }
}
