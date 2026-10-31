//! End-to-end tests: real TLS + WebSocket against an in-process agent.

mod common;

use std::time::Duration;

use common::*;
use serde_json::json;

fn assert_error(msg: &serde_json::Value, code: &str) {
    assert_eq!(msg["type"], "error", "expected error, got {msg}");
    assert_eq!(msg["code"], code, "got {msg}");
    assert!(msg["message"].is_string());
}

#[tokio::test]
async fn happy_path_pair_then_requests() {
    let dir = tempfile::tempdir().unwrap();
    let server = start(dir.path()).await;
    let pairing = server.new_pairing();
    assert_eq!(pairing.token.len(), 43);
    assert!(
        pairing
            .uri
            .contains(&format!("&fp={}&", server.fingerprint()))
    );

    let phone = Phone::new("Pixel 8");
    let mut ws = connect(&server).await;
    let hello = recv(&mut ws).await;
    assert_eq!(hello["type"], "hello");
    assert_eq!(hello["protocol"], 1);
    assert_eq!(hello["host_id"], server.host_id());
    assert_eq!(hello["host_name"], "TEST-PC");
    assert_eq!(hello["agent_version"], env!("CARGO_PKG_VERSION"));
    let nonce = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        hello["nonce"].as_str().unwrap(),
    )
    .unwrap();
    assert_eq!(nonce.len(), 32);

    send(&mut ws, phone.pair_msg(&hello, &pairing.token)).await;
    let ready = recv(&mut ws).await;
    assert_eq!(
        ready,
        json!({
            "type": "ready",
            "device_id": phone.device_id,
            "tier": "admin",
            "host_id": server.host_id(),
            "host_name": "TEST-PC",
        })
    );

    let pong = call(&mut ws, 1, "system.ping", json!({})).await;
    assert_eq!(pong["ok"], true);
    assert_eq!(pong["result"]["pong"], true);
    assert!(pong["result"]["time_ms"].as_i64().unwrap() > 1_600_000_000_000);

    // params is optional
    send(
        &mut ws,
        json!({"type": "req", "id": 2, "method": "system.info"}),
    )
    .await;
    let info = recv(&mut ws).await;
    assert_eq!(info["id"], 2);
    assert_eq!(info["ok"], true);
    let r = &info["result"];
    assert_eq!(r["host_id"], server.host_id());
    assert_eq!(r["host_name"], "TEST-PC");
    assert_eq!(r["agent_version"], env!("CARGO_PKG_VERSION"));
    assert!(r["os"].as_str().unwrap().len() > 3);
    assert!(r["uptime_s"].is_u64());

    let list = call(&mut ws, 3, "devices.list", json!({})).await;
    let devices = list["result"]["devices"].as_array().unwrap();
    assert_eq!(devices.len(), 1);
    let d = &devices[0];
    assert_eq!(d["device_id"], phone.device_id);
    assert_eq!(d["device_name"], "Pixel 8");
    assert_eq!(d["tier"], "admin");
    assert_eq!(d["this"], true);
    assert!(d["paired_at_ms"].as_i64().unwrap() > 0);
    assert!(d["last_seen_ms"].as_i64().unwrap() > 0);

    let audit = std::fs::read_to_string(dir.path().join("audit.log")).unwrap();
    assert!(audit.contains(&format!("\tpair_ok\t{}\t127.0.0.1:", phone.device_id)));
    drop(ws);
    server.shutdown().await;
}

