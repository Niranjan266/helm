//! Phase 5 end-to-end tests (PROTOCOL §13) against a `FakePlatform`: no real recording,
//! shell, pen, toast, Wi-Fi or HTTP — everything goes through the fake.

mod common;

use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::Duration;

use common::*;
use futures_util::StreamExt;
use helm_agent::platform::PenPhase;
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
        send(
            &mut self.ws,
            json!({"type":"req","id":id,"method":method,"params":params}),
        )
        .await;
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

    async fn event_where(&mut self, name: &str, pred: impl Fn(&Value) -> bool) -> Value {
        let is = |m: &Value| m["type"] == "event" && m["name"] == name && pred(&m["data"]);
        if let Some(i) = self.backlog.iter().position(is) {
            return self.backlog.remove(i).unwrap()["data"].clone();
        }
        loop {
            let m = self
                .read(Duration::from_secs(5))
                .await
                .unwrap_or_else(|| panic!("no {name} event"));
            if is(&m) {
                return m["data"].clone();
            }
            self.backlog.push_back(m);
        }
    }

    async fn event(&mut self, name: &str) -> Value {
        self.event_where(name, |_| true).await
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

async fn with_tier(
    server: &helm_agent::ServerHandle,
    dir: &Path,
    phone: &Phone,
    tier: &str,
) -> Client {
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

fn audit(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("audit.log")).unwrap_or_default()
}

#[tokio::test]
async fn hello_advertises_p5_and_tiers_are_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _fake) = start_fake_with(dir.path(), |_| {}).await;
    let mut ws = try_connect_path(server.port(), server.fingerprint(), "/ws")
        .await
        .unwrap();
    let hello = recv(&mut ws).await;
    assert!(hello["features"].as_array().unwrap().contains(&json!("p5")));
    drop(ws);
    let names = server.state().router().method_names();
    for m in [
        "record.start", "record.stop", "term.open", "term.write", "term.resize", "term.close",
        "notify.push", "plugins.list", "plugins.set", "triggers.get", "triggers.set",
        "ha.config.set", "ha.states",
    ] {
        assert!(names.contains(&m), "{m} missing");
    }
    let (_admin, phone) = paired(&server).await;
    let mut remote = with_tier(&server, dir.path(), &phone, "remote").await;
    for m in ["term.open", "term.write", "term.close", "plugins.set", "ha.config.set"] {
        assert_eq!(remote.err(m, json!({})).await, "forbidden", "{m}");
    }
    let mut viewer = with_tier(&server, dir.path(), &phone, "viewer").await;
    for m in ["record.start", "notify.push", "triggers.set"] {
        assert_eq!(viewer.err(m, json!({})).await, "forbidden", "{m}");
    }
    viewer.ok("plugins.list", json!({})).await;
    viewer.ok("triggers.get", json!({})).await;
    server.shutdown().await;
}

#[tokio::test]
async fn record_without_stream_uses_own_encoder_and_stops_on_disconnect() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |_| {}).await;
    let (mut c, _) = paired(&server).await;
    assert_eq!(c.err("record.stop", json!({})).await, "bad_params");
    let r = c.ok("record.start", json!({"audio": true})).await;
    let path = r["path"].as_str().unwrap().to_string();
    assert!(path.starts_with(dir.path().join("videos").to_str().unwrap()), "{path}");
    assert!(path.ends_with(".mp4"));
    assert_eq!(c.err("record.start", json!({})).await, "bad_params");
    let rec = fake.recordings()[0].clone();
    assert!(rec.config.audio && rec.config.width == 1920 && rec.config.fps == 30);
    wait_until(|| {
        rec.video_frames.load(Ordering::SeqCst) > 5 && rec.audio_packets.load(Ordering::SeqCst) > 2
    })
    .await;
    assert!(rec.keyframes.load(Ordering::SeqCst) >= 1);
    let video = fake.video().unwrap();
    let done = c.ok("record.stop", json!({})).await;
    assert_eq!(done["path"], path);
    assert!(done["duration_ms"].as_u64().unwrap() > 0);
    assert!(Path::new(&path).exists());
    assert!(video.is_stopped(), "private encoder stopped");
    assert_eq!(fake.audio_streams(), 0);

    // A second recording is finalized when the phone disconnects.
    c.ok("record.start", json!({})).await;
    let rec2 = fake.recordings()[1].clone();
    assert!(!rec2.config.audio);
    wait_until(|| rec2.video_frames.load(Ordering::SeqCst) > 2).await;
    drop(c);
    wait_until(|| rec2.stopped.load(Ordering::SeqCst)).await;
    wait_until(|| server.state().p5.recording_count() == 0).await;
    server.shutdown().await;
}

