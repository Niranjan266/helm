//! Test helpers: start a server in-process and act like the phone.
#![allow(dead_code)]

use std::net::Ipv4Addr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures_util::{SinkExt, StreamExt};
pub use helm_agent::platform::fake::FakePlatform;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use p256::pkcs8::EncodePublicKey;
use rustls::DigitallySignedStruct;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

pub type Ws = WebSocketStream<TlsStream<TcpStream>>;

pub async fn start(dir: &Path) -> helm_agent::ServerHandle {
    start_with(dir, |_| {}).await
}

pub async fn start_with(
    dir: &Path,
    tweak: impl FnOnce(&mut helm_agent::Config),
) -> helm_agent::ServerHandle {
    start_fake_with(dir, tweak).await.0
}

/// Start a server that controls a [`FakePlatform`] (never this PC) with dry-run power.
/// Uploads go to `<dir>/downloads`; downloads are allowed from `<dir>/profile`.
pub async fn start_fake_with(
    dir: &Path,
    tweak: impl FnOnce(&mut helm_agent::Config),
) -> (helm_agent::ServerHandle, Arc<FakePlatform>) {
    let fake = FakePlatform::new();
    let mut config = helm_agent::Config::new(dir);
    config.port = 0;
    config.enable_mdns = false;
    config.host_name = Some("TEST-PC".into());
    config.addresses = Some(vec![Ipv4Addr::LOCALHOST]);
    config.platform = Some(fake.clone());
    config.power_dry_run = true;
    config.downloads_dir = Some(dir.join("downloads"));
    std::fs::create_dir_all(dir.join("profile")).unwrap();
    config.download_root = Some(dir.join("profile"));
    config.windows_poll = Duration::from_millis(100);
    tweak(&mut config);
    // Safety net: tests must never drive the real PC.
    assert!(config.platform.is_some() && config.power_dry_run);
    let server = helm_agent::start(config).await.expect("server starts");
    (server, fake)
}

/// Accepts the server only if SHA-256(leaf DER) equals the pinned fingerprint.
#[derive(Debug)]
struct PinnedVerifier {
    fingerprint: String,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let actual = hex::encode(Sha256::digest(end_entity.as_ref()));
        if actual == self.fingerprint {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(format!(
                "fingerprint mismatch: {actual}"
            )))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

pub async fn try_connect_path(port: u16, fingerprint: &str, path: &str) -> anyhow::Result<Ws> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedVerifier {
            fingerprint: fingerprint.to_string(),
            provider,
        }))
        .with_no_client_auth();
    let tcp = TcpStream::connect(("127.0.0.1", port)).await?;
    let tls = TlsConnector::from(Arc::new(config))
        .connect(ServerName::try_from("helm-agent.local")?, tcp)
        .await?;
    let (ws, _) = tokio_tungstenite::client_async_with_config(
        format!("wss://127.0.0.1:{port}{path}"),
        tls,
        None,
    )
    .await?;
    Ok(ws)
}

pub async fn connect(server: &helm_agent::ServerHandle) -> Ws {
    try_connect_path(server.port(), server.fingerprint(), "/ws")
        .await
        .expect("connect")
}

/// Next JSON text message (skips control frames). Panics on close/timeout.
pub async fn recv(ws: &mut Ws) -> Value {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("timed out waiting for a message")
            .expect("connection closed")
            .expect("websocket error");
        match msg {
            Message::Text(t) => return serde_json::from_str(t.as_str()).expect("valid JSON"),
            Message::Close(f) => panic!("unexpected close: {f:?}"),
            _ => continue,
        }
    }
}

/// Assert the server closes the connection (close frame or EOF) soon.
pub async fn expect_closed(ws: &mut Ws) {
    let deadline = Duration::from_secs(5);
    let result = tokio::time::timeout(deadline, async {
        loop {
            match ws.next().await {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return,
                Some(Ok(Message::Text(t))) => panic!("expected close, got {t}"),
                Some(Ok(_)) => continue,
            }
        }
    })
    .await;
    assert!(result.is_ok(), "server did not close the connection");
}

pub async fn send(ws: &mut Ws, value: Value) {
    ws.send(Message::text(value.to_string()))
        .await
        .expect("send");
}

/// A fake phone: device id + P-256 key.
pub struct Phone {
    pub device_id: String,
    pub name: String,
    pub key: SigningKey,
}

impl Phone {
    pub fn new(name: &str) -> Phone {
        Phone {
            device_id: uuid::Uuid::new_v4().to_string(),
            name: name.to_string(),
            key: SigningKey::random(&mut rand::rngs::OsRng),
        }
    }

    pub fn public_key_b64(&self) -> String {
        let der = self.key.verifying_key().to_public_key_der().unwrap();
        STANDARD.encode(der.as_bytes())
    }

    pub fn sign(&self, purpose: &str, host_id: &str, nonce: &str) -> String {
        let msg = format!("helm-v1|{purpose}|{host_id}|{nonce}");
        let sig: Signature = self.key.sign(msg.as_bytes());
        STANDARD.encode(sig.to_der().as_bytes())
    }

    pub fn pair_msg(&self, hello: &Value, token: &str) -> Value {
        let host_id = hello["host_id"].as_str().unwrap();
        let nonce = hello["nonce"].as_str().unwrap();
        json!({
            "type": "pair",
            "token": token,
            "device_id": self.device_id,
            "device_name": self.name,
            "public_key": self.public_key_b64(),
            "signature": self.sign("pair", host_id, nonce),
        })
    }

    pub fn auth_msg(&self, hello: &Value) -> Value {
        let host_id = hello["host_id"].as_str().unwrap();
        let nonce = hello["nonce"].as_str().unwrap();
        json!({
            "type": "auth",
            "device_id": self.device_id,
            "signature": self.sign("auth", host_id, nonce),
        })
    }

    /// Connect and pair; returns the open session and the `ready` message.
    pub async fn pair(&self, server: &helm_agent::ServerHandle, token: &str) -> (Ws, Value) {
        let mut ws = connect(server).await;
        let hello = recv(&mut ws).await;
        send(&mut ws, self.pair_msg(&hello, token)).await;
        let reply = recv(&mut ws).await;
        (ws, reply)
    }

    /// Connect and auth; returns the open session and the reply (`ready` or `error`).
    pub async fn auth(&self, server: &helm_agent::ServerHandle) -> (Ws, Value) {
        let mut ws = connect(server).await;
        let hello = recv(&mut ws).await;
        send(&mut ws, self.auth_msg(&hello)).await;
        let reply = recv(&mut ws).await;
        (ws, reply)
    }
}

/// Send a request and wait for the response with the same id.
pub async fn call(ws: &mut Ws, id: i64, method: &str, params: Value) -> Value {
    send(
        ws,
        json!({"type": "req", "id": id, "method": method, "params": params}),
    )
    .await;
    loop {
        let msg = recv(ws).await;
        if msg["type"] == "res" && msg["id"] == id {
            return msg;
        }
    }
}
