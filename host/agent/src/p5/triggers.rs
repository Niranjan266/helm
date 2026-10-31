//! `triggers.get` / `triggers.set` (PROTOCOL §13) on top of `helm_plugins::trigger`.
//!
//! The engine decides *when*; the agent keeps the parsed [`DeckAction`] of every trigger
//! (deck actions here include `ha`, which the crate's validator does not know) and gives the
//! engine a unique placeholder action per trigger. Triggers are stored as sent by the phone
//! in `triggers.json`; `when` accepts the short forms `{"app_start":"x.exe"}`,
//! `{"wifi":"Home"}` and `{"phone_connected":"<device_id>"|null}` as well as the object forms.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use helm_plugins::trigger::{Clock, Event, FakeClock, Now, Trigger, TriggerEngine};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::store::{DeckAction, read_json, validate_deck_action};
use crate::util;

use super::lock;

pub const TRIGGERS_FILE: &str = "triggers.json";
pub const MAX_TRIGGERS: usize = 64;

/// Real time with a time-zone offset the agent refreshes from the platform; or a
/// [`FakeClock`] in tests.
#[derive(Clone, Debug)]
pub struct AgentClock {
    start: Instant,
    offset_minutes: Arc<AtomicI32>,
    fake: Option<FakeClock>,
}

impl AgentClock {
    pub fn system() -> AgentClock {
        AgentClock {
            start: Instant::now(),
            offset_minutes: Arc::new(AtomicI32::new(0)),
            fake: None,
        }
    }

    pub fn fake(clock: FakeClock) -> AgentClock {
        AgentClock {
            fake: Some(clock),
            ..AgentClock::system()
        }
    }

    pub fn set_offset(&self, minutes: i32) {
        self.offset_minutes.store(minutes, Ordering::Relaxed);
    }
}

impl Clock for AgentClock {
    fn now(&self) -> Now {
        if let Some(f) = &self.fake {
            return f.now();
        }
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Now {
            mono_ms: self.start.elapsed().as_millis() as u64,
            local_minute: secs.div_euclid(60)
                + i64::from(self.offset_minutes.load(Ordering::Relaxed)),
        }
    }
}

/// Short `when` forms → the engine's object forms.
fn normalize_when(when: &Value) -> Value {
    let Some(obj) = when.as_object() else {
        return when.clone();
    };
    if obj.len() != 1 {
        return when.clone();
    }
    let (k, v) = obj.iter().next().expect("one entry");
    match (k.as_str(), v) {
        ("app_start", Value::String(exe)) => json!({"app_start": {"exe": exe}}),
        ("wifi", Value::String(ssid)) => json!({"wifi": {"ssid": ssid}}),
        ("phone_connected", Value::String(id)) => json!({"phone_connected": {"device_id": id}}),
        ("phone_connected", Value::Null) => json!({"phone_connected": {"device_id": null}}),
        ("phone_connected", Value::Bool(true)) => json!({"phone_connected": {"device_id": null}}),
        _ => when.clone(),
    }
}

/// Validate the phone's trigger list: engine triggers (placeholder actions) plus the real
/// action of each trigger id.
pub fn parse(raw: &[Value]) -> Result<(Vec<Trigger>, HashMap<String, DeckAction>), String> {
    if raw.len() > MAX_TRIGGERS {
        return Err(format!("at most {MAX_TRIGGERS} triggers"));
    }
    let mut engine = Vec::with_capacity(raw.len());
    let mut actions = HashMap::new();
    for t in raw {
        let obj = t.as_object().ok_or("each trigger must be an object")?;
        let id = obj
            .get("id")
            .and_then(Value::as_str)
            .ok_or("trigger id (string) is required")?;
        let action: DeckAction = serde_json::from_value(
            obj.get("action")
                .cloned()
                .ok_or_else(|| format!("trigger {id:?}: action is required"))?,
        )
        .map_err(|e| format!("trigger {id:?}: action: {e}"))?;
        validate_deck_action(&action).map_err(|e| format!("trigger {id:?}: {e}"))?;
        let mut def = t.clone();
        def["when"] = normalize_when(obj.get("when").unwrap_or(&Value::Null));
        // Unique per trigger so the engine's "same action once per batch" rule never merges
        // two different triggers; the agent dedupes the real actions itself.
        def["action"] = json!({"text": format!("trigger:{id}")});
        let parsed: Trigger =
            serde_json::from_value(def).map_err(|e| format!("trigger {id:?}: {e}"))?;
        actions.insert(id.to_string(), action);
        engine.push(parsed);
    }
    Ok((engine, actions))
}

