//! Offline tests: fixture release JSON, an in-memory fetcher, a fixed test key.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use helm_update::verify::{
    parse_checksum, parse_public_key, parse_signature, sha256_bytes, sign_digest, verify_signature,
};
use helm_update::{
    Arch, Asset, Error, Fetcher, SigningKey, UpdateConfig, Updater, Version, parse_releases,
    select_update,
};

const FIXTURE: &[u8] = include_bytes!("fixtures/releases.json");
const API: &str = "https://api.example.test";
const PKG_URL: &str =
    "https://github.com/example/helm/releases/download/v0.3.0/Helm-Setup-0.3.0-x64.exe";
const PKG_NAME: &str = "Helm-Setup-0.3.0-x64.exe";

fn test_key() -> SigningKey {
    SigningKey::from_bytes(&[7u8; 32])
}

fn v(s: &str) -> Version {
    Version::parse(s).unwrap()
}

// ---------------------------------------------------------------- versions

#[test]
fn parses_versions() {
    assert_eq!(v("1.2.3"), Version::new(1, 2, 3));
    assert_eq!(v("v1.2.3"), Version::new(1, 2, 3));
    assert_eq!(v("1.2.3+build.9"), Version::new(1, 2, 3));
    assert!(!v("1.2.3-beta.1").is_stable());
    assert_eq!(v("1.2.3-beta.1").to_string(), "1.2.3-beta.1");
    for bad in [
        "",
        "1",
        "1.2",
        "1.2.3.4",
        "01.2.3",
        "1.2.x",
        "1.2.3-",
        "1.2.3-a..b",
        "1.2.3-01",
        "nightly",
        "1.2.3+",
    ] {
        assert!(Version::parse(bad).is_err(), "{bad:?} should be rejected");
    }
}

#[test]
fn semver_precedence() {
    // The precedence chain from semver.org section 11.
    let chain = [
        "1.0.0-alpha",
        "1.0.0-alpha.1",
        "1.0.0-alpha.beta",
        "1.0.0-beta",
        "1.0.0-beta.2",
        "1.0.0-beta.11",
        "1.0.0-rc.1",
        "1.0.0",
        "1.0.1",
        "1.1.0",
        "2.0.0",
        "10.0.0",
    ];
    for pair in chain.windows(2) {
        assert!(v(pair[0]) < v(pair[1]), "{} < {}", pair[0], pair[1]);
    }
    assert!(v("0.2.10") > v("0.2.9"));
    assert_eq!(v("1.0.0+a").cmp(&v("1.0.0+b")), std::cmp::Ordering::Equal);
}

// ---------------------------------------------------------------- selection

#[test]
fn picks_newest_stable_x64_installer() {
    let releases = parse_releases(FIXTURE).unwrap();
    let c = select_update(&releases, &v("0.2.0"), Arch::X64).unwrap();
    assert_eq!(c.version, v("0.3.0"));
    assert_eq!(
        c.package.name, PKG_NAME,
        "prefers .exe over .zip, skips arm64"
    );
    assert_eq!(c.signature.name, format!("{PKG_NAME}.sig"));
    assert_eq!(c.checksum.name, format!("{PKG_NAME}.sha256"));
    assert_eq!(
        c.notes.as_deref(),
        Some("Phone file browser, media controls.")
    );
}

#[test]
fn up_to_date_returns_none() {
    let releases = parse_releases(FIXTURE).unwrap();
    assert!(select_update(&releases, &v("0.3.0"), Arch::X64).is_none());
    // A newer local build (e.g. a dev build) never "updates" backwards.
    assert!(select_update(&releases, &v("9.0.0"), Arch::X64).is_none());
}

#[test]
fn skips_releases_without_signature() {
    let releases: Vec<_> = parse_releases(FIXTURE)
        .unwrap()
        .into_iter()
        .filter(|r| r.tag_name != "v0.3.0")
        .collect();
    // 0.2.10 lacks a .sig, so 0.2.9 is the newest verifiable release.
    let c = select_update(&releases, &v("0.2.0"), Arch::X64).unwrap();
    assert_eq!(c.version, v("0.2.9"));
}

