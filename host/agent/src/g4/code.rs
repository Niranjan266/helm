//! Short-code pairing, pure parts (hello feature `g4`, `Docs/protocol/pairing-security.md`).
//!
//! The 6-digit code never travels on the wire. Each side commits to every digit, bound to the
//! TLS certificate fingerprint *it sees*, the hello/phone nonces and the phone's identity:
//!
//! ```text
//! h      = SHA-256("helm-g4-code-v1|" host_id "|" nonce_a "|" nonce_p "|" fp "|" device_id "|" public_key)
//! C(R,i) = HMAC-SHA256(key = r(R,i) [32 random bytes], "helm-g4-commit|" R "|" i "|" digit_i "|" hex(h))
//! ```
//!
//! Commitments are exchanged first, then openings `r` alternate one digit at a time
//! (agent opens digit i, phone checks it, phone opens digit i, agent checks it, ...). A man in
//! the middle sees a different fingerprint on each side, so it has to guess every digit before
//! it can learn it: one online guess per code, and a code is used for one attempt only.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use rand::Rng;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// Number of digits in a pairing code.
pub const DIGITS: usize = 6;
/// Length of each commitment opening `r` (bytes).
pub const R_LEN: usize = 32;
/// How long a shown code stays valid.
pub const CODE_LIFETIME: Duration = Duration::from_secs(5 * 60);
/// Failed code attempts (inside [`FAILURE_WINDOW`]) that lock code pairing.
pub const MAX_FAILURES: usize = 3;
pub const FAILURE_WINDOW: Duration = Duration::from_secs(10 * 60);
/// How long code pairing stays locked after [`MAX_FAILURES`].
pub const CODE_LOCKOUT: Duration = Duration::from_secs(10 * 60);

/// Who made a commitment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Agent,
    Phone,
}

impl Role {
    pub fn tag(self) -> &'static str {
        match self {
            Role::Agent => "A",
            Role::Phone => "P",
        }
    }
}

/// HMAC-SHA256 (RFC 2104).
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let inner = Sha256::new().chain_update(ipad).chain_update(msg).finalize();
    Sha256::new()
        .chain_update(opad)
        .chain_update(inner)
        .finalize()
        .into()
}

/// Everything the code authenticates. Strings exactly as they appear on the wire
/// (`fingerprint` = 64 lowercase hex chars of SHA-256 over the leaf certificate DER).
#[derive(Clone, Debug)]
pub struct Transcript<'a> {
    pub host_id: &'a str,
    /// `hello.nonce` (base64 text).
    pub nonce_a: &'a str,
    /// `pair_code.nonce` (base64 text).
    pub nonce_p: &'a str,
    pub fingerprint: &'a str,
    pub device_id: &'a str,
    /// `pair_code.public_key` (base64 SPKI text).
    pub public_key: &'a str,
}

pub fn transcript_hash(t: &Transcript<'_>) -> [u8; 32] {
    let text = format!(
        "helm-g4-code-v1|{}|{}|{}|{}|{}|{}",
        t.host_id,
        t.nonce_a,
        t.nonce_p,
        t.fingerprint.to_ascii_lowercase(),
        t.device_id,
        t.public_key
    );
    Sha256::digest(text.as_bytes()).into()
}

/// Commitment to digit `digit` (0-9) of round `round` (1-based).
pub fn commitment(role: Role, round: usize, digit: u8, h: &[u8; 32], r: &[u8]) -> [u8; 32] {
    let msg = format!(
        "helm-g4-commit|{}|{round}|{digit}|{}",
        role.tag(),
        hex::encode(h)
    );
    hmac_sha256(r, msg.as_bytes())
}

/// Does opening `r` reveal `digit` for commitment `commit`? Constant-time compare.
pub fn opening_matches(
    commit: &[u8],
    role: Role,
    round: usize,
    digit: u8,
    h: &[u8; 32],
    r: &[u8],
) -> bool {
    if r.len() != R_LEN || commit.len() != 32 {
        return false;
    }
    bool::from(commitment(role, round, digit, h, r).ct_eq(commit))
}

/// `"123 456"`, `"123-456"` or `"123456"` → digits. `None` unless exactly 6 digits.
pub fn parse_code(text: &str) -> Option<[u8; DIGITS]> {
    let digits: Vec<u8> = text
        .chars()
        .filter(|c| !matches!(c, ' ' | '-'))
        .map(|c| c.to_digit(10).map(|d| d as u8))
        .collect::<Option<Vec<u8>>>()?;
    digits.try_into().ok()
}

