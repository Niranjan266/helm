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
    /// Phase 3: run actions in order (at most [`MAX_MACRO_STEPS`] steps in all).
    Macro(Vec<DeckAction>),
    /// Phase 3: run one action `times` times.
    Repeat(Repeat),
    /// Phase 3: PowerPoint / browser / OBS remote command (see [`REMOTE_ACTIONS`]).
    Remote(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Repeat {
    pub times: u32,
    pub action: Box<DeckAction>,
}

/// Most leaf actions one button may run (macros and repeats expanded).
pub const MAX_MACRO_STEPS: usize = 50;
const MAX_NESTING: usize = 4;

/// Fixed `remote` commands; `obs.scene:<name>` is also accepted.
pub const REMOTE_ACTIONS: &[&str] = &[
    "powerpoint.next",
    "powerpoint.prev",
    "powerpoint.start",
    "powerpoint.end",
    "browser.back",
    "browser.forward",
    "browser.tab_next",
    "browser.tab_prev",
    "browser.reload",
    "obs.toggle_record",
    "obs.toggle_stream",
];

/// Hotkey sent for a keyboard `remote` command (None = not a keyboard command).
pub fn remote_hotkey(name: &str) -> Option<&'static str> {
    Some(match name {
        "powerpoint.next" => "right",
        "powerpoint.prev" => "left",
        "powerpoint.start" => "f5",
        "powerpoint.end" => "esc",
        "browser.back" => "alt+left",
        "browser.forward" => "alt+right",
        "browser.tab_next" => "ctrl+tab",
        "browser.tab_prev" => "ctrl+shift+tab",
        "browser.reload" => "f5",
        _ => return None,
    })
}

/// Live state a button shows (`deck.state` events).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Bind {
    MicMuted,
    AudioMuted,
    MediaPlaying,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateBinding {
    pub bind: Bind,
}

/// Live text a button shows (`deck.widget` events).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Widget {
    Cpu,
    Ram,
    Clock,
    NowPlaying,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeckButton {
    pub id: String,
    pub label: String,
    pub color: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub action: DeckAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<StateBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub widget: Option<Widget>,
}

/// Check one action (recursively); returns the number of leaf steps it runs.
fn validate_action(action: &DeckAction, depth: usize) -> Result<usize, String> {
    if depth > MAX_NESTING {
        return Err(format!("actions nest at most {MAX_NESTING} levels deep"));
    }
    match action {
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
        DeckAction::Remote(name) => {
            let ok = REMOTE_ACTIONS.contains(&name.as_str())
                || name
                    .strip_prefix("obs.scene:")
                    .is_some_and(|scene| check_len("scene", scene, 256, false).is_ok());
            if !ok {
                return Err(format!("unknown remote action {name:?}"));
            }
        }
        DeckAction::Macro(steps) => {
            if steps.is_empty() {
                return Err("macro must not be empty".into());
            }
            let mut total = 0usize;
            for s in steps {
                total += validate_action(s, depth + 1)?;
                if total > MAX_MACRO_STEPS {
                    return Err(format!("at most {MAX_MACRO_STEPS} steps per button"));
                }
            }
            return Ok(total);
        }
        DeckAction::Repeat(r) => {
            if r.times == 0 || r.times as usize > MAX_MACRO_STEPS {
                return Err(format!("repeat times must be 1..={MAX_MACRO_STEPS}"));
            }
            let total = validate_action(&r.action, depth + 1)? * r.times as usize;
            if total > MAX_MACRO_STEPS {
                return Err(format!("at most {MAX_MACRO_STEPS} steps per button"));
            }
            return Ok(total);
        }
    }
    Ok(1)
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
        validate_action(&b.action, 0)?;
    }
    Ok(())
}

/// Dock + deck files with a lock so concurrent `set`s don't interleave.
pub struct LayoutStore {
    dir: PathBuf,
    lock: Mutex<()>,
}

pub(crate) fn read_json<T: DeserializeOwned>(path: &Path) -> anyhow::Result<Option<T>> {
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
    use serde_json::{Value, json};

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
            state: None,
            widget: None,
        };
        assert!(validate_buttons(&[b(DeckAction::Hotkey("ctrl+c".into()))]).is_ok());
        assert!(validate_buttons(&[b(DeckAction::Hotkey("ctrl+nope".into()))]).is_err());
        assert!(validate_buttons(&[b(DeckAction::VolumeStep(3.0))]).is_err());
        assert!(validate_buttons(&[b(DeckAction::DelayMs(MAX_DELAY_MS + 1))]).is_err());
    }

    #[test]
    fn phase3_actions_validate() {
        let b = |action: Value| {
            let v = json!({"id":"b","label":"B","color":"#000000","action":action});
            serde_json::from_value::<DeckButton>(v).map(|b| validate_buttons(&[b]))
        };
        let ok = |a: Value| assert!(matches!(b(a.clone()), Ok(Ok(()))), "{a}");
        let bad = |a: Value| assert!(!matches!(b(a.clone()), Ok(Ok(()))), "{a}");
        ok(json!({"macro":[{"hotkey":"ctrl+c"},{"delay_ms":100},{"text":"x"}]}));
        ok(json!({"repeat":{"times":5,"action":{"volume_step":0.02}}}));
        ok(json!({"remote":"powerpoint.next"}));
        ok(json!({"remote":"obs.scene:Gaming"}));
        bad(json!({"remote":"obs.scene:"}));
        bad(json!({"remote":"rm -rf"}));
        bad(json!({"macro":[]}));
        bad(json!({"repeat":{"times":0,"action":{"text":"x"}}}));
        // 51 expanded steps
        bad(json!({"macro":[{"repeat":{"times":50,"action":{"text":"x"}}},{"text":"y"}]}));
        let deep = json!({"macro":[{"macro":[{"macro":[{"macro":[{"macro":[{"text":"x"}]}]}]}]}]});
        bad(deep);
        for name in REMOTE_ACTIONS {
            if let Some(k) = remote_hotkey(name) {
                assert!(helm_win_input::parse_shortcut(k).is_some(), "{k}");
            }
        }
        let with_state = json!({"id":"m","label":"Mic","color":"#000000",
            "action":{"hotkey":"ctrl+shift+m"},"state":{"bind":"mic_muted"},"widget":"cpu"});
        let parsed: DeckButton = serde_json::from_value(with_state.clone()).unwrap();
        assert_eq!(
            parsed.state,
            Some(StateBinding {
                bind: Bind::MicMuted
            })
        );
        assert_eq!(serde_json::to_value(&parsed).unwrap(), with_state);
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
