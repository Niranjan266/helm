//! Clipboard text and images, and a change watcher (ROADMAP G3).
//!
//! All functions retry for a short while when another program has the clipboard open and
//! then fail with [`Error::ClipboardBusy`](crate::Error::ClipboardBusy).

use crate::error::Result;

/// What changed on the clipboard (reported by [`ClipboardWatcher`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipboardChange {
    /// `GetClipboardSequenceNumber` after the change.
    pub sequence: u32,
    /// Unicode text is available.
    pub has_text: bool,
    /// An image (DIB or PNG) is available.
    pub has_image: bool,
}

/// Converts lone `\n` (and lone `\r`) to the `\r\n` line endings Windows programs expect.
pub fn to_crlf(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 16);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push_str("\r\n");
            }
            '\n' => out.push_str("\r\n"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(windows)]
pub use crate::win::clipboard::{
    get_image_png, get_text, sequence_number, set_image_png, set_text,
};

#[cfg(not(windows))]
mod stub {
    use super::*;
    use crate::Error;

    /// Returns the clipboard text, or `None` if there is none.
    pub fn get_text() -> Result<Option<String>> {
        Err(Error::Unsupported)
    }
    /// Replaces the clipboard with `text` (line endings converted to CRLF).
    pub fn set_text(_text: &str) -> Result<()> {
        Err(Error::Unsupported)
    }
    /// Returns the clipboard image as PNG, or `None` if there is none.
    pub fn get_image_png() -> Result<Option<Vec<u8>>> {
        Err(Error::Unsupported)
    }
    /// Replaces the clipboard with a PNG image (as CF_DIB plus the registered "PNG" format).
    pub fn set_image_png(_png: &[u8]) -> Result<()> {
        Err(Error::Unsupported)
    }
    /// The clipboard sequence number.
    pub fn sequence_number() -> Result<u32> {
        Err(Error::Unsupported)
    }
}

#[cfg(not(windows))]
pub use stub::*;

/// Calls back when the clipboard changes, from its own thread (a hidden message-only
/// window registered with `AddClipboardFormatListener`). Changes made through this crate's
/// `set_text` / `set_image_png` are not reported.
pub struct ClipboardWatcher {
    #[cfg(windows)]
    inner: Option<crate::win::clipboard::Watcher>,
}

impl ClipboardWatcher {
    pub fn start<F>(callback: F) -> Result<Self>
    where
        F: Fn(ClipboardChange) + Send + 'static,
    {
        #[cfg(windows)]
        {
            Ok(Self {
                inner: Some(crate::win::clipboard::Watcher::start(Box::new(callback))?),
            })
        }
        #[cfg(not(windows))]
        {
            let _ = callback;
            Err(crate::Error::Unsupported)
        }
    }

    /// Removes the listener, destroys the window and joins the thread. Safe to call twice.
    pub fn stop(&mut self) {
        #[cfg(windows)]
        if let Some(w) = self.inner.take() {
            w.stop();
        }
    }
}

impl Drop for ClipboardWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the user had on the clipboard before a live test.
    enum Saved {
        Text(String),
        Image(Vec<u8>),
        Nothing,
    }

    fn save() -> Saved {
        if let Some(t) = get_text().unwrap() {
            Saved::Text(t)
        } else if let Some(png) = get_image_png().unwrap() {
            Saved::Image(png)
        } else {
            Saved::Nothing
        }
    }

    fn restore(saved: Saved) {
        match saved {
            Saved::Text(t) => set_text(&t).unwrap(),
            Saved::Image(png) => set_image_png(&png).unwrap(),
            Saved::Nothing => {}
        }
    }

    #[test]
    fn crlf_conversion() {
        assert_eq!(to_crlf("a\nb"), "a\r\nb");
        assert_eq!(to_crlf("a\r\nb"), "a\r\nb");
        assert_eq!(to_crlf("a\rb\n"), "a\r\nb\r\n");
        assert_eq!(to_crlf("\n\n"), "\r\n\r\n");
        assert_eq!(to_crlf("😀"), "😀");
    }

    /// Live: touches the real clipboard, restoring the previous text afterwards.
    #[test]
    #[ignore = "uses the real clipboard"]
    fn live_text_round_trip() {
        let saved = save();
        let sample = "Helm clipboard test ✓ 😀\nline two";
        set_text(sample).unwrap();
        let got = get_text().unwrap();
        restore(saved);
        assert_eq!(got.as_deref(), Some(to_crlf(sample).as_str()));
    }

    /// Live: puts an image on the clipboard and reads it back, then restores the old text.
    #[test]
    #[ignore = "uses the real clipboard"]
    fn live_image_round_trip() {
        use crate::dib::{RgbaImage, decode_png, encode_png};
        let saved = save();
        let img = RgbaImage {
            width: 3,
            height: 2,
            pixels: (0..24u8)
                .map(|i| if i % 4 == 3 { 255 } else { i * 10 })
                .collect(),
        };
        set_image_png(&encode_png(&img).unwrap()).unwrap();
        let got = get_image_png().unwrap().expect("image on clipboard");
        restore(saved);
        assert_eq!(decode_png(&got).unwrap(), img);
    }

    /// Live: our own writes are ignored by the watcher; restores the old text.
    #[test]
    #[ignore = "uses the real clipboard"]
    fn live_watcher_ignores_own_changes() {
        use std::sync::mpsc;
        use std::time::Duration;
        let saved = save();
        let (tx, rx) = mpsc::channel();
        let mut w = ClipboardWatcher::start(move |c| {
            let _ = tx.send(c);
        })
        .unwrap();
        set_text("Helm watcher test").unwrap();
        let seen = rx.recv_timeout(Duration::from_millis(500));
        w.stop();
        restore(saved);
        assert!(seen.is_err(), "own change was reported: {seen:?}");
    }

    /// Live: a change by another process (clip.exe) is reported; restores the old content.
    #[test]
    #[ignore = "uses the real clipboard"]
    fn live_watcher_reports_external_changes() {
        use std::sync::mpsc;
        use std::time::Duration;
        let saved = save();
        let (tx, rx) = mpsc::channel();
        let mut w = ClipboardWatcher::start(move |c| {
            let _ = tx.send(c);
        })
        .unwrap();
        let status = std::process::Command::new("cmd")
            .args(["/C", "echo helm external| clip"])
            .status()
            .unwrap();
        let seen = rx.recv_timeout(Duration::from_secs(3));
        w.stop();
        restore(saved);
        assert!(status.success());
        let change = seen.expect("external change not reported");
        assert!(change.has_text);
    }
}
