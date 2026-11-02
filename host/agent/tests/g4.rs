//! G4 (Docs/protocol/pairing-security.md): short-code pairing, approve on PC, settings.
//! Real TLS + WebSocket against an in-process agent on the fake platform (no real toasts).

mod common;

use std::time::Duration;

use common::*;
use helm_agent::g4::code::{self, Role, Transcript};
use helm_agent::g4::{HostEvent, Outcome};
use serde_json::{Value, json};

fn assert_error(msg: &Value, code: &str) {
    assert_eq!(msg["type"], "error", "expected error, got {msg}");
    assert_eq!(msg["code"], code, "got {msg}");
}

fn b64(bytes: &[u8]) -> String {
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes)
}

fn unb64(text: &str) -> Vec<u8> {
    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, text).unwrap()
}

fn rand32() -> Vec<u8> {
    (0..32).map(|_| rand::random::<u8>()).collect()
}

#[derive(Debug)]
#[allow(clippy::large_enum_variant)] // short-lived test helper
enum CodeResult {
    /// The agent's last message (`ready`, `error` or `pair_pending`) and the socket.
    Agent(Value, Ws),
    /// The phone found an agent opening that does not match (wrong code / man in the middle).
    PhoneRejected(u64),
}

/// The phone side of code pairing. `fp_seen` = the certificate fingerprint the phone saw
/// (a man in the middle would present its own). `verify = false` = a phone that skips its
/// checks (to test that the agent catches a mismatch on its own).
async fn pair_code(
    server: &helm_agent::ServerHandle,
    phone: &Phone,
    code_text: &str,
    fp_seen: &str,
    verify: bool,
) -> CodeResult {
    let digits = code::parse_code(code_text).expect("6 digits");
    let mut ws = connect(server).await;
    let hello = recv(&mut ws).await;
    assert!(hello["features"].as_array().unwrap().contains(&json!("g4")));
    let host_id = hello["host_id"].as_str().unwrap().to_string();
    let nonce_a = hello["nonce"].as_str().unwrap().to_string();
    let nonce_p = b64(&rand32());
    let pk = phone.public_key_b64();
    let h = code::transcript_hash(&Transcript {
        host_id: &host_id,
        nonce_a: &nonce_a,
        nonce_p: &nonce_p,
        fingerprint: fp_seen,
        device_id: &phone.device_id,
        public_key: &pk,
    });
    let r_p: Vec<Vec<u8>> = (0..6).map(|_| rand32()).collect();
    let commits: Vec<String> = (0..6)
        .map(|i| b64(&code::commitment(Role::Phone, i + 1, digits[i], &h, &r_p[i])))
        .collect();
    send(
        &mut ws,
        json!({
            "type": "pair_code",
            "device_id": phone.device_id,
            "device_name": phone.name,
            "public_key": pk,
            "nonce": nonce_p,
            "commits": commits,
            "signature": phone.sign("pair_code", &host_id, &nonce_a),
        }),
    )
    .await;
    let msg = recv(&mut ws).await;
    if msg["type"] != "code_commit" {
        return CodeResult::Agent(msg, ws);
    }
    let commits_a: Vec<Vec<u8>> = msg["commits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| unb64(c.as_str().unwrap()))
        .collect();
    assert_eq!(commits_a.len(), 6);
    for i in 0..6 {
        let open = recv(&mut ws).await;
        if open["type"] != "code_open" {
            return CodeResult::Agent(open, ws);
        }
        assert_eq!(open["i"], i as u64 + 1);
        let r_a = unb64(open["r"].as_str().unwrap());
        if verify
            && !code::opening_matches(&commits_a[i], Role::Agent, i + 1, digits[i], &h, &r_a)
        {
            send(&mut ws, json!({"type": "error", "code": "bad_code", "message": "mismatch"}))
                .await;
            let reply = recv(&mut ws).await;
            assert_error(&reply, "bad_code");
            expect_closed(&mut ws).await;
            return CodeResult::PhoneRejected(i as u64 + 1);
        }
        send(&mut ws, json!({"type": "code_open", "i": i + 1, "r": b64(&r_p[i])})).await;
    }
    let last = recv(&mut ws).await;
    CodeResult::Agent(last, ws)
}

fn agent_reply(r: CodeResult) -> (Value, Ws) {
    match r {
        CodeResult::Agent(v, ws) => (v, ws),
        other => panic!("expected an agent reply, got {other:?}"),
    }
}

/// A code with the digit at `pos` changed.
fn wrong(code: &str, pos: usize) -> String {
    code.char_indices()
        .map(|(i, c)| {
            if i == pos {
                char::from_digit((c.to_digit(10).unwrap() + 1) % 10, 10).unwrap()
            } else {
                c
            }
        })
        .collect()
}

async fn next_event(rx: &mut tokio::sync::broadcast::Receiver<HostEvent>) -> HostEvent {
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("host event")
        .expect("event channel")
}

async fn wait_toasts(fake: &FakePlatform, n: usize) {
    for _ in 0..200 {
        if fake.toasts().len() >= n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("toast {n} was not shown");
}

#[tokio::test]
async fn code_pairing_happy_path_then_auth() {
    let dir = tempfile::tempdir().unwrap();
    let server = start(dir.path()).await;
    let mut events = server.state().g4.subscribe();
    let shown = server.new_code_pairing().unwrap();
    assert_eq!(shown.code.len(), 6);
    assert_eq!(shown.host_name, "TEST-PC");
    assert!(matches!(next_event(&mut events).await, HostEvent::PairingCode { code, .. } if code == shown.code));

    let phone = Phone::new("Pixel 8");
    let (ready, ws) = agent_reply(
        pair_code(&server, &phone, &code::display_code(&shown.code), server.fingerprint(), true).await,
    );
    assert_eq!(ready["type"], "ready", "{ready}");
    assert_eq!(ready["device_id"], phone.device_id.as_str());
    assert_eq!(ready["tier"], "admin");
    drop(ws);
    assert!(matches!(next_event(&mut events).await, HostEvent::PairingCodeEnded { reason } if reason == "used"));
    assert!(matches!(next_event(&mut events).await, HostEvent::Paired { method, .. } if method == "code"));
    assert_eq!(server.devices().unwrap().len(), 1);
    assert!(server.state().g4.current_code().is_none());

    let (_ws, ready) = phone.auth(&server).await;
    assert_eq!(ready["type"], "ready");
    let audit = std::fs::read_to_string(dir.path().join("audit.log")).unwrap();
    assert!(audit.contains("method=code"));
    server.shutdown().await;
}

#[tokio::test]
async fn wrong_code_is_caught_and_the_code_is_used_up() {
    let dir = tempfile::tempdir().unwrap();
    let server = start(dir.path()).await;
    let shown = server.new_code_pairing().unwrap();
    let phone = Phone::new("Pixel");
    // Digit 3 wrong: the phone sees the PC's opening of digit 3 not match and stops.
    let r = pair_code(&server, &phone, &wrong(&shown.code, 2), server.fingerprint(), true).await;
    assert!(matches!(r, CodeResult::PhoneRejected(3)), "{r:?}");
    // The real code no longer works: one attempt per code.
    let (reply, _ws) =
        agent_reply(pair_code(&server, &phone, &shown.code, server.fingerprint(), true).await);
    assert_error(&reply, "bad_code");
    assert!(server.devices().unwrap().is_empty());

    // A phone that skips its own checks is caught by the agent at the first wrong digit.
    let shown = server.new_code_pairing().unwrap();
    let (reply, _ws) = agent_reply(
        pair_code(&server, &phone, &wrong(&shown.code, 0), server.fingerprint(), false).await,
    );
    assert_error(&reply, "bad_code");
    assert!(server.devices().unwrap().is_empty());
    server.shutdown().await;
}

#[tokio::test]
async fn man_in_the_middle_certificate_is_detected_by_both_sides() {
    let dir = tempfile::tempdir().unwrap();
    let server = start(dir.path()).await;
    let phone = Phone::new("Pixel");
    let other_fp = "ee".repeat(32);
    // Correct code, but the phone saw another certificate: the PC's first opening fails.
    let shown = server.new_code_pairing().unwrap();
    let r = pair_code(&server, &phone, &shown.code, &other_fp, true).await;
    assert!(matches!(r, CodeResult::PhoneRejected(1)), "{r:?}");
    // And the agent rejects commitments made for another certificate on its own.
    let shown = server.new_code_pairing().unwrap();
    let (reply, _ws) =
        agent_reply(pair_code(&server, &phone, &shown.code, &other_fp, false).await);
    assert_error(&reply, "bad_code");
    assert!(server.devices().unwrap().is_empty());
    server.shutdown().await;
}

#[tokio::test]
async fn three_failures_lock_code_pairing() {
    let dir = tempfile::tempdir().unwrap();
    let server = start(dir.path()).await;
    let mut events = server.state().g4.subscribe();
    let phone = Phone::new("Attacker");
    for _ in 0..3 {
        let shown = server.new_code_pairing().unwrap();
        let _ = pair_code(&server, &phone, &wrong(&shown.code, 0), server.fingerprint(), false).await;
    }
    let mut locked = false;
    while let Ok(Ok(ev)) = tokio::time::timeout(Duration::from_millis(500), events.recv()).await {
        if matches!(ev, HostEvent::PairingLocked { .. }) {
            locked = true;
        }
    }
    assert!(locked, "PairingLocked event");
    assert!(server.new_code_pairing().is_err(), "no new code while locked");
    let (reply, _ws) =
        agent_reply(pair_code(&server, &phone, "000000", server.fingerprint(), true).await);
    assert_error(&reply, "pairing_locked");
    // QR pairing is separate and still works.
    let p = server.new_pairing();
    let (_ws, ready) = Phone::new("Owner").pair(&server, &p.token).await;
    assert_eq!(ready["type"], "ready");
    server.shutdown().await;
}

#[tokio::test]
async fn bad_signature_does_not_use_up_the_code() {
    let dir = tempfile::tempdir().unwrap();
    let server = start(dir.path()).await;
    let shown = server.new_code_pairing().unwrap();
    let phone = Phone::new("Pixel");
    let mut ws = connect(&server).await;
    let _hello = recv(&mut ws).await;
    send(
        &mut ws,
        json!({
            "type": "pair_code", "device_id": phone.device_id, "device_name": "x",
            "public_key": phone.public_key_b64(), "nonce": b64(&rand32()),
            "commits": vec![b64(&rand32()); 6], "signature": phone.sign("pair_code", "wrong", "n"),
        }),
    )
    .await;
    assert_error(&recv(&mut ws).await, "bad_signature");
    assert_eq!(server.state().g4.current_code().as_deref(), Some(shown.code.as_str()));
    let (ready, _ws) =
        agent_reply(pair_code(&server, &phone, &shown.code, server.fingerprint(), true).await);
    assert_eq!(ready["type"], "ready");
    server.shutdown().await;
}

#[tokio::test]
async fn approval_deny_allow_and_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let (server, fake) = start_fake_with(dir.path(), |c| {
        c.approval_timeout = Duration::from_millis(600);
    })
    .await;
    server.state().g4.set_require_approval(true).unwrap();
    let mut events = server.state().g4.subscribe();

    // Deny from the toast.
    let p = server.new_pairing();
    let phone = Phone::new("Pixel 8");
    let (mut ws, pending) = phone.pair(&server, &p.token).await;
    assert_eq!(pending["type"], "pair_pending");
    assert!(pending["timeout_s"].is_u64());
    let HostEvent::PairingRequest(req) = next_event(&mut events).await else {
        panic!("expected PairingRequest");
    };
    assert_eq!(req.device_name, "Pixel 8");
    assert_eq!(req.method, "qr");
    assert_eq!(server.state().g4.pending().len(), 1);
    wait_toasts(&fake, 1).await;
    let toast = &fake.toasts()[0];
    let ids: Vec<&str> = toast.actions.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, ["allow", "deny"]);
    assert!(toast.title.contains("Pixel 8"));
    assert!(fake.click_toast(0, Some("deny"), None));
    assert_error(&recv(&mut ws).await, "denied");
    assert!(matches!(next_event(&mut events).await,
        HostEvent::PairingResolved { outcome: Outcome::Denied, .. }));
    assert!(server.devices().unwrap().is_empty());

    // Allow from the toast.
    let p = server.new_pairing();
    let (mut ws, pending) = phone.pair(&server, &p.token).await;
    assert_eq!(pending["type"], "pair_pending");
    wait_toasts(&fake, 2).await;
    assert!(fake.click_toast(1, Some("allow"), None));
    assert_eq!(recv(&mut ws).await["type"], "ready");
    assert_eq!(server.devices().unwrap().len(), 1);
    drop(ws);

    // The same phone with the same key re-pairing is not asked about again.
    let p = server.new_pairing();
    let (_ws, ready) = phone.pair(&server, &p.token).await;
    assert_eq!(ready["type"], "ready");

    // Nobody answers: denied after the timeout.
    let p = server.new_pairing();
    let (mut ws, pending) = Phone::new("Other").pair(&server, &p.token).await;
    assert_eq!(pending["type"], "pair_pending");
    assert_error(&recv(&mut ws).await, "denied");
    assert_eq!(server.devices().unwrap().len(), 1);
    let audit = std::fs::read_to_string(dir.path().join("audit.log")).unwrap();
    assert!(audit.contains("outcome=denied") && audit.contains("outcome=allowed"));
    assert!(audit.contains("outcome=timeout"));
    server.shutdown().await;
}

