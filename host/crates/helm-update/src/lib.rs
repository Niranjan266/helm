//! Helm update checker.
//!
//! 1. [`Updater::check`] fetches the GitHub Releases list for a configurable
//!    repository and picks the newest stable x64 package newer than the
//!    running version ([`select_update`]).
//! 2. [`Updater::download_and_verify`] downloads it to a temp folder, checks
//!    its SHA-256 against the `<asset>.sha256` sidecar and its Ed25519
//!    signature (`<asset>.sig`) against a public key compiled into the
//!    binary. Only a file that passes both checks is left on disk.
//!
//! The release key is compiled in from the `HELM_UPDATE_PUBKEY` environment
//! variable (64 hex chars) at build time. Without it, [`compiled_public_key`]
//! is `None` and updates cannot be verified, so they are refused.

pub mod fetch;
pub mod release;
pub mod verify;
pub mod version;

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub use ed25519_dalek::{Signature, SigningKey, VerifyingKey};
pub use fetch::Fetcher;
#[cfg(feature = "http")]
pub use fetch::HttpFetcher;
pub use release::{Arch, Asset, Release, UpdateCandidate, parse_releases, select_update};
pub use version::{Version, VersionError};

/// Hex public key baked in at build time (`HELM_UPDATE_PUBKEY`), if any.
pub const COMPILED_PUBLIC_KEY_HEX: Option<&str> = option_env!("HELM_UPDATE_PUBKEY");

/// Default `owner/repo`, overridable at build time with `HELM_UPDATE_REPO`.
pub const DEFAULT_REPO: &str = match option_env!("HELM_UPDATE_REPO") {
    Some(r) => r,
    None => "niranjan266/helm",
};

