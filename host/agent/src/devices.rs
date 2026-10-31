//! Paired (trusted) devices, persisted in devices.json.
//!
//! Every operation reads the file, changes it and writes it back atomically, so the
//! `helm-agent devices` / `helm-agent revoke` commands and a running agent see the same data.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::router::Tier;
use crate::util;

pub const DEVICES_FILE: &str = "devices.json";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Device {
    pub device_id: String,
    pub device_name: String,
    /// base64 standard of the X.509 SubjectPublicKeyInfo DER.
    pub public_key: String,
    pub tier: Tier,
    pub paired_at_ms: i64,
    pub last_seen_ms: i64,
}

#[derive(Default, Serialize, Deserialize)]
struct DevicesFile {
    version: u32,
    devices: Vec<Device>,
}

pub struct DeviceStore {
    path: PathBuf,
    lock: Mutex<()>,
}

impl DeviceStore {
    pub fn new(data_dir: &Path) -> DeviceStore {
        DeviceStore {
            path: data_dir.join(DEVICES_FILE),
            lock: Mutex::new(()),
        }
    }

    fn read(&self) -> anyhow::Result<Vec<Device>> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) if text.trim().is_empty() => Ok(Vec::new()),
            Ok(text) => Ok(serde_json::from_str::<DevicesFile>(&text)?.devices),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e.into()),
        }
    }

    fn write(&self, devices: Vec<Device>) -> anyhow::Result<()> {
        let file = DevicesFile {
            version: 1,
            devices,
        };
        util::atomic_write(&self.path, serde_json::to_string_pretty(&file)?.as_bytes())?;
        Ok(())
    }

    fn modify<R>(&self, f: impl FnOnce(&mut Vec<Device>) -> R) -> anyhow::Result<R> {
        let _guard = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut devices = self.read()?;
        let result = f(&mut devices);
        self.write(devices)?;
        Ok(result)
    }

    pub fn list(&self) -> anyhow::Result<Vec<Device>> {
        let _guard = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        self.read()
    }

    pub fn get(&self, device_id: &str) -> anyhow::Result<Option<Device>> {
        Ok(self.list()?.into_iter().find(|d| d.device_id == device_id))
    }

    pub fn is_paired(&self, device_id: &str) -> bool {
        matches!(self.get(device_id), Ok(Some(_)))
    }

    /// Add a device, or replace key/name/tier if the same device_id pairs again.
    pub fn upsert(&self, device: Device) -> anyhow::Result<()> {
        self.modify(|devices| {
            if let Some(existing) = devices.iter_mut().find(|d| d.device_id == device.device_id) {
                *existing = device;
            } else {
                devices.push(device);
            }
        })
    }

    pub fn touch(&self, device_id: &str, now_ms: i64) -> anyhow::Result<()> {
        self.modify(|devices| {
            if let Some(d) = devices.iter_mut().find(|d| d.device_id == device_id) {
                d.last_seen_ms = now_ms;
            }
        })
    }

    /// Remove a device. Returns false if it was not paired.
    pub fn revoke(&self, device_id: &str) -> anyhow::Result<bool> {
        self.modify(|devices| {
            let before = devices.len();
            devices.retain(|d| d.device_id != device_id);
            devices.len() != before
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(id: &str, name: &str) -> Device {
        Device {
            device_id: id.into(),
            device_name: name.into(),
            public_key: "AAAA".into(),
            tier: Tier::Admin,
            paired_at_ms: 1,
            last_seen_ms: 1,
        }
    }

    #[test]
    fn upsert_touch_revoke() {
        let dir = tempfile::tempdir().unwrap();
        let store = DeviceStore::new(dir.path());
        assert!(store.list().unwrap().is_empty());
        store.upsert(dev("a", "one")).unwrap();
        store.upsert(dev("a", "renamed")).unwrap();
        store.upsert(dev("b", "two")).unwrap();
        let list = store.list().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].device_name, "renamed");
        store.touch("a", 99).unwrap();
        assert_eq!(store.get("a").unwrap().unwrap().last_seen_ms, 99);
        // A second store on the same folder sees the same data.
        let other = DeviceStore::new(dir.path());
        assert!(other.revoke("a").unwrap());
        assert!(!store.revoke("a").unwrap());
        assert!(!store.is_paired("a"));
        assert!(store.is_paired("b"));
    }
}