#[derive(Serialize, Deserialize)]
struct TriggersFile {
    version: u32,
    triggers: Vec<Value>,
}

pub struct Triggers {
    path: PathBuf,
    clock: AgentClock,
    engine: Mutex<TriggerEngine<AgentClock>>,
    actions: Mutex<HashMap<String, DeckAction>>,
    raw: Mutex<Vec<Value>>,
}

impl Triggers {
    pub fn load(data_dir: &Path, clock: AgentClock) -> Triggers {
        let engine = TriggerEngine::new(clock.clone(), Vec::new()).expect("empty list is valid");
        let t = Triggers {
            path: data_dir.join(TRIGGERS_FILE),
            clock,
            engine: Mutex::new(engine),
            actions: Mutex::new(HashMap::new()),
            raw: Mutex::new(Vec::new()),
        };
        match read_json::<TriggersFile>(&t.path) {
            Ok(Some(f)) => {
                if let Err(e) = t.apply(f.triggers) {
                    tracing::warn!("{TRIGGERS_FILE} ignored: {e}");
                }
            }
            Ok(None) => {}
            Err(e) => tracing::warn!("{TRIGGERS_FILE} unreadable: {e:#}"),
        }
        t
    }

    pub fn clock(&self) -> &AgentClock {
        &self.clock
    }

    fn apply(&self, raw: Vec<Value>) -> Result<(), String> {
        let (defs, actions) = parse(&raw)?;
        lock(&self.engine)
            .set_triggers(defs)
            .map_err(|e| e.to_string())?;
        *lock(&self.actions) = actions;
        *lock(&self.raw) = raw;
        Ok(())
    }

    pub fn get(&self) -> Vec<Value> {
        lock(&self.raw).clone()
    }

    /// Validate, apply and save (all-or-nothing).
    pub fn set(&self, raw: Vec<Value>) -> Result<(), String> {
        parse(&raw)?;
        let file = TriggersFile {
            version: 1,
            triggers: raw.clone(),
        };
        let text = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
        self.apply(raw)?;
        util::atomic_write(&self.path, text.as_bytes())
            .map_err(|e| format!("could not save {TRIGGERS_FILE}: {e}"))
    }

    fn actions_for(&self, firings: Vec<helm_plugins::trigger::Firing>) -> Vec<(String, DeckAction)> {
        let actions = lock(&self.actions);
        let mut out: Vec<(String, DeckAction)> = Vec::new();
        let mut seen = HashSet::new();
        for f in firings {
            if let Some(a) = actions.get(&f.trigger_id)
                && !out.iter().any(|(_, o)| o == a)
                && seen.insert(f.trigger_id.clone())
            {
                out.push((f.trigger_id, a.clone()));
            }
        }
        out
    }

    /// Time triggers due now.
    pub fn tick(&self) -> Vec<(String, DeckAction)> {
        let firings = lock(&self.engine).tick();
        self.actions_for(firings)
    }

