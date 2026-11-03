//! Phase 4 end to end: the agent's relay client against an in-process fake relay that
//! implements tunnel protocol v1 (relay/README.md). The phone side runs the normal pinned
//! TLS + `/ws` session over a relay channel. Nothing leaves 127.0.0.1.

mod common;

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::*;
use futures_util::{SinkExt, StreamExt};
use helm_agent::relay::{FRAME_CLOSE, FRAME_DATA, FRAME_OPEN, decode_frame, encode_frame};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

enum Cmd {
    Open(u32, DuplexStream),
    Kick,
}

/// A fake relay: one host socket at a time, challenge/auth/ready, then the mux.
struct FakeRelay {
    port: u16,
    auths: Arc<AtomicUsize>,
    host_ids: Arc<Mutex<Vec<String>>>,
    host: Arc<Mutex<Option<mpsc::UnboundedSender<Cmd>>>>,
    next_ch: AtomicUsize,
}

impl FakeRelay {
    async fn start() -> FakeRelay {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let auths = Arc::new(AtomicUsize::new(0));
        let host_ids = Arc::new(Mutex::new(Vec::new()));
        let host: Arc<Mutex<Option<mpsc::UnboundedSender<Cmd>>>> = Arc::new(Mutex::new(None));
        let (a, h, ids) = (auths.clone(), host.clone(), host_ids.clone());
        tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                tokio::spawn(host_conn(tcp, a.clone(), h.clone(), ids.clone()));
            }
        });
        FakeRelay {
            port,
            auths,
            host_ids,
            host,
            next_ch: AtomicUsize::new(1),
        }
    }

    fn url(&self) -> String {
        format!("ws://127.0.0.1:{}", self.port)
    }

    fn connected(&self) -> bool {
        self.host
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|t| !t.is_closed())
    }

    /// Like a phone hitting `/connect/<host_id>`: returns the phone end of the byte stream.
    fn open(&self) -> DuplexStream {
        let ch = self.next_ch.fetch_add(1, Ordering::SeqCst) as u32;
        let (phone, relay_side) = tokio::io::duplex(256 * 1024);
        let tx = self.host.lock().unwrap().clone().expect("host connected");
        assert!(tx.send(Cmd::Open(ch, relay_side)).is_ok(), "host alive");
        phone
    }

    fn kick(&self) {
        if let Some(tx) = self.host.lock().unwrap().take() {
            let _ = tx.send(Cmd::Kick);
        }
    }
}