#[tokio::test]
async fn approval_through_the_api_for_code_pairing() {
    let dir = tempfile::tempdir().unwrap();
    let server = start(dir.path()).await;
    server.state().g4.set_require_approval(true).unwrap();
    let mut events = server.state().g4.subscribe();
    let shown = server.new_code_pairing().unwrap();
    let state = server.state().clone();
    // A desktop app would render PairingRequest and call resolve().
    let approver = tokio::spawn(async move {
        loop {
            if let HostEvent::PairingRequest(req) = next_event(&mut events).await {
                assert_eq!(req.method, "code");
                assert!(state.g4.resolve(req.request_id, true));
                return;
            }
        }
    });
    let phone = Phone::new("Pixel");
    let (pending, mut ws) =
        agent_reply(pair_code(&server, &phone, &shown.code, server.fingerprint(), true).await);
    assert_eq!(pending["type"], "pair_pending");
    assert_eq!(recv(&mut ws).await["type"], "ready");
    approver.await.unwrap();
    assert!(!server.state().g4.resolve(999, true));
    server.shutdown().await;
}

#[tokio::test]
async fn pairing_settings_methods() {
    let dir = tempfile::tempdir().unwrap();
    let server = start(dir.path()).await;
    let p = server.new_pairing();
    let phone = Phone::new("Pixel");
    let (mut ws, ready) = phone.pair(&server, &p.token).await;
    assert_eq!(ready["type"], "ready");
    let r = call(&mut ws, 1, "pairing.settings.get", json!({})).await;
    assert_eq!(r["result"], json!({"require_approval": false}));
    let r = call(&mut ws, 2, "pairing.settings.set", json!({"require_approval": true})).await;
    assert_eq!(r["result"], json!({"require_approval": true}));
    let r = call(&mut ws, 3, "pairing.settings.set", json!({})).await;
    assert_eq!(r["error"]["code"], "bad_params");
    assert!(server.state().g4.settings().require_approval);
    let saved = std::fs::read_to_string(dir.path().join("pairing.json")).unwrap();
    assert!(saved.contains("\"require_approval\": true"));
    server.shutdown().await;
}
