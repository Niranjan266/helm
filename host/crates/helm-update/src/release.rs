//! GitHub Releases JSON model and update selection.

use serde::Deserialize;

use crate::version::Version;

/// Subset of a GitHub release object (`GET /repos/{owner}/{repo}/releases`).
#[derive(Debug, Clone, Deserialize)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub html_url: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
    #[serde(default)]
    pub size: u64,
}

/// CPU architecture of the wanted build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    X64,
}

impl Arch {
    fn tokens(self) -> &'static [&'static str] {
        match self {
            Arch::X64 => &["x64", "x86_64", "amd64", "win64"],
        }
    }
}

/// Installable file extensions, most preferred first.
const PACKAGE_EXTENSIONS: &[&str] = &["exe", "msi", "zip"];

/// Suffix of the detached Ed25519 signature sidecar (`<asset>.sig`).
pub const SIG_SUFFIX: &str = ".sig";
/// Suffix of the SHA-256 checksum sidecar (`<asset>.sha256`).
pub const SHA256_SUFFIX: &str = ".sha256";

/// A newer release with a verifiable package for this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateCandidate {
    pub version: Version,
    pub tag: String,
    pub notes: Option<String>,
    pub page_url: Option<String>,
    pub package: Asset,
    pub signature: Asset,
    pub checksum: Asset,
}

/// Parses the GitHub releases list.
pub fn parse_releases(json: &[u8]) -> Result<Vec<Release>, serde_json::Error> {
    serde_json::from_slice(json)
}

/// Picks the newest stable release newer than `current` that carries a
/// package for `arch` plus its `.sig` and `.sha256` sidecars.
///
/// Drafts, GitHub pre-releases and tags with a semver pre-release part are
/// skipped; so are tags that are not valid versions.
pub fn select_update(
    releases: &[Release],
    current: &Version,
    arch: Arch,
) -> Option<UpdateCandidate> {
    releases
        .iter()
        .filter(|r| !r.draft && !r.prerelease)
        .filter_map(|r| {
            let v = Version::parse(&r.tag_name).ok()?;
            (v.is_stable() && v > *current).then_some((v, r))
        })
        .filter_map(|(v, r)| candidate_for(r, v, arch))
        .max_by(|a, b| a.version.cmp(&b.version))
}

fn candidate_for(r: &Release, version: Version, arch: Arch) -> Option<UpdateCandidate> {
    let find = |name: &str| r.assets.iter().find(|a| a.name == name).cloned();
    let package = pick_package(&r.assets, arch)?;
    let signature = find(&format!("{}{SIG_SUFFIX}", package.name))?;
    let checksum = find(&format!("{}{SHA256_SUFFIX}", package.name))?;
    Some(UpdateCandidate {
        version,
        tag: r.tag_name.clone(),
        notes: r.body.clone(),
        page_url: r.html_url.clone(),
        package: package.clone(),
        signature,
        checksum,
    })
}

/// Best package asset for `arch`: matching arch token, preferred extension.
pub fn pick_package(assets: &[Asset], arch: Arch) -> Option<&Asset> {
    assets
        .iter()
        .filter_map(|a| {
            let lower = a.name.to_ascii_lowercase();
            let (stem, ext) = lower.rsplit_once('.')?;
            let rank = PACKAGE_EXTENSIONS.iter().position(|e| *e == ext)?;
            let arch_match = stem
                .split(['-', '.', ' '])
                .any(|t| arch.tokens().contains(&t));
            arch_match.then_some((rank, a))
        })
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, a)| a)
}
