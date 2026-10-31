//! Phase 1 end-to-end tests (PROTOCOL §9) against a `FakePlatform`: nothing here touches the
//! real PC (no input, launches, volume, brightness, clipboard, media or power changes).

mod common;

use std::collections::VecDeque;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use common::*;
use futures_util::{SinkExt, StreamExt};
use helm_agent::platform::PlatformEvent;
use helm_agent::platform::fake::{TINY_PNG, sample_media, sample_window};
use helm_win_input::{MouseButton, Op};
use helm_win_system::power::PowerAction;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio_tungstenite::tungstenite::Message;

/// A phone session that keeps events and binary frames that arrive between responses.
struct Client {
    ws: Ws,
    backlog: VecDeque<Value>,
    binary: Vec<Vec<u8>>,
    next_id: i64,
}

impl Client {
    fn new(ws: Ws) -> Client {
        Client {
            ws,
            backlog: VecDeque::new(),
            binary: Vec::new(),
            next_id: 1,
        }
    }

    async fn read(&mut self, wait: Duration) -> Option<Value> {
        loop {
            let msg = tokio::time::timeout(wait, self.ws.next()).await.ok()?;
            match msg.expect("connection closed").expect("websocket error") {
                Message::Text(t) => return Some(serde_json::from_str(t.as_str()).unwrap()),
                Message::Binary(b) => self.binary.push(b.to_vec()),
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

    /// Call and require success; returns `result`.
    async fn ok(&mut self, method: &str, params: Value) -> Value {
        let r = self.call(method, params).await;
        assert_eq!(r["ok"], true, "{method} failed: {r}");
        r["result"].clone()
    }

    /// Call and require an error; returns its code.
    async fn err(&mut self, method: &str, params: Value) -> String {
        let r = self.call(method, params).await;
        assert_eq!(r["ok"], false, "{method} should fail: {r}");
        r["error"]["code"].as_str().unwrap().to_string()
    }

    async fn event(&mut self, name: &str) -> Value {
        if let Some(i) = self
            .backlog
            .iter()
            .position(|m| m["type"] == "event" && m["name"] == name)
        {
            return self.backlog.remove(i).unwrap()["data"].clone();
        }
        loop {
            let m = self
                .read(Duration::from_secs(5))
                .await
                .unwrap_or_else(|| panic!("no {name} event"));
            if m["type"] == "event" && m["name"] == name {
                return m["data"].clone();
            }
            self.backlog.push_back(m);
        }
    }

    /// Assert no `name` event arrives within `ms`.
    async fn no_event(&mut self, name: &str, ms: u64) {
        let is = |m: &Value| m["type"] == "event" && m["name"] == name;
        assert!(!self.backlog.iter().any(is), "unexpected {name}");
        let deadline = tokio::time::Instant::now() + Duration::from_millis(ms);
        while let Some(m) = self
            .read(deadline.saturating_duration_since(tokio::time::Instant::now()))
            .await
        {
            assert!(!is(&m), "unexpected {name}: {m}");
            self.backlog.push_back(m);
        }
    }

    async fn input(&mut self, frame: Value) {
        send(&mut self.ws, frame).await;
    }

    async fn binary(&mut self, data: Vec<u8>) {
        self.ws.send(Message::binary(data)).await.unwrap();
    }
}

async fn paired(server: &helm_agent::ServerHandle) -> (Client, Phone) {
    let phone = Phone::new("Pixel");
    let token = server.new_pairing().token;
    let (ws, ready) = phone.pair(server, &token).await;
    assert_eq!(ready["type"], "ready", "{ready}");
    (Client::new(ws), phone)
}

async fn second_session(server: &helm_agent::ServerHandle, phone: &Phone) -> Client {
    let (ws, ready) = phone.auth(server).await;
    assert_eq!(ready["type"], "ready", "{ready}");
    Client::new(ws)
}

/// Change a paired device's tier in devices.json, then connect with it.
async fn with_tier(
    server: &helm_agent::ServerHandle,
    dir: &std::path::Path,
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
    Client::new(ws)
}

fn b64(data: &[u8]) -> String {
    STANDARD.encode(data)
}

fn frame(id_hex: &str, offset: u64, data: &[u8]) -> Vec<u8> {
    let mut f = hex::decode(id_hex).unwrap();
    f.extend_from_slice(&offset.to_be_bytes());
    f.extend_from_slice(data);
    f
}

async fn wait_for(mut cond: impl FnMut() -> bool) {
    for _ in 0..100 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    panic!("condition not met in time");
}

#[tokio::test]
async fn hello_advertises_p1_and_every_method_exists() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _fake) = start_fake_with(dir.path(), |_| {}).await;
    let mut ws = connect(&server).await;
    let hello = recv(&mut ws).await;
    assert_eq!(hello["features"], json!(["p1"]));
    let names = server.state().router().method_names();
    for m in [
        "apps.list",
        "apps.icon",
        "apps.launch",
        "open",
        "windows.list",
        "windows.focus",
        "windows.minimize",
        "windows.maximize",
        "windows.restore",
        "windows.close",
        "dock.get",
        "dock.set",
        "media.state",
        "media.artwork",
        "media.control",
        "audio.get",
        "audio.set",
        "display.brightness.list",
        "display.brightness.set",
        "power.action",
        "power.cancel",
        "power.status",
        "deck.get",
        "deck.set",
        "deck.run",
        "clipboard.get",
        "clipboard.set",
        "clipboard.watch",
        "file.upload.begin",
        "file.upload.end",
        "file.download",
        "screen.monitors",
        "screen.capture",
        "system.info",
        "system.ping",
    ] {
        assert!(names.contains(&m), "missing {m}");
    }
    server.shutdown().await;
}

#[tokio::test]
async fn apps_open_and_windows() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |_| {}).await;
    let (mut c, _) = paired(&server).await;

    let apps = c.ok("apps.list", json!({})).await;
    assert_eq!(
        apps["apps"][0],
        json!({"app_id":"Fake.Notes","name":"Notes","kind":"desktop"})
    );
    assert_eq!(apps["apps"][1]["kind"], "packaged");
    let icon = c
        .ok("apps.icon", json!({"app_id":"Fake.Notes","size":48}))
        .await;
    assert_eq!(
        STANDARD.decode(icon["png"].as_str().unwrap()).unwrap(),
        TINY_PNG
    );
    assert_eq!(
        c.err("apps.icon", json!({"app_id":"nope"})).await,
        "bad_params"
    );
    assert_eq!(c.err("apps.icon", json!({})).await, "bad_params");

    c.ok("apps.launch", json!({"app_id":"Fake.Notes"})).await;
    assert_eq!(
        c.err("apps.launch", json!({"app_id":"Evil"})).await,
        "bad_params"
    );

    c.ok("open", json!({"target":"https://example.com/x"}))
        .await;
    let existing = dir.path().join("profile");
    c.ok("open", json!({"target": existing.to_str().unwrap()}))
        .await;
    for bad in [
        "relative\\path",
        "ftp://x",
        "file:///C:/x",
        "\\\\server\\share",
        "C:\\does\\not\\exist",
    ] {
        assert_eq!(
            c.err("open", json!({"target": bad})).await,
            "bad_params",
            "{bad}"
        );
    }

    let w = c.ok("windows.list", json!({})).await;
    assert_eq!(
        w["windows"][0],
        json!({"hwnd":100,"title":"Notes","pid":1100,"app_id":"Fake.Notes",
               "is_minimized":false,"is_foreground":true})
    );
    let f = c.ok("windows.focus", json!({"hwnd":200})).await;
    assert_eq!(f, json!({"focused":true,"method":"set_foreground_window"}));
    for m in [
        "windows.minimize",
        "windows.maximize",
        "windows.restore",
        "windows.close",
    ] {
        assert_eq!(c.ok(m, json!({"hwnd":200})).await, json!({}));
    }
    assert_eq!(
        c.err("windows.close", json!({"hwnd":999})).await,
        "bad_params"
    );
    assert_eq!(
        c.err("windows.close", json!({"hwnd":"x"})).await,
        "bad_params"
    );

    let target = existing.to_str().unwrap();
    assert_eq!(
        fake.calls(),
        vec![
            "app_icon_png:Fake.Notes:48".to_string(),
            "launch_app:Fake.Notes".into(),
            "open_target:https://example.com/x".into(),
            format!("open_target:{target}"),
            "focus_window:200".into(),
            "window_command:200:Minimize".into(),
            "window_command:200:Maximize".into(),
            "window_command:200:Restore".into(),
            "window_command:200:Close".into(),
        ]
    );
    server.shutdown().await;
}

