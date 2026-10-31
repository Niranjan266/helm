//! Rendering the pairing link as a QR code (console text and SVG file).

use qrcode::render::{svg, unicode};
use qrcode::{EcLevel, QrCode};

fn code(text: &str) -> anyhow::Result<QrCode> {
    Ok(QrCode::with_error_correction_level(text, EcLevel::M)?)
}

/// QR made of half-block characters, drawn for a dark console background (light modules
/// are printed as blocks, dark modules as spaces), with a quiet zone.
pub fn to_unicode(text: &str) -> anyhow::Result<String> {
    Ok(code(text)?
        .render::<unicode::Dense1x2>()
        .dark_color(unicode::Dense1x2::Light)
        .light_color(unicode::Dense1x2::Dark)
        .quiet_zone(true)
        .build())
}

/// Standalone SVG (black on white).
pub fn to_svg(text: &str) -> anyhow::Result<String> {
    Ok(code(text)?
        .render::<svg::Color>()
        .min_dimensions(320, 320)
        .dark_color(svg::Color("#000000"))
        .light_color(svg::Color("#ffffff"))
        .quiet_zone(true)
        .build())
}

#[cfg(test)]
mod tests {
    #[test]
    fn renders() {
        let uri = "helm://pair?v=1&id=x&n=PC&a=192.168.1.2&p=47820&fp=00&t=abc";
        assert!(super::to_svg(uri).unwrap().contains("<svg"));
        assert!(super::to_unicode(uri).unwrap().lines().count() > 10);
    }
}
