//! g9 (Docs/protocol/phone-control.md): the agent against a fake phone over real TLS.

mod common;

use std::time::Duration;

use common::*;
use futures_util::SinkExt;
use helm_agent::phone::{
    EditKey, GlobalAction, PhoneEvent, PhoneInput, RequestOptions,
};
use helm_agent::stream::{KIND_VIDEO, KIND_VIDEO_KEY, encode_frame};
use serde_json::{Value, json};
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::Message;

struct Control {
    ws: Ws,
    next: i64,
}

impl Control {
    async fn call(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        call(&mut self.ws, self.next, method, params).await
    }

    async fn ok(&mut self, method: &str, params: Value) -> Value {
        let r = self.call(method, params).await;
        assert_eq!(r["ok"], true, "{method}: {r}");
        r["result"].clone()
    }

    /// Next event with `name`, skipping everything else.
    async fn event(&mut self, name: &str) -> Value {
        loop {
            let m = recv(&mut self.ws).await;
            if m["type"] == "event" && m["name"] == name {
                return m["data"].clone();
            }
        }
    }
}

async fn paired(server: &helm_agent::ServerHandle) -> (Control, Phone) {
    let phone = Phone::new("Pixel");
    let token = server.new_pairing().token;
    let (ws, ready) = phone.pair(server, &token).await;
    assert_eq!(ready["type"], "ready");
    (Control { ws, next: 0 }, phone)
}

async fn next_phone_event(rx: &mut broadcast::Receiver<PhoneEvent>) -> PhoneEvent {
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("no phone event in time")
        .expect("event channel")
}

async fn wait_ended(rx: &mut broadcast::Receiver<PhoneEvent>) -> (String, bool) {
    loop {
        if let PhoneEvent::Ended { reason, was_active, .. } = next_phone_event(rx).await {
            return (reason, was_active);
        }
    }
}

async fn open_stream(server: &helm_agent::ServerHandle, phone: &Phone, token: &str) -> Ws {
    let mut ws = try_connect_path(server.port(), server.fingerprint(), "/stream")
        .await
        .expect("connect /stream");
    let hello = recv(&mut ws).await;
    send(&mut ws, phone.auth_msg(&hello)).await;
    assert_eq!(recv(&mut ws).await["type"], "ready");
    send(&mut ws, json!({"type":"phone.open","token":token})).await;
    ws
}

