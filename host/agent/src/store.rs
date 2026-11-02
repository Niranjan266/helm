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
    /// G5 (`Docs/protocol/dock-deck-2.md`): a named group of app/open tiles.
    Folder,
}

/// Most tiles inside one dock folder.
pub const MAX_FOLDER_TILES: usize = 32;

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
    /// G5: icon reference (`b:<name>` built-in, `c:<hash>` uploaded via `deck.icon.put`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// G5: the tiles inside a `folder` tile (app/open only, no nesting).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<Tile>>,
}

/// G5: the dock as Phase 1 phones see it: folders replaced by their tiles, in place.
pub fn flatten_pages(pages: &[Vec<Tile>]) -> Vec<Vec<Tile>> {
    pages
        .iter()
        .map(|page| {
            page.iter()
                .flat_map(|t| match t.kind {
                    TileKind::Folder => t.children.clone().unwrap_or_default(),
                    _ => vec![t.clone()],
                })
                .collect()
        })
        .collect()
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
    /// Phase 5: call a Home Assistant service (`{"ha":{"service":"light.toggle","entity_id":"…"}}`).
    Ha(HaCall),
}

/// A Home Assistant service call from a deck button or trigger.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HaCall {
    /// `domain.service`, e.g. `light.toggle`.
    pub service: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_id: Option<String>,
    /// Extra service data (merged with `entity_id`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Map<String, serde_json::Value>>,
}

impl HaCall {
    /// `(domain, service, data)` ready for the Home Assistant client.
    pub fn parts(&self) -> Result<(String, String, serde_json::Value), String> {
        let (domain, service) = self
            .service
            .split_once('.')
            .ok_or_else(|| format!("ha service {:?} must look like light.toggle", self.service))?;
        let slug = |s: &str| {
            (1..=64).contains(&s.len())
                && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        };
        if !slug(domain) || !slug(service) {
            return Err(format!("ha service {:?} is not valid", self.service));
        }
        let mut data = self.data.clone().unwrap_or_default();
        if let Some(id) = &self.entity_id {
            if !helm_plugins::ha::valid_entity_id(id) {
                return Err(format!("ha entity_id {id:?} is not valid"));
            }
            data.insert("entity_id".into(), serde_json::Value::String(id.clone()));
        }
        if serde_json::to_vec(&data).map(|v| v.len()).unwrap_or(usize::MAX) > MAX_TEXT {
            return Err("ha data is too large".into());
        }
        Ok((domain.into(), service.into(), serde_json::Value::Object(data)))
    }
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
    /// G5: grid size (omitted = `1x1`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<ButtonSize>,
}

/// G5: deck button size in grid cells (columns x rows).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ButtonSize {
    #[serde(rename = "1x1")]
    One,
    #[serde(rename = "2x1")]
    Wide,
    #[serde(rename = "2x2")]
    Big,
}

/// G5: a named deck page listing button ids in display order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeckPage {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub buttons: Vec<String>,
}

pub const MAX_DECK_PAGES: usize = 32;
/// Id of the page an old (Phase 1) deck migrates into.
pub const DEFAULT_PAGE_ID: &str = "main";

/// Make `pages` consistent with `buttons`: unknown and repeated ids dropped, buttons on no
/// page appended to the first one, and at least one page (a Phase 1 deck migrates to a
/// single "Main" page holding every button).
pub fn normalize_deck_pages(mut pages: Vec<DeckPage>, buttons: &[DeckButton]) -> Vec<DeckPage> {
    if pages.is_empty() {
        pages.push(DeckPage {
            id: DEFAULT_PAGE_ID.into(),
            name: "Main".into(),
            buttons: Vec::new(),
        });
    }
    let known: std::collections::HashSet<&str> = buttons.iter().map(|b| b.id.as_str()).collect();
    let mut seen = std::collections::HashSet::new();
    for p in &mut pages {
        p.buttons
            .retain(|id| known.contains(id.as_str()) && seen.insert(id.clone()));
    }
    for b in buttons {
        if !seen.contains(&b.id) {
            pages[0].buttons.push(b.id.clone());
        }
    }
    pages
}

