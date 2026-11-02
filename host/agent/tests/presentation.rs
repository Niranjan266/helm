//! G2 presentation mode (`Docs/protocol/presentation.md`) end to end against a
//! `FakePlatform`: no keys reach this PC, PowerPoint is never automated, no overlay is drawn.

mod common;

use std::collections::VecDeque;
use std::path::Path;
use std::time::Duration;

use common::*;
use futures_util::StreamExt;
use helm_agent::platform::Monitor;
use helm_agent::platform::Rect;
use helm_agent::present::SlideInfo;
use helm_agent::present::fake::LaserCall;
use helm_win_input::Op;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

struct Client {
    ws: Ws,
    backlog: VecDeque<Value>,
    next_id: i64,
}

impl Client {
    async fn read(&mut self, wait: Duration) -> Option<Value> {
        loop {
            let msg = tokio::time::timeout(wait, self.ws.next()).await.ok()?;
            match msg.expect("connection closed").expect("websocket error") {
                Message::Text(t) => return Some(serde_json::from_str(t.as_str()).unwrap()),
                Message::Close(f) => panic!("unexpected close: {f:?}"),
                _ => {}
            }
        }
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        send(&mut self.ws, json!({"type":"req","id":id,"method":method,"params":params})).await;
        loop {
            let m = self
                .read(Duration::from_secs(10))
                .await
                .unwrap_or_else(|| panic!("no response to {method}"));
            if m["type"] == "res" && m["id"] == id {
                return m;
            }
            self.backlog.push_back(m);
        }
    }

    async fn ok(&mut self, method: &str, params: Value) -> Value {
        let r = self.call(method, params).await;
        assert_eq!(r["ok"], true, "{method} failed: {r}");
        r["result"].clone()
    }

    async fn err(&mut self, method: &str, params: Value) -> String {
        let r = self.call(method, params).await;
        assert_eq!(r["ok"], false, "{method} should fail: {r}");
        r["error"]["code"].as_str().unwrap().to_string()
    }

    async fn event(&mut self, name: &str) -> Value {
        if let Some(i) = self.backlog.iter().position(|m| m["name"] == name) {
            return self.backlog.remove(i).unwrap()["data"].clone();
        }
        loop {
            let m = self.read(Duration::from_secs(5)).await.unwrap_or_else(|| panic!("no {name}"));
            if m["type"] == "event" && m["name"] == name {
                return m["data"].clone();
            }
            self.backlog.push_back(m);
        }
    }
}

fn client(ws: Ws) -> Client {
    Client {
        ws,
        backlog: VecDeque::new(),
        next_id: 1,
    }
}

async fn paired(server: &helm_agent::ServerHandle) -> (Client, Phone) {
    let phone = Phone::new("Pixel");
    let token = server.new_pairing().token;
    let (ws, ready) = phone.pair(server, &token).await;
    assert_eq!(ready["type"], "ready", "{ready}");
    (client(ws), phone)
}

async fn with_tier(server: &helm_agent::ServerHandle, dir: &Path, phone: &Phone, tier: &str) -> Client {
    let path = dir.join("devices.json");
    let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    for d in v["devices"].as_array_mut().unwrap() {
        if d["device_id"] == phone.device_id {
            d["tier"] = json!(tier);
        }
    }
    std::fs::write(&path, serde_json::to_string(&v).unwrap()).unwrap();
    let (ws, ready) = phone.auth(server).await;
    assert_eq!(ready["tier"], tier, "{ready}");
    client(ws)
}

async fn wait_until(mut cond: impl FnMut() -> bool) {
    for _ in 0..200 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("condition not met in time");
}