async fn audit_has(server: &helm_agent::ServerHandle, needle: &str) -> bool {
    for _ in 0..100 {
        let tail = server.state().audit.tail(200).unwrap();
        if tail.iter().any(|l| l.contains(needle)) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

/// Request, accept and open; returns the stream socket and the request id.
async fn live_session(
    server: &helm_agent::ServerHandle,
    ctl: &mut Control,
    phone: &Phone,
    control: bool,
) -> (Ws, u64) {
    let st = server.state().clone();
    ctl.ok("phone.caps", json!({"enabled": true, "accessibility": control})).await;
    let info = st.phone.request(&st, &phone.device_id, RequestOptions::default()).unwrap();
    let ev = ctl.event("phone.request").await;
    assert_eq!(ev["request_id"], info.id);
    assert_eq!(ev["pc_name"], "TEST-PC");
    assert_eq!(ev["relayed"], false);
    assert_eq!(ev["max_long"], 1920);
    let r = ctl
        .ok(
            "phone.accept",
            json!({"request_id": info.id, "width": 720, "height": 1600, "rotation": 0, "control": control}),
        )
        .await;
    let ws = open_stream(server, phone, r["token"].as_str().unwrap()).await;
    (ws, info.id)
}

#[tokio::test]
async fn hello_advertises_g9_and_old_phones_are_reported_unsupported() {
    let dir = tempfile::tempdir().unwrap();
    let server = start(dir.path()).await;
    let mut ws = connect(&server).await;
    let hello = recv(&mut ws).await;
    assert!(hello["features"].as_array().unwrap().contains(&json!("g9")));

    let (_ctl, phone) = paired(&server).await;
    let st = server.state().clone();
    let e = st.phone.request(&st, "nobody", RequestOptions::default()).unwrap_err();
    assert_eq!(e.code, "not_connected");
    // Connected, but it never said it speaks g9.
    let e = st.phone.request(&st, &phone.device_id, RequestOptions::default()).unwrap_err();
    assert_eq!(e.code, "unsupported");
    server.shutdown().await;
}

#[tokio::test]
async fn off_by_default_means_no_request_reaches_the_phone() {
    let dir = tempfile::tempdir().unwrap();
    let server = start(dir.path()).await;
    let (mut ctl, phone) = paired(&server).await;
    let st = server.state().clone();
    ctl.ok("phone.caps", json!({"enabled": false, "accessibility": false})).await;
    assert_eq!(st.phone.targets().len(), 1);
    assert!(!st.phone.targets()[0].enabled);
    let e = st.phone.request(&st, &phone.device_id, RequestOptions::default()).unwrap_err();
    assert_eq!(e.code, "disabled");
    assert!(st.phone.current().is_none());
    server.shutdown().await;
}

#[tokio::test]
async fn decline_ends_the_request_and_is_audited() {
    let dir = tempfile::tempdir().unwrap();
    let server = start(dir.path()).await;
    let (mut ctl, phone) = paired(&server).await;
    let st = server.state().clone();
    let mut events = st.phone.subscribe();
    ctl.ok("phone.caps", json!({"enabled": true, "accessibility": true})).await;
    let info = st.phone.request(&st, &phone.device_id, RequestOptions::default()).unwrap();
    // Only one at a time.
    let e = st.phone.request(&st, &phone.device_id, RequestOptions::default()).unwrap_err();
    assert_eq!(e.code, "busy");
    let ev = ctl.event("phone.request").await;
    ctl.ok("phone.decline", json!({"request_id": ev["request_id"], "reason": "declined"})).await;
    assert_eq!(wait_ended(&mut events).await, ("declined".to_string(), false));
    assert!(st.phone.current().is_none());
    assert!(audit_has(&server, "phone_request\t").await);
    assert!(audit_has(&server, &format!("id={} reason=declined", info.id)).await);
    // A second answer to the same request finds nothing.
    let r = ctl.call("phone.decline", json!({"request_id": info.id, "reason": "declined"})).await;
    assert_eq!(r["ok"], false);
    server.shutdown().await;
}

#[tokio::test]
async fn unanswered_request_times_out_and_the_phone_is_told() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _fake) = start_fake_with(dir.path(), |c| {
        c.phone_request_timeout = Duration::from_millis(100);
    })
    .await;
    let (mut ctl, phone) = paired(&server).await;
    let st = server.state().clone();
    let mut events = st.phone.subscribe();
    ctl.ok("phone.caps", json!({"enabled": true, "accessibility": true})).await;
    let info = st.phone.request(&st, &phone.device_id, RequestOptions::default()).unwrap();
    assert_eq!(wait_ended(&mut events).await, ("timeout".to_string(), false));
    let cancel = ctl.event("phone.cancel").await;
    assert_eq!(cancel["request_id"], info.id);
    assert_eq!(cancel["reason"], "timeout");
    server.shutdown().await;
}

#[tokio::test]
async fn accepted_session_carries_frames_and_input_and_audits_start_and_stop() {
    let dir = tempfile::tempdir().unwrap();
    let server = start(dir.path()).await;
    let (mut ctl, phone) = paired(&server).await;
    let st = server.state().clone();
    let mut events = st.phone.subscribe();
    let (mut stream, id) = live_session(&server, &mut ctl, &phone, true).await;

    // Waiting -> Accepted -> Active.
    loop {
        if let PhoneEvent::Active { info } = next_phone_event(&mut events).await {
            assert_eq!((info.width, info.height, info.control), (720, 1600, true));
            break;
        }
    }
    assert!(audit_has(&server, "phone_session_start").await);
    let idr = ctl.event("phone.idr").await;
    assert_eq!(idr, json!({}));

    // Phone -> PC: delta frames before the first keyframe are useless and dropped.
    let delta = encode_frame(KIND_VIDEO, 1, &[0, 0, 1, 0x41]);
    let key = encode_frame(KIND_VIDEO_KEY, 2, &[0, 0, 1, 0x67, 0, 0, 1, 0x65]);
    let delta2 = encode_frame(KIND_VIDEO, 3, &[0, 0, 1, 0x41, 9]);
    stream.send(Message::binary(delta.clone())).await.unwrap();
    stream.send(Message::binary(key.clone())).await.unwrap();
    stream.send(Message::binary(delta2.clone())).await.unwrap();
    let first = tokio::time::timeout(Duration::from_secs(5), st.phone.next_frame()).await.unwrap();
    assert_eq!(first, key);
    let second = tokio::time::timeout(Duration::from_secs(5), st.phone.next_frame()).await.unwrap();
    assert_eq!(second, delta2);

    // A fragmented keyframe arrives whole.
    let mut big = vec![0x55u8; 1_500_000];
    big[..4].copy_from_slice(&[0, 0, 0, 1]);
    let (a, b) = big.split_at(900_000);
    stream.send(Message::binary(encode_frame(0x06, 7, a))).await.unwrap();
    stream.send(Message::binary(encode_frame(KIND_VIDEO_KEY, 7, b))).await.unwrap();
    let joined = tokio::time::timeout(Duration::from_secs(5), st.phone.next_frame()).await.unwrap();
    assert_eq!(joined.len(), 9 + big.len());
    assert_eq!(joined[0], KIND_VIDEO_KEY);
    assert_eq!(&joined[9..], &big[..]);

    // PC -> phone: normalised input as `phone.in` events on the stream socket.
    st.phone.input(PhoneInput::Tap { x: 0.5, y: 2.0 }).unwrap();
    st.phone.input(PhoneInput::Global { action: GlobalAction::Back }).unwrap();
    st.phone.input(PhoneInput::Text { text: "héllo".into() }).unwrap();
    st.phone.input(PhoneInput::Key { key: EditKey::Enter }).unwrap();
    let want = [
        json!({"k":"tap","x":0.5,"y":1.0}),
        json!({"k":"global","action":"back"}),
        json!({"k":"text","text":"héllo"}),
        json!({"k":"key","key":"enter"}),
    ];
    for w in want {
        let m = recv(&mut stream).await;
        assert_eq!(m["type"], "event");
        assert_eq!(m["name"], "phone.in");
        assert_eq!(m["data"], w);
    }

    // Rotation and secure state reach the desktop app.
    ctl.ok(
        "phone.state",
        json!({"request_id": id, "width": 1600, "height": 720, "rotation": 90, "secure": "password"}),
    )
    .await;
    loop {
        if let PhoneEvent::State { info } = next_phone_event(&mut events).await {
            assert_eq!((info.width, info.height, info.rotation), (1600, 720, 90));
            assert_eq!(info.secure.as_deref(), Some("password"));
            break;
        }
    }

    // Stop on the phone: audited with its reason; no more input.
    ctl.ok("phone.stop", json!({"request_id": id, "reason": "screen_locked"})).await;
    assert_eq!(wait_ended(&mut events).await, ("screen_locked".to_string(), true));
    assert!(audit_has(&server, "phone_session_stop").await);
    assert!(audit_has(&server, "reason=screen_locked").await);
    assert_eq!(st.phone.input(PhoneInput::Tap { x: 0.0, y: 0.0 }).unwrap_err().code, "not_allowed");
    server.shutdown().await;
}

