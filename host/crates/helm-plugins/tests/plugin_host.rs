mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{FakeServer, json};
use helm_plugins::http::StdHttpTransport;
use helm_plugins::plugin::{
    HostServices, LogLevel, Manifest, PluginError, PluginHost, PluginLimits, PluginPackage, codes,
};
use serde_json::{Value, json as j};

#[derive(Default)]
struct Recorder {
    logs: Mutex<Vec<(String, LogLevel, String)>>,
    actions: Mutex<Vec<Value>>,
    events: Mutex<Vec<(String, Value)>>,
}

impl HostServices for Recorder {
    fn log(&self, id: &str, level: LogLevel, msg: &str) {
        self.logs
            .lock()
            .unwrap()
            .push((id.into(), level, msg.into()));
    }
    fn run_deck_action(&self, _id: &str, action: &Value) -> Result<(), String> {
        self.actions.lock().unwrap().push(action.clone());
        Ok(())
    }
    fn emit_event(&self, _id: &str, name: &str, data: &Value) {
        self.events
            .lock()
            .unwrap()
            .push((name.into(), data.clone()));
    }
}

const ALL_CAPS: [&str; 3] = ["deck.action", "http:127.0.0.1", "notify"];

/// Test plugin in WAT. `{URL}` / `{URL_LEN}` are substituted per test.
fn plugin_wat(url: &str) -> String {
    let wat = r#"
(module
  (import "helm" "log" (func $log (param i32 i32 i32)))
  (import "helm" "run_deck_action" (func $deck (param i32 i32) (result i32)))
  (import "helm" "http_get" (func $get (param i32 i32 i32 i32) (result i32)))
  (import "helm" "http_status" (func $status (result i32)))
  (import "helm" "emit_event" (func $emit (param i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (global $bump (mut i32) (i32.const 16384))
  (data (i32.const 0) "plugin started")
  (data (i32.const 32) "{\"hotkey\":\"ctrl+shift+esc\"}")
  (data (i32.const 96) "{\"format_disk\":true}")
  (data (i32.const 128) "ping")
  (data (i32.const 144) "{\"n\":1}")
  (data (i32.const 160) "http://example.com/")
  (data (i32.const 256) "{URL}")
  (func (export "init") (result i32)
    (call $log (i32.const 1) (i32.const 0) (i32.const 14)) (i32.const 0))
  (func (export "deck_ok") (result i32) (call $deck (i32.const 32) (i32.const 27)))
  (func (export "deck_bad") (result i32) (call $deck (i32.const 96) (i32.const 20)))
  (func (export "deck_bad_ptr") (result i32) (call $deck (i32.const 0x7fff0000) (i32.const 10)))
  (func (export "fetch") (result i32)
    (call $get (i32.const 256) (i32.const {URL_LEN}) (i32.const 8192) (i32.const 1024)))
  (func (export "fetch_small") (result i32)
    (call $get (i32.const 256) (i32.const {URL_LEN}) (i32.const 8192) (i32.const 2)))
  (func (export "fetch_denied") (result i32)
    (call $get (i32.const 160) (i32.const 19) (i32.const 8192) (i32.const 1024)))
  (func (export "status") (result i32) (call $status))
  (func (export "first_byte") (result i32) (i32.load8_u (i32.const 8192)))
  (func (export "emit") (result i32)
    (call $emit (i32.const 128) (i32.const 4) (i32.const 144) (i32.const 7)))
  (func (export "spin") (result i32) (loop $l (br $l)) (i32.const 0))
  (func (export "grow") (result i32) (memory.grow (i32.const 1000)))
  (func (export "alloc") (param i32) (result i32)
    (local $p i32)
    (local.set $p (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get 0)))
    (local.get $p))
  (func (export "handle") (param i32 i32) (result i32)
    (call $log (i32.const 2) (local.get 0) (local.get 1))
    (local.get 1))
)"#;
    wat.replace("{URL_LEN}", &url.len().to_string())
        .replace("{URL}", url)
}

fn manifest(caps: &[&str]) -> Manifest {
    Manifest {
        id: "test.plugin".into(),
        name: "Test".into(),
        version: "1.0.0".into(),
        capabilities: caps.iter().map(|s| s.to_string()).collect(),
        description: None,
    }
}

fn package(url: &str) -> PluginPackage {
    let wasm = wat::parse_str(plugin_wat(url)).unwrap();
    PluginPackage::new(manifest(&ALL_CAPS), wasm).unwrap()
}

fn host_with(limits: PluginLimits) -> (PluginHost, Arc<Recorder>) {
    let rec = Arc::new(Recorder::default());
    let host = PluginHost::new(limits, rec.clone(), Arc::new(StdHttpTransport::default())).unwrap();
    (host, rec)
}

fn approve(caps: &[&str]) -> Vec<String> {
    caps.iter().map(|s| s.to_string()).collect()
}

#[test]
fn loads_from_folder_and_runs_init() {
    let dir = std::env::temp_dir().join(format!("helm-plugins-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("plugin.json"),
        serde_json::to_string(&manifest(&ALL_CAPS)).unwrap(),
    )
    .unwrap();
    std::fs::write(
        dir.join("module.wasm"),
        wat::parse_str(plugin_wat("x")).unwrap(),
    )
    .unwrap();
    let (host, rec) = host_with(PluginLimits::default());
    let pkg = host.load_dir(&dir).unwrap();
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(pkg.manifest.id, "test.plugin");
    let p = host.instantiate(&pkg, &approve(&ALL_CAPS)).unwrap();
    assert_eq!(p.id(), "test.plugin");
    assert_eq!(
        rec.logs.lock().unwrap()[0],
        (
            "test.plugin".to_string(),
            LogLevel::Info,
            "plugin started".to_string()
        )
    );
}

#[test]
fn deck_actions_are_validated_and_capability_gated() {
    let (host, rec) = host_with(PluginLimits::default());
    let mut p = host
        .instantiate(&package("x"), &approve(&ALL_CAPS))
        .unwrap();
    assert_eq!(p.call("deck_ok").unwrap(), codes::OK);
    assert_eq!(p.call("deck_bad").unwrap(), codes::BAD_INPUT);
    assert_eq!(p.call("deck_bad_ptr").unwrap(), codes::BAD_MEMORY);
    assert_eq!(
        *rec.actions.lock().unwrap(),
        vec![j!({"hotkey":"ctrl+shift+esc"})]
    );

    // Requested but not approved by the user -> denied.
    let mut p = host
        .instantiate(&package("x"), &approve(&["notify"]))
        .unwrap();
    assert_eq!(p.call("deck_ok").unwrap(), codes::DENIED);
    assert_eq!(rec.actions.lock().unwrap().len(), 1);
}

#[test]
fn http_get_only_reaches_allowed_hosts() {
    let srv = FakeServer::start(|r| {
        if r.path == "/data" {
            json(200, r#"{"ok":true}"#)
        } else {
            json(404, "")
        }
    });
    let (host, _) = host_with(PluginLimits::default());
    let mut p = host
        .instantiate(&package(&srv.url("/data")), &approve(&ALL_CAPS))
        .unwrap();
    assert_eq!(p.call("fetch").unwrap(), 11);
    assert_eq!(p.call("status").unwrap(), 200);
    assert_eq!(p.call("first_byte").unwrap(), i32::from(b'{'));
    // Truncated buffer still reports the full length.
    assert_eq!(p.call("fetch_small").unwrap(), 11);
    assert_eq!(p.call("fetch_denied").unwrap(), codes::DENIED);
    assert_eq!(srv.requests().len(), 2);

    // Without the http capability nothing is fetched.
    let mut p = host
        .instantiate(&package(&srv.url("/data")), &approve(&["deck.action"]))
        .unwrap();
    assert_eq!(p.call("fetch").unwrap(), codes::DENIED);
    assert_eq!(srv.requests().len(), 2);
}

#[test]
fn http_calls_per_invocation_are_limited() {
    let srv = FakeServer::start(|_| json(200, "x"));
    let (host, _) = host_with(PluginLimits {
        max_http_per_call: 0,
        ..PluginLimits::default()
    });
    let mut p = host
        .instantiate(&package(&srv.url("/")), &approve(&ALL_CAPS))
        .unwrap();
    assert_eq!(p.call("fetch").unwrap(), codes::LIMIT);
    assert!(srv.requests().is_empty());
}

#[test]
fn emit_event_needs_notify() {
    let (host, rec) = host_with(PluginLimits::default());
    let mut p = host
        .instantiate(&package("x"), &approve(&ALL_CAPS))
        .unwrap();
    assert_eq!(p.call("emit").unwrap(), codes::OK);
    assert_eq!(
        *rec.events.lock().unwrap(),
        vec![("ping".to_string(), j!({"n":1}))]
    );
    let mut p = host
        .instantiate(&package("x"), &approve(&["deck.action"]))
        .unwrap();
    assert_eq!(p.call("emit").unwrap(), codes::DENIED);
}

#[test]
fn fuel_exhaustion_traps_and_poisons() {
    let (host, _) = host_with(PluginLimits {
        fuel_per_call: 100_000,
        timeout: Duration::from_secs(30),
        ..PluginLimits::default()
    });
    let mut p = host.instantiate(&package("x"), &[]).unwrap();
    assert!(matches!(p.call("spin"), Err(PluginError::OutOfFuel)));
    assert!(p.is_poisoned());
    assert!(matches!(p.call("deck_ok"), Err(PluginError::Poisoned)));
}

#[test]
fn wall_clock_timeout_interrupts() {
    let (host, _) = host_with(PluginLimits {
        fuel_per_call: u64::MAX / 2,
        timeout: Duration::from_millis(50),
        ..PluginLimits::default()
    });
    let mut p = host.instantiate(&package("x"), &[]).unwrap();
    let t = std::time::Instant::now();
    assert!(matches!(p.call("spin"), Err(PluginError::Timeout)));
    assert!(t.elapsed() < Duration::from_secs(5));
}

#[test]
fn memory_cap_is_enforced() {
    let (host, _) = host_with(PluginLimits::default()); // 16 MiB
    let mut p = host.instantiate(&package("x"), &[]).unwrap();
    assert_eq!(p.call("grow").unwrap(), -1);
    let big = wat::parse_str(r#"(module (memory (export "memory") 512))"#).unwrap();
    let pkg = PluginPackage::new(manifest(&[]), big).unwrap();
    assert!(matches!(
        host.instantiate(&pkg, &[]),
        Err(PluginError::Instantiate(_))
    ));
}

#[test]
fn rejects_foreign_imports_missing_memory_and_garbage() {
    let (host, _) = host_with(PluginLimits::default());
    let wasi = wat::parse_str(
        r#"(module (import "wasi_snapshot_preview1" "fd_write" (func (param i32 i32 i32 i32) (result i32))) (memory (export "memory") 1))"#,
    )
    .unwrap();
    let pkg = PluginPackage::new(manifest(&[]), wasi).unwrap();
    assert!(matches!(
        host.instantiate(&pkg, &[]),
        Err(PluginError::Instantiate(_))
    ));
    let nomem = PluginPackage::new(manifest(&[]), wat::parse_str("(module)").unwrap()).unwrap();
    assert!(matches!(
        host.instantiate(&nomem, &[]),
        Err(PluginError::MissingExport(_))
    ));
    let junk = PluginPackage::new(manifest(&[]), b"not wasm".to_vec()).unwrap();
    assert!(matches!(
        host.instantiate(&junk, &[]),
        Err(PluginError::Compile(_))
    ));
    let (small, _) = host_with(PluginLimits {
        max_module_bytes: 4,
        ..PluginLimits::default()
    });
    assert!(matches!(
        small.instantiate(&junk, &[]),
        Err(PluginError::ModuleTooLarge(_))
    ));
}

#[test]
fn handle_passes_input_through_alloc() {
    let (host, rec) = host_with(PluginLimits::default());
    let mut p = host.instantiate(&package("x"), &[]).unwrap();
    assert_eq!(p.handle(b"hello plugin").unwrap(), 12);
    let logs = rec.logs.lock().unwrap();
    assert_eq!(logs.last().unwrap().2, "hello plugin");
    assert_eq!(logs.last().unwrap().1, LogLevel::Warn);
    drop(logs);
    let too_big = vec![0u8; PluginLimits::default().max_input_bytes + 1];
    assert!(matches!(
        p.handle(&too_big),
        Err(PluginError::InputTooLarge(_))
    ));
    assert!(matches!(p.call("nope"), Err(PluginError::MissingExport(_))));
}
