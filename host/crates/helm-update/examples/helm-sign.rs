//! helm-sign: release signing tool for Helm maintainers.
//!
//! ```text
//! helm-sign keygen <secret-key-file>          new keypair; prints the public key
//! helm-sign pubkey <secret-key-file>          print the public key again
//! helm-sign sign   <secret-key-file> <file>   writes <file>.sig and <file>.sha256
//! helm-sign verify <public-key-hex>  <file>   checks <file>.sig and <file>.sha256
//! ```
//!
//! The secret key file holds the 32-byte Ed25519 seed as 64 hex chars. Keep
//! it offline; build releases with `HELM_UPDATE_PUBKEY=<public key hex>`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use helm_update::SigningKey;
use helm_update::verify::{
    parse_checksum, parse_public_key, parse_signature, sha256_reader, sign_digest, verify_signature,
};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match refs.as_slice() {
        ["keygen", out] => keygen(Path::new(out)),
        ["pubkey", key] => load_key(Path::new(key)).map(|k| {
            println!("{}", hex::encode(k.verifying_key().as_bytes()));
        }),
        ["sign", key, file] => sign(Path::new(key), Path::new(file)),
        ["verify", pubkey, file] => verify(pubkey, Path::new(file)),
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("helm-sign: {e}");
            ExitCode::FAILURE
        }
    }
}

const USAGE: &str = "usage:
  helm-sign keygen <secret-key-file>
  helm-sign pubkey <secret-key-file>
  helm-sign sign   <secret-key-file> <file>
  helm-sign verify <public-key-hex> <file>";

fn keygen(out: &Path) -> Result<(), String> {
    if out.exists() {
        return Err(format!(
            "{} already exists; refusing to overwrite",
            out.display()
        ));
    }
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).map_err(|e| format!("OS RNG failed: {e}"))?;
    let key = SigningKey::from_bytes(&seed);
    fs::write(out, format!("{}\n", hex::encode(seed))).map_err(|e| e.to_string())?;
    println!("secret key written to {} (keep it offline)", out.display());
    println!(
        "public key: {}",
        hex::encode(key.verifying_key().as_bytes())
    );
    Ok(())
}

fn load_key(path: &Path) -> Result<SigningKey, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut seed = [0u8; 32];
    hex::decode_to_slice(text.trim(), &mut seed)
        .map_err(|_| "secret key file must be 64 hex chars")?;
    Ok(SigningKey::from_bytes(&seed))
}

fn file_name(path: &Path) -> Result<String, String> {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string)
        .ok_or_else(|| format!("bad file name {}", path.display()))
}

fn digest_of(path: &Path) -> Result<[u8; 32], String> {
    let f = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    sha256_reader(f).map_err(|e| e.to_string())
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

fn sign(key: &Path, file: &Path) -> Result<(), String> {
    let key = load_key(key)?;
    let name = file_name(file)?;
    let digest = digest_of(file)?;
    let sig = sign_digest(&key, &name, &digest);
    let sha_path = sidecar(file, ".sha256");
    let sig_path = sidecar(file, ".sig");
    fs::write(&sha_path, format!("{}  {name}\n", hex::encode(digest)))
        .map_err(|e| e.to_string())?;
    fs::write(&sig_path, format!("{}\n", hex::encode(sig.to_bytes())))
        .map_err(|e| e.to_string())?;
    println!("wrote {} and {}", sha_path.display(), sig_path.display());
    Ok(())
}

fn verify(pubkey: &str, file: &Path) -> Result<(), String> {
    let key = parse_public_key(pubkey).map_err(|e| e.to_string())?;
    let name = file_name(file)?;
    let digest = digest_of(file)?;
    let read = |p: PathBuf| fs::read(&p).map_err(|e| format!("{}: {e}", p.display()));
    let expected = parse_checksum(&read(sidecar(file, ".sha256"))?).map_err(|e| e.to_string())?;
    if expected != digest {
        return Err("SHA-256 mismatch".into());
    }
    let sig = parse_signature(&read(sidecar(file, ".sig"))?).map_err(|e| e.to_string())?;
    verify_signature(&key, &name, &digest, &sig).map_err(|e| e.to_string())?;
    println!("OK: {name} checksum and signature verify");
    Ok(())
}
