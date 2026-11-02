//! Clipboard history (everyday.md §1): the last [`MAX_ITEMS`] items, in memory, optionally
//! persisted DPAPI-encrypted in `clipboard-history.bin`. Opt-in (`everyday.json`).

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::lock;
use crate::util;

pub const MAX_ITEMS: usize = 20;
pub const MAX_TEXT_BYTES: usize = 1024 * 1024;
pub const MAX_PNG_BYTES: usize = 8 * 1024 * 1024;
pub const PREVIEW_CHARS: usize = 2000;
pub const SETTINGS_FILE: &str = "everyday.json";
pub const HISTORY_FILE: &str = "clipboard-history.bin";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub persist: bool,
}

#[derive(Default, Serialize, Deserialize)]
struct SettingsFile {
    #[serde(default)]
    clipboard: ClipSettings,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Content {
    Text(String),
    /// Full PNG (base64 in the persisted file).
    Image(#[serde(with = "b64")] Vec<u8>),
}

mod b64 {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&crate::util::b64_encode(v))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        crate::util::b64_decode(&s).ok_or_else(|| serde::de::Error::custom("bad base64"))
    }
}

#[derive(Clone, Debug)]
pub struct Item {
    pub id: u64,
    pub time_ms: u64,
    pub content: Content,
    /// Thumbnail + full size, for images.
    pub thumb: Option<(Vec<u8>, u32, u32)>,
}

#[derive(Serialize, Deserialize)]
struct Persisted {
    time_ms: u64,
    content: Content,
}

struct Inner {
    settings: ClipSettings,
    items: VecDeque<Item>,
    next_id: u64,
}

pub struct ClipHistory {
    dir: PathBuf,
    inner: Mutex<Inner>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn make_item(id: u64, time_ms: u64, content: Content) -> Item {
    let thumb = match &content {
        Content::Image(png) => super::thumb::thumbnail(png, super::thumb::THUMB_MAX),
        Content::Text(_) => None,
    };
    Item {
        id,
        time_ms,
        content,
        thumb,
    }
}

impl ClipHistory {
    pub fn load(dir: &Path) -> ClipHistory {
        let settings = std::fs::read(dir.join(SETTINGS_FILE))
            .ok()
            .and_then(|b| serde_json::from_slice::<SettingsFile>(&b).ok())
            .map(|f| f.clipboard)
            .unwrap_or_default();
        let mut items = VecDeque::new();
        let mut next_id = 1;
        if settings.enabled && settings.persist {
            match read_persisted(&dir.join(HISTORY_FILE)) {
                Ok(list) => {
                    for p in list.into_iter().take(MAX_ITEMS) {
                        items.push_back(make_item(next_id, p.time_ms, p.content));
                        next_id += 1;
                    }
                }
                Err(e) => tracing::warn!("{HISTORY_FILE} unreadable: {e:#}"),
            }
        }
        ClipHistory {
            dir: dir.to_path_buf(),
            inner: Mutex::new(Inner {
                settings,
                items,
                next_id,
            }),
        }
    }

    pub fn settings(&self) -> ClipSettings {
        lock(&self.inner).settings
    }

    /// Change the settings (saved). Turning `enabled` off clears the list; turning `persist`
    /// (or `enabled`) off deletes the encrypted file.
    pub fn set_settings(&self, enabled: Option<bool>, persist: Option<bool>) -> anyhow::Result<()> {
        let mut g = lock(&self.inner);
        if let Some(e) = enabled {
            g.settings.enabled = e;
        }
        if let Some(p) = persist {
            g.settings.persist = p;
        }
        if !g.settings.enabled {
            g.items.clear();
        }
        let file = SettingsFile {
            clipboard: g.settings,
        };
        util::atomic_write(
            &self.dir.join(SETTINGS_FILE),
            serde_json::to_string_pretty(&file)?.as_bytes(),
        )?;
        self.save_locked(&g)
    }

