# helm-update

Update check for the Helm PC agent. MIT licensed.

- Reads the GitHub Releases list of a configurable `owner/repo`
  (`UpdateConfig::repo`, default `DEFAULT_REPO`, overridable at build time
  with `HELM_UPDATE_REPO`).
- Picks the newest **stable** release newer than the running version
  (drafts, GitHub pre-releases and semver pre-release tags are skipped) that
  has an x64 package (name token `x64`/`x86_64`/`amd64`/`win64`; our
  `Helm-windows-x64-setup.exe` first, then other `setup`/`installer` names, then
  a bare program such as `Helm-windows-x64.exe`; `.exe` > `.msi` > `.zip`) plus
  `<asset>.sig` and `<asset>.sha256` sidecars.
- Downloads to a temp folder over HTTPS only, checks size, SHA-256 and an
  Ed25519 signature (`verify_strict`). Bad files are deleted.

The signed message is `"helm-update-v1\n" || file name || "\n" || SHA-256(file)`,
so a signature cannot be reused for a different asset name.

## Release key

The public key (64 hex chars) is compiled in from the committed
[`keys/update-ed25519.pub`](../../../keys/update-ed25519.pub), which the Android
build reads too. `HELM_UPDATE_PUBKEY` overrides it for a fork; set it empty to
build with updates off. With no valid key, `Updater::new` returns
`Error::NoPublicKey` and updates are refused.

The private key lives outside the repository, in `%USERPROFILE%\.helm\`.
`scripts/release.ps1` signs every asset with it:

```sh
cargo run --example helm-sign -- keygen <secret-key-file>   # prints the public key; keep the file offline
cargo run --example helm-sign -- sign <secret-key-file> Helm-windows-x64-setup.exe
cargo run --example helm-sign -- verify <pubkey-hex> Helm-windows-x64-setup.exe
```

Upload the package, `.sig` and `.sha256` to the GitHub release.

## Usage

```rust
let cfg = helm_update::UpdateConfig::new(helm_update::Version::new(0, 1, 0));
let up = helm_update::Updater::new(cfg, helm_update::HttpFetcher::new("helm-agent/0.1.0"))?;
if let Some(c) = up.check()? {
    let installer = up.download_and_verify(&c)?;
}
```

Tests (`cargo test`) use a fixture release JSON, an in-memory `Fetcher` and a
fixed test key; they never touch the network. Build with
`--no-default-features` to drop the HTTP client.