#[tokio::test]
async fn terminal_open_write_resize_close_and_audit() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |_| {}).await;
    let (mut c, _) = paired(&server).await;
    assert_eq!(c.err("term.open", json!({"shell":"bash","cols":80,"rows":24})).await, "bad_params");
    assert_eq!(c.err("term.open", json!({"shell":"cmd","cols":0,"rows":24})).await, "bad_params");
    let id = c
        .ok("term.open", json!({"shell":"powershell","cols":120,"rows":30}))
        .await["term_id"]
        .as_str()
        .unwrap()
        .to_string();
    let term = fake.terminals()[0].clone();
    assert_eq!(*term.size.lock().unwrap(), (120, 30));
    let b64 = |s: &[u8]| {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(s)
    };
    c.ok("term.write", json!({"term_id": id, "data_b64": b64(b"dir\r")})).await;
    let out = c.event("term.output").await;
    assert_eq!(out, json!({"term_id": id, "data_b64": b64(b"dir\r")}));
    assert_eq!(term.input.lock().unwrap().as_slice(), b"dir\r");
    assert_eq!(c.err("term.write", json!({"term_id": id, "data_b64": "!!"})).await, "bad_params");
    assert_eq!(c.err("term.write", json!({"term_id": "nope", "data_b64": ""})).await, "bad_params");
    c.ok("term.resize", json!({"term_id": id, "cols": 100, "rows": 40})).await;
    assert_eq!(*term.size.lock().unwrap(), (100, 40));
    // The shell exits by itself.
    c.ok("term.write", json!({"term_id": id, "data_b64": b64(b"exit\r")})).await;
    assert_eq!(c.event("term.exit").await, json!({"term_id": id, "code": 0}));
    assert_eq!(c.err("term.close", json!({"term_id": id})).await, "bad_params");

    // term.close kills; disconnect kills the rest.
    let id2 = c.ok("term.open", json!({"shell":"cmd","cols":80,"rows":24})).await["term_id"]
        .as_str()
        .unwrap()
        .to_string();
    c.ok("term.close", json!({"term_id": id2})).await;
    let t2 = fake.terminals()[1].clone();
    assert!(t2.killed.load(Ordering::SeqCst));
    c.ok("term.open", json!({"shell":"cmd","cols":80,"rows":24})).await;
    let t3 = fake.terminals()[2].clone();
    drop(c);
    wait_until(|| t3.killed.load(Ordering::SeqCst)).await;

    let log = audit(dir.path());
    for needle in ["term.open", "term.write", "term.resize", "term.close", "bytes=4"] {
        assert!(log.contains(needle), "{needle} not audited:\n{log}");
    }
    assert!(!log.contains("ZGlyDQ"), "keystrokes must not be logged");
    server.shutdown().await;
}

#[tokio::test]
async fn pen_frames_map_to_the_monitor_and_leave_when_idle() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |_| {}).await;
    let (mut c, _) = paired(&server).await;
    send(&mut c.ws, json!({"type":"in","k":"pn","x":0.5,"y":0.5,"state":"hover"})).await;
    send(
        &mut c.ws,
        json!({"type":"in","k":"pn","x":1.0,"y":0.0,"pressure":0.6,"tilt_x":15,"tilt_y":-5,"state":"down","eraser":true}),
    )
    .await;
    send(&mut c.ws, json!({"type":"in","k":"pn","x":1.0,"y":0.0,"state":"up"})).await;
    // No more frames: the agent sends "leave" itself after ~300 ms.
    wait_until(|| fake.pen_samples().len() == 4).await;
    let s = fake.pen_samples();
    assert_eq!((s[0].x, s[0].y, s[0].phase), (960, 540, PenPhase::Hover));
    assert_eq!((s[1].x, s[1].y, s[1].phase), (1919, 0, PenPhase::Contact));
    assert!((s[1].pressure - 0.6).abs() < 1e-6 && s[1].tilt_x == 15.0 && s[1].tilt_y == -5.0);
    assert!(s[1].eraser);
    assert_eq!(s[2].phase, PenPhase::Up);
    assert_eq!((s[3].phase, s[3].x), (PenPhase::Leave, 1919));
    send(&mut c.ws, json!({"type":"in","k":"pn","x":0.5,"state":"down"})).await;
    assert_eq!(c.event("input.error").await["code"], "bad_params");
    // Disconnect while touching: leave, then the device is released.
    send(&mut c.ws, json!({"type":"in","k":"pn","x":0.1,"y":0.1,"state":"down","pressure":1})).await;
    wait_until(|| fake.pen_samples().len() == 5).await;
    drop(c);
    wait_until(|| fake.pen_releases() == 1).await;
    assert_eq!(fake.pen_samples().last().unwrap().phase, PenPhase::Leave);
    server.shutdown().await;
}

