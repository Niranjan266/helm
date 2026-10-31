//! Live, read-only tests against the real desktop (Part A) plus encoder enumeration.
//! They only read the screen; nothing is clicked, typed or moved.

#![cfg(windows)]

use helm_win_capture::{
    Error, list_h264_encoders, monitors, screenshot_png, screenshot_primary_rgba, virtual_desktop_rect,
};

fn decode_png(bytes: &[u8]) -> (u32, u32, png::ColorType, Vec<u8>) {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().expect("valid PNG header");
    let mut buf = vec![0; reader.output_buffer_size().expect("size")];
    let info = reader.next_frame(&mut buf).expect("valid PNG data");
    buf.truncate(info.buffer_size());
    (info.width, info.height, info.color_type, buf)
}

#[test]
fn monitors_are_listed_with_one_primary() {
    let ms = monitors();
    assert!(!ms.is_empty(), "no monitors");
    assert_eq!(ms.iter().filter(|m| m.is_primary).count(), 1, "{ms:?}");
    assert!(ms[0].is_primary, "primary is listed first");
    let p = &ms[0];
    assert_eq!((p.x, p.y), (0, 0), "primary monitor is at the virtual-desktop origin");
    for m in &ms {
        assert!(m.width > 0 && m.height > 0, "{m:?}");
        assert!(m.scale >= 1.0 && m.scale <= 5.0, "{m:?}");
        assert!(m.id.starts_with(r"\\.\"), "{m:?}");
        assert!(!m.name.is_empty());
    }
}

/// Monitor sizes must be physical pixels even though this test process never declared DPI
/// awareness (the library switches the thread to per-monitor-v2 itself).
#[test]
fn sizes_are_physical_without_process_dpi_awareness() {
    let m = monitors().into_iter().find(|m| m.is_primary).unwrap();
    let img = screenshot_primary_rgba().unwrap();
    assert_eq!((img.width, img.height), (m.width, m.height));
    let (_, _, vw, vh) = virtual_desktop_rect();
    assert!(vw >= m.width && vh >= m.height);
}

#[test]
fn primary_screenshot_decodes_as_png_with_monitor_size() {
    let m = monitors().into_iter().find(|m| m.is_primary).unwrap();
    let png_bytes = screenshot_png(Some(&m.id)).unwrap();
    assert_eq!(&png_bytes[..8], b"\x89PNG\r\n\x1a\n");
    let (w, h, color, pixels) = decode_png(&png_bytes);
    assert_eq!((w, h), (m.width, m.height));
    assert_eq!(color, png::ColorType::Rgba);
    assert!(pixels.chunks_exact(4).all(|p| p[3] == 255), "screenshot is opaque");
    // A real desktop is never a single flat colour.
    let first = &pixels[..4];
    assert!(pixels.chunks_exact(4).any(|p| p != first), "screenshot is uniform");
}

#[test]
fn virtual_desktop_screenshot_has_virtual_desktop_size() {
    let (_, _, vw, vh) = virtual_desktop_rect();
    let (w, h, _, _) = decode_png(&screenshot_png(None).unwrap());
    assert_eq!((w, h), (vw, vh));
}

#[test]
fn unknown_monitor_is_an_error() {
    assert!(matches!(screenshot_png(Some(r"\\.\DISPLAY999")), Err(Error::MonitorNotFound(_))));
}

#[test]
fn an_h264_encoder_is_installed() {
    let encs = list_h264_encoders().unwrap();
    assert!(!encs.is_empty(), "Windows always ships the software H.264 MFT");
    assert!(encs.iter().any(|e| !e.hardware), "software fallback present: {encs:?}");
}
