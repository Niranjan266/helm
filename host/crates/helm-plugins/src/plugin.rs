//! Sandboxed WASM plugin host.
//!
//! A plugin is a folder holding `plugin.json` and `module.wasm`:
//!
//! ```json
//! {"id":"ha-lights","name":"HA lights","version":"1.0.0",
//!  "capabilities":["deck.action","http:homeassistant.local","notify"]}
//! ```
//!
//! The module may import only these functions from module `"helm"`
//! (pointers/lengths are into the plugin's exported `memory`):
//!
//! | import | signature | capability |
//! |---|---|---|
//! | `log(level, ptr, len)` | `(i32,i32,i32)` | none (0 debug, 1 info, 2 warn, 3 error) |
//! | `run_deck_action(ptr, len) -> rc` | `(i32,i32)->i32` | `deck.action` |
//! | `http_get(url_ptr, url_len, out_ptr, out_cap) -> len` | `(i32,i32,i32,i32)->i32` | `http:<host>` |
//! | `http_status() -> status` | `()->i32` | none (status of the last `http_get`) |
//! | `emit_event(name_ptr, name_len, json_ptr, json_len) -> rc` | `(i32,i32,i32,i32)->i32` | `notify` |
//!
//! Return codes are in [`codes`]. `http_get` writes at most `out_cap` bytes
//! and returns the full body length, so a result larger than `out_cap`
//! means "truncated, retry with a bigger buffer".
//!
//! Exports: `memory` (required), `init() -> i32` (optional),
//! `alloc(len) -> ptr` + `handle(ptr, len) -> i32` (optional, for
//! [`PluginInstance::handle`]), and any other `() -> i32` function callable
//! by name with [`PluginInstance::call`].
//!
//! Every call runs with a fuel budget, a wall-clock deadline (epoch
//! interruption), a memory cap and a host-call budget. A plugin that traps
//! or hits a limit is poisoned and refuses further calls until reloaded.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use wasmtime::{
    Caller, Config, Engine, Extern, Instance, Linker, Memory, Module, Store, StoreLimits,
    StoreLimitsBuilder, Trap,
};

use crate::deck;
use crate::http::{HttpRequest, HttpTransport, Url};

pub const MANIFEST_FILE: &str = "plugin.json";
pub const MODULE_FILE: &str = "module.wasm";
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
const EPOCH_TICK: Duration = Duration::from_millis(10);

/// Host function return codes (non-negative values mean success).
pub mod codes {
    pub const OK: i32 = 0;
    /// Capability not granted / host not allowed.
    pub const DENIED: i32 = -1;
    /// Pointer or length outside the plugin's memory.
    pub const BAD_MEMORY: i32 = -2;
    /// Not UTF-8, invalid JSON, invalid action or URL.
    pub const BAD_INPUT: i32 = -3;
    /// The host-side operation failed (network error, action failed).
    pub const HOST_ERROR: i32 = -4;
    /// Per-call host-call or HTTP budget used up.
    pub const LIMIT: i32 = -5;
    /// Input or response larger than allowed.
    pub const TOO_LARGE: i32 = -6;
}

#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("cannot read {path}: {reason}")]
    Io { path: String, reason: String },
    #[error("invalid manifest: {0}")]
    Manifest(String),
    #[error("module is larger than {0} bytes")]
    ModuleTooLarge(usize),
    #[error("invalid module: {0}")]
    Compile(String),
    #[error("instantiation failed: {0}")]
    Instantiate(String),
    #[error("missing or mistyped export `{0}`")]
    MissingExport(String),
    #[error("input larger than {0} bytes")]
    InputTooLarge(usize),
    #[error("plugin ran out of fuel")]
    OutOfFuel,
    #[error("plugin exceeded its time limit")]
    Timeout,
    #[error("plugin trapped: {0}")]
    Trap(String),
    #[error("plugin is disabled after an earlier failure; reload it")]
    Poisoned,
    #[error("engine error: {0}")]
    Engine(String),
}

