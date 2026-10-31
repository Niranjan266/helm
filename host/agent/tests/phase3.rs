//! Phase 3 end-to-end tests (PROTOCOL §11) against a `FakePlatform` and a fake OBS server:
//! nothing here touches the real PC (no kills, priorities, volume, windows, desktops, OBS).

mod common;

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use common::*;
use futures_util::{SinkExt, StreamExt};
use helm_agent::platform::fake::sample_stats;
use helm_win_input::Op;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

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

    /// Next event `name` whose data satisfies `pred`.
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

    /// Drop queued messages and assert no `name` event arrives within `ms`.
    async fn no_event(&mut self, name: &str, ms: u64) {
        self.backlog.clear();
        let deadline = tokio::time::Instant::now() + Duration::from_millis(ms);
        while let Some(m) = self
            .read(deadline.saturating_duration_since(tokio::time::Instant::now()))
            .await
        {
            assert!(
                !(m["type"] == "event" && m["name"] == name),
                "unexpected {name}: {m}"
            );
        }
    }
}

async fn paired(server: &helm_agent::ServerHandle) -> (Client, Phone) {
    let phone = Phone::new("Pixel");
    let token = server.new_pairing().token;
    let (ws, ready) = phone.pair(server, &token).await;
    assert_eq!(ready["type"], "ready", "{ready}");
    (client(ws), phone)
}

fn client(ws: Ws) -> Client {
    Client {
        ws,
        backlog: VecDeque::new(),
        next_id: 1,
    }
}

/// Change a paired device's tier in devices.json, then connect with it.
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

const P3_METHODS: &[&str] = &[
    "stats.subscribe",
    "stats.unsubscribe",
    "alerts.get",
    "alerts.set",
    "process.list",
    "process.kill",
    "process.priority",
    "audio.sessions",
    "audio.session.set",
    "windows.move",
    "windows.snap",
    "desktop.switch",
    "profiles.get",
    "profiles.set",
    "fs.list",
    "fs.delete",
    "fs.mkdir",
    "fs.open",
    "devices.set_tier",
    "audit.list",
];

#[tokio::test]
async fn hello_advertises_p3_and_methods_exist() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _fake) = start_fake_with(dir.path(), |_| {}).await;
    let mut ws = connect(&server).await;
    let hello = recv(&mut ws).await;
    let features = hello["features"].as_array().unwrap();
    assert!(features.contains(&json!("p1")) && features.contains(&json!("p3")));
    let names = server.state().router().method_names();
    for m in P3_METHODS {
        assert!(names.contains(m), "missing {m}");
    }
    server.shutdown().await;
}

#[tokio::test]
async fn stats_subscribe_ticks_and_unsubscribe_stops() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |_| {}).await;
    fake.with(|s| s.stats = sample_stats(42.0, 50));
    let (mut c, _) = paired(&server).await;
    assert_eq!(
        c.err("stats.subscribe", json!({"interval_ms": 100})).await,
        "bad_params"
    );
    assert_eq!(c.err("stats.subscribe", json!({})).await, "bad_params");
    c.ok("stats.subscribe", json!({"interval_ms": 500})).await;
    let tick = c.event("stats.tick").await;
    assert_eq!(tick["cpu_total"], 42.0);
    assert_eq!(tick["ram_total"], 100_000_000);
    assert_eq!(tick["disks"][0]["mount"], "C:\\");
    c.ok("stats.unsubscribe", json!({})).await;
    c.no_event("stats.tick", 1200).await;
    server.shutdown().await;
}

