//! Helm automation building blocks for the PC agent (Phase 5b step C).
//!
//! - [`plugin`]: sandboxed WASM plugin host (wasmtime) with a small,
//!   capability-gated host API.
//! - [`trigger`]: deterministic trigger engine (time, app start, phone
//!   connected, Wi-Fi) that turns events into deck actions.
//! - [`ha`]: Home Assistant REST client.
//! - [`deck`]: validation of deck action JSON (PROTOCOL.md §9.3 / §11).
//! - [`http`]: minimal HTTP/1.1 transport trait plus a std-only client.
//!
//! Nothing here executes deck actions itself: the agent supplies that
//! through [`plugin::HostServices`] and by running [`trigger::Firing`]s.

#![forbid(unsafe_code)]

pub mod deck;
pub mod ha;
pub mod http;
pub mod plugin;
pub mod trigger;
