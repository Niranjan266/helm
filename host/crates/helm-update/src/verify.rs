//! SHA-256 checksums and Ed25519 release signatures.
//!
//! The signed message binds the file name and its digest:
//!
//! ```text
//! "helm-update-v1\n" || <asset file name> || "\n" || SHA-256(file)   (32 raw bytes)
//! ```
//!
//! Binding the name stops a validly signed file from being re-served under a
//! different (for example, newer-looking) asset name.

use std::io::Read;

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};

use crate::Error;

const CONTEXT: &[u8] = b"helm-update-v1\n";

/// Builds the exact bytes that are signed for `file_name` with `digest`.
pub fn signed_message(file_name: &str, digest: &[u8; 32]) -> Vec<u8> {
    let mut m = Vec::with_capacity(CONTEXT.len() + file_name.len() + 1 + 32);
    m.extend_from_slice(CONTEXT);
    m.extend_from_slice(file_name.as_bytes());
    m.push(b'\n');
    m.extend_from_slice(digest);
    m
}

/// SHA-256 of everything `reader` yields.
pub fn sha256_reader(mut reader: impl Read) -> std::io::Result<[u8; 32]> {
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            return Ok(h.finalize().into());
        }
        h.update(&buf[..n]);
    }
}

pub fn sha256_bytes(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// Parses a `.sha256` sidecar: 64 hex chars, optionally followed by
/// whitespace and a file name (`sha256sum` format).
pub fn parse_checksum(text: &[u8]) -> Result<[u8; 32], Error> {
    let s = std::str::from_utf8(text).map_err(|_| Error::BadChecksumFile)?;
    let hex_part = s.split_whitespace().next().ok_or(Error::BadChecksumFile)?;
    let mut out = [0u8; 32];
    hex::decode_to_slice(hex_part, &mut out).map_err(|_| Error::BadChecksumFile)?;
    Ok(out)
}

/// Parses a `.sig` sidecar: either 64 raw bytes or 128 hex chars.
pub fn parse_signature(data: &[u8]) -> Result<Signature, Error> {
    let bytes: [u8; 64] = if data.len() == 64 {
        data.try_into().map_err(|_| Error::BadSignatureFile)?
    } else {
        let s = std::str::from_utf8(data).map_err(|_| Error::BadSignatureFile)?;
        let mut out = [0u8; 64];
        hex::decode_to_slice(s.trim(), &mut out).map_err(|_| Error::BadSignatureFile)?;
        out
    };
    Ok(Signature::from_bytes(&bytes))
}

/// Parses a 32-byte Ed25519 public key from hex, rejecting weak keys.
pub fn parse_public_key(hex_text: &str) -> Result<VerifyingKey, Error> {
    let mut bytes = [0u8; 32];
    hex::decode_to_slice(hex_text.trim(), &mut bytes).map_err(|_| Error::BadPublicKey)?;
    let key = VerifyingKey::from_bytes(&bytes).map_err(|_| Error::BadPublicKey)?;
    if key.is_weak() {
        return Err(Error::BadPublicKey);
    }
    Ok(key)
}

/// Verifies `sig` over (`file_name`, `digest`) with strict Ed25519 rules.
pub fn verify_signature(
    key: &VerifyingKey,
    file_name: &str,
    digest: &[u8; 32],
    sig: &Signature,
) -> Result<(), Error> {
    key.verify_strict(&signed_message(file_name, digest), sig)
        .map_err(|_| Error::SignatureMismatch)
}

/// Signs (`file_name`, `digest`). Used by the `helm-sign` tool and tests.
pub fn sign_digest(key: &SigningKey, file_name: &str, digest: &[u8; 32]) -> Signature {
    key.sign(&signed_message(file_name, digest))
}