#[tokio::test]
async fn media_audio_brightness_screen_and_info() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |_| {}).await;
    let (mut c, _) = paired(&server).await;

    let st = c.ok("media.state", json!({})).await;
    assert_eq!(st["state"]["title"], "Fake Song");
    assert_eq!(st["state"]["status"], "playing");
    let art = c.ok("media.artwork", json!({})).await;
    assert_eq!(
        STANDARD.decode(art["png"].as_str().unwrap()).unwrap(),
        TINY_PNG
    );
    c.ok("media.control", json!({"action":"toggle_play_pause"}))
        .await;
    c.ok("media.control", json!({"action":{"seek_ms":5000}}))
        .await;
    assert_eq!(
        c.err("media.control", json!({"action":"explode"})).await,
        "bad_params"
    );
    fake.with(|s| {
        s.media = None;
        s.artwork = None;
    });
    assert_eq!(c.ok("media.state", json!({})).await, json!({"state":null}));
    assert_eq!(c.ok("media.artwork", json!({})).await, json!({"png":null}));

    assert_eq!(
        c.ok("audio.get", json!({})).await,
        json!({"level":0.5,"muted":false})
    );
    c.ok("audio.set", json!({"level":0.25})).await;
    c.ok("audio.set", json!({"muted":true})).await;
    assert_eq!(
        c.ok("audio.get", json!({})).await,
        json!({"level":0.25,"muted":true})
    );
    assert_eq!(c.err("audio.set", json!({"level":1.5})).await, "bad_params");
    assert_eq!(c.err("audio.set", json!({})).await, "bad_params");

    let d = c.ok("display.brightness.list", json!({})).await;
    assert_eq!(
        d["displays"][0],
        json!({"id":"wmi:0","name":"Built-in display","kind":"internal","level":70,"supported":true})
    );
    c.ok("display.brightness.set", json!({"id":"wmi:0","level":40}))
        .await;
    assert_eq!(
        c.err("display.brightness.set", json!({"id":"wmi:0","level":101}))
            .await,
        "bad_params"
    );
    assert_eq!(
        c.err("display.brightness.set", json!({"id":"x","level":10}))
            .await,
        "bad_params"
    );

    let mons = c.ok("screen.monitors", json!({})).await;
    assert_eq!(
        mons["monitors"][0],
        json!({"id":"\\\\.\\DISPLAY1","name":"Fake Monitor","x":0,"y":0,"width":1920,
               "height":1080,"is_primary":true})
    );
    let shot = c.ok("screen.capture", json!({})).await;
    assert_eq!(
        STANDARD.decode(shot["png"].as_str().unwrap()).unwrap(),
        TINY_PNG
    );
    c.ok("screen.capture", json!({"monitor_id":"\\\\.\\DISPLAY1"}))
        .await;
    assert_eq!(
        c.err("screen.capture", json!({"monitor_id":"nope"})).await,
        "bad_params"
    );

    let info = c.ok("system.info", json!({})).await;
    assert_eq!(
        info["macs"],
        json!([{"mac":"aa:bb:cc:dd:ee:ff","adapter_name":"Ethernet","locally_administered":false}])
    );
    assert_eq!(info["battery"]["percent"], 80);

    assert_eq!(
        fake.calls(),
        vec![
            "media_control:TogglePlayPause",
            "media_control:SeekMs(5000)",
            "set_volume:0.25",
            "set_muted:true",
            "set_brightness:wmi:0:40",
            "screenshot_png:-",
            "screenshot_png:\\\\.\\DISPLAY1",
        ]
    );
    server.shutdown().await;
}