pub fn validate_deck_pages(pages: &[DeckPage]) -> Result<(), String> {
    if pages.len() > MAX_DECK_PAGES {
        return Err(format!("at most {MAX_DECK_PAGES} deck pages"));
    }
    let mut ids = std::collections::HashSet::new();
    for p in pages {
        check_len("page id", &p.id, MAX_ID, false)?;
        if !ids.insert(p.id.as_str()) {
            return Err(format!("duplicate page id {:?}", p.id));
        }
        check_len("page name", &p.name, 64, true)?;
        if p.buttons.len() > MAX_BUTTONS {
            return Err(format!("at most {MAX_BUTTONS} buttons per page"));
        }
    }
    Ok(())
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
        DeckAction::Ha(call) => {
            call.parts()?;
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
    /// G5 (version 2). Missing in Phase 1 files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pages: Vec<DeckPage>,
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
            validate_tile(t, &mut ids, true)?;
        }
    }
    Ok(())
}

fn validate_tile<'a>(
    t: &'a Tile,
    ids: &mut std::collections::HashSet<&'a str>,
    top_level: bool,
) -> Result<(), String> {
    check_len("tile id", &t.id, MAX_ID, false)?;
    if !ids.insert(t.id.as_str()) {
        return Err(format!("duplicate tile id {:?}", t.id));
    }
    if let Some(l) = &t.label {
        check_len("label", l, 200, true)?;
    }
    if let Some(i) = &t.icon {
        check_len("icon", i, 64, true)?;
    }
    if t.kind != TileKind::Folder && t.children.is_some() {
        return Err("only folder tiles have children".into());
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
        TileKind::Folder => {
            if !top_level {
                return Err("folders cannot hold folders".into());
            }
            let children = t.children.as_deref().unwrap_or_default();
            if children.is_empty() || children.len() > MAX_FOLDER_TILES {
                return Err(format!("a folder holds 1..={MAX_FOLDER_TILES} tiles"));
            }
            for c in children {
                validate_tile(c, ids, false)?;
            }
        }
    }
    Ok(())
}