    /// Feed one event.
    pub fn handle(&self, event: &Event) -> Vec<(String, DeckAction)> {
        let firings = lock(&self.engine).handle(event);
        self.actions_for(firings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helm_plugins::trigger::Weekday;

    fn triggers(dir: &Path, clock: FakeClock) -> Triggers {
        Triggers::load(dir, AgentClock::fake(clock))
    }

    #[test]
    fn short_and_object_forms_parse() {
        let raw = vec![
            json!({"id":"a","when":{"app_start":"Code.exe"},"action":{"launch":"Fake.Notes"}}),
            json!({"id":"b","when":{"app_start":{"exe":"obs64.exe"}},"action":{"media":"pause"}}),
            json!({"id":"c","when":{"phone_connected":null},"action":{"volume_step":0.1}}),
            json!({"id":"d","when":{"wifi":"Home"},"action":{"ha":{"service":"light.toggle","entity_id":"light.desk"}}}),
            json!({"id":"e","when":{"time":"07:30 mon-fri"},"action":{"text":"hi"}}),
        ];
        let (defs, actions) = parse(&raw).unwrap();
        assert_eq!(defs.len(), 5);
        assert!(matches!(actions["d"], DeckAction::Ha(_)));
        for bad in [
            json!([{"id":"x","when":{"time":"25:00"},"action":{"text":"a"}}]),
            json!([{"id":"x","when":{"app_start":"a.exe"},"action":{"hotkey":"nope+nope"}}]),
            json!([{"id":"x","when":{"wifi":"a"},"action":{"ha":{"service":"Light"}}}]),
            json!([{"id":"x","when":{"moon":"full"},"action":{"text":"a"}}]),
            json!([{"id":"x","when":{"wifi":"a"}}]),
            json!([{"id":"x","when":{"wifi":"a"},"action":{"text":"a"}},{"id":"x","when":{"wifi":"b"},"action":{"text":"b"}}]),
        ] {
            let list: Vec<Value> = serde_json::from_value(bad.clone()).unwrap();
            let r = parse(&list).and_then(|(d, _)| {
                TriggerEngine::new(AgentClock::system(), d)
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            });
            assert!(r.is_err(), "{bad}");
        }
    }

    #[test]
    fn time_triggers_fire_once_per_minute_and_persist() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FakeClock::at(Weekday::Mon, 7, 29);
        let t = triggers(dir.path(), clock.clone());
        t.set(vec![
            json!({"id":"m","when":{"time":"07:30 mon-fri"},"action":{"launch":"Fake.Notes"}}),
            json!({"id":"w","when":{"time":"07:30 weekends"},"action":{"text":"weekend"}}),
        ])
        .unwrap();
        assert!(t.tick().is_empty());
        clock.advance_min(1);
        let fired = t.tick();
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].0, "m");
        assert_eq!(fired[0].1, DeckAction::Launch("Fake.Notes".into()));
        clock.advance_ms(20_000);
        assert!(t.tick().is_empty(), "same minute fires once");
        // Reloaded from triggers.json.
        let again = triggers(dir.path(), FakeClock::at(Weekday::Sat, 7, 30));
        assert_eq!(again.get().len(), 2);
        let fired = again.tick();
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].0, "w");
    }

    #[test]
    fn events_are_edge_triggered_and_deduped() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FakeClock::at(Weekday::Tue, 12, 0);
        let t = triggers(dir.path(), clock.clone());
        t.set(vec![
            json!({"id":"a","when":{"app_start":"game.exe"},"action":{"media":"pause"}}),
            json!({"id":"b","when":{"app_start":"GAME"},"action":{"media":"pause"}}),
            json!({"id":"p","when":{"phone_connected":"dev1"},"action":{"text":"hi"}}),
            json!({"id":"w","when":{"wifi":"Home"},"action":{"text":"home"}}),
        ])
        .unwrap();
        let fired = t.handle(&Event::AppStarted {
            exe: "C:\\Games\\game.exe".into(),
        });
        assert_eq!(fired.len(), 1, "identical actions run once: {fired:?}");
        assert!(t
            .handle(&Event::AppStarted {
                exe: "game.exe".into()
            })
            .is_empty());
        assert!(t
            .handle(&Event::PhoneConnected {
                device_id: "other".into()
            })
            .is_empty());
        assert_eq!(
            t.handle(&Event::PhoneConnected {
                device_id: "dev1".into()
            })
            .len(),
            1
        );
        assert_eq!(
            t.handle(&Event::WifiConnected {
                ssid: "Home".into()
            })
            .len(),
            1
        );
        assert!(t
            .handle(&Event::WifiConnected {
                ssid: "Home".into()
            })
            .is_empty());
        // A bad list changes nothing.
        assert!(t.set(vec![json!({"id":"z"})]).is_err());
        assert_eq!(t.get().len(), 4);
    }
}
