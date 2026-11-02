//! G3 end-to-end tests (`Docs/protocol/streaming-2.md`) against the `FakePlatform`:
//! `windows.thumb`, window streams, the gaming preset and input over the `/stream` socket
//! (with a latency measurement of both input paths; run with `--nocapture` to see it).

mod common;

use std::time::{Duration, Instant};

use common::*;
use futures_util::StreamExt;
use helm_agent::platform::VideoSource;
use helm_win_input::Op;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

struct Ctl {
    ws: Ws,
    next: i64,
}

impl Ctl {
    async fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        send(&mut self.ws, json!({"type":"req","id":id,"method":method,"params":params})).await;
        loop {
            let m = recv(&mut self.ws).await;
            if m["type"] == "res" && m["id"] == id {
                return m;
            }
        }
    }
    async fn ok(&mut self, method: &str, params: Value) -> Value {
        let r = self.call(method, params).await;
        assert_eq!(r["ok"], true, "{method}: {r}");
        r["result"].clone()
    }
    async fn err(&mut self, method: &str, params: Value) -> String {
        let r = self.call(method, params).await;
        assert_eq!(r["ok"], false, "{method} should fail: {r}");
        r["error"]["code"].as_str().unwrap().to_string()
    }
}

async fn paired(server: &helm_agent::ServerHandle) -> (Ctl, Phone, Value) {
    let phone = Phone::new("Pixel");
    let mut ws = connect(server).await;
    let hello = recv(&mut ws).await;
    let token = server.new_pairing().token;
    send(&mut ws, phone.pair_msg(&hello, &token)).await;
    let ready = recv(&mut ws).await;
    assert_eq!(ready["type"], "ready", "{ready}");
    (Ctl { ws, next: 1 }, phone, hello)
}

async fn open_stream(server: &helm_agent::ServerHandle, phone: &Phone, token: &str) -> Ws {
    let mut ws = try_connect_path(server.port(), server.fingerprint(), "/stream")
        .await
        .expect("connect /stream");
    let hello = recv(&mut ws).await;
    send(&mut ws, phone.auth_msg(&hello)).await;
    assert_eq!(recv(&mut ws).await["type"], "ready");
    send(&mut ws, json!({"type":"stream.open","token":token})).await;
    // First frame proves the stream is attached.
    loop {
        let m = tokio::time::timeout(Duration::from_secs(5), ws.next()).await.unwrap().unwrap().unwrap();
        if matches!(m, Message::Binary(_)) {
            return ws;
        }
    }
}

async fn wait_for(mut cond: impl FnMut() -> bool) {
    for _ in 0..200 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("condition not met in time");
}

#[tokio::test]
async fn thumbnails_window_stream_and_gaming_preset() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |_| {}).await;
    fake.with(|s| s.windows[1].is_minimized = true);
    let (mut ctl, _phone, hello) = paired(&server).await;
    assert!(hello["features"].as_array().unwrap().contains(&json!("g3")));

    let r = ctl.ok("windows.thumb", json!({"hwnd": 100, "size": 128})).await;
    assert!(r["png"].as_str().unwrap().len() > 20);
    assert!(fake.calls().contains(&"window_thumbnail_png:100:128".to_string()));
    let r = ctl.ok("windows.thumb", json!({"hwnd": 200})).await;
    assert!(r["png"].is_null(), "minimized window has no thumbnail: {r}");
    assert_eq!(ctl.err("windows.thumb", json!({"hwnd": 999})).await, "bad_params");
    assert_eq!(ctl.err("windows.thumb", json!({"hwnd": 100, "size": 4})).await, "bad_params");

    // A window stream with the gaming preset: 720p120, audio on, low-latency tuning.
    let r = ctl.ok("stream.start", json!({"hwnd": 100, "preset": "gaming"})).await;
    assert_eq!(r["preset"], "gaming");
    assert_eq!(r["fps"], 120);
    assert_eq!((r["width"].as_u64(), r["height"].as_u64()), (Some(1280), Some(720)));
    assert_eq!(r["audio"], true);
    assert_eq!(r["input_on_stream"], true);
    let p = fake.video().unwrap().params.lock().unwrap().clone().unwrap();
    assert_eq!(p.source, VideoSource::Window(100));
    assert!(p.low_latency);
    // Default preset stays as before.
    let r = ctl.ok("stream.start", json!({"monitor_id": "\\\\.\\DISPLAY1"})).await;
    assert_eq!(r["preset"], "default");
    let p = fake.video().unwrap().params.lock().unwrap().clone().unwrap();
    assert!(!p.low_latency);
    assert_eq!(ctl.err("stream.start", json!({"preset": "turbo"})).await, "bad_params");
    server.shutdown().await;
}

