//! Presentation mode (`Docs/protocol/presentation.md`, hello feature `g2`): detect the
//! presentation app in front (PowerPoint, Google Slides in a browser, a PDF viewer), move
//! through slides with keys, read PowerPoint's slide number + speaker notes through its
//! automation interface, and draw a laser pointer on a click-through overlay window.
//!
//! The PC side goes through [`PresentBackend`] so tests use [`fake::FakePresent`] and never
//! touch this computer. Everything here except the backends is plain logic.

use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;
use serde_json::Value;

use crate::platform::{ForegroundInfo, PResult, Rect};
use crate::router::RpcError;

pub mod fake;
pub mod overlay;
#[cfg(windows)]
pub(crate) mod win;

/// Longest speaker-notes text sent to the phone (characters).
pub const MAX_NOTES_CHARS: usize = 16_000;

/// A presentation app the agent knows how to drive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PresentApp {
    Powerpoint,
    /// Google Slides in a browser.
    Slides,
    /// A PDF viewer (desktop app or a browser tab showing a PDF).
    Pdf,
}

/// What is in front, as far as presenting goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Detected {
    pub app: PresentApp,
    /// The window belongs to a web browser (Slides, or a PDF tab).
    pub in_browser: bool,
}

const BROWSERS: &[&str] = &[
    "chrome.exe",
    "msedge.exe",
    "firefox.exe",
    "brave.exe",
    "opera.exe",
    "vivaldi.exe",
    "arc.exe",
    "chromium.exe",
];

const PDF_VIEWERS: &[&str] = &[
    "acrord32.exe",
    "acrobat.exe",
    "sumatrapdf.exe",
    "foxitpdfreader.exe",
    "foxitreader.exe",
    "foxitpdfeditor.exe",
    "foxitphantompdf.exe",
    "pdfxedit.exe",
    "pdfxcview.exe",
    "okular.exe",
    "nitropdf.exe",
    "pdfgear.exe",
];

/// Classify the foreground window by its exe name and title.
pub fn classify(fg: &ForegroundInfo) -> Option<Detected> {
    let exe = fg.exe_name.to_ascii_lowercase();
    let title = fg.title.to_lowercase();
    if exe == "powerpnt.exe" {
        return Some(Detected {
            app: PresentApp::Powerpoint,
            in_browser: false,
        });
    }
    if PDF_VIEWERS.contains(&exe.as_str()) {
        return Some(Detected {
            app: PresentApp::Pdf,
            in_browser: false,
        });
    }
    if BROWSERS.contains(&exe.as_str()) {
        if title.contains("google slides") {
            return Some(Detected {
                app: PresentApp::Slides,
                in_browser: true,
            });
        }
        // Browser PDF tabs are titled after the file ("deck.pdf - Profile - Edge").
        let first = title.split([' ', '\u{2013}', '\u{2014}']).next().unwrap_or("");
        if title.ends_with(".pdf") || first.ends_with(".pdf") || title.contains(".pdf - ") {
            return Some(Detected {
                app: PresentApp::Pdf,
                in_browser: true,
            });
        }
    }
    None
}

/// `present.go` actions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GoAction {
    Next,
    Prev,
    First,
    Last,
    Black,
    Start,
    End,
}

impl GoAction {
    pub fn parse(s: &str) -> Option<GoAction> {
        Some(match s {
            "next" => GoAction::Next,
            "prev" => GoAction::Prev,
            "first" => GoAction::First,
            "last" => GoAction::Last,
            "black" => GoAction::Black,
            "start" => GoAction::Start,
            "end" => GoAction::End,
            _ => return None,
        })
    }
}

