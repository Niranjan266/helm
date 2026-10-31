//! Tiny VT helper: strip escape sequences to get the plain text out of ConPTY output.
//! Useful for tests, logs and "search the scrollback" features; the phone should render the
//! raw bytes with a real terminal emulator instead.

/// Removes CSI (`ESC [ ... final`), OSC (`ESC ] ... BEL` or `ESC ] ... ESC \`), other
/// two-byte `ESC x` sequences and C0 controls except `\n`, `\r` and `\t`. Invalid UTF-8 is
/// replaced with U+FFFD.
pub fn strip(bytes: &[u8]) -> String {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == 0x1b {
            match bytes.get(i + 1) {
                Some(b'[') => {
                    // CSI: parameters / intermediates 0x20..=0x3F, final byte 0x40..=0x7E.
                    i += 2;
                    while i < bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) {
                        i += 1;
                    }
                    i += 1;
                }
                Some(b']') => {
                    // OSC: terminated by BEL or ST (ESC \).
                    i += 2;
                    while i < bytes.len() {
                        if bytes[i] == 0x07 {
                            i += 1;
                            break;
                        }
                        if bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'\\') {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                }
                Some(_) => i += 2,
                None => i += 1,
            }
            continue;
        }
        if (b < 0x20 && b != b'\n' && b != b'\r' && b != b'\t') || b == 0x7f {
            i += 1;
            continue;
        }
        out.push(b);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::strip;

    #[test]
    fn plain_text_passes() {
        assert_eq!(strip(b"hello\r\nworld\t!"), "hello\r\nworld\t!");
    }

    #[test]
    fn csi_osc_and_controls_removed() {
        let s = b"\x1b[?25l\x1b[2J\x1b[m\x1b[Hhelm-ok\x1b[K\x1b]0;C:\\WINDOWS\\cmd.exe\x07\x1b[?25h\x08";
        assert_eq!(strip(s), "helm-ok");
        assert_eq!(strip(b"a\x1b]0;title\x1b\\b"), "ab");
        assert_eq!(strip(b"x\x1b=y\x1b"), "xy");
        assert_eq!(strip(b"\x1b[38;2;255;0;0mred\x1b[0m"), "red");
    }

    #[test]
    fn unterminated_sequences_do_not_panic() {
        assert_eq!(strip(b"ok\x1b["), "ok");
        assert_eq!(strip(b"ok\x1b]0;never ends"), "ok");
    }

    #[test]
    fn utf8_kept() {
        assert_eq!(strip("é→✓".as_bytes()), "é→✓");
    }
}
