//! Small PNG thumbnails for clipboard-history images (pure: `png` crate decode, box filter,
//! RGBA encode).

/// Longest side of a thumbnail.
pub const THUMB_MAX: u32 = 128;

/// `(thumbnail png, full width, full height)`; `None` if `png` cannot be decoded.
pub fn thumbnail(png_bytes: &[u8], max_side: u32) -> Option<(Vec<u8>, u32, u32)> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(png_bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let (w, h) = {
        let info = reader.info();
        (info.width, info.height)
    };
    // 64 Mpx is far beyond any screenshot; refuse instead of allocating gigabytes.
    if w == 0 || h == 0 || u64::from(w) * u64::from(h) > 64_000_000 {
        return None;
    }
    let mut buf = vec![0u8; reader.output_buffer_size()?];
    let frame = reader.next_frame(&mut buf).ok()?;
    let data = &buf[..frame.buffer_size()];
    let channels = match frame.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => return None,
    };
    let stride = frame.line_size;
    let px = |x: u32, y: u32| -> [u8; 4] {
        let i = y as usize * stride + x as usize * channels;
        match channels {
            1 => [data[i], data[i], data[i], 255],
            2 => [data[i], data[i], data[i], data[i + 1]],
            3 => [data[i], data[i + 1], data[i + 2], 255],
            _ => [data[i], data[i + 1], data[i + 2], data[i + 3]],
        }
    };
    let scale = (f64::from(max_side.max(1)) / f64::from(w.max(h))).min(1.0);
    let tw = ((f64::from(w) * scale).round() as u32).max(1);
    let th = ((f64::from(h) * scale).round() as u32).max(1);
    let mut out = Vec::with_capacity((tw * th * 4) as usize);
    for ty in 0..th {
        let y0 = (u64::from(ty) * u64::from(h) / u64::from(th)) as u32;
        let y1 = ((u64::from(ty + 1) * u64::from(h) / u64::from(th)) as u32).max(y0 + 1).min(h);
        for tx in 0..tw {
            let x0 = (u64::from(tx) * u64::from(w) / u64::from(tw)) as u32;
            let x1 = ((u64::from(tx + 1) * u64::from(w) / u64::from(tw)) as u32).max(x0 + 1).min(w);
            let mut sum = [0u64; 4];
            // Sample at most 8x8 source pixels per output pixel (fast enough, smooth enough).
            let sx = ((x1 - x0) / 8).max(1);
            let sy = ((y1 - y0) / 8).max(1);
            let mut n = 0u64;
            let mut y = y0;
            while y < y1 {
                let mut x = x0;
                while x < x1 {
                    let p = px(x, y);
                    for c in 0..4 {
                        sum[c] += u64::from(p[c]);
                    }
                    n += 1;
                    x += sx;
                }
                y += sy;
            }
            for s in sum {
                out.push((s / n.max(1)) as u8);
            }
        }
    }
    let mut encoded = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut encoded, tw, th);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().ok()?;
        writer.write_image_data(&out).ok()?;
    }
    Some((encoded, w, h))
}

#[cfg(test)]
pub(crate) fn test_png(w: u32, h: u32) -> Vec<u8> {
    let mut v = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut v, w, h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().unwrap();
        let data: Vec<u8> = (0..w * h * 3).map(|i| (i % 251) as u8).collect();
        wr.write_image_data(&data).unwrap();
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size_of_png(bytes: &[u8]) -> (u32, u32) {
        let r = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().unwrap();
        (r.info().width, r.info().height)
    }

    #[test]
    fn scales_longest_side_and_keeps_aspect() {
        let (t, w, h) = thumbnail(&test_png(400, 100), THUMB_MAX).unwrap();
        assert_eq!((w, h), (400, 100));
        assert_eq!(size_of_png(&t), (128, 32));
        let (t, ..) = thumbnail(&test_png(10, 20), THUMB_MAX).unwrap();
        assert_eq!(size_of_png(&t), (10, 20), "small images are not enlarged");
        let (t, ..) = thumbnail(crate::platform::fake::TINY_PNG, THUMB_MAX).unwrap();
        assert_eq!(size_of_png(&t), (1, 1));
    }

    #[test]
    fn garbage_is_rejected() {
        assert!(thumbnail(b"not a png", THUMB_MAX).is_none());
    }
}
