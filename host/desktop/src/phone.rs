//! The Phone page's Rust side (g9, Docs/protocol/phone-control.md): commands over the agent's
//! [`helm_agent::phone::PhoneHub`], and the pumps that carry its events and frames to the window.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use helm_agent::AppState;
use helm_agent::phone::{PhoneInfo, PhoneInput, RequestOptions, event_json};
use serde::Serialize;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{Emitter, State};
use tokio::sync::Semaphore;
use tokio::sync::broadcast::error::RecvError;

use crate::controller::{Controller, PhoneView};

/// Window event: a session changed; the payload is a serialized `PhoneEvent`.
pub const PHONE_EVENT: &str = "helm://phone";
/// Frames sent to the window and not yet acknowledged. Beyond this, the agent's queue drops.
const FRAMES_IN_FLIGHT: usize = 4;

type Ctl<'a> = State<'a, Arc<Controller>>;

/// What the frame pump shares with `phone_ack`.
#[derive(Default)]
pub struct FramePump {
    credit: Mutex<Option<Arc<Semaphore>>>,
    task: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}

impl FramePump {
    fn stop(&self) {
        if let Some(t) = lock(&self.task).take() {
            t.abort();
        }
        *lock(&self.credit) = None;
    }

    fn start(&self, state: Arc<AppState>, channel: Channel<InvokeResponseBody>) {
        self.stop();
        let credit = Arc::new(Semaphore::new(FRAMES_IN_FLIGHT));
        *lock(&self.credit) = Some(credit.clone());
        let task = tauri::async_runtime::spawn(async move {
            loop {
                let Ok(permit) = credit.acquire().await else { return };
                // The window gives this back through `phone_ack`.
                permit.forget();
                let frame = state.phone.next_frame().await;
                if channel.send(InvokeResponseBody::Raw(frame)).is_err() {
                    return;
                }
            }
        });
        *lock(&self.task) = Some(task);
    }