#[tokio::test]
async fn pc_events_are_dropped_without_a_session_or_a_grant() {
    let dir = tempfile::tempdir().unwrap();
    let server = start(dir.path()).await;
    let (mut ctl, phone) = paired(&server).await;
    let st = server.state().clone();
    let tap = || PhoneInput::Tap { x: 0.1, y: 0.1 };

    // No session.
    assert_eq!(st.phone.input(tap()).unwrap_err().code, "not_allowed");

    // View only: the picture flows, input does not.
    let mut events = st.phone.subscribe();
    let (mut stream, id) = live_session(&server, &mut ctl, &phone, false).await;
    loop {
        if matches!(next_phone_event(&mut events).await, PhoneEvent::Active { .. }) {
            break;
        }
    }
    assert_eq!(st.phone.input(tap()).unwrap_err().code, "not_allowed");
    let key = encode_frame(KIND_VIDEO_KEY, 1, &[0, 0, 1, 0x65]);
    stream.send(Message::binary(key.clone())).await.unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), st.phone.next_frame()).await.unwrap(),
        key
    );
    // The phone grants control later (the service was switched on): now it goes through.
    ctl.ok("phone.state", json!({"request_id": id, "control": true, "accessibility": true})).await;
    st.phone.input(tap()).unwrap();
    assert_eq!(recv(&mut stream).await["data"]["k"], "tap");
    // And takes it back.
    ctl.ok("phone.state", json!({"request_id": id, "control": false})).await;
    assert_eq!(st.phone.input(tap()).unwrap_err().code, "not_allowed");
    server.shutdown().await;
}

