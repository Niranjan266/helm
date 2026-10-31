//! One-time pairing tokens, lockout after repeated bad tokens, and the `helm://pair` URI.

use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use subtle::ConstantTimeEq;

use crate::util;

pub const TOKEN_LIFETIME: Duration = Duration::from_secs(5 * 60);
pub const MAX_BAD_TOKENS: u32 = 5;
pub const LOCKOUT: Duration = Duration::from_secs(60);

/// A freshly issued pairing link.
#[derive(Clone, Debug)]
pub struct Pairing {
    pub uri: String,
    pub token: String,
    pub expires_at: Instant,
}

#[derive(Debug, PartialEq, Eq)]
pub enum TokenCheck {
    /// Token was right; it has been taken out (call `restore` if pairing fails later).
    /// Carries the token's expiry.
    Ok(Instant),
    Bad,
    Locked,
}

struct ActiveToken {
    token: String,
    expires_at: Instant,
}

pub struct PairingState {
    active: Option<ActiveToken>,
    bad_in_a_row: u32,
    locked_until: Option<Instant>,
    lifetime: Duration,
    lockout: Duration,
}

/// 32 random bytes, base64url without padding (43 characters).
pub fn new_token() -> String {
    util::b64url_encode(&util::random_bytes(32))
}

impl PairingState {
    pub fn new(lifetime: Duration, lockout: Duration) -> PairingState {
        PairingState {
            active: None,
            bad_in_a_row: 0,
            locked_until: None,
            lifetime,
            lockout,
        }
    }

    /// Issue a new token, replacing any previous one (only one is active at a time).
    pub fn issue(&mut self) -> (String, Instant) {
        let token = new_token();
        let expires_at = Instant::now() + self.lifetime;
        self.active = Some(ActiveToken {
            token: token.clone(),
            expires_at,
        });
        (token, expires_at)
    }

    pub fn is_locked(&self) -> bool {
        self.locked_until.is_some_and(|t| Instant::now() < t)
    }

    /// Check a presented token. A correct token is removed immediately so it can't be used
    /// twice, even by two connections racing.
    pub fn check_and_take(&mut self, presented: &str) -> TokenCheck {
        let now = Instant::now();
        if self.is_locked() {
            return TokenCheck::Locked;
        }
        self.locked_until = None;
        if let Some(active) = &self.active
            && active.expires_at <= now
        {
            self.active = None;
        }
        let matched = match &self.active {
            Some(active) if bool::from(active.token.as_bytes().ct_eq(presented.as_bytes())) => {
                Some(active.expires_at)
            }
            _ => None,
        };
        if let Some(expires_at) = matched {
            self.bad_in_a_row = 0;
            self.active = None;
            return TokenCheck::Ok(expires_at);
        }
        self.bad_in_a_row += 1;
        if self.bad_in_a_row >= MAX_BAD_TOKENS {
            self.bad_in_a_row = 0;
            self.locked_until = Some(now + self.lockout);
        }
        TokenCheck::Bad
    }

    /// Put a token back after a pairing attempt that had the right token but failed later
    /// (e.g. bad signature) — unless a newer token was issued meanwhile.
    pub fn restore(&mut self, token: &str, expires_at: Instant) {
        if self.active.is_none() && Instant::now() < expires_at {
            self.active = Some(ActiveToken {
                token: token.to_string(),
                expires_at,
            });
        }
    }
}

/// Build the pairing URI exactly as PROTOCOL.md §4 describes.
pub fn build_uri(
    host_id: &str,
    host_name: &str,
    addresses: &[Ipv4Addr],
    port: u16,
    fingerprint: &str,
    token: &str,
) -> String {
    let addrs = addresses
        .iter()
        .map(|a| a.to_string())
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "helm://pair?v=1&id={}&n={}&a={}&p={}&fp={}&t={}",
        util::url_encode(host_id),
        util::url_encode(host_name),
        addrs,
        port,
        fingerprint,
        token
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_format() {
        let t = new_token();
        assert_eq!(t.len(), 43);
        assert!(
            t.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
    }

    #[test]
    fn uri_format() {
        let uri = build_uri(
            "1b4e28ba-2fa1-11d2-883f-0016d3cca427",
            "My PC",
            &[Ipv4Addr::new(192, 168, 1, 20), Ipv4Addr::new(10, 0, 0, 5)],
            47820,
            &"ab".repeat(32),
            "tok_-123",
        );
        assert_eq!(
            uri,
            format!(
                "helm://pair?v=1&id=1b4e28ba-2fa1-11d2-883f-0016d3cca427&n=My%20PC\
                 &a=192.168.1.20,10.0.0.5&p=47820&fp={}&t=tok_-123",
                "ab".repeat(32)
            )
        );
    }

    #[test]
    fn single_use_and_lockout() {
        let mut s = PairingState::new(TOKEN_LIFETIME, LOCKOUT);
        let (t, _) = s.issue();
        assert!(matches!(s.check_and_take(&t), TokenCheck::Ok(_)));
        assert_eq!(s.check_and_take(&t), TokenCheck::Bad);
        let (t2, _) = s.issue();
        for _ in 0..3 {
            assert_eq!(s.check_and_take("nope"), TokenCheck::Bad);
        }
        // 4 bad in a row so far (one from the reused token); the 5th locks.
        assert_eq!(s.check_and_take("nope"), TokenCheck::Bad);
        assert_eq!(s.check_and_take(&t2), TokenCheck::Locked);
    }

    #[test]
    fn good_token_resets_counter() {
        let mut s = PairingState::new(TOKEN_LIFETIME, LOCKOUT);
        for _ in 0..4 {
            s.check_and_take("x");
        }
        let (t, _) = s.issue();
        assert!(matches!(s.check_and_take(&t), TokenCheck::Ok(_)));
        for _ in 0..4 {
            assert_eq!(s.check_and_take("x"), TokenCheck::Bad);
        }
        assert!(!s.is_locked());
    }

    #[test]
    fn expired_token_is_bad() {
        let mut s = PairingState::new(Duration::from_millis(0), LOCKOUT);
        let (t, _) = s.issue();
        assert_eq!(s.check_and_take(&t), TokenCheck::Bad);
    }

    #[test]
    fn lockout_ends() {
        let mut s = PairingState::new(TOKEN_LIFETIME, Duration::from_millis(30));
        for _ in 0..5 {
            s.check_and_take("x");
        }
        assert!(s.is_locked());
        std::thread::sleep(Duration::from_millis(50));
        let (t, _) = s.issue();
        assert!(matches!(s.check_and_take(&t), TokenCheck::Ok(_)));
    }
}
