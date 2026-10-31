//! Append-only audit log (audit.log): one tab-separated line per security event.
//!
//! Line format: `<RFC 3339 UTC time>\t<event>\t<device_id or ->\t<peer address or ->[\t<detail>]`

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::util;

pub const AUDIT_FILE: &str = "audit.log";

pub struct AuditLog {
    path: PathBuf,
    lock: Mutex<()>,
}

fn clean(field: &str) -> String {
    let s: String = field
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(200)
        .collect();
    if s.is_empty() { "-".to_string() } else { s }
}

impl AuditLog {
    pub fn new(data_dir: &Path) -> AuditLog {
        AuditLog {
            path: data_dir.join(AUDIT_FILE),
            lock: Mutex::new(()),
        }
    }

    /// Record an event. Failures to write are logged, never fatal.
    pub fn record(&self, event: &str, device_id: Option<&str>, peer: Option<&str>, detail: &str) {
        let mut line = format!(
            "{}\t{}\t{}\t{}",
            util::now_rfc3339(),
            clean(event),
            clean(device_id.unwrap_or("")),
            clean(peer.unwrap_or(""))
        );
        if !detail.is_empty() {
            line.push('\t');
            line.push_str(&clean(detail));
        }
        line.push('\n');
        let _guard = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let result = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .and_then(|mut f| f.write_all(line.as_bytes()));
        if let Err(e) = result {
            tracing::warn!("could not write audit log {}: {e}", self.path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_lines() {
        let dir = tempfile::tempdir().unwrap();
        let log = AuditLog::new(dir.path());
        log.record("pair_ok", Some("dev"), Some("1.2.3.4:5"), "name=Pixel");
        log.record("auth_fail", None, None, "bad\nline");
        let text = std::fs::read_to_string(dir.path().join(AUDIT_FILE)).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\tpair_ok\tdev\t1.2.3.4:5\tname=Pixel"));
        assert!(lines[1].contains("\tauth_fail\t-\t-\tbad line"));
    }
}