#[tokio::test]
async fn power_is_dry_run_scheduled_and_broadcast() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _fake) = start_fake_with(dir.path(), |_| {}).await;
    let dry = server
        .state()
        .power_dry_run
        .clone()
        .expect("dry-run backend");
    let (mut a, phone) = paired(&server).await;
    let mut b = second_session(&server, &phone).await;

    assert_eq!(
        a.ok("power.status", json!({})).await,
        json!({"scheduled":null})
    );
    let r = a
        .ok("power.action", json!({"action":"shutdown","delay_s":3600}))
        .await;
    assert_eq!(r["scheduled"]["action"], "shutdown");
    let due = r["scheduled"]["due_unix_ms"].as_u64().unwrap();
    assert!(due > 1_600_000_000_000);
    assert_eq!(b.event("power.scheduled").await, r);
    assert_eq!(b.ok("power.status", json!({})).await, r);
    assert_eq!(
        a.ok("power.cancel", json!({})).await,
        json!({"scheduled":null})
    );
    assert_eq!(b.event("power.scheduled").await, json!({"scheduled":null}));
    assert!(dry.executed().is_empty());

    // Immediate: answered first, then run (dry run).
    assert_eq!(
        a.ok("power.action", json!({"action":"lock"})).await,
        json!({"scheduled":null})
    );
    wait_for(|| dry.executed() == vec![PowerAction::Lock]).await;

    // A short timer fires and announces that nothing is pending any more.
    a.ok("power.action", json!({"action":"sleep","delay_s":1}))
        .await;
    assert_eq!(
        b.event("power.scheduled").await["scheduled"]["action"],
        "sleep"
    );
    assert_eq!(b.event("power.scheduled").await, json!({"scheduled":null}));
    assert_eq!(dry.executed(), vec![PowerAction::Lock, PowerAction::Sleep]);

    assert_eq!(
        a.err("power.action", json!({"action":"format"})).await,
        "bad_params"
    );
    assert_eq!(a.err("power.action", json!({})).await, "bad_params");
    let audit = std::fs::read_to_string(dir.path().join("audit.log")).unwrap();
    assert!(audit.contains("\tpower\t") && audit.contains("\tpower_cancel\t"));
    server.shutdown().await;
}

