//! g6 end-to-end tests (Docs/protocol/everyday.md) against a `FakePlatform`: no real
//! clipboard, Explorer, notifications, power action or sound — everything goes through fakes.

mod common;

use std::collections::VecDeque;
use std::path::Path;
use std::time::Duration;

use common::*;
use futures_util::StreamExt;
use helm_agent::platform::{NotifyAccess, PcNotification, PlatformEvent};
use helm_win_system::power::PowerAction;
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

    /// True if `name` arrives within `wait`.
    async fn sees(&mut self, name: &str, wait: Duration) -> bool {
        let is = |m: &Value| m["type"] == "event" && m["name"] == name;
        if self.backlog.iter().any(is) {
            return true;
        }
        let deadline = tokio::time::Instant::now() + wait;
        while let Some(left) = deadline.checked_duration_since(tokio::time::Instant::now()) {
            match self.read(left).await {
                Some(m) if is(&m) => return true,
                Some(m) => self.backlog.push_back(m),
                None => return false,
            }
        }
        false
    }
}

fn client(ws: Ws) -> Client {
    Client {
        ws,
        backlog: VecDeque::new(),
        next_id: 1,
    }
}

async fn paired_as(server: &helm_agent::ServerHandle, name: &str) -> (Client, Phone) {
    let phone = Phone::new(name);
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

const QUIET: Duration = Duration::from_millis(400);

#[tokio::test]
async fn hello_advertises_g6_and_tiers_are_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _fake) = start_fake_with(dir.path(), |_| {}).await;
    let mut ws = try_connect_path(server.port(), server.fingerprint(), "/ws")
        .await
        .unwrap();
    let hello = recv(&mut ws).await;
    assert!(hello["features"].as_array().unwrap().contains(&json!("g6")));
    drop(ws);
    let names = server.state().router().method_names();
    for m in [
        "clipboard.history", "clipboard.history.set", "clipboard.history.clear",
        "clipboard.history.delete", "clipboard.pick", "file.drop", "phone.list", "phone.ring",
        "pc.notify.watch", "schedule.list", "schedule.set", "system.health",
    ] {
        assert!(names.contains(&m), "{m} missing");
    }
    let (_admin, phone) = paired_as(&server, "Pixel").await;
    let mut viewer = with_tier(&server, dir.path(), &phone, "viewer").await;
    for m in ["clipboard.history", "clipboard.pick", "file.drop", "phone.ring", "pc.notify.watch", "schedule.set"] {
        assert_eq!(viewer.err(m, json!({})).await, "forbidden", "{m}");
    }
    viewer.ok("schedule.list", json!({})).await;
    viewer.ok("phone.list", json!({})).await;
    viewer.ok("system.health", json!({})).await;
    let mut remote = with_tier(&server, dir.path(), &phone, "remote").await;
    let power = json!({"schedules":[{"id":"off","enabled":true,"time":"23:00","days":[1],
                                      "action":{"power":"shutdown"}}]});
    assert_eq!(remote.err("schedule.set", power).await, "forbidden");
    remote
        .ok("schedule.set", json!({"schedules":[{"id":"m","enabled":true,"time":"23:00","days":[1],
                                                  "action":{"volume_step":0.1}}]}))
        .await;
    server.shutdown().await;
}

