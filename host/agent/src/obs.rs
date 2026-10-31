//! Minimal obs-websocket v5 client for deck `remote` actions (PROTOCOL §11).
//!
//! Settings come from `obs.json` in the data folder: `{"password": "...", "port": 4455}`
//! (both optional; the host is always 127.0.0.1). Without `obs.json` OBS actions fail with
//! `bad_params`, so nothing is ever sent to OBS unless the user set it up.
//!
//! Flow per action: connect, read `Hello` (op 0), send `Identify` (op 1, with the auth string
//! when the server asks for one), wait for `Identified` (op 2), send one `Request` (op 6),
//! wait for its `RequestResponse` (op 7), close.

use std::path::Path;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;

use crate::router::RpcError;
use crate::util;

pub const OBS_FILE: &str = "obs.json";
pub const DEFAULT_PORT: u16 = 4455;
const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Default, Deserialize)]
pub struct ObsConfig {
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
}

pub fn load_config(data_dir: &Path) -> Result<ObsConfig, RpcError> {
    match crate::store::read_json::<ObsConfig>(&data_dir.join(OBS_FILE)) {
        Ok(Some(c)) => Ok(c),
        Ok(None) => Err(RpcError::bad_params(
            "OBS is not set up: create obs.json in the Helm data folder",
        )),
        Err(e) => Err(RpcError::bad_params(format!("obs.json is invalid: {e}"))),
    }
}

/// `base64(sha256(base64(sha256(password + salt)) + challenge))`.
pub fn auth_string(password: &str, salt: &str, challenge: &str) -> String {
    let secret = util::b64_encode(&Sha256::digest(format!("{password}{salt}").as_bytes()));
    util::b64_encode(&Sha256::digest(format!("{secret}{challenge}").as_bytes()))
}

/// Map a deck `remote` name (`obs.*`) to an obs-websocket request.
pub fn request_for(name: &str) -> Option<(&'static str, Value)> {
    match name {
        "obs.toggle_record" => Some(("ToggleRecord", json!({}))),
        "obs.toggle_stream" => Some(("ToggleStream", json!({}))),
        _ => name
            .strip_prefix("obs.scene:")
            .filter(|s| !s.is_empty())
            .map(|scene| ("SetCurrentProgramScene", json!({"sceneName": scene}))),
    }
}

/// Run a deck `remote` OBS action.
pub async fn run_remote(data_dir: &Path, name: &str) -> Result<(), RpcError> {
    let (request_type, data) = request_for(name)
        .ok_or_else(|| RpcError::bad_params(format!("unknown remote {name:?}")))?;
    let config = load_config(data_dir)?;
    tokio::time::timeout(TIMEOUT, request(&config, request_type, data))
        .await
        .map_err(|_| RpcError::internal("OBS did not answer in time"))?
        .map(|_| ())
}

fn obs_err(what: impl std::fmt::Display) -> RpcError {
    RpcError::internal(format!("OBS: {what}"))
}

async fn next_json<S>(ws: &mut S) -> Result<Value, RpcError>
where
    S: futures_util::Stream<Item = tokio_tungstenite::tungstenite::Result<Message>> + Unpin,
{
    loop {
        match ws.next().await {
            Some(Ok(Message::Text(t))) => {
                return serde_json::from_str(t.as_str()).map_err(|_| obs_err("bad message"));
            }
            Some(Ok(Message::Close(frame))) => {
                let code = frame.as_ref().map(|f| u16::from(f.code)).unwrap_or(0);
                return Err(if code == 4009 {
                    RpcError::bad_params("OBS password is wrong (check obs.json)")
                } else {
                    obs_err(format!("connection closed ({code})"))
                });
            }
            Some(Ok(_)) => continue,
            Some(Err(e)) => return Err(obs_err(e)),
            None => return Err(obs_err("connection closed")),
        }
    }
}

/// Send one request; returns its `responseData` (or `null`).
pub async fn request(
    config: &ObsConfig,
    request_type: &str,
    data: Value,
) -> Result<Value, RpcError> {
    let port = config.port.unwrap_or(DEFAULT_PORT);
    let tcp = TcpStream::connect(("127.0.0.1", port)).await.map_err(|e| {
        obs_err(format!(
            "not reachable on port {port} ({e}); is OBS running?"
        ))
    })?;
    let (mut ws, _) = tokio_tungstenite::client_async(format!("ws://127.0.0.1:{port}"), tcp)
        .await
        .map_err(obs_err)?;

    let hello = next_json(&mut ws).await?;
    if hello["op"] != 0 {
        return Err(obs_err("expected Hello"));
    }
    let mut identify = json!({"rpcVersion": 1, "eventSubscriptions": 0});
    if let Some(auth) = hello["d"].get("authentication") {
        let (Some(challenge), Some(salt)) = (auth["challenge"].as_str(), auth["salt"].as_str())
        else {
            return Err(obs_err("bad authentication challenge"));
        };
        let password = config
            .password
            .as_deref()
            .ok_or_else(|| RpcError::bad_params("OBS needs a password: set it in obs.json"))?;
        identify["authentication"] = json!(auth_string(password, salt, challenge));
    }
    ws.send(Message::text(json!({"op": 1, "d": identify}).to_string()))
        .await
        .map_err(obs_err)?;
    let identified = next_json(&mut ws).await?;
    if identified["op"] != 2 {
        return Err(obs_err("expected Identified"));
    }

    let request_id = uuid::Uuid::new_v4().to_string();
    let req = json!({"op": 6, "d": {
        "requestType": request_type,
        "requestId": request_id,
        "requestData": data,
    }});
    ws.send(Message::text(req.to_string()))
        .await
        .map_err(obs_err)?;
    let result = loop {
        let msg = next_json(&mut ws).await?;
        if msg["op"] == 7 && msg["d"]["requestId"] == request_id.as_str() {
            break msg;
        }
    };
    let _ = ws.close(None).await;
    let status = &result["d"]["requestStatus"];
    if status["result"] == true {
        Ok(result["d"]
            .get("responseData")
            .cloned()
            .unwrap_or(Value::Null))
    } else {
        Err(RpcError::bad_params(format!(
            "OBS refused {request_type}: {}",
            status["comment"].as_str().unwrap_or("no reason given")
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_remote_names() {
        assert_eq!(request_for("obs.toggle_record").unwrap().0, "ToggleRecord");
        assert_eq!(request_for("obs.toggle_stream").unwrap().0, "ToggleStream");
        assert_eq!(
            request_for("obs.scene:Game").unwrap(),
            ("SetCurrentProgramScene", json!({"sceneName":"Game"}))
        );
        assert!(request_for("obs.scene:").is_none());
        assert!(request_for("browser.back").is_none());
    }

    #[test]
    fn auth_string_is_deterministic_base64() {
        let a = auth_string("pw", "salt", "chal");
        assert_eq!(a, auth_string("pw", "salt", "chal"));
        assert_ne!(a, auth_string("pw2", "salt", "chal"));
        assert_eq!(util::b64_decode(&a).unwrap().len(), 32);
    }

    #[test]
    fn missing_config_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_config(dir.path()).unwrap_err().code, "bad_params");
    }
}