// ---------------------------------------------------------------- manifest

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Capability {
    DeckAction,
    Notify,
    /// `http:<host>` or `http:<host>:<port>`; scheme may be http or https.
    Http {
        host: String,
        port: Option<u16>,
    },
}

impl Capability {
    pub fn parse(s: &str) -> Result<Self, PluginError> {
        let bad = || PluginError::Manifest(format!("unknown capability `{s}`"));
        match s {
            "deck.action" => Ok(Self::DeckAction),
            "notify" => Ok(Self::Notify),
            _ => {
                let rest = s.strip_prefix("http:").ok_or_else(bad)?;
                let (host, port) = match rest.split_once(':') {
                    Some((h, p)) => (h, Some(p.parse::<u16>().map_err(|_| bad())?)),
                    None => (rest, None),
                };
                let host_ok = !host.is_empty()
                    && host.len() <= 253
                    && host
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
                    && !host.starts_with(['.', '-']);
                if !host_ok {
                    return Err(bad());
                }
                Ok(Self::Http {
                    host: host.to_ascii_lowercase(),
                    port,
                })
            }
        }
    }
}

impl Manifest {
    pub fn parse(json: &str) -> Result<Self, PluginError> {
        let m: Manifest =
            serde_json::from_str(json).map_err(|e| PluginError::Manifest(e.to_string()))?;
        m.validate()?;
        Ok(m)
    }

    pub fn validate(&self) -> Result<(), PluginError> {
        let id_ok = (1..=64).contains(&self.id.len())
            && self
                .id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
            && self.id.as_bytes()[0].is_ascii_alphanumeric();
        if !id_ok {
            return Err(PluginError::Manifest(format!(
                "id `{}` must be 1-64 chars of a-z 0-9 . _ - starting with a letter or digit",
                self.id
            )));
        }
        if self.name.trim().is_empty() || self.name.len() > 100 {
            return Err(PluginError::Manifest("name must be 1-100 chars".into()));
        }
        if self.version.trim().is_empty() || self.version.len() > 32 {
            return Err(PluginError::Manifest("version must be 1-32 chars".into()));
        }
        if self.capabilities.len() > 32 {
            return Err(PluginError::Manifest("too many capabilities".into()));
        }
        for c in &self.capabilities {
            Capability::parse(c)?;
        }
        Ok(())
    }

    /// Capabilities the manifest asks for.
    pub fn requested(&self) -> Vec<Capability> {
        self.capabilities
            .iter()
            .filter_map(|c| Capability::parse(c).ok())
            .collect()
    }
}

/// Effective permissions of one plugin instance.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Grants {
    pub deck_action: bool,
    pub notify: bool,
    pub http: Vec<(String, Option<u16>)>,
}

impl Grants {
    /// Requested ∩ approved. Approval strings use the manifest syntax.
    pub fn effective(manifest: &Manifest, approved: &[String]) -> Self {
        let approved: Vec<Capability> = approved
            .iter()
            .filter_map(|c| Capability::parse(c).ok())
            .collect();
        let mut g = Grants::default();
        for cap in manifest.requested() {
            if !approved.contains(&cap) {
                continue;
            }
            match cap {
                Capability::DeckAction => g.deck_action = true,
                Capability::Notify => g.notify = true,
                Capability::Http { host, port } => g.http.push((host, port)),
            }
        }
        g
    }

    pub fn allows_url(&self, url: &Url) -> bool {
        self.http
            .iter()
            .any(|(h, p)| *h == url.host && p.is_none_or(|p| p == url.port))
    }
}

/// A plugin read from disk (or built in memory), not yet instantiated.
#[derive(Debug, Clone)]
pub struct PluginPackage {
    pub manifest: Manifest,
    pub wasm: Vec<u8>,
}

