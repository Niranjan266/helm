//! The PC's long-lived identity: host id, self-signed certificate and its private key.

use std::path::Path;

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{dpapi, util};

pub const IDENTITY_FILE: &str = "identity.json";
/// The same identity with the key sealed for this computer (DPAPI machine scope), written
/// by the desktop app when "Stay reachable after restart" is turned on so the sign-in
/// service can serve before anyone signs in. Removed again when it is turned off.
pub const SERVICE_IDENTITY_FILE: &str = "service-identity.json";

#[derive(Clone)]
pub struct Identity {
    pub host_id: String,
    pub host_name: String,
    /// Leaf certificate, DER.
    pub cert_der: Vec<u8>,
    /// Private key, PKCS#8 DER (decrypted, kept in memory only).
    pub key_pkcs8: Vec<u8>,
    /// Lowercase hex SHA-256 of `cert_der`.
    pub fingerprint: String,
}

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Identity")
            .field("host_id", &self.host_id)
            .field("host_name", &self.host_name)
            .field("fingerprint", &self.fingerprint)
            .finish_non_exhaustive()
    }
}

#[derive(Serialize, Deserialize)]
struct IdentityFile {
    version: u32,
    host_id: String,
    /// base64 standard of the certificate DER.
    cert_der: String,
    key: StoredKey,
}

#[derive(Serialize, Deserialize)]
struct StoredKey {
    /// "dpapi" (Windows, current user), "dpapi-machine" (Windows, this computer) or "none".
    protection: String,
    /// base64 standard of the (possibly encrypted) PKCS#8 DER.
    data: String,
}

pub fn fingerprint_of(cert_der: &[u8]) -> String {
    hex::encode(Sha256::digest(cert_der))
}

/// Windows computer name (falls back to "Helm-PC").
pub fn computer_name() -> String {
    hostname::get()
        .ok()
        .map(|h| h.to_string_lossy().trim().to_string())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "Helm-PC".to_string())
}

impl Identity {
    /// Load identity.json from `data_dir`, or create a new identity the first time.
    pub fn load_or_create(data_dir: &Path, host_name: String) -> anyhow::Result<Identity> {
        let path = data_dir.join(IDENTITY_FILE);
        if path.exists() {
            return Self::load(&path, host_name)
                .with_context(|| format!("reading {}", path.display()));
        }
        let identity = Self::generate(host_name)?;
        identity.save(&path)?;
        tracing::info!("created new host identity {}", identity.host_id);
        Ok(identity)
    }

    /// Load identity.json from `data_dir`; never creates one (the sign-in service).
    pub fn load_existing(data_dir: &Path, host_name: String) -> anyhow::Result<Identity> {
        let path = data_dir.join(IDENTITY_FILE);
        Self::load(&path, host_name).with_context(|| format!("reading {}", path.display()))
    }

    /// Write this identity to `path` with the key sealed for this computer
    /// ([`dpapi::protect_machine`]). Only call it for a file inside the user's own data
    /// folder (readable by the user, SYSTEM and administrators only).
    pub fn save_for_service(&self, path: &Path) -> anyhow::Result<()> {
        let protected = dpapi::protect_machine(&self.key_pkcs8)?;
        let scheme = if cfg!(windows) {
            dpapi::MACHINE_SCHEME
        } else {
            "none"
        };
        self.write_file(path, scheme, &protected)
    }

    fn load(path: &Path, host_name: String) -> anyhow::Result<Identity> {
        let text = std::fs::read_to_string(path)?;
        let file: IdentityFile = serde_json::from_str(&text)?;
        if file.version != 1 {
            bail!("unsupported identity version {}", file.version);
        }
        let cert_der = util::b64_decode(&file.cert_der).context("bad cert_der")?;
        let stored = util::b64_decode(&file.key.data).context("bad key data")?;
        let key_pkcs8 = match file.key.protection.as_str() {
            "dpapi" | dpapi::MACHINE_SCHEME => {
                #[cfg(windows)]
                {
                    dpapi::unprotect(&stored).context(
                        "could not decrypt the private key with DPAPI (was identity.json \
                         copied from another Windows user or PC?)",
                    )?
                }
                #[cfg(not(windows))]
                bail!("identity.json key is DPAPI-protected; only Windows can read it")
            }
            "none" => stored,
            other => bail!("unknown key protection {other:?}"),
        };
        uuid::Uuid::parse_str(&file.host_id).context("bad host_id")?;
        Ok(Identity {
            fingerprint: fingerprint_of(&cert_der),
            host_id: file.host_id,
            host_name,
            cert_der,
            key_pkcs8,
        })
    }

