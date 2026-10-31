//! Pure parts: toast description, XML generation, activation-argument encoding, AUMID checks.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// Windows allows at most 5 buttons (inputs not counted) per toast.
pub const MAX_ACTIONS: usize = 5;

const ARG_BODY: &str = "helm:body";
const ARG_ACTION: &str = "helm:action:";

/// How the app shows up in the toast header and in Settings → Notifications.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppRegistration {
    /// AppUserModelID, e.g. `"Helm.Agent"`. Letters, digits, `.`, `-`, `_`; at most 128 chars.
    pub aumid: String,
    /// Name shown in the toast header, e.g. `"Helm"`.
    pub display_name: String,
    /// Optional absolute path to a .png / .ico used as the header icon.
    #[serde(default)]
    pub icon_path: Option<PathBuf>,
}

/// One button.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToastAction {
    /// Returned in [`ToastEvent::Activated::action_id`].
    pub id: String,
    pub label: String,
}

/// An inline reply text box, with its own send button.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplyBox {
    /// Action id reported when the send button is pressed.
    pub id: String,
    #[serde(default)]
    pub placeholder: String,
    /// Label of the send button (default "Reply").
    #[serde(default)]
    pub button_label: String,
}

/// A toast to show.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Toast {
    pub title: String,
    #[serde(default)]
    pub body: String,
    /// Source app name (e.g. the phone app), shown as attribution text under the body.
    #[serde(default)]
    pub app_name: Option<String>,
    /// Absolute path of an image shown as the (circle-cropped) app logo.
    #[serde(default)]
    pub image: Option<PathBuf>,
    #[serde(default)]
    pub actions: Vec<ToastAction>,
    #[serde(default)]
    pub reply: Option<ReplyBox>,
    /// Tag / group identify the toast for replacing or removing it later.
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
    /// No sound.
    #[serde(default)]
    pub silent: bool,
}

/// What happened to a shown toast.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ToastEvent {
    /// The user clicked the toast body (`action_id: None`) or a button / reply send.
    Activated { action_id: Option<String>, reply: Option<String> },
    /// Closed without activation.
    Dismissed { reason: DismissReason },
    /// Windows could not show it.
    Failed { message: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DismissReason {
    UserCanceled,
    ApplicationHidden,
    TimedOut,
}

/// Check an AppUserModelID.
pub fn validate_aumid(aumid: &str) -> Result<()> {
    if aumid.is_empty() || aumid.len() > 128 {
        return Err(Error::InvalidInput("AUMID must be 1..=128 characters".into()));
    }
    if !aumid.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')) {
        return Err(Error::InvalidInput(format!("AUMID {aumid:?} may only contain letters, digits, '.', '-', '_'")));
    }
    Ok(())
}

/// XML-escape text and attribute values.
pub fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // Control characters other than tab / newline are not valid XML 1.0.
            c if (c as u32) < 0x20 && c != '\t' && c != '\n' && c != '\r' => {}
            c => out.push(c),
        }
    }
    out
}

/// Absolute Windows path → `file:///C:/...` URI (percent-encoding the characters that matter).
pub fn file_uri(path: &Path) -> Result<String> {
    let s = path.to_string_lossy();
    let bytes = s.as_bytes();
    let drive_abs = bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/');
    if !drive_abs {
        return Err(Error::InvalidInput(format!("image path must be absolute (C:\\...): {s}")));
    }
    let mut out = String::from("file:///");
    for c in s.chars() {
        match c {
            '\\' => out.push('/'),
            ' ' => out.push_str("%20"),
            '#' => out.push_str("%23"),
            '%' => out.push_str("%25"),
            '?' => out.push_str("%3F"),
            c => out.push(c),
        }
    }
    Ok(out)
}

/// Activation argument for a button id.
pub fn action_argument(id: &str) -> String {
    format!("{ARG_ACTION}{id}")
}

/// Decode the activation argument: `None` = body click (or a foreign argument).
pub fn parse_argument(arg: &str) -> Option<String> {
    if arg == ARG_BODY {
        return None;
    }
    arg.strip_prefix(ARG_ACTION).map(str::to_owned)
}