#[tokio::test]
async fn clipboard_history_is_opt_in_skips_password_managers_and_picks() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |_| {}).await;
    let (mut c, _) = paired_as(&server, "Pixel").await;
    let h = c.ok("clipboard.history", json!({})).await;
    assert_eq!(h["enabled"], false);
    assert_eq!(h["items"], json!([]));

    // Off: changes are not recorded.
    fake.with(|s| s.clipboard_text = Some("before".into()));
    assert!(fake.emit(PlatformEvent::Clipboard { has_text: true, has_image: false }));
    assert!(!c.sees("clipboard.history.changed", QUIET).await);

    let h = c.ok("clipboard.history.set", json!({"enabled": true, "persist": true})).await;
    assert_eq!((h["enabled"].clone(), h["persist"].clone()), (json!(true), json!(true)));

    fake.with(|s| s.clipboard_text = Some("hello world".into()));
    fake.emit(PlatformEvent::Clipboard { has_text: true, has_image: false });
    c.event("clipboard.history.changed").await;

    // A password manager's copy is skipped.
    fake.with(|s| {
        s.clipboard_text = Some("hunter2".into());
        s.clipboard_excluded = true;
    });
    fake.emit(PlatformEvent::Clipboard { has_text: true, has_image: false });
    assert!(!c.sees("clipboard.history.changed", QUIET).await);

    fake.with(|s| {
        s.clipboard_excluded = false;
        s.clipboard_text = None;
        s.clipboard_png = Some(helm_agent::platform::fake::TINY_PNG.to_vec());
    });
    fake.emit(PlatformEvent::Clipboard { has_text: false, has_image: true });
    c.event("clipboard.history.changed").await;

    let h = c.ok("clipboard.history", json!({})).await;
    let items = h["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{h}");
    assert_eq!(items[0]["kind"], "image");
    assert_eq!((items[0]["width"].clone(), items[0]["height"].clone()), (json!(1), json!(1)));
    assert!(items[0]["thumb_png"].is_string());
    assert_eq!(items[1]["text"], "hello world");
    assert!(!h.to_string().contains("hunter2"));
    assert!(dir.path().join("clipboard-history.bin").exists());

    let text_id = items[1]["id"].clone();
    let img_id = items[0]["id"].clone();
    assert_eq!(c.ok("clipboard.pick", json!({"id": text_id, "to": "phone"})).await, json!({"text":"hello world"}));
    let png = c.ok("clipboard.pick", json!({"id": img_id, "to": "phone"})).await;
    assert!(png["png"].is_string());
    c.ok("clipboard.pick", json!({"id": text_id, "to": "pc"})).await;
    assert!(fake.calls().contains(&"set_clipboard_text:hello world".to_string()));
    c.event("clipboard.history.changed").await;
    assert_eq!(c.ok("clipboard.history", json!({})).await["items"][0]["id"], text_id, "moved to top");
    assert_eq!(c.err("clipboard.pick", json!({"id": 999, "to": "pc"})).await, "bad_params");
    assert_eq!(c.err("clipboard.pick", json!({"id": text_id, "to": "fax"})).await, "bad_params");

    c.ok("clipboard.history.delete", json!({"id": img_id})).await;
    assert_eq!(c.err("clipboard.history.delete", json!({"id": img_id})).await, "bad_params");
    c.ok("clipboard.history.clear", json!({})).await;
    assert_eq!(c.ok("clipboard.history", json!({})).await["items"], json!([]));
    let h = c.ok("clipboard.history.set", json!({"enabled": false})).await;
    assert_eq!(h["enabled"], false);
    assert!(!dir.path().join("clipboard-history.bin").exists());
    server.shutdown().await;
}

#[tokio::test]
async fn file_drop_opens_reveals_or_saves_only_received_files() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |_| {}).await;
    let (mut c, _) = paired_as(&server, "Pixel").await;
    let downloads = dir.path().join("downloads");
    std::fs::create_dir_all(&downloads).unwrap();
    let photo = downloads.join("photo.jpg");
    let exe = downloads.join("setup.exe");
    std::fs::write(&photo, b"jpg").unwrap();
    std::fs::write(&exe, b"MZ").unwrap();
    std::fs::write(dir.path().join("outside.txt"), b"x").unwrap();
    let p = |x: &Path| x.to_str().unwrap().to_string();

    let r = c.ok("file.drop", json!({"path": p(&photo), "mode": "open"})).await;
    assert_eq!(r["mode"], "open");
    assert!(fake.calls().contains(&format!("open_target:{}", p(&photo))));
    let r = c.ok("file.drop", json!({"path": p(&exe), "mode": "open"})).await;
    assert_eq!(r["mode"], "reveal", "programs are never opened");
    assert!(!fake.calls().iter().any(|x| x.starts_with("open_target:") && x.ends_with("setup.exe")));
    assert!(fake.calls().contains(&format!("reveal_in_explorer:{}", p(&exe))));
    let before = fake.calls().len();
    let r = c.ok("file.drop", json!({"path": p(&photo), "mode": "save"})).await;
    assert_eq!(r["mode"], "save");
    assert_eq!(fake.calls().len(), before, "save does nothing more");
    for bad in [
        json!({"path": p(&dir.path().join("outside.txt")), "mode": "open"}),
        json!({"path": p(&downloads.join("missing.txt")), "mode": "open"}),
        json!({"path": p(&downloads), "mode": "reveal"}),
        json!({"path": p(&photo), "mode": "print"}),
        json!({"path": "photo.jpg", "mode": "open"}),
    ] {
        assert_eq!(c.err("file.drop", bad.clone()).await, "bad_params", "{bad}");
    }
    server.shutdown().await;
}

