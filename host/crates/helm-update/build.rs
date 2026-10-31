// Rebuild when the compiled-in release key or default repository changes.
fn main() {
    println!("cargo:rerun-if-env-changed=HELM_UPDATE_PUBKEY");
    println!("cargo:rerun-if-env-changed=HELM_UPDATE_REPO");
}
