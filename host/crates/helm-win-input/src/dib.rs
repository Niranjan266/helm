//! Device-independent bitmap (CF_DIB / CF_DIBV5 clipboard data) ↔ PNG conversion.
//!
//! Written from the `BITMAPINFOHEADER`, `BITMAPV5HEADER` and "Bitmap Storage" pages on
//! Microsoft Learn. A packed DIB is: header, optional colour masks, optional colour table,
//! then the pixel rows (each padded to 4 bytes; bottom-up unless the height is negative).

use crate::error::{Error, Result};

/// Refuse images above this many pixels (protects against absurd headers).
const MAX_PIXELS: u64 = 120_000_000;

const BI_RGB: u32 = 0;
const BI_BITFIELDS: u32 = 3;
const BI_PNG: u32 = 5;
const BI_ALPHABITFIELDS: u32 = 6;

/// An 8-bit RGBA image, rows top to bottom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

fn err(msg: impl Into<String>) -> Error {
    Error::Image(msg.into())
}

fn u16_at(b: &[u8], off: usize) -> Result<u16> {
    b.get(off..off + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or_else(|| err("DIB truncated"))
}

fn u32_at(b: &[u8], off: usize) -> Result<u32> {
    b.get(off..off + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| err("DIB truncated"))
}

/// Extracts a masked channel and scales it to 0..=255.
fn channel(px: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let bits = (mask >> shift).count_ones();
    let v = (px & mask) >> shift;
    if bits >= 8 {
        (v >> (bits - 8)) as u8
    } else {
        let max = (1u32 << bits) - 1;
        ((v * 255 + max / 2) / max) as u8
    }
}

/// Decodes a packed DIB (the bytes of a CF_DIB or CF_DIBV5 clipboard item).
///
/// Supports 1/4/8-bit palettes, 16-bit (555 or bit fields), 24-bit, and 32-bit (BI_RGB or
/// bit fields, with alpha when present), plus BI_PNG passthrough via [`dib_to_png`].
pub fn decode_dib(dib: &[u8]) -> Result<RgbaImage> {
    let header_size = u32_at(dib, 0)? as usize;
    if header_size < 40 {
        return Err(err(format!("unsupported DIB header size {header_size}")));
    }
    let width = u32_at(dib, 4)? as i32;
    let height = u32_at(dib, 8)? as i32;
    let bpp = u16_at(dib, 14)?;
    let compression = u32_at(dib, 16)?;
    let clr_used = u32_at(dib, 32)?;
    if width <= 0 || height == 0 || height == i32::MIN {
        return Err(err(format!("bad DIB size {width}x{height}")));
    }
    let top_down = height < 0;
    let (w, h) = (width as u32, height.unsigned_abs());
    if w as u64 * h as u64 > MAX_PIXELS {
        return Err(err("DIB too large"));
    }

    // Colour masks: inside the header for V2+ headers, otherwise right after a 40-byte
    // header when the compression says bit fields.
    let mut extra = 0usize;
    let masks: Option<[u32; 4]> = match compression {
        BI_BITFIELDS | BI_ALPHABITFIELDS => {
            let base = 40;
            let with_alpha = compression == BI_ALPHABITFIELDS || header_size >= 56;
            if header_size == 40 {
                extra = if compression == BI_ALPHABITFIELDS {
                    16
                } else {
                    12
                };
            }
            let a = if with_alpha {
                u32_at(dib, base + 12)?
            } else {
                0
            };
            Some([
                u32_at(dib, base)?,
                u32_at(dib, base + 4)?,
                u32_at(dib, base + 8)?,
                a,
            ])
        }
        BI_RGB => None,
        other => return Err(err(format!("unsupported DIB compression {other}"))),
    };

    let palette_len = if bpp <= 8 {
        let max = 1usize << bpp;
        match clr_used as usize {
            0 => max,
            n if n <= max => n,
            _ => return Err(err("DIB colour table too large")),
        }
    } else {
        0
    };
    let palette_off = header_size + extra;
    let palette: Vec<[u8; 4]> = (0..palette_len)
        .map(|i| {
            let o = palette_off + i * 4;
            dib.get(o..o + 4)
                .map(|q| [q[2], q[1], q[0], 255])
                .ok_or_else(|| err("DIB colour table truncated"))
        })
        .collect::<Result<_>>()?;
    let pixel_off = palette_off + palette_len * 4;

    let stride = (w as usize)
        .checked_mul(bpp as usize)
        .map(|bits| bits.div_ceil(32) * 4)
        .ok_or_else(|| err("DIB too large"))?;
    let needed = stride
        .checked_mul(h as usize)
        .and_then(|n| n.checked_add(pixel_off))
        .ok_or_else(|| err("DIB too large"))?;
    if dib.len() < needed {
        return Err(err(format!(
            "DIB truncated: {} of {} bytes",
            dib.len(),
            needed
        )));
    }

    let mut out = vec![0u8; w as usize * h as usize * 4];
    // For 32-bit BI_RGB the fourth byte is officially unused but often carries alpha.
    let mut alpha_seen = false;
    for row in 0..h as usize {
        let src_row = if top_down { row } else { h as usize - 1 - row };
        let src = &dib[pixel_off + src_row * stride..pixel_off + (src_row + 1) * stride];
        let dst = &mut out[row * w as usize * 4..(row + 1) * w as usize * 4];
        for x in 0..w as usize {
            let rgba: [u8; 4] = match bpp {
                1 | 4 | 8 => {
                    let bit = x * bpp as usize;
                    let byte = src[bit / 8];
                    let shift = 8 - bpp as usize - (bit % 8);
                    let idx = (byte >> shift) & ((1u16 << bpp) - 1) as u8;
                    *palette
                        .get(idx as usize)
                        .ok_or_else(|| err("DIB palette index out of range"))?
                }
                16 => {
                    let px = u16::from_le_bytes([src[x * 2], src[x * 2 + 1]]) as u32;
                    let [r, g, b, a] = masks.unwrap_or([0x7C00, 0x03E0, 0x001F, 0]);
                    let alpha = if a != 0 { channel(px, a) } else { 255 };
                    [channel(px, r), channel(px, g), channel(px, b), alpha]
                }
                24 => [src[x * 3 + 2], src[x * 3 + 1], src[x * 3], 255],
                32 => {
                    let p = &src[x * 4..x * 4 + 4];
                    let px = u32::from_le_bytes([p[0], p[1], p[2], p[3]]);
                    match masks {
                        Some([r, g, b, a]) => {
                            let alpha = channel(px, a);
                            alpha_seen |= a != 0 && alpha != 0;
                            [
                                channel(px, r),
                                channel(px, g),
                                channel(px, b),
                                if a != 0 { alpha } else { 255 },
                            ]
                        }
                        None => {
                            alpha_seen |= p[3] != 0;
                            [p[2], p[1], p[0], p[3]]
                        }
                    }
                }
                other => return Err(err(format!("unsupported DIB bit depth {other}"))),
            };
            dst[x * 4..x * 4 + 4].copy_from_slice(&rgba);
        }
    }
    // An all-zero alpha channel means "no alpha", not "fully transparent".
    let alpha_channel_used = bpp == 32 && masks.is_none_or(|m| m[3] != 0);
    if alpha_channel_used && !alpha_seen {
        for px in out.chunks_exact_mut(4) {
            px[3] = 255;
        }
    }
    Ok(RgbaImage {
        width: w,
        height: h,
        pixels: out,
    })
}

/// Encodes a top-down RGBA image as a packed DIB: `BITMAPINFOHEADER`, 32-bit BI_RGB,
/// bottom-up rows, BGRA byte order (alpha straight, in the fourth byte).
pub fn encode_dib(img: &RgbaImage) -> Result<Vec<u8>> {
    check_len(img)?;
    let (w, h) = (img.width as usize, img.height as usize);
    let image_size = w * h * 4;
    let mut out = Vec::with_capacity(40 + image_size);
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(img.width as i32).to_le_bytes());
    out.extend_from_slice(&(img.height as i32).to_le_bytes()); // positive = bottom-up
    out.extend_from_slice(&1u16.to_le_bytes()); // planes
    out.extend_from_slice(&32u16.to_le_bytes()); // bit count
    out.extend_from_slice(&BI_RGB.to_le_bytes());
    out.extend_from_slice(&(image_size as u32).to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes()); // 72 DPI in pixels per metre
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // colours used
    out.extend_from_slice(&0u32.to_le_bytes()); // colours important
    for row in (0..h).rev() {
        for px in img.pixels[row * w * 4..(row + 1) * w * 4].chunks_exact(4) {
            out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
    }
    Ok(out)
}

fn check_len(img: &RgbaImage) -> Result<()> {
    if img.width == 0
        || img.height == 0
        || img.width > i32::MAX as u32
        || img.height > i32::MAX as u32
    {
        return Err(err("empty or oversized image"));
    }
    if img.width as u64 * img.height as u64 > MAX_PIXELS {
        return Err(err("image too large"));
    }
    if img.pixels.len() as u64 != img.width as u64 * img.height as u64 * 4 {
        return Err(err("pixel buffer size does not match dimensions"));
    }
    Ok(())
}

/// Encodes an RGBA image as PNG.
pub fn encode_png(img: &RgbaImage) -> Result<Vec<u8>> {
    check_len(img)?;
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, img.width, img.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().map_err(|e| err(e.to_string()))?;
        writer
            .write_image_data(&img.pixels)
            .map_err(|e| err(e.to_string()))?;
        writer.finish().map_err(|e| err(e.to_string()))?;
    }
    Ok(out)
}