    /// Record new clipboard content. Returns true when the list changed.
    pub fn push(&self, content: Content) -> bool {
        let ok_size = match &content {
            Content::Text(t) => !t.is_empty() && t.len() <= MAX_TEXT_BYTES,
            Content::Image(p) => !p.is_empty() && p.len() <= MAX_PNG_BYTES,
        };
        if !ok_size {
            return false;
        }
        let mut g = lock(&self.inner);
        if !g.settings.enabled {
            return false;
        }
        let now = now_ms();
        if let Some(first) = g.items.front_mut()
            && first.content == content
        {
            first.time_ms = now;
        } else {
            let id = g.next_id;
            g.next_id += 1;
            g.items.retain(|i| i.content != content);
            g.items.push_front(make_item(id, now, content));
            g.items.truncate(MAX_ITEMS);
        }
        if let Err(e) = self.save_locked(&g) {
            tracing::warn!("saving {HISTORY_FILE}: {e:#}");
        }
        true
    }

    pub fn get(&self, id: u64) -> Option<Item> {
        lock(&self.inner).items.iter().find(|i| i.id == id).cloned()
    }

    /// Move an item to the top (after it was copied on the PC).
    pub fn touch(&self, id: u64) -> bool {
        let mut g = lock(&self.inner);
        let Some(pos) = g.items.iter().position(|i| i.id == id) else {
            return false;
        };
        let mut item = g.items.remove(pos).expect("position is valid");
        item.time_ms = now_ms();
        g.items.push_front(item);
        let _ = self.save_locked(&g);
        true
    }

    pub fn delete(&self, id: u64) -> bool {
        let mut g = lock(&self.inner);
        let before = g.items.len();
        g.items.retain(|i| i.id != id);
        let changed = g.items.len() != before;
        if changed {
            let _ = self.save_locked(&g);
        }
        changed
    }

    pub fn clear(&self) {
        let mut g = lock(&self.inner);
        g.items.clear();
        let _ = self.save_locked(&g);
    }