/// A uniformly random 6-digit code (leading zeros allowed).
pub fn random_code() -> String {
    let n: u32 = rand::rngs::OsRng.gen_range(0..1_000_000);
    format!("{n:06}")
}

/// `"123456"` → `"123 456"` (how it is shown).
pub fn display_code(code: &str) -> String {
    if code.len() == DIGITS {
        format!("{} {}", &code[..3], &code[3..])
    } else {
        code.to_string()
    }
}

/// Result of taking the active code for an attempt.
#[derive(Debug, PartialEq, Eq)]
pub enum Take {
    /// The code (it is now used up, whatever happens next).
    Ok([u8; DIGITS]),
    /// No code is shown (never issued, expired or already used).
    NoCode,
    Locked,
}

/// The shown code, its single use, and the failure lockout.
pub struct CodeState {
    active: Option<(String, Instant)>,
    failures: VecDeque<Instant>,
    locked_until: Option<Instant>,
    lifetime: Duration,
    window: Duration,
    lockout: Duration,
}

impl Default for CodeState {
    fn default() -> Self {
        CodeState::new(CODE_LIFETIME, FAILURE_WINDOW, CODE_LOCKOUT)
    }
}

impl CodeState {
    pub fn new(lifetime: Duration, window: Duration, lockout: Duration) -> CodeState {
        CodeState {
            active: None,
            failures: VecDeque::new(),
            locked_until: None,
            lifetime,
            window,
            lockout,
        }
    }

    /// Remaining lockout, if locked.
    pub fn locked_for(&self) -> Option<Duration> {
        let until = self.locked_until?;
        let now = Instant::now();
        (now < until).then(|| until - now)
    }

    /// Show a new code (replaces the previous one). Refused while locked.
    pub fn issue(&mut self) -> Result<(String, Instant), Duration> {
        if let Some(left) = self.locked_for() {
            return Err(left);
        }
        let code = random_code();
        let expires_at = Instant::now() + self.lifetime;
        self.active = Some((code.clone(), expires_at));
        Ok((code, expires_at))
    }

    /// The active code, if any and not expired (without using it).
    pub fn current(&self) -> Option<(String, Instant)> {
        self.active
            .as_ref()
            .filter(|(_, exp)| Instant::now() < *exp)
            .cloned()
    }

    /// Take the code for one attempt. It can never be used again.
    pub fn take(&mut self) -> Take {
        if self.locked_for().is_some() {
            return Take::Locked;
        }
        match self.active.take() {
            Some((code, exp)) if Instant::now() < exp => match parse_code(&code) {
                Some(d) => Take::Ok(d),
                None => Take::NoCode,
            },
            _ => Take::NoCode,
        }
    }

    /// Forget the shown code without an attempt.
    pub fn clear(&mut self) {
        self.active = None;
    }

    /// Count a failed attempt. Returns true when this one started a lockout.
    pub fn record_failure(&mut self) -> bool {
        let now = Instant::now();
        while self
            .failures
            .front()
            .is_some_and(|t| now.duration_since(*t) > self.window)
        {
            self.failures.pop_front();
        }
        self.failures.push_back(now);
        if self.failures.len() >= MAX_FAILURES {
            self.failures.clear();
            self.locked_until = Some(now + self.lockout);
            self.active = None;
            return true;
        }
        false
    }

