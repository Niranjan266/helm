// Resolves the release public key compiled into the updater:
// 1. HELM_UPDATE_PUBKEY if set (a fork's own key; empty = build with updates disabled);
// 2. otherwise the committed keys/update-ed25519.pub at the repository root.
// A build with neither has no key, and updates are refused at run time.
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-env-changed=HELM_UPDATE_PUBKEY");
    println!("cargo:rerun-if-env-changed=HELM_UPDATE_REPO");
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let file = Path::new(&manifest).join("../../../keys/update-ed25519.pub");
    println!("cargo:rerun-if-changed={}", file.display());

    let key = std::env::var("HELM_UPDATE_PUBKEY")
        .ok()
        .or_else(|| std::fs::read_to_string(&file).ok())
        .unwrap_or_default();
    println!("cargo:rustc-env=HELM_UPDATE_RESOLVED_PUBKEY={}", key.trim());
}