#[tokio::test]
async fn auth_survives_restart() {
    let dir = tempfile::tempdir().unwrap();
    let phone = Phone::new("Pixel");
    let (host_id, fingerprint) = {
        let server = start(dir.path()).await;
        let token = server.new_pairing().token;
        let (_ws, ready) = phone.pair(&server, &token).await;
        assert_eq!(ready["type"], "ready");
        let ids = (
            server.host_id().to_string(),
            server.fingerprint().to_string(),
        );
        server.shutdown().await;
        ids
    };
    let paired_at = {
        let text = std::fs::read_to_string(dir.path().join("devices.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        v["devices"][0]["last_seen_ms"].as_i64().unwrap()
    };
    tokio::time::sleep(Duration::from_millis(20)).await;

    // "Reboot": a brand-new server on the same data dir.
    let server = start(dir.path()).await;
    assert_eq!(server.host_id(), host_id);
    assert_eq!(server.fingerprint(), fingerprint);
    let (mut ws, ready) = phone.auth(&server).await;
    assert_eq!(ready["type"], "ready", "{ready}");
    assert_eq!(ready["device_id"], phone.device_id);
    assert_eq!(ready["tier"], "admin");
    let list = call(&mut ws, 1, "devices.list", json!({})).await;
    let last_seen = list["result"]["devices"][0]["last_seen_ms"]
        .as_i64()
        .unwrap();
    assert!(
        last_seen > paired_at,
        "last_seen_ms was not updated on auth"
    );
    let audit = std::fs::read_to_string(dir.path().join("audit.log")).unwrap();
    assert!(audit.contains("\tauth_ok\t"));
    server.shutdown().await;
}

#[tokio::test]
async fn token_is_single_use() {
    let dir = tempfile::tempdir().unwrap();
    let server = start(dir.path()).await;
    let token = server.new_pairing().token;
    let (_a, ready) = Phone::new("A").pair(&server, &token).await;
    assert_eq!(ready["type"], "ready");
    let (mut b, reply) = Phone::new("B").pair(&server, &token).await;
    assert_error(&reply, "bad_token");
    expect_closed(&mut b).await;
    server.shutdown().await;
}

#[tokio::test]
async fn new_token_replaces_old_one() {
    let dir = tempfile::tempdir().unwrap();
    let server = start(dir.path()).await;
    let old = server.new_pairing().token;
    let new = server.new_pairing().token;
    let (_ws, reply) = Phone::new("A").pair(&server, &old).await;
    assert_error(&reply, "bad_token");
    let (_ws, reply) = Phone::new("A").pair(&server, &new).await;
    assert_eq!(reply["type"], "ready");
    server.shutdown().await;
}

#[tokio::test]
async fn wrong_token_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let server = start(dir.path()).await;
    let _ = server.new_pairing();
    let (mut ws, reply) = Phone::new("A")
        .pair(&server, "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA")
        .await;
    assert_error(&reply, "bad_token");
    expect_closed(&mut ws).await;
    assert!(server.devices().unwrap().is_empty());
    let audit = std::fs::read_to_string(dir.path().join("audit.log")).unwrap();
    assert!(audit.contains("\tpair_fail\t"));
    server.shutdown().await;
}

#[tokio::test]
async fn expired_token_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_with(dir.path(), |c| c.token_lifetime = Duration::from_millis(50)).await;
    let token = server.new_pairing().token;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let (_ws, reply) = Phone::new("A").pair(&server, &token).await;
    assert_error(&reply, "bad_token");
    server.shutdown().await;
}

#[tokio::test]
async fn lockout_after_five_bad_tokens() {
    let dir = tempfile::tempdir().unwrap();
    let server = start(dir.path()).await;
    let good = server.new_pairing().token;
    let phone = Phone::new("Attacker");
    for i in 0..5 {
        let (_ws, reply) = phone.pair(&server, &format!("wrong-{i}")).await;
        assert_error(&reply, "bad_token");
    }
    // Even the right token is refused while locked.
    let (mut ws, reply) = phone.pair(&server, &good).await;
    assert_error(&reply, "pairing_locked");
    expect_closed(&mut ws).await;
    server.shutdown().await;
}

#[tokio::test]
async fn lockout_expires() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_with(dir.path(), |c| c.lockout = Duration::from_millis(200)).await;
    let phone = Phone::new("P");
    for _ in 0..5 {
        let (_ws, reply) = phone.pair(&server, "nope").await;
        assert_error(&reply, "bad_token");
    }
    let (_ws, reply) = phone.pair(&server, "nope").await;
    assert_error(&reply, "pairing_locked");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let token = server.new_pairing().token;
    let (_ws, reply) = phone.pair(&server, &token).await;
    assert_eq!(reply["type"], "ready", "{reply}");
    server.shutdown().await;
}

#[tokio::test]
async fn unknown_device_auth() {
    let dir = tempfile::tempdir().unwrap();
    let server = start(dir.path()).await;
    let (mut ws, reply) = Phone::new("Stranger").auth(&server).await;
    assert_error(&reply, "unknown_device");
    expect_closed(&mut ws).await;
    server.shutdown().await;
}