/// Decodes any PNG (palette, grey, 16-bit, with or without alpha) into 8-bit RGBA.
pub fn decode_png(bytes: &[u8]) -> Result<RgbaImage> {
    let limits = png::Limits {
        bytes: 512 * 1024 * 1024,
    };
    let mut dec = png::Decoder::new_with_limits(std::io::Cursor::new(bytes), limits);
    dec.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = dec.read_info().map_err(|e| err(e.to_string()))?;
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| err("PNG too large"))?;
    let mut buf = vec![0u8; size];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| err(e.to_string()))?;
    if info.bit_depth != png::BitDepth::Eight {
        return Err(err("unexpected PNG bit depth after normalising"));
    }
    let (w, h) = (info.width as usize, info.height as usize);
    let mut pixels = Vec::with_capacity(w * h * 4);
    for row in buf[..h * info.line_size].chunks_exact(info.line_size) {
        let row = &row[..w * info.color_type.samples()];
        match info.color_type {
            png::ColorType::Rgba => pixels.extend_from_slice(row),
            png::ColorType::Rgb => row
                .chunks_exact(3)
                .for_each(|p| pixels.extend_from_slice(&[p[0], p[1], p[2], 255])),
            png::ColorType::Grayscale => row
                .iter()
                .for_each(|&g| pixels.extend_from_slice(&[g, g, g, 255])),
            png::ColorType::GrayscaleAlpha => row
                .chunks_exact(2)
                .for_each(|p| pixels.extend_from_slice(&[p[0], p[0], p[0], p[1]])),
            png::ColorType::Indexed => return Err(err("PNG palette was not expanded")),
        }
    }
    let img = RgbaImage {
        width: info.width,
        height: info.height,
        pixels,
    };
    check_len(&img)?;
    Ok(img)
}

