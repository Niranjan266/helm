//! G5 (`Docs/protocol/dock-deck-2.md`): app usage stats (`usage.json`) for "smart order",
//! and custom deck/dock icons uploaded from the phone (`deck-icons\`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::platform::{ForegroundInfo, blocking};
use crate::router::RpcError;
use crate::server::AppState;
use crate::store::read_json;
use crate::util;

pub const USAGE_FILE: &str = "usage.json";
pub const ICON_DIR: &str = "deck-icons";
/// Most apps remembered; the lowest-ranked are forgotten first.
pub const MAX_USAGE_APPS: usize = 2000;
/// Largest uploaded icon (encoded bytes).
pub const MAX_ICON_BYTES: usize = 256 * 1024;
pub const MAX_ICONS: usize = 512;
/// A launch counts this many times more than bringing an app to the front.
const LAUNCH_WEIGHT: f64 = 3.0;
/// Usage counts halve every this many days without use.
const HALF_LIFE_DAYS: f64 = 14.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AppUsage {
    #[serde(default)]
    pub launches: u64,
    #[serde(default)]
    pub focus: u64,
    #[serde(default)]
    pub last_used_ms: u64,
}

/// Ranking score: weighted counts, decayed by time since last use.
pub fn score(u: &AppUsage, now_ms: u64) -> f64 {
    let base = LAUNCH_WEIGHT * u.launches as f64 + u.focus as f64;
    let age_days = now_ms.saturating_sub(u.last_used_ms) as f64 / 86_400_000.0;
    base * 0.5f64.powf(age_days / HALF_LIFE_DAYS)
}

/// Most used first (ties: most recent, then app id).
pub fn rank(apps: &HashMap<String, AppUsage>, now_ms: u64) -> Vec<(String, AppUsage, f64)> {
    let mut v: Vec<(String, AppUsage, f64)> = apps
        .iter()
        .map(|(id, u)| (id.clone(), *u, score(u, now_ms)))
        .collect();
    v.sort_by(|a, b| {
        b.2.total_cmp(&a.2)
            .then(b.1.last_used_ms.cmp(&a.1.last_used_ms))
            .then(a.0.cmp(&b.0))
    });
    v
}

