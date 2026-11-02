//! Wire messages and signature checks (PROTOCOL.md §5, §6).

use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{Signature, VerifyingKey};
use p256::pkcs8::DecodePublicKey;
use serde_json::{Value, json};

use crate::util;

pub const PROTOCOL_VERSION: u32 = 1;
pub const AGENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Maximum WebSocket message / frame size (1 MiB).
pub const MAX_MESSAGE: usize = 1024 * 1024;

/// The string both sides sign: `helm-v1|<purpose>|<host_id>|<nonce>`.
pub fn signed_string(purpose: &str, host_id: &str, nonce: &str) -> String {
    format!("helm-v1|{purpose}|{host_id}|{nonce}")
}

/// Parse a base64 SPKI DER public key and require it to be P-256.
pub fn parse_public_key(b64_spki: &str) -> Option<VerifyingKey> {
    let der = util::b64_decode(b64_spki)?;
    VerifyingKey::from_public_key_der(&der).ok()
}

/// Verify a base64 DER SHA256withECDSA signature over `message`.
pub fn verify_signature(key: &VerifyingKey, message: &str, b64_der_sig: &str) -> bool {
    let Some(der) = util::b64_decode(b64_der_sig) else {
        return false;
    };
    let Ok(sig) = Signature::from_der(&der) else {
        return false;
    };
    // ECDSA accepts both s and n-s; Android may produce either form.
    let sig = sig.normalize_s().unwrap_or(sig);
    key.verify(message.as_bytes(), &sig).is_ok()
}

pub fn hello(host_id: &str, host_name: &str, nonce: &str) -> Value {
    json!({
        "type": "hello",
        "protocol": PROTOCOL_VERSION,
        "host_id": host_id,
        "host_name": host_name,
        "nonce": nonce,
        "agent_version": AGENT_VERSION,
        "features": FEATURES,
    })
}

/// Optional protocol features this agent speaks (sent in `hello`).
pub const FEATURES: &[&str] = &["p1", "p2", "p3", "p4", "p5", "g3", "g4", "g6"];

pub fn event(name: &str, data: Value) -> Value {
    json!({"type": "event", "name": name, "data": data})
}

pub fn ready(device_id: &str, tier: &str, host_id: &str, host_name: &str) -> Value {
    json!({
        "type": "ready",
        "device_id": device_id,
        "tier": tier,
        "host_id": host_id,
        "host_name": host_name,
    })
}

pub fn error(code: &str, message: &str) -> Value {
    json!({"type": "error", "code": code, "message": message})
}

pub fn res_ok(id: i64, result: Value) -> Value {
    json!({"type": "res", "id": id, "ok": true, "result": result})
}

pub fn res_err(id: i64, code: &str, message: &str) -> Value {
    json!({"type": "res", "id": id, "ok": false, "error": {"code": code, "message": message}})
}

/// Handshake error codes (§5.3).
pub mod codes {
    pub const BAD_MESSAGE: &str = "bad_message";
    pub const BAD_TOKEN: &str = "bad_token";
    pub const PAIRING_LOCKED: &str = "pairing_locked";
    pub const UNKNOWN_DEVICE: &str = "unknown_device";
    pub const BAD_SIGNATURE: &str = "bad_signature";
    pub const TIMEOUT: &str = "timeout";
    /// g4: the PC owner denied the pairing (or did not answer in time).
    pub const DENIED: &str = "denied";
    /// g4: short-code pairing failed (wrong code, no code shown, or a man in the middle).
    pub const BAD_CODE: &str = "bad_code";
}

/// g4 `pair_code` (short-code pairing, `Docs/protocol/pairing-security.md`).
#[derive(Debug, Clone)]
pub struct PairCode {
    pub device_id: String,
    pub device_name: String,
    pub public_key: String,
    /// Phone nonce (base64, 32 bytes).
    pub nonce: String,
    /// 6 base64 commitments, one per code digit.
    pub commits: Vec<String>,
    /// Signature over `helm-v1|pair_code|<host_id>|<hello nonce>`.
    pub signature: String,
}

/// What the phone sent in reply to `hello`.
#[derive(Debug)]
pub enum Handshake {
    Pair {
        token: String,
        device_id: String,
        device_name: String,
        public_key: String,
        signature: String,
    },
    Auth {
        device_id: String,
        signature: String,
    },
    /// g4 short-code pairing.
    PairCode(PairCode),
}

fn field<'a>(v: &'a Value, name: &str) -> Result<&'a str, String> {
    v.get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing or non-string field \"{name}\""))
}

fn device_id_field(v: &Value) -> Result<String, String> {
    let id = field(v, "device_id")?;
    if id.is_empty() || id.len() > 128 {
        return Err("device_id must be 1-128 characters".into());
    }
    Ok(id.to_string())
}