    pub fn record_success(&mut self) {
        self.failures.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shared with the Kotlin side (`CodePairingTest.interopVector`): same inputs, same bytes.
    pub(crate) fn vector() -> ([u8; 32], String) {
        let t = Transcript {
            host_id: "1b4e28ba-2fa1-11d2-883f-0016d3cca427",
            nonce_a: "bm9uY2UtYWdlbnQtMDEyMzQ1Njc4OWFiY2RlZjAxMjM=",
            nonce_p: "bm9uY2UtcGhvbmUtMDEyMzQ1Njc4OWFiY2RlZjAxMjM=",
            fingerprint: &"ab".repeat(32),
            device_id: "9f8e7d6c-0000-4000-8000-123456789abc",
            public_key: "MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEexample",
        };
        (transcript_hash(&t), t.fingerprint.to_string())
    }

    #[test]
    fn hmac_rfc4231_case_2() {
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            hex::encode(mac),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        // Key longer than the block (case 6).
        let mac = hmac_sha256(
            &[0xaa; 131],
            b"Test Using Larger Than Block-Size Key - Hash Key First",
        );
        assert_eq!(
            hex::encode(mac),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn interop_vector() {
        let (h, _) = vector();
        assert_eq!(
            hex::encode(h),
            "a16fbf9b52ce476d4c6c20ab349e56b7d9cd8ec271c959d63cb0a842e71aea4c"
        );
        let r_a1 = [0x11u8; 32];
        let r_p6 = [0x26u8; 32];
        assert_eq!(
            hex::encode(commitment(Role::Agent, 1, 4, &h, &r_a1)),
            "e7cd6b49d48eb85513216cdb6904f2ee3279ef9dec51b8629327f57d3b2fcaf7"
        );
        assert_eq!(
            hex::encode(commitment(Role::Phone, 6, 9, &h, &r_p6)),
            "c114c93a81becd531ae0455c5a7255f8ccb62e67e837ff4fad8875eb0faf5b80"
        );
    }

    #[test]
    fn openings_bind_digit_role_round_and_fingerprint() {
        let (h, _) = vector();
        let r = [7u8; 32];
        let c = commitment(Role::Agent, 2, 5, &h, &r);
        assert!(opening_matches(&c, Role::Agent, 2, 5, &h, &r));
        assert!(!opening_matches(&c, Role::Agent, 2, 6, &h, &r));
        assert!(!opening_matches(&c, Role::Phone, 2, 5, &h, &r));
        assert!(!opening_matches(&c, Role::Agent, 3, 5, &h, &r));
        assert!(!opening_matches(&c, Role::Agent, 2, 5, &h, &[8u8; 32]));
        assert!(!opening_matches(&c, Role::Agent, 2, 5, &h, &[7u8; 31]));
        // A different certificate (man in the middle) gives a different transcript.
        let other = transcript_hash(&Transcript {
            host_id: "1b4e28ba-2fa1-11d2-883f-0016d3cca427",
            nonce_a: "bm9uY2UtYWdlbnQtMDEyMzQ1Njc4OWFiY2RlZjAxMjM=",
            nonce_p: "bm9uY2UtcGhvbmUtMDEyMzQ1Njc4OWFiY2RlZjAxMjM=",
            fingerprint: &"cd".repeat(32),
            device_id: "9f8e7d6c-0000-4000-8000-123456789abc",
            public_key: "MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEexample",
        });
        assert!(!opening_matches(&c, Role::Agent, 2, 5, &other, &r));
    }

    #[test]
    fn code_parsing_and_display() {
        assert_eq!(parse_code("012 345"), Some([0, 1, 2, 3, 4, 5]));
        assert_eq!(parse_code("012-345"), Some([0, 1, 2, 3, 4, 5]));
        assert_eq!(parse_code("12345"), None);
        assert_eq!(parse_code("1234567"), None);
        assert_eq!(parse_code("12a456"), None);
        assert_eq!(display_code("012345"), "012 345");
        for _ in 0..50 {
            let c = random_code();
            assert_eq!(c.len(), 6);
            assert!(parse_code(&c).is_some());
        }
    }

    #[test]
    fn code_is_single_use() {
        let mut s = CodeState::default();
        assert_eq!(s.take(), Take::NoCode);
        let (code, _) = s.issue().unwrap();
        assert_eq!(s.current().unwrap().0, code);
        assert_eq!(s.take(), Take::Ok(parse_code(&code).unwrap()));
        assert_eq!(s.take(), Take::NoCode);
        assert!(s.current().is_none());
    }

    #[test]
    fn expired_code_is_gone() {
        let mut s = CodeState::new(Duration::ZERO, FAILURE_WINDOW, CODE_LOCKOUT);
        s.issue().unwrap();
        assert_eq!(s.take(), Take::NoCode);
    }

    #[test]
    fn lockout_after_three_failures_then_ends() {
        let mut s = CodeState::new(CODE_LIFETIME, FAILURE_WINDOW, Duration::from_millis(40));
        assert!(!s.record_failure());
        assert!(!s.record_failure());
        s.issue().unwrap();
        assert!(s.record_failure());
        assert!(s.locked_for().is_some());
        assert_eq!(s.take(), Take::Locked);
        assert!(s.issue().is_err());
        std::thread::sleep(Duration::from_millis(60));
        assert!(s.locked_for().is_none());
        let (code, _) = s.issue().unwrap();
        assert_eq!(s.take(), Take::Ok(parse_code(&code).unwrap()));
    }

    #[test]
    fn success_resets_failures_and_old_failures_expire() {
        let mut s = CodeState::default();
        s.record_failure();
        s.record_failure();
        s.record_success();
        assert!(!s.record_failure());
        assert!(!s.record_failure());
        let mut s = CodeState::new(CODE_LIFETIME, Duration::from_millis(10), CODE_LOCKOUT);
        s.record_failure();
        s.record_failure();
        std::thread::sleep(Duration::from_millis(30));
        assert!(!s.record_failure());
    }
}
