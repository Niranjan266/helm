//! Text-focus hint: tells the agent whether the focused UI element accepts typing, so the
//! phone can open its keyboard automatically (ROADMAP C2, event `input.textFocus`).

use std::time::{Duration, Instant};

use crate::error::Result;

/// How long the focus must stay put before a change is reported.
pub const DEBOUNCE: Duration = Duration::from_millis(100);

/// UI Automation control type id for Edit controls.
pub const UIA_EDIT_CONTROL_TYPE: i32 = 50004;
/// UI Automation control type id for Document controls.
pub const UIA_DOCUMENT_CONTROL_TYPE: i32 = 50030;

/// Decides whether a focused element is a text-input target.
///
/// * Edit and Document controls count, unless they expose a read-only Value pattern
///   (e.g. a browser page that is just being viewed).
/// * Any other keyboard-focusable element counts if it has a Value pattern that is not
///   read-only (custom text boxes, editable combo boxes, …).
///
/// `value_read_only` is `None` when the element has no Value pattern.
pub fn is_text_target(
    control_type: i32,
    keyboard_focusable: bool,
    value_read_only: Option<bool>,
) -> bool {
    if control_type == UIA_EDIT_CONTROL_TYPE || control_type == UIA_DOCUMENT_CONTROL_TYPE {
        return value_read_only != Some(true);
    }
    keyboard_focusable && value_read_only == Some(false)
}

/// Debounces a stream of booleans: a value is reported once it has been stable for the
/// delay, and only if it differs from the last reported value.
#[derive(Debug, Clone)]
pub struct Debouncer {
    delay: Duration,
    pending: Option<(bool, Instant)>,
    last: Option<bool>,
}

impl Debouncer {
    pub fn new(delay: Duration) -> Self {
        Self {
            delay,
            pending: None,
            last: None,
        }
    }

    /// Reports `value` immediately (used for the initial state).
    pub fn force(&mut self, value: bool) -> Option<bool> {
        self.pending = None;
        if self.last == Some(value) {
            return None;
        }
        self.last = Some(value);
        Some(value)
    }

    /// Records a new observation made at `now`.
    pub fn push(&mut self, value: bool, now: Instant) {
        self.pending = Some((value, now + self.delay));
    }

    /// When the pending value becomes due, if any.
    pub fn deadline(&self) -> Option<Instant> {
        self.pending.map(|(_, at)| at)
    }

    /// Returns the value to report if the pending one is due and is a change.
    pub fn poll(&mut self, now: Instant) -> Option<bool> {
        match self.pending {
            Some((value, at)) if now >= at => {
                self.pending = None;
                self.force(value)
            }
            _ => None,
        }
    }
}

/// Watches keyboard focus with UI Automation on its own thread.
///
/// The callback runs on the watcher thread: once with the initial state, then whenever
/// the answer changes (debounced by [`DEBOUNCE`]). Stop it with [`TextFocusWatcher::stop`]
/// or by dropping it.
pub struct TextFocusWatcher {
    #[cfg(windows)]
    inner: Option<crate::win::focus::Worker>,
}

impl TextFocusWatcher {
    pub fn start<F>(callback: F) -> Result<Self>
    where
        F: Fn(bool) + Send + 'static,
    {
        #[cfg(windows)]
        {
            Ok(Self {
                inner: Some(crate::win::focus::Worker::start(Box::new(callback))?),
            })
        }
        #[cfg(not(windows))]
        {
            let _ = callback;
            Err(crate::Error::Unsupported)
        }
    }

    /// Unregisters the UI Automation handler and joins the thread. Safe to call twice.
    pub fn stop(&mut self) {
        #[cfg(windows)]
        if let Some(worker) = self.inner.take() {
            worker.stop();
        }
    }
}

impl Drop for TextFocusWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification() {
        const BUTTON: i32 = 50000;
        const COMBO: i32 = 50003;
        assert!(is_text_target(UIA_EDIT_CONTROL_TYPE, true, None));
        assert!(is_text_target(UIA_EDIT_CONTROL_TYPE, true, Some(false)));
        assert!(!is_text_target(UIA_EDIT_CONTROL_TYPE, true, Some(true)));
        assert!(is_text_target(UIA_DOCUMENT_CONTROL_TYPE, false, None));
        assert!(!is_text_target(UIA_DOCUMENT_CONTROL_TYPE, true, Some(true)));
        assert!(is_text_target(COMBO, true, Some(false)));
        assert!(!is_text_target(COMBO, false, Some(false)));
        assert!(!is_text_target(COMBO, true, Some(true)));
        assert!(!is_text_target(BUTTON, true, None));
        assert!(!is_text_target(0, false, None));
    }

    #[test]
    fn debounce_reports_only_stable_changes() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut d = Debouncer::new(Duration::from_millis(100));
        assert_eq!(d.force(false), Some(false));
        assert_eq!(d.poll(ms(0)), None);
        // Quick flicker true->false within the window: nothing reported.
        d.push(true, ms(0));
        d.push(false, ms(50));
        assert_eq!(d.poll(ms(120)), None);
        assert_eq!(d.poll(ms(160)), None);
        assert_eq!(d.deadline(), None);
        // Stable true is reported once, after the delay.
        d.push(true, ms(200));
        assert_eq!(d.deadline(), Some(ms(300)));
        assert_eq!(d.poll(ms(299)), None);
        assert_eq!(d.poll(ms(300)), Some(true));
        assert_eq!(d.poll(ms(400)), None);
        // Same value again: no report.
        d.push(true, ms(500));
        assert_eq!(d.poll(ms(700)), None);
        assert_eq!(d.force(true), None);
    }
}