fn device_name_field(v: &Value) -> Result<String, String> {
    let mut device_name: String = field(v, "device_name")?
        .chars()
        .filter(|c| !c.is_control())
        .take(100)
        .collect();
    if device_name.trim().is_empty() {
        device_name = "Phone".to_string();
    }
    Ok(device_name)
}

pub fn parse_handshake(text: &str) -> Result<Handshake, String> {
    let v: Value = serde_json::from_str(text).map_err(|_| "message is not JSON".to_string())?;
    match v.get("type").and_then(Value::as_str) {
        Some("pair_code") => {
            let commits = v
                .get("commits")
                .and_then(Value::as_array)
                .ok_or("missing \"commits\" array")?
                .iter()
                .map(|c| c.as_str().map(str::to_string))
                .collect::<Option<Vec<String>>>()
                .ok_or("commits must be strings")?;
            Ok(Handshake::PairCode(PairCode {
                device_id: device_id_field(&v)?,
                device_name: device_name_field(&v)?,
                public_key: field(&v, "public_key")?.to_string(),
                nonce: field(&v, "nonce")?.to_string(),
                commits,
                signature: field(&v, "signature")?.to_string(),
            }))
        }
        Some("pair") => {
            let device_name = device_name_field(&v)?;
            Ok(Handshake::Pair {
                token: field(&v, "token")?.to_string(),
                device_id: device_id_field(&v)?,
                device_name,
                public_key: field(&v, "public_key")?.to_string(),
                signature: field(&v, "signature")?.to_string(),
            })
        }
        Some("auth") => Ok(Handshake::Auth {
            device_id: device_id_field(&v)?,
            signature: field(&v, "signature")?.to_string(),
        }),
        Some(other) => Err(format!("expected pair or auth, got \"{other}\"")),
        None => Err("missing \"type\"".into()),
    }
}

/// A request after `ready`.
#[derive(Debug)]
pub struct Request {
    pub id: i64,
    pub method: String,
    pub params: Value,
}

pub fn parse_request(text: &str) -> Result<Request, String> {
    let v: Value = serde_json::from_str(text).map_err(|_| "message is not JSON".to_string())?;
    if v.get("type").and_then(Value::as_str) != Some("req") {
        return Err("expected type \"req\"".into());
    }
    let id = v
        .get("id")
        .and_then(Value::as_i64)
        .ok_or("missing or non-integer \"id\"")?;
    let method = field(&v, "method")?.to_string();
    let params = match v.get("params") {
        None | Some(Value::Null) => json!({}),
        Some(p) => p.clone(),
    };
    Ok(Request { id, method, params })
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::SigningKey;
    use p256::ecdsa::signature::Signer;
    use p256::pkcs8::EncodePublicKey;

    #[test]
    fn signed_string_format() {
        assert_eq!(
            signed_string("pair", "host-1", "abc+/=="),
            "helm-v1|pair|host-1|abc+/=="
        );
        assert_eq!(signed_string("auth", "h", "n"), "helm-v1|auth|h|n");
    }

    #[test]
    fn verify_round_trip() {
        let sk = SigningKey::random(&mut rand::rngs::OsRng);
        let spki = sk.verifying_key().to_public_key_der().unwrap();
        let key = parse_public_key(&util::b64_encode(spki.as_bytes())).unwrap();
        let msg = signed_string("auth", "h", "n");
        let sig: Signature = sk.sign(msg.as_bytes());
        let sig_b64 = util::b64_encode(sig.to_der().as_bytes());
        assert!(verify_signature(&key, &msg, &sig_b64));
        assert!(!verify_signature(
            &key,
            &signed_string("pair", "h", "n"),
            &sig_b64
        ));
        assert!(!verify_signature(&key, &msg, "not base64!"));
        assert!(parse_public_key("AAAA").is_none());
    }

    #[test]
    fn parse_messages() {
        assert!(matches!(
            parse_handshake(r#"{"type":"auth","device_id":"d","signature":"s"}"#),
            Ok(Handshake::Auth { .. })
        ));
        assert!(parse_handshake(r#"{"type":"auth","device_id":"d"}"#).is_err());
        assert!(parse_handshake(r#"{"type":"hello"}"#).is_err());
        assert!(parse_handshake("nope").is_err());
        let r = parse_request(r#"{"type":"req","id":7,"method":"system.ping"}"#).unwrap();
        assert_eq!(r.id, 7);
        assert_eq!(r.params, json!({}));
        assert!(parse_request(r#"{"type":"req","id":"7","method":"x"}"#).is_err());
    }
}
