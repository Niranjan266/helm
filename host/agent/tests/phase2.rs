//! Phase 2 end-to-end tests (PROTOCOL §10) against a `FakePlatform` whose streamer emits
//! synthetic H.264 access units. The `#[ignore]`d test at the end streams the real screen.

mod common;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use common::*;
use futures_util::StreamExt;
use helm_agent::platform::Monitor;
use helm_win_input::Op;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

/// A parsed stream frame.
#[derive(Debug, Clone)]
struct Frame {
    kind: u8,
    pts: u64,
    payload: Vec<u8>,
}

fn parse(b: &[u8]) -> Frame {
    assert!(b.len() >= 9, "short frame");
    Frame {
        kind: b[0],
        pts: u64::from_be_bytes(b[1..9].try_into().unwrap()),
        payload: b[9..].to_vec(),
    }
}

/// Fake AU sequence number (written after the slice NAL header).
fn seq(f: &Frame) -> u32 {
    let p = &f.payload;
    let at = if f.kind == 0x02 { 21 } else { 5 };
    u32::from_be_bytes(p[at..at + 4].try_into().unwrap())
}

/// Control session that keeps events seen while waiting for responses.
struct Ctl {
    ws: Ws,
    next: i64,
    events: Vec<Value>,
}

impl Ctl {
    async fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        send(
            &mut self.ws,
            json!({"type":"req","id":id,"method":method,"params":params}),
        )
        .await;
        loop {
            let m = recv(&mut self.ws).await;
            if m["type"] == "res" && m["id"] == id {
                return m;
            }
            self.events.push(m);
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
    async fn event(&mut self, name: &str) -> Value {
        if let Some(i) = self.events.iter().position(|m| m["name"] == name) {
            return self.events.remove(i)["data"].clone();
        }
        loop {
            let m = recv(&mut self.ws).await;
            if m["type"] == "event" && m["name"] == name {
                return m["data"].clone();
            }
            self.events.push(m);
        }
    }
}

async fn paired(server: &helm_agent::ServerHandle) -> (Ctl, Phone) {
    let phone = Phone::new("Pixel");
    let token = server.new_pairing().token;
    let (ws, ready) = phone.pair(server, &token).await;
    assert_eq!(ready["type"], "ready", "{ready}");
    (
        Ctl {
            ws,
            next: 1,
            events: vec![],
        },
        phone,
    )
}

/// Open `/stream`, auth, `stream.open`. Returns the socket (ready to receive frames).
async fn open_stream(server: &helm_agent::ServerHandle, phone: &Phone, token: &str) -> Ws {
    let mut ws = try_connect_path(server.port(), server.fingerprint(), "/stream")
        .await
        .expect("connect /stream");
    let hello = recv(&mut ws).await;
    assert_eq!(hello["type"], "hello");
    send(&mut ws, phone.auth_msg(&hello)).await;
    let ready = recv(&mut ws).await;
    assert_eq!(ready["type"], "ready", "{ready}");
    send(&mut ws, json!({"type":"stream.open","token":token})).await;
    ws
}

/// Next binary frame (panics on text, close or timeout).
async fn next_frame(ws: &mut Ws) -> Frame {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("timed out waiting for a stream frame")
            .expect("stream closed")
            .expect("websocket error");
        match msg {
            Message::Binary(b) => return parse(&b),
            Message::Text(t) => panic!("unexpected text on /stream: {t}"),
            Message::Close(f) => panic!("unexpected close: {f:?}"),
            _ => {}
        }
    }
}