impl PluginPackage {
    pub fn new(manifest: Manifest, wasm: Vec<u8>) -> Result<Self, PluginError> {
        manifest.validate()?;
        Ok(Self { manifest, wasm })
    }

    /// Load `plugin.json` + `module.wasm` from a plugin folder.
    pub fn from_dir(dir: &Path, max_module_bytes: usize) -> Result<Self, PluginError> {
        let read = |name: &str, max: u64| -> Result<Vec<u8>, PluginError> {
            let p = dir.join(name);
            let io = |e: std::io::Error| PluginError::Io {
                path: p.display().to_string(),
                reason: e.to_string(),
            };
            let len = std::fs::metadata(&p).map_err(io)?.len();
            if len > max {
                return Err(if name == MODULE_FILE {
                    PluginError::ModuleTooLarge(max_module_bytes)
                } else {
                    PluginError::Manifest("plugin.json too large".into())
                });
            }
            std::fs::read(&p).map_err(io)
        };
        let manifest_bytes = read(MANIFEST_FILE, MAX_MANIFEST_BYTES)?;
        let text = std::str::from_utf8(&manifest_bytes)
            .map_err(|_| PluginError::Manifest("not UTF-8".into()))?;
        let manifest = Manifest::parse(text)?;
        let wasm = read(MODULE_FILE, max_module_bytes as u64)?;
        Ok(Self { manifest, wasm })
    }
}

// ---------------------------------------------------------------- host

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    fn from_i32(v: i32) -> Self {
        match v {
            0 => Self::Debug,
            2 => Self::Warn,
            3 => Self::Error,
            _ => Self::Info,
        }
    }
}

/// What the agent provides to plugins. Deck actions arrive already validated.
pub trait HostServices: Send + Sync {
    fn log(&self, plugin_id: &str, level: LogLevel, message: &str);
    fn run_deck_action(&self, plugin_id: &str, action: &Value) -> Result<(), String>;
    fn emit_event(&self, plugin_id: &str, name: &str, data: &Value);
}

#[derive(Debug, Clone)]
pub struct PluginLimits {
    pub fuel_per_call: u64,
    pub timeout: Duration,
    pub memory_bytes: usize,
    pub max_module_bytes: usize,
    pub max_input_bytes: usize,
    pub max_http_body: usize,
    pub max_http_per_call: u32,
    pub max_host_calls_per_call: u32,
    pub max_log_bytes: usize,
    pub max_event_bytes: usize,
}

impl Default for PluginLimits {
    fn default() -> Self {
        Self {
            fuel_per_call: 50_000_000,
            timeout: Duration::from_secs(2),
            memory_bytes: 16 * 1024 * 1024,
            max_module_bytes: 8 * 1024 * 1024,
            max_input_bytes: 64 * 1024,
            max_http_body: 1024 * 1024,
            max_http_per_call: 4,
            max_host_calls_per_call: 256,
            max_log_bytes: 4096,
            max_event_bytes: 64 * 1024,
        }
    }
}

struct StoreData {
    store_limits: StoreLimits,
    plugin_id: String,
    grants: Grants,
    limits: PluginLimits,
    services: Arc<dyn HostServices>,
    http: Arc<dyn HttpTransport>,
    host_calls: u32,
    http_calls: u32,
    last_status: i32,
}

