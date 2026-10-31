//! Pure, platform-independent helpers. Everything here is unit-tested and has no side effects.

use std::path::Path;

use crate::{AppEntry, AppKind, Error, Result};

/// Smallest and largest icon edge length accepted by [`crate::app_icon_png`].
pub const MIN_ICON_PX: u32 = 16;
/// See [`MIN_ICON_PX`].
pub const MAX_ICON_PX: u32 = 256;

/// Longest app id accepted from callers (parsing names are far shorter in practice).
pub const MAX_APP_ID_LEN: usize = 2048;

/// What [`classify_target`] decided an `open_target` argument is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// An `http`, `https` or `mailto` URL (already validated).
    Url(String),
    /// An absolute local path (existence is checked by the caller).
    Path(String),
}

/// Rejects strings that contain control characters (including NUL, CR, LF, TAB).
fn has_control_chars(s: &str) -> bool {
    s.chars().any(char::is_control)
}

/// Returns the URL scheme if `s` starts with `scheme:` per RFC 3986
/// (ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )). A single letter followed by `:` is a
/// drive letter, not a scheme, so it returns `None`.
pub fn url_scheme(s: &str) -> Option<&str> {
    let colon = s.find(':')?;
    let scheme = &s[..colon];
    let mut chars = scheme.chars();
    let first = chars.next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.') {
        return None;
    }
    if scheme.len() < 2 {
        return None; // "C:" drive letter
    }
    Some(scheme)
}

/// Validates a URL for `open_target`: only `http`, `https` and `mailto` are allowed.
pub fn validate_url(url: &str) -> Result<String> {
    let url = url.trim();
    if url.is_empty() || url.len() > 8192 {
        return Err(Error::InvalidArgument("URL is empty or too long".into()));
    }
    if has_control_chars(url) || url.contains(' ') {
        return Err(Error::InvalidArgument(
            "URL contains whitespace or control characters".into(),
        ));
    }
    let scheme = url_scheme(url)
        .ok_or_else(|| Error::InvalidArgument("not a URL".into()))?
        .to_ascii_lowercase();
    let rest = &url[scheme.len() + 1..];
    match scheme.as_str() {
        "http" | "https" => {
            let after = rest
                .strip_prefix("//")
                .ok_or_else(|| Error::InvalidArgument("http(s) URL must contain //".into()))?;
            let authority = after.split(['/', '?', '#']).next().unwrap_or("");
            // Strip optional userinfo, then port.
            let host_port = authority.rsplit('@').next().unwrap_or("");
            let host = if host_port.starts_with('[') {
                host_port
                    .split(']')
                    .next()
                    .unwrap_or("")
                    .trim_start_matches('[')
            } else {
                host_port.split(':').next().unwrap_or("")
            };
            if host.is_empty() {
                return Err(Error::InvalidArgument("URL has no host".into()));
            }
            if authority.contains('\\') {
                return Err(Error::InvalidArgument(
                    "URL host contains a backslash".into(),
                ));
            }
        }
        "mailto" => {
            if rest.is_empty() {
                return Err(Error::InvalidArgument("mailto URL has no address".into()));
            }
        }
        other => {
            return Err(Error::InvalidArgument(format!(
                "URL scheme '{other}' is not allowed (only http, https, mailto)"
            )));
        }
    }
    Ok(url.to_string())
}

/// Decides whether an `open_target` argument is a URL or a path, and validates its shape.
/// Paths must be absolute local paths (drive-letter form); UNC (`\\server\share`) and
/// device (`\\?\`, `\\.\`) paths are rejected so a remote caller cannot make the PC
/// authenticate to an arbitrary SMB server.
pub fn classify_target(target: &str) -> Result<Target> {
    let t = target.trim();
    if t.is_empty() {
        return Err(Error::InvalidArgument("target is empty".into()));
    }
    if has_control_chars(t) {
        return Err(Error::InvalidArgument(
            "target contains control characters".into(),
        ));
    }
    if url_scheme(t).is_some() {
        return validate_url(t).map(Target::Url);
    }
    if t.starts_with("\\\\") || t.starts_with("//") {
        return Err(Error::InvalidArgument(
            "UNC and device paths are not allowed".into(),
        ));
    }
    if t.len() > 32_767 {
        return Err(Error::InvalidArgument("path too long".into()));
    }
    let b = t.as_bytes();
    let drive_abs = b.len() >= 3
        && b[0].is_ascii_alphabetic()
        && b[1] == b':'
        && (b[2] == b'\\' || b[2] == b'/');
    if !drive_abs {
        return Err(Error::InvalidArgument(
            "path must be absolute (e.g. C:\\Users\\...)".into(),
        ));
    }
    Ok(Target::Path(t.to_string()))
}