/// The key combination for `action` in `app` (`None` app = generic viewer).
/// `None` = this app has no key for it.
pub fn keys_for(app: Option<&Detected>, action: GoAction) -> Option<&'static str> {
    use GoAction::*;
    let Some(d) = app else {
        return match action {
            Next => Some("right"),
            Prev => Some("left"),
            First => Some("home"),
            Last => Some("end"),
            End => Some("esc"),
            Black | Start => None,
        };
    };
    match (d.app, action) {
        (PresentApp::Powerpoint | PresentApp::Slides, Next) => Some("right"),
        (PresentApp::Powerpoint | PresentApp::Slides, Prev) => Some("left"),
        (_, First) => Some("home"),
        (_, Last) => Some("end"),
        (_, End) => Some("esc"),
        // PowerPoint and Slides both toggle a black screen with B.
        (PresentApp::Powerpoint | PresentApp::Slides, Black) => Some("b"),
        (PresentApp::Powerpoint, Start) => Some("f5"),
        (PresentApp::Slides, Start) => Some("ctrl+f5"),
        (PresentApp::Pdf, Next) => Some("pagedown"),
        (PresentApp::Pdf, Prev) => Some("pageup"),
        // Acrobat / Sumatra / Foxit: Ctrl+L = full screen. In a browser that would focus the
        // address bar, so there is no start key there.
        (PresentApp::Pdf, Start) if !d.in_browser => Some("ctrl+l"),
        (PresentApp::Pdf, Start | Black) => None,
    }
}

/// PowerPoint's current slide (automation).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct SlideInfo {
    /// 1-based slide number.
    pub index: u32,
    pub count: u32,
    /// Speaker notes (may be empty), at most [`MAX_NOTES_CHARS`].
    pub notes: String,
    /// A slide show is running (false = the editor's current slide).
    pub showing: bool,
}

/// Clean up speaker notes: PowerPoint uses `\r` (and `\u{b}` for soft breaks).
pub fn clean_notes(raw: &str) -> String {
    let s: String = raw
        .replace("\r\n", "\n")
        .chars()
        .map(|c| if c == '\r' || c == '\u{b}' { '\n' } else { c })
        .filter(|c| *c == '\n' || *c == '\t' || !c.is_control())
        .collect();
    let s = s.trim();
    s.chars().take(MAX_NOTES_CHARS).collect()
}

/// The PC side of presentation mode.
pub trait PresentBackend: Send + Sync {
    /// The window in front now.
    fn foreground(&self) -> PResult<Option<ForegroundInfo>>;
    /// PowerPoint's slide via automation. `Ok(None)` = PowerPoint has no open presentation;
    /// `Err(unsupported)` = PowerPoint is not running or cannot be automated.
    fn powerpoint(&self) -> PResult<Option<SlideInfo>>;
    /// Show the laser dot at (`x`,`y`) in 0..1 of `rect` (virtual-desktop pixels).
    fn laser_show(&self, rect: Rect, x: f32, y: f32, trail: bool) -> PResult<()>;
    fn laser_hide(&self) -> PResult<()>;
}

/// Laser settings from `present.laser` (last caller wins).
#[derive(Clone, Debug, Default, PartialEq)]
struct LaserCfg {
    /// Session that configured or last drove the laser.
    owner: Option<u64>,
    rect: Option<Rect>,
    monitor_id: Option<String>,
    trail: bool,
    visible: bool,
}

/// Presentation state shared by all sessions of one platform.
pub struct PresentHub {
    backend: Arc<dyn PresentBackend>,
    laser: Mutex<LaserCfg>,
}

impl std::fmt::Debug for PresentHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PresentHub").finish_non_exhaustive()
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Parsed `lp` frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LaserFrame {
    At(f32, f32),
    Off,
}

/// Parse `{"k":"lp","x":0..1,"y":0..1,"on":bool}` (`on` defaults to true; `x`/`y` are only
/// required when on). Out-of-range coordinates are clamped.
pub fn parse_laser(frame: &Value) -> Option<LaserFrame> {
    let on = match frame.get("on") {
        None | Some(Value::Null) => true,
        Some(v) => v.as_bool()?,
    };
    if !on {
        return Some(LaserFrame::Off);
    }
    let x = frame.get("x")?.as_f64().filter(|f| f.is_finite())?;
    let y = frame.get("y")?.as_f64().filter(|f| f.is_finite())?;
    Some(LaserFrame::At(
        x.clamp(0.0, 1.0) as f32,
        y.clamp(0.0, 1.0) as f32,
    ))
}