fn key_presses(ops: &[Op]) -> Vec<u16> {
    ops.iter()
        .filter_map(|o| match o {
            Op::Key { vk, down: true } => Some(*vk),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn hello_advertises_g2_and_tiers_are_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _fake) = start_fake_with(dir.path(), |_| {}).await;
    let mut ws = try_connect_path(server.port(), server.fingerprint(), "/ws").await.unwrap();
    let hello = recv(&mut ws).await;
    assert!(hello["features"].as_array().unwrap().contains(&json!("g2")), "{hello}");
    drop(ws);
    let names = server.state().router().method_names();
    for m in ["present.state", "present.go", "present.laser"] {
        assert!(names.contains(&m), "{m} missing");
    }
    let (_admin, phone) = paired(&server).await;
    let mut viewer = with_tier(&server, dir.path(), &phone, "viewer").await;
    viewer.ok("present.state", json!({})).await;
    for m in ["present.go", "present.laser"] {
        assert_eq!(viewer.err(m, json!({"action":"next"})).await, "forbidden", "{m}");
    }
    send(&mut viewer.ws, json!({"type":"in","k":"lp","x":0.5,"y":0.5})).await;
    assert_eq!(viewer.event("input.error").await["code"], "forbidden");
    server.shutdown().await;
}

#[tokio::test]
async fn state_reports_app_and_powerpoint_slide() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |_| {}).await;
    let (mut c, _) = paired(&server).await;
    let present = fake.present_fake();

    let s = c.ok("present.state", json!({})).await;
    assert_eq!(s["app"], Value::Null);
    assert_eq!(s["slide"], Value::Null);
    assert_eq!(s["slide_status"], "unsupported");
    assert_eq!(s["laser"]["visible"], false);

    present.set_foreground("POWERPNT.EXE", "PowerPoint Slide Show - Q3.pptx");
    present.set_powerpoint(Some(None));
    let s = c.ok("present.state", json!({})).await;
    assert_eq!(s["app"], "powerpoint");
    assert_eq!(s["exe"], "POWERPNT.EXE");
    assert_eq!(s["slide_status"], "no_presentation");

    present.set_powerpoint(Some(Some(SlideInfo {
        index: 3,
        count: 12,
        notes: "Mention the margin".into(),
        showing: true,
    })));
    let s = c.ok("present.state", json!({})).await;
    assert_eq!(s["slide_status"], "ok");
    assert_eq!(s["slide"], json!({"index":3,"count":12,"notes":"Mention the margin","showing":true}));

    present.set_foreground("chrome.exe", "Deck - Google Slides - Google Chrome");
    assert_eq!(c.ok("present.state", json!({})).await["app"], "slides");
    server.shutdown().await;
}

#[tokio::test]
async fn go_validates_and_sends_app_keys() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |_| {}).await;
    let (mut c, _) = paired(&server).await;
    let present = fake.present_fake();

    assert_eq!(c.err("present.go", json!({})).await, "bad_params");
    assert_eq!(c.err("present.go", json!({"action":"jump"})).await, "bad_params");
    assert_eq!(c.err("present.go", json!({"action":5})).await, "bad_params");
    // Nothing presenting: generic keys work, start/black have none.
    assert_eq!(c.err("present.go", json!({"action":"start"})).await, "unsupported");
    assert_eq!(c.err("present.go", json!({"action":"black"})).await, "unsupported");
    assert!(fake.ops().is_empty(), "nothing sent for refused actions");
    let r = c.ok("present.go", json!({"action":"next"})).await;
    assert_eq!(r, json!({"app":null,"keys":"right"}));

    present.set_foreground("POWERPNT.EXE", "PowerPoint Slide Show");
    assert_eq!(c.ok("present.go", json!({"action":"black"})).await["keys"], "b");
    assert_eq!(c.ok("present.go", json!({"action":"start"})).await["keys"], "f5");
    present.set_foreground("msedge.exe", "Plan - Google Slides - Work - Microsoft Edge");
    let r = c.ok("present.go", json!({"action":"start"})).await;
    assert_eq!(r, json!({"app":"slides","keys":"ctrl+f5"}));
    present.set_foreground("SumatraPDF.exe", "handout.pdf");
    assert_eq!(c.ok("present.go", json!({"action":"prev"})).await["keys"], "pageup");
    assert_eq!(c.err("present.go", json!({"action":"black"})).await, "unsupported");

    // right, B, F5, ctrl+F5, page up — each pressed and released.
    let ops = fake.ops();
    assert_eq!(key_presses(&ops), vec![0x27, 0x42, 0x74, 0x11, 0x74, 0x21]);
    let releases = ops.iter().filter(|o| matches!(o, Op::Key { down: false, .. })).count();
    assert_eq!(releases, 6);
    server.shutdown().await;
}