#[tokio::test]
async fn alerts_persist_fire_and_clear() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |_| {}).await;
    server.state().p3.set_tick(Duration::from_millis(50));
    let (mut c, phone) = paired(&server).await;
    assert_eq!(c.ok("alerts.get", json!({})).await, json!({"rules": []}));
    let rules = json!([
        {"id":"cpu","kind":"cpu_above","threshold":80.0,"for_s":0},
        {"id":"disk","kind":"disk_free_below","threshold":5.0,"mount":"C:\\"}
    ]);
    c.ok("alerts.set", json!({"rules": rules})).await;
    assert_eq!(c.ok("alerts.get", json!({})).await["rules"], rules);
    assert!(dir.path().join("alerts.json").exists());
    let bad = json!([{"id":"x","kind":"cpu_above","threshold":500}]);
    assert_eq!(
        c.err("alerts.set", json!({"rules": bad})).await,
        "bad_params"
    );
    let bad = json!([{"id":"x","kind":"fan_speed","threshold":5}]);
    assert_eq!(
        c.err("alerts.set", json!({"rules": bad})).await,
        "bad_params"
    );

    fake.with(|s| s.stats = sample_stats(95.0, 50));
    let fired = c.event_where("alert", |d| d["id"] == "cpu").await;
    assert_eq!(fired["firing"], true);
    assert_eq!(fired["value"], 95.0);
    fake.with(|s| s.stats = sample_stats(10.0, 50));
    let cleared = c.event_where("alert", |d| d["id"] == "cpu").await;
    assert_eq!(cleared["firing"], false);

    let mut viewer = with_tier(&server, dir.path(), &phone, "viewer").await;
    assert_eq!(viewer.ok("alerts.get", json!({})).await["rules"], rules);
    assert_eq!(
        viewer.err("alerts.set", json!({"rules": []})).await,
        "forbidden"
    );
    server.shutdown().await;
}

#[tokio::test]
async fn processes_list_kill_priority() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |_| {}).await;
    let (mut c, phone) = paired(&server).await;
    let list = c.ok("process.list", json!({})).await;
    assert_eq!(list["processes"][0]["pid"], 1000);
    assert_eq!(list["processes"][0]["name"], "notes.exe");
    assert_eq!(list["processes"][0]["ram_bytes"], 10_000_000);

    c.ok(
        "process.priority",
        json!({"pid":2000,"level":"below_normal"}),
    )
    .await;
    assert_eq!(
        c.err("process.priority", json!({"pid":2000,"level":"realtime"}))
            .await,
        "bad_params"
    );
    assert_eq!(
        c.err("process.priority", json!({"pid":2000,"level":"turbo"}))
            .await,
        "bad_params"
    );
    c.ok("process.kill", json!({"pid":2000})).await;
    assert_eq!(
        c.err("process.kill", json!({"pid":2000})).await,
        "bad_params"
    );
    for pid in [0, 4, std::process::id()] {
        assert_eq!(
            c.err("process.kill", json!({ "pid": pid })).await,
            "forbidden"
        );
    }
    assert_eq!(
        fake.calls(),
        vec!["set_process_priority:2000:BelowNormal", "kill_process:2000"]
    );
    let audit = std::fs::read_to_string(dir.path().join("audit.log")).unwrap();
    assert!(audit.contains("\tprocess_kill\t") && audit.contains("pid=2000"));

    let mut remote = with_tier(&server, dir.path(), &phone, "remote").await;
    assert!(remote.ok("process.list", json!({})).await["processes"].is_array());
    assert_eq!(
        remote.err("process.kill", json!({"pid":1000})).await,
        "forbidden"
    );
    server.shutdown().await;
}

#[tokio::test]
async fn audio_sessions_windows_and_desktops() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |_| {}).await;
    let (mut c, phone) = paired(&server).await;
    let s = c.ok("audio.sessions", json!({})).await;
    assert_eq!(s["sessions"][0]["id"], "sess-1");
    assert_eq!(s["sessions"][0]["name"], "Music");
    c.ok(
        "audio.session.set",
        json!({"id":"sess-1","level":0.25,"muted":true}),
    )
    .await;
    assert_eq!(
        c.err("audio.session.set", json!({"id":"sess-1"})).await,
        "bad_params"
    );
    assert_eq!(
        c.err("audio.session.set", json!({"id":"sess-1","level":2}))
            .await,
        "bad_params"
    );
    assert_eq!(
        c.err("audio.session.set", json!({"id":"nope","muted":true}))
            .await,
        "bad_params"
    );
    let s = c.ok("audio.sessions", json!({})).await;
    assert_eq!(s["sessions"][0]["muted"], true);

    c.ok("windows.move", json!({"hwnd":100,"monitor_index":0}))
        .await;
    assert_eq!(
        c.err("windows.move", json!({"hwnd":100,"monitor_index":5}))
            .await,
        "bad_params"
    );
    c.ok("windows.snap", json!({"hwnd":200,"pos":"left"})).await;
    assert_eq!(
        c.err("windows.snap", json!({"hwnd":200,"pos":"diagonal"}))
            .await,
        "bad_params"
    );
    assert_eq!(
        c.err("windows.snap", json!({"hwnd":999,"pos":"left"}))
            .await,
        "bad_params"
    );
    for dir_name in ["next", "prev", "new", "close"] {
        c.ok("desktop.switch", json!({ "dir": dir_name })).await;
    }
    assert_eq!(
        c.err("desktop.switch", json!({"dir":"up"})).await,
        "bad_params"
    );
    assert_eq!(
        fake.calls(),
        vec![
            "set_session_volume:sess-1:0.25",
            "set_session_muted:sess-1:true",
            "move_window:100:0",
            "snap_window:200:Left",
            "virtual_desktop:Next",
            "virtual_desktop:Prev",
            "virtual_desktop:New",
            "virtual_desktop:Close",
        ]
    );
    let mut viewer = with_tier(&server, dir.path(), &phone, "viewer").await;
    assert!(viewer.ok("audio.sessions", json!({})).await["sessions"].is_array());
    assert_eq!(
        viewer
            .err("windows.snap", json!({"hwnd":200,"pos":"left"}))
            .await,
        "forbidden"
    );
    server.shutdown().await;
}