impl PresentHub {
    pub fn new(backend: Arc<dyn PresentBackend>) -> PresentHub {
        PresentHub {
            backend,
            laser: Mutex::new(LaserCfg::default()),
        }
    }

    /// A hub whose backend reports `unsupported` for everything (platforms without one).
    pub fn unsupported() -> Arc<PresentHub> {
        static HUB: OnceLock<Arc<PresentHub>> = OnceLock::new();
        HUB.get_or_init(|| Arc::new(PresentHub::new(Arc::new(NoBackend))))
            .clone()
    }

    pub fn backend(&self) -> &Arc<dyn PresentBackend> {
        &self.backend
    }

    /// `present.laser`: choose the monitor (`None` = the session's default) and trail.
    pub fn configure(&self, session: u64, rect: Option<Rect>, monitor_id: Option<String>, trail: bool) {
        let mut l = lock(&self.laser);
        l.owner = Some(session);
        l.rect = rect;
        l.monitor_id = monitor_id;
        l.trail = trail;
    }

    /// The configured monitor id, if any.
    pub fn laser_monitor(&self) -> Option<String> {
        lock(&self.laser).monitor_id.clone()
    }

    /// One `lp` frame. `fallback` gives the monitor when `present.laser` chose none. Returns
    /// an `input.error` code on failure.
    pub fn frame(
        &self,
        session: u64,
        frame: &Value,
        fallback: impl FnOnce() -> Option<Rect>,
    ) -> Result<(), &'static str> {
        let parsed = parse_laser(frame).ok_or("bad_params")?;
        let mut l = lock(&self.laser);
        match parsed {
            LaserFrame::Off => {
                if l.visible {
                    l.visible = false;
                    self.backend.laser_hide().map_err(|_| "input_failed")?;
                }
                Ok(())
            }
            LaserFrame::At(x, y) => {
                let rect = match l.rect {
                    Some(r) => r,
                    None => fallback().ok_or("internal")?,
                };
                l.owner = Some(session);
                l.visible = true;
                let trail = l.trail;
                self.backend
                    .laser_show(rect, x, y, trail)
                    .map_err(|_| "input_failed")
            }
        }
    }

    /// A session ended: hide the laser if it was driving it, and forget its settings.
    pub fn session_ended(&self, session: u64) {
        let mut l = lock(&self.laser);
        if l.owner == Some(session) {
            if l.visible {
                let _ = self.backend.laser_hide();
            }
            *l = LaserCfg::default();
        }
    }

    pub fn laser_visible(&self) -> bool {
        lock(&self.laser).visible
    }
}

struct NoBackend;

impl PresentBackend for NoBackend {
    fn foreground(&self) -> PResult<Option<ForegroundInfo>> {
        Err(RpcError::unsupported("presentation mode is not available on this PC"))
    }
    fn powerpoint(&self) -> PResult<Option<SlideInfo>> {
        Err(RpcError::unsupported("PowerPoint automation is not available"))
    }
    fn laser_show(&self, _: Rect, _: f32, _: f32, _: bool) -> PResult<()> {
        Err(RpcError::unsupported("the laser overlay is not available"))
    }
    fn laser_hide(&self) -> PResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fg(exe: &str, title: &str) -> ForegroundInfo {
        ForegroundInfo {
            hwnd: 1,
            pid: 2,
            exe: None,
            exe_name: exe.into(),
            title: title.into(),
        }
    }

    #[test]
    fn classifies_presentation_apps() {
        let d = |e, t| classify(&fg(e, t)).map(|d| (d.app, d.in_browser));
        assert_eq!(d("POWERPNT.EXE", "PowerPoint Slide Show - deck"), Some((PresentApp::Powerpoint, false)));
        assert_eq!(d("chrome.exe", "Q3 review - Google Slides - Google Chrome"), Some((PresentApp::Slides, true)));
        assert_eq!(d("msedge.exe", "handout.pdf - Personal - Microsoft Edge"), Some((PresentApp::Pdf, true)));
        assert_eq!(d("firefox.exe", "notes.pdf"), Some((PresentApp::Pdf, true)));
        assert_eq!(d("SumatraPDF.exe", "x"), Some((PresentApp::Pdf, false)));
        assert_eq!(d("AcroRd32.exe", "x"), Some((PresentApp::Pdf, false)));
        assert_eq!(d("chrome.exe", "YouTube - Google Chrome"), None);
        assert_eq!(d("notepad.exe", "slides.pdf"), None);
    }

