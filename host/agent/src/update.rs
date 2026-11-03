//! Phase 5 update check (`helm-update`): GitHub Releases + SHA-256 + Ed25519.
//!
//! This module only checks and, on request, downloads and verifies the installer. Running it is
//! the desktop app's job, and only when the user asks (`host/desktop/src/updater.rs`). Builds
//! without a compiled-in release key refuse to check at all.

use std::path::PathBuf;

pub use helm_update::release::WINDOWS_SETUP_ASSET;
pub use helm_update::{Asset, UpdateCandidate, Version};
use helm_update::{Fetcher, UpdateConfig, Updater};

use crate::proto::AGENT_VERSION;

/// Result of an update check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStatus {
    /// Updates are refused in this build (no release key compiled in).
    Disabled,
    UpToDate,
    Available(Box<UpdateCandidate>),
    Failed(String),
}

impl UpdateStatus {
    /// One line for the console / log / message box.
    pub fn summary(&self) -> String {
        match self {
            UpdateStatus::Disabled => "Updates are disabled in this build: no release signing key \
                                       was compiled in (HELM_UPDATE_PUBKEY)."
                .to_string(),
            UpdateStatus::UpToDate => format!("Helm {AGENT_VERSION} is up to date."),
            UpdateStatus::Available(c) => {
                format!(
                    "Helm {} is available (you have {AGENT_VERSION}).",
                    c.version
                )
            }
            UpdateStatus::Failed(e) => format!("Update check failed: {e}"),
        }
    }
}

/// The running agent's version.
pub fn current_version() -> Version {
    Version::parse(AGENT_VERSION).unwrap_or(Version::new(0, 0, 0))
}

pub fn config() -> UpdateConfig {
    UpdateConfig::new(current_version())
}

/// Check with any updater (tests pass an in-memory fetcher and a test key).
pub fn check_with<F: Fetcher>(updater: &Updater<F>) -> UpdateStatus {
    match updater.check() {
        Ok(Some(c)) => UpdateStatus::Available(Box::new(c)),
        Ok(None) => UpdateStatus::UpToDate,
        Err(e) => UpdateStatus::Failed(e.to_string()),
    }
}

fn real_updater() -> Result<Updater<helm_update::HttpFetcher>, helm_update::Error> {
    Updater::new(
        config(),
        helm_update::HttpFetcher::new(format!("helm-agent/{AGENT_VERSION}")),
    )
}

/// Check GitHub Releases (blocking; call from `spawn_blocking`). Refuses without a key.
pub fn check() -> UpdateStatus {
    match real_updater() {
        Ok(up) => check_with(&up),
        Err(helm_update::Error::NoPublicKey) | Err(helm_update::Error::BadPublicKey) => {
            UpdateStatus::Disabled
        }
        Err(e) => UpdateStatus::Failed(e.to_string()),
    }
}

/// Download and verify the installer of `c` (blocking). The caller shows it to the user;
/// nothing is run.
pub fn download(c: &UpdateCandidate) -> Result<PathBuf, String> {
    let up = real_updater().map_err(|e| e.to_string())?;
    up.download_and_verify(c).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    struct Fixture(Vec<u8>);
    impl Fetcher for Fixture {
        fn open(
            &self,
            url: &str,
            _accept: &str,
        ) -> Result<Box<dyn Read + Send>, helm_update::Error> {
            assert!(url.starts_with("https://"));
            Ok(Box::new(std::io::Cursor::new(self.0.clone())))
        }
    }

    fn releases(tag: &str) -> Vec<u8> {
        let asset = |name: &str| {
            serde_json::json!({
                "name": name,
                "size": 10,
                "browser_download_url": format!("https://example.invalid/{name}"),
            })
        };
        let pkg = format!("Helm-Setup-{}-x64.exe", tag.trim_start_matches('v'));
        serde_json::to_vec(&serde_json::json!([{
            "tag_name": tag,
            "draft": false,
            "prerelease": false,
            "html_url": "https://example.invalid/release",
            "assets": [asset(&pkg), asset(&format!("{pkg}.sig")), asset(&format!("{pkg}.sha256"))],
        }]))
        .unwrap()
    }

    fn updater(json: Vec<u8>) -> Updater<Fixture> {
        let key = helm_update::SigningKey::from_bytes(&[7u8; 32]).verifying_key();
        Updater::with_key(config(), key, Fixture(json))
    }

    #[test]
    fn no_key_means_disabled() {
        // Only in a build with no key (HELM_UPDATE_PUBKEY=""); the check then never hits the net.
        if helm_update::compiled_public_key().is_none() {
            assert_eq!(check(), UpdateStatus::Disabled);
            assert!(check().summary().contains("disabled"));
        }
    }

    #[test]
    fn no_key_refuses_at_the_updater_too() {
        let up = Updater::with_key_hex(config(), Some(""), Fixture(Vec::new()));
        assert!(matches!(up, Err(helm_update::Error::NoPublicKey)));
    }

    #[test]
    fn newer_release_is_offered_older_is_not() {
        match check_with(&updater(releases("v99.0.0"))) {
            UpdateStatus::Available(c) => {
                assert_eq!(c.version, Version::new(99, 0, 0));
                assert_eq!(
                    c.page_url.as_deref(),
                    Some("https://example.invalid/release")
                );
            }
            other => panic!("expected an update, got {other:?}"),
        }
        assert_eq!(
            check_with(&updater(releases("v0.0.1"))),
            UpdateStatus::UpToDate
        );
        assert!(matches!(
            check_with(&updater(b"not json".to_vec())),
            UpdateStatus::Failed(_)
        ));
    }
}