#[tokio::test]
async fn notify_push_shows_a_toast_and_reports_clicks() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |_| {}).await;
    let (mut c, _) = paired(&server).await;
    use base64::Engine;
    let icon = base64::engine::general_purpose::STANDARD.encode(helm_agent::platform::fake::TINY_PNG);
    c.ok(
        "notify.push",
        json!({"app":"WhatsApp","title":"Alice","body":"Lunch?","icon_png_b64":icon,
               "actions":[{"id":"read","label":"Mark read"}],"reply":true,"key":"wa-42"}),
    )
    .await;
    let t = &fake.toasts()[0];
    assert_eq!((t.title.as_str(), t.body.as_str()), ("Alice", "Lunch?"));
    assert_eq!(t.tag.as_deref(), Some("wa-42"));
    assert!(t.image.as_ref().unwrap().starts_with(dir.path()));
    assert!(fake.click_toast(0, Some("read"), None));
    assert_eq!(c.event("notify.action").await, json!({"key":"wa-42","action_id":"read"}));
    assert!(fake.click_toast(0, Some("reply"), Some("on my way")));
    assert_eq!(
        c.event("notify.action").await,
        json!({"key":"wa-42","action_id":"reply","reply_text":"on my way"})
    );
    assert!(fake.click_toast(0, None, None));
    assert_eq!(c.event("notify.action").await["action_id"], Value::Null);
    assert_eq!(c.err("notify.push", json!({"title":"x"})).await, "bad_params");
    server.shutdown().await;
}

const PLUGIN_WAT: &str = r#"(module
  (import "helm" "run_deck_action" (func $run (param i32 i32) (result i32)))
  (import "helm" "emit_event" (func $emit (param i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 0) "{\"launch\":\"Fake.Notes\"}")
  (data (i32.const 64) "hello{\"n\":1}")
  (func (export "init") (result i32)
    (call $run (i32.const 0) (i32.const 23)))
  (func (export "alloc") (param i32) (result i32) (i32.const 1024))
  (func (export "handle") (param i32 i32) (result i32)
    (call $emit (i32.const 64) (i32.const 5) (i32.const 69) (i32.const 7))))"#;

fn install_plugin(dir: &Path, folder: &str, manifest: Value, wasm: &[u8]) {
    let p = dir.join("plugins").join(folder);
    std::fs::create_dir_all(&p).unwrap();
    std::fs::write(p.join("plugin.json"), manifest.to_string()).unwrap();
    std::fs::write(p.join("module.wasm"), wasm).unwrap();
}

#[tokio::test]
async fn plugins_list_grant_run_and_report_errors() {
    let dir = tempfile::tempdir().unwrap();
    let wasm = wat::parse_str(PLUGIN_WAT).unwrap();
    install_plugin(
        dir.path(),
        "lights",
        json!({"id":"lights","name":"Lights","version":"1.0.0","capabilities":["deck.action","notify"]}),
        &wasm,
    );
    install_plugin(
        dir.path(),
        "broken",
        json!({"id":"broken","name":"Broken","version":"0.1","capabilities":[]}),
        b"not wasm",
    );
    let (server, fake) = start_fake_with(dir.path(), |c| {
        c.wifi_poll = Duration::from_millis(50);
    })
    .await;
    let (mut c, _) = paired(&server).await;
    let list = c.ok("plugins.list", json!({})).await["plugins"].clone();
    assert_eq!(list.as_array().unwrap().len(), 2);
    assert_eq!(list[1]["id"], "lights");
    assert_eq!(list[1]["enabled"], false);
    assert_eq!(list[1]["capabilities"], json!(["deck.action", "notify"]));

    // Enabled without the deck.action grant: init's deck action is denied -> load error.
    let list = c
        .ok("plugins.set", json!({"id":"lights","enabled":true,"granted":["notify"]}))
        .await["plugins"]
        .clone();
    assert!(list[1]["error"].as_str().unwrap().contains("init"), "{list}");
    assert!(!fake.calls().iter().any(|c| c == "launch_app:Fake.Notes"));
    // With it: init launches the app.
    let list = c
        .ok("plugins.set", json!({"id":"lights","enabled":true,"granted":["deck.action","notify"]}))
        .await["plugins"]
        .clone();
    assert!(list[1].get("error").is_none(), "{list}");
    assert_eq!(list[1]["running"], true);
    wait_until(|| fake.calls().iter().any(|c| c == "launch_app:Fake.Notes")).await;
    // Trigger events reach the plugin's `handle`, which emits an event.
    fake.with(|s| s.wifi_ssid = Some("Home".into()));
    let ev = c.event("plugin.event").await;
    assert_eq!(ev, json!({"plugin_id":"lights","name":"hello","data":{"n":1}}));

    let list = c.ok("plugins.set", json!({"id":"broken","enabled":true})).await["plugins"].clone();
    assert!(list[0]["error"].is_string());
    assert_eq!(c.err("plugins.set", json!({"id":"nope","enabled":true})).await, "bad_params");
    assert_eq!(
        c.err("plugins.set", json!({"id":"lights","enabled":true,"granted":["root"]})).await,
        "bad_params"
    );
    // Settings persist.
    let saved = std::fs::read_to_string(dir.path().join("plugins.json")).unwrap();
    assert!(saved.contains("deck.action"));
    server.shutdown().await;
}