#[tokio::test]
async fn bad_signature_on_auth_and_pair() {
    let dir = tempfile::tempdir().unwrap();
    let server = start(dir.path()).await;
    let phone = Phone::new("P");
    let token = server.new_pairing().token;
    let (_ws, ready) = phone.pair(&server, &token).await;
    assert_eq!(ready["type"], "ready");

    // auth signed over a different nonce
    let mut ws = connect(&server).await;
    let hello = recv(&mut ws).await;
    let mut msg = phone.auth_msg(&hello);
    msg["signature"] = json!(phone.sign("auth", server.host_id(), "some-other-nonce"));
    send(&mut ws, msg).await;
    assert_error(&recv(&mut ws).await, "bad_signature");
    expect_closed(&mut ws).await;

    // auth signed with purpose=pair
    let mut ws = connect(&server).await;
    let hello = recv(&mut ws).await;
    let mut msg = phone.auth_msg(&hello);
    msg["signature"] =
        json!(phone.sign("pair", server.host_id(), hello["nonce"].as_str().unwrap()));
    send(&mut ws, msg).await;
    assert_error(&recv(&mut ws).await, "bad_signature");

    // auth signed by a different key
    let impostor = Phone {
        device_id: phone.device_id.clone(),
        name: "fake".into(),
        key: Phone::new("x").key,
    };
    let (_ws, reply) = impostor.auth(&server).await;
    assert_error(&reply, "bad_signature");

    // pair with a bad signature, then the same token still works for the real phone
    let token = server.new_pairing().token;
    let other = Phone::new("Other");
    let mut ws = connect(&server).await;
    let hello = recv(&mut ws).await;
    let mut msg = other.pair_msg(&hello, &token);
    msg["signature"] = json!("MEUCIQ==");
    send(&mut ws, msg).await;
    assert_error(&recv(&mut ws).await, "bad_signature");

    // pair with a public key that isn't P-256 SPKI
    let mut ws = connect(&server).await;
    let hello = recv(&mut ws).await;
    let mut msg = other.pair_msg(&hello, &token);
    msg["public_key"] = json!("AAAA");
    send(&mut ws, msg).await;
    assert_error(&recv(&mut ws).await, "bad_signature");

    let (_ws, reply) = other.pair(&server, &token).await;
    assert_eq!(reply["type"], "ready", "{reply}");
    server.shutdown().await;
}

#[tokio::test]
async fn revoke_other_device_then_its_auth_fails() {
    let dir = tempfile::tempdir().unwrap();
    let server = start(dir.path()).await;
    let a = Phone::new("A");
    let b = Phone::new("B");
    let (mut wa, _) = a.pair(&server, &server.new_pairing().token).await;
    let (mut wb, ready_b) = b.pair(&server, &server.new_pairing().token).await;
    assert_eq!(ready_b["type"], "ready");

    let list = call(&mut wa, 1, "devices.list", json!({})).await;
    let devices = list["result"]["devices"].as_array().unwrap();
    assert_eq!(devices.len(), 2);
    let this_flags: Vec<bool> = devices
        .iter()
        .map(|d| d["this"].as_bool().unwrap())
        .collect();
    assert_eq!(this_flags.iter().filter(|t| **t).count(), 1);

    let res = call(
        &mut wa,
        2,
        "devices.revoke",
        json!({"device_id": b.device_id}),
    )
    .await;
    assert_eq!(res["ok"], true);
    assert_eq!(res["result"], json!({"revoked": true}));

    // B's open session is closed, and B can no longer authenticate.
    expect_closed(&mut wb).await;
    let (_ws, reply) = b.auth(&server).await;
    assert_error(&reply, "unknown_device");

    // A is unaffected.
    let pong = call(&mut wa, 3, "system.ping", json!({})).await;
    assert_eq!(pong["ok"], true);
    let audit = std::fs::read_to_string(dir.path().join("audit.log")).unwrap();
    assert!(audit.contains(&format!("\trevoke\t{}\t", b.device_id)));
    server.shutdown().await;
}

#[tokio::test]
async fn device_can_revoke_itself() {
    let dir = tempfile::tempdir().unwrap();
    let server = start(dir.path()).await;
    let a = Phone::new("A");
    let (mut wa, _) = a.pair(&server, &server.new_pairing().token).await;
    let res = call(
        &mut wa,
        9,
        "devices.revoke",
        json!({"device_id": a.device_id}),
    )
    .await;
    assert_eq!(res["result"]["revoked"], true);
    expect_closed(&mut wa).await;
    let (_ws, reply) = a.auth(&server).await;
    assert_error(&reply, "unknown_device");
    server.shutdown().await;
}