    #[test]
    fn keys_per_app() {
        let ppt = Detected { app: PresentApp::Powerpoint, in_browser: false };
        let slides = Detected { app: PresentApp::Slides, in_browser: true };
        let pdf = Detected { app: PresentApp::Pdf, in_browser: false };
        let web_pdf = Detected { app: PresentApp::Pdf, in_browser: true };
        assert_eq!(keys_for(Some(&ppt), GoAction::Next), Some("right"));
        assert_eq!(keys_for(Some(&ppt), GoAction::Black), Some("b"));
        assert_eq!(keys_for(Some(&ppt), GoAction::Start), Some("f5"));
        assert_eq!(keys_for(Some(&slides), GoAction::Start), Some("ctrl+f5"));
        assert_eq!(keys_for(Some(&pdf), GoAction::Next), Some("pagedown"));
        assert_eq!(keys_for(Some(&pdf), GoAction::Start), Some("ctrl+l"));
        assert_eq!(keys_for(Some(&web_pdf), GoAction::Start), None);
        assert_eq!(keys_for(Some(&pdf), GoAction::Black), None);
        assert_eq!(keys_for(None, GoAction::Last), Some("end"));
        assert_eq!(keys_for(None, GoAction::Black), None);
        // Every key name used is one the injector knows.
        for app in [None, Some(&ppt), Some(&slides), Some(&pdf), Some(&web_pdf)] {
            for a in ["next", "prev", "first", "last", "black", "start", "end"] {
                if let Some(k) = keys_for(app, GoAction::parse(a).unwrap()) {
                    assert!(helm_win_input::parse_shortcut(k).is_some(), "{k}");
                }
            }
        }
        assert_eq!(GoAction::parse("jump"), None);
    }

    #[test]
    fn notes_are_cleaned_and_capped() {
        assert_eq!(clean_notes(" a\rb\u{b}c\r\nd\u{7} "), "a\nb\nc\nd");
        assert_eq!(clean_notes(&"x".repeat(MAX_NOTES_CHARS + 50)).len(), MAX_NOTES_CHARS);
    }

    #[test]
    fn laser_frames_parse_and_clamp() {
        assert_eq!(parse_laser(&json!({"x":0.5,"y":0.25,"on":true})), Some(LaserFrame::At(0.5, 0.25)));
        assert_eq!(parse_laser(&json!({"x":1.5,"y":-2})), Some(LaserFrame::At(1.0, 0.0)));
        assert_eq!(parse_laser(&json!({"on":false})), Some(LaserFrame::Off));
        assert_eq!(parse_laser(&json!({"x":0.5})), None);
        assert_eq!(parse_laser(&json!({"x":"a","y":0})), None);
        assert_eq!(parse_laser(&json!({"x":0,"y":0,"on":"yes"})), None);
    }

    #[test]
    fn hub_hides_laser_when_owner_leaves() {
        let (hub, fake) = fake::FakePresenter::default().parts();
        let rect = Rect { x: 0, y: 0, width: 100, height: 100 };
        hub.frame(7, &json!({"x":0.1,"y":0.2}), || Some(rect)).unwrap();
        assert!(hub.laser_visible());
        hub.session_ended(8); // someone else
        assert!(hub.laser_visible());
        hub.session_ended(7);
        assert!(!hub.laser_visible());
        assert_eq!(fake.laser_log().last().unwrap(), &fake::LaserCall::Hide);
        assert_eq!(hub.frame(7, &json!({"x":0.1,"y":0.2}), || None), Err("internal"));
    }
}
