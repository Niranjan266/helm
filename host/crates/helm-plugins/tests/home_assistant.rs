mod common;

use std::sync::Arc;

use common::{FakeServer, Reply, json};
use helm_plugins::ha::{HaClient, HaError};
use helm_plugins::http::{HttpError, StdHttpTransport};
use serde_json::json as j;

const TOKEN: &str = "test-token-abc";

fn client(srv: &FakeServer) -> HaClient {
    HaClient::new(&srv.base(), TOKEN, Arc::new(StdHttpTransport::default())).unwrap()
}

#[test]
fn lists_states_with_bearer_token() {
    let srv = FakeServer::start(|r| match r.path.as_str() {
        "/api/states" => json(
            200,
            r#"[{"entity_id":"light.kitchen","state":"on","attributes":{"friendly_name":"Kitchen"},"last_changed":"2026-10-31T10:00:00+00:00"},
                {"entity_id":"sensor.temp","state":"21.5"}]"#,
        ),
        _ => json(404, "{}"),
    });
    let states = client(&srv).states().unwrap();
    assert_eq!(states.len(), 2);
    assert_eq!(states[0].friendly_name(), Some("Kitchen"));
    assert_eq!(states[1].state, "21.5");
    assert!(states[1].attributes.is_empty());
    let reqs = srv.requests();
    assert_eq!(reqs[0].method, "GET");
    assert_eq!(
        reqs[0].header("authorization"),
        Some("Bearer test-token-abc")
    );
    assert!(reqs[0].header("host").unwrap().starts_with("127.0.0.1:"));
}

#[test]
fn single_state_and_ping() {
    let srv = FakeServer::start(|r| match r.path.as_str() {
        "/api/" => json(200, r#"{"message":"API running."}"#),
        "/api/states/switch.fan" => json(200, r#"{"entity_id":"switch.fan","state":"off"}"#),
        _ => json(404, r#"{"message":"Entity not found."}"#),
    });
    let c = client(&srv);
    assert_eq!(c.ping().unwrap(), "API running.");
    assert_eq!(c.state("switch.fan").unwrap().state, "off");
    assert_eq!(c.state("switch.nope"), Err(HaError::NotFound));
}

#[test]
fn calls_service_with_json_body() {
    let srv = FakeServer::start(|r| {
        if r.method == "POST" && r.path == "/api/services/light/turn_on" {
            json(200, r#"[{"entity_id":"light.kitchen","state":"on"}]"#)
        } else {
            json(400, "bad")
        }
    });
    let changed = client(&srv)
        .call_service(
            "light",
            "turn_on",
            &j!({"entity_id":"light.kitchen","brightness_pct":40}),
        )
        .unwrap();
    assert_eq!(changed[0].state, "on");
    let req = &srv.requests()[0];
    assert_eq!(req.header("content-type"), Some("application/json"));
    let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
    assert_eq!(body, j!({"entity_id":"light.kitchen","brightness_pct":40}));
}

#[test]
fn null_data_sends_empty_object_and_empty_reply_is_ok() {
    let srv = FakeServer::start(|_| Reply::Normal(200, vec![], Vec::new()));
    let out = client(&srv)
        .call_service("scene", "turn_on", &serde_json::Value::Null)
        .unwrap();
    assert!(out.is_empty());
    assert_eq!(srv.requests()[0].body, b"{}");
}

#[test]
fn maps_error_statuses() {
    let srv = FakeServer::start(|r| match r.path.as_str() {
        "/api/states" => json(401, r#"{"message":"Unauthorized"}"#),
        "/api/services/light/bad" => json(400, r#"{"message":"Invalid data"}"#),
        "/api/services/light/boom" => json(500, "oops"),
        "/api/services/light/teapot" => json(418, ""),
        _ => json(200, "not json"),
    });
    let c = client(&srv);
    assert_eq!(c.states(), Err(HaError::Unauthorized));
    assert!(
        matches!(c.call_service("light", "bad", &j!({})), Err(HaError::BadRequest(m)) if m.contains("Invalid data"))
    );
    assert_eq!(
        c.call_service("light", "boom", &j!({})),
        Err(HaError::Server {
            status: 500,
            body: "oops".into()
        })
    );
    assert_eq!(
        c.call_service("light", "teapot", &j!({})),
        Err(HaError::UnexpectedStatus(418))
    );
    assert!(matches!(c.ping(), Err(HaError::Decode(_))));
}

#[test]
fn rejects_bad_names_before_any_request() {
    let srv = FakeServer::start(|_| json(200, "[]"));
    let c = client(&srv);
    assert!(matches!(c.state("../config"), Err(HaError::InvalidName(_))));
    assert!(matches!(
        c.state("light.x/../../y"),
        Err(HaError::InvalidName(_))
    ));
    assert!(matches!(
        c.call_service("light/../x", "on", &j!({})),
        Err(HaError::InvalidName(_))
    ));
    assert!(matches!(
        c.call_service("light", "on?x=1", &j!({})),
        Err(HaError::InvalidName(_))
    ));
    assert!(matches!(
        c.call_service("light", "on", &j!([1])),
        Err(HaError::InvalidConfig(_))
    ));
    assert!(srv.requests().is_empty());
}

#[test]
fn decodes_chunked_responses() {
    let srv = FakeServer::start(|_| {
        let body = r#"[{"entity_id":"a.b","state":"x"}]"#;
        let (a, b) = body.split_at(10);
        Reply::Raw(
            format!(
                "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{a}\r\n{:x}\r\n{b}\r\n0\r\n\r\n",
                a.len(),
                b.len()
            )
            .into_bytes(),
        )
    });
    assert_eq!(client(&srv).states().unwrap()[0].entity_id, "a.b");
}

#[test]
fn config_and_transport_errors() {
    let t = Arc::new(StdHttpTransport::default());
    assert!(matches!(
        HaClient::new("ha.local", TOKEN, t.clone()),
        Err(HaError::InvalidConfig(_))
    ));
    assert!(matches!(
        HaClient::new("http://ha.local", "", t.clone()),
        Err(HaError::InvalidConfig(_))
    ));
    assert!(matches!(
        HaClient::new("http://ha.local", "a\nb", t.clone()),
        Err(HaError::InvalidConfig(_))
    ));
    assert!(matches!(
        HaClient::new("http://ha.local/?x", TOKEN, t.clone()),
        Err(HaError::InvalidConfig(_))
    ));

    // https needs a TLS transport.
    let c = HaClient::new("https://ha.local:8123/", TOKEN, t.clone()).unwrap();
    assert!(matches!(
        c.states(),
        Err(HaError::Transport(HttpError::UnsupportedScheme(_)))
    ));

    // Nothing listening: bind then drop to get a free port.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let c = HaClient::new(&format!("http://127.0.0.1:{port}"), TOKEN, t).unwrap();
    assert!(matches!(
        c.states(),
        Err(HaError::Transport(HttpError::Connect(_)))
    ));

    // Debug never shows the token.
    assert!(!format!("{c:?}").contains(TOKEN));
}

#[test]
fn base_url_path_prefix_is_kept() {
    let srv = FakeServer::start(|r| {
        if r.path == "/ha/api/states" {
            json(200, "[]")
        } else {
            json(404, "")
        }
    });
    let c = HaClient::new(
        &format!("{}/ha/", srv.base()),
        TOKEN,
        Arc::new(StdHttpTransport::default()),
    )
    .unwrap();
    assert!(c.states().unwrap().is_empty());
}