#[tokio::test]
async fn dock_and_deck_persist_broadcast_and_run() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |_| {}).await;
    let (mut a, phone) = paired(&server).await;
    let mut b = second_session(&server, &phone).await;

    assert_eq!(a.ok("dock.get", json!({})).await, json!({"pages":[]}));
    let pages = json!([
        [{"id":"t1","kind":"app","app_id":"Fake.Notes","label":"Notes"}],
        [{"id":"t2","kind":"open","target":"https://example.com"}]
    ]);
    a.ok("dock.set", json!({"pages": pages})).await;
    assert_eq!(b.event("dock.changed").await, json!({"pages": pages}));
    assert_eq!(a.event("dock.changed").await, json!({"pages": pages}));
    assert_eq!(b.ok("dock.get", json!({})).await, json!({"pages": pages}));
    assert!(dir.path().join("dock.json").exists());
    let bad = json!([[{"id":"x","kind":"app"}]]);
    assert_eq!(a.err("dock.set", json!({"pages": bad})).await, "bad_params");
    assert_eq!(
        a.err("dock.set", json!({"pages": "nope"})).await,
        "bad_params"
    );

    let buttons = json!([
        {"id":"hk","label":"Task Mgr","color":"#f00","action":{"hotkey":"ctrl+shift+esc"}},
        {"id":"tx","label":"Hi","color":"#0f0","icon":"wave","action":{"text":"hi"}},
        {"id":"ln","label":"Notes","color":"#00f","action":{"launch":"Fake.Notes"}},
        {"id":"op","label":"Web","color":"#00f","action":{"open":"https://example.com"}},
        {"id":"md","label":"Play","color":"#00f","action":{"media":"play"}},
        {"id":"vu","label":"Vol+","color":"#00f","action":{"volume_step":0.1}},
        {"id":"dl","label":"Wait","color":"#00f","action":{"delay_ms":50}}
    ]);
    a.ok("deck.set", json!({"buttons": buttons})).await;
    assert_eq!(b.event("deck.changed").await, json!({"buttons": buttons}));
    assert_eq!(
        b.ok("deck.get", json!({})).await,
        json!({"buttons": buttons})
    );
    assert!(dir.path().join("deck.json").exists());
    let bad = json!([{"id":"x","label":"","color":"","action":{"hotkey":"ctrl+nokey"}}]);
    assert_eq!(
        a.err("deck.set", json!({"buttons": bad})).await,
        "bad_params"
    );

    for id in ["hk", "tx", "ln", "op", "md", "vu", "dl"] {
        a.ok("deck.run", json!({"id": id})).await;
    }
    assert_eq!(
        a.err("deck.run", json!({"id":"missing"})).await,
        "bad_params"
    );
    assert_eq!(
        fake.calls(),
        vec![
            "launch_app:Fake.Notes",
            "open_target:https://example.com",
            "media_control:Play",
            "set_volume:0.60",
        ]
    );
    let ops = fake.ops();
    // ctrl+shift+esc pressed in order and released in reverse, then "hi" typed.
    assert_eq!(
        &ops[..6],
        &[
            Op::Key {
                vk: 0x11,
                down: true
            },
            Op::Key {
                vk: 0x10,
                down: true
            },
            Op::Key {
                vk: 0x1B,
                down: true
            },
            Op::Key {
                vk: 0x1B,
                down: false
            },
            Op::Key {
                vk: 0x10,
                down: false
            },
            Op::Key {
                vk: 0x11,
                down: false
            },
        ]
    );
    assert!(ops.contains(&Op::Unicode {
        unit: 'h' as u16,
        down: true
    }));

    // Layout survives a restart.
    server.shutdown().await;
    let (server, _fake) = start_fake_with(dir.path(), |_| {}).await;
    let mut c = second_session(&server, &phone).await;
    assert_eq!(c.ok("dock.get", json!({})).await, json!({"pages": pages}));
    assert_eq!(
        c.ok("deck.get", json!({})).await,
        json!({"buttons": buttons})
    );
    server.shutdown().await;
}

