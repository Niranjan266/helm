//! Home Assistant (PROTOCOL §13): `ha.config.set{url,token}` stores the long-lived token
//! encrypted with DPAPI (like the identity key) in `ha.json`; `ha.states` and the deck action
//! `{"ha":{...}}` use `helm_plugins::ha::HaClient` over the platform's HTTP(S) transport.

use std::path::{Path, PathBuf};

use helm_plugins::ha::{HaClient, HaError};
use serde::{Deserialize, Serialize};

use crate::router::RpcError;
use crate::store::read_json;
use crate::{dpapi, util};

pub const HA_FILE: &str = "ha.json";

#[derive(Serialize, Deserialize)]
struct HaFile {
    version: u32,
    url: String,
    /// `dpapi` on Windows (`none` on development builds elsewhere).
    token_protection: String,
    /// Base64 of the protected token.
    token: String,
}

/// URL + token in memory.
#[derive(Clone)]
pub struct HaConfig {
    pub url: String,
    pub token: String,
}

impl std::fmt::Debug for HaConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HaConfig")
            .field("url", &self.url)
            .field("token", &"<redacted>")
            .finish()
    }
}

pub fn path(dir: &Path) -> PathBuf {
    dir.join(HA_FILE)
}

pub fn save(dir: &Path, config: &HaConfig) -> anyhow::Result<()> {
    let sealed = dpapi::protect(config.token.as_bytes())?;
    let file = HaFile {
        version: 1,
        url: config.url.clone(),
        token_protection: dpapi::SCHEME.to_string(),
        token: util::b64_encode(&sealed),
    };
    util::atomic_write(&path(dir), serde_json::to_string_pretty(&file)?.as_bytes())?;
    Ok(())
}

pub fn load(dir: &Path) -> anyhow::Result<Option<HaConfig>> {
    let Some(file) = read_json::<HaFile>(&path(dir))? else {
        return Ok(None);
    };
    if file.token_protection != dpapi::SCHEME {
        anyhow::bail!("{HA_FILE} was written with {:?} protection", file.token_protection);
    }
    let sealed = util::b64_decode(&file.token)
        .ok_or_else(|| anyhow::anyhow!("{HA_FILE}: token is not base64"))?;
    let token = String::from_utf8(dpapi::unprotect(&sealed)?)?;
    Ok(Some(HaConfig {
        url: file.url,
        token,
    }))
}

pub fn ha_err(e: HaError) -> RpcError {
    match e {
        HaError::InvalidConfig(m) => RpcError::bad_params(format!("Home Assistant: {m}")),
        HaError::InvalidName(n) => RpcError::bad_params(format!("Home Assistant: invalid name {n:?}")),
        HaError::BadRequest(m) => RpcError::bad_params(format!("Home Assistant: bad request: {m}")),
        HaError::NotFound => RpcError::bad_params("Home Assistant: not found"),
        other => RpcError::internal(format!("Home Assistant: {other}")),
    }
}

/// Build a client (checks the URL and token shape).
pub fn client(
    config: &HaConfig,
    http: std::sync::Arc<dyn crate::platform::HttpTransport>,
) -> Result<HaClient, RpcError> {
    HaClient::new(&config.url, &config.token, http).map_err(ha_err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_encrypted_at_rest_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(dir.path()).unwrap().is_none());
        let cfg = HaConfig {
            url: "http://ha.local:8123".into(),
            token: "secret-token-123".into(),
        };
        save(dir.path(), &cfg).unwrap();
        let text = std::fs::read_to_string(path(dir.path())).unwrap();
        assert!(!text.contains("secret-token-123"), "{text}");
        let back = load(dir.path()).unwrap().unwrap();
        assert_eq!((back.url.as_str(), back.token.as_str()), ("http://ha.local:8123", "secret-token-123"));
        assert!(!format!("{back:?}").contains("secret"));
    }
}