/// Check a toast before building it.
pub fn validate_toast(t: &Toast) -> Result<()> {
    if t.title.trim().is_empty() {
        return Err(Error::InvalidInput("toast title is empty".into()));
    }
    let n = t.actions.len() + usize::from(t.reply.is_some());
    if n > MAX_ACTIONS {
        return Err(Error::InvalidInput(format!("at most {MAX_ACTIONS} buttons (reply counts as one), got {n}")));
    }
    let mut ids: Vec<&str> = t.actions.iter().map(|a| a.id.as_str()).collect();
    if let Some(r) = &t.reply {
        ids.push(&r.id);
    }
    if ids.iter().any(|i| i.is_empty()) {
        return Err(Error::InvalidInput("action ids must not be empty".into()));
    }
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    sorted.dedup();
    if sorted.len() != ids.len() {
        return Err(Error::InvalidInput("action ids must be unique".into()));
    }
    if let Some(p) = &t.image {
        file_uri(p)?;
    }
    Ok(())
}

/// The reply text box id inside the XML (fixed; the user-facing id is the button's).
pub(crate) const REPLY_INPUT_ID: &str = "helmReply";

/// Build the toast XML (ToastGeneric template).
pub fn toast_xml(t: &Toast) -> Result<String> {
    validate_toast(t)?;
    let mut x = String::with_capacity(512);
    x.push_str(&format!(r#"<toast launch="{ARG_BODY}" activationType="foreground">"#));
    x.push_str(r#"<visual><binding template="ToastGeneric">"#);
    x.push_str(&format!(r#"<text hint-maxLines="1">{}</text>"#, xml_escape(&t.title)));
    if !t.body.is_empty() {
        x.push_str(&format!("<text>{}</text>", xml_escape(&t.body)));
    }
    if let Some(app) = t.app_name.as_deref().filter(|a| !a.is_empty()) {
        x.push_str(&format!(r#"<text placement="attribution">{}</text>"#, xml_escape(app)));
    }
    if let Some(img) = &t.image {
        x.push_str(&format!(
            r#"<image placement="appLogoOverride" hint-crop="circle" src="{}"/>"#,
            xml_escape(&file_uri(img)?)
        ));
    }
    x.push_str("</binding></visual>");
    if !t.actions.is_empty() || t.reply.is_some() {
        x.push_str("<actions>");
        if let Some(r) = &t.reply {
            x.push_str(&format!(
                r#"<input id="{REPLY_INPUT_ID}" type="text" placeHolderContent="{}"/>"#,
                xml_escape(&r.placeholder)
            ));
            let label = if r.button_label.is_empty() { "Reply" } else { &r.button_label };
            x.push_str(&format!(
                r#"<action content="{}" arguments="{}" activationType="foreground" hint-inputId="{REPLY_INPUT_ID}"/>"#,
                xml_escape(label),
                xml_escape(&action_argument(&r.id))
            ));
        }
        for a in &t.actions {
            x.push_str(&format!(
                r#"<action content="{}" arguments="{}" activationType="foreground"/>"#,
                xml_escape(&a.label),
                xml_escape(&action_argument(&a.id))
            ));
        }
        x.push_str("</actions>");
    }
    if t.silent {
        x.push_str(r#"<audio silent="true"/>"#);
    }
    x.push_str("</toast>");
    Ok(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Toast {
        Toast { title: "Hi".into(), body: "there".into(), ..Default::default() }
    }

    #[test]
    fn aumid_rules() {
        assert!(validate_aumid("Helm.Agent").is_ok());
        assert!(validate_aumid("Helm_Agent-2").is_ok());
        assert!(validate_aumid("").is_err());
        assert!(validate_aumid("has space").is_err());
        assert!(validate_aumid(r"a\b").is_err());
        assert!(validate_aumid(&"a".repeat(129)).is_err());
    }

    #[test]
    fn escaping() {
        assert_eq!(xml_escape(r#"<a & "b" 'c'>"#), "&lt;a &amp; &quot;b&quot; &apos;c&apos;&gt;");
        assert_eq!(xml_escape("x\u{1}y\nz"), "xy\nz");
    }

    #[test]
    fn file_uris() {
        assert_eq!(file_uri(Path::new(r"C:\Users\a b\#1.png")).unwrap(), "file:///C:/Users/a%20b/%231.png");
        assert!(file_uri(Path::new(r"rel\x.png")).is_err());
        assert!(file_uri(Path::new(r"\\server\share\x.png")).is_err());
    }

    #[test]
    fn arguments_round_trip() {
        assert_eq!(parse_argument(&action_argument("mark_read")), Some("mark_read".into()));
        assert_eq!(parse_argument("helm:body"), None);
        assert_eq!(parse_argument("something-else"), None);
    }

    #[test]
    fn minimal_xml() {
        let x = toast_xml(&base()).unwrap();
        assert!(x.starts_with(r#"<toast launch="helm:body" activationType="foreground">"#));
        assert!(x.contains(r#"<text hint-maxLines="1">Hi</text><text>there</text>"#));
        assert!(!x.contains("<actions>"));
        assert!(!x.contains("<audio"));
    }

    #[test]
    fn full_xml() {
        let mut t = base();
        t.title = "A & B".into();
        t.app_name = Some("WhatsApp".into());
        t.image = Some(PathBuf::from(r"C:\tmp\icon.png"));
        t.actions = vec![ToastAction { id: "read".into(), label: "Mark \"read\"".into() }];
        t.reply = Some(ReplyBox { id: "reply".into(), placeholder: "Type…".into(), button_label: String::new() });
        t.silent = true;
        let x = toast_xml(&t).unwrap();
        assert!(x.contains("A &amp; B"));
        assert!(x.contains(r#"<text placement="attribution">WhatsApp</text>"#));
        assert!(x.contains(r#"src="file:///C:/tmp/icon.png""#));
        assert!(x.contains(r#"<input id="helmReply" type="text" placeHolderContent="Type…"/>"#));
        assert!(x.contains(r#"content="Reply" arguments="helm:action:reply" activationType="foreground" hint-inputId="helmReply""#));
        assert!(x.contains(r#"content="Mark &quot;read&quot;" arguments="helm:action:read""#));
        assert!(x.ends_with(r#"<audio silent="true"/></toast>"#));
        // Reply input must come before the actions (schema order).
        assert!(x.find("<input").unwrap() < x.find("<action ").unwrap());
    }

    #[test]
    fn validation() {
        let mut t = base();
        t.title = "  ".into();
        assert!(toast_xml(&t).is_err());
        let mut t = base();
        t.actions = (0..6).map(|i| ToastAction { id: i.to_string(), label: "x".into() }).collect();
        assert!(toast_xml(&t).is_err());
        let mut t = base();
        t.actions = vec![ToastAction { id: "a".into(), label: "x".into() }; 2];
        assert!(toast_xml(&t).is_err());
        let mut t = base();
        t.actions = vec![ToastAction { id: "".into(), label: "x".into() }];
        assert!(toast_xml(&t).is_err());
        let mut t = base();
        t.image = Some(PathBuf::from("relative.png"));
        assert!(toast_xml(&t).is_err());
    }

    #[test]
    fn serde_shapes() {
        let t: Toast = serde_json::from_str(r#"{"title":"T","actions":[{"id":"ok","label":"OK"}]}"#).unwrap();
        assert_eq!(t.actions[0].id, "ok");
        assert!(t.reply.is_none() && !t.silent);
        let e = ToastEvent::Activated { action_id: Some("ok".into()), reply: None };
        assert_eq!(serde_json::to_string(&e).unwrap(), r#"{"event":"activated","action_id":"ok","reply":null}"#);
        let d = ToastEvent::Dismissed { reason: DismissReason::TimedOut };
        assert_eq!(serde_json::to_string(&d).unwrap(), r#"{"event":"dismissed","reason":"timed_out"}"#);
    }
}