/// Owns the wasmtime engine and the epoch ticker thread.
pub struct PluginHost {
    engine: Engine,
    linker: Linker<StoreData>,
    limits: PluginLimits,
    services: Arc<dyn HostServices>,
    http: Arc<dyn HttpTransport>,
    stop: Arc<AtomicBool>,
    ticker: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for PluginHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginHost")
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl PluginHost {
    pub fn new(
        limits: PluginLimits,
        services: Arc<dyn HostServices>,
        http: Arc<dyn HttpTransport>,
    ) -> Result<Self, PluginError> {
        let mut config = Config::new();
        config.consume_fuel(true);
        config.epoch_interruption(true);
        config.max_wasm_stack(512 * 1024);
        let engine = Engine::new(&config).map_err(|e| PluginError::Engine(e.to_string()))?;
        let linker = build_linker(&engine)?;
        let stop = Arc::new(AtomicBool::new(false));
        let ticker = {
            let engine = engine.clone();
            let stop = stop.clone();
            std::thread::Builder::new()
                .name("helm-plugins-epoch".into())
                .spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        std::thread::sleep(EPOCH_TICK);
                        engine.increment_epoch();
                    }
                })
                .map_err(|e| PluginError::Engine(e.to_string()))?
        };
        Ok(Self {
            engine,
            linker,
            limits,
            services,
            http,
            stop,
            ticker: Some(ticker),
        })
    }

    pub fn limits(&self) -> &PluginLimits {
        &self.limits
    }

    /// Read a plugin folder using this host's module size limit.
    pub fn load_dir(&self, dir: &Path) -> Result<PluginPackage, PluginError> {
        PluginPackage::from_dir(dir, self.limits.max_module_bytes)
    }

    /// Compile and instantiate. `approved` is the user-approved capability
    /// list; the plugin gets only what it both requested and was approved.
    /// Runs the optional `init` export.
    pub fn instantiate(
        &self,
        pkg: &PluginPackage,
        approved: &[String],
    ) -> Result<PluginInstance, PluginError> {
        if pkg.wasm.len() > self.limits.max_module_bytes {
            return Err(PluginError::ModuleTooLarge(self.limits.max_module_bytes));
        }
        pkg.manifest.validate()?;
        let module = Module::from_binary(&self.engine, &pkg.wasm)
            .map_err(|e| PluginError::Compile(e.to_string()))?;
        let data = StoreData {
            store_limits: StoreLimitsBuilder::new()
                .memory_size(self.limits.memory_bytes)
                .instances(1)
                .memories(1)
                .tables(4)
                .table_elements(10_000)
                .build(),
            plugin_id: pkg.manifest.id.clone(),
            grants: Grants::effective(&pkg.manifest, approved),
            limits: self.limits.clone(),
            services: self.services.clone(),
            http: self.http.clone(),
            host_calls: 0,
            http_calls: 0,
            last_status: 0,
        };
        let mut store = Store::new(&self.engine, data);
        store.limiter(|d| &mut d.store_limits);
        let ticks = self.timeout_ticks();
        reset_budget(&mut store, ticks)?;
        let instance =
            self.linker
                .instantiate(&mut store, &module)
                .map_err(|e| match classify(&e) {
                    Some(err) => err,
                    None => PluginError::Instantiate(e.to_string()),
                })?;
        if instance.get_memory(&mut store, "memory").is_none() {
            return Err(PluginError::MissingExport("memory".into()));
        }
        let mut inst = PluginInstance {
            store,
            instance,
            ticks,
            poisoned: false,
        };
        if inst.has_export("init") {
            let rc = inst.call("init")?;
            if rc < 0 {
                return Err(PluginError::Instantiate(format!("init returned {rc}")));
            }
        }
        Ok(inst)
    }

    fn timeout_ticks(&self) -> u64 {
        let tick = EPOCH_TICK.as_millis().max(1);
        (self.limits.timeout.as_millis().div_ceil(tick) as u64).max(1)
    }
}

impl Drop for PluginHost {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.ticker.take() {
            let _ = t.join();
        }
    }
}

fn reset_budget(store: &mut Store<StoreData>, ticks: u64) -> Result<(), PluginError> {
    let fuel = store.data().limits.fuel_per_call;
    store
        .set_fuel(fuel)
        .map_err(|e| PluginError::Engine(e.to_string()))?;
    store.set_epoch_deadline(ticks);
    let d = store.data_mut();
    d.host_calls = 0;
    d.http_calls = 0;
    Ok(())
}