/// Frames for `dur`.
async fn collect(ws: &mut Ws, dur: Duration) -> Vec<Frame> {
    let end = Instant::now() + dur;
    let mut out = vec![];
    while Instant::now() < end {
        out.push(next_frame(ws).await);
    }
    out
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
async fn stream_delivers_keyframe_first_audio_and_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |_| {}).await;
    let (mut ctl, phone) = paired(&server).await;
    let r = ctl
        .ok(
            "stream.start",
            json!({"max_width":1280,"max_height":720,"fps":60,"bitrate_kbps":6000,"audio":true}),
        )
        .await;
    assert_eq!(r["width"], 1280);
    assert_eq!(r["height"], 720);
    assert_eq!(r["fps"], 60);
    assert_eq!(r["codec"], "h264");
    assert_eq!(r["encoder"], "hardware");
    assert_eq!(r["audio"], true);
    assert_eq!(r["opus_pre_skip"], 120);
    assert_eq!(r["token"].as_str().unwrap().len(), 43);
    // No monitor_id / hwnd = primary monitor.
    let params = fake.video().unwrap().params.lock().unwrap().clone().unwrap();
    assert_eq!(params.source, helm_agent::platform::VideoSource::Monitor(None));

    let mut ws = open_stream(&server, &phone, r["token"].as_str().unwrap()).await;
    let frames = collect(&mut ws, Duration::from_millis(700)).await;
    let video: Vec<&Frame> = frames.iter().filter(|f| f.kind <= 2).collect();
    assert!(video.len() >= 10, "only {} video frames", video.len());
    assert_eq!(video[0].kind, 0x02, "first video frame must be a keyframe");
    assert_eq!(&video[0].payload[..5], &[0, 0, 0, 1, 0x67], "keyframe starts with SPS");
    assert!(video.windows(2).all(|w| w[1].pts > w[0].pts), "video pts increase");

    let audio: Vec<&Frame> = frames.iter().filter(|f| f.kind == 0x03).collect();
    assert!(audio.len() >= 10, "only {} audio packets", audio.len());
    assert_eq!(audio[0].payload, vec![0xFC, 0xFF, 0xFE]);
    assert!(audio.windows(2).all(|w| w[1].pts == w[0].pts + 20_000));
    // Audio and video share one clock (both near "now", not each from its own zero).
    let skew = audio[0].pts as i64 - video[0].pts as i64;
    assert!(skew.abs() < 500_000, "a/v skew {skew} µs");

    // Cursor: shape (once) before the first position; 960,540 on 1920x1080 → 640,360.
    let shape_at = frames.iter().position(|f| f.kind == 0x05).expect("cursor shape");
    let pos_at = frames.iter().position(|f| f.kind == 0x04).expect("cursor position");
    assert!(shape_at < pos_at);
    assert_eq!(frames.iter().filter(|f| f.kind == 0x05).count(), 1);
    let shape = &frames[shape_at].payload;
    assert_eq!(&shape[..12], &[0, 0, 0, 1, 0, 1, 0, 1, 0, 0, 0, 0]);
    assert_eq!(&shape[12..16], &[0x89, b'P', b'N', b'G']);
    let pos: Value = serde_json::from_slice(&frames[pos_at].payload).unwrap();
    assert_eq!(pos, json!({"x":640,"y":360,"visible":true,"shape_id":1}));
    // Position updates only when it changes.
    assert_eq!(frames.iter().filter(|f| f.kind == 0x04).count(), 1);
    fake.with(|s| s.cursor.x = 0);
    let moved = loop {
        let f = next_frame(&mut ws).await;
        if f.kind == 0x04 {
            break f;
        }
    };
    let pos: Value = serde_json::from_slice(&moved.payload).unwrap();
    assert_eq!(pos["x"], 0);

    // stream.stop closes the stream socket and stops the encoder and audio.
    ctl.ok("stream.stop", json!({})).await;
    expect_closed_binary(&mut ws).await;
    let v = fake.video().unwrap();
    wait_for(|| v.is_stopped() && fake.audio_streams() == 0).await;
    server.shutdown().await;
}

/// Like `expect_closed`, but binary frames may still be in flight.
async fn expect_closed_binary(ws: &mut Ws) {
    let r = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match ws.next().await {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return,
                Some(Ok(Message::Text(t))) => panic!("expected close, got {t}"),
                Some(Ok(_)) => continue,
            }
        }
    })
    .await;
    assert!(r.is_ok(), "stream socket not closed");
}

/// Expect `error{code}` then close on a `/stream` socket.
async fn expect_error(ws: &mut Ws, code: &str) {
    let m = recv(ws).await;
    assert_eq!(m["type"], "error", "{m}");
    assert_eq!(m["code"], code, "{m}");
    expect_closed(ws).await;
}