#[allow(clippy::result_large_err)] // the callback type is fixed by tungstenite
async fn host_conn(
    tcp: tokio::net::TcpStream,
    auths: Arc<AtomicUsize>,
    host: Arc<Mutex<Option<mpsc::UnboundedSender<Cmd>>>>,
    host_ids: Arc<Mutex<Vec<String>>>,
) {
    let path = Arc::new(Mutex::new(String::new()));
    let p = path.clone();
    let mut ws = tokio_tungstenite::accept_hdr_async(
        tcp,
        move |req: &tokio_tungstenite::tungstenite::handshake::server::Request, resp| {
            *p.lock().unwrap() = req.uri().path().to_string();
            Ok(resp)
        },
    )
    .await
    .unwrap();
    let path = path.lock().unwrap().clone();
    let host_id = path.strip_prefix("/host/").expect("host path").to_string();
    // 1.1 challenge
    let nonce = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";
    ws.send(Message::text(
        json!({"type":"challenge","v":1,"nonce":nonce}).to_string(),
    ))
    .await
    .unwrap();
    // 1.2 auth
    let auth: Value = match ws.next().await {
        Some(Ok(Message::Text(t))) => serde_json::from_str(t.as_str()).unwrap(),
        other => panic!("expected auth, got {other:?}"),
    };
    assert_eq!(auth["type"], "auth");
    let key = helm_agent::proto::parse_public_key(auth["public_key"].as_str().unwrap())
        .expect("P-256 SPKI");
    let signed = format!("helm-relay-v1|host|{host_id}|{nonce}");
    assert!(helm_agent::proto::verify_signature(
        &key,
        &signed,
        auth["signature"].as_str().unwrap()
    ));
    auths.fetch_add(1, Ordering::SeqCst);
    host_ids.lock().unwrap().push(host_id);
    ws.send(Message::text(
        json!({"type":"ready","v":1,"registered":true,"max_phones":4,"max_frame":65536})
            .to_string(),
    ))
    .await
    .unwrap();

    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel();
    // A newer host replaces the old one (like the real relay).
    *host.lock().unwrap() = Some(cmd_tx);
    let (frames_tx, mut frames_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let mut to_phone: HashMap<u32, mpsc::UnboundedSender<Vec<u8>>> = HashMap::new();
    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => match cmd {
                Some(Cmd::Open(ch, io)) => {
                    let (mut rd, mut wr) = tokio::io::split(io);
                    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
                    to_phone.insert(ch, tx);
                    tokio::spawn(async move {
                        while let Some(d) = rx.recv().await {
                            if wr.write_all(&d).await.is_err() { break; }
                        }
                        let _ = wr.shutdown().await;
                    });
                    ws.send(Message::binary(encode_frame(FRAME_OPEN, ch, &[]))).await.unwrap();
                    let ftx = frames_tx.clone();
                    tokio::spawn(async move {
                        // Odd chunk sizes: tunnel boundaries carry no meaning.
                        let mut buf = vec![0u8; 777];
                        loop {
                            match rd.read(&mut buf).await {
                                Ok(0) | Err(_) => break,
                                Ok(n) => { let _ = ftx.send(encode_frame(FRAME_DATA, ch, &buf[..n])); }
                            }
                        }
                        let _ = ftx.send(encode_frame(FRAME_CLOSE, ch, &[]));
                    });
                }
                Some(Cmd::Kick) | None => {
                    let _ = ws.close(None).await;
                    return;
                }
            },
            Some(f) = frames_rx.recv() => {
                if ws.send(Message::binary(f)).await.is_err() { return; }
            }
            msg = ws.next() => match msg {
                Some(Ok(Message::Binary(b))) => {
                    let (kind, ch, payload) = decode_frame(&b).expect("valid frame from host");
                    match kind {
                        FRAME_DATA => if let Some(tx) = to_phone.get(&ch) { let _ = tx.send(payload.to_vec()); },
                        FRAME_CLOSE => { to_phone.remove(&ch); }
                        other => panic!("host sent frame type {other}"),
                    }
                }
                Some(Ok(Message::Text(t))) => {
                    assert_eq!(t.as_str(), "ping");
                    let _ = ws.send(Message::text("pong")).await;
                }
                Some(Ok(_)) => {}
                _ => return,
            }
        }
    }
}

type RelayWs = WebSocketStream<TlsStream<DuplexStream>>;

/// Phone side over a relay channel: pinned TLS, then `GET /ws`.
async fn phone_over_relay(relay: &FakeRelay, fingerprint: &str, host_id: &str) -> RelayWs {
    let io = relay.open();
    let tls = TlsConnector::from(Arc::new(pinned_client_config(fingerprint)))
        .connect(
            rustls::pki_types::ServerName::try_from("helm-agent.local").unwrap(),
            io,
        )
        .await
        .expect("pinned TLS through the tunnel");
    let (ws, _) =
        tokio_tungstenite::client_async(format!("wss://{host_id}.relay.helm.invalid/ws"), tls)
            .await
            .expect("websocket through the tunnel");
    ws
}

async fn rx(ws: &mut RelayWs) -> Value {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("timed out")
            .expect("closed")
            .expect("ws error");
        match msg {
            Message::Text(t) => return serde_json::from_str(t.as_str()).unwrap(),
            Message::Close(f) => panic!("unexpected close {f:?}"),
            _ => {}
        }
    }
}

async fn tx(ws: &mut RelayWs, v: Value) {
    ws.send(Message::text(v.to_string())).await.unwrap();
}

async fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    for _ in 0..200 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for {what}");
}