#[derive(Default, Serialize, Deserialize)]
struct UsageFile {
    version: u32,
    #[serde(default)]
    apps: HashMap<String, AppUsage>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsageKind {
    Launch,
    Focus,
}

pub struct G5State {
    dir: PathBuf,
    usage: Mutex<Option<HashMap<String, AppUsage>>>,
    last_foreground_app: Mutex<Option<String>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl G5State {
    pub fn new(data_dir: &Path) -> G5State {
        G5State {
            dir: data_dir.to_path_buf(),
            usage: Mutex::new(None),
            last_foreground_app: Mutex::new(None),
        }
    }

    fn with_usage<R>(&self, f: impl FnOnce(&mut HashMap<String, AppUsage>) -> R) -> R {
        let mut g = lock(&self.usage);
        let map = g.get_or_insert_with(|| {
            match read_json::<UsageFile>(&self.dir.join(USAGE_FILE)) {
                Ok(f) => f.map(|f| f.apps).unwrap_or_default(),
                Err(e) => {
                    tracing::warn!("usage.json unreadable: {e:#}");
                    HashMap::new()
                }
            }
        });
        f(map)
    }

    /// Count one launch / foreground switch of `app_id` and save `usage.json`.
    pub fn record(&self, app_id: &str, kind: UsageKind, now_ms: u64) {
        if app_id.is_empty() || app_id.len() > 1024 {
            return;
        }
        let snapshot = self.with_usage(|m| {
            let u = m.entry(app_id.to_string()).or_default();
            match kind {
                UsageKind::Launch => u.launches += 1,
                UsageKind::Focus => u.focus += 1,
            }
            u.last_used_ms = u.last_used_ms.max(now_ms);
            if m.len() > MAX_USAGE_APPS {
                let ranked = rank(m, now_ms);
                for (id, _, _) in ranked.into_iter().skip(MAX_USAGE_APPS) {
                    m.remove(&id);
                }
            }
            m.clone()
        });
        let file = UsageFile {
            version: 1,
            apps: snapshot,
        };
        match serde_json::to_vec_pretty(&file) {
            Ok(bytes) => {
                if let Err(e) = util::atomic_write(&self.dir.join(USAGE_FILE), &bytes) {
                    tracing::warn!("saving usage.json: {e:#}");
                }
            }
            Err(e) => tracing::warn!("usage.json: {e}"),
        }
    }

    pub fn ranked(&self, now_ms: u64) -> Vec<(String, AppUsage, f64)> {
        self.with_usage(|m| rank(m, now_ms))
    }

    /// The foreground app changed: count a focus unless it is the same app as before.
    pub fn on_foreground_app(&self, app_id: Option<String>, now_ms: u64) {
        let mut last = lock(&self.last_foreground_app);
        if *last == app_id {
            return;
        }
        last.clone_from(&app_id);
        drop(last);
        if let Some(id) = app_id {
            self.record(&id, UsageKind::Focus, now_ms);
        }
    }

    // ---- custom icons

    fn icon_dir(&self) -> PathBuf {
        self.dir.join(ICON_DIR)
    }

    /// Store an uploaded PNG/JPEG; returns its icon reference `c:<32 hex>`.
    pub fn put_icon(&self, bytes: &[u8]) -> Result<String, String> {
        if bytes.is_empty() || bytes.len() > MAX_ICON_BYTES {
            return Err(format!("icon must be 1..={MAX_ICON_BYTES} bytes"));
        }
        let png = bytes.starts_with(b"\x89PNG\r\n\x1a\n");
        let jpeg = bytes.starts_with(&[0xFF, 0xD8, 0xFF]);
        if !png && !jpeg {
            return Err("icon must be a PNG or JPEG image".into());
        }
        let hash = hex::encode(Sha256::digest(bytes))[..32].to_string();
        let dir = self.icon_dir();
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(format!("{hash}.img"));
        if !path.exists() {
            let count = std::fs::read_dir(&dir).map(|r| r.count()).unwrap_or(0);
            if count >= MAX_ICONS {
                return Err(format!("at most {MAX_ICONS} custom icons"));
            }
            util::atomic_write(&path, bytes).map_err(|e| e.to_string())?;
        }
        Ok(format!("c:{hash}"))
    }

    /// Bytes of an uploaded icon (`None` = no such icon).
    pub fn get_icon(&self, icon: &str) -> Result<Option<Vec<u8>>, String> {
        let hash = parse_custom_icon(icon).ok_or_else(|| format!("{icon:?} is not a custom icon"))?;
        match std::fs::read(self.icon_dir().join(format!("{hash}.img"))) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// `c:<32 lowercase hex>` → the hex part.
pub fn parse_custom_icon(icon: &str) -> Option<&str> {
    let h = icon.strip_prefix("c:")?;
    (h.len() == 32 && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        .then_some(h)
}


fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// A launch from the phone (`apps.launch`, a dock tile or a deck button) succeeded.
pub async fn record_launch(state: &Arc<AppState>, app_id: &str) {
    let st = state.clone();
    let id = app_id.to_string();
    let _ = blocking(move || {
        st.g5.record(&id, UsageKind::Launch, now_ms());
        Ok::<(), RpcError>(())
    })
    .await;
}

/// From the Phase 3 foreground watcher: map the window to an app id and count a focus.
pub async fn on_foreground(state: &Arc<AppState>, info: &ForegroundInfo) {
    let st = state.clone();
    let hwnd = info.hwnd;
    let exe = info.exe.clone();
    let _ = blocking(move || {
        let windows = st.platform.list_windows().unwrap_or_default();
        let app_id = windows
            .iter()
            .find(|w| w.hwnd == hwnd)
            .and_then(|w| w.app_id.clone())
            .or_else(|| {
                let exe = exe?;
                st.platform.list_apps().ok()?.into_iter().find_map(|a| {
                    a.target_path
                        .as_deref()
                        .is_some_and(|t| t.eq_ignore_ascii_case(&exe))
                        .then_some(a.app_id)
                })
            });
        st.g5.on_foreground_app(app_id, now_ms());
        Ok::<(), RpcError>(())
    })
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 86_400_000;

    #[test]
    fn ranking_weighs_launches_and_decays_with_age() {
        let now = 100 * DAY;
        let mut m = HashMap::new();
        m.insert("old".to_string(), AppUsage { launches: 10, focus: 0, last_used_ms: now - 60 * DAY });
        m.insert("fresh".to_string(), AppUsage { launches: 2, focus: 1, last_used_ms: now });
        m.insert("focus".to_string(), AppUsage { launches: 0, focus: 6, last_used_ms: now });
        m.insert("tie".to_string(), AppUsage { launches: 0, focus: 6, last_used_ms: now - 1 });
        let order: Vec<String> = rank(&m, now).into_iter().map(|r| r.0).collect();
        // fresh = 7, focus = 6, tie ≈ 6 (older), old = 30 * 2^-(60/14) ≈ 1.5
        assert_eq!(order, ["fresh", "focus", "tie", "old"]);
        assert!((score(&m["fresh"], now) - 7.0).abs() < 1e-9);
        assert!((score(&m["old"], now) - 30.0 * 0.5f64.powf(60.0 / 14.0)).abs() < 1e-9);
    }

    #[test]
    fn record_persists_and_skips_repeat_foreground() {
        let dir = tempfile::tempdir().unwrap();
        let g = G5State::new(dir.path());
        g.record("A", UsageKind::Launch, 10);
        g.on_foreground_app(Some("A".into()), 20);
        g.on_foreground_app(Some("A".into()), 30); // same app: not counted
        g.on_foreground_app(None, 40);
        g.on_foreground_app(Some("A".into()), 50);
        let again = G5State::new(dir.path());
        let r = again.ranked(50);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].1, AppUsage { launches: 1, focus: 2, last_used_ms: 50 });
    }

    #[test]
    fn icons_round_trip_and_reject_junk() {
        let dir = tempfile::tempdir().unwrap();
        let g = G5State::new(dir.path());
        let png = b"\x89PNG\r\n\x1a\nfakepngdata".to_vec();
        let id = g.put_icon(&png).unwrap();
        assert!(parse_custom_icon(&id).is_some(), "{id}");
        assert_eq!(g.put_icon(&png).unwrap(), id);
        assert_eq!(g.get_icon(&id).unwrap(), Some(png));
        assert_eq!(g.get_icon("c:00000000000000000000000000000000").unwrap(), None);
        assert!(g.get_icon("c:../../etc").is_err());
        assert!(g.get_icon("b:play").is_err());
        assert!(g.put_icon(b"GIF89a").is_err());
        assert!(g.put_icon(&vec![0xFF; MAX_ICON_BYTES + 1]).is_err());
    }
}
