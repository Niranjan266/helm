//! The agent as the sign-in service runs it (`Config::pre_login`): same identity and paired
//! phones, but only screen-less methods. Fake platform and dry-run power only.

mod common;

use std::time::Duration;

use common::*;
use helm_win_system::power::PowerAction;
use serde_json::json;

#[tokio::test]
async fn pre_login_serves_only_screenless_methods() {
    let dir = tempfile::tempdir().unwrap();

    // Pair a phone with the normal agent first (as the desktop app would have).
    let server = start(dir.path()).await;
    let host_id = server.host_id().to_string();
    let phone = Phone::new("Pixel");
    let token = server.new_pairing().token;
    let (_ws, ready) = phone.pair(&server, &token).await;
    assert_eq!(ready["type"], "ready", "{ready}");
    drop(_ws);
    server.shutdown().await;

    let (server, _fake) = start_fake_with(dir.path(), |c| c.pre_login = true).await;
    assert_eq!(server.host_id(), host_id, "same identity before sign-in");
    let dry = server.state().power_dry_run.clone().unwrap();
    let (mut ws, ready) = phone.auth(&server).await;
    assert_eq!(ready["type"], "ready", "{ready}");

    let info = call(&mut ws, 1, "system.info", json!({})).await;
    assert_eq!(info["ok"], true, "{info}");
    assert_eq!(info["result"]["pre_login"], true);
    assert_eq!(call(&mut ws, 2, "system.ping", json!({})).await["ok"], true);

    // Screen, input, files, apps and settings changes are simply not there.
    for (i, m) in [
        "screen.capture",
        "input.batch",
        "apps.list",
        "files.list",
        "devices.revoke",
        "relay.set",
    ]
    .iter()
    .enumerate()
    {
        let r = call(&mut ws, 10 + i as i64, m, json!({})).await;
        assert_eq!(r["ok"], false, "{m}: {r}");
        assert_eq!(r["error"]["code"], "unknown_method", "{m}: {r}");
    }

    // Lock is a no-op (the sign-in screen is up), sign-out has nobody to sign out.
    let r = call(&mut ws, 30, "power.action", json!({"action":"lock"})).await;
    assert_eq!(r["ok"], true, "{r}");
    let r = call(&mut ws, 31, "power.action", json!({"action":"sign_out"})).await;
    assert_eq!(r["error"]["code"], "unsupported", "{r}");
    let r = call(&mut ws, 32, "power.action", json!({"action":"restart"})).await;
    assert_eq!(r["ok"], true, "{r}");
    for _ in 0..100 {
        if !dry.executed().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    assert_eq!(dry.executed(), vec![PowerAction::Restart]);
    server.shutdown().await;
}

#[tokio::test]
async fn pre_login_never_creates_an_identity() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = helm_agent::Config::new(dir.path());
    config.port = 0;
    config.enable_mdns = false;
    config.platform = Some(FakePlatform::new());
    config.power_dry_run = true;
    config.pre_login = true;
    assert!(helm_agent::start(config).await.is_err());
    assert!(!dir.path().join("identity.json").exists());
}
