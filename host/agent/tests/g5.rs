//! G5 end-to-end tests (`Docs/protocol/dock-deck-2.md`) against a `FakePlatform`:
//! dock folders, deck pages and sizes, usage ranking, custom icons, Phase 1 compatibility.

mod common;

use std::collections::VecDeque;
use std::time::Duration;

use common::*;
use futures_util::StreamExt;
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

    async fn event(&mut self, name: &str) -> Value {
        let is = |m: &Value| m["type"] == "event" && m["name"] == name;
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
}

async fn paired(server: &helm_agent::ServerHandle) -> Client {
    let phone = Phone::new("Pixel");
    let token = server.new_pairing().token;
    let (ws, ready) = phone.pair(server, &token).await;
    assert_eq!(ready["type"], "ready", "{ready}");
    Client {
        ws,
        backlog: VecDeque::new(),
        next_id: 1,
    }
}

#[tokio::test]
async fn hello_advertises_g5() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _fake) = start_fake_with(dir.path(), |_| {}).await;
    let mut ws = connect(&server).await;
    let hello = recv(&mut ws).await;
    assert!(hello["features"].as_array().unwrap().contains(&json!("g5")));
    server.shutdown().await;
}

#[tokio::test]
async fn dock_folders_round_trip_and_flatten_for_old_phones() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _fake) = start_fake_with(dir.path(), |_| {}).await;
    let mut c = paired(&server).await;
    let pages = json!([[
        {"id":"a","kind":"app","app_id":"Fake.Notes"},
        {"id":"f","kind":"folder","label":"Work","icon":"b:folder","children":[
            {"id":"b","kind":"app","app_id":"Fake.Store_abc!App"},
            {"id":"c","kind":"open","target":"https://example.com"}
        ]},
        {"id":"d","kind":"open","target":"https://example.org","icon":"b:globe"}
    ]]);
    c.ok("dock.set", json!({"pages": pages})).await;
    let flat = json!([[
        {"id":"a","kind":"app","app_id":"Fake.Notes"},
        {"id":"b","kind":"app","app_id":"Fake.Store_abc!App"},
        {"id":"c","kind":"open","target":"https://example.com"},
        {"id":"d","kind":"open","target":"https://example.org","icon":"b:globe"}
    ]]);
    assert_eq!(c.event("dock.changed").await, json!({"pages": flat}));
    assert_eq!(c.ok("dock.get", json!({})).await, json!({"pages": flat}));
    assert_eq!(
        c.ok("dock.get", json!({"v":2})).await,
        json!({"pages": pages, "v": 2})
    );
    let raw: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("dock.json")).unwrap())
            .unwrap();
    assert_eq!(raw["version"], 2);
    // Folders inside folders and empty folders are refused.
    let nested = json!([[{"id":"f","kind":"folder","children":[
        {"id":"g","kind":"folder","children":[{"id":"x","kind":"app","app_id":"Fake.Notes"}]}]}]]);
    assert_eq!(c.err("dock.set", json!({"pages": nested})).await, "bad_params");
    let empty = json!([[{"id":"f","kind":"folder","children":[]}]]);
    assert_eq!(c.err("dock.set", json!({"pages": empty})).await, "bad_params");
    server.shutdown().await;
}

#[tokio::test]
async fn old_dock_file_reads_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let pages = json!([[{"id":"t1","kind":"app","app_id":"Fake.Notes","label":"Notes"}]]);
    std::fs::write(
        dir.path().join("dock.json"),
        json!({"version":1,"pages":pages}).to_string(),
    )
    .unwrap();
    let (server, _fake) = start_fake_with(dir.path(), |_| {}).await;
    let mut c = paired(&server).await;
    assert_eq!(c.ok("dock.get", json!({})).await, json!({"pages": pages}));
    assert_eq!(
        c.ok("dock.get", json!({"v":2})).await,
        json!({"pages": pages, "v": 2})
    );
    server.shutdown().await;
}

