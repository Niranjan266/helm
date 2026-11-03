//! g8 end-to-end tests (Docs/protocol/pointer.md) against a `FakePlatform`: the probe and
//! the injector are fakes, so no test moves the real cursor.

mod common;

use std::time::Duration;

use common::*;
use futures_util::StreamExt;
use helm_agent::platform::Rect;
use helm_agent::pointer::PointerProbe;
use helm_win_input::Op;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

async fn call(ws: &mut Ws, id: i64, method: &str, params: Value) -> Value {
    send(ws, json!({"type":"req","id":id,"method":method,"params":params})).await;
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(10), ws.next())
            .await
            .expect("no response")
            .expect("closed")
            .expect("ws error");
        if let Message::Text(t) = msg {
            let v: Value = serde_json::from_str(t.as_str()).unwrap();
            if v["type"] == "res" && v["id"] == id {
                return v;
            }
        }
    }
}

fn probe(x: i32, y: i32) -> PointerProbe {
    PointerProbe {
        x,
        y,
        bounds: Rect { x: 0, y: 0, width: 1920, height: 1080 },
        exclusive_fullscreen: false,
        cursor_clipped: false,
        cursor_hidden: false,
    }
}

async fn wait_ops(fake: &helm_agent::platform::fake::FakePlatform, n: usize) -> Vec<Op> {
    for _ in 0..200 {
        let ops = fake.ops();
        if ops.len() >= n {
            return ops;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("expected {n} ops, got {:?}", fake.ops());
}

#[tokio::test]
async fn hello_advertises_g8_and_the_mode_is_saved() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _fake) = start_fake_with(dir.path(), |_| {}).await;
    let mut ws = try_connect_path(server.port(), server.fingerprint(), "/ws")
        .await
        .unwrap();
    let hello = recv(&mut ws).await;
    assert!(hello["features"].as_array().unwrap().contains(&json!("g8")));
    drop(ws);

    let phone = Phone::new("Pixel");
    let token = server.new_pairing().token;
    let (mut ws, ready) = phone.pair(&server, &token).await;
    assert_eq!(ready["type"], "ready", "{ready}");

    let r = call(&mut ws, 1, "pointer.settings", json!({})).await;
    assert_eq!(r["result"]["mode"], "auto");
    let r = call(&mut ws, 2, "pointer.set", json!({"mode": "fast"})).await;
    assert_eq!(r["error"]["code"], "bad_params");
    let r = call(&mut ws, 3, "pointer.set", json!({"mode": "relative"})).await;
    assert_eq!(r["result"]["mode"], "relative");
    let saved = std::fs::read_to_string(dir.path().join("pointer.json")).unwrap();
    assert!(saved.contains("relative"), "{saved}");
    server.shutdown().await;
}

#[tokio::test]
async fn mm_frames_become_absolute_moves_unless_the_app_needs_raw_input() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |_| {}).await;
    let phone = Phone::new("Pixel");
    let token = server.new_pairing().token;
    let (mut ws, ready) = phone.pair(&server, &token).await;
    assert_eq!(ready["type"], "ready", "{ready}");

    // No probe (unsupported): relative, exactly as before g8.
    send(&mut ws, json!({"type":"in","k":"mm","dx":3,"dy":2})).await;
    assert_eq!(wait_ops(&fake, 1).await, vec![Op::Move { dx: 3, dy: 2 }]);

    // Auto + normal desktop: absolute from the real position, the fraction is carried.
    fake.set_pointer_probe(Some(probe(100, 100)));
    send(&mut ws, json!({"type":"in","k":"mm","dx":3.5,"dy":-2})).await;
    assert_eq!(wait_ops(&fake, 2).await[1], Op::MoveTo { x: 103, y: 98 });
    send(&mut ws, json!({"type":"in","k":"mm","dx":3.5,"dy":0})).await;
    assert_eq!(wait_ops(&fake, 3).await[2], Op::MoveTo { x: 104, y: 100 });

    // A cursor-clipped foreground app (a game) keeps relative motion in auto...
    let mut clipped = probe(100, 100);
    clipped.cursor_clipped = true;
    fake.set_pointer_probe(Some(clipped));
    send(&mut ws, json!({"type":"in","k":"mm","dx":4,"dy":1})).await;
    assert_eq!(wait_ops(&fake, 4).await[3], Op::Move { dx: 4, dy: 1 });

    // ...but `absolute` forces absolute and `relative` forces relative.
    let r = call(&mut ws, 1, "pointer.set", json!({"mode": "absolute"})).await;
    assert_eq!(r["result"]["mode"], "absolute");
    // The input thread reads the mode per frame, so no reconnect is needed.
    send(&mut ws, json!({"type":"in","k":"mm","dx":5,"dy":0})).await;
    assert_eq!(wait_ops(&fake, 5).await[4], Op::MoveTo { x: 105, y: 100 });
    call(&mut ws, 2, "pointer.set", json!({"mode": "relative"})).await;
    fake.set_pointer_probe(Some(probe(100, 100)));
    send(&mut ws, json!({"type":"in","k":"mm","dx":1,"dy":1})).await;
    assert_eq!(wait_ops(&fake, 6).await[5], Op::Move { dx: 1, dy: 1 });
    server.shutdown().await;
}