#[tokio::test]
async fn triggers_fire_on_foreground_phone_and_wifi() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |c| {
        c.wifi_poll = Duration::from_millis(50);
    })
    .await;
    let (mut c, phone) = paired(&server).await;
    assert_eq!(c.ok("triggers.get", json!({})).await, json!({"triggers": []}));
    let triggers = json!([
        {"id":"game","when":{"app_start":"game.exe"},"action":{"media":"pause"}},
        {"id":"phone","when":{"phone_connected":phone.device_id},"action":{"launch":"Fake.Notes"}},
        {"id":"home","when":{"wifi":"HomeNet"},"action":{"volume_step":0.1}},
        {"id":"morning","when":{"time":"07:30 mon-fri"},"action":{"text":"hi"}}
    ]);
    c.ok("triggers.set", json!({"triggers": triggers})).await;
    assert_eq!(c.ok("triggers.get", json!({})).await["triggers"], triggers);
    assert_eq!(
        c.err("triggers.set", json!({"triggers":[{"id":"x","when":{"time":"99:99"},"action":{"text":"a"}}]}))
            .await,
        "bad_params"
    );
    assert_eq!(c.ok("triggers.get", json!({})).await["triggers"], triggers);

    wait_until(|| fake.emit_foreground("Game.exe")).await;
    wait_until(|| fake.calls().iter().any(|c| c == "media_control:Pause")).await;

    fake.with(|s| s.wifi_ssid = Some("HomeNet".into()));
    wait_until(|| fake.calls().iter().any(|c| c.starts_with("set_volume:0.60"))).await;

    // Reconnect: phone_connected fires once the device's last session ended.
    drop(c);
    wait_until(|| server.state().sessions().is_empty()).await;
    let (_ws, _) = phone.auth(&server).await;
    wait_until(|| fake.calls().iter().any(|c| c == "launch_app:Fake.Notes")).await;
    assert!(std::fs::read_to_string(dir.path().join("triggers.json"))
        .unwrap()
        .contains("morning"));
    server.shutdown().await;
}

#[tokio::test]
async fn home_assistant_config_states_and_deck_action() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |_| {}).await;
    let (mut c, _) = paired(&server).await;
    assert_eq!(c.err("ha.states", json!({})).await, "bad_params");
    assert_eq!(
        c.err("ha.config.set", json!({"url":"ftp://x","token":"t"})).await,
        "bad_params"
    );
    c.ok("ha.config.set", json!({"url":"http://ha.local:8123/","token":"llat-secret"}))
        .await;
    let saved = std::fs::read_to_string(dir.path().join("ha.json")).unwrap();
    assert!(!saved.contains("llat-secret"), "token must be encrypted: {saved}");
    let log = audit(dir.path());
    assert!(log.contains("ha.config.set") && !log.contains("llat-secret"), "{log}");

    let http = fake.http_fake();
    http.route(
        "GET",
        "http://ha.local:8123/api/states",
        200,
        r#"[{"entity_id":"light.desk","state":"on","attributes":{"friendly_name":"Desk"}}]"#,
    );
    let states = c.ok("ha.states", json!({})).await["states"].clone();
    assert_eq!(states[0]["entity_id"], "light.desk");
    let req = http.requests().last().unwrap().clone();
    assert!(req.headers.contains(&("Authorization".into(), "Bearer llat-secret".into())));

    http.route("POST", "http://ha.local:8123/api/services/light/toggle", 200, "[]");
    c.ok(
        "deck.set",
        json!({"buttons":[{"id":"l","label":"Desk","color":"#ffaa00",
               "action":{"ha":{"service":"light.toggle","entity_id":"light.desk"}}}]}),
    )
    .await;
    c.ok("deck.run", json!({"id":"l"})).await;
    let req = http.requests().last().unwrap().clone();
    assert_eq!(req.method, "POST");
    assert_eq!(serde_json::from_slice::<Value>(&req.body).unwrap(), json!({"entity_id":"light.desk"}));
    assert_eq!(
        c.err("deck.set", json!({"buttons":[{"id":"b","label":"","color":"",
               "action":{"ha":{"service":"Light Toggle"}}}]})).await,
        "bad_params"
    );
    // HA errors surface (404 from the server).
    http.route("POST", "http://ha.local:8123/api/services/light/toggle", 401, "");
    assert_eq!(c.err("deck.run", json!({"id":"l"})).await, "internal");
    server.shutdown().await;
}