#[tokio::test]
async fn backpressure_drops_non_key_frames_and_asks_for_a_keyframe() {
    let dir = tempfile::tempdir().unwrap();
    let server = start(dir.path()).await;
    let (mut ctl, phone) = paired(&server).await;
    let st = server.state().clone();
    let mut events = st.phone.subscribe();
    let (mut stream, _id) = live_session(&server, &mut ctl, &phone, true).await;
    loop {
        if matches!(next_phone_event(&mut events).await, PhoneEvent::Active { .. }) {
            break;
        }
    }
    ctl.event("phone.idr").await; // the one sent on open

    // Nobody reads next_frame: only 3 fit.
    stream.send(Message::binary(encode_frame(KIND_VIDEO_KEY, 1, &[1]))).await.unwrap();
    for pts in 2..8u64 {
        stream.send(Message::binary(encode_frame(KIND_VIDEO, pts, &[pts as u8]))).await.unwrap();
    }
    stream.flush().await.unwrap();
    for _ in 0..100 {
        if st.phone.dropped_frames() >= 4 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(st.phone.dropped_frames(), 4);
    let drain = |n: u64| {
        let st = st.clone();
        async move {
            let f = tokio::time::timeout(Duration::from_secs(5), st.phone.next_frame()).await.unwrap();
            assert_eq!(f[8] as u64, n, "pts low byte");
        }
    };
    drain(1).await;
    drain(2).await;
    drain(3).await;
    // Drained after drops: the phone is asked for a keyframe.
    assert_eq!(ctl.event("phone.idr").await, json!({}));
    server.shutdown().await;
}

#[tokio::test]
async fn disconnect_ends_the_session_and_tokens_are_bound_and_single_use() {
    let dir = tempfile::tempdir().unwrap();
    let server = start(dir.path()).await;
    let (mut ctl, phone) = paired(&server).await;
    let st = server.state().clone();
    let mut events = st.phone.subscribe();
    ctl.ok("phone.caps", json!({"enabled": true, "accessibility": true})).await;
    let info = st.phone.request(&st, &phone.device_id, RequestOptions::default()).unwrap();
    let r = ctl
        .ok(
            "phone.accept",
            json!({"request_id": info.id, "width": 720, "height": 1600, "control": true}),
        )
        .await;
    let token = r["token"].as_str().unwrap().to_string();

    // Bad tokens are refused and do not affect a session.
    let mut bad = open_stream(&server, &phone, "nope").await;
    assert_eq!(recv(&mut bad).await["code"], "bad_token");

    // A different paired phone cannot use this one's token (it burns it).
    let other = Phone::new("Other");
    let pairing = server.new_pairing().token;
    let (_other_ctl, ready) = other.pair(&server, &pairing).await;
    assert_eq!(ready["type"], "ready");
    let mut stolen = open_stream(&server, &other, &token).await;
    assert_eq!(recv(&mut stolen).await["code"], "bad_token");
    let mut late = open_stream(&server, &phone, &token).await;
    assert_eq!(recv(&mut late).await["code"], "bad_token");

    // Control socket gone: session over, audited as a request that never went live.
    ctl.ws.close(None).await.unwrap();
    assert_eq!(wait_ended(&mut events).await, ("disconnected".to_string(), false));
    assert!(st.phone.targets().iter().all(|t| t.device_id != phone.device_id));
    server.shutdown().await;
}

#[tokio::test]
async fn stream_socket_loss_stops_a_live_session_and_pc_stop_tells_the_phone() {
    let dir = tempfile::tempdir().unwrap();
    let server = start(dir.path()).await;
    let (mut ctl, phone) = paired(&server).await;
    let st = server.state().clone();
    let mut events = st.phone.subscribe();

    let (mut stream, id) = live_session(&server, &mut ctl, &phone, true).await;
    loop {
        if matches!(next_phone_event(&mut events).await, PhoneEvent::Active { .. }) {
            break;
        }
    }
    stream.close(None).await.unwrap();
    assert_eq!(wait_ended(&mut events).await, ("disconnected".to_string(), true));
    assert!(audit_has(&server, &format!("id={id} reason=disconnected")).await);

    // New session; the PC ends it: the phone hears `phone.cancel` and the socket closes.
    let (mut stream, id) = live_session(&server, &mut ctl, &phone, true).await;
    loop {
        if matches!(next_phone_event(&mut events).await, PhoneEvent::Active { .. }) {
            break;
        }
    }
    st.phone.stop(&st);
    assert_eq!(wait_ended(&mut events).await, ("stopped_by_pc".to_string(), true));
    let cancel = ctl.event("phone.cancel").await;
    assert_eq!(cancel["request_id"], id);
    assert_eq!(cancel["reason"], "stopped_by_pc");
    expect_closed(&mut stream).await;
    server.shutdown().await;
}

#[tokio::test]
async fn a_phone_cannot_answer_for_another_phones_request() {
    let dir = tempfile::tempdir().unwrap();
    let server = start(dir.path()).await;
    let (mut ctl, phone) = paired(&server).await;
    let st = server.state().clone();
    ctl.ok("phone.caps", json!({"enabled": true, "accessibility": true})).await;
    let info = st.phone.request(&st, &phone.device_id, RequestOptions::default()).unwrap();

    let other = Phone::new("Other");
    let pairing = server.new_pairing().token;
    let (ws, _) = other.pair(&server, &pairing).await;
    let mut other_ctl = Control { ws, next: 0 };
    for (method, params) in [
        ("phone.accept", json!({"request_id": info.id, "width": 720, "height": 1600, "control": true})),
        ("phone.decline", json!({"request_id": info.id, "reason": "declined"})),
        ("phone.stop", json!({"request_id": info.id})),
    ] {
        assert_eq!(other_ctl.call(method, params).await["ok"], false, "{method}");
    }
    assert!(st.phone.current().is_some());
    server.shutdown().await;
}