#[tokio::test]
async fn laser_frames_drive_the_overlay_on_the_chosen_monitor() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |_| {}).await;
    fake.with(|s| {
        s.monitors.push(Monitor {
            id: "\\\\.\\DISPLAY2".into(),
            name: "Projector".into(),
            x: 1920,
            y: -200,
            width: 1280,
            height: 720,
            is_primary: false,
        })
    });
    let present = fake.present_fake();
    let (mut c, phone) = paired(&server).await;

    assert_eq!(c.err("present.laser", json!({"monitor_id":"nope"})).await, "bad_params");
    assert_eq!(c.err("present.laser", json!({"trail":"yes"})).await, "bad_params");

    // Without present.laser the primary monitor is used.
    send(&mut c.ws, json!({"type":"in","k":"lp","x":0.5,"y":0.25,"on":true})).await;
    let primary = Rect { x: 0, y: 0, width: 1920, height: 1080 };
    wait_until(|| present.laser_log().len() == 1).await;
    assert_eq!(
        present.laser_log()[0],
        LaserCall::Show { rect: primary, x: 0.5, y: 0.25, trail: false }
    );

    let r = c.ok("present.laser", json!({"monitor_id":"\\\\.\\DISPLAY2","trail":true})).await;
    assert_eq!(r["monitor"]["name"], "Projector");
    assert_eq!(r["trail"], true);
    send(&mut c.ws, json!({"type":"in","k":"lp","x":2.0,"y":0.5})).await;
    let projector = Rect { x: 1920, y: -200, width: 1280, height: 720 };
    wait_until(|| present.laser_log().len() == 2).await;
    assert_eq!(
        present.laser_log()[1],
        LaserCall::Show { rect: projector, x: 1.0, y: 0.5, trail: true }
    );
    assert_eq!(c.ok("present.state", json!({})).await["laser"], json!({"visible":true,"monitor_id":"\\\\.\\DISPLAY2"}));

    send(&mut c.ws, json!({"type":"in","k":"lp","on":false})).await;
    wait_until(|| present.laser_log().len() == 3).await;
    assert_eq!(present.laser_log()[2], LaserCall::Hide);
    // Off again while hidden does nothing.
    send(&mut c.ws, json!({"type":"in","k":"lp","on":false})).await;
    send(&mut c.ws, json!({"type":"in","k":"lp","x":0.1})).await;
    assert_eq!(c.event("input.error").await["code"], "bad_params");
    assert_eq!(present.laser_log().len(), 3);

    // Disconnecting while the laser shows hides it.
    send(&mut c.ws, json!({"type":"in","k":"lp","x":0.3,"y":0.3})).await;
    wait_until(|| present.laser_log().len() == 4).await;
    drop(c);
    wait_until(|| present.laser_log().len() == 5).await;
    assert_eq!(present.laser_log()[4], LaserCall::Hide);

    // A new session starts from defaults again (primary monitor, no trail).
    let (ws, _) = phone.auth(&server).await;
    let mut c2 = client(ws);
    send(&mut c2.ws, json!({"type":"in","k":"lp","x":0.0,"y":0.0})).await;
    wait_until(|| present.laser_log().len() == 6).await;
    assert_eq!(
        present.laser_log()[5],
        LaserCall::Show { rect: primary, x: 0.0, y: 0.0, trail: false }
    );
    c2.ok("present.state", json!({})).await;
    server.shutdown().await;
}

/// Live check on this PC (run by hand): draws the laser across the primary monitor for ~2 s
/// and reads PowerPoint's state (read-only). Presses no keys.
/// `cargo test --test presentation -- --ignored --nocapture`
#[cfg(windows)]
#[test]
#[ignore]
fn live_laser_overlay_and_powerpoint() {
    use helm_agent::platform::Platform;
    let dir = tempfile::tempdir().unwrap();
    let real = helm_agent::platform::real::RealPlatform::new(dir.path());
    let hub = real.presenter();
    let m = real.monitors().unwrap().into_iter().find(|m| m.is_primary).unwrap();
    hub.configure(1, Some(Rect::from(&m)), None, true);
    for i in 0..=120 {
        let t = i as f32 / 120.0;
        let y = 0.5 + 0.25 * (t * std::f32::consts::TAU).sin();
        hub.frame(1, &json!({"x": 0.1 + 0.8 * t, "y": y}), || None).unwrap();
        std::thread::sleep(Duration::from_millis(16));
    }
    hub.frame(1, &json!({"on": false}), || None).unwrap();
    println!("foreground: {:?}", hub.backend().foreground());
    println!("powerpoint: {:?}", hub.backend().powerpoint());
    std::thread::sleep(Duration::from_millis(300));
}