#[tokio::test]
async fn find_my_phone_rings_the_other_phone_and_from_the_tray() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _fake) = start_fake_with(dir.path(), |_| {}).await;
    let (mut a, phone_a) = paired_as(&server, "Pixel").await;
    let (mut b, phone_b) = paired_as(&server, "Galaxy").await;

    let list = a.ok("phone.list", json!({})).await;
    let phones = list["phones"].as_array().unwrap();
    assert_eq!(phones.len(), 2);
    let me = phones.iter().find(|p| p["device_id"] == phone_a.device_id).unwrap();
    assert_eq!((me["this"].clone(), me["online"].clone()), (json!(true), json!(true)));

    let r = a.ok("phone.ring", json!({"device_id": phone_b.device_id})).await;
    assert_eq!(r["rang"], 1);
    let ev = b.event("phone.ring").await;
    assert_eq!(ev["stop"], false);
    assert_eq!(ev["from"]["device_id"], phone_a.device_id);
    assert_eq!(ev["from"]["name"], "Pixel");
    assert!(!a.sees("phone.ring", QUIET).await, "the caller is not rung");

    // Without device_id: every other phone.
    assert_eq!(a.ok("phone.ring", json!({"stop": true})).await["rang"], 1);
    assert_eq!(b.event("phone.ring").await["stop"], true);

    // Tray: every connected phone, from the PC.
    assert_eq!(helm_agent::g6::ring_from_pc(server.state()), 2);
    assert_eq!(a.event("phone.ring").await["from"]["name"], "TEST-PC");
    b.event("phone.ring").await;

    drop(b);
    wait_until(|| server.state().sessions().len() == 1).await;
    assert_eq!(a.ok("phone.ring", json!({"device_id": phone_b.device_id})).await["rang"], 0);
    let list = a.ok("phone.list", json!({})).await;
    let other = list["phones"].as_array().unwrap().iter().find(|p| p["device_id"] == phone_b.device_id).cloned().unwrap();
    assert_eq!(other["online"], false);
    server.shutdown().await;
}

fn notif(id: &str, app: &str, title: &str) -> PcNotification {
    PcNotification {
        id: id.into(),
        app: app.into(),
        title: title.into(),
        body: "body".into(),
        time_ms: 1,
    }
}

#[tokio::test]
async fn pc_notifications_are_forwarded_only_when_watching_and_allowed() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |_| {}).await;
    server.state().g6.set_notify_poll(Duration::from_millis(30));
    let (mut c, _) = paired_as(&server, "Pixel").await;

    fake.with(|s| {
        s.notify_access = NotifyAccess::Denied;
        s.pc_notifications = vec![notif("1", "Outlook", "old")];
    });
    assert_eq!(c.ok("pc.notify.watch", json!({"on": true})).await["access"], "denied");
    fake.with(|s| s.pc_notifications.push(notif("2", "Outlook", "new while denied")));
    assert!(!c.sees("pc.notification", QUIET).await);

    fake.with(|s| s.notify_access = NotifyAccess::Allowed);
    assert_eq!(c.ok("pc.notify.watch", json!({"on": true})).await["access"], "allowed");
    fake.with(|s| {
        s.pc_notifications.push(notif("3", "Helm", "own toast"));
        s.pc_notifications.push(notif("4", "Teams", "Standup"));
    });
    let ev = c.event("pc.notification").await;
    assert_eq!(ev["id"], "4");
    assert_eq!(ev["app"], "Teams");
    assert_eq!(ev["title"], "Standup");
    assert_eq!(ev["body"], "body");
    assert!(!c.sees("pc.notification", QUIET).await, "baseline and own toasts are not sent");

    c.ok("pc.notify.watch", json!({"on": false})).await;
    fake.with(|s| s.pc_notifications.push(notif("5", "Teams", "after off")));
    assert!(!c.sees("pc.notification", QUIET).await);
    assert_eq!(server.state().g6.notify_watching(), 0);

    fake.with(|s| s.notify_access = NotifyAccess::Unsupported);
    assert_eq!(c.ok("pc.notify.watch", json!({"on": true})).await["access"], "unsupported");
    assert_eq!(c.err("pc.notify.watch", json!({})).await, "bad_params");
    server.shutdown().await;
}