#[tokio::test]
async fn stream_token_is_single_use_device_bound_and_expires() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |c| {
        c.stream_token_lifetime = Duration::from_millis(1500);
    })
    .await;
    let (mut ctl, phone) = paired(&server).await;
    let other = Phone::new("Other");
    let token = server.new_pairing().token;
    let (_other_ws, ready) = other.pair(&server, &token).await;
    assert_eq!(ready["type"], "ready");

    // Wrong token, then a token used by another paired device: rejected.
    let mut ws = open_stream(&server, &phone, "not-a-token").await;
    expect_error(&mut ws, "bad_token").await;
    let t = ctl.ok("stream.start", json!({})).await["token"]
        .as_str()
        .unwrap()
        .to_string();
    let mut ws = open_stream(&server, &other, &t).await;
    expect_error(&mut ws, "bad_token").await;
    // ...and that attempt burnt it (single use, whoever presents it).
    let mut ws = open_stream(&server, &phone, &t).await;
    expect_error(&mut ws, "bad_token").await;

    // A good token works once.
    let t = ctl.ok("stream.start", json!({})).await["token"]
        .as_str()
        .unwrap()
        .to_string();
    let mut ws = open_stream(&server, &phone, &t).await;
    let _ = next_frame(&mut ws).await;
    let mut again = open_stream(&server, &phone, &t).await;
    expect_error(&mut again, "bad_token").await;
    drop(ws);

    // Expiry: never opened → token refused, stream ended and stopped.
    let t = ctl.ok("stream.start", json!({})).await["token"]
        .as_str()
        .unwrap()
        .to_string();
    let v = fake.video().unwrap();
    let ended = ctl.event("stream.ended").await;
    assert!(ended["reason"].as_str().unwrap().contains("not opened"), "{ended}");
    wait_for(|| v.is_stopped()).await;
    let mut ws = open_stream(&server, &phone, &t).await;
    expect_error(&mut ws, "bad_token").await;
    server.shutdown().await;
}

#[tokio::test]
async fn stream_socket_rejects_pair_and_needs_stream_open() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _fake) = start_fake_with(dir.path(), |_| {}).await;
    let phone = Phone::new("New");
    let token = server.new_pairing().token;
    let mut ws = try_connect_path(server.port(), server.fingerprint(), "/stream")
        .await
        .unwrap();
    let hello = recv(&mut ws).await;
    let features = hello["features"].as_array().unwrap();
    for f in ["p1", "p2", "p3", "p4", "p5"] {
        assert!(features.contains(&json!(f)), "{f} in {features:?}");
    }
    send(&mut ws, phone.pair_msg(&hello, &token)).await;
    expect_error(&mut ws, "bad_message").await;
    assert!(server.devices().unwrap().is_empty(), "nothing paired via /stream");
    // The pairing token was not consumed: pairing on /ws still works.
    let (_ws, ready) = phone.pair(&server, &token).await;
    assert_eq!(ready["type"], "ready", "{ready}");
    // After auth, anything but stream.open is refused.
    let mut ws = try_connect_path(server.port(), server.fingerprint(), "/stream")
        .await
        .unwrap();
    let hello = recv(&mut ws).await;
    send(&mut ws, phone.auth_msg(&hello)).await;
    assert_eq!(recv(&mut ws).await["type"], "ready");
    send(&mut ws, json!({"type":"req","id":1,"method":"system.ping"})).await;
    expect_error(&mut ws, "bad_message").await;
    // Unknown paths are still refused.
    assert!(
        try_connect_path(server.port(), server.fingerprint(), "/other")
            .await
            .is_err()
    );
    server.shutdown().await;
}

#[tokio::test]
async fn backpressure_drops_only_until_a_keyframe_and_requests_idr() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |_| {}).await;
    fake.with(|s| s.au_size = 512 * 1024); // 60 fps × 512 KiB ≈ 30 MB/s
    let (mut ctl, phone) = paired(&server).await;
    let t = ctl.ok("stream.start", json!({"fps":60})).await["token"]
        .as_str()
        .unwrap()
        .to_string();
    let mut ws = open_stream(&server, &phone, &t).await;
    let v = fake.video().unwrap();
    let _ = next_frame(&mut ws).await;
    let requests_after_attach = v.keyframe_requests.load(Ordering::SeqCst);
    // Stop reading: TCP buffers fill, the agent's queue fills, frames get dropped.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let frames = collect(&mut ws, Duration::from_millis(2500)).await;
    let video: Vec<&Frame> = frames.iter().filter(|f| f.kind <= 2).collect();
    let emitted = v.frames.load(Ordering::SeqCst);
    assert!(
        (video.len() as u64) < emitted,
        "some frames must have been dropped ({} of {emitted})",
        video.len()
    );
    // Every gap in the sequence resumes at a keyframe (the decoder never sees broken refs).
    let mut gaps = 0;
    for w in video.windows(2) {
        if seq(w[1]) != seq(w[0]) + 1 {
            gaps += 1;
            assert_eq!(w[1].kind, 0x02, "frame after a gap must be a keyframe");
        }
    }
    assert!(gaps >= 1, "expected at least one drop gap");
    assert!(
        v.keyframe_requests.load(Ordering::SeqCst) > requests_after_attach,
        "IDR requested once the queue drained"
    );
    server.shutdown().await;
}