#[tokio::test]
async fn relay_tunnel_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let relay = FakeRelay::start().await;
    let (server, fake) = start_fake_with(dir.path(), |_| {}).await;

    // Pair on the LAN, then turn the relay on through the protocol.
    let phone = Phone::new("Pixel");
    let (mut lan, ready) = phone.pair(&server, &server.new_pairing().token).await;
    assert_eq!(ready["type"], "ready");
    let r = call(&mut lan, 1, "relay.get", json!({})).await;
    assert_eq!(r["result"]["enabled"], false);
    let r = call(
        &mut lan,
        2,
        "relay.set",
        json!({"enabled": true, "url": "ws://relay.example.com"}),
    )
    .await;
    assert_eq!(
        r["error"]["code"], "bad_params",
        "plain ws only to loopback"
    );
    let r = call(
        &mut lan,
        3,
        "relay.set",
        json!({"enabled": true, "url": relay.url()}),
    )
    .await;
    assert_eq!(r["ok"], true, "{r}");
    wait_until("relay auth", || relay.connected()).await;
    assert_eq!(relay.host_ids.lock().unwrap()[0], server.host_id());
    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("relay.json")).unwrap())
            .unwrap();
    assert_eq!(saved, json!({"enabled": true, "url": relay.url()}));
    let mut connected = false;
    for i in 0..50 {
        let r = call(&mut lan, 10 + i, "relay.get", json!({})).await;
        if r["result"]["connected"] == true {
            connected = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(connected);

    // A paired phone authenticates through the tunnel and uses the normal protocol.
    let mut ws = phone_over_relay(&relay, server.fingerprint(), server.host_id()).await;
    let hello = rx(&mut ws).await;
    assert_eq!(hello["type"], "hello");
    assert!(hello["features"].as_array().unwrap().contains(&json!("p4")));
    tx(&mut ws, phone.auth_msg(&hello)).await;
    assert_eq!(rx(&mut ws).await["type"], "ready");
    tx(
        &mut ws,
        json!({"type":"req","id":1,"method":"system.info","params":{}}),
    )
    .await;
    let info = rx(&mut ws).await;
    assert_eq!(
        info["result"]["relay"],
        json!({"enabled": true, "url": relay.url()})
    );
    // Bigger payloads cross many tunnel chunks.
    let big = "x".repeat(300_000);
    tx(
        &mut ws,
        json!({"type":"req","id":2,"method":"clipboard.set","params":{"text": big}}),
    )
    .await;
    assert_eq!(rx(&mut ws).await["ok"], true);
    tx(
        &mut ws,
        json!({"type":"req","id":3,"method":"clipboard.get","params":{}}),
    )
    .await;
    assert_eq!(
        rx(&mut ws).await["result"]["text"].as_str().unwrap().len(),
        300_000
    );
    // Input incl. the gyro air-mouse kind.
    tx(&mut ws, json!({"type":"in","k":"gy","dx":4,"dy":-3})).await;
    wait_until("gy input", || !fake.ops().is_empty()).await;

    let audit = std::fs::read_to_string(dir.path().join("audit.log")).unwrap();
    assert!(
        audit.contains("auth_ok") && audit.contains("via=relay ch="),
        "{audit}"
    );

    // The relay drops the host: the agent reconnects with backoff (1 s ± jitter).
    drop(ws);
    relay.kick();
    wait_until("relay reconnect", || {
        relay.auths.load(Ordering::SeqCst) >= 2
    })
    .await;
    wait_until("host socket back", || relay.connected()).await;
    let mut ws3 = phone_over_relay(&relay, server.fingerprint(), server.host_id()).await;
    let hello3 = rx(&mut ws3).await;
    tx(&mut ws3, phone.auth_msg(&hello3)).await;
    assert_eq!(rx(&mut ws3).await["type"], "ready");
    drop(ws3);

    // Disabling closes the host socket.
    let r = call(&mut lan, 100, "relay.set", json!({"enabled": false})).await;
    assert_eq!(r["result"]["enabled"], false);
    wait_until("relay disconnect", || !relay.connected()).await;
    drop(lan);
    server.shutdown().await;
}

/// A phone answering `hello` over a fresh relay channel with one `pair` message.
async fn pair_over_relay(
    relay: &FakeRelay,
    server: &helm_agent::ServerHandle,
    phone: &Phone,
    token: &str,
) -> (RelayWs, Value) {
    let mut ws = phone_over_relay(relay, server.fingerprint(), server.host_id()).await;
    let hello = rx(&mut ws).await;
    tx(&mut ws, phone.pair_msg(&hello, token)).await;
    let reply = rx(&mut ws).await;
    (ws, reply)
}

#[tokio::test]
async fn qr_token_pairs_over_the_relay_and_the_qr_names_the_relay() {
    let dir = tempfile::tempdir().unwrap();
    let relay = FakeRelay::start().await;
    let (server, _) = start_fake_with(dir.path(), |_| {}).await;
    assert!(
        !server.new_pairing().uri.contains("&r="),
        "no relay in the QR while it is off"
    );
    server
        .state()
        .relay
        .set(Some(true), Some(&relay.url()))
        .unwrap();
    wait_until("relay auth", || relay.connected()).await;

    let pairing = server.new_pairing();
    let encoded = relay.url().replace(':', "%3A").replace('/', "%2F");
    assert!(
        pairing.uri.ends_with(&format!("&r={encoded}")),
        "{}",
        pairing.uri
    );

    let phone = Phone::new("Away");
    let (ws, reply) = pair_over_relay(&relay, &server, &phone, &pairing.token).await;
    assert_eq!(reply["type"], "ready", "{reply}");
    assert_eq!(reply["tier"], "admin");
    assert!(
        server
            .devices()
            .unwrap()
            .iter()
            .any(|d| d.device_name == "Away")
    );
    let audit = std::fs::read_to_string(dir.path().join("audit.log")).unwrap();
    assert!(
        audit
            .lines()
            .any(|l| l.contains("pair_ok") && l.contains("via=relay")),
        "{audit}"
    );

    // Single use, over the relay as well.
    let stranger = Phone::new("Second");
    let (_ws2, reply) = pair_over_relay(&relay, &server, &stranger, &pairing.token).await;
    assert_eq!(reply["code"], "bad_token", "{reply}");
    drop(ws);

    // Turning the relay off takes it out of the next QR.
    server.state().relay.set(Some(false), None).unwrap();
    assert!(!server.new_pairing().uri.contains("&r="));
    server.shutdown().await;
}

#[tokio::test]
async fn asking_before_pairing_applies_over_the_relay_and_says_where_from() {
    let dir = tempfile::tempdir().unwrap();
    let relay = FakeRelay::start().await;
    let (server, fake) = start_fake_with(dir.path(), |_| {}).await;
    server
        .state()
        .relay
        .set(Some(true), Some(&relay.url()))
        .unwrap();
    wait_until("relay auth", || relay.connected()).await;
    server.state().g4.set_require_approval(true).unwrap();
    let phone = Phone::new("Away");
    let token = server.new_pairing().token;
    let (mut ws, reply) = pair_over_relay(&relay, &server, &phone, &token).await;
    assert_eq!(reply["type"], "pair_pending", "{reply}");
    assert!(server.devices().unwrap().is_empty(), "nothing stored yet");
    wait_until("toast", || !fake.toasts().is_empty()).await;
    assert!(
        fake.toasts()[0].body.contains("through the relay"),
        "{}",
        fake.toasts()[0].body
    );
    assert!(fake.click_toast(0, Some("allow"), None));
    assert_eq!(rx(&mut ws).await["type"], "ready");
    assert_eq!(server.devices().unwrap().len(), 1);
    server.shutdown().await;
}

#[tokio::test]
async fn bad_tokens_over_the_relay_hit_the_same_lockout_as_the_lan() {
    let dir = tempfile::tempdir().unwrap();
    let relay = FakeRelay::start().await;
    let (server, _) = start_fake_with(dir.path(), |_| {}).await;
    server
        .state()
        .relay
        .set(Some(true), Some(&relay.url()))
        .unwrap();
    wait_until("relay auth", || relay.connected()).await;
    let good = server.new_pairing().token;
    let attacker = Phone::new("Attacker");
    for _ in 0..helm_agent::pairing::MAX_BAD_TOKENS {
        let (_ws, reply) = pair_over_relay(&relay, &server, &attacker, "wrong").await;
        assert_eq!(reply["code"], "bad_token", "{reply}");
    }
    // Locked now: not even the right token gets in, whichever path it comes from.
    let (_ws, reply) = pair_over_relay(&relay, &server, &attacker, &good).await;
    assert_eq!(reply["code"], "pairing_locked", "{reply}");
    let mut lan = connect(&server).await;
    let hello = recv(&mut lan).await;
    send(&mut lan, Phone::new("Lan").pair_msg(&hello, &good)).await;
    assert_eq!(recv(&mut lan).await["code"], "pairing_locked");
    assert!(server.devices().unwrap().is_empty());
    server.shutdown().await;
}

#[tokio::test]
async fn six_digit_code_pairing_stays_refused_over_the_relay() {
    let dir = tempfile::tempdir().unwrap();
    let relay = FakeRelay::start().await;
    let (server, _) = start_fake_with(dir.path(), |_| {}).await;
    server
        .state()
        .relay
        .set(Some(true), Some(&relay.url()))
        .unwrap();
    wait_until("relay auth", || relay.connected()).await;
    server.new_code_pairing().unwrap();
    let phone = Phone::new("Code");
    let mut ws = phone_over_relay(&relay, server.fingerprint(), server.host_id()).await;
    let hello = rx(&mut ws).await;
    let nonce = hello["nonce"].as_str().unwrap();
    tx(
        &mut ws,
        json!({
            "type": "pair_code", "device_id": phone.device_id, "device_name": "Code",
            "public_key": phone.public_key_b64(),
            "nonce": helm_agent::util::b64_encode(&[7u8; 32]),
            "commits": vec![helm_agent::util::b64_encode(&[1u8; 32]); 6],
            "signature": phone.sign("pair_code", server.host_id(), nonce),
        }),
    )
    .await;
    let err = rx(&mut ws).await;
    assert_eq!(err["type"], "error");
    assert_eq!(err["code"], "bad_message", "{err}");
    assert!(err["message"].as_str().unwrap().contains("local network"));
    let audit = std::fs::read_to_string(dir.path().join("audit.log")).unwrap();
    assert!(
        audit
            .lines()
            .any(|l| l.contains("pair_fail") && l.contains("relayed")),
        "{audit}"
    );
    assert!(server.devices().unwrap().is_empty());
    server.shutdown().await;
}

#[tokio::test]
async fn relay_status_reports_the_reason_and_recovers() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _) = start_fake_with(dir.path(), |_| {}).await;
    // A port nobody listens on: "not connected", with a reason.
    let dead = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead_url = format!("ws://127.0.0.1:{}", dead.local_addr().unwrap().port());
    drop(dead);
    let st = server.state().clone();
    st.relay.set(Some(true), Some(&dead_url)).unwrap();
    wait_until("relay error", || st.relay.status().last_error.is_some()).await;
    assert!(!st.relay.status().connected);
    // A good address clears the error, then connects.
    let relay = FakeRelay::start().await;
    st.relay.set(None, Some(&relay.url())).unwrap();
    wait_until("relay up", || st.relay.status().connected).await;
    assert_eq!(st.relay.status().last_error, None);
    // Off: neither connected nor in error.
    st.relay.set(Some(false), None).unwrap();
    wait_until("relay off", || !st.relay.status().connected).await;
    assert_eq!(st.relay.status().last_error, None);
    server.shutdown().await;
}