/// Converts CF_DIB / CF_DIBV5 bytes to PNG bytes.
pub fn dib_to_png(dib: &[u8]) -> Result<Vec<u8>> {
    let header_size = u32_at(dib, 0)? as usize;
    if header_size >= 40 && u32_at(dib, 16)? == BI_PNG {
        // The pixel data is already a PNG.
        let data = &dib[header_size.min(dib.len())..];
        if data.starts_with(PNG_SIGNATURE) {
            return Ok(data.to_vec());
        }
        return Err(err("BI_PNG DIB without PNG data"));
    }
    encode_png(&decode_dib(dib)?)
}

/// Converts PNG bytes to CF_DIB bytes (see [`encode_dib`]).
pub fn png_to_dib(png_bytes: &[u8]) -> Result<Vec<u8>> {
    encode_dib(&decode_png(png_bytes)?)
}

/// The eight-byte PNG file signature.
pub const PNG_SIGNATURE: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic(w: u32, h: u32) -> RgbaImage {
        let mut pixels = Vec::new();
        for y in 0..h {
            for x in 0..w {
                pixels.extend_from_slice(&[
                    (x * 37 % 256) as u8,
                    (y * 59 % 256) as u8,
                    ((x + y) * 13 % 256) as u8,
                    ((x * 7 + y * 3) % 200 + 30) as u8,
                ]);
            }
        }
        RgbaImage {
            width: w,
            height: h,
            pixels,
        }
    }

    /// Builds a 40-byte-header DIB by hand.
    fn header(w: i32, h: i32, bpp: u16, compression: u32, clr_used: u32) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&40u32.to_le_bytes());
        v.extend_from_slice(&w.to_le_bytes());
        v.extend_from_slice(&h.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&bpp.to_le_bytes());
        v.extend_from_slice(&compression.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(&0i32.to_le_bytes());
        v.extend_from_slice(&0i32.to_le_bytes());
        v.extend_from_slice(&clr_used.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v
    }

    #[test]
    fn png_dib_png_round_trip() {
        for (w, h) in [(1, 1), (3, 2), (17, 5), (64, 33)] {
            let img = synthetic(w, h);
            let png1 = encode_png(&img).unwrap();
            assert!(png1.starts_with(PNG_SIGNATURE));
            let dib = png_to_dib(&png1).unwrap();
            assert_eq!(dib.len(), 40 + (w * h * 4) as usize);
            let png2 = dib_to_png(&dib).unwrap();
            assert_eq!(decode_png(&png2).unwrap(), img, "{w}x{h}");
        }
    }

    #[test]
    fn dib_is_bottom_up_bgra() {
        let img = RgbaImage {
            width: 1,
            height: 2,
            pixels: vec![1, 2, 3, 4, 5, 6, 7, 8],
        };
        let dib = encode_dib(&img).unwrap();
        assert_eq!(&dib[40..], &[7, 6, 5, 8, 3, 2, 1, 4]);
        assert_eq!(decode_dib(&dib).unwrap(), img);
    }

    #[test]
    fn decode_24bit_bottom_up_with_padding() {
        // 2x2, rows of 6 bytes padded to 8; bottom row first.
        let mut dib = header(2, 2, 24, BI_RGB, 0);
        dib.extend_from_slice(&[0, 0, 255, 0, 255, 0, 0, 0]); // bottom: red, green
        dib.extend_from_slice(&[255, 0, 0, 255, 255, 255, 0, 0]); // top: blue, white
        let img = decode_dib(&dib).unwrap();
        assert_eq!(
            img.pixels,
            vec![
                0, 0, 255, 255, 255, 255, 255, 255, 255, 0, 0, 255, 0, 255, 0, 255
            ]
        );
    }

    #[test]
    fn decode_top_down_32bit_without_alpha_is_opaque() {
        let mut dib = header(2, -1, 32, BI_RGB, 0);
        dib.extend_from_slice(&[10, 20, 30, 0, 40, 50, 60, 0]);
        let img = decode_dib(&dib).unwrap();
        assert_eq!(img.pixels, vec![30, 20, 10, 255, 60, 50, 40, 255]);
    }

    #[test]
    fn decode_32bit_bitfields_and_565() {
        let mut dib = header(1, 1, 32, BI_BITFIELDS, 0);
        for m in [0x0000_00FFu32, 0x0000_FF00, 0x00FF_0000] {
            dib.extend_from_slice(&m.to_le_bytes());
        }
        dib.extend_from_slice(&[1, 2, 3, 99]); // R in low byte with these masks
        assert_eq!(decode_dib(&dib).unwrap().pixels, vec![1, 2, 3, 255]);

        let mut dib = header(2, 1, 16, BI_BITFIELDS, 0);
        for m in [0xF800u32, 0x07E0, 0x001F] {
            dib.extend_from_slice(&m.to_le_bytes());
        }
        dib.extend_from_slice(&0xF800u16.to_le_bytes()); // pure red
        dib.extend_from_slice(&0x07FFu16.to_le_bytes()); // cyan
        assert_eq!(
            decode_dib(&dib).unwrap().pixels,
            vec![255, 0, 0, 255, 0, 255, 255, 255]
        );
    }

    #[test]
    fn decode_v5_header_with_alpha_mask() {
        let mut dib = Vec::new();
        dib.extend_from_slice(&124u32.to_le_bytes());
        dib.extend_from_slice(&1i32.to_le_bytes());
        dib.extend_from_slice(&1i32.to_le_bytes());
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&32u16.to_le_bytes());
        dib.extend_from_slice(&BI_BITFIELDS.to_le_bytes());
        dib.resize(40, 0);
        for m in [0x00FF_0000u32, 0x0000_FF00, 0x0000_00FF, 0xFF00_0000] {
            dib.extend_from_slice(&m.to_le_bytes());
        }
        dib.resize(124, 0);
        dib.extend_from_slice(&[30, 20, 10, 128]);
        assert_eq!(decode_dib(&dib).unwrap().pixels, vec![10, 20, 30, 128]);
    }

    #[test]
    fn decode_palettes() {
        // 8-bit, 2 colours used, 3x1 -> row of 3 bytes padded to 4.
        let mut dib = header(3, 1, 8, BI_RGB, 2);
        dib.extend_from_slice(&[0, 0, 0, 0, 255, 255, 255, 0]);
        dib.extend_from_slice(&[1, 0, 1, 0]);
        assert_eq!(
            decode_dib(&dib).unwrap().pixels,
            vec![255, 255, 255, 255, 0, 0, 0, 255, 255, 255, 255, 255]
        );
        // 1-bit, 10x1: bits 1010000011 -> padded to 4 bytes.
        let mut dib = header(10, 1, 1, BI_RGB, 0);
        dib.extend_from_slice(&[0, 0, 0, 0, 0, 0, 255, 0]); // black, red
        dib.extend_from_slice(&[0b1010_0000, 0b1100_0000, 0, 0]);
        let px = decode_dib(&dib).unwrap().pixels;
        let reds: Vec<bool> = px.chunks(4).map(|p| p[0] == 255).collect();
        assert_eq!(
            reds,
            [
                true, false, true, false, false, false, false, false, true, true
            ]
        );
        // 4-bit index beyond the colour table is an error.
        let mut dib = header(1, 1, 4, BI_RGB, 1);
        dib.extend_from_slice(&[0, 0, 0, 0]);
        dib.extend_from_slice(&[0x50, 0, 0, 0]);
        assert!(decode_dib(&dib).is_err());
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode_dib(&[]).is_err());
        assert!(decode_dib(&[1, 2, 3, 4, 5]).is_err());
        let mut dib = header(100, 100, 32, BI_RGB, 0);
        dib.extend_from_slice(&[0; 16]);
        assert!(decode_dib(&dib).is_err(), "truncated pixel data");
        let dib = header(70000, 70000, 32, BI_RGB, 0);
        assert!(decode_dib(&dib).is_err(), "too large");
        let mut dib = header(1, 1, 32, 1 /* BI_RLE8 */, 0);
        dib.extend_from_slice(&[0; 4]);
        assert!(decode_dib(&dib).is_err());
        assert!(decode_png(b"not a png").is_err());
        assert!(
            encode_png(&RgbaImage {
                width: 2,
                height: 2,
                pixels: vec![0; 3]
            })
            .is_err()
        );
    }

    #[test]
    fn bi_png_passthrough() {
        let png_bytes = encode_png(&synthetic(2, 2)).unwrap();
        let mut dib = header(2, 2, 0, BI_PNG, 0);
        dib.extend_from_slice(&png_bytes);
        assert_eq!(dib_to_png(&dib).unwrap(), png_bytes);
    }

    #[test]
    fn grey_and_rgb_pngs_become_rgba() {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, 2, 1);
            enc.set_color(png::ColorType::Grayscale);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc.write_header().unwrap();
            w.write_image_data(&[0, 200]).unwrap();
            w.finish().unwrap();
        }
        assert_eq!(
            decode_png(&out).unwrap().pixels,
            vec![0, 0, 0, 255, 200, 200, 200, 255]
        );
    }
}