fn classify(e: &wasmtime::Error) -> Option<PluginError> {
    match e.downcast_ref::<Trap>()? {
        Trap::OutOfFuel => Some(PluginError::OutOfFuel),
        Trap::Interrupt => Some(PluginError::Timeout),
        t => Some(PluginError::Trap(t.to_string())),
    }
}

/// One running plugin with its own store and memory.
pub struct PluginInstance {
    store: Store<StoreData>,
    instance: Instance,
    ticks: u64,
    poisoned: bool,
}

impl std::fmt::Debug for PluginInstance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginInstance")
            .field("id", &self.id())
            .field("grants", self.grants())
            .field("poisoned", &self.poisoned)
            .finish()
    }
}

impl PluginInstance {
    pub fn id(&self) -> &str {
        &self.store.data().plugin_id
    }

    pub fn grants(&self) -> &Grants {
        &self.store.data().grants
    }

    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    pub fn has_export(&mut self, name: &str) -> bool {
        self.instance.get_export(&mut self.store, name).is_some()
    }

    fn fail(&mut self, e: wasmtime::Error) -> PluginError {
        self.poisoned = true;
        classify(&e).unwrap_or_else(|| PluginError::Trap(e.to_string()))
    }

    fn prepare(&mut self) -> Result<(), PluginError> {
        if self.poisoned {
            return Err(PluginError::Poisoned);
        }
        reset_budget(&mut self.store, self.ticks)
    }

    /// Call an exported `() -> i32` function.
    pub fn call(&mut self, export: &str) -> Result<i32, PluginError> {
        self.prepare()?;
        let f = self
            .instance
            .get_typed_func::<(), i32>(&mut self.store, export)
            .map_err(|_| PluginError::MissingExport(export.into()))?;
        f.call(&mut self.store, ()).map_err(|e| self.fail(e))
    }

    /// Copy `input` into plugin memory via `alloc(len)` and call
    /// `handle(ptr, len)`.
    pub fn handle(&mut self, input: &[u8]) -> Result<i32, PluginError> {
        let max = self.store.data().limits.max_input_bytes;
        if input.len() > max {
            return Err(PluginError::InputTooLarge(max));
        }
        self.prepare()?;
        let alloc = self
            .instance
            .get_typed_func::<i32, i32>(&mut self.store, "alloc")
            .map_err(|_| PluginError::MissingExport("alloc".into()))?;
        let handle = self
            .instance
            .get_typed_func::<(i32, i32), i32>(&mut self.store, "handle")
            .map_err(|_| PluginError::MissingExport("handle".into()))?;
        let len = input.len() as i32;
        let ptr = alloc.call(&mut self.store, len).map_err(|e| self.fail(e))?;
        let mem = self
            .instance
            .get_memory(&mut self.store, "memory")
            .ok_or_else(|| PluginError::MissingExport("memory".into()))?;
        let start = ptr as u32 as usize;
        match mem
            .data_mut(&mut self.store)
            .get_mut(start..start.saturating_add(input.len()))
        {
            Some(dst) => dst.copy_from_slice(input),
            None => {
                self.poisoned = true;
                return Err(PluginError::Trap(
                    "alloc returned an out-of-bounds pointer".into(),
                ));
            }
        }
        handle
            .call(&mut self.store, (ptr, len))
            .map_err(|e| self.fail(e))
    }
}

// ---------------------------------------------------------------- linker

fn memory(caller: &mut Caller<'_, StoreData>) -> Result<Memory, i32> {
    caller
        .get_export("memory")
        .and_then(Extern::into_memory)
        .ok_or(codes::BAD_MEMORY)
}