#[tokio::test]
async fn request_errors() {
    let dir = tempfile::tempdir().unwrap();
    let server = start(dir.path()).await;
    let (mut ws, _) = Phone::new("A")
        .pair(&server, &server.new_pairing().token)
        .await;

    let r = call(&mut ws, 1, "nope.method", json!({})).await;
    assert_eq!(r["ok"], false);
    assert_eq!(r["error"]["code"], "unknown_method");
    assert!(r["error"]["message"].is_string());

    let r = call(&mut ws, 2, "devices.revoke", json!({})).await;
    assert_eq!(r["error"]["code"], "bad_params");

    let r = call(&mut ws, 3, "system.ping", json!("not an object")).await;
    assert_eq!(r["error"]["code"], "bad_params");

    // Malformed message after ready: error frame, connection stays open.
    send(&mut ws, json!({"type": "req", "method": "system.ping"})).await;
    assert_error(&recv(&mut ws).await, "bad_message");
    let r = call(&mut ws, 4, "system.ping", json!({})).await;
    assert_eq!(r["ok"], true);
    server.shutdown().await;
}

#[tokio::test]
async fn many_requests_in_flight() {
    let dir = tempfile::tempdir().unwrap();
    let server = start(dir.path()).await;
    let (mut ws, _) = Phone::new("A")
        .pair(&server, &server.new_pairing().token)
        .await;
    for id in 1..=20 {
        let method = if id % 2 == 0 {
            "system.ping"
        } else {
            "system.info"
        };
        send(&mut ws, json!({"type": "req", "id": id, "method": method})).await;
    }
    let mut seen = std::collections::HashSet::new();
    while seen.len() < 20 {
        let r = recv(&mut ws).await;
        assert_eq!(r["type"], "res");
        assert_eq!(r["ok"], true);
        seen.insert(r["id"].as_i64().unwrap());
    }
    assert_eq!(seen, (1..=20).collect());
    server.shutdown().await;
}

#[tokio::test]
async fn handshake_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let server = start_with(dir.path(), |c| {
        c.handshake_timeout = Duration::from_millis(300)
    })
    .await;
    let mut ws = connect(&server).await;
    let hello = recv(&mut ws).await;
    assert_eq!(hello["type"], "hello");
    let reply = recv(&mut ws).await; // say nothing; the agent gives up
    assert_error(&reply, "timeout");
    expect_closed(&mut ws).await;
    server.shutdown().await;
}

#[tokio::test]
async fn bad_messages_in_handshake() {
    let dir = tempfile::tempdir().unwrap();
    let server = start(dir.path()).await;
    for bad in [
        "not json".to_string(),
        json!({"type": "hello"}).to_string(),
        json!({"type": "auth"}).to_string(),
        json!({"type": "req", "id": 1, "method": "system.ping"}).to_string(),
    ] {
        let mut ws = connect(&server).await;
        let _hello = recv(&mut ws).await;
        futures_util::SinkExt::send(&mut ws, tokio_tungstenite::tungstenite::Message::text(bad))
            .await
            .unwrap();
        assert_error(&recv(&mut ws).await, "bad_message");
        expect_closed(&mut ws).await;
    }
    server.shutdown().await;
}

#[tokio::test]
async fn re_pairing_same_device_replaces_key() {
    let dir = tempfile::tempdir().unwrap();
    let server = start(dir.path()).await;
    let first = Phone::new("Old name");
    let (_ws, r) = first.pair(&server, &server.new_pairing().token).await;
    assert_eq!(r["type"], "ready");
    let second = Phone {
        device_id: first.device_id.clone(),
        name: "New name".into(),
        key: Phone::new("x").key,
    };
    let (_ws, r) = second.pair(&server, &server.new_pairing().token).await;
    assert_eq!(r["type"], "ready");
    let devices = server.devices().unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].device_name, "New name");
    // The old key no longer works; the new one does.
    let (_ws, r) = first.auth(&server).await;
    assert_error(&r, "bad_signature");
    let (_ws, r) = second.auth(&server).await;
    assert_eq!(r["type"], "ready");
    server.shutdown().await;
}

#[tokio::test]
async fn wrong_path_and_wrong_pin_fail() {
    let dir = tempfile::tempdir().unwrap();
    let server = start(dir.path()).await;
    assert!(
        try_connect_path(server.port(), server.fingerprint(), "/other")
            .await
            .is_err()
    );
    let wrong_fp = "0".repeat(64);
    assert!(
        try_connect_path(server.port(), &wrong_fp, "/ws")
            .await
            .is_err()
    );
    server.shutdown().await;
}