/// Upper bound for the releases JSON and for sidecar files.
const MAX_JSON_BYTES: u64 = 4 * 1024 * 1024;
const MAX_SIDECAR_BYTES: u64 = 4 * 1024;
/// Upper bound for a package when GitHub reports no size.
const MAX_PACKAGE_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no release public key was compiled in (set HELM_UPDATE_PUBKEY at build time)")]
    NoPublicKey,
    #[error("invalid Ed25519 public key")]
    BadPublicKey,
    #[error("invalid repository {0:?}, expected owner/repo")]
    BadRepo(String),
    #[error("refusing non-HTTPS URL {0}")]
    InsecureUrl(String),
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("response larger than {0} bytes")]
    TooLarge(u64),
    #[error("could not parse releases JSON: {0}")]
    BadReleaseJson(#[from] serde_json::Error),
    #[error("malformed .sha256 file")]
    BadChecksumFile,
    #[error("malformed .sig file")]
    BadSignatureFile,
    #[error("unsafe asset file name {0:?}")]
    BadAssetName(String),
    #[error("downloaded {got} bytes, release says {expected}")]
    SizeMismatch { expected: u64, got: u64 },
    #[error("SHA-256 mismatch")]
    ChecksumMismatch,
    #[error("Ed25519 signature does not verify")]
    SignatureMismatch,
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// The public key compiled into this build, if a valid one was provided.
pub fn compiled_public_key() -> Option<VerifyingKey> {
    COMPILED_PUBLIC_KEY_HEX.and_then(|h| verify::parse_public_key(h).ok())
}

#[derive(Debug, Clone)]
pub struct UpdateConfig {
    /// `owner/repo` on GitHub.
    pub repo: String,
    /// API root, normally `https://api.github.com`.
    pub api_base: String,
    pub arch: Arch,
    pub current: Version,
    /// Folder that receives verified downloads.
    pub download_dir: PathBuf,
}

impl UpdateConfig {
    pub fn new(current: Version) -> Self {
        Self {
            repo: DEFAULT_REPO.to_string(),
            api_base: "https://api.github.com".to_string(),
            arch: Arch::X64,
            current,
            download_dir: std::env::temp_dir().join("helm-update"),
        }
    }

    pub fn releases_url(&self) -> Result<String, Error> {
        let ok = self
            .repo
            .split_once('/')
            .is_some_and(|(o, r)| valid_slug(o) && valid_slug(r));
        if !ok {
            return Err(Error::BadRepo(self.repo.clone()));
        }
        Ok(format!(
            "{}/repos/{}/releases?per_page=30",
            self.api_base.trim_end_matches('/'),
            self.repo
        ))
    }
}

fn valid_slug(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

pub struct Updater<F: Fetcher> {
    config: UpdateConfig,
    key: VerifyingKey,
    fetcher: F,
}

impl<F: Fetcher> Updater<F> {
    /// Uses the compiled-in release key.
    pub fn new(config: UpdateConfig, fetcher: F) -> Result<Self, Error> {
        let key = compiled_public_key().ok_or(Error::NoPublicKey)?;
        Ok(Self::with_key(config, key, fetcher))
    }

    /// Uses an explicit key (tests, forks with their own signing key).
    pub fn with_key(config: UpdateConfig, key: VerifyingKey, fetcher: F) -> Self {
        Self {
            config,
            key,
            fetcher,
        }
    }

    pub fn config(&self) -> &UpdateConfig {
        &self.config
    }

    /// Returns the newest applicable release, or `None` when up to date.
    pub fn check(&self) -> Result<Option<UpdateCandidate>, Error> {
        let url = self.config.releases_url()?;
        let body = fetch::read_limited(
            self.fetcher.open(&url, "application/vnd.github+json")?,
            MAX_JSON_BYTES,
        )?;
        let releases = parse_releases(&body)?;
        Ok(select_update(
            &releases,
            &self.config.current,
            self.config.arch,
        ))
    }

    /// Downloads the package and returns its path only if both the SHA-256
    /// and the Ed25519 signature verify. Partial or bad files are removed.
    pub fn download_and_verify(&self, c: &UpdateCandidate) -> Result<PathBuf, Error> {
        let name = &c.package.name;
        if !safe_file_name(name) {
            return Err(Error::BadAssetName(name.clone()));
        }
        for a in [&c.package, &c.signature, &c.checksum] {
            fetch::require_https(&a.browser_download_url)?;
        }

        let sidecar = |a: &Asset| -> Result<Vec<u8>, Error> {
            let r = self
                .fetcher
                .open(&a.browser_download_url, "application/octet-stream")?;
            fetch::read_limited(r, MAX_SIDECAR_BYTES)
        };
        let expected = verify::parse_checksum(&sidecar(&c.checksum)?)?;
        let sig = verify::parse_signature(&sidecar(&c.signature)?)?;

        fs::create_dir_all(&self.config.download_dir)?;
        let final_path = self.config.download_dir.join(name);
        let part_path = self.config.download_dir.join(format!("{name}.part"));

        let result = self.download_to(c, &part_path, &expected, &sig);
        match result {
            Ok(()) => {
                if final_path.exists() {
                    fs::remove_file(&final_path)?;
                }
                fs::rename(&part_path, &final_path)?;
                Ok(final_path)
            }
            Err(e) => {
                let _ = fs::remove_file(&part_path);
                Err(e)
            }
        }
    }

    fn download_to(
        &self,
        c: &UpdateCandidate,
        part: &Path,
        expected: &[u8; 32],
        sig: &Signature,
    ) -> Result<(), Error> {
        let limit = if c.package.size > 0 {
            c.package.size
        } else {
            MAX_PACKAGE_BYTES
        };
        let mut reader = self
            .fetcher
            .open(&c.package.browser_download_url, "application/octet-stream")?;
        let mut file = fs::File::create(part)?;
        let mut hashing = HashingWriter::new(&mut file);
        let got = io::copy(&mut io::Read::take(&mut reader, limit + 1), &mut hashing)?;
        let digest = hashing.finish();
        file.flush()?;
        file.sync_all()?;
        drop(file);

        if got > limit {
            return Err(Error::TooLarge(limit));
        }
        if c.package.size > 0 && got != c.package.size {
            return Err(Error::SizeMismatch {
                expected: c.package.size,
                got,
            });
        }
        if &digest != expected {
            return Err(Error::ChecksumMismatch);
        }
        verify::verify_signature(&self.key, &c.package.name, &digest, sig)
    }
}

/// A plain file name: no separators, no `..`, no drive or reserved chars.
fn safe_file_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 200
        && !name.starts_with('.')
        && !name.contains("..")
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "-_.+ ".contains(ch))
}

struct HashingWriter<W: Write> {
    inner: W,
    hasher: sha2::Sha256,
}

impl<W: Write> HashingWriter<W> {
    fn new(inner: W) -> Self {
        use sha2::Digest;
        Self {
            inner,
            hasher: sha2::Sha256::new(),
        }
    }
    fn finish(self) -> [u8; 32] {
        use sha2::Digest;
        self.hasher.finalize().into()
    }
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        use sha2::Digest;
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}