/// Checks that a validated path exists on disk.
pub fn ensure_exists(path: &str) -> Result<()> {
    if Path::new(path).exists() {
        Ok(())
    } else {
        Err(Error::NotFound(format!("path does not exist: {path}")))
    }
}

/// Basic sanity check for app ids received from callers.
pub fn validate_app_id(app_id: &str) -> Result<()> {
    if app_id.is_empty() || app_id.len() > MAX_APP_ID_LEN {
        return Err(Error::InvalidArgument("app_id is empty or too long".into()));
    }
    if has_control_chars(app_id) {
        return Err(Error::InvalidArgument(
            "app_id contains control characters".into(),
        ));
    }
    Ok(())
}

/// Heuristic: is this AppsFolder parsing name a packaged-app AUMID?
///
/// Packaged AUMIDs have the form `<PackageFamilyName>!<ApplicationId>`, where the package
/// family name is `<Name>_<PublisherId>` and the publisher id is 13 lowercase base32
/// characters (`0-9`, `a-z` without `i l o u`). Win32 apps that registered an explicit
/// AUMID (e.g. `Microsoft.Windows.Explorer`) do not match this shape.
pub fn is_packaged_aumid(parsing_name: &str) -> bool {
    let Some((family, app)) = parsing_name.split_once('!') else {
        return false;
    };
    if app.is_empty() || family.contains(['\\', '/', ':']) || app.contains(['\\', '/', ':']) {
        return false;
    }
    let Some((name, publisher)) = family.rsplit_once('_') else {
        return false;
    };
    !name.is_empty()
        && publisher.len() == 13
        && publisher
            .chars()
            .all(|c| c.is_ascii_digit() || (c.is_ascii_lowercase() && !"ilou".contains(c)))
}

/// Infers the app kind from its parsing name.
pub fn kind_for(parsing_name: &str) -> AppKind {
    if is_packaged_aumid(parsing_name) {
        AppKind::Packaged
    } else {
        AppKind::Desktop
    }
}

/// If a parsing name starts with a known-folder GUID (`{GUID}\rest`), returns the GUID
/// text without braces and the remainder.
pub fn split_known_folder_path(parsing_name: &str) -> Option<(&str, &str)> {
    let rest = parsing_name.strip_prefix('{')?;
    let close = rest.find('}')?;
    let guid = &rest[..close];
    let tail = rest[close + 1..].strip_prefix('\\')?;
    if guid.len() != 36 || !guid.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return None;
    }
    Some((guid, tail))
}

/// Parses `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx` into a u128 (as used by `GUID::from_u128`).
pub fn parse_guid_u128(s: &str) -> Option<u128> {
    let parts: Vec<&str> = s.split('-').collect();
    let lens = [8, 4, 4, 4, 12];
    if parts.len() != 5 || parts.iter().zip(lens).any(|(p, l)| p.len() != l) {
        return None;
    }
    u128::from_str_radix(&parts.concat(), 16).ok()
}

/// Removes duplicate app ids (case-insensitive, first wins) and sorts by name, then id.
pub fn dedupe_and_sort(mut apps: Vec<AppEntry>) -> Vec<AppEntry> {
    let mut seen = std::collections::HashSet::new();
    apps.retain(|a| seen.insert(a.app_id.to_lowercase()));
    apps.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.app_id.cmp(&b.app_id))
    });
    apps
}

