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
}