/// Check one action (triggers, plugins) with the same rules as a deck button.
pub fn validate_deck_action(action: &DeckAction) -> Result<(), String> {
    validate_action(action, 0).map(|_| ())
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
        let g5 = pages
            .iter()
            .flatten()
            .any(|t| t.kind == TileKind::Folder || t.icon.is_some());
        let file = DockFile {
            version: if g5 { 2 } else { 1 },
            pages,
        };
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

    /// Phase 1 `deck.set`: replaces the buttons but keeps the G5 pages and, for buttons
    /// sent without a `size`, their stored size (old phones know neither).
    pub fn set_deck(&self, mut buttons: Vec<DeckButton>) -> anyhow::Result<()> {
        let _g = self.guard();
        let old = read_json::<DeckFile>(&self.dir.join(DECK_FILE))?;
        let (old_buttons, old_pages) = old.map(|f| (f.buttons, f.pages)).unwrap_or_default();
        for b in &mut buttons {
            if b.size.is_none() {
                b.size = old_buttons
                    .iter()
                    .find(|o| o.id == b.id)
                    .and_then(|o| o.size);
            }
        }
        self.write_deck(buttons, old_pages)
    }

    /// G5 `deck.set{buttons,pages}`: replaces both.
    pub fn set_deck2(&self, buttons: Vec<DeckButton>, pages: Vec<DeckPage>) -> anyhow::Result<()> {
        let _g = self.guard();
        self.write_deck(buttons, pages)
    }

    fn write_deck(&self, buttons: Vec<DeckButton>, pages: Vec<DeckPage>) -> anyhow::Result<()> {
        let pages = if pages.is_empty() {
            pages
        } else {
            normalize_deck_pages(pages, &buttons)
        };
        let g5 = !pages.is_empty() || buttons.iter().any(|b| b.size.is_some());
        let file = DeckFile {
            version: if g5 { 2 } else { 1 },
            buttons,
            pages,
        };
        util::atomic_write(
            &self.dir.join(DECK_FILE),
            serde_json::to_string_pretty(&file)?.as_bytes(),
        )?;
        Ok(())
    }

    /// G5: buttons and pages, normalized (a Phase 1 deck has one "Main" page).
    pub fn deck_pages(&self) -> anyhow::Result<(Vec<DeckButton>, Vec<DeckPage>)> {
        let _g = self.guard();
        let (buttons, pages) = read_json::<DeckFile>(&self.dir.join(DECK_FILE))?
            .map(|f| (f.buttons, f.pages))
            .unwrap_or_default();
        let pages = normalize_deck_pages(pages, &buttons);
        Ok((buttons, pages))
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
            icon: None,
            children: None,
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
            size: None,
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
            icon: None,
            children: None,
        }]];
        s.set_dock(pages.clone()).unwrap();
        assert_eq!(s.dock().unwrap(), pages);
    }

    #[test]
    fn g5_folders_validate_and_flatten() {
        let pages: Vec<Vec<Tile>> = serde_json::from_value(json!([[
            {"id":"a","kind":"app","app_id":"Fake.Notes"},
            {"id":"f","kind":"folder","label":"Work","children":[
                {"id":"b","kind":"app","app_id":"Fake.Store_abc!App"},
                {"id":"c","kind":"open","target":"https://example.com","icon":"b:globe"}
            ]}
        ]]))
        .unwrap();
        assert!(validate_pages(&pages).is_ok());
        let flat = flatten_pages(&pages);
        let ids: Vec<&str> = flat[0].iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
        let bad = |v: Value| {
            let p: Vec<Vec<Tile>> = serde_json::from_value(v).unwrap();
            assert!(validate_pages(&p).is_err());
        };
        bad(json!([[{"id":"f","kind":"folder","children":[]}]]));
        bad(json!([[{"id":"f","kind":"folder"}]]));
        bad(json!([[{"id":"f","kind":"folder","children":[
            {"id":"g","kind":"folder","children":[{"id":"x","kind":"app","app_id":"a"}]}]}]]));
        bad(json!([[{"id":"x","kind":"app","app_id":"a"},
            {"id":"f","kind":"folder","children":[{"id":"x","kind":"app","app_id":"b"}]}]]));
        bad(json!([[{"id":"x","kind":"app","app_id":"a","children":[]}]]));
    }

    #[test]
    fn g5_old_deck_file_migrates_to_one_page() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(DECK_FILE),
            r##"{"version":1,"buttons":[
                {"id":"a","label":"A","color":"#000000","action":{"text":"x"}},
                {"id":"b","label":"B","color":"#000000","action":{"text":"y"}}]}"##,
        )
        .unwrap();
        let s = LayoutStore::new(dir.path());
        assert_eq!(s.deck().unwrap().len(), 2);
        let (_, pages) = s.deck_pages().unwrap();
        assert_eq!(
            pages,
            vec![DeckPage {
                id: "main".into(),
                name: "Main".into(),
                buttons: vec!["a".into(), "b".into()]
            }]
        );
        // G5 save with sizes and two pages.
        let mut buttons = s.deck().unwrap();
        buttons[1].size = Some(ButtonSize::Big);
        let two = vec![
            DeckPage { id: "p1".into(), name: "One".into(), buttons: vec!["a".into(), "zz".into()] },
            DeckPage { id: "p2".into(), name: "Two".into(), buttons: vec!["b".into(), "a".into()] },
        ];
        s.set_deck2(buttons.clone(), two).unwrap();
        let (_, pages) = s.deck_pages().unwrap();
        assert_eq!(pages[0].buttons, ["a"]);
        assert_eq!(pages[1].buttons, ["b"]);
        // A Phase 1 save (no size, no pages) keeps the stored size and pages.
        let mut legacy = buttons.clone();
        legacy[1].size = None;
        legacy.push(serde_json::from_value(json!(
            {"id":"c","label":"C","color":"#000000","action":{"text":"z"}})).unwrap());
        s.set_deck(legacy).unwrap();
        let (after, pages) = s.deck_pages().unwrap();
        assert_eq!(after[1].size, Some(ButtonSize::Big));
        assert_eq!(pages[0].buttons, ["a", "c"]);
        assert_eq!(pages[1].buttons, ["b"]);
        let raw: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join(DECK_FILE)).unwrap()).unwrap();
        assert_eq!(raw["version"], 2);
        assert_eq!(raw["buttons"][1]["size"], "2x2");
    }

    #[test]
    fn g5_deck_pages_validate() {
        let p = |id: &str| DeckPage { id: id.into(), name: "x".into(), buttons: vec![] };
        assert!(validate_deck_pages(&[p("a"), p("b")]).is_ok());
        assert!(validate_deck_pages(&[p("a"), p("a")]).is_err());
        assert!(validate_deck_pages(&[p("")]).is_err());
        assert!(serde_json::from_value::<ButtonSize>(json!("3x3")).is_err());
    }
}
