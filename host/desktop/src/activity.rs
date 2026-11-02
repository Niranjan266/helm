//! Audit-log lines (`audit.log`) as rows for the Activity page and the Home log.

use std::collections::HashMap;

use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Entry {
    /// RFC 3339 UTC time as written by the agent.
    pub time: String,
    pub event: String,
    pub device_id: Option<String>,
    /// Device name when the device is (still) paired.
    pub device_name: Option<String>,
    pub peer: Option<String>,
    pub detail: String,
    /// One readable sentence.
    pub text: String,
}

fn field(s: Option<&str>) -> Option<String> {
    s.filter(|v| !v.is_empty() && *v != "-").map(str::to_string)
}

/// Parse `<time>\t<event>\t<device or ->\t<peer or ->[\t<detail>]`. `None` for junk lines.
pub fn parse_line(line: &str, names: &HashMap<String, String>) -> Option<Entry> {
    let mut parts = line.splitn(5, '\t');
    let time = parts.next()?.trim().to_string();
    let event = parts.next()?.trim().to_string();
    if time.is_empty() || event.is_empty() {
        return None;
    }
    let device_id = field(parts.next());
    let peer = field(parts.next());
    let detail = parts.next().unwrap_or("").trim().to_string();
    let device_name = device_id.as_ref().and_then(|d| names.get(d).cloned());
    let text = describe(&event, device_name.as_deref(), &detail);
    Some(Entry {
        time,
        event,
        device_id,
        device_name,
        peer,
        detail,
        text,
    })
}

/// A short sentence for an audit event.
pub fn describe(event: &str, name: Option<&str>, detail: &str) -> String {
    let who = name.unwrap_or("A phone");
    let reason = |d: &str| d.replace('_', " ");
    match event {
        "pair_ok" => match detail.strip_prefix("name=") {
            Some(n) if !n.is_empty() => format!("Paired {n}"),
            _ => format!("Paired {who}"),
        },
        "auth_ok" => format!("{who} connected"),
        "pair_fail" => format!("Pairing refused ({})", reason(detail)),
        "auth_fail" => format!("Connection refused ({})", reason(detail)),
        "revoke" => format!("Removed {who}"),
        "set_tier" => match detail.split_whitespace().find_map(|p| p.strip_prefix("tier=")) {
            Some(t) => format!("{who} is now {t}"),
            None => format!("Changed access for {who}"),
        },
        "handshake_timeout" => "A connection timed out before signing in".into(),
        "relay_up" => "Relay connected".into(),
        "relay_down" => "Relay disconnected".into(),
        "relay_set" => "Relay settings changed".into(),
        "power_run" => format!("Scheduled power action: {detail}"),
        other if detail.is_empty() => reason(other),
        other => format!("{} ({detail})", reason(other)),
    }
}

/// Parse many lines, newest first.
pub fn parse_lines(lines: &[String], names: &HashMap<String, String>) -> Vec<Entry> {
    lines
        .iter()
        .rev()
        .filter_map(|l| parse_line(l, names))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> HashMap<String, String> {
        HashMap::from([("dev1".to_string(), "Pixel 8".to_string())])
    }

    #[test]
    fn parses_full_and_short_lines() {
        let e = parse_line(
            "2026-11-01T09:41:00Z\tauth_ok\tdev1\t192.168.0.42:5555",
            &names(),
        )
        .unwrap();
        assert_eq!(e.device_name.as_deref(), Some("Pixel 8"));
        assert_eq!(e.peer.as_deref(), Some("192.168.0.42:5555"));
        assert_eq!(e.text, "Pixel 8 connected");
        assert_eq!(e.detail, "");

        let e = parse_line(
            "2026-11-01T09:40:00Z\tpair_fail\t-\t-\tbad_token",
            &names(),
        )
        .unwrap();
        assert_eq!((e.device_id, e.peer), (None, None));
        assert_eq!(e.text, "Pairing refused (bad token)");
        assert!(parse_line("", &names()).is_none());
        assert!(parse_line("only-time", &names()).is_none());
    }

    #[test]
    fn descriptions() {
        assert_eq!(describe("pair_ok", None, "name=Galaxy Tab"), "Paired Galaxy Tab");
        assert_eq!(describe("revoke", Some("Pixel 8"), "by=desktop"), "Removed Pixel 8");
        assert_eq!(
            describe("set_tier", Some("Pixel 8"), "tier=viewer by=desktop"),
            "Pixel 8 is now viewer"
        );
        assert_eq!(describe("auth_ok", None, ""), "A phone connected");
        assert_eq!(describe("file_saved", None, ""), "file saved");
        assert_eq!(describe("file_saved", None, "x.txt"), "file saved (x.txt)");
    }

    #[test]
    fn newest_first() {
        let lines = vec![
            "t1\tauth_ok\tdev1\t-".to_string(),
            "junk".to_string(),
            "t2\trevoke\tdev1\tdesktop\tby=desktop".to_string(),
        ];
        let rows = parse_lines(&lines, &names());
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].time, "t2");
    }
}
