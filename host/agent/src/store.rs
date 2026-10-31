//! Dock and deck layouts (`dock.json`, `deck.json` in the data folder), validated and
//! written atomically.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::platform::MediaAction;
use crate::util;

pub const DOCK_FILE: &str = "dock.json";
pub const DECK_FILE: &str = "deck.json";

const MAX_PAGES: usize = 32;
const MAX_TILES_PER_PAGE: usize = 64;
const MAX_BUTTONS: usize = 256;
const MAX_ID: usize = 128;
const MAX_TEXT: usize = 4096;
pub const MAX_DELAY_MS: u64 = 60_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TileKind {
    App,
    Open,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tile {
    pub id: String,
    pub kind: TileKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// A deck button's single action (PROTOCOL §9.3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeckAction {
    Hotkey(String),
    Text(String),
    Launch(String),
    Open(String),
    Media(MediaAction),
    VolumeStep(f64),
    DelayMs(u64),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeckButton {
    pub id: String,
    pub label: String,
    pub color: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub action: DeckAction,
}

#[derive(Serialize, Deserialize)]
struct DockFile {
    version: u32,
    pages: Vec<Vec<Tile>>,
}

#[derive(Serialize, Deserialize)]
struct DeckFile {
    version: u32,
    buttons: Vec<DeckButton>,
}

fn check_len(what: &str, s: &str, max: usize, allow_empty: bool) -> Result<(), String> {
    if !allow_empty && s.trim().is_empty() {
        return Err(format!("{what} must not be empty"));
    }
    if s.len() > max {
        return Err(format!("{what} is longer than {max} bytes"));
    }
    if s.chars().any(|c| c.is_control() && c != '\n' && c != '\t') {
        return Err(format!("{what} contains control characters"));
    }
    Ok(())
}

pub fn validate_pages(pages: &[Vec<Tile>]) -> Result<(), String> {
    if pages.len() > MAX_PAGES {
        return Err(format!("at most {MAX_PAGES} pages"));
    }
    let mut ids = std::collections::HashSet::new();
    for page in pages {
        if page.len() > MAX_TILES_PER_PAGE {
            return Err(format!("at most {MAX_TILES_PER_PAGE} tiles per page"));
        }
        for t in page {
            check_len("tile id", &t.id, MAX_ID, false)?;
            if !ids.insert(t.id.as_str()) {
                return Err(format!("duplicate tile id {:?}", t.id));
            }
            if let Some(l) = &t.label {
                check_len("label", l, 200, true)?;
            }
            match t.kind {
                TileKind::App => {
                    let a = t.app_id.as_deref().ok_or("app tile needs app_id")?;
                    check_len("app_id", a, 1024, false)?;
                }
                TileKind::Open => {
                    let target = t.target.as_deref().ok_or("open tile needs target")?;
                    check_len("target", target, MAX_TEXT, false)?;
                }
            }
        }
    }
    Ok(())
}

pub fn validate_buttons(buttons: &[DeckButton]) -> Result<(), String> {
    if buttons.len() > MAX_BUTTONS {
        return Err(format!("at most {MAX_BUTTONS} buttons"));
    }
    let mut ids = std::collections::HashSet::new();
    for b in buttons {
        check_len("button id", &b.id, MAX_ID, false)?;
        if !ids.insert(b.id.as_str()) {
            return Err(format!("duplicate button id {:?}", b.id));
        }
        check_len("label", &b.label, 200, true)?;
        check_len("color", &b.color, 32, true)?;
        if let Some(i) = &b.icon {
            check_len("icon", i, 64, true)?;
        }
        match &b.action {
            DeckAction::Hotkey(k) => {
                if helm_win_input::parse_shortcut(k).is_none() {
                    return Err(format!("unknown hotkey {k:?}"));
                }
            }
            DeckAction::Text(t) => check_len("text", t, MAX_TEXT, false)?,
            DeckAction::Launch(a) => check_len("launch app_id", a, 1024, false)?,
            DeckAction::Open(t) => check_len("open target", t, MAX_TEXT, false)?,
            DeckAction::Media(_) => {}
            DeckAction::VolumeStep(s) => {
                if !s.is_finite() || !(-1.0..=1.0).contains(s) {
                    return Err("volume_step must be between -1 and 1".into());
                }
            }
            DeckAction::DelayMs(ms) => {
                if *ms > MAX_DELAY_MS {
                    return Err(format!("delay_ms must be at most {MAX_DELAY_MS}"));
                }
            }
        }
    }
    Ok(())
}

/// Dock + deck files with a lock so concurrent `set`s don't interleave.
pub struct LayoutStore {
    dir: PathBuf,
    lock: Mutex<()>,
}

fn read_json<T: DeserializeOwned>(path: &Path) -> anyhow::Result<Option<T>> {
    match std::fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(None),
        Ok(text) => Ok(Some(serde_json::from_str(&text)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

impl LayoutStore {
    pub fn new(dir: &Path) -> LayoutStore {
        LayoutStore {
            dir: dir.to_path_buf(),
            lock: Mutex::new(()),
        }
    }

    fn guard(&self) -> std::sync::MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn dock(&self) -> anyhow::Result<Vec<Vec<Tile>>> {
        let _g = self.guard();
        Ok(read_json::<DockFile>(&self.dir.join(DOCK_FILE))?
            .map(|f| f.pages)
            .unwrap_or_default())
    }

    pub fn set_dock(&self, pages: Vec<Vec<Tile>>) -> anyhow::Result<()> {
        let _g = self.guard();
        let file = DockFile { version: 1, pages };
        util::atomic_write(
            &self.dir.join(DOCK_FILE),
            serde_json::to_string_pretty(&file)?.as_bytes(),
        )?;
        Ok(())
    }

    pub fn deck(&self) -> anyhow::Result<Vec<DeckButton>> {
        let _g = self.guard();
        Ok(read_json::<DeckFile>(&self.dir.join(DECK_FILE))?
            .map(|f| f.buttons)
            .unwrap_or_default())
    }

    pub fn set_deck(&self, buttons: Vec<DeckButton>) -> anyhow::Result<()> {
        let _g = self.guard();
        let file = DeckFile {
            version: 1,
            buttons,
        };
        util::atomic_write(
            &self.dir.join(DECK_FILE),
            serde_json::to_string_pretty(&file)?.as_bytes(),
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn deck_action_shapes() {
        let cases = [
            json!({"hotkey":"ctrl+shift+esc"}),
            json!({"text":"hi"}),
            json!({"launch":"Fake.Notes"}),
            json!({"open":"https://example.com"}),
            json!({"media":"toggle_play_pause"}),
            json!({"media":{"seek_ms":5000}}),
            json!({"volume_step":0.05}),
            json!({"delay_ms":200}),
        ];
        for c in cases {
            let a: DeckAction = serde_json::from_value(c.clone()).unwrap();
            assert_eq!(serde_json::to_value(&a).unwrap(), c);
        }
        assert!(serde_json::from_value::<DeckAction>(json!({"format_disk":true})).is_err());
    }

    #[test]
    fn validation_rejects_bad_layouts() {
        let tile = |id: &str| Tile {
            id: id.into(),
            kind: TileKind::App,
            app_id: Some("x".into()),
            target: None,
            label: None,
        };
        assert!(validate_pages(&[vec![tile("a"), tile("b")]]).is_ok());
        assert!(validate_pages(&[vec![tile("a")], vec![tile("a")]]).is_err());
        let mut no_app = tile("c");
        no_app.app_id = None;
        assert!(validate_pages(&[vec![no_app]]).is_err());
        let b = |action| DeckButton {
            id: "b".into(),
            label: "B".into(),
            color: "#ff0000".into(),
            icon: None,
            action,
        };
        assert!(validate_buttons(&[b(DeckAction::Hotkey("ctrl+c".into()))]).is_ok());
        assert!(validate_buttons(&[b(DeckAction::Hotkey("ctrl+nope".into()))]).is_err());
        assert!(validate_buttons(&[b(DeckAction::VolumeStep(3.0))]).is_err());
        assert!(validate_buttons(&[b(DeckAction::DelayMs(MAX_DELAY_MS + 1))]).is_err());
    }

    #[test]
    fn store_round_trip_and_empty_default() {
        let dir = tempfile::tempdir().unwrap();
        let s = LayoutStore::new(dir.path());
        assert!(s.dock().unwrap().is_empty());
        assert!(s.deck().unwrap().is_empty());
        let pages = vec![vec![Tile {
            id: "t".into(),
            kind: TileKind::Open,
            app_id: None,
            target: Some("https://example.com".into()),
            label: Some("Ex".into()),
        }]];
        s.set_dock(pages.clone()).unwrap();
        assert_eq!(s.dock().unwrap(), pages);
    }
}