#[tokio::test]
async fn clipboard_get_set_and_watch() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |_| {}).await;
    let (mut a, phone) = paired(&server).await;
    let mut b = second_session(&server, &phone).await;

    assert_eq!(
        a.ok("clipboard.get", json!({})).await,
        json!({"text":"fake clipboard"})
    );
    a.ok("clipboard.set", json!({"text":"from phone"})).await;
    a.ok("clipboard.set", json!({"png": b64(TINY_PNG)})).await;
    assert_eq!(
        a.ok("clipboard.get", json!({})).await,
        json!({"png": b64(TINY_PNG)})
    );
    assert_eq!(a.err("clipboard.set", json!({})).await, "bad_params");
    assert_eq!(
        a.err("clipboard.set", json!({"text":"a","png":"b"})).await,
        "bad_params"
    );
    assert_eq!(
        a.err("clipboard.set", json!({"png":"%%%"})).await,
        "bad_params"
    );
    assert_eq!(
        a.err("clipboard.set", json!({"png": b64(b"not png")}))
            .await,
        "bad_params"
    );
    assert_eq!(a.err("clipboard.watch", json!({})).await, "bad_params");

    a.ok("clipboard.watch", json!({"on": true})).await;
    fake.with(|s| s.clipboard_text = Some("copied on PC".into()));
    assert!(fake.emit(PlatformEvent::Clipboard {
        has_text: true,
        has_image: false
    }));
    assert_eq!(
        a.event("clipboard.changed").await,
        json!({"text":"copied on PC","has_image":false})
    );
    b.no_event("clipboard.changed", 300).await;

    a.ok("clipboard.watch", json!({"on": false})).await;
    fake.emit(PlatformEvent::Clipboard {
        has_text: false,
        has_image: true,
    });
    a.no_event("clipboard.changed", 300).await;
    assert_eq!(
        fake.calls(),
        vec!["set_clipboard_text:from phone", "set_clipboard_png:69"]
    );
    server.shutdown().await;
}

#[tokio::test]
async fn watcher_events_reach_every_session() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |_| {}).await;
    assert!(fake.watchers_running());
    let (mut a, phone) = paired(&server).await;
    let mut b = second_session(&server, &phone).await;

    fake.emit(PlatformEvent::Media(Some(sample_media("Next Song"))));
    fake.emit(PlatformEvent::Volume(helm_agent::platform::VolumeState {
        level: 0.75,
        muted: true,
    }));
    fake.emit(PlatformEvent::TextFocus(true));
    for c in [&mut a, &mut b] {
        assert_eq!(
            c.event("media.changed").await["state"]["title"],
            "Next Song"
        );
        assert_eq!(
            c.event("audio.changed").await,
            json!({"level":0.75,"muted":true})
        );
        assert_eq!(c.event("input.textFocus").await, json!({"focused":true}));
    }
    fake.emit(PlatformEvent::Media(None));
    assert_eq!(a.event("media.changed").await, json!({"state":null}));

    // Window polling: a new window → windows.changed (empty data) to everyone.
    tokio::time::sleep(Duration::from_millis(300)).await;
    fake.with(|s| s.windows.push(sample_window(300, "New")));
    assert_eq!(a.event("windows.changed").await, json!({}));
    assert_eq!(b.event("windows.changed").await, json!({}));
    server.shutdown().await;
}