/// Time from sending one `mm` frame until the fake injector has it.
async fn one_move(fake: &FakePlatform, ws: &mut Ws, dx: i32) -> Duration {
    let t0 = Instant::now();
    send(ws, json!({"type":"in","k":"mm","dx":dx,"dy":0})).await;
    let want = Op::Move { dx, dy: 0 };
    loop {
        if fake.ops().iter().rev().take(4).any(|o| *o == want) {
            return t0.elapsed();
        }
        assert!(t0.elapsed() < Duration::from_secs(5), "move {dx} never arrived");
        tokio::task::yield_now().await;
    }
}

fn median_us(mut v: Vec<Duration>) -> u128 {
    v.sort();
    v[v.len() / 2].as_micros()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn input_over_stream_socket_and_latency() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |_| {}).await;
    let (mut ctl, phone, _) = paired(&server).await;
    let r = ctl.ok("stream.start", json!({"max_width": 1280, "max_height": 720, "fps": 30})).await;
    let mut st = open_stream(&server, &phone, r["token"].as_str().unwrap()).await;

    // Same semantics as the control socket: buttons, absolute moves mapped onto the source.
    send(&mut st, json!({"type":"in","k":"ma","x":0.5,"y":0.5})).await;
    wait_for(|| fake.ops().contains(&Op::MoveTo { x: 960, y: 540 })).await;
    send(&mut st, json!({"type":"in","k":"mb","button":"left","down":true})).await;
    send(&mut st, json!({"type":"in","k":"mb","button":"left","down":false})).await;
    wait_for(|| fake.ops().iter().filter(|o| matches!(o, Op::Button { .. })).count() >= 2).await;
    // Anything that is not an `in` frame is ignored; the stream keeps flowing.
    send(&mut st, json!({"type":"req","id":1,"method":"power.shutdown"})).await;
    assert!(!fake.calls().iter().any(|c| c.contains("shutdown")));

    // Latency: idle, then with a ~900 KB request in flight on the control socket.
    let n = 40;
    let (mut ctl_idle, mut st_idle, mut ctl_busy, mut st_busy) = (vec![], vec![], vec![], vec![]);
    let pad = "x".repeat(900 * 1024);
    for i in 0..n {
        ctl_idle.push(one_move(&fake, &mut ctl.ws, 1000 + i).await);
        st_idle.push(one_move(&fake, &mut st, 2000 + i).await);
    }
    for i in 0..n {
        let big = json!({"type":"req","id":10_000 + i,"method":"system.ping","params":{"pad":pad}});
        send(&mut ctl.ws, big.clone()).await;
        ctl_busy.push(one_move(&fake, &mut ctl.ws, 3000 + i).await);
        send(&mut ctl.ws, big).await;
        st_busy.push(one_move(&fake, &mut st, 4000 + i).await);
    }
    println!(
        "input latency median (µs), {n} samples each: idle control={} stream={}; \
         control busy with a 900 KB request: control={} stream={}",
        median_us(ctl_idle),
        median_us(st_idle),
        median_us(ctl_busy),
        median_us(st_busy)
    );
    // The control socket still works as the fallback after all of this.
    assert!(ctl.ok("system.ping", json!({})).await.is_object());
    server.shutdown().await;
}

#[tokio::test]
async fn stream_socket_input_errors_reach_the_control_session() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |_| {}).await;
    let (mut ctl, phone, _) = paired(&server).await;
    let r = ctl.ok("stream.start", json!({"fps": 30})).await;
    let mut st = open_stream(&server, &phone, r["token"].as_str().unwrap()).await;
    send(&mut st, json!({"type":"in","k":"mm","dx":7,"dy":0})).await;
    wait_for(|| fake.ops().contains(&Op::Move { dx: 7, dy: 0 })).await;
    // Bad frames on /stream report through the owning control session.
    send(&mut st, json!({"type":"in","k":"zz"})).await;
    loop {
        let m = recv(&mut ctl.ws).await;
        if m["type"] == "event" && m["name"] == "input.error" {
            assert_eq!(m["data"]["code"], "bad_params");
            break;
        }
    }
    server.shutdown().await;
}
