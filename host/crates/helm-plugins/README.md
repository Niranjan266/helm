# helm-plugins

Automation building blocks for the Helm PC agent (ROADMAP Phase 5b step C:
Plugins H8, Triggers H7, Smart home H9). MIT licensed. Pure-Rust
dependencies (wasmtime + Cranelift, serde, thiserror); no WASI, no TLS stack.

## Modules

- `plugin` — WASM plugin host. A plugin is a folder with `plugin.json`
  (`{id,name,version,capabilities}`) and `module.wasm`. Capabilities:
  `deck.action`, `http:<host>[:port]`, `notify`. The effective grant is
  *requested ∩ user-approved*. Host imports (module `"helm"`): `log`,
  `run_deck_action` (validated deck JSON, needs `deck.action`), `http_get`
  + `http_status` (allowed hosts only, no redirects), `emit_event` (needs
  `notify`). Return codes in `plugin::codes`. Each call gets a fuel budget,
  a wall-clock deadline (epoch interruption), a memory cap, and host-call /
  HTTP budgets. A plugin that traps or hits a limit is poisoned until reloaded.
  Exports: `memory` (required), `init`, `alloc` + `handle`, any `() -> i32`.
- `trigger` — deterministic trigger engine with an injectable `Clock`
  (`SystemClock` with a UTC offset, `FakeClock` for tests). Conditions:
  `{"time":"HH:MM days"}` (`mon-fri`, `weekends`, `sat,sun`, `daily`…),
  `{"app_start":{"exe"}}`, `{"phone_connected":{"device_id"?}}`,
  `{"wifi":{"ssid"}}`. Edge-triggered (dedupes repeats), per-trigger debounce
  (default 2 s), 5-minute catch-up for late ticks, no re-fire on DST fall-back,
  identical actions in one batch are returned once. It returns `Firing`s; the
  agent runs them.
- `ha` — Home Assistant REST client (long-lived token): `ping`, `states`,
  `state(entity_id)`, `call_service(domain, service, data)`. Typed `HaError`;
  names are validated before any request; the token is never shown in `Debug`.
- `deck` — deck action validation (PROTOCOL.md §9.3, §11; macro ≤ 50 steps).
- `http` — `HttpTransport` trait plus `StdHttpTransport` (plain `http://`
  only). For `https://` plug in a TLS transport from the agent.

## Tests

```
cargo test
cargo clippy --all-targets -- -D warnings
```

The plugin tests build a small WAT plugin with the `wat` crate (no external
toolchain). HTTP and Home Assistant tests use only a local fake server on
127.0.0.1.