#[tokio::test]
async fn relay_settings_survive_restart_and_shutdown_is_clean() {
    let dir = tempfile::tempdir().unwrap();
    let relay = FakeRelay::start().await;
    std::fs::write(
        dir.path().join("relay.json"),
        json!({"enabled": true, "url": relay.url()}).to_string(),
    )
    .unwrap();
    let (server, _) = start_fake_with(dir.path(), |_| {}).await;
    wait_until("relay auth", || relay.connected()).await;
    let phone = Phone::new("P");
    let (lan, _) = phone.pair(&server, &server.new_pairing().token).await;
    let mut ws = phone_over_relay(&relay, server.fingerprint(), server.host_id()).await;
    let hello = rx(&mut ws).await;
    tx(&mut ws, phone.auth_msg(&hello)).await;
    assert_eq!(rx(&mut ws).await["type"], "ready");
    drop(lan);
    tokio::time::timeout(Duration::from_secs(10), server.shutdown())
        .await
        .expect("shutdown with a relayed session finishes");
    // The relayed session got a proper close.
    let closed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match ws.next().await {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return,
                _ => {}
            }
        }
    })
    .await;
    assert!(closed.is_ok());
}

#[tokio::test]
async fn wol_send_and_relay_tiers() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |_| {}).await;
    let phone = Phone::new("P");
    let (mut ws, _) = phone.pair(&server, &server.new_pairing().token).await;
    let r = call(&mut ws, 1, "wol.send", json!({"mac": "aa-bb-cc-dd-ee-ff"})).await;
    assert_eq!(
        r["result"],
        json!({"sent": true, "mac": "AA:BB:CC:DD:EE:FF"})
    );
    assert!(fake.calls().contains(&"send_wol:aabbccddeeff".to_string()));
    let r = call(&mut ws, 2, "wol.send", json!({"mac": "nope"})).await;
    assert_eq!(r["error"]["code"], "bad_params");
    let r = call(&mut ws, 3, "wol.send", json!({})).await;
    assert_eq!(r["error"]["code"], "bad_params");
    let r = call(&mut ws, 4, "relay.set", json!({"enabled": true})).await;
    assert_eq!(r["error"]["code"], "bad_params", "no url configured");
    let r = call(&mut ws, 5, "system.info", json!({})).await;
    assert_eq!(r["result"]["relay"], json!({"enabled": false, "url": ""}));
    // gy is handled exactly like mm.
    send(&mut ws, json!({"type":"in","k":"gy","dx":2,"dy":1})).await;
    for _ in 0..100 {
        if !fake.ops().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let gy_ops = fake.ops();
    assert!(!gy_ops.is_empty());
    send(&mut ws, json!({"type":"in","k":"mm","dx":2,"dy":1})).await;
    for _ in 0..100 {
        if fake.ops().len() >= gy_ops.len() * 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let all = fake.ops();
    assert_eq!(&all[gy_ops.len()..], &gy_ops[..]);
    server.shutdown().await;
}