#[tokio::test]
async fn input_frames_inject_without_replies_and_release_on_disconnect() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |_| {}).await;
    let (mut c, phone) = paired(&server).await;

    c.input(json!({"type":"in","k":"mm","dx":3.5,"dy":-2}))
        .await;
    c.input(json!({"type":"in","k":"mm","dx":0.5,"dy":0})).await;
    c.input(json!({"type":"in","k":"mb","button":"left","down":true}))
        .await;
    c.input(json!({"type":"in","k":"ms","dx":0,"dy":1})).await;
    c.input(json!({"type":"in","k":"kd","vk":16})).await;
    c.input(json!({"type":"in","k":"tx","text":"a"})).await;
    c.input(json!({"type":"in","k":"sc","keys":[17,67]})).await;
    // No replies: the next message is the ping's response.
    send(
        &mut c.ws,
        json!({"type":"req","id":99,"method":"system.ping"}),
    )
    .await;
    let next = c.read(Duration::from_secs(5)).await.unwrap();
    assert_eq!(next["id"], 99, "{next}");
    wait_for(|| fake.ops().len() >= 10).await;
    assert_eq!(
        fake.ops(),
        vec![
            Op::Move { dx: 3, dy: -2 },
            Op::Move { dx: 1, dy: 0 },
            Op::Button {
                button: MouseButton::Left,
                down: true
            },
            Op::Wheel { delta: 120 },
            Op::Key { vk: 16, down: true },
            Op::Unicode {
                unit: 'a' as u16,
                down: true
            },
            Op::Unicode {
                unit: 'a' as u16,
                down: false
            },
            Op::Key { vk: 17, down: true },
            Op::Key { vk: 67, down: true },
            Op::Key {
                vk: 67,
                down: false
            },
            Op::Key {
                vk: 17,
                down: false
            },
        ]
    );

    // A bad frame is reported once as input.error, without closing the session.
    c.input(json!({"type":"in","k":"zz"})).await;
    c.input(json!({"type":"in","k":"mm"})).await;
    assert_eq!(c.event("input.error").await, json!({"code":"bad_params"}));
    c.no_event("input.error", 200).await;

    // Disconnect: held shift and left button are released.
    drop(c);
    wait_for(|| {
        let ops = fake.ops();
        ops.ends_with(&[
            Op::Key {
                vk: 16,
                down: false,
            },
            Op::Button {
                button: MouseButton::Left,
                down: false,
            },
        ])
    })
    .await;

    // Elevation block: reported once, then again only after input worked in between.
    let mut c = second_session(&server, &phone).await;
    fake.set_block_input(true);
    c.input(json!({"type":"in","k":"kd","vk":65})).await;
    c.input(json!({"type":"in","k":"ku","vk":65})).await;
    assert_eq!(
        c.event("input.error").await,
        json!({"code":"blocked_by_elevation"})
    );
    c.no_event("input.error", 300).await;
    server.shutdown().await;
}