    fn ack(&self) {
        if let Some(c) = lock(&self.credit).as_ref()
            && c.available_permits() < FRAMES_IN_FLIGHT
        {
            c.add_permits(1);
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// One row of the phone picker.
#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct TargetView {
    pub device_id: String,
    pub name: String,
    pub via: &'static str,
    /// Its app speaks g9.
    pub supported: bool,
    /// "Let PC see this phone" is on.
    pub enabled: bool,
    pub accessibility: bool,
}

/// Connected phones only, with what each of them currently allows.
pub fn target_views(phones: Vec<PhoneView>, caps: &HashMap<String, (bool, bool)>) -> Vec<TargetView> {
    phones
        .into_iter()
        .filter(|p| p.online)
        .map(|p| {
            let c = caps.get(&p.device_id).copied();
            TargetView {
                via: p.via.unwrap_or("lan"),
                supported: c.is_some(),
                enabled: c.is_some_and(|c| c.0),
                accessibility: c.is_some_and(|c| c.1),
                device_id: p.device_id,
                name: p.name,
            }
        })
        .collect()
}

fn agent(ctl: &Controller) -> Result<Arc<AppState>, String> {
    ctl.agent()
        .ok_or_else(|| "Helm is not running. Resume connections first.".to_string())
}

#[tauri::command]
pub fn phone_targets(ctl: Ctl<'_>) -> Vec<TargetView> {
    let caps: HashMap<String, (bool, bool)> = ctl
        .agent()
        .map(|st| {
            st.phone
                .targets()
                .into_iter()
                .map(|t| (t.device_id, (t.enabled, t.accessibility)))
                .collect()
        })
        .unwrap_or_default();
    target_views(ctl.phones(), &caps)
}

/// Ask a phone. Async so that it runs inside the runtime the agent's timers need.
#[tauri::command]
pub async fn phone_request(
    ctl: Ctl<'_>,
    pump: State<'_, FramePump>,
    device_id: String,
    on_frame: Channel<InvokeResponseBody>,
) -> Result<PhoneInfo, String> {
    let st = agent(&ctl)?;
    let info = st
        .phone
        .request(&st, &device_id, RequestOptions::default())
        .map_err(|e| e.message)?;
    pump.start(st, on_frame);
    Ok(info)
}

#[tauri::command]
pub fn phone_stop(ctl: Ctl<'_>, pump: State<'_, FramePump>) -> Result<(), String> {
    let st = agent(&ctl)?;
    st.phone.stop(&st);
    pump.stop();
    Ok(())
}

/// The window took one frame.
#[tauri::command]
pub fn phone_ack(pump: State<'_, FramePump>) {
    pump.ack();
}

#[tauri::command]
pub fn phone_keyframe(ctl: Ctl<'_>) -> Result<(), String> {
    agent(&ctl)?.phone.request_keyframe();
    Ok(())
}

/// One input event. The agent drops it unless a session is live with control granted.
#[tauri::command]
pub fn phone_input(ctl: Ctl<'_>, event: PhoneInput) -> Result<(), String> {
    agent(&ctl)?.phone.input(event).map_err(|e| e.message)
}

/// Forward session events to the window until the agent stops; frames stop with the session.
pub async fn pump_events(app: tauri::AppHandle, state: Arc<AppState>) {
    use helm_agent::phone::PhoneEvent;
    use tauri::Manager;
    let mut events = state.phone.subscribe();
    loop {
        match events.recv().await {
            Ok(ev) => {
                if matches!(ev, PhoneEvent::Ended { .. })
                    && let Some(pump) = app.try_state::<FramePump>()
                {
                    pump.stop();
                }
                let _ = app.emit(PHONE_EVENT, event_json(&ev));
            }
            Err(RecvError::Lagged(_)) => {
                let _ = app.emit(PHONE_EVENT, event_json(&PhoneEvent::Targets));
            }
            Err(RecvError::Closed) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helm_agent::router::Tier;

    fn phone(id: &str, online: bool, via: Option<&'static str>) -> PhoneView {
        PhoneView {
            device_id: id.into(),
            name: format!("Phone {id}"),
            tier: Tier::Admin,
            paired_at_ms: 0,
            last_seen_ms: 0,
            online,
            via,
            peer: None,
        }
    }

    #[test]
    fn only_connected_phones_are_offered_with_their_caps() {
        let caps = HashMap::from([
            ("a".to_string(), (true, true)),
            ("c".to_string(), (false, false)),
        ]);
        let list = target_views(
            vec![
                phone("a", true, Some("relay")),
                phone("b", true, Some("lan")),
                phone("c", true, None),
                phone("d", false, None),
            ],
            &caps,
        );
        assert_eq!(list.len(), 3, "the offline phone is not offered");
        assert_eq!(
            list[0],
            TargetView {
                device_id: "a".into(),
                name: "Phone a".into(),
                via: "relay",
                supported: true,
                enabled: true,
                accessibility: true,
            }
        );
        assert!(!list[1].supported && !list[1].enabled, "old app");
        assert!(list[2].supported && !list[2].enabled, "switched off");
        assert_eq!(list[2].via, "lan");
    }

    #[test]
    fn input_commands_take_the_wire_form() {
        let e: PhoneInput = serde_json::from_str(r#"{"k":"tap","x":0.5,"y":0.25}"#).unwrap();
        assert_eq!(e, PhoneInput::Tap { x: 0.5, y: 0.25 });
        let s: PhoneInput =
            serde_json::from_str(r#"{"k":"scroll","x":0.5,"y":0.5,"dy":1.5,"dx":0}"#).unwrap();
        assert_eq!(s, PhoneInput::Scroll { x: 0.5, y: 0.5, dy: 1.5, dx: 0.0 });
    }

    /// The window's pure logic (`ui/phone-core.js`) is plain JS, tested under Node when present.
    #[test]
    fn phone_core_js_tests() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
        let run = std::process::Command::new("node")
            .arg("--test")
            .arg(dir.join("phone-core.test.mjs"))
            .output();
        match run {
            Ok(out) => assert!(
                out.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ),
            Err(e) => eprintln!("skipped: node is not available ({e})"),
        }
    }
}