#[tokio::test]
async fn schedules_run_deck_and_power_actions_and_one_offs_disable() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |_| {}).await;
    fake.with(|s| s.utc_offset_minutes = 120);
    let (mut c, _) = paired_as(&server, "Pixel").await;
    assert_eq!(c.ok("schedule.list", json!({})).await["schedules"], json!([]));

    // A one-off ~2 hours from now (local HH:MM computed with the fake +02:00 offset).
    let local = helm_agent::g6::now_ms() as i64 + 120 * 60_000 + 2 * 3_600_000;
    let mins = (local.div_euclid(60_000)).rem_euclid(1440);
    let time = format!("{:02}:{:02}", mins / 60, mins % 60);
    let r = c
        .ok("schedule.set", json!({"schedules":[
            {"id":"vol","name":"Louder","enabled":true,"time":time,"days":[],"action":{"volume_step":0.1},
             "next_ms": 5, "last_run_ms": 5},
            {"id":"off","enabled":true,"time":time,"days":[1,2,3,4,5,6,7],"action":{"power":"shutdown"}},
            {"id":"never","enabled":false,"time":"03:00","days":[1],"action":{"text":"x"}},
        ]}))
        .await;
    let list = r["schedules"].as_array().unwrap();
    assert!(list[0]["date"].is_string(), "one-off date filled in: {r}");
    assert_eq!(list[0]["last_run_ms"], Value::Null, "agent-owned fields are ignored");
    let next = list[0]["next_ms"].as_u64().unwrap();
    assert_eq!(list[1]["next_ms"], next);
    assert_eq!(list[2]["next_ms"], Value::Null);
    c.event("schedule.changed").await;
    assert!(dir.path().join("schedules.json").exists());

    helm_agent::g6::schedule_tick_at(server.state(), next + 1_000);
    let mut ran = vec![
        c.event("schedule.ran").await,
        c.event("schedule.ran").await,
    ];
    ran.sort_by_key(|v| v["id"].as_str().unwrap().to_string());
    assert_eq!(ran[0], json!({"id":"off","ok":true}));
    assert_eq!(ran[1], json!({"id":"vol","ok":true}));
    assert!(fake.calls().iter().any(|x| x.starts_with("set_volume:")));
    let dry = server.state().power_dry_run.as_ref().unwrap();
    assert_eq!(dry.executed(), vec![PowerAction::Shutdown]);
    let changed = c.event("schedule.changed").await;
    let vol = &changed["schedules"][0];
    assert_eq!(vol["enabled"], false, "one-off disabled after its run");
    assert!(vol["last_run_ms"].is_u64());

    // Same moment again: nothing runs twice.
    helm_agent::g6::schedule_tick_at(server.state(), next + 2_000);
    assert!(!c.sees("schedule.ran", QUIET).await);
    assert!(std::fs::read_to_string(dir.path().join("audit.log")).unwrap().contains("schedule_run"));

    for bad in [
        json!({}),
        json!({"schedules":[{"id":"x","time":"25:00","action":{"text":"x"}}]}),
        json!({"schedules":[{"id":"x","time":"10:00","action":{"nope":1}}]}),
        json!({"schedules":[{"id":"x","time":"10:00","days":[9],"action":{"text":"x"}}]}),
    ] {
        assert_eq!(c.err("schedule.set", bad.clone()).await, "bad_params", "{bad}");
    }
    server.shutdown().await;
}

#[tokio::test]
async fn health_card_reports_battery_temp_disks_and_uptime() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |_| {}).await;
    let (mut c, _) = paired_as(&server, "Pixel").await;
    let h = c.ok("system.health", json!({})).await;
    assert_eq!(h["cpu_temp_c"], 48.5);
    assert_eq!(h["uptime_s"], 3600);
    assert!(h["agent_uptime_s"].is_u64());
    assert_eq!(h["disks"][0]["mount"], "C:\\");
    assert_eq!(h["disks"][0]["free_bytes"], 100);
    assert_eq!(h["disks"][0]["total_bytes"], 1000);
    fake.with(|s| {
        s.cpu_temp_c = None;
        s.battery = None;
    });
    let h = c.ok("system.health", json!({})).await;
    assert_eq!(h["cpu_temp_c"], Value::Null);
    assert_eq!(h["battery"], Value::Null);
    server.shutdown().await;
}