#[tokio::test]
async fn upload_round_trip_hash_check_and_traversal() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _fake) = start_fake_with(dir.path(), |_| {}).await;
    let (mut c, _) = paired(&server).await;
    let downloads = dir.path().join("downloads");

    let data: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    let sha = hex::encode(Sha256::digest(&data));
    let upload =
        |name: &str, size: usize, sha: &str| json!({"name": name, "size": size, "sha256": sha});
    let r = c
        .ok("file.upload.begin", upload("photo.jpg", data.len(), &sha))
        .await;
    let id = r["transfer_id"].as_str().unwrap().to_string();
    assert_eq!(id.len(), 32);
    c.binary(frame(&id, 0, &data[..262_144])).await;
    c.binary(frame(&id, 262_144, &data[262_144..])).await;
    let r = c.ok("file.upload.end", json!({"transfer_id": id})).await;
    let path = std::path::PathBuf::from(r["path"].as_str().unwrap());
    assert_eq!(path, downloads.join("photo.jpg"));
    assert_eq!(std::fs::read(&path).unwrap(), data);

    // Same name again gets a numbered name.
    let r = c
        .ok(
            "file.upload.begin",
            upload("photo.jpg", 3, &hex::encode(Sha256::digest(b"abc"))),
        )
        .await;
    let id = r["transfer_id"].as_str().unwrap().to_string();
    c.binary(frame(&id, 0, b"abc")).await;
    let r = c.ok("file.upload.end", json!({"transfer_id": id})).await;
    assert_eq!(
        r["path"].as_str().unwrap(),
        downloads.join("photo (1).jpg").to_str().unwrap()
    );

    // Hash mismatch: rejected and nothing left behind.
    let r = c.ok("file.upload.begin", upload("bad.bin", 3, &sha)).await;
    let id = r["transfer_id"].as_str().unwrap().to_string();
    c.binary(frame(&id, 0, b"xyz")).await;
    assert_eq!(
        c.err("file.upload.end", json!({"transfer_id": id})).await,
        "bad_params"
    );
    assert!(!downloads.join("bad.bin").exists());

    // Out-of-order offset and short data fail at end.
    let r = c.ok("file.upload.begin", upload("gap.bin", 6, &sha)).await;
    let id = r["transfer_id"].as_str().unwrap().to_string();
    c.binary(frame(&id, 3, b"abc")).await;
    assert_eq!(
        c.err("file.upload.end", json!({"transfer_id": id})).await,
        "bad_params"
    );
    assert_eq!(
        c.err("file.upload.end", json!({"transfer_id": id})).await,
        "bad_params"
    );

    // Unsafe names, size limit, bad hash text.
    for bad in [
        "../evil.txt",
        "..\\evil.txt",
        "C:\\Windows\\evil.dll",
        "a/b",
        "..",
        "",
        "x\u{1}",
    ] {
        assert_eq!(
            c.err("file.upload.begin", upload(bad, 1, &sha)).await,
            "bad_params",
            "{bad:?}"
        );
    }
    let too_big = 4u64 * 1024 * 1024 * 1024 + 1;
    assert_eq!(
        c.err(
            "file.upload.begin",
            json!({"name":"big","size":too_big,"sha256":sha})
        )
        .await,
        "bad_params"
    );
    assert_eq!(
        c.err("file.upload.begin", upload("x", 1, "abc")).await,
        "bad_params"
    );
    // Exactly 4 GiB is allowed to start.
    let r = c
        .ok(
            "file.upload.begin",
            json!({"name":"max.bin","size":4u64<<30,"sha256":sha}),
        )
        .await;
    assert!(r["transfer_id"].is_string());

    let names: Vec<String> = std::fs::read_dir(&downloads)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        names
            .iter()
            .all(|n| n == "photo.jpg" || n == "photo (1).jpg" || n.ends_with(".part"))
    );
    assert!(!dir.path().join("evil.txt").exists());

    // Disconnect drops the unfinished upload's temp file.
    drop(c);
    wait_for(|| {
        std::fs::read_dir(&downloads)
            .unwrap()
            .all(|e| !e.unwrap().file_name().to_string_lossy().ends_with(".part"))
    })
    .await;
    server.shutdown().await;
}

#[tokio::test]
async fn download_streams_frames_then_done() {
    let dir = tempfile::tempdir().unwrap();
    let profile_dir = tempfile::tempdir().unwrap();
    let profile = profile_dir.path().to_path_buf();
    let root = profile.clone();
    let (server, _fake) = start_fake_with(dir.path(), move |c| c.download_root = Some(root)).await;
    let (mut c, _) = paired(&server).await;
    let data: Vec<u8> = (0..600_000u32).map(|i| (i * 7 % 256) as u8).collect();
    let file = profile.join("report.pdf");
    std::fs::write(&file, &data).unwrap();

    let r = c
        .ok("file.download", json!({"path": file.to_str().unwrap()}))
        .await;
    assert_eq!(r["size"], data.len());
    assert_eq!(r["name"], "report.pdf");
    assert_eq!(r["sha256"], hex::encode(Sha256::digest(&data)));
    let id = r["transfer_id"].as_str().unwrap().to_string();
    assert_eq!(c.event("file.done").await, json!({"transfer_id": id}));
    let mut got = Vec::new();
    for f in &c.binary {
        assert_eq!(hex::encode(&f[..16]), id);
        let offset = u64::from_be_bytes(f[16..24].try_into().unwrap());
        assert_eq!(offset as usize, got.len());
        assert!(f.len() - 24 <= 256 * 1024);
        got.extend_from_slice(&f[24..]);
    }
    assert_eq!(
        hex::encode(Sha256::digest(&got)),
        r["sha256"].as_str().unwrap()
    );
    assert_eq!(got, data);

    // Empty file: no frames, just done.
    c.binary.clear();
    std::fs::write(profile.join("empty.txt"), b"").unwrap();
    let r = c
        .ok(
            "file.download",
            json!({"path": profile.join("empty.txt").to_str().unwrap()}),
        )
        .await;
    c.event("file.done").await;
    assert!(c.binary.is_empty());
    assert_eq!(r["sha256"], hex::encode(Sha256::digest(b"")));

    // Outside the profile, traversal, folders, missing files.
    let outside = dir.path().join("devices.json");
    assert_eq!(
        c.err("file.download", json!({"path": outside.to_str().unwrap()}))
            .await,
        "forbidden"
    );
    let trav = format!("{}\\..\\devices.json", profile.display());
    assert_eq!(
        c.err("file.download", json!({"path": trav})).await,
        "bad_params"
    );
    assert_eq!(
        c.err("file.download", json!({"path": profile.to_str().unwrap()}))
            .await,
        "bad_params"
    );
    assert_eq!(
        c.err("file.download", json!({"path":"C:\\nope\\nope.txt"}))
            .await,
        "bad_params"
    );
    assert_eq!(
        c.err("file.download", json!({"path":"report.pdf"})).await,
        "bad_params"
    );
    let audit = std::fs::read_to_string(dir.path().join("audit.log")).unwrap();
    assert!(audit.contains("\tfile_download\t"));
    server.shutdown().await;
}