#[test]
fn arch_token_matching() {
    let asset = |n: &str| Asset {
        name: n.into(),
        browser_download_url: format!("https://x/{n}"),
        size: 1,
    };
    let pick = |names: &[&str]| {
        let assets: Vec<_> = names.iter().map(|n| asset(n)).collect();
        helm_update::release::pick_package(&assets, Arch::X64).map(|a| a.name.clone())
    };
    assert_eq!(
        pick(&["helm_x86_64.msi"]),
        None,
        "underscore is not a separator"
    );
    assert_eq!(
        pick(&["helm-x86_64.msi"]).as_deref(),
        Some("helm-x86_64.msi")
    );
    assert_eq!(
        pick(&["helm-amd64.zip", "helm-amd64.msi"]).as_deref(),
        Some("helm-amd64.msi")
    );
    assert_eq!(pick(&["helm-x64.exe.sig", "helm-x64.tar.gz"]), None);
    assert_eq!(pick(&["helm-arm64.exe", "helm-xx64.exe"]), None);
}

// ---------------------------------------------------------------- signatures

#[test]
fn signature_roundtrip_and_tamper() {
    let key = test_key();
    let pubkey = key.verifying_key();
    let digest = sha256_bytes(b"installer bytes");
    let sig = sign_digest(&key, PKG_NAME, &digest);
    verify_signature(&pubkey, PKG_NAME, &digest, &sig).unwrap();

    let other = sha256_bytes(b"evil bytes");
    assert!(matches!(
        verify_signature(&pubkey, PKG_NAME, &other, &sig),
        Err(Error::SignatureMismatch)
    ));
    assert!(
        matches!(
            verify_signature(&pubkey, "Helm-Setup-9.9.9-x64.exe", &digest, &sig),
            Err(Error::SignatureMismatch)
        ),
        "signature is bound to the file name"
    );
    let wrong = SigningKey::from_bytes(&[8u8; 32]).verifying_key();
    assert!(verify_signature(&wrong, PKG_NAME, &digest, &sig).is_err());
}

#[test]
fn sidecar_parsing() {
    let d = sha256_bytes(b"x");
    let hex_d = hex::encode(d);
    assert_eq!(parse_checksum(hex_d.as_bytes()).unwrap(), d);
    assert_eq!(
        parse_checksum(format!("{hex_d}  file.exe\n").as_bytes()).unwrap(),
        d
    );
    assert!(parse_checksum(b"zz").is_err());
    assert!(parse_checksum(b"").is_err());

    let sig = sign_digest(&test_key(), "f", &d);
    assert_eq!(
        parse_signature(&sig.to_bytes()).unwrap(),
        sig,
        "raw 64 bytes"
    );
    let hex_sig = format!("{}\n", hex::encode(sig.to_bytes()));
    assert_eq!(
        parse_signature(hex_sig.as_bytes()).unwrap(),
        sig,
        "hex text"
    );
    assert!(parse_signature(b"short").is_err());
}

#[test]
fn public_key_parsing() {
    let pk = hex::encode(test_key().verifying_key().as_bytes());
    assert!(parse_public_key(&pk).is_ok());
    assert!(parse_public_key("abcd").is_err());
    assert!(
        parse_public_key(&"00".repeat(32)).is_err(),
        "small-order key rejected"
    );
}

// ---------------------------------------------------------------- end to end

struct MemFetcher {
    files: HashMap<String, Vec<u8>>,
    log: RefCell<Vec<String>>,
}

impl Fetcher for MemFetcher {
    fn open(&self, url: &str, _accept: &str) -> Result<Box<dyn Read + Send>, Error> {
        self.log.borrow_mut().push(url.to_string());
        self.files
            .get(url)
            .map(|b| Box::new(Cursor::new(b.clone())) as Box<dyn Read + Send>)
            .ok_or_else(|| Error::Http(format!("404 {url}")))
    }
}

fn package_bytes() -> Vec<u8> {
    (0..4096u32).map(|i| (i * 31 % 251) as u8).collect()
}

/// A fetcher serving the fixture plus a correctly signed 0.3.0 installer.
fn fetcher_with(pkg: Vec<u8>, signed_pkg: &[u8], key: &SigningKey) -> MemFetcher {
    let digest = sha256_bytes(signed_pkg);
    let sig = sign_digest(key, PKG_NAME, &digest);
    let mut files = HashMap::new();
    files.insert(
        format!("{API}/repos/example/helm/releases?per_page=30"),
        FIXTURE.to_vec(),
    );
    files.insert(PKG_URL.to_string(), pkg);
    files.insert(
        format!("{PKG_URL}.sig"),
        hex::encode(sig.to_bytes()).into_bytes(),
    );
    files.insert(
        format!("{PKG_URL}.sha256"),
        format!("{}  {PKG_NAME}\n", hex::encode(digest)).into_bytes(),
    );
    MemFetcher {
        files,
        log: RefCell::new(Vec::new()),
    }
}