#[tokio::test]
async fn profiles_follow_the_foreground_app() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |_| {}).await;
    let (mut c, _) = paired(&server).await;
    assert_eq!(
        c.ok("profiles.get", json!({})).await,
        json!({"profiles": [], "active": null})
    );
    let profiles = json!([
        {"id":"code","name":"Coding","match_exe":"Code.exe","deck_id":"dev","dock_page":1},
        {"id":"game","name":"Gaming","match_exe":"game.exe"}
    ]);
    c.ok("profiles.set", json!({"profiles": profiles})).await;
    assert_eq!(c.ok("profiles.get", json!({})).await["profiles"], profiles);
    let dup = json!([{"id":"a","name":"A"},{"id":"a","name":"B"}]);
    assert_eq!(
        c.err("profiles.set", json!({"profiles": dup})).await,
        "bad_params"
    );

    wait_until(|| fake.emit_foreground("CODE.EXE")).await;
    assert_eq!(c.event("profile.active").await, json!({"id":"code"}));
    assert!(fake.emit_foreground("notepad.exe"));
    assert_eq!(c.event("profile.active").await, json!({"id": null}));
    assert!(fake.emit_foreground("game.exe"));
    assert_eq!(c.event("profile.active").await, json!({"id":"game"}));
    assert_eq!(c.ok("profiles.get", json!({})).await["active"], "game");
    // Removing the matching profile re-evaluates the current app.
    c.ok("profiles.set", json!({"profiles": []})).await;
    assert_eq!(c.event("profile.active").await, json!({"id": null}));
    server.shutdown().await;
}

async fn wait_until(mut cond: impl FnMut() -> bool) {
    for _ in 0..100 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    panic!("condition not met in time");
}