#[tokio::test]
async fn deck_pages_sizes_and_phase1_compatibility() {
    let dir = tempfile::tempdir().unwrap();
    // A Phase 1 deck file: one implicit page.
    std::fs::write(
        dir.path().join("deck.json"),
        r##"{"version":1,"buttons":[
            {"id":"a","label":"A","color":"#000000","action":{"text":"x"}},
            {"id":"b","label":"B","color":"#000000","action":{"launch":"Fake.Notes"}}]}"##,
    )
    .unwrap();
    let (server, _fake) = start_fake_with(dir.path(), |_| {}).await;
    let mut c = paired(&server).await;
    let v2 = c.ok("deck.get", json!({"v":2})).await;
    assert_eq!(
        v2["pages"],
        json!([{"id":"main","name":"Main","buttons":["a","b"]}])
    );

    let buttons = json!([
        {"id":"a","label":"A","color":"#000000","action":{"text":"x"},"size":"2x1"},
        {"id":"b","label":"B","color":"#000000","action":{"launch":"Fake.Notes"},"icon":"b:play","size":"2x2"},
        {"id":"c","label":"C","color":"#000000","action":{"text":"y"}}
    ]);
    let pages = json!([
        {"id":"p1","name":"Work","buttons":["a","c"]},
        {"id":"p2","name":"Media","buttons":["b"]}
    ]);
    c.ok("deck.set", json!({"buttons": buttons, "pages": pages})).await;
    c.event("deck.changed").await;
    let v2 = c.ok("deck.get", json!({"v":2})).await;
    assert_eq!(v2["buttons"], buttons);
    assert_eq!(v2["pages"], pages);
    // Phase 1 view: just the buttons.
    assert_eq!(c.ok("deck.get", json!({})).await, json!({"buttons": buttons}));

    // An old phone saves without sizes or pages: both are kept; a new button lands on page 1.
    let legacy = json!([
        {"id":"a","label":"A2","color":"#000000","action":{"text":"x"}},
        {"id":"b","label":"B","color":"#000000","action":{"launch":"Fake.Notes"}},
        {"id":"n","label":"New","color":"#000000","action":{"text":"n"}}
    ]);
    c.ok("deck.set", json!({"buttons": legacy})).await;
    let v2 = c.ok("deck.get", json!({"v":2})).await;
    assert_eq!(v2["buttons"][0]["size"], "2x1");
    assert_eq!(v2["buttons"][0]["label"], "A2");
    assert_eq!(v2["buttons"][1]["size"], "2x2");
    assert_eq!(
        v2["pages"],
        json!([
            {"id":"p1","name":"Work","buttons":["a","n"]},
            {"id":"p2","name":"Media","buttons":["b"]}
        ])
    );

    let dup = json!([{"id":"x","name":"1","buttons":[]},{"id":"x","name":"2","buttons":[]}]);
    assert_eq!(
        c.err("deck.set", json!({"buttons": legacy, "pages": dup})).await,
        "bad_params"
    );
    let bad_size = json!([{"id":"a","label":"A","color":"#000000","action":{"text":"x"},"size":"3x3"}]);
    assert_eq!(c.err("deck.set", json!({"buttons": bad_size})).await, "bad_params");
    server.shutdown().await;
}

#[tokio::test]
async fn usage_counts_launches_and_foreground() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |_| {}).await;
    let mut c = paired(&server).await;
    assert_eq!(c.ok("apps.usage", json!({})).await, json!({"apps": []}));

    c.ok("apps.launch", json!({"app_id":"Fake.Store_abc!App"})).await;
    c.ok(
        "deck.set",
        json!({"buttons":[{"id":"s","label":"S","color":"#000000","action":{"launch":"Fake.Store_abc!App"}}]}),
    )
    .await;
    c.ok("deck.run", json!({"id":"s"})).await;
    c.ok("apps.launch", json!({"app_id":"Fake.Notes"})).await;
    // Failed launches are not counted.
    assert_eq!(c.err("apps.launch", json!({"app_id":"Nope"})).await, "bad_params");

    // Foreground watcher: hwnd 100 is the fake "Notes" window.
    for _ in 0..200 {
        if fake.emit_foreground("Notes.exe") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let mut usage = Value::Null;
    for _ in 0..200 {
        usage = c.ok("apps.usage", json!({})).await;
        if usage["apps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["app_id"] == "Fake.Notes" && a["focus"] == 1)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let apps = usage["apps"].as_array().unwrap();
    assert_eq!(apps.len(), 2, "{usage}");
    assert_eq!(apps[0]["app_id"], "Fake.Store_abc!App");
    assert_eq!(apps[0]["launches"], 2);
    assert_eq!(apps[1]["app_id"], "Fake.Notes");
    assert_eq!(apps[1]["launches"], 1);
    assert_eq!(apps[1]["focus"], 1);
    assert!(apps[0]["last_used_ms"].as_u64().unwrap() > 0);
    assert!(dir.path().join("usage.json").exists());
    let one = c.ok("apps.usage", json!({"limit":1})).await;
    assert_eq!(one["apps"].as_array().unwrap().len(), 1);
    server.shutdown().await;
}

#[tokio::test]
async fn custom_icons_upload_and_fetch() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _fake) = start_fake_with(dir.path(), |_| {}).await;
    let mut c = paired(&server).await;
    let png = helm_agent::util::b64_encode(b"\x89PNG\r\n\x1a\nnot-really-a-png");
    let r = c.ok("deck.icon.put", json!({"png": png})).await;
    let icon = r["icon"].as_str().unwrap().to_string();
    assert!(icon.starts_with("c:") && icon.len() == 34, "{icon}");
    assert_eq!(c.ok("deck.icon.get", json!({"icon": icon})).await["png"], json!(png));
    assert_eq!(
        c.err("deck.icon.get", json!({"icon":"c:ffffffffffffffffffffffffffffffff"})).await,
        "bad_params"
    );
    assert_eq!(c.err("deck.icon.get", json!({"icon":"c:..\\..\\x"})).await, "bad_params");
    let gif = helm_agent::util::b64_encode(b"GIF89a....");
    assert_eq!(c.err("deck.icon.put", json!({"png": gif})).await, "bad_params");
    assert_eq!(c.err("deck.icon.put", json!({"png": "!!"})).await, "bad_params");
    // The icon can be used on a deck button.
    c.ok(
        "deck.set",
        json!({"buttons":[{"id":"i","label":"","color":"#000000","icon":icon,"action":{"text":"x"}}]}),
    )
    .await;
    server.shutdown().await;
}
