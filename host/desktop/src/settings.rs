//! Desktop-only settings (`desktop.json` in the data folder). Relay settings live in the
//! agent's own `relay.json`; Start with Windows lives in the registry.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const SETTINGS_FILE: &str = "desktop.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// TCP port the agent listens on (0 = any free port).
    pub port: u16,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            port: helm_agent::DEFAULT_PORT,
        }
    }
}

fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(SETTINGS_FILE)
}

impl Settings {
    /// Missing or unreadable file = defaults.
    pub fn load(data_dir: &Path) -> Settings {
        std::fs::read_to_string(path(data_dir))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, data_dir: &Path) -> anyhow::Result<()> {
        std::fs::create_dir_all(data_dir)?;
        let text = serde_json::to_string_pretty(self)?;
        helm_agent::util::atomic_write(&path(data_dir), text.as_bytes())?;
        Ok(())
    }
}

/// Ports a user may pick: 0 (any free port) or 1024–65535 (no privileged ports).
pub fn validate_port(port: u32) -> Result<u16, String> {
    match port {
        0 | 1024..=65535 => Ok(port as u16),
        _ => Err("Use 0 (any free port) or a port from 1024 to 65535.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_or_bad_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Settings::load(dir.path()).port, helm_agent::DEFAULT_PORT);
        std::fs::write(dir.path().join(SETTINGS_FILE), "not json").unwrap();
        assert_eq!(Settings::load(dir.path()), Settings::default());
        std::fs::write(dir.path().join(SETTINGS_FILE), "{}").unwrap();
        assert_eq!(Settings::load(dir.path()), Settings::default());
    }

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        Settings { port: 50000 }.save(dir.path()).unwrap();
        assert_eq!(Settings::load(dir.path()).port, 50000);
    }

    #[test]
    fn port_rules() {
        assert_eq!(validate_port(0), Ok(0));
        assert_eq!(validate_port(47820), Ok(47820));
        assert!(validate_port(80).is_err());
        assert!(validate_port(70000).is_err());
    }
}