#[tokio::test]
async fn fs_browses_roots_and_blocks_data_dir() {
    let top = tempfile::tempdir().unwrap();
    let data = top.path().join("data");
    let profile = top.path().join("profile");
    let drive = top.path().join("drive");
    std::fs::create_dir_all(&profile).unwrap();
    std::fs::create_dir_all(drive.join("Games")).unwrap();
    std::fs::write(profile.join("notes.txt"), "hello").unwrap();
    let outside = top.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let (server, fake) = start_fake_with(&data, |c| {
        c.download_root = Some(profile.clone());
    })
    .await;
    // The data folder sits on a "fixed drive" too: it must stay hidden and refused.
    let drive_with_data = top.path().to_path_buf();
    fake.with(|s| s.fixed_drives = vec![drive.clone(), drive_with_data.clone()]);
    let (mut c, phone) = paired(&server).await;
    let p = |x: &Path| x.to_string_lossy().into_owned();

    let roots = c.ok("fs.list", json!({"path": ""})).await;
    assert_eq!(roots["entries"].as_array().unwrap().len(), 3);

    let l = c.ok("fs.list", json!({"path": p(&profile)})).await;
    assert_eq!(l["entries"][0]["name"], "notes.txt");
    assert_eq!(l["entries"][0]["is_dir"], false);
    assert_eq!(l["entries"][0]["size"], 5);
    assert!(l["entries"][0]["modified_ms"].as_u64().unwrap() > 0);

    let top_list = c.ok("fs.list", json!({"path": p(top.path())})).await;
    let names: Vec<&str> = top_list["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"profile") && !names.contains(&"data"),
        "{names:?}"
    );
    assert_eq!(
        c.err("fs.list", json!({"path": p(&data)})).await,
        "forbidden"
    );
    assert_eq!(
        c.err("fs.open", json!({"path": p(&data.join("devices.json"))}))
            .await,
        "forbidden"
    );
    assert_eq!(
        c.err("fs.delete", json!({"path": p(&data.join("devices.json"))}))
            .await,
        "forbidden"
    );
    assert_eq!(
        c.err("fs.list", json!({"path": "relative\\x"})).await,
        "bad_params"
    );
    assert_eq!(c.err("fs.list", json!({})).await, "bad_params");

    fake.with(|s| s.fixed_drives = vec![drive.clone()]);
    assert_eq!(
        c.err("fs.list", json!({"path": p(&outside)})).await,
        "forbidden"
    );

    let newdir = drive.join("Saves");
    c.ok("fs.mkdir", json!({"path": p(&newdir)})).await;
    assert!(newdir.is_dir());
    assert_eq!(
        c.err("fs.mkdir", json!({"path": p(&newdir)})).await,
        "bad_params"
    );
    c.ok("fs.open", json!({"path": p(&profile.join("notes.txt"))}))
        .await;
    c.ok("fs.delete", json!({"path": p(&profile.join("notes.txt"))}))
        .await;
    assert!(!profile.join("notes.txt").exists());
    c.ok("fs.delete", json!({"path": p(&newdir)})).await;
    assert!(!newdir.exists());
    assert_eq!(
        c.err("fs.delete", json!({"path": p(&drive)})).await,
        "forbidden"
    );
    let opened: Vec<String> = fake
        .calls()
        .into_iter()
        .filter(|c| c.starts_with("open_target:"))
        .collect();
    assert_eq!(opened.len(), 1);
    assert!(opened[0].ends_with("notes.txt") && !opened[0].contains(r"\\?\"));
    let audit = std::fs::read_to_string(data.join("audit.log")).unwrap();
    assert!(audit.contains("\tfs_delete\t") && audit.contains("\tfs_mkdir\t"));

    let mut remote = with_tier(&server, &data, &phone, "remote").await;
    assert_eq!(
        remote.err("fs.list", json!({"path": ""})).await,
        "forbidden"
    );
    server.shutdown().await;
}

#[tokio::test]
async fn set_tier_and_audit_list() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _fake) = start_fake_with(dir.path(), |_| {}).await;
    let (mut admin, _) = paired(&server).await;
    let (mut other, other_phone) = paired(&server).await;
    other.ok("system.ping", json!({})).await;

    assert_eq!(
        admin
            .err(
                "devices.set_tier",
                json!({"device_id": other_phone.device_id, "tier":"root"})
            )
            .await,
        "bad_params"
    );
    assert_eq!(
        admin
            .err(
                "devices.set_tier",
                json!({"device_id":"missing","tier":"viewer"})
            )
            .await,
        "bad_params"
    );
    admin
        .ok(
            "devices.set_tier",
            json!({"device_id": other_phone.device_id, "tier":"viewer"}),
        )
        .await;
    // The changed device is disconnected and comes back with its new tier.
    expect_closed(&mut other.ws).await;
    let (ws, ready) = other_phone.auth(&server).await;
    assert_eq!(ready["tier"], "viewer");
    let mut other = client(ws);
    assert_eq!(other.err("audit.list", json!({})).await, "forbidden");
    let list = admin.ok("devices.list", json!({})).await;
    let me = list["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["this"] == true)
        .unwrap()
        .clone();
    assert_eq!(
        admin
            .err(
                "devices.set_tier",
                json!({"device_id": me["device_id"], "tier":"viewer"})
            )
            .await,
        "bad_params"
    );

    let lines = admin.ok("audit.list", json!({"limit": 3})).await["lines"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(lines.len(), 3);
    let all = admin.ok("audit.list", json!({"limit": 1000})).await["lines"]
        .as_array()
        .unwrap()
        .clone();
    assert!(
        all.iter()
            .any(|l| l.as_str().unwrap().contains("\tset_tier\t"))
    );
    assert!(
        all.iter()
            .any(|l| l.as_str().unwrap().contains("\tpair_ok\t"))
    );
    assert_eq!(
        admin.err("audit.list", json!({"limit": "x"})).await,
        "bad_params"
    );
    server.shutdown().await;
}

#[tokio::test]
async fn deck_macros_repeat_remote_state_and_widgets() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |_| {}).await;
    server.state().p3.set_tick(Duration::from_millis(50));
    fake.with(|s| s.stats = sample_stats(37.0, 25));
    let (mut c, _) = paired(&server).await;
    let buttons = json!([
        {"id":"mac","label":"Macro","color":"#000000",
         "action":{"macro":[{"text":"a"},{"delay_ms":10},{"text":"b"}]}},
        {"id":"rep","label":"Vol","color":"#000000",
         "action":{"repeat":{"times":3,"action":{"volume_step":0.1}}}},
        {"id":"ppt","label":"Next","color":"#000000","action":{"remote":"powerpoint.next"}},
        {"id":"mic","label":"Mic","color":"#000000","action":{"hotkey":"ctrl+shift+m"},
         "state":{"bind":"mic_muted"}},
        {"id":"mute","label":"Mute","color":"#000000","action":{"text":"m"},
         "state":{"bind":"audio_muted"}},
        {"id":"cpu","label":"CPU","color":"#000000","action":{"delay_ms":0},"widget":"cpu"},
        {"id":"clk","label":"Clock","color":"#000000","action":{"delay_ms":0},"widget":"clock"},
        {"id":"np","label":"Now","color":"#000000","action":{"delay_ms":0},"widget":"now_playing"}
    ]);
    c.ok("deck.set", json!({"buttons": buttons})).await;
    assert_eq!(c.ok("deck.get", json!({})).await["buttons"], buttons);
    let too_long = json!([{"id":"x","label":"","color":"",
        "action":{"repeat":{"times":51,"action":{"text":"a"}}}}]);
    assert_eq!(
        c.err("deck.set", json!({"buttons": too_long})).await,
        "bad_params"
    );

    c.ok("deck.run", json!({"id":"mac"})).await;
    c.ok("deck.run", json!({"id":"rep"})).await;
    let before = fake.ops().len();
    c.ok("deck.run", json!({"id":"ppt"})).await;
    // VK_RIGHT down + up.
    assert_eq!(
        &fake.ops()[before..],
        &[
            Op::Key {
                vk: 0x27,
                down: true
            },
            Op::Key {
                vk: 0x27,
                down: false
            }
        ]
    );
    let calls = fake.calls();
    assert_eq!(
        calls,
        vec!["set_volume:0.60", "set_volume:0.70", "set_volume:0.80"]
    );

    assert_eq!(
        c.event_where("deck.state", |d| d["id"] == "mic").await,
        json!({"id":"mic","on":false})
    );
    assert_eq!(
        c.event_where("deck.widget", |d| d["id"] == "cpu").await,
        json!({"id":"cpu","text":"CPU 37%"})
    );
    assert_eq!(
        c.event_where("deck.widget", |d| d["id"] == "clk").await,
        json!({"id":"clk","text":"12:34"})
    );
    assert_eq!(
        c.event_where("deck.widget", |d| d["id"] == "np").await["text"],
        "Fake Song \u{2014} Fake Artist"
    );
    fake.with(|s| s.mic_muted = Some(true));
    assert_eq!(
        c.event_where("deck.state", |d| d["id"] == "mic").await,
        json!({"id":"mic","on":true})
    );
    fake.with(|s| s.volume.muted = true);
    assert_eq!(
        c.event_where("deck.state", |d| d["id"] == "mute" && d["on"] == true)
            .await,
        json!({"id":"mute","on":true})
    );
    server.shutdown().await;
}

