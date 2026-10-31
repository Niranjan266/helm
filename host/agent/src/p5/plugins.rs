//! `plugins.list` / `plugins.set` (PROTOCOL §13): WASM plugins from `<data_dir>\plugins\<folder>\`
//! (`plugin.json` + `module.wasm`) run in the `helm-plugins` sandbox. The user's choices
//! (enabled, granted capabilities) live in `plugins.json`; a plugin only gets what it both
//! requested and was granted. Plugins see trigger events through their `handle` export.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

use helm_plugins::plugin::{
    Capability, HostServices, LogLevel, MANIFEST_FILE, Manifest, PluginHost, PluginInstance,
    PluginLimits,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::server::AppState;
use crate::store::{DeckAction, read_json, validate_deck_action};
use crate::util;

use super::lock;

pub const PLUGINS_DIR: &str = "plugins";
pub const PLUGINS_FILE: &str = "plugins.json";
const MAX_PLUGINS: usize = 64;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginSetting {
    pub enabled: bool,
    #[serde(default)]
    pub granted: Vec<String>,
}

#[derive(Serialize, Deserialize, Default)]
struct SettingsFile {
    version: u32,
    plugins: BTreeMap<String, PluginSetting>,
}

/// One plugin folder found on disk.
#[derive(Clone, Debug)]
pub struct Found {
    pub folder: PathBuf,
    /// `Err` = unreadable manifest (the id is then the folder name).
    pub manifest: Result<Manifest, String>,
    pub id: String,
}

/// Every plugin folder (sorted by id).
pub fn scan(dir: &Path) -> Vec<Found> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<Found> = rd
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .take(MAX_PLUGINS)
        .map(|e| {
            let folder = e.path();
            let manifest = std::fs::read_to_string(folder.join(MANIFEST_FILE))
                .map_err(|e| format!("{MANIFEST_FILE}: {e}"))
                .and_then(|t| Manifest::parse(&t).map_err(|e| e.to_string()));
            let id = match &manifest {
                Ok(m) => m.id.clone(),
                Err(_) => e.file_name().to_string_lossy().into_owned(),
            };
            Found {
                folder,
                manifest,
                id,
            }
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// What the agent offers plugins: deck actions, `plugin.event` broadcasts, logging.
struct Services {
    state: Weak<AppState>,
    rt: tokio::runtime::Handle,
}

impl HostServices for Services {
    fn log(&self, plugin_id: &str, level: LogLevel, message: &str) {
        match level {
            LogLevel::Error => tracing::error!("plugin {plugin_id}: {message}"),
            LogLevel::Warn => tracing::warn!("plugin {plugin_id}: {message}"),
            LogLevel::Info => tracing::info!("plugin {plugin_id}: {message}"),
            LogLevel::Debug => tracing::debug!("plugin {plugin_id}: {message}"),
        }
    }

    fn run_deck_action(&self, plugin_id: &str, action: &Value) -> Result<(), String> {
        let action: DeckAction =
            serde_json::from_value(action.clone()).map_err(|e| e.to_string())?;
        validate_deck_action(&action)?;
        let state = self.state.upgrade().ok_or("agent is shutting down")?;
        let id = plugin_id.to_string();
        self.rt.spawn(async move {
            if let Err(e) = crate::modules::layout::run_action(&state, action).await {
                tracing::info!("plugin {id}: deck action failed: {}", e.message);
            }
        });
        Ok(())
    }

    fn emit_event(&self, plugin_id: &str, name: &str, data: &Value) {
        if let Some(state) = self.state.upgrade() {
            state.broadcast(
                "plugin.event",
                json!({"plugin_id": plugin_id, "name": name, "data": data}),
            );
        }
    }
}

pub struct Plugins {
    dir: PathBuf,
    settings_path: PathBuf,
    file_lock: Mutex<()>,
    host: Mutex<Option<Arc<PluginHost>>>,
    instances: Mutex<HashMap<String, Arc<Mutex<PluginInstance>>>>,
    errors: Mutex<HashMap<String, String>>,
}

impl Plugins {
    pub fn new(data_dir: &Path) -> Plugins {
        Plugins {
            dir: data_dir.join(PLUGINS_DIR),
            settings_path: data_dir.join(PLUGINS_FILE),
            file_lock: Mutex::new(()),
            host: Mutex::new(None),
            instances: Mutex::new(HashMap::new()),
            errors: Mutex::new(HashMap::new()),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn settings(&self) -> BTreeMap<String, PluginSetting> {
        let _g = lock(&self.file_lock);
        match read_json::<SettingsFile>(&self.settings_path) {
            Ok(Some(f)) => f.plugins,
            Ok(None) => BTreeMap::new(),
            Err(e) => {
                tracing::warn!("{PLUGINS_FILE} unreadable: {e:#}");
                BTreeMap::new()
            }
        }
    }

    fn save_setting(&self, id: &str, setting: PluginSetting) -> anyhow::Result<()> {
        let mut all = self.settings();
        let _g = lock(&self.file_lock);
        all.insert(id.to_string(), setting);
        let file = SettingsFile {
            version: 1,
            plugins: all,
        };
        util::atomic_write(
            &self.settings_path,
            serde_json::to_string_pretty(&file)?.as_bytes(),
        )?;
        Ok(())
    }

    /// `plugins.list` entries.
    pub fn list(&self) -> Vec<Value> {
        let settings = self.settings();
        let errors = lock(&self.errors).clone();
        let running: Vec<String> = lock(&self.instances).keys().cloned().collect();
        scan(&self.dir)
            .into_iter()
            .map(|f| {
                let s = settings.get(&f.id).cloned().unwrap_or_default();
                let (name, version, caps) = match &f.manifest {
                    Ok(m) => (m.name.clone(), m.version.clone(), m.capabilities.clone()),
                    Err(_) => (f.id.clone(), String::new(), Vec::new()),
                };
                let mut v = json!({
                    "id": f.id,
                    "name": name,
                    "version": version,
                    "capabilities": caps,
                    "granted": s.granted,
                    "enabled": s.enabled,
                    "running": running.contains(&f.id),
                });
                let error = match &f.manifest {
                    Err(e) => Some(e.clone()),
                    Ok(_) => errors.get(&f.id).cloned(),
                };
                if let Some(e) = error {
                    v["error"] = Value::String(e);
                }
                v
            })
            .collect()
    }

    fn host(&self, state: &Arc<AppState>) -> Result<Arc<PluginHost>, String> {
        let mut host = lock(&self.host);
        if let Some(h) = host.as_ref() {
            return Ok(h.clone());
        }
        let services = Arc::new(Services {
            state: Arc::downgrade(state),
            rt: tokio::runtime::Handle::current(),
        });
        let h = Arc::new(
            PluginHost::new(PluginLimits::default(), services, state.platform.http())
                .map_err(|e| e.to_string())?,
        );
        *host = Some(h.clone());
        Ok(h)
    }

    /// (Re)load one plugin according to its setting. Blocking (compiles WASM); call on a
    /// blocking thread inside the runtime.
    fn reload(&self, state: &Arc<AppState>, found: &Found, setting: &PluginSetting) {
        let old = lock(&self.instances).remove(&found.id);
        drop(old);
        lock(&self.errors).remove(&found.id);
        if !setting.enabled {
            return;
        }
        let result = (|| -> Result<PluginInstance, String> {
            found.manifest.as_ref().map_err(Clone::clone)?;
            let host = self.host(state)?;
            let pkg = host.load_dir(&found.folder).map_err(|e| e.to_string())?;
            host.instantiate(&pkg, &setting.granted)
                .map_err(|e| e.to_string())
        })();
        match result {
            Ok(inst) => {
                tracing::info!("plugin {} loaded", found.id);
                lock(&self.instances).insert(found.id.clone(), Arc::new(Mutex::new(inst)));
            }
            Err(e) => {
                tracing::warn!("plugin {} failed to load: {e}", found.id);
                lock(&self.errors).insert(found.id.clone(), e);
            }
        }
    }

    /// Load every enabled plugin (agent start). Blocking.
    pub fn load_all(&self, state: &Arc<AppState>) {
        let settings = self.settings();
        for f in scan(&self.dir) {
            let s = settings.get(&f.id).cloned().unwrap_or_default();
            if s.enabled {
                self.reload(state, &f, &s);
            }
        }
    }

    /// `plugins.set`: validate, save, reload. Blocking.
    pub fn set(
        &self,
        state: &Arc<AppState>,
        id: &str,
        enabled: bool,
        granted: Vec<String>,
    ) -> Result<(), String> {
        let found = scan(&self.dir)
            .into_iter()
            .find(|f| f.id == id)
            .ok_or_else(|| format!("no plugin {id:?} in {}", self.dir.display()))?;
        if granted.len() > 32 {
            return Err("at most 32 granted capabilities".into());
        }
        for g in &granted {
            Capability::parse(g).map_err(|e| e.to_string())?;
        }
        let setting = PluginSetting { enabled, granted };
        self.save_setting(id, setting.clone())
            .map_err(|e| format!("could not save {PLUGINS_FILE}: {e}"))?;
        self.reload(state, &found, &setting);
        Ok(())
    }

    /// Give an event (JSON) to every running plugin that exports `handle`. Blocking.
    pub fn dispatch(&self, event: &Value) {
        let input = event.to_string();
        let instances: Vec<(String, Arc<Mutex<PluginInstance>>)> = lock(&self.instances)
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (id, inst) in instances {
            let mut inst = lock(&inst);
            if !inst.has_export("handle") {
                continue;
            }
            if let Err(e) = inst.handle(input.as_bytes()) {
                tracing::warn!("plugin {id} failed: {e}");
                if inst.is_poisoned() {
                    drop(inst);
                    lock(&self.instances).remove(&id);
                    lock(&self.errors).insert(id, e.to_string());
                }
            }
        }
    }

    pub fn has_running(&self) -> bool {
        !lock(&self.instances).is_empty()
    }
}