    pub fn len(&self) -> usize {
        lock(&self.inner).items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// `{enabled, persist, items}` for the phone.
    pub fn to_json(&self) -> Value {
        let g = lock(&self.inner);
        let items: Vec<Value> = g.items.iter().map(item_json).collect();
        json!({"enabled": g.settings.enabled, "persist": g.settings.persist, "items": items})
    }

    fn save_locked(&self, g: &Inner) -> anyhow::Result<()> {
        let path = self.dir.join(HISTORY_FILE);
        if !(g.settings.enabled && g.settings.persist) {
            match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                _ => return Ok(()),
            }
        }
        let list: Vec<Persisted> = g
            .items
            .iter()
            .map(|i| Persisted {
                time_ms: i.time_ms,
                content: i.content.clone(),
            })
            .collect();
        let plain = serde_json::to_vec(&list)?;
        let sealed = crate::dpapi::protect(&plain)?;
        util::atomic_write(&path, &sealed)?;
        Ok(())
    }
}

fn read_persisted(path: &Path) -> anyhow::Result<Vec<Persisted>> {
    let sealed = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let plain = crate::dpapi::unprotect(&sealed)?;
    Ok(serde_json::from_slice(&plain)?)
}

fn item_json(i: &Item) -> Value {
    match &i.content {
        Content::Text(t) => {
            let truncated = t.chars().count() > PREVIEW_CHARS;
            let mut v = json!({
                "id": i.id, "kind": "text", "time_ms": i.time_ms, "size": t.len(),
                "text": t.chars().take(PREVIEW_CHARS).collect::<String>(),
            });
            if truncated {
                v["truncated"] = Value::Bool(true);
            }
            v
        }
        Content::Image(png) => {
            let mut v = json!({"id": i.id, "kind": "image", "time_ms": i.time_ms, "size": png.len()});
            if let Some((thumb, w, h)) = &i.thumb {
                v["thumb_png"] = Value::String(util::b64_encode(thumb));
                v["width"] = json!(w);
                v["height"] = json!(h);
            }
            v
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Content {
        Content::Text(s.into())
    }

    #[test]
    fn off_by_default_and_records_only_when_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let h = ClipHistory::load(dir.path());
        assert_eq!(h.settings(), ClipSettings::default());
        assert!(!h.push(text("a")));
        assert!(h.is_empty());
        h.set_settings(Some(true), None).unwrap();
        assert!(h.push(text("a")));
        assert_eq!(h.len(), 1);
    }

    #[test]
    fn keeps_last_twenty_newest_first_and_dedups() {
        let dir = tempfile::tempdir().unwrap();
        let h = ClipHistory::load(dir.path());
        h.set_settings(Some(true), None).unwrap();
        for i in 0..25 {
            h.push(text(&format!("item {i}")));
        }
        let v = h.to_json();
        let items = v["items"].as_array().unwrap();
        assert_eq!(items.len(), MAX_ITEMS);
        assert_eq!(items[0]["text"], "item 24");
        assert_eq!(items[19]["text"], "item 5");
        // Same as newest: only the time changes.
        h.push(text("item 24"));
        assert_eq!(h.len(), MAX_ITEMS);
        // An older duplicate moves to the top with a new id.
        let old_id = items[3]["id"].as_u64().unwrap();
        h.push(text("item 21"));
        let v = h.to_json();
        assert_eq!(v["items"][0]["text"], "item 21");
        assert!(h.get(old_id).is_none());
        assert_eq!(h.len(), MAX_ITEMS);
    }

    #[test]
    fn previews_and_limits() {
        let dir = tempfile::tempdir().unwrap();
        let h = ClipHistory::load(dir.path());
        h.set_settings(Some(true), None).unwrap();
        assert!(!h.push(text("")));
        assert!(!h.push(Content::Text("x".repeat(MAX_TEXT_BYTES + 1))));
        h.push(Content::Text("é".repeat(PREVIEW_CHARS + 5)));
        let v = h.to_json();
        assert_eq!(v["items"][0]["truncated"], true);
        assert_eq!(v["items"][0]["text"].as_str().unwrap().chars().count(), PREVIEW_CHARS);
        assert_eq!(v["items"][0]["size"], (PREVIEW_CHARS + 5) * 2);
        h.push(Content::Image(super::super::thumb::test_png(300, 150)));
        let v = h.to_json();
        assert_eq!(v["items"][0]["kind"], "image");
        assert_eq!(v["items"][0]["width"], 300);
        assert!(v["items"][0]["thumb_png"].is_string());
    }

    #[test]
    fn persist_is_encrypted_reloaded_and_deleted_when_off() {
        let dir = tempfile::tempdir().unwrap();
        let h = ClipHistory::load(dir.path());
        h.set_settings(Some(true), Some(true)).unwrap();
        h.push(text("secret-ish note"));
        h.push(Content::Image(crate::platform::fake::TINY_PNG.to_vec()));
        let file = dir.path().join(HISTORY_FILE);
        let raw = std::fs::read(&file).unwrap();
        if crate::dpapi::SCHEME == "dpapi" {
            assert!(!String::from_utf8_lossy(&raw).contains("secret-ish"), "encrypted at rest");
        }
        let again = ClipHistory::load(dir.path());
        assert_eq!(again.len(), 2);
        assert_eq!(again.to_json()["items"][1]["text"], "secret-ish note");
        again.set_settings(None, Some(false)).unwrap();
        assert!(!file.exists());
        assert_eq!(again.len(), 2, "persist off keeps the in-memory list");
        again.set_settings(Some(false), None).unwrap();
        assert!(again.is_empty(), "disabling clears the list");
        assert!(ClipHistory::load(dir.path()).is_empty());
    }

    #[test]
    fn touch_delete_clear() {
        let dir = tempfile::tempdir().unwrap();
        let h = ClipHistory::load(dir.path());
        h.set_settings(Some(true), None).unwrap();
        h.push(text("a"));
        h.push(text("b"));
        let a = h.to_json()["items"][1]["id"].as_u64().unwrap();
        assert!(h.touch(a));
        assert_eq!(h.to_json()["items"][0]["text"], "a");
        assert!(h.delete(a));
        assert!(!h.delete(a));
        assert!(!h.touch(999));
        h.clear();
        assert!(h.is_empty());
    }
}