/// A fake obs-websocket v5 server: checks the auth string, records each request.
async fn fake_obs(password: &'static str) -> (u16, Arc<Mutex<Vec<Value>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let log = log.clone();
            tokio::spawn(async move {
                let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
                let (challenge, salt) = ("Y2hhbGxlbmdl", "c2FsdA==");
                let hello = json!({"op":0,"d":{"obsWebSocketVersion":"5.0.0","rpcVersion":1,
                    "authentication":{"challenge":challenge,"salt":salt}}});
                ws.send(Message::text(hello.to_string())).await.unwrap();
                let Some(Ok(Message::Text(t))) = ws.next().await else {
                    return;
                };
                let identify: Value = serde_json::from_str(t.as_str()).unwrap();
                let secret =
                    STANDARD.encode(Sha256::digest(format!("{password}{salt}").as_bytes()));
                let expected =
                    STANDARD.encode(Sha256::digest(format!("{secret}{challenge}").as_bytes()));
                if identify["op"] != 1 || identify["d"]["authentication"] != expected.as_str() {
                    let frame = CloseFrame {
                        code: CloseCode::from(4009),
                        reason: "Authentication failed.".into(),
                    };
                    let _ = ws.send(Message::Close(Some(frame))).await;
                    return;
                }
                ws.send(Message::text(
                    json!({"op":2,"d":{"negotiatedRpcVersion":1}}).to_string(),
                ))
                .await
                .unwrap();
                while let Some(Ok(Message::Text(t))) = ws.next().await {
                    let req: Value = serde_json::from_str(t.as_str()).unwrap();
                    log.lock().unwrap().push(req["d"].clone());
                    let ok = req["d"]["requestType"] != "SetCurrentProgramScene"
                        || req["d"]["requestData"]["sceneName"] != "Missing";
                    let res = json!({"op":7,"d":{"requestType":req["d"]["requestType"],
                        "requestId":req["d"]["requestId"],
                        "requestStatus":{"result":ok,"code":if ok {100} else {600},
                            "comment": if ok { Value::Null } else { json!("No source was found") }}}});
                    ws.send(Message::text(res.to_string())).await.unwrap();
                }
            });
        }
    });
    (port, seen)
}