#[tokio::test]
async fn tiers_are_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |_| {}).await;
    let (_admin, phone) = paired(&server).await;
    let file = dir.path().join("profile").join("f.txt");
    std::fs::write(&file, b"x").unwrap();
    let sha = hex::encode(Sha256::digest(b"x"));

    let mut v = with_tier(&server, dir.path(), &phone, "viewer").await;
    for m in [
        "apps.list",
        "windows.list",
        "dock.get",
        "deck.get",
        "media.state",
        "audio.get",
        "display.brightness.list",
        "power.status",
        "screen.monitors",
        "system.info",
    ] {
        v.ok(m, json!({})).await;
    }
    for (m, p) in [
        ("apps.launch", json!({"app_id":"Fake.Notes"})),
        ("open", json!({"target":"https://example.com"})),
        ("windows.close", json!({"hwnd":100})),
        ("dock.set", json!({"pages":[]})),
        ("deck.run", json!({"id":"x"})),
        ("media.control", json!({"action":"play"})),
        ("audio.set", json!({"level":0.1})),
        ("display.brightness.set", json!({"id":"wmi:0","level":1})),
        ("clipboard.get", json!({})),
        ("clipboard.watch", json!({"on":true})),
        ("screen.capture", json!({})),
        (
            "file.upload.begin",
            json!({"name":"a","size":1,"sha256":sha}),
        ),
        ("power.action", json!({"action":"lock"})),
        ("power.cancel", json!({})),
        ("file.download", json!({"path": file.to_str().unwrap()})),
    ] {
        assert_eq!(v.err(m, p).await, "forbidden", "{m}");
    }
    v.input(json!({"type":"in","k":"kd","vk":65})).await;
    assert_eq!(v.event("input.error").await, json!({"code":"forbidden"}));
    drop(v);

    let mut r = with_tier(&server, dir.path(), &phone, "remote").await;
    r.ok("apps.launch", json!({"app_id":"Fake.Notes"})).await;
    r.ok("clipboard.get", json!({})).await;
    r.ok(
        "file.upload.begin",
        json!({"name":"a","size":1,"sha256":sha}),
    )
    .await;
    assert_eq!(
        r.err("power.action", json!({"action":"lock"})).await,
        "forbidden"
    );
    assert_eq!(r.err("power.cancel", json!({})).await, "forbidden");
    assert_eq!(
        r.err("file.download", json!({"path": file.to_str().unwrap()}))
            .await,
        "forbidden"
    );
    r.input(json!({"type":"in","k":"kd","vk":65})).await;
    wait_for(|| fake.ops().contains(&Op::Key { vk: 65, down: true })).await;
    assert!(
        server
            .state()
            .power_dry_run
            .as_ref()
            .unwrap()
            .executed()
            .is_empty()
    );
    assert_eq!(fake.calls(), vec!["launch_app:Fake.Notes"]);
    server.shutdown().await;
}