fn temp_dir() -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    std::env::temp_dir().join(format!(
        "helm-update-test-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ))
}

fn config(dir: &Path) -> UpdateConfig {
    let mut c = UpdateConfig::new(v("0.2.0"));
    c.repo = "example/helm".into();
    c.api_base = API.into();
    c.download_dir = dir.to_path_buf();
    c
}

#[test]
fn check_download_verify_success() {
    let dir = temp_dir();
    let pkg = package_bytes();
    let key = test_key();
    let up = Updater::with_key(
        config(&dir),
        key.verifying_key(),
        fetcher_with(pkg.clone(), &pkg, &key),
    );

    let c = up.check().unwrap().expect("update available");
    assert_eq!(c.version, v("0.3.0"));
    let path = up.download_and_verify(&c).unwrap();
    assert_eq!(path, dir.join(PKG_NAME));
    assert_eq!(std::fs::read(&path).unwrap(), pkg);
    assert!(!dir.join(format!("{PKG_NAME}.part")).exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn corrupted_download_is_rejected_and_removed() {
    let dir = temp_dir();
    let pkg = package_bytes();
    let mut bad = pkg.clone();
    bad[100] ^= 0xFF;
    let key = test_key();
    let up = Updater::with_key(
        config(&dir),
        key.verifying_key(),
        fetcher_with(bad, &pkg, &key),
    );
    let c = up.check().unwrap().unwrap();
    assert!(matches!(
        up.download_and_verify(&c),
        Err(Error::ChecksumMismatch)
    ));
    assert!(!dir.join(PKG_NAME).exists());
    assert!(!dir.join(format!("{PKG_NAME}.part")).exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn checksum_ok_but_wrong_signer_is_rejected() {
    let dir = temp_dir();
    let pkg = package_bytes();
    let attacker = SigningKey::from_bytes(&[9u8; 32]);
    let up = Updater::with_key(
        config(&dir),
        test_key().verifying_key(),
        fetcher_with(pkg.clone(), &pkg, &attacker),
    );
    let c = up.check().unwrap().unwrap();
    assert!(matches!(
        up.download_and_verify(&c),
        Err(Error::SignatureMismatch)
    ));
    assert!(!dir.join(PKG_NAME).exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn size_mismatch_is_rejected() {
    let dir = temp_dir();
    let pkg = package_bytes();
    let short = pkg[..1000].to_vec();
    let key = test_key();
    let up = Updater::with_key(
        config(&dir),
        key.verifying_key(),
        fetcher_with(short.clone(), &short, &key),
    );
    let c = up.check().unwrap().unwrap();
    assert!(matches!(
        up.download_and_verify(&c),
        Err(Error::SizeMismatch {
            expected: 4096,
            got: 1000
        })
    ));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn refuses_plain_http_and_bad_input() {
    let dir = temp_dir();
    let pkg = package_bytes();
    let key = test_key();
    let up = Updater::with_key(
        config(&dir),
        key.verifying_key(),
        fetcher_with(pkg.clone(), &pkg, &key),
    );
    let mut c = up.check().unwrap().unwrap();
    c.package.browser_download_url = c
        .package
        .browser_download_url
        .replace("https://", "http://");
    assert!(matches!(
        up.download_and_verify(&c),
        Err(Error::InsecureUrl(_))
    ));

    let mut c2 = up.check().unwrap().unwrap();
    c2.package.name = "..\\evil.exe".into();
    assert!(matches!(
        up.download_and_verify(&c2),
        Err(Error::BadAssetName(_))
    ));

    let mut cfg = config(&dir);
    cfg.repo = "no-slash".into();
    assert!(matches!(cfg.releases_url(), Err(Error::BadRepo(_))));
    cfg.repo = "a/../b".into();
    assert!(cfg.releases_url().is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn oversized_release_json_is_rejected() {
    let key = test_key();
    let mut f = fetcher_with(vec![], &[], &key);
    f.files.insert(
        format!("{API}/repos/example/helm/releases?per_page=30"),
        vec![b' '; 5 * 1024 * 1024],
    );
    let up = Updater::with_key(config(&temp_dir()), key.verifying_key(), f);
    assert!(matches!(up.check(), Err(Error::TooLarge(_))));
}