    fn generate(host_name: String) -> anyhow::Result<Identity> {
        let key_pair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
        let mut params = rcgen::CertificateParams::new(vec!["helm-agent.local".to_string()])?;
        let mut dn = rcgen::DistinguishedName::new();
        dn.push(rcgen::DnType::CommonName, "Helm Agent");
        dn.push(rcgen::DnType::OrganizationName, "Helm");
        params.distinguished_name = dn;
        // "Kept forever": a validity window far into the future. The phone pins the
        // fingerprint and does not check dates.
        params.not_before = rcgen::date_time_ymd(2024, 1, 1);
        params.not_after = rcgen::date_time_ymd(2124, 1, 1);
        let cert = params.self_signed(&key_pair)?;
        let cert_der = cert.der().to_vec();
        Ok(Identity {
            host_id: uuid::Uuid::new_v4().to_string(),
            host_name,
            fingerprint: fingerprint_of(&cert_der),
            cert_der,
            key_pkcs8: key_pair.serialize_der(),
        })
    }

    fn save(&self, path: &Path) -> anyhow::Result<()> {
        let protected = dpapi::protect(&self.key_pkcs8)?;
        self.write_file(path, dpapi::SCHEME, &protected)
    }

    fn write_file(&self, path: &Path, scheme: &str, protected: &[u8]) -> anyhow::Result<()> {
        let file = IdentityFile {
            version: 1,
            host_id: self.host_id.clone(),
            cert_der: util::b64_encode(&self.cert_der),
            key: StoredKey {
                protection: scheme.to_string(),
                data: util::b64_encode(protected),
            },
        };
        util::atomic_write(path, serde_json::to_string_pretty(&file)?.as_bytes())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_created_once_and_reused() {
        let dir = tempfile::tempdir().unwrap();
        let a = Identity::load_or_create(dir.path(), "PC".into()).unwrap();
        let b = Identity::load_or_create(dir.path(), "PC".into()).unwrap();
        assert_eq!(a.host_id, b.host_id);
        assert_eq!(a.fingerprint, b.fingerprint);
        assert_eq!(a.key_pkcs8, b.key_pkcs8);
        assert_eq!(a.fingerprint.len(), 64);
        assert!(
            a.fingerprint
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        let text = std::fs::read_to_string(dir.path().join(IDENTITY_FILE)).unwrap();
        assert!(text.contains(dpapi::SCHEME));
    }

    #[test]
    fn load_existing_never_creates() {
        let dir = tempfile::tempdir().unwrap();
        assert!(Identity::load_existing(dir.path(), "PC".into()).is_err());
        assert!(!dir.path().join(IDENTITY_FILE).exists());
    }

    #[test]
    fn service_copy_is_the_same_identity() {
        let user = tempfile::tempdir().unwrap();
        let service = tempfile::tempdir().unwrap();
        let a = Identity::load_or_create(user.path(), "PC".into()).unwrap();
        a.save_for_service(&service.path().join(IDENTITY_FILE)).unwrap();
        let text = std::fs::read_to_string(service.path().join(IDENTITY_FILE)).unwrap();
        if cfg!(windows) {
            assert!(text.contains(dpapi::MACHINE_SCHEME));
        }
        let b = Identity::load_existing(service.path(), "PC".into()).unwrap();
        assert_eq!(a.host_id, b.host_id);
        assert_eq!(a.fingerprint, b.fingerprint);
        assert_eq!(a.key_pkcs8, b.key_pkcs8);
    }
}