fn read_bytes(
    caller: &mut Caller<'_, StoreData>,
    ptr: i32,
    len: i32,
    max: usize,
) -> Result<Vec<u8>, i32> {
    let len = usize::try_from(len).map_err(|_| codes::BAD_MEMORY)?;
    if len > max {
        return Err(codes::TOO_LARGE);
    }
    let mem = memory(caller)?;
    let start = ptr as u32 as usize;
    mem.data(&*caller)
        .get(start..start.checked_add(len).ok_or(codes::BAD_MEMORY)?)
        .map(<[u8]>::to_vec)
        .ok_or(codes::BAD_MEMORY)
}

fn write_bytes(caller: &mut Caller<'_, StoreData>, ptr: i32, bytes: &[u8]) -> Result<(), i32> {
    let mem = memory(caller)?;
    let start = ptr as u32 as usize;
    let end = start.checked_add(bytes.len()).ok_or(codes::BAD_MEMORY)?;
    mem.data_mut(&mut *caller)
        .get_mut(start..end)
        .ok_or(codes::BAD_MEMORY)?
        .copy_from_slice(bytes);
    Ok(())
}

fn charge(caller: &mut Caller<'_, StoreData>) -> bool {
    let d = caller.data_mut();
    if d.host_calls >= d.limits.max_host_calls_per_call {
        return false;
    }
    d.host_calls += 1;
    true
}

fn read_str(
    caller: &mut Caller<'_, StoreData>,
    ptr: i32,
    len: i32,
    max: usize,
) -> Result<String, i32> {
    String::from_utf8(read_bytes(caller, ptr, len, max)?).map_err(|_| codes::BAD_INPUT)
}

fn valid_event_name(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
}