/// Converts a top-down 32-bit BGRA buffer from GDI into straight (non-premultiplied) RGBA.
///
/// * If every alpha byte is 0 the bitmap has no alpha channel: pixels are made opaque.
/// * If any colour channel exceeds its alpha the data cannot be premultiplied, so colours
///   are kept as-is; otherwise colours are un-premultiplied (`c * 255 / a`, rounded).
pub fn bgra_to_rgba(bgra: &[u8]) -> Vec<u8> {
    let px = bgra.chunks_exact(4);
    let no_alpha = px.clone().all(|p| p[3] == 0);
    let premultiplied = !no_alpha
        && px
            .clone()
            .all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]);
    let mut out = Vec::with_capacity(bgra.len());
    for p in px {
        let (b, g, r, a) = (p[0], p[1], p[2], p[3]);
        if no_alpha {
            out.extend_from_slice(&[r, g, b, 255]);
        } else if premultiplied && a != 0 && a != 255 {
            let un = |c: u8| ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
            out.extend_from_slice(&[un(r), un(g), un(b), a]);
        } else if a == 0 {
            out.extend_from_slice(&[0, 0, 0, 0]);
        } else {
            out.extend_from_slice(&[r, g, b, a]);
        }
    }
    out
}

/// Downscales straight RGBA by area averaging (alpha-weighted colours). Upscaling is not
/// done: if the target is not smaller in both dimensions the input is returned unchanged.
pub fn downscale_rgba(src: &[u8], w: u32, h: u32, tw: u32, th: u32) -> (Vec<u8>, u32, u32) {
    if tw == 0 || th == 0 || tw >= w || th >= h || src.len() != (w * h * 4) as usize {
        return (src.to_vec(), w, h);
    }
    let mut out = vec![0u8; (tw * th * 4) as usize];
    for ty in 0..th {
        let y0 = ty * h / th;
        let y1 = ((ty + 1) * h / th).max(y0 + 1);
        for tx in 0..tw {
            let x0 = tx * w / tw;
            let x1 = ((tx + 1) * w / tw).max(x0 + 1);
            let (mut sr, mut sg, mut sb, mut sa, mut n) = (0u64, 0u64, 0u64, 0u64, 0u64);
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = ((y * w + x) * 4) as usize;
                    let a = src[i + 3] as u64;
                    sr += src[i] as u64 * a;
                    sg += src[i + 1] as u64 * a;
                    sb += src[i + 2] as u64 * a;
                    sa += a;
                    n += 1;
                }
            }
            let o = ((ty * tw + tx) * 4) as usize;
            let avg = |sum: u64| {
                sum.checked_add(sa / 2)
                    .and_then(|s| s.checked_div(sa))
                    .unwrap_or(0) as u8
            };
            out[o] = avg(sr);
            out[o + 1] = avg(sg);
            out[o + 2] = avg(sb);
            out[o + 3] = ((sa + n / 2) / n) as u8;
        }
    }
    (out, tw, th)
}

/// Encodes straight RGBA8 pixels as a PNG.
pub fn encode_png(rgba: &[u8], w: u32, h: u32) -> Result<Vec<u8>> {
    if rgba.len() != (w as usize) * (h as usize) * 4 || w == 0 || h == 0 {
        return Err(Error::InvalidArgument(
            "pixel buffer does not match dimensions".into(),
        ));
    }
    let mut buf = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut buf, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().map_err(|e| Error::Png(e.to_string()))?;
        writer
            .write_image_data(rgba)
            .map_err(|e| Error::Png(e.to_string()))?;
        writer.finish().map_err(|e| Error::Png(e.to_string()))?;
    }
    Ok(buf)
}

/// Clamps a requested icon size into the supported range.
pub fn clamp_icon_size(size_px: u32) -> u32 {
    size_px.clamp(MIN_ICON_PX, MAX_ICON_PX)
}

