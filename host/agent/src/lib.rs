//! Helm PC agent (`helm-agent`): the Windows side of Helm.
//!
//! Phase 0: TLS WebSocket server with a pinned self-signed certificate, QR pairing,
//! persistent trusted devices, mDNS discovery and a small request router.
//! The wire contract is `Docs/PROTOCOL.md`.
//!
//! ```no_run
//! # async fn demo() -> anyhow::Result<()> {
//! let mut config = helm_agent::Config::new("C:/temp/helm");
//! config.port = 0;
//! config.enable_mdns = false;
//! let server = helm_agent::start(config).await?;
//! println!("{} {}", server.fingerprint(), server.new_pairing().uri);
//! server.shutdown().await;
//! # Ok(()) }
//! ```

pub mod audit;
pub mod devices;
pub mod dpapi;
pub mod identity;
pub mod mdns;
pub mod modules;
pub mod net;
pub mod pairing;
pub mod proto;
pub mod qr;
pub mod router;
pub mod server;
pub mod sysinfo;
pub mod util;

pub use pairing::Pairing;
pub use server::{AppState, Config, DEFAULT_PORT, ServerHandle, default_data_dir, start};