#[tokio::test]
async fn idr_config_stats_and_adaptive_bitrate() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |_| {}).await;
    let (mut ctl, phone) = paired(&server).await;
    assert_eq!(ctl.err("stream.idr", json!({})).await, "bad_params");
    assert_eq!(ctl.ok("stream.stats", json!({"dropped":1})).await, json!({}));
    let t = ctl
        .ok("stream.start", json!({"bitrate_kbps":10000,"fps":30}))
        .await["token"]
        .as_str()
        .unwrap()
        .to_string();
    let mut ws = open_stream(&server, &phone, &t).await;
    let _ = next_frame(&mut ws).await;
    let v = fake.video().unwrap();
    let before = v.keyframe_requests.load(Ordering::SeqCst);
    ctl.ok("stream.idr", json!({})).await;
    assert_eq!(v.keyframe_requests.load(Ordering::SeqCst), before + 1);

    let r = ctl
        .ok(
            "stream.stats",
            json!({"rx_kbps":9000,"decoded_fps":29.5,"dropped":2,"rtt_ms":8}),
        )
        .await;
    assert_eq!(r["bitrate_kbps"], 8000, "dropped frames: -20 %");
    assert_eq!(v.bitrates.lock().unwrap().last(), Some(&8000));
    let r = ctl.ok("stream.stats", json!({"dropped":0})).await;
    assert_eq!(r["bitrate_kbps"], 8000, "healthy for < 5 s: unchanged");

    let r = ctl.ok("stream.config", json!({"bitrate_kbps":4000})).await;
    assert_eq!(r["bitrate_kbps"], 4000);
    assert_eq!(v.bitrates.lock().unwrap().last(), Some(&4000));
    assert_eq!(ctl.err("stream.config", json!({"fps":0})).await, "bad_params");
    // fps change restarts the encoder; frames keep flowing with increasing pts.
    let r = ctl.ok("stream.config", json!({"fps":15})).await;
    assert_eq!(r["fps"], 15);
    let v2 = fake.video().unwrap();
    assert!(!Arc::ptr_eq(&v, &v2));
    assert!(v.is_stopped());
    assert_eq!(v2.params.lock().unwrap().as_ref().unwrap().bitrate_kbps, 4000);
    let frames = collect(&mut ws, Duration::from_millis(500)).await;
    let video: Vec<&Frame> = frames.iter().filter(|f| f.kind <= 2).collect();
    assert!(!video.is_empty());
    assert!(video.windows(2).all(|w| w[1].pts > w[0].pts));

    // Phase 5: recording taps the running stream (fake recorder, nothing written but a stub).
    assert!(ctl.ok("record.start", json!({"audio":true})).await["path"].is_string());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(ctl.ok("record.stop", json!({})).await["duration_ms"].is_u64());
    server.shutdown().await;
}

#[tokio::test]
async fn stream_ends_by_itself_and_on_disconnect() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |_| {}).await;
    let (mut ctl, phone) = paired(&server).await;
    let t = ctl.ok("stream.start", json!({})).await["token"]
        .as_str()
        .unwrap()
        .to_string();
    let mut ws = open_stream(&server, &phone, &t).await;
    let _ = next_frame(&mut ws).await;
    *fake.video().unwrap().end_with.lock().unwrap() = Some("monitor unplugged".into());
    assert_eq!(ctl.event("stream.ended").await["reason"], "monitor unplugged");
    expect_closed_binary(&mut ws).await;

    // A new stream; the control socket goes away → stream stops, stream socket closes.
    let t = ctl.ok("stream.start", json!({})).await["token"]
        .as_str()
        .unwrap()
        .to_string();
    let mut ws = open_stream(&server, &phone, &t).await;
    let _ = next_frame(&mut ws).await;
    let v = fake.video().unwrap();
    drop(ctl);
    expect_closed_binary(&mut ws).await;
    wait_for(|| v.is_stopped()).await;
    server.shutdown().await;
}