/// 64-bit FNV-1a hash (stable across runs and platforms, unlike `DefaultHasher`).
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// File name used by the icon cache for an app id and size. App ids are compared
/// case-insensitively by the shell, so the key is built from the lower-cased id. A short
/// readable prefix helps when browsing the cache folder.
pub fn icon_cache_key(app_id: &str, size_px: u32) -> String {
    let lower = app_id.to_lowercase();
    let readable: String = lower
        .rsplit(['\\', '/', '!'])
        .next()
        .unwrap_or("")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-')
        .take(32)
        .collect();
    format!(
        "{readable}-{:016x}-{size_px}.png",
        fnv1a64(lower.as_bytes())
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str, name: &str) -> AppEntry {
        AppEntry {
            app_id: id.into(),
            name: name.into(),
            kind: kind_for(id),
            target_path: None,
        }
    }

    #[test]
    fn url_scheme_detection() {
        assert_eq!(url_scheme("https://x"), Some("https"));
        assert_eq!(url_scheme("mailto:a@b"), Some("mailto"));
        assert_eq!(url_scheme("C:\\Windows"), None);
        assert_eq!(url_scheme("c:/x"), None);
        assert_eq!(url_scheme("1abc:x"), None);
        assert_eq!(url_scheme("no colon"), None);
        assert_eq!(url_scheme("ms-settings:display"), Some("ms-settings"));
    }

    #[test]
    fn url_validation() {
        assert!(validate_url("https://example.com").is_ok());
        assert!(validate_url("HTTP://example.com:8080/a?b#c").is_ok());
        assert!(validate_url("https://user@[::1]:443/").is_ok());
        assert!(validate_url("mailto:someone@example.com").is_ok());
        assert!(validate_url("https:example.com").is_err());
        assert!(validate_url("https:///path").is_err());
        assert!(validate_url("file:///C:/Windows").is_err());
        assert!(validate_url("javascript:alert(1)").is_err());
        assert!(validate_url("ms-settings:display").is_err());
        assert!(validate_url("shell:AppsFolder").is_err());
        assert!(validate_url("https://a.com/\nx").is_err());
        assert!(validate_url("https://a.com/ x").is_err());
        assert!(validate_url("mailto:").is_err());
        assert!(validate_url("https://a\\b.com/").is_err());
    }

    #[test]
    fn target_classification() {
        assert_eq!(
            classify_target("https://a.com").unwrap(),
            Target::Url("https://a.com".into())
        );
        assert_eq!(
            classify_target(" C:\\Windows ").unwrap(),
            Target::Path("C:\\Windows".into())
        );
        assert_eq!(
            classify_target("d:/x/y.txt").unwrap(),
            Target::Path("d:/x/y.txt".into())
        );
        assert!(classify_target("relative\\path").is_err());
        assert!(classify_target("C:relative").is_err());
        assert!(classify_target("\\\\server\\share").is_err());
        assert!(classify_target("\\\\?\\C:\\x").is_err());
        assert!(classify_target("//server/share").is_err());
        assert!(classify_target("").is_err());
        assert!(classify_target("C:\\a\0b").is_err());
        assert!(classify_target("ftp://x").is_err());
    }

    #[test]
    fn existence_check() {
        let dir = std::env::temp_dir();
        assert!(ensure_exists(dir.to_str().unwrap()).is_ok());
        assert!(matches!(
            ensure_exists("Z:\\definitely\\missing\\helm"),
            Err(Error::NotFound(_))
        ));
    }

    #[test]
    fn app_id_validation() {
        assert!(validate_app_id("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App").is_ok());
        assert!(validate_app_id("").is_err());
        assert!(validate_app_id("a\nb").is_err());
        assert!(validate_app_id(&"x".repeat(MAX_APP_ID_LEN + 1)).is_err());
    }

    #[test]
    fn packaged_detection() {
        assert!(is_packaged_aumid(
            "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"
        ));
        assert!(is_packaged_aumid(
            "Microsoft.WindowsTerminal_8wekyb3d8bbwe!App"
        ));
        assert!(!is_packaged_aumid("Microsoft.Windows.Explorer"));
        assert!(!is_packaged_aumid(
            "{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\notepad.exe"
        ));
        assert!(!is_packaged_aumid("Chrome"));
        assert!(!is_packaged_aumid("Foo_8wekyb3d8bbwe!"));
        assert!(!is_packaged_aumid("Foo_8WEKYB3D8BBWE!App"));
        assert!(!is_packaged_aumid("C:\\x_8wekyb3d8bbwe!App"));
        assert_eq!(
            kind_for("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"),
            AppKind::Packaged
        );
        assert_eq!(kind_for("Chrome"), AppKind::Desktop);
    }

    #[test]
    fn known_folder_split_and_guid() {
        let (g, rest) =
            split_known_folder_path("{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\notepad.exe").unwrap();
        assert_eq!(g, "1AC14E77-02E7-4E5D-B744-2EB1AE5198B7");
        assert_eq!(rest, "notepad.exe");
        assert!(split_known_folder_path("C:\\x").is_none());
        assert!(split_known_folder_path("{bad}\\x").is_none());
        assert_eq!(
            parse_guid_u128("1AC14E77-02E7-4E5D-B744-2EB1AE5198B7"),
            Some(0x1AC14E77_02E7_4E5D_B744_2EB1AE5198B7)
        );
        assert_eq!(parse_guid_u128("1AC14E77-02E7-4E5D-B744"), None);
        assert_eq!(
            parse_guid_u128("zzzzzzzz-02E7-4E5D-B744-2EB1AE5198B7"),
            None
        );
    }

    #[test]
    fn dedupe_sorts_and_removes_duplicates() {
        let apps = vec![
            app("b!x", "beta"),
            app("A_8wekyb3d8bbwe!App", "Alpha"),
            app("a_8wekyb3d8bbwe!app", "Alpha dup"),
            app("c", "Charlie"),
            app("b2", "Beta"),
        ];
        let out = dedupe_and_sort(apps);
        let ids: Vec<&str> = out.iter().map(|a| a.app_id.as_str()).collect();
        assert_eq!(ids, ["A_8wekyb3d8bbwe!App", "b!x", "b2", "c"]);
        assert_eq!(out[0].kind, AppKind::Packaged);
    }

    #[test]
    fn pixel_conversion_premultiplied() {
        // BGRA premultiplied: half-transparent pure red => r = 128, a = 128.
        let out = bgra_to_rgba(&[0, 0, 128, 128, 0, 0, 0, 0, 10, 20, 30, 255]);
        assert_eq!(&out[0..4], &[255, 0, 0, 128]);
        assert_eq!(&out[4..8], &[0, 0, 0, 0]);
        assert_eq!(&out[8..12], &[30, 20, 10, 255]);
    }

    #[test]
    fn pixel_conversion_no_alpha_is_opaque() {
        let out = bgra_to_rgba(&[1, 2, 3, 0, 4, 5, 6, 0]);
        assert_eq!(out, [3, 2, 1, 255, 6, 5, 4, 255]);
    }

    #[test]
    fn pixel_conversion_straight_alpha_kept() {
        // colour > alpha means the data is not premultiplied; keep values.
        let out = bgra_to_rgba(&[200, 100, 50, 100]);
        assert_eq!(out, [50, 100, 200, 100]);
    }

    #[test]
    fn downscale_averages() {
        // 2x2 -> 1x1: two opaque white, two transparent.
        let src = [
            255, 255, 255, 255, 0, 0, 0, 0, 0, 0, 0, 0, 255, 255, 255, 255,
        ];
        let (out, w, h) = downscale_rgba(&src, 2, 2, 1, 1);
        assert_eq!((w, h), (1, 1));
        assert_eq!(out, [255, 255, 255, 128]);
        let (same, w2, h2) = downscale_rgba(&src, 2, 2, 4, 4);
        assert_eq!((w2, h2), (2, 2));
        assert_eq!(same, src);
    }

    #[test]
    fn png_roundtrip_header() {
        let png = encode_png(&[255, 0, 0, 255, 0, 255, 0, 128], 2, 1).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let dec = png::Decoder::new(std::io::Cursor::new(png));
        let mut reader = dec.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (2, 1));
        assert_eq!(&buf[..8], &[255, 0, 0, 255, 0, 255, 0, 128]);
        assert!(encode_png(&[0; 4], 2, 2).is_err());
    }

    #[test]
    fn icon_sizes_clamped() {
        assert_eq!(clamp_icon_size(0), MIN_ICON_PX);
        assert_eq!(clamp_icon_size(48), 48);
        assert_eq!(clamp_icon_size(10_000), MAX_ICON_PX);
    }

    #[test]
    fn cache_keys_are_stable_and_distinct() {
        let a = icon_cache_key("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App", 64);
        assert_eq!(
            a,
            icon_cache_key("microsoft.windowscalculator_8wekyb3d8bbwe!app", 64)
        );
        assert_ne!(
            a,
            icon_cache_key("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App", 32)
        );
        assert_ne!(a, icon_cache_key("Other_8wekyb3d8bbwe!App", 64));
        assert!(a.starts_with("app-") && a.ends_with("-64.png"));
        let weird = icon_cache_key("{GUID}\\..\\..\\evil:name*?.exe", 32);
        assert!(!weird.contains(['\\', '/', ':', '*', '?']));
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}
