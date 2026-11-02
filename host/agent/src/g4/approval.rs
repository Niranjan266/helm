//! "Ask before a new phone pairs" (g4): the setting (`pairing.json`) and pending requests.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::util;

pub const SETTINGS_FILE: &str = "pairing.json";
/// How long a pairing waits for Allow / Deny on the PC.
pub const APPROVAL_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingSettings {
    /// Ask on the PC (toast / desktop app / console) before a new phone is stored.
    #[serde(default)]
    pub require_approval: bool,
}

/// A pairing that waits for the PC owner.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PendingPairing {
    pub request_id: u64,
    pub device_id: String,
    pub device_name: String,
    /// Peer address (`ip:port`).
    pub peer: String,
    /// `"qr"` or `"code"`.
    pub method: String,
    pub expires_unix_ms: i64,
}

/// How a request ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Allowed,
    Denied,
    Timeout,
    /// The phone went away while waiting.
    Cancelled,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Allowed => "allowed",
            Outcome::Denied => "denied",
            Outcome::Timeout => "timeout",
            Outcome::Cancelled => "cancelled",
        }
    }
}

pub struct Approvals {
    path: PathBuf,
    settings: Mutex<PairingSettings>,
    pending: Mutex<HashMap<u64, (PendingPairing, oneshot::Sender<bool>)>>,
    next: AtomicU64,
    pub timeout: Duration,
}

impl Approvals {
    pub fn new(data_dir: &Path, timeout: Duration) -> Approvals {
        let path = data_dir.join(SETTINGS_FILE);
        let settings = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Approvals {
            path,
            settings: Mutex::new(settings),
            pending: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            timeout,
        }
    }

    pub fn settings(&self) -> PairingSettings {
        self.settings
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub fn require_approval(&self) -> bool {
        self.settings().require_approval
    }

    /// Save and apply the setting.
    pub fn set_require_approval(&self, on: bool) -> anyhow::Result<PairingSettings> {
        let mut s = self.settings.lock().unwrap_or_else(|p| p.into_inner());
        let next = PairingSettings {
            require_approval: on,
        };
        util::atomic_write(&self.path, serde_json::to_string_pretty(&next)?.as_bytes())?;
        *s = next.clone();
        Ok(next)
    }

    /// Register a request; the receiver gets `true` (allow) or `false` (deny).
    pub fn begin(
        &self,
        device_id: &str,
        device_name: &str,
        peer: &str,
        method: &str,
    ) -> (PendingPairing, oneshot::Receiver<bool>) {
        let request_id = self.next.fetch_add(1, Ordering::Relaxed);
        let info = PendingPairing {
            request_id,
            device_id: device_id.to_string(),
            device_name: device_name.to_string(),
            peer: peer.to_string(),
            method: method.to_string(),
            expires_unix_ms: util::now_ms() + self.timeout.as_millis() as i64,
        };
        let (tx, rx) = oneshot::channel();
        self.lock().insert(request_id, (info.clone(), tx));
        (info, rx)
    }

    /// Allow or deny a waiting request. False if it is no longer waiting.
    pub fn resolve(&self, request_id: u64, allow: bool) -> bool {
        match self.lock().remove(&request_id) {
            Some((_, tx)) => tx.send(allow).is_ok(),
            None => false,
        }
    }

    /// Drop a request (timed out / phone gone).
    pub fn forget(&self, request_id: u64) {
        self.lock().remove(&request_id);
    }

    /// Requests waiting now, oldest first.
    pub fn pending(&self) -> Vec<PendingPairing> {
        let mut v: Vec<PendingPairing> = self.lock().values().map(|(p, _)| p.clone()).collect();
        v.sort_by_key(|p| p.request_id);
        v
    }

    fn lock(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<u64, (PendingPairing, oneshot::Sender<bool>)>> {
        self.pending.lock().unwrap_or_else(|p| p.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setting_persists() {
        let dir = tempfile::tempdir().unwrap();
        let a = Approvals::new(dir.path(), APPROVAL_TIMEOUT);
        assert!(!a.require_approval());
        a.set_require_approval(true).unwrap();
        assert!(Approvals::new(dir.path(), APPROVAL_TIMEOUT).require_approval());
    }

    #[tokio::test]
    async fn resolve_once() {
        let dir = tempfile::tempdir().unwrap();
        let a = Approvals::new(dir.path(), APPROVAL_TIMEOUT);
        let (info, rx) = a.begin("d", "Pixel", "1.2.3.4:5", "qr");
        assert_eq!(a.pending(), vec![info.clone()]);
        assert!(a.resolve(info.request_id, false));
        assert!(!a.resolve(info.request_id, true));
        assert!(!rx.await.unwrap());
        assert!(a.pending().is_empty());
        let (info, _rx) = a.begin("d", "Pixel", "p", "code");
        a.forget(info.request_id);
        assert!(!a.resolve(info.request_id, true));
    }
}