#[tokio::test]
async fn tiers_and_absolute_move() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |_| {}).await;
    fake.with(|s| {
        s.monitors.push(Monitor {
            id: "\\\\.\\DISPLAY2".into(),
            name: "Left".into(),
            x: -1280,
            y: 0,
            width: 1280,
            height: 1024,
            is_primary: false,
        })
    });
    let (mut ctl, phone) = paired(&server).await;
    // Viewer: every stream method is forbidden.
    let path = dir.path().join("devices.json");
    let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    v["devices"][0]["tier"] = json!("viewer");
    std::fs::write(&path, v.to_string()).unwrap();
    let (ws, ready) = phone.auth(&server).await;
    assert_eq!(ready["tier"], "viewer");
    let mut viewer = Ctl {
        ws,
        next: 1,
        events: vec![],
    };
    for m in ["stream.start", "stream.stop", "stream.idr", "stream.config", "stream.stats", "record.start"] {
        assert_eq!(viewer.err(m, json!({})).await, "forbidden", "{m}");
    }
    send(&mut viewer.ws, json!({"type":"in","k":"ma","x":0.5,"y":0.5})).await;
    assert_eq!(viewer.event("input.error").await["code"], "forbidden");

    // Admin (paired before the edit) streams fine. No stream: `ma` maps to the primary.
    send(&mut ctl.ws, json!({"type":"in","k":"ma","x":0.5,"y":0.5})).await;
    wait_for(|| fake.ops().contains(&Op::MoveTo { x: 960, y: 540 })).await;
    // Streaming the left monitor: `ma` is normalised to it.
    ctl.ok("stream.start", json!({"monitor_id":"\\\\.\\DISPLAY2"})).await;
    send(&mut ctl.ws, json!({"type":"in","k":"ma","x":0.0,"y":1.0})).await;
    wait_for(|| fake.ops().contains(&Op::MoveTo { x: -1280, y: 1023 })).await;
    send(&mut ctl.ws, json!({"type":"in","k":"ma","x":"left"})).await;
    assert_eq!(ctl.event("input.error").await["code"], "bad_params");
    assert_eq!(
        ctl.err("stream.start", json!({"monitor_id":"nope"})).await,
        "bad_params"
    );
    server.shutdown().await;
}

/// Streams the real primary monitor for 2 s through the real agent (read-only: no input is
/// injected). Run with `cargo test --test phase2 -- --ignored`.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn live_screen_stream_delivers_keyframes() {
    helm_agent::screen::make_process_dpi_aware();
    let dir = tempfile::tempdir().unwrap();
    let mut config = helm_agent::Config::new(dir.path());
    config.port = 0;
    config.enable_mdns = false;
    config.host_name = Some("TEST-PC".into());
    config.addresses = Some(vec![std::net::Ipv4Addr::LOCALHOST]);
    config.platform = Some(Arc::new(helm_agent::platform::real::RealPlatform::new(
        &dir.path().join("icons"),
    )));
    config.power_dry_run = true;
    let server = helm_agent::start(config).await.unwrap();
    let (mut ctl, phone) = paired(&server).await;
    let r = ctl
        .ok(
            "stream.start",
            json!({"max_width":1280,"max_height":720,"fps":30,"bitrate_kbps":4000,"audio":true}),
        )
        .await;
    println!("stream.start → {r}");
    let mut ws = open_stream(&server, &phone, r["token"].as_str().unwrap()).await;
    let started = Instant::now();
    let mut frames = vec![];
    while started.elapsed() < Duration::from_secs(1) {
        frames.push(next_frame(&mut ws).await);
    }
    ctl.ok("stream.idr", json!({})).await;
    while started.elapsed() < Duration::from_secs(2) {
        frames.push(next_frame(&mut ws).await);
    }
    ctl.ok("stream.stop", json!({})).await;
    let video: Vec<&Frame> = frames.iter().filter(|f| f.kind <= 2).collect();
    let keys: Vec<&&Frame> = video.iter().filter(|f| f.kind == 0x02).collect();
    let count = |k: u8| frames.iter().filter(|f| f.kind == k).count();
    println!(
        "{} video ({} key), {} audio, {} cursor, {} shapes, {} bytes",
        video.len(),
        keys.len(),
        count(3),
        count(4),
        count(5),
        video.iter().map(|f| f.payload.len()).sum::<usize>()
    );
    assert!(video.len() >= 20, "too few video frames: {}", video.len());
    assert_eq!(video[0].kind, 0x02, "first frame is a keyframe");
    assert!(keys.len() >= 2, "requested IDR arrived");
    for k in &keys {
        let nal_types: Vec<u8> = k
            .payload
            .windows(4)
            .filter(|w| w[..3] == [0, 0, 1])
            .map(|w| w[3] & 0x1F)
            .collect();
        assert!(nal_types.contains(&7) && nal_types.contains(&8) && nal_types.contains(&5), "{nal_types:?}");
    }
    assert!(video.windows(2).all(|w| w[1].pts > w[0].pts));
    server.shutdown().await;
}
