//! Helm PC agent (`helm-agent`): the Windows side of Helm.
//!
//! Phase 0: TLS WebSocket server with a pinned self-signed certificate, QR pairing,
//! persistent trusted devices, mDNS discovery and a small request router.
//! Phase 1: apps, windows, input, media, volume, brightness, power, dock/deck, clipboard,
//! file transfer, screenshots and live events, all behind [`platform::Platform`].
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
pub mod cursor;
pub mod desktop;
pub mod devices;
pub mod dpapi;
pub mod files;
pub mod g4;
pub mod g6;
pub mod g5;
pub mod http;
pub mod identity;
pub mod mdns;
pub mod modules;
pub mod net;
pub mod obs;
pub mod p3;
pub mod p5;
pub mod pairing;
pub mod paths;
pub mod platform;
pub mod present;
pub mod proto;
pub mod qr;
pub mod relay;
pub mod router;
pub mod screen;
pub mod server;
pub mod session;
pub mod store;
pub mod stream;
pub mod sysinfo;
#[cfg(windows)]
pub mod tray;
pub mod update;
pub mod util;
pub mod wol;

pub use pairing::Pairing;
pub use server::{AgentEvent, AppState, Config, DEFAULT_PORT, ServerHandle, default_data_dir, start};