fn build_linker(engine: &Engine) -> Result<Linker<StoreData>, PluginError> {
    let mut l: Linker<StoreData> = Linker::new(engine);
    let e = |e: wasmtime::Error| PluginError::Engine(e.to_string());

    l.func_wrap(
        "helm",
        "log",
        |mut c: Caller<'_, StoreData>, level: i32, ptr: i32, len: i32| {
            if !charge(&mut c) {
                return;
            }
            let max = c.data().limits.max_log_bytes;
            let len = len.min(max as i32);
            let Ok(bytes) = read_bytes(&mut c, ptr, len, max) else {
                return;
            };
            let msg = String::from_utf8_lossy(&bytes);
            let d = c.data();
            d.services
                .log(&d.plugin_id, LogLevel::from_i32(level), &msg);
        },
    )
    .map_err(e)?;

    l.func_wrap(
        "helm",
        "run_deck_action",
        |mut c: Caller<'_, StoreData>, ptr: i32, len: i32| -> i32 {
            if !charge(&mut c) {
                return codes::LIMIT;
            }
            if !c.data().grants.deck_action {
                return codes::DENIED;
            }
            let text = match read_str(&mut c, ptr, len, deck::MAX_ACTION_BYTES) {
                Ok(t) => t,
                Err(rc) => return rc,
            };
            let Ok(action) = deck::parse_action(&text) else {
                return codes::BAD_INPUT;
            };
            let d = c.data();
            match d.services.run_deck_action(&d.plugin_id, &action) {
                Ok(()) => codes::OK,
                Err(_) => codes::HOST_ERROR,
            }
        },
    )
    .map_err(e)?;

    l.func_wrap(
        "helm",
        "http_get",
        |mut c: Caller<'_, StoreData>,
         url_ptr: i32,
         url_len: i32,
         out_ptr: i32,
         out_cap: i32|
         -> i32 {
            if !charge(&mut c) {
                return codes::LIMIT;
            }
            let url_text = match read_str(&mut c, url_ptr, url_len, 2048) {
                Ok(t) => t,
                Err(rc) => return rc,
            };
            let Ok(url) = Url::parse(&url_text) else {
                return codes::BAD_INPUT;
            };
            if !c.data().grants.allows_url(&url) {
                return codes::DENIED;
            }
            if c.data().http_calls >= c.data().limits.max_http_per_call {
                return codes::LIMIT;
            }
            c.data_mut().http_calls += 1;
            let Ok(cap) = usize::try_from(out_cap) else {
                return codes::BAD_MEMORY;
            };
            let http = c.data().http.clone();
            let resp = match http.send(&HttpRequest::get(url_text)) {
                Ok(r) => r,
                Err(_) => return codes::HOST_ERROR,
            };
            if resp.body.len() > c.data().limits.max_http_body {
                return codes::TOO_LARGE;
            }
            c.data_mut().last_status = i32::from(resp.status);
            let n = resp.body.len().min(cap);
            if let Err(rc) = write_bytes(&mut c, out_ptr, &resp.body[..n]) {
                return rc;
            }
            resp.body.len() as i32
        },
    )
    .map_err(e)?;

    l.func_wrap("helm", "http_status", |c: Caller<'_, StoreData>| -> i32 {
        c.data().last_status
    })
    .map_err(e)?;

    l.func_wrap(
        "helm",
        "emit_event",
        |mut c: Caller<'_, StoreData>, name_ptr: i32, name_len: i32, ptr: i32, len: i32| -> i32 {
            if !charge(&mut c) {
                return codes::LIMIT;
            }
            if !c.data().grants.notify {
                return codes::DENIED;
            }
            let name = match read_str(&mut c, name_ptr, name_len, 64) {
                Ok(n) if valid_event_name(&n) => n,
                Ok(_) => return codes::BAD_INPUT,
                Err(rc) => return rc,
            };
            let max = c.data().limits.max_event_bytes;
            let text = match read_str(&mut c, ptr, len, max) {
                Ok(t) => t,
                Err(rc) => return rc,
            };
            let Ok(data) = serde_json::from_str::<Value>(&text) else {
                return codes::BAD_INPUT;
            };
            let d = c.data();
            d.services.emit_event(&d.plugin_id, &name, &data);
            codes::OK
        },
    )
    .map_err(e)?;

    Ok(l)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_parsing_and_grants() {
        assert_eq!(
            Capability::parse("deck.action").unwrap(),
            Capability::DeckAction
        );
        assert_eq!(
            Capability::parse("http:HA.local:8123").unwrap(),
            Capability::Http {
                host: "ha.local".into(),
                port: Some(8123)
            }
        );
        for bad in [
            "http:",
            "http:a b",
            "http:x:notaport",
            "fs.write",
            "http:-x",
        ] {
            assert!(Capability::parse(bad).is_err(), "{bad}");
        }
        let m = Manifest::parse(
            r#"{"id":"p","name":"P","version":"1","capabilities":["deck.action","http:a.lan","notify"]}"#,
        )
        .unwrap();
        let g = Grants::effective(
            &m,
            &[
                "http:a.lan".into(),
                "deck.action".into(),
                "http:b.lan".into(),
            ],
        );
        assert!(g.deck_action && !g.notify);
        assert_eq!(g.http, vec![("a.lan".to_string(), None)]);
        assert!(g.allows_url(&Url::parse("http://A.lan:81/x").unwrap()));
        assert!(!g.allows_url(&Url::parse("http://b.lan/").unwrap()));
    }

    #[test]
    fn manifest_validation() {
        for bad in [
            r#"{"id":"","name":"x","version":"1"}"#,
            r#"{"id":"Bad","name":"x","version":"1"}"#,
            r#"{"id":"../x","name":"x","version":"1"}"#,
            r#"{"id":"ok","name":" ","version":"1"}"#,
            r#"{"id":"ok","name":"x","version":""}"#,
            r#"{"id":"ok","name":"x","version":"1","capabilities":["shell"]}"#,
            r#"{"id":"ok"}"#,
        ] {
            assert!(Manifest::parse(bad).is_err(), "{bad}");
        }
        assert!(Manifest::parse(r#"{"id":"a.b-c_1","name":"x","version":"1"}"#).is_ok());
    }
}