#[tokio::test]
async fn deck_obs_actions_use_obs_websocket_v5() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _fake) = start_fake_with(dir.path(), |_| {}).await;
    let (mut c, _) = paired(&server).await;
    let buttons = json!([
        {"id":"rec","label":"Rec","color":"#000000","action":{"remote":"obs.toggle_record"}},
        {"id":"live","label":"Live","color":"#000000","action":{"remote":"obs.toggle_stream"}},
        {"id":"scene","label":"Game","color":"#000000","action":{"remote":"obs.scene:Game"}},
        {"id":"gone","label":"Gone","color":"#000000","action":{"remote":"obs.scene:Missing"}}
    ]);
    c.ok("deck.set", json!({"buttons": buttons})).await;
    // Not set up: refused without contacting anything.
    assert_eq!(c.err("deck.run", json!({"id":"rec"})).await, "bad_params");

    let (port, seen) = fake_obs("s3cret").await;
    std::fs::write(
        dir.path().join("obs.json"),
        json!({"password":"s3cret","port":port}).to_string(),
    )
    .unwrap();
    c.ok("deck.run", json!({"id":"rec"})).await;
    c.ok("deck.run", json!({"id":"live"})).await;
    c.ok("deck.run", json!({"id":"scene"})).await;
    assert_eq!(c.err("deck.run", json!({"id":"gone"})).await, "bad_params");
    {
        let seen = seen.lock().unwrap();
        let types: Vec<&str> = seen
            .iter()
            .map(|r| r["requestType"].as_str().unwrap())
            .collect();
        assert_eq!(
            types,
            [
                "ToggleRecord",
                "ToggleStream",
                "SetCurrentProgramScene",
                "SetCurrentProgramScene"
            ]
        );
        assert_eq!(seen[2]["requestData"], json!({"sceneName":"Game"}));
    }

    std::fs::write(
        dir.path().join("obs.json"),
        json!({"password":"wrong","port":port}).to_string(),
    )
    .unwrap();
    let r = c.call("deck.run", json!({"id":"rec"})).await;
    assert_eq!(r["error"]["code"], "bad_params", "{r}");
    assert!(r["error"]["message"].as_str().unwrap().contains("password"));
    assert_eq!(seen.lock().unwrap().len(), 4);
    server.shutdown().await;
}
